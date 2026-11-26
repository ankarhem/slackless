//! Cursor pagination (`cursor` param in, `response_metadata.next_cursor` out).

use std::collections::HashSet;
use std::future::Future;

use futures::stream::{self, Stream, TryStreamExt};

use crate::ApiError;

/// One page of a cursor-paginated method.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Cursor for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Walks every page of a cursor-paginated method, yielding items in order.
///
/// `fetch` gets `None` for the first page, then each page's `next_cursor`. Stops when a page has
/// no cursor, or when Slack hands back a cursor already followed (a loop guard that also catches
/// cycles such as `a -> b -> a`).
///
/// ```no_run
/// # use futures::TryStreamExt;
/// # use slackless_api::{ApiError, SlackApi, paginate};
/// # async fn example(api: &SlackApi) -> Result<(), ApiError> {
/// let all: Vec<_> = paginate(|c| api.users_list(Some(200), c)).try_collect().await?;
/// # let _ = all;
/// # Ok(())
/// # }
/// ```
pub fn paginate<T, F, Fut>(fetch: F) -> impl Stream<Item = Result<T, ApiError>>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>, ApiError>>,
{
    // State: Some(cursor) while there is a page to fetch (None when done), and every cursor
    // followed so far.
    stream::try_unfold(
        (fetch, Some(None::<String>), HashSet::new()),
        |(mut fetch, state, mut followed)| async move {
            let Some(cursor) = state else {
                return Ok::<_, ApiError>(None);
            };
            let page = fetch(cursor).await?;
            // `insert` is false for a cursor already followed: a cycle ends the walk.
            let next = page
                .next_cursor
                .filter(|c| !c.is_empty() && followed.insert(c.clone()))
                .map(Some);
            Ok(Some((page.items, (fetch, next, followed))))
        },
    )
    .map_ok(|items| stream::iter(items.into_iter().map(Ok)))
    .try_flatten()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn paginate_stops_when_slack_hands_back_a_cursor_already_followed() {
        // (cursor asked for, items, next_cursor) per page: a cursor repeated right away
        // (a -> a) or after a detour (a -> b -> a) ends the walk with every page read once.
        let self_loop: &[(Option<&str>, &[u32], &str)] =
            &[(None, &[1, 2], "a"), (Some("a"), &[3], "a")];
        let two_cycle: &[(Option<&str>, &[u32], &str)] = &[
            (None, &[1], "a"),
            (Some("a"), &[2], "b"),
            (Some("b"), &[3], "a"),
        ];
        for pages in [self_loop, two_cycle] {
            let fetches = AtomicUsize::new(0);
            let items: Vec<u32> = paginate(|cursor| {
                assert!(
                    fetches.fetch_add(1, Ordering::SeqCst) < pages.len(),
                    "a page was fetched twice: {pages:?}"
                );
                let (_, items, next) = pages
                    .iter()
                    .find(|(asked, ..)| *asked == cursor.as_deref())
                    .copied()
                    .unwrap_or_else(|| panic!("unexpected cursor {cursor:?}"));
                async move {
                    Ok(Page {
                        items: items.to_vec(),
                        next_cursor: Some(next.to_owned()),
                    })
                }
            })
            .try_collect()
            .await
            .unwrap();
            assert_eq!(items, [1, 2, 3]);
        }
    }

    #[tokio::test]
    async fn paginate_continues_past_empty_page_with_cursor() {
        let items: Vec<u32> = paginate(|cursor| async move {
            match cursor.as_deref() {
                None => Ok(Page {
                    items: vec![],
                    next_cursor: Some("p2".to_owned()),
                }),
                Some("p2") => Ok(Page {
                    items: vec![10, 20],
                    next_cursor: None,
                }),
                other => panic!("unexpected cursor: {other:?}"),
            }
        })
        .try_collect()
        .await
        .unwrap();

        assert_eq!(items, vec![10, 20]);
    }

    #[tokio::test]
    async fn paginate_propagates_mid_stream_errors() {
        let res: Result<Vec<u32>, ApiError> = paginate(|cursor| async move {
            match cursor.as_deref() {
                None => Ok(Page {
                    items: vec![1],
                    next_cursor: Some("p2".to_owned()),
                }),
                Some("p2") => Err(ApiError::Slack("internal_error".to_owned())),
                other => panic!("unexpected cursor: {other:?}"),
            }
        })
        .try_collect()
        .await;

        assert!(matches!(res, Err(ApiError::Slack(ref e)) if e == "internal_error"));
    }
}
