//! Embedded HTTP service assembly and startup for the shell.
//!
//! Route partitioning (architecture.md "Plane A contract", first URL segment
//! partitioning):
//! - `/_shell/*` shell management (no token)
//! - `/api/{ns}/{rest}` component API gateway (shell-level token validated,
//!   auth positive rule)
//! - `/comps/{id}/*` component UI static (no token: scripts/dynamic imports
//!   cannot carry custom headers)

use crate::auth;
use crate::config::SHELL_PORT;
use crate::gateway;
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;

/// Assemble the shell router (pub(crate) so tests can inject a mock registry).
pub fn build_router(state: Arc<AppState>) -> Router {
    // Unified State generic across the whole chain: merge requires both sub
    // routers to share the same S (routes with no State needs are also
    // registered on Router<Arc<AppState>>, and the final with_state converges
    // to Router<()>)
    let shell_api = Router::<Arc<AppState>>::new()
        .route("/_shell/health", get(health))
        .route("/_shell/registry", get(registry_view));

    // Auth positive rule: only /api mounts auth (an independent sub router
    // merged in, leaving /_shell and /comps untouched)
    let component_api = Router::<Arc<AppState>>::new()
        .route("/api/{*rest}", any(gateway::gateway_handler))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state.shell_token),
            auth::require_bearer,
        ));

    Router::<Arc<AppState>>::new()
        .merge(shell_api)
        // Static UI: only exposes the ui/ subdirectory inside the component
        // package (binaries/resources/data files have no URL mapping,
        // architecture.md "Install and lifecycle" contract)
        .route("/comps/{*rest}", get(comps_static))
        .merge(component_api)
        .with_state(state)
        // CORS covers every partition: the shell webview's origin
        // (tauri://localhost / http://tauri.localhost / dev's
        // http://localhost:5173) is cross-origin to 39876, and both the
        // component ESM dynamic loading and component API fetch are
        // cross-origin — CORS headers are needed to pass browser checks.
        .layer(axum::middleware::from_fn(cors_middleware))
}

/// CORS middleware.
///
/// Security note (architecture.md "Security"): the token is the
/// platform-invariant first wall (no token always means 401); CORS was never
/// this service's security dependency — following the "assume the port being
/// reachable is also safe" principle, allowing any Origin is the direct
/// application of that principle (a malicious web page can reach the port
/// but cannot carry the shell-injected token, so component data stays out of
/// reach). CORS here only solves one engineering problem: letting the
/// cross-origin component module script loading and API fetch inside the
/// webview pass the browser's same-origin policy checks.
///
/// Policy: Origin reflection (rather than `*`, compatible with possible
/// future credentialed extensions); preflight OPTIONS short-circuits the
/// response (never enters the gateway/static handling); request headers
/// reflect what the preflight declares, with `*` as the fallback when
/// undeclared (this service has no cookie credentials, so `*` is valid).
async fn cors_middleware(req: Request, next: Next) -> Response {
    let origin = req.headers().get(header::ORIGIN).cloned();
    let requested_headers = req
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .cloned();
    let preflight = req.method() == Method::OPTIONS;

    let mut res = if preflight {
        let mut r = Response::new(axum::body::Body::empty());
        *r.status_mut() = StatusCode::NO_CONTENT;
        r
    } else {
        next.run(req).await
    };

    if let Some(origin) = origin {
        res.headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    {
        let headers = res.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, DELETE, HEAD, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            requested_headers.unwrap_or_else(|| HeaderValue::from_static("*")),
        );
        headers.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("*"),
        );
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    res
}

/// `GET /comps/{id}/{rel}`: component UI static files.
/// URL space maps to `components/{id}/ui/{rel}` — only the ui/ subdirectory
/// inside the package is exposed; the rest of the install directory
/// (binaries, resources) never enters the URL space.
async fn comps_static(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(rest): axum::extract::Path<String>,
) -> Response {
    use axum::http::header;
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    let Some((id, rel)) = rest.split_once('/') else {
        return not_found();
    };
    let base = state.layout.component_dir(id).join("ui");
    let path = base.join(rel);
    // Traversal guard: after canonicalization the path must still land
    // inside that component's ui/ directory
    let (canonical, canonical_base) = match (
        tokio::fs::canonicalize(&path).await,
        tokio::fs::canonicalize(&base).await,
    ) {
        (Ok(p), Ok(b)) => (p, b),
        _ => return not_found(),
    };
    if !canonical.starts_with(&canonical_base) {
        return not_found();
    }
    match tokio::fs::read(&canonical).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, static_mime(&canonical))], bytes).into_response(),
        Err(_) => not_found(),
    }
}

/// Static resource Content-Type (common component UI types; unrecognized
/// falls to octet-stream).
fn static_mime(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "html" | "htm" => "text/html",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `GET /_shell/health`: the shell's own health (no token).
async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// `GET /_shell/registry`: overview of currently running components (no token).
async fn registry_view(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "components": state.registry.list().await }))
}

/// Start the embedded service (spawned from tauri setup). A bind failure is
/// only logged — the shell UI is not blocked by it,
/// but the component API/static services will be unavailable (a port
/// conflict is an environment-level failure requiring manual intervention).
pub async fn run(state: AppState) -> std::io::Result<()> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], SHELL_PORT));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("shell-embedded service started: http://{addr}");
    axum::serve(listener, build_router(Arc::new(state))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RunningComponent;
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{header::AUTHORIZATION, StatusCode};
    use tokio::sync::watch;
    use tower::ServiceExt;

    fn manifest() -> slsdk_rs::ComponentManifest {
        serde_json::from_str(
            r#"{
            "id": "mockapp",
            "version": "0.1.0",
            "min_shell_version": "0.1.0",
            "namespace": "mockns",
            "display_name": "Mock",
            "ui": { "ui_type": "module", "entry": "assets/main.js" },
            "menu": null,
            "routes": [],
            "launch": { "command": "mock-bin", "args": [] },
            "files": [ { "path": "mock-bin", "sha256": "aa" } ]
        }"#,
        )
        .unwrap()
    }

    async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
        axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn shell_health_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(Arc::new(AppState::new(dir.path()).unwrap()));
        let req = Request::builder()
            .uri("/_shell/health")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body_bytes(res).await).unwrap();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn api_requires_token_but_shell_and_static_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        // Static file preparation: the direct evidence that /comps is
        // unauthenticated (UI lives in the package's ui/ subdirectory)
        let comps = state.layout.component_dir("mockapp");
        std::fs::create_dir_all(comps.join("ui")).unwrap();
        std::fs::write(comps.join("ui").join("index.html"), "<html>ui</html>").unwrap();
        // Install contents outside ui/ (binaries/resources) must not be URL
        // accessible (B2 exposure-surface containment)
        std::fs::write(comps.join("server-bin"), "#!/bin/sh").unwrap();

        let router = build_router(Arc::clone(&state));

        // /api without token → 401 (positive rule)
        let req = Request::builder()
            .uri("/api/anything")
            .body(Body::empty())
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // /_shell without token
        let req = Request::builder()
            .uri("/_shell/health")
            .body(Body::empty())
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // /comps without token (files inside ui/ reachable)
        let req = Request::builder()
            .uri("/comps/mockapp/index.html")
            .body(Body::empty())
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_bytes(res).await, b"<html>ui</html>");

        // Install contents outside ui/ unreachable (404; the static server
        // mounts only the ui/ subdirectory)
        let req = Request::builder()
            .uri("/comps/mockapp/server-bin")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn registry_view_lists_running_components() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        state
            .registry
            .register(
                "mockns",
                RunningComponent {
                    manifest: Arc::new(manifest()),
                    endpoint: slsdk_rs::Endpoint::Tcp {
                        addr: "127.0.0.1:9001".parse().unwrap(),
                    },
                    token: Arc::new("comp-token".to_string()),
                    pid: None,
                    stop: watch::channel(()).0,
                },
            )
            .await
            .unwrap();

        let router = build_router(state);
        let req = Request::builder()
            .uri("/_shell/registry")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body_bytes(res).await).unwrap();
        let first = &body["components"][0];
        assert_eq!(first["namespace"], "mockns");
        assert_eq!(first["id"], "mockapp");
        assert_eq!(first["endpoint"], "tcp:127.0.0.1:9001");
    }

    #[tokio::test]
    async fn api_with_token_reaches_gateway_404_for_unknown_ns() {
        // With a token the request passes auth into the gateway; unregistered
        // ns → gateway 404 (not auth 401)
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        let token = state.shell_token.as_str().to_string();
        let router = build_router(state);

        let req = Request::builder()
            .uri("/api/nowhere/items")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cors_preflight_is_short_circuited_with_headers() {
        // Component API requests carrying Authorization trigger preflight:
        // OPTIONS must be short-circuited and never enter the gateway
        // (otherwise OPTIONS for an unregistered ns would get a 404 that
        // confuses the browser's judgment)
        let dir = tempfile::tempdir().unwrap();
        let router = build_router(Arc::new(AppState::new(dir.path()).unwrap()));

        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/mockns/items")
            .header(header::ORIGIN, "http://localhost:5173")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
                .unwrap(),
            "authorization"
        );
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .unwrap(),
            "GET, POST, PUT, DELETE, HEAD, OPTIONS"
        );
    }

    #[tokio::test]
    async fn static_ui_responses_carry_cors_headers() {
        // The component ESM dynamically imported in the module tier is a
        // cross-origin module script, forcing CORS checks: /comps responses
        // must carry Allow-Origin
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        let comps = state.layout.component_dir("mockapp");
        std::fs::create_dir_all(comps.join("ui")).unwrap();
        std::fs::write(comps.join("ui").join("main.js"), "export default 1;").unwrap();

        let router = build_router(state);
        let req = Request::builder()
            .uri("/comps/mockapp/main.js")
            .header(header::ORIGIN, "tauri://localhost")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "tauri://localhost"
        );
    }
}
