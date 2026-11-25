//! Cursor pagination (`cursor` param in, `response_metadata.next_cursor` out).

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
/// no cursor, or when Slack hands back the cursor it was just given (a loop guard).
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
    // State: Some(cursor) while there is a page to fetch; None when done.
    stream::try_unfold(
        (fetch, Some(None::<String>)),
        |(mut fetch, state)| async move {
            let Some(cursor) = state else {
                return Ok::<_, ApiError>(None);
            };
            let page = fetch(cursor.clone()).await?;
            let next = page
                .next_cursor
                .filter(|c| Some(c) != cursor.as_ref())
                .map(Some);
            Ok(Some((page.items, (fetch, next))))
        },
    )
    .map_ok(|items| stream::iter(items.into_iter().map(Ok)))
    .try_flatten()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn paginate_stops_on_repeated_cursor_loop_guard() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_ref = Arc::clone(&calls);
        let items: Vec<u32> = paginate(move |cursor| {
            let calls = Arc::clone(&calls_ref);
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                match cursor.as_deref() {
                    None => Ok(Page {
                        items: vec![1, 2],
                        next_cursor: Some("stuck".to_owned()),
                    }),
                    Some("stuck") if n < 5 => Ok(Page {
                        items: vec![3],
                        // Returns the exact cursor it was just given: loop guard must terminate.
                        next_cursor: Some("stuck".to_owned()),
                    }),
                    _ => unreachable!("loop guard should have stopped after repeated cursor"),
                }
            }
        })
        .try_collect()
        .await
        .unwrap();

        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
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
