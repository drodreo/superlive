//! Unified API gateway: /api/{ns}/{rest} looks up the registry, strips the
//! prefix, forwards verbatim.
//!
//! The shell has zero business branching: each request only does
//! "validate token (auth middleware) → look up table → strip → forward
//! bytes" (architecture.md "one-sentence architecture"). The body is always
//! a raw byte stream ("unified model: identity + stream") — the shell never
//! parses component business bytes; inbound requests carry the shell-level
//! token, which is replaced with the per-component token on forwarding (the
//! component's first wall). The ws upgrade takes a dedicated branch: after
//! both ends upgrade, a bidirectional pump moves bytes and the shell only
//! relays.

use crate::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Json, Response};
use hyper::rt::{Read, Write};
use hyper_util::rt::TokioIo;
use slsdk_rs::Endpoint;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

/// Placeholder authority for shell→component forwarded URIs: the hyper
/// client needs an absolute URI to build the request, and the component side
/// does not read Host (its path space is autonomous).
pub(crate) const UPSTREAM_AUTHORITY: &str = "sl-component";

/// Hop-by-hop headers stripped before forwarding (RFC 9110 7.6.1: proxy
/// duty). The ws branch is the exception — Connection/Upgrade are the
/// upgrade handshake itself and must be passed through to the component.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// Strip hop-by-hop headers (host removed too: the hyper client resets it
/// based on the forwarded URI).
fn strip_hop_headers(headers: &mut HeaderMap, is_ws: bool) {
    let to_remove: Vec<HeaderName> = headers
        .keys()
        .filter(|name| {
            if name.as_str() == "host" {
                return true;
            }
            !is_ws && is_hop_by_hop(name)
        })
        .cloned()
        .collect();
    for name in to_remove {
        headers.remove(&name);
    }
}

/// Build the forwarded URI: the component only sees its own path space
/// (/api/{ns} stripped), query preserved verbatim.
fn forward_uri(sub_path: &str, query: Option<&str>) -> Result<Uri, axum::http::uri::InvalidUri> {
    let query_part = query.map(|q| format!("?{q}")).unwrap_or_default();
    format!("http://{UPSTREAM_AUTHORITY}/{sub_path}{query_part}").parse()
}

/// Gateway internal error forms (uniformly mapped to 502).
#[derive(Debug)]
enum GatewayError {
    /// Failed to connect to the component (not listening / exited / UDS path
    /// missing, etc.).
    Connect(io::Error),
    /// HTTP protocol handshake or request send failure.
    Protocol(hyper::Error),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::Connect(source) => {
                write!(f, "failed to connect to the component: {source}")
            }
            GatewayError::Protocol(source) => {
                write!(f, "component protocol exchange failed: {source}")
            }
        }
    }
}

impl std::error::Error for GatewayError {}

/// Shell↔component connection abstraction: unifies the UDS / TCP transports
/// for use by the hyper conn.
pub(crate) enum CompStream {
    #[cfg(unix)]
    Uds(TokioIo<tokio::net::UnixStream>),
    Tcp(TokioIo<tokio::net::TcpStream>),
}

impl Read for CompStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        // Both inner variants are Unpin TokioIo, so the get_mut projection
        // needs no unsafe
        match self.get_mut() {
            #[cfg(unix)]
            CompStream::Uds(io) => Pin::new(io).poll_read(cx, buf),
            CompStream::Tcp(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl Write for CompStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            CompStream::Uds(io) => Pin::new(io).poll_write(cx, buf),
            CompStream::Tcp(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            CompStream::Uds(io) => Pin::new(io).poll_flush(cx),
            CompStream::Tcp(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            CompStream::Uds(io) => Pin::new(io).poll_shutdown(cx),
            CompStream::Tcp(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

/// Establish a connection to the component (transport selected by the
/// endpoint descriptor).
pub(crate) async fn connect_component(endpoint: &Endpoint) -> io::Result<CompStream> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Uds { path } => Ok(CompStream::Uds(TokioIo::new(
            tokio::net::UnixStream::connect(path).await?,
        ))),
        #[cfg(windows)]
        Endpoint::Uds { path } => Err(io::Error::other(format!(
            "UDS transport is not supported on Windows (tokio/mio ecosystem limitation): {path}"
        ))),
        Endpoint::Tcp { addr } => Ok(CompStream::Tcp(TokioIo::new(
            tokio::net::TcpStream::connect(addr).await?,
        ))),
    }
}

/// hyper IO → tokio IO adapter.
///
/// hyper 1.x's `Upgraded` only implements `hyper::rt::Read/Write`, while
/// `tokio::io::copy_bidirectional` needs the tokio traits — hyper-util only
/// ships the adapter in the other direction (TokioIo), so this fills in the
/// missing direction. The read direction goes through an internal buffer
/// (one memcpy; imperceptible in the local ws scenario), zero unsafe.
pub(crate) struct HyperToTokio<T> {
    inner: T,
    buf: Vec<u8>,
}

impl<T> HyperToTokio<T> {
    pub(crate) fn new(inner: T) -> Self {
        Self {
            inner,
            buf: vec![0u8; 64 * 1024],
        }
    }
}

impl<T: Read + Unpin> tokio::io::AsyncRead for HyperToTokio<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        dst: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        // Borrow at most dst's remaining capacity from hyper per poll: a
        // single hyper poll may fill the entire ReadBuf, and more than dst's
        // capacity would leave the data nowhere to go (truncation loss or
        // out-of-bounds)
        let cap = dst.remaining().min(this.buf.len());
        let mut rb = hyper::rt::ReadBuf::new(&mut this.buf[..cap]);
        match Pin::new(&mut this.inner).poll_read(cx, rb.unfilled()) {
            std::task::Poll::Ready(Ok(())) => {}
            std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
        let filled = rb.filled();
        if filled.is_empty() {
            // hyper convention: empty filled = EOF from the peer; tokio
            // semantics = no advance and return Ok
            dst.clear();
            return std::task::Poll::Ready(Ok(()));
        }
        dst.initialize_unfilled_to(filled.len())
            .copy_from_slice(filled);
        dst.advance(filled.len());
        std::task::Poll::Ready(Ok(()))
    }
}

impl<T: Write + Unpin> tokio::io::AsyncWrite for HyperToTokio<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// Strip the shell-level token from the query before forwarding (the ws
/// compromise channel's credential is not handed down to the component side;
/// component-side auth uses the per-component token in the Authorization
/// header).
fn strip_query_token(query: &str) -> String {
    query
        .split('&')
        .filter(|pair| !pair.starts_with("token=") && *pair != "token")
        .collect::<Vec<_>>()
        .join("&")
}

/// Gateway entry: /api/{ns}/{rest} dispatch.
pub async fn gateway_handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    // The wildcard route guarantees the /api prefix; this branch is a
    // defensive fallback
    let Some(rest) = path.strip_prefix("/api/") else {
        return error_response(StatusCode::NOT_FOUND, "missing the /api prefix");
    };
    let (namespace, sub_path) = match rest.split_once('/') {
        Some((ns, sub)) => (ns, sub),
        None => (rest, ""),
    };
    let Some(snapshot) = state.registry.lookup(namespace).await else {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("namespace not registered: {namespace}"),
        );
    };

    let (mut parts, body) = req.into_parts();
    // The hyper server injects OnUpgrade into extensions for upgrade
    // requests; taking ownership (remove) is the shell-side upgrade channel,
    // and its presence is the ws determination
    let inbound_upgrade = parts.extensions.remove::<hyper::upgrade::OnUpgrade>();
    let is_ws = inbound_upgrade.is_some();
    let query = parts.uri.query().and_then(|q| {
        let stripped = strip_query_token(q);
        // If stripping leaves it empty, carry no query (avoids a forwarded
        // URI with a dangling ?)
        if stripped.is_empty() {
            None
        } else {
            Some(stripped)
        }
    });

    parts.uri = match forward_uri(sub_path, query.as_deref()) {
        Ok(uri) => uri,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("failed to build the forwarding path: {e}"),
            )
        }
    };
    strip_hop_headers(&mut parts.headers, is_ws);
    match HeaderValue::from_str(&format!("Bearer {}", snapshot.token)) {
        Ok(value) => {
            parts.headers.insert(AUTHORIZATION, value);
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to build the component token header: {e}"),
            )
        }
    }
    let forward_req = hyper::Request::from_parts(parts, body);

    match forward(&snapshot.endpoint, forward_req, is_ws, inbound_upgrade).await {
        Ok(res) => res,
        Err(e) => {
            log::warn!("component forwarding failed (ns={namespace}): {e}");
            error_response(
                StatusCode::BAD_GATEWAY,
                &format!("component unreachable: {e}"),
            )
        }
    }
}

/// Forward the request to the component. Each request builds a fresh
/// connection (local UDS/TCP connects are microsecond-scale — the
/// first-version tradeoff of having no connection pool).
async fn forward(
    endpoint: &Endpoint,
    req: hyper::Request<Body>,
    is_ws: bool,
    inbound_upgrade: Option<hyper::upgrade::OnUpgrade>,
) -> Result<Response, GatewayError> {
    let stream = connect_component(endpoint)
        .await
        .map_err(GatewayError::Connect)?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(stream)
        .await
        .map_err(GatewayError::Protocol)?;

    if is_ws {
        let Some(inbound) = inbound_upgrade else {
            // is_ws is derived from inbound_upgrade's presence, so this
            // branch is unreachable; defensive fallback
            return Ok(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "upgrade channel missing",
            ));
        };
        // The client connection must have upgrade support explicitly enabled:
        // a Connection without with_upgrades will not fulfill the outbound
        // OnUpgrade after 101 (the shell side would get an UpgradeExpected
        // cancellation error instead). conn must be driven before
        // send_request — the response's arrival depends on the protocol
        // connection being polled, and starting the driver only after
        // send_request creates a waiting deadlock; upgrade fulfillment
        // (after 101) is driven continuously by that task until the
        // connection ends.
        let upgradeable = conn.with_upgrades();
        tokio::spawn(async move {
            if let Err(e) = upgradeable.await {
                log::debug!("component connection ended: {e}");
            }
        });
        let res = sender
            .send_request(req)
            .await
            .map_err(GatewayError::Protocol)?;
        if res.status() != StatusCode::SWITCHING_PROTOCOLS {
            // Component refuses the upgrade: the connection closes when conn
            // drops, and the error response is relayed verbatim
            return Ok(map_response(res));
        }
        // Outbound upgrade channel: for a 101 response the hyper client
        // injects OnUpgrade into extensions (OnUpgrade itself implements
        // Future — awaiting it yields the Upgraded)
        let out_upgrade = res.extensions().get::<hyper::upgrade::OnUpgrade>().cloned();
        // The component's 101 response is relayed verbatim (Sec-WebSocket-Accept
        // and other headers are in parts); the outbound OnUpgrade has been
        // cloned away, so clear it to avoid leaking downstream
        let (mut res_parts, _) = res.into_parts();
        res_parts.extensions.clear();
        let response = Response::from_parts(res_parts, Body::empty());
        tokio::spawn(async move {
            let pump = async {
                let shell_side = inbound
                    .await
                    .map_err(|e| io::Error::other(format!("shell-side upgrade failed: {e}")));
                let component_side = async {
                    // The protocol connection is driven continuously by the
                    // independent task started before send_request; an
                    // upgrade fulfillment failure surfaces through the
                    // cancellation/failure path of out.await
                    let out = out_upgrade.ok_or_else(|| {
                        io::Error::other("component response is missing the upgrade channel")
                    })?;
                    out.await.map_err(|e| {
                        io::Error::other(format!("component-side upgrade failed: {e}"))
                    })
                }
                .await;
                match (shell_side, component_side) {
                    (Ok(a), Ok(b)) => {
                        let mut a = HyperToTokio::new(a);
                        let mut b = HyperToTokio::new(b);
                        tokio::io::copy_bidirectional(&mut a, &mut b).await
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            };
            match pump.await {
                Ok((up, down)) => {
                    log::debug!("ws pump finished ({up} bytes up, {down} bytes down)")
                }
                Err(e) => log::debug!("ws pump disconnected: {e}"),
            }
        });
        Ok(response)
    } else {
        // conn must be polled continuously for the protocol to progress;
        // hand it to an independent task to drive until the connection ends
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                log::debug!("component connection ended: {e}");
            }
        });
        let res = sender
            .send_request(req)
            .await
            .map_err(GatewayError::Protocol)?;
        Ok(map_response(res))
    }
}

/// hyper response → axum response: the body stays streaming (Incoming is
/// handed directly to the axum Body, not buffered). The response direction
/// also strips hop-by-hop headers (RFC 9110 7.6.1 bidirectional proxy duty);
/// this function only serves the non-upgrade branch (ws 101 takes the
/// dedicated construction), is_ws is always false.
fn map_response(res: hyper::Response<hyper::body::Incoming>) -> Response {
    let (mut parts, body) = res.into_parts();
    strip_hop_headers(&mut parts.headers, false);
    Response::from_parts(parts, Body::new(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::generate_token;
    use crate::registry::RunningComponent;
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::routing::{get, post};
    use axum::Router;
    use std::path::Path;
    use tokio::sync::{watch, Mutex};
    use tower::ServiceExt;

    fn test_manifest() -> slsdk_rs::ComponentManifest {
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

    fn running_component(endpoint: Endpoint) -> RunningComponent {
        RunningComponent {
            manifest: Arc::new(test_manifest()),
            endpoint,
            token: Arc::new("comp-token".to_string()),
            pid: None,
            stop: watch::channel(()).0,
        }
    }

    /// Assemble a shell router with a mock component hanging in the registry.
    async fn router_with_mock(dir: &Path, endpoint: Endpoint) -> (Router, Arc<AppState>) {
        let state = Arc::new(AppState::new(dir).unwrap());
        state
            .registry
            .register("mockns", running_component(endpoint))
            .await
            .unwrap();
        let router = crate::server::build_router(Arc::clone(&state));
        (router, state)
    }

    fn get_req(path: &str, shell_token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path);
        if let Some(t) = shell_token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn body_bytes(res: Response) -> Vec<u8> {
        axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    // ---------- Mock component ----------

    #[derive(Debug, Clone)]
    struct Hit {
        path: String,
        query: Option<String>,
        auth: Option<String>,
    }
    type Hits = Arc<Mutex<Vec<Hit>>>;

    async fn record(State(hits): State<Hits>, req: Request) -> Response {
        hits.lock().await.push(Hit {
            path: req.uri().path().to_string(),
            query: req.uri().query().map(str::to_string),
            auth: req
                .headers()
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
        });
        (StatusCode::OK, "mock-ok").into_response()
    }

    async fn echo(req: Request) -> Response {
        // Relay the byte stream verbatim: verifies the body passes through
        // the full chain
        Response::new(req.into_body())
    }

    async fn ws_echo(ws: WebSocketUpgrade) -> Response {
        ws.on_upgrade(|mut socket: WebSocket| async move {
            while let Some(Ok(msg)) = socket.recv().await {
                match msg {
                    Message::Text(text) => {
                        if socket.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
    }

    fn mock_router(hits: Hits) -> Router {
        Router::new()
            .route("/items", get(record))
            .route("/items/{*rest}", get(record))
            .route("/echo", post(echo))
            .route("/ws", get(ws_echo))
            .with_state(hits)
    }

    async fn serve_mock_tcp() -> Endpoint {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, mock_router(Arc::new(Mutex::new(Vec::new()))))
                .await
                .unwrap();
        });
        Endpoint::Tcp { addr }
    }

    // ---------- Forwarding behavior ----------

    #[tokio::test]
    async fn forwards_with_prefix_stripped_and_component_token() {
        let dir = tempfile::tempdir().unwrap();
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_hits = Arc::clone(&hits);
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/items", get(record))
                    .route("/items/{*rest}", get(record))
                    .with_state(mock_hits),
            )
            .await
            .unwrap();
        });

        let (router, state) = router_with_mock(dir.path(), Endpoint::Tcp { addr }).await;
        let shell_token = state.shell_token.as_str().to_string();

        let res = router
            .oneshot(get_req("/api/mockns/items/1?q=2", Some(&shell_token)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_bytes(res).await, b"mock-ok");

        let recorded = hits.lock().await;
        assert_eq!(recorded.len(), 1);
        // /api/mockns prefix stripped: the component only sees its own path space
        assert_eq!(recorded[0].path, "/items/1");
        // query preserved verbatim
        assert_eq!(recorded[0].query.as_deref(), Some("q=2"));
        // Replaced with the per-component token on forwarding (shell-level
        // token not handed down)
        assert_eq!(recorded[0].auth.as_deref(), Some("Bearer comp-token"));
    }

    #[tokio::test]
    async fn query_token_authenticates_and_is_stripped_before_forwarding() {
        // End-to-end of the ws compromise channel: ?token= passes shell-level
        // auth and is stripped before forwarding (the component side only
        // sees the business query, not the shell-level credential)
        let dir = tempfile::tempdir().unwrap();
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock_hits = Arc::clone(&hits);
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/items", get(record))
                    .route("/items/{*rest}", get(record))
                    .with_state(mock_hits),
            )
            .await
            .unwrap();
        });

        let (router, state) = router_with_mock(dir.path(), Endpoint::Tcp { addr }).await;
        let shell_token = state.shell_token.as_str().to_string();

        let res = router
            .oneshot(get_req(
                &format!("/api/mockns/items/7?q=2&token={shell_token}"),
                None, // No Authorization: force the query channel
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let recorded = hits.lock().await;
        assert_eq!(recorded.len(), 1);
        // The component-side query contains no shell-level token
        assert_eq!(recorded[0].query.as_deref(), Some("q=2"));
    }

    #[tokio::test]
    async fn post_body_streams_through() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = serve_mock_tcp().await;
        let (router, state) = router_with_mock(dir.path(), endpoint).await;

        let req = Request::builder()
            .method("POST")
            .uri("/api/mockns/echo")
            .header(AUTHORIZATION, format!("Bearer {}", state.shell_token))
            .body(Body::from("\u{1f600}-binary-\u{0}\u{1}bytes"))
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            body_bytes(res).await,
            "\u{1f600}-binary-\u{0}\u{1}bytes".as_bytes()
        );
    }

    #[tokio::test]
    async fn unregistered_namespace_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        let router = crate::server::build_router(Arc::clone(&state));
        let res = router
            .oneshot(get_req(
                "/api/nowhere/items",
                Some(state.shell_token.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unreachable_component_is_502() {
        let dir = tempfile::tempdir().unwrap();
        // Points at a loopback port guaranteed to have no listener
        let endpoint = Endpoint::Tcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        };
        let (router, state) = router_with_mock(dir.path(), endpoint).await;
        let res = router
            .oneshot(get_req(
                "/api/mockns/items",
                Some(state.shell_token.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    }

    // ---------- UDS transport ----------

    #[cfg(unix)]
    #[tokio::test]
    async fn forwards_over_uds() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("mock.sock");
        let hits: Hits = Arc::new(Mutex::new(Vec::new()));
        let mock_hits = Arc::clone(&hits);
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/items", get(record))
                    .with_state(mock_hits),
            )
            .await
            .unwrap();
        });

        let (router, state) = router_with_mock(
            dir.path(),
            Endpoint::Uds {
                path: sock.to_string_lossy().into_owned(),
            },
        )
        .await;
        let res = router
            .oneshot(get_req(
                "/api/mockns/items",
                Some(state.shell_token.as_str()),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(hits.lock().await[0].path, "/items");
    }

    // ---------- ws upgrade ----------

    #[tokio::test]
    async fn websocket_upgrade_is_proxied_bidirectionally() {
        // The ws forwarding chain is multi-task cooperation (server / conn
        // driver / pump); any link hanging dead would block the test
        // forever — the timeout fallback turns a deadlock into an explicit
        // failure instead of a hang
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let dir = tempfile::tempdir().unwrap();

            // Mock component: ws echo (real socket)
            let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mock_addr = mock_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(mock_listener, Router::new().route("/ws", get(ws_echo)))
                    .await
                    .unwrap();
            });

            // Shell: full router on a real socket (upgrade cannot go through oneshot)
            let state = Arc::new(AppState::new(dir.path()).unwrap());
            state
                .registry
                .register(
                    "mockns",
                    running_component(Endpoint::Tcp { addr: mock_addr }),
                )
                .await
                .unwrap();
            let shell_token = state.shell_token.as_str().to_string();
            let shell_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let shell_addr = shell_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(
                    shell_listener,
                    crate::server::build_router(Arc::clone(&state)),
                )
                .await
                .unwrap();
            });

            // RFC 6455 raw handshake (with the shell-level token: the auth
            // positive rule treats ws the same as anything else)
            let mut stream = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
            use tokio::io::AsyncWriteExt;
            let handshake = format!(
                "GET /api/mockns/ws HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Authorization: Bearer {shell_token}\r\n\
             \r\n"
            );
            stream.write_all(handshake.as_bytes()).await.unwrap();
            let head = read_http_head(&mut stream).await;
            assert!(
                head.starts_with("HTTP/1.1 101"),
                "expected a 101 upgrade response: {head}"
            );
            assert!(
                // Header names are case-insensitive: hyper relays the
                // component's response with lowercase header names
                head.to_ascii_lowercase().contains("sec-websocket-accept"),
                "the component accept header should be echoed back verbatim: {head}"
            );

            // Bidirectional pump: send a masked text frame, receive the echo frame
            send_masked_text(&mut stream, b"hi-shell").await;
            let echoed = read_text_frame(&mut stream).await;
            assert_eq!(echoed, b"hi-shell");
        })
        .await
        .expect("the ws forwarding test should finish within 10 seconds (timeout = deadlock in the forwarding chain)");
    }

    #[tokio::test]
    async fn websocket_without_token_is_401() {
        let dir = tempfile::tempdir().unwrap();
        let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = mock_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(mock_listener, Router::new().route("/ws", get(ws_echo)))
                .await
                .unwrap();
        });
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        state
            .registry
            .register(
                "mockns",
                running_component(Endpoint::Tcp { addr: mock_addr }),
            )
            .await
            .unwrap();
        let shell_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let shell_addr = shell_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(shell_listener, crate::server::build_router(state)).await;
        });

        let mut stream = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
        use tokio::io::AsyncWriteExt;
        let handshake = "GET /api/mockns/ws HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n";
        stream.write_all(handshake.as_bytes()).await.unwrap();
        let head = read_http_head(&mut stream).await;
        assert!(
            head.starts_with("HTTP/1.1 401"),
            "a ws handshake without a token should be rejected by auth: {head}"
        );
    }

    /// Read byte by byte until the empty line (end of headers), returning the
    /// full header text (test-only; handshake headers are tiny).
    async fn read_http_head(stream: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// RFC 6455 client→server text frame (must be masked); only supports
    /// len < 126 (sufficient for tests).
    async fn send_masked_text(stream: &mut tokio::net::TcpStream, payload: &[u8]) {
        use tokio::io::AsyncWriteExt;
        assert!(payload.len() < 126);
        let mut mask = [0u8; 4];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut mask);
        let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
        frame.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        stream.write_all(&frame).await.unwrap();
    }

    /// RFC 6455 server→client text frame read (no mask, len < 126).
    async fn read_text_frame(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).await.unwrap();
        let len = (header[1] & 0x7f) as usize;
        assert_eq!(
            header[1] & 0x80,
            0,
            "server-to-client frames must not be masked"
        );
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
        payload
    }

    #[test]
    fn hop_header_detection() {
        assert!(is_hop_by_hop(&HeaderName::from_static("connection")));
        assert!(is_hop_by_hop(&HeaderName::from_static("transfer-encoding")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("content-type")));
        assert!(!is_hop_by_hop(&HeaderName::from_static("authorization")));
    }

    #[test]
    fn forward_uri_strips_and_keeps_query() {
        let uri = forward_uri("items/1", Some("a=1&b=2")).unwrap();
        assert_eq!(uri.path(), "/items/1");
        assert_eq!(uri.query(), Some("a=1&b=2"));
        let uri = forward_uri("", None).unwrap();
        assert_eq!(uri.path(), "/");
        assert!(uri.query().is_none());
    }

    #[test]
    fn token_generation_usable_for_headers() {
        // The generated value must be directly usable as a Bearer header
        // (pure hex, no illegal characters)
        let token = generate_token();
        assert!(HeaderValue::from_str(&format!("Bearer {token}")).is_ok());
    }
}
