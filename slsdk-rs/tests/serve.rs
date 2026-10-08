//! serve() integration smoke tests (Linux UDS scenario; the Windows branch is cfg-isolated
//! and not tested).
//!
//! The env contract (SL_ENDPOINT/SL_TOKEN) is process-global state, so all scenarios run
//! serially inside a single test function, avoiding parallel tests stomping on each other's
//! env.

#![cfg(unix)]

use axum::http::{header::AUTHORIZATION, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use hyper::rt::{Read, Write};
use hyper_util::client::legacy::{connect::Connected, connect::Connection, Client};
use hyper_util::rt::{TokioExecutor, TokioIo};
use slsdk_rs::{ComponentManifest, Endpoint, ENV_ENDPOINT, ENV_TOKEN};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::net::UnixStream;
use tower_service::Service;

/// Minimal manifest (serve only reads id/version for the /health response).
fn test_manifest() -> ComponentManifest {
    serde_json::from_str(
        r#"{
        "id": "xatodo",
        "version": "0.2.0",
        "min_shell_version": "0.1.0",
        "namespace": "todo",
        "display_name": "XaTodo",
        "ui": { "ui_type": "module", "entry": "assets/main.js" },
        "menu": null,
        "routes": [],
        "launch": { "command": "xatodo-server", "args": [] },
        "files": [ { "path": "xatodo-server", "sha256": "aa" } ]
    }"#,
    )
    .unwrap()
}

// ---------- UDS connector for the hyper-util legacy client ----------
// hyper-util does not ship a UDS connector; a newtype bypasses the orphan rule (Connection
// is a foreign trait).

#[derive(Clone)]
struct UdsConnector(PathBuf);

struct UdsIo(TokioIo<UnixStream>);

impl Service<axum::http::Uri> for UdsConnector {
    type Response = UdsIo;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<UdsIo, std::io::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: axum::http::Uri) -> Self::Future {
        let path = self.0.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(path).await?;
            Ok(UdsIo(TokioIo::new(stream)))
        })
    }
}

impl Connection for UdsIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl Read for UdsIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        // All UdsIo fields are Unpin, so the get_mut projection needs no unsafe
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl Write for UdsIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

fn uds_client(path: PathBuf) -> Client<UdsConnector, axum::body::Body> {
    Client::builder(TokioExecutor::new()).build(UdsConnector(path))
}

async fn get_request(
    client: &Client<UdsConnector, axum::body::Body>,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().uri(format!("http://localhost{path}"));
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let req = builder.body(axum::body::Body::empty()).unwrap();
    let res = client.request(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn uds_and_tcp_lifecycle_smoke() {
    // ===== Scenario 1: shell-launched shape (UDS + token) =====
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("slsdk-uds-test.sock");
    std::env::set_var(ENV_ENDPOINT, format!("uds:{}", sock.display()));
    std::env::set_var(ENV_TOKEN, "test-token-123");

    // Sample component business route: the shell strips /api/{ns} before forwarding; the
    // component only sees its own path space
    let routes = Router::new().route("/items", get(|| async { "items-ok" }));
    let info = slsdk_rs::serve(routes, &test_manifest()).await.unwrap();

    // ServeInfo reports the actual listen address (UDS expands to the real socket path)
    assert_eq!(
        info.endpoint,
        Endpoint::Uds {
            path: sock.display().to_string()
        }
    );

    let client = uds_client(sock.clone());

    // Hit /health with the token: 200 + component identity (JSON)
    let (status, body) = get_request(&client, "/health", Some("test-token-123")).await;
    assert_eq!(status, StatusCode::OK);
    let health: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["id"], "xatodo");
    assert_eq!(health["version"], "0.2.0");

    // Hit the business route with the token: the token middleware covers all merged routes
    // (text/plain raw bytes)
    let (status, body) = get_request(&client, "/items", Some("test-token-123")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(String::from_utf8_lossy(&body), "items-ok");

    // No token → 401 (architecture.md "Security": no token means 401, always)
    let (status, _) = get_request(&client, "/health", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Wrong token → 401 (constant-time comparison path)
    let (status, _) = get_request(&client, "/health", Some("wrong-token")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Lowercase scheme (bearer) → 200: RFC 7235 specifies auth scheme is case-insensitive
    let req = Request::builder()
        .uri("http://localhost/health")
        .header(AUTHORIZATION, "bearer test-token-123")
        .body(axum::body::Body::empty())
        .unwrap();
    let res = client.request(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The shutdown handle triggers graceful exit; after wait() returns the socket file is
    // cleaned up
    info.shutdown();
    info.wait().await.unwrap();
    assert!(
        !sock.exists(),
        "the UDS socket file should be cleaned up after graceful shutdown"
    );

    // ===== Scenario 2: standalone debugging shape (no env → tcp:127.0.0.1:0 + no auth) =====
    std::env::remove_var(ENV_ENDPOINT);
    std::env::remove_var(ENV_TOKEN);
    let info = slsdk_rs::serve(Router::new(), &test_manifest())
        .await
        .unwrap();
    let addr = match &info.endpoint {
        Endpoint::Tcp { addr } => *addr,
        other => panic!("standalone debug mode should be Tcp, got {other:?}"),
    };
    // :0 has expanded to the actually assigned port
    assert_ne!(addr.port(), 0);

    // No-auth passthrough: hand-written minimal HTTP/1.1 request to verify (this scenario
    // needs no token semantics)
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert!(
        text.starts_with("HTTP/1.1 200"),
        "standalone debug mode should serve unauthenticated requests, actual response: {text}"
    );
    assert!(
        text.contains("\"id\":\"xatodo\""),
        "health should return the component identity: {text}"
    );

    // The SIGTERM path is not tested here (process-level signals should not be injected
    // into the test process); the shutdown handle covers the same exit channel
    info.shutdown();
    info.wait().await.unwrap();
}
