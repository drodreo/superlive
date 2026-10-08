//! Shell-level auth: Bearer verification and token generation for the /api
//! positive rule.
//!
//! Security model (architecture.md "Security"): the token is the
//! platform-invariant first wall, "assume the port being reachable is also
//! safe" — no token always means 401. The shell holds a single shell-level
//! token (randomly generated at each startup); the webview side obtains it
//! via a tauri command (wired in the second stage of the injection
//! mechanism). Constant-time comparison guards against timing side
//! channels, the same scheme as the slsdk-rs component side.

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Generate a 64-character hex random token (32 bytes of entropy).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time byte comparison (guards against timing side channels).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

/// Bearer extraction and constant-time comparison. The scheme is
/// case-insensitive per RFC 7235.
fn bearer_matches(header_value: &str, expected: &str) -> bool {
    let Some((scheme, token)) = header_value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && constant_time_eq(token.trim_start().as_bytes(), expected.as_bytes())
}

/// Query-form token matching (the ws compromise channel, architecture.md
/// contract: a browser-native WebSocket cannot carry custom headers, so the
/// token travels via `?token=`).
/// The token is hex64 (produced by generate_token); no URL decoding needed,
/// compared verbatim.
fn query_token_matches(query: &str, expected: &str) -> bool {
    query.split('&').any(|pair| match pair.split_once('=') {
        Some(("token", value)) => constant_time_eq(value.as_bytes(), expected.as_bytes()),
        _ => false,
    })
}

/// Auth middleware for /api routes: validates the shell-level token, no
/// match always means 401 + WWW-Authenticate.
/// /_shell and /comps do not mount this middleware (auth positive rule: the
/// first URL segment is the traffic-type discriminator).
pub async fn require_bearer(
    State(expected): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Response {
    let authorized = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| bearer_matches(value, &expected))
        // With no Authorization header, fall back to the ?token= query (ws
        // compromise channel, see query_token_matches)
        || req
            .uri()
            .query()
            .is_some_and(|q| query_token_matches(q, &expected));
    if authorized {
        next.run(req).await
    } else {
        // RFC 7235: a 401 should carry WWW-Authenticate describing the challenge
        let mut res = StatusCode::UNAUTHORIZED.into_response();
        res.headers_mut()
            .insert("WWW-Authenticate", HeaderValue::from_static("Bearer"));
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_router(token: Arc<String>) -> Router {
        Router::new()
            .route("/api/ping", get(|| async { "pong" }))
            .layer(axum::middleware::from_fn_with_state(token, require_bearer))
    }

    async fn body_text(res: Response) -> String {
        let bytes = BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn missing_token_rejected() {
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            res.headers().get("WWW-Authenticate").unwrap(),
            HeaderValue::from_static("Bearer")
        );
    }

    #[tokio::test]
    async fn wrong_token_rejected() {
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping")
            .header(AUTHORIZATION, "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_token_passes() {
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping")
            .header(AUTHORIZATION, "Bearer tok")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_text(res).await, "pong");
    }

    #[tokio::test]
    async fn query_token_passes_without_header() {
        // The ws compromise channel (architecture.md): ?token= backstop when
        // there is no header
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping?foo=1&token=tok&bar=2")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_query_token_rejected() {
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping?token=wrong")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn lowercase_scheme_passes() {
        // RFC 7235: the auth scheme is case-insensitive
        let router = test_router(Arc::new("tok".to_string()));
        let req = Request::builder()
            .uri("/api/ping")
            .header(AUTHORIZATION, "bearer tok")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn malformed_header_rejected() {
        let router = test_router(Arc::new("tok".to_string()));
        // Missing space separator: the split_once(' ') failure path
        let req = Request::builder()
            .uri("/api/ping")
            .header(AUTHORIZATION, "Bearertok")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn generated_token_is_hex64_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(
            a, b,
            "two generated tokens should never match (random entropy)"
        );
    }
}
