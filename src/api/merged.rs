//! Writes addressed to a merged work.
//!
//! A merge leaves the merged work as a tombstone whose id, read, answers
//! with the work it was merged into (`service::load`). A write under that id
//! would land on the tombstone, where nothing reads it again — or, were it
//! followed, on the survivor, which a client deleting "the duplicate" by its
//! old id does not mean. So it is refused: `409` with the code
//! `work_merged`, the survivor's id in the body and its address in
//! `Location`, for the caller to write there knowingly. Undoing the merge is
//! the one write the tombstone's id is for.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{db::repo::alias, state::AppState};

/// The code a refused write carries.
pub const CODE: &str = "work_merged";

/// The work a request writes under `/api/v1/items/{id}…`, when a merge may
/// have folded it: none for a read, for another route, for an address that
/// is not a work's id, or for the undo of a merge.
fn addressed<'a>(method: &Method, path: &'a str) -> Option<&'a str> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let rest = path.strip_prefix("/api/v1/items/")?;
    let (id, tail) = match rest.split_once('/') {
        Some((id, tail)) => (id, Some(tail)),
        None => (rest, None),
    };
    if tail == Some("unmerge") || uuid::Uuid::try_parse(id).is_err() {
        return None;
    }
    Some(id)
}

/// Let a write through unless it is addressed to a merged work.
pub async fn refuse_writes(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(id) = addressed(request.method(), request.uri().path()) else {
        return next.run(request).await;
    };
    match alias::for_address(&state.db, id).await {
        Ok(Some(survivor)) => refused(id, &survivor),
        Ok(None) => next.run(request).await,
        // The write meets the same database next, and says what it says.
        Err(err) => {
            tracing::warn!(error = %err, id, "could not tell whether a work was merged");
            next.run(request).await
        }
    }
}

fn refused(id: &str, survivor: &str) -> Response {
    let mut response = (
        StatusCode::CONFLICT,
        Json(json!({
            "error": CODE,
            "message": format!(
                "{id} was merged into {survivor}: write to {survivor}, or undo the merge"
            ),
            "survivor": survivor,
        })),
    )
        .into_response();
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/items/{survivor}")) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b1e4a52-4f3c-4f43-9d55-3a8e0f6c2b11";

    #[test]
    fn only_a_write_under_a_works_id_is_looked_at() {
        let at = |method: Method, path: &str| addressed(&method, path).map(str::to_string);
        assert_eq!(
            at(Method::PUT, &format!("/api/v1/items/{ID}")),
            Some(ID.into())
        );
        assert_eq!(
            at(Method::POST, &format!("/api/v1/items/{ID}/locks")),
            Some(ID.into())
        );
        assert_eq!(
            at(Method::DELETE, &format!("/api/v1/items/{ID}/seasons/1")),
            Some(ID.into())
        );
        // Reads follow the merge on their own.
        assert_eq!(at(Method::GET, &format!("/api/v1/items/{ID}")), None);
        // The undo is addressed to the tombstone on purpose.
        assert_eq!(
            at(Method::POST, &format!("/api/v1/items/{ID}/unmerge")),
            None
        );
        // A new work, an address that is not an id, another route.
        assert_eq!(at(Method::POST, "/api/v1/items"), None);
        assert_eq!(at(Method::POST, "/api/v1/items/heat-1995/locks"), None);
        assert_eq!(at(Method::POST, &format!("/api/v1/people/{ID}")), None);
    }
}
