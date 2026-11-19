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
/// ```ignore
/// let all: Vec<_> = paginate(|c| api.users_list(Some(200), c)).try_collect().await?;
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
