//! Token custody: everything except `/healthz` requires `Authorization: Bearer`; `/events` also accepts `?token=`.

use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.cfg.token.as_deref() else { return next.run(req).await };
    let path = req.uri().path();
    if path == "/healthz" {
        return next.run(req).await;
    }
    let bearer =
        req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let query = if path == "/events" { query_token(req.uri().query()) } else { None };
    if bearer.into_iter().chain(query).any(|t| same(t, expected)) {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], "token requerido").into_response()
    }
}

fn query_token(query: Option<&str>) -> Option<&str> {
    query?.split('&').find_map(|kv| kv.strip_prefix("token="))
}

/// Comparison that does not stop at the first mismatch.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_token_is_extracted() {
        assert_eq!(query_token(Some("a=1&token=xyz")), Some("xyz"));
        assert_eq!(query_token(Some("a=1")), None);
        assert_eq!(query_token(None), None);
    }

    #[test]
    fn same_compares_whole_strings() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
