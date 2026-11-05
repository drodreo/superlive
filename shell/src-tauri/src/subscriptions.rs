//! The shell-side subscription plane: `GET /subscriptions/{ns}/{path…}` plus
//! the Plane-B event-stream bridge.
//!
//! Contract (architecture "the driver seat" / push-channel decisions):
//!
//! - **Namespace dispatch**: the first URL segment is the component
//!   namespace — the same registry dispatch key as `/api/{ns}` (first-URL-
//!   segment partitioning). The remainder (`{path…}`, e.g. `surface`) is
//!   **reserved for future resource-level subscriptions and does not
//!   participate in routing**: the shell bridges per namespace segment and
//!   every subscriber of a namespace receives the component's whole feed.
//! - **One component, one event stream**: the first subscriber for a
//!   namespace makes the shell open a single HTTP chunked connection to the
//!   component's standardized `GET /events` endpoint (same transport and
//!   per-component token as the `/api` forwarding path). Incoming NDJSON
//!   lines are fanned out verbatim (no translation — the payload IS the
//!   A2UI messages, verbatim (no translation — the payload IS the A2UI
//!   control language; clients filter by `surfaceId`) to every WebSocket
//!   subscriber of that namespace.
//! - **No replay**: the bridge only forwards live lines; a subscriber that
//!   connects mid-stream sees nothing older (the component-side contract).
//!   Recovery is the client's job: reconnect + re-pull queries.
//! - **Bridge resilience**: the bridge reconnects with exponential backoff
//!   (500ms doubling, 30s cap) for as long as at least one subscriber holds
//!   the feed — including while the component is down/unregistered (lookup
//!   retried each round). The last subscriber leaving tears the feed and the
//!   bridge down.
//!
//! Authentication is the shell-level positive rule (`?token=` fallback
//! included — browser-native WebSockets cannot carry headers), same as
//! `/api`: mounted in `server.rs` via the `auth::require_bearer` layer.

use crate::gateway;
use crate::registry::{Registry, RunningSnapshot};
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use slsdk_rs::EVENTS_CONTENT_TYPE;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::{RwLock, watch};

/// Broadcast buffer per feed: a ws subscriber that falls this many messages
/// behind is skipped forward (`Lagged`) — the no-replay contract says skip,
/// never block the component.
const FEED_CAPACITY: usize = 256;

/// Reconnect backoff: 500ms doubling per consecutive failure, capped at 30s.
const RECONNECT_BASE_MS: u64 = 500;
const RECONNECT_CAP_MS: u64 = 30_000;

fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(
        (RECONNECT_BASE_MS.saturating_mul(1u64 << attempt.min(16))).min(RECONNECT_CAP_MS),
    )
}

/// A live per-namespace feed: the fan-out bus plus its bridge task controls.
struct FeedHandle {
    tx: broadcast::Sender<String>,
    /// Live ws subscriber count (drives the last-subscriber teardown).
    refs: AtomicUsize,
    /// Signals the bridge task to exit (last subscriber left).
    kill: watch::Sender<bool>,
}

/// Registry of per-namespace feeds. The lock only guards short map
/// operations and is never held across an await (same discipline as
/// registry.rs).
#[derive(Default)]
pub struct SubscriptionHub {
    feeds: RwLock<HashMap<String, FeedHandle>>,
}

impl SubscriptionHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires (or lazily creates) the namespace's feed and returns a fresh
    /// subscriber receiver. Called once per accepted ws upgrade, inside the
    /// upgrade callback — the ref acquired here is symmetrically released
    /// when the pump ends.
    pub async fn subscribe(
        &self,
        namespace: &str,
        registry: Arc<Registry>,
    ) -> broadcast::Receiver<String> {
        let mut feeds = self.feeds.write().await;
        if let Some(feed) = feeds.get(namespace) {
            feed.refs.fetch_add(1, Ordering::SeqCst);
            return feed.tx.subscribe();
        }
        let (tx, _) = broadcast::channel(FEED_CAPACITY);
        let (kill, kill_rx) = watch::channel(false);
        feeds.insert(
            namespace.to_string(),
            FeedHandle {
                tx: tx.clone(),
                refs: AtomicUsize::new(1),
                kill,
            },
        );
        tokio::spawn(bridge_task(
            namespace.to_string(),
            registry,
            tx.clone(),
            kill_rx,
        ));
        tx.subscribe()
    }

    /// Releases one subscription; the last release stops the bridge and
    /// drops the feed.
    pub async fn release(&self, namespace: &str) {
        let mut feeds = self.feeds.write().await;
        if let Some(feed) = feeds.get(namespace) {
            if feed.refs.fetch_sub(1, Ordering::SeqCst) == 1 {
                let _ = feed.kill.send(true);
                feeds.remove(namespace);
            }
        }
    }

    /// Live feed count (diagnostics/tests).
    pub async fn feed_count(&self) -> usize {
        self.feeds.read().await.len()
    }
}

/// `GET /subscriptions/{*rest}`: ws upgrade handoff. The path after the
/// literal segment is `{ns}[/{reserved…}]` — see the module doc for the
/// routing semantics.
pub async fn subscriptions_handler(
    State(state): State<Arc<crate::AppState>>,
    Path(rest): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let (namespace, reserved) = match rest.split_once('/') {
        Some((ns, sub)) => (ns, sub),
        None => (rest.as_str(), ""),
    };
    // Readiness gate: an unregistered namespace has nothing to bridge (the
    // bridge itself re-checks the registry on every reconnect round, so a
    // component that appears later is picked up on the next subscription).
    if state.registry.lookup(namespace).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            format!("namespace not registered: {namespace}"),
        )
            .into_response();
    }
    // `reserved` is pinned as non-routing (module doc): threaded through for
    // observability only. Owned for the 'static upgrade closure.
    let namespace = namespace.to_string();
    let reserved = reserved.to_string();
    ws.on_upgrade(move |socket| async move {
        subscription_pump(
            state.subscriptions.clone(),
            state.registry.clone(),
            namespace,
            reserved.as_str(),
            socket,
        )
        .await;
    })
}

/// One ws subscriber's lifetime: acquire the feed, forward lines, release.
async fn subscription_pump(
    hub: Arc<SubscriptionHub>,
    registry: Arc<Registry>,
    namespace: String,
    reserved: &str,
    mut socket: WebSocket,
) {
    let mut rx = hub.subscribe(&namespace, registry).await;
    log::debug!("subscription open (ns={namespace}, path=/{reserved}): pumping the component feed");
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                // Receive-only channel by contract: client frames are
                // drained, never interpreted. Pings get the protocol-level
                // pong the browser expects.
                Some(Ok(Message::Ping(payload))) => {
                    let _ = socket.send(Message::Pong(payload)).await;
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            line = rx.recv() => match line {
                Ok(line) => {
                    if socket.send(Message::Text(line.into())).await.is_err() {
                        break;
                    }
                }
                // Skipped forward after a lag: stay connected (no replay —
                // the next line is the newest state).
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!(
                        "ws subscriber lagged on ns={namespace}: {skipped} message(s) skipped"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    hub.release(&namespace).await;
    log::debug!("subscription closed (ns={namespace})");
}

// ===== the bridge task =====

/// Outcome of one component-stream connection round.
enum StreamOutcome {
    /// kill signal: stop everything.
    Killed,
    /// The connection ended on its own (error, non-event response, or the
    /// component closed the stream). `progressed` = at least one live NDJSON
    /// line was forwarded during the round — proof of a working event plane,
    /// so the caller resets the backoff; a round without progress (connect
    /// failure, handshake rejection, zero lines) advances it.
    Ended { progressed: bool },
}

/// The per-namespace bridge: keeps exactly one connection to the component's
/// `/events` endpoint alive for as long as the feed exists, reconnecting
/// with backoff through component downtime (module doc = the contract).
async fn bridge_task(
    namespace: String,
    registry: Arc<Registry>,
    tx: broadcast::Sender<String>,
    mut kill: watch::Receiver<bool>,
) {
    let mut attempt: u32 = 0;
    loop {
        if kill_requested(&mut kill) {
            return;
        }
        let Some(snapshot) = registry.lookup(&namespace).await else {
            // Component down/unregistered: hold the feed (ws subscribers stay
            // connected) and keep polling the registry.
            log::debug!("bridge: namespace {namespace} not registered, waiting");
            if sleep_or_kill(&mut kill, backoff_delay(attempt)).await {
                return;
            }
            attempt += 1;
            continue;
        };
        match pump_component_stream(&snapshot, &tx, &mut kill).await {
            StreamOutcome::Killed => return,
            StreamOutcome::Ended { progressed } => {
                if progressed {
                    // The component demonstrably streamed: the next failure
                    // restarts from the first backoff tier.
                    attempt = 0;
                } else {
                    attempt += 1;
                }
            }
        }
        if sleep_or_kill(&mut kill, backoff_delay(attempt)).await {
            return;
        }
        attempt += 1;
    }
}

/// One connection round: connect → request `/events` → verify the protocol
/// marker → stream NDJSON lines onto the fan-out bus.
async fn pump_component_stream(
    snapshot: &RunningSnapshot,
    tx: &broadcast::Sender<String>,
    kill: &mut watch::Receiver<bool>,
) -> StreamOutcome {
    // Same connection facility as the /api forwarding path (UDS/TCP unified).
    let stream = match gateway::connect_component(&snapshot.endpoint).await {
        Ok(stream) => stream,
        Err(error) => {
            log::debug!("bridge: connect failed: {error}");
            return StreamOutcome::Ended { progressed: false };
        }
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(stream).await {
        Ok(pair) => pair,
        Err(error) => {
            log::debug!("bridge: http handshake failed: {error}");
            return StreamOutcome::Ended { progressed: false };
        }
    };
    // Drive the protocol connection (the response's arrival depends on it;
    // same pattern as the gateway's non-upgrade branch).
    tokio::spawn(async move {
        if let Err(error) = conn.await {
            log::debug!("bridge: component connection ended: {error}");
        }
    });
    let request = Request::builder()
        .method("GET")
        .uri("/events")
        .header(header::AUTHORIZATION, format!("Bearer {}", snapshot.token))
        .body(Body::empty());
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            log::debug!("bridge: event request build failed: {error}");
            return StreamOutcome::Ended { progressed: false };
        }
    };
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(error) => {
            log::debug!("bridge: event request failed: {error}");
            return StreamOutcome::Ended { progressed: false };
        }
    };
    // The vendor content-type is the protocol handshake: anything else (404
    // on components predating the event plane, misconfigured servers) counts
    // as "no event plane" and retries with backoff.
    let content_type_ok = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with(EVENTS_CONTENT_TYPE))
        .unwrap_or(false);
    if response.status() != StatusCode::OK || !content_type_ok {
        log::debug!(
            "bridge: /events answered {} (content-type ok: {content_type_ok}) — no usable event plane yet",
            response.status()
        );
        return StreamOutcome::Ended { progressed: false };
    }
    // Connection established: track live-line progress so the caller can
    // reset its backoff (a rejected/empty stream must not).

    let mut body = response.into_body();
    let mut buffer: Vec<u8> = Vec::new();
    let mut progressed = false;
    loop {
        tokio::select! {
            _ = kill_changed(kill) => return StreamOutcome::Killed,
            frame = body.frame() => match frame {
                Some(Ok(frame)) => {
                    // Trailers (rare) carry no payload — skip.
                    if let Ok(chunk) = frame.into_data() {
                        buffer.extend_from_slice(&chunk);
                        for line in drain_lines(&mut buffer) {
                            // Err(SendError) = no live subscribers: the
                            // documented drop, not a failure.
                            progressed = true;
                            let _ = tx.send(line);
                        }
                    }
                }
                Some(Err(error)) => {
                    log::debug!("bridge: component stream errored: {error}");
                    return StreamOutcome::Ended { progressed };
                }
                None => {
                    // The component closed the stream (its graceful exit or
                    // an idle-timeout): normal round end.
                    log::debug!("bridge: component stream closed");
                    return StreamOutcome::Ended { progressed };
                }
            },
        }
    }
}

/// Non-blocking check of the kill flag. The awaiting variants are
/// `kill_changed` (resolve on the next flip) and `sleep_or_kill` (flip or
/// timeout).
fn kill_requested(kill: &mut watch::Receiver<bool>) -> bool {
    *kill.borrow_and_update()
}

/// Waits for the kill signal. Returns true when the bridge should stop
/// (signal received or the feed's sender side is gone).
async fn sleep_or_kill(kill: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        _ = kill_changed(kill) => true,
        _ = tokio::time::sleep(delay) => false,
    }
}

/// Resolves when the kill flag flips to true (or the sender side is gone).
async fn kill_changed(kill: &mut watch::Receiver<bool>) {
    loop {
        if kill.changed().await.is_err() {
            return;
        }
        if *kill.borrow_and_update() {
            return;
        }
    }
}

/// Removes and returns the complete `\n`-terminated lines from the buffer,
/// leaving any trailing partial line in place (an incomplete line is an
/// incomplete message: dropped by contract, never delivered). Empty lines
/// and invalid UTF-8 are skipped — the wire is NDJSON.
fn drain_lines(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = buffer.drain(..=position).collect();
        // Strip the trailing newline (and tolerate a stray \r).
        let mut line = &line[..line.len() - 1];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() {
            continue;
        }
        match String::from_utf8(line.to_vec()) {
            Ok(text) => lines.push(text),
            Err(_) => log::warn!("bridge: dropped a non-UTF8 event line"),
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use crate::registry::RunningComponent;
    use axum::Router;
    use axum::routing::get;
    use futures_core::Stream;
    use serde_json::json;
    use slsdk_rs::Endpoint;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex;
    use tokio::sync::mpsc;

    // ---------- pure helpers ----------

    #[test]
    fn drain_lines_splits_and_preserves_the_partial_tail() {
        let mut buffer = b"{\"a\":1}\n{\"b\":".to_vec();
        let lines = drain_lines(&mut buffer);
        assert_eq!(lines, vec!["{\"a\":1}".to_string()]);
        assert_eq!(buffer, b"{\"b\":");

        buffer.extend_from_slice(b"2}\r\n\n{\"c\":3}\n");
        let lines = drain_lines(&mut buffer);
        assert_eq!(
            lines,
            vec!["{\"b\":2}".to_string(), "{\"c\":3}".to_string()]
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn drain_lines_skips_invalid_utf8_but_keeps_going() {
        let mut buffer = b"ok\n\xff\xfe\nfine\n".to_vec();
        let lines = drain_lines(&mut buffer);
        assert_eq!(lines, vec!["ok".to_string(), "fine".to_string()]);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_delay(0), Duration::from_millis(500));
        assert_eq!(backoff_delay(1), Duration::from_millis(1000));
        assert_eq!(backoff_delay(2), Duration::from_millis(2000));
        assert_eq!(backoff_delay(7), Duration::from_millis(30_000));
        assert_eq!(backoff_delay(100), Duration::from_millis(30_000));
    }

    // ---------- ws plane integration (real sockets; upgrades cannot go
    // through router.oneshot — same harness shape as the gateway tests) ------

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

    // ---------- mock component (real socket, chunked NDJSON) ----------

    /// The feed the next `/events` connection consumes (take-on-connect).
    /// `None` = the endpoint answers but streams nothing yet (the bridge
    /// backs off and retries). Swapping in a fresh receiver simulates the
    /// component restarting its event plane; dropping the current sender
    /// simulates the stream ending.
    type FeedSlot = Arc<Mutex<Option<broadcast::Receiver<String>>>>;

    /// Test-only broadcast→byte-stream adapter (a hand-rolled
    /// `futures_core::Stream` keeps the tokio-stream wrapper out of the dev
    /// dependency set). tokio's broadcast receiver has no poll-based receive
    /// API (mpsc does), so a spawned task relays lines into an mpsc channel.
    struct BroadcastStreamShim {
        rx: mpsc::Receiver<String>,
    }

    impl BroadcastStreamShim {
        fn new(bcast: broadcast::Receiver<String>) -> Self {
            let (tx, rx) = mpsc::channel::<String>(16);
            tokio::spawn(async move {
                let mut bcast = bcast;
                loop {
                    match bcast.recv().await {
                        Ok(line) => {
                            if tx.send(line).await.is_err() {
                                // Consumer gone — stop relaying.
                                break;
                            }
                        }
                        // Lagged = skip forward (same shape as the real
                        // bridge's consumer side).
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            Self { rx }
        }
    }

    impl Stream for BroadcastStreamShim {
        type Item = Result<Vec<u8>, std::convert::Infallible>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.get_mut();
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(mut line)) => {
                    line.push('\n');
                    Poll::Ready(Some(Ok(line.into_bytes())))
                }
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    /// The vendor content-type is the protocol handshake — the mock answers
    /// exactly like slsdk-rs's events endpoint (the bridge accepts nothing
    /// else).
    fn chunked_ndjson(stream: BroadcastStreamShim) -> Response {
        (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static(slsdk_rs::EVENTS_CONTENT_TYPE),
            )],
            axum::body::Body::from_stream(stream),
        )
            .into_response()
    }

    /// Mock component: `/events` streams whatever receiver currently sits in
    /// the slot as NDJSON, one connection per HTTP request (the same
    /// connect-per-subscriber shape the real bridge produces).
    async fn serve_events_component(listener: tokio::net::TcpListener, slot: FeedSlot) {
        let router = Router::new().route(
            "/events",
            get(move || async move {
                match slot.lock().await.take() {
                    Some(rx) => chunked_ndjson(BroadcastStreamShim::new(rx)),
                    None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                }
            }),
        );
        let _ = axum::serve(listener, router).await;
    }

    /// RFC 6455 client handshake against the shell (query-token variant of
    /// the gateway tests' helper).
    async fn ws_handshake(
        stream: &mut tokio::net::TcpStream,
        path: &str,
        shell_token: Option<&str>,
    ) -> String {
        let mut request = format!(
            "GET {path} HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n"
        );
        if let Some(token) = shell_token {
            request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
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

    /// RFC 6455 server→client text frame read (no mask; 7-bit and 16-bit
    /// lengths — NDJSON lines routinely exceed 125 bytes).
    async fn read_text_frame(stream: &mut tokio::net::TcpStream) -> String {
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(
            header[1] & 0x80,
            0,
            "server-to-client frames must not be masked"
        );
        let mut len = (header[1] & 0x7f) as usize;
        if len == 126 {
            let mut extended = [0u8; 2];
            stream.read_exact(&mut extended).await.unwrap();
            len = u16::from_be_bytes(extended) as usize;
        }
        assert_ne!(len, 127, "64-bit frame lengths are not exercised here");
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await.unwrap();
        String::from_utf8(payload).unwrap()
    }

    /// Polls `probe` until it turns true (deadlock insurance: every waiting
    /// test has an explicit deadline).
    async fn poll_until<Fut>(mut probe: impl FnMut() -> Fut, label: &str)
    where
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if probe().await {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("condition not met within 5s: {label}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Spins up the mock component + the shell router on real sockets and
    /// registers the namespace (the common fixture of the ws tests).
    async fn shell_and_component() -> (Endpoint, Endpoint, Arc<AppState>, FeedSlot) {
        let dir = tempfile::tempdir().unwrap();
        let component_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let component_addr = component_listener.local_addr().unwrap();
        let slot: FeedSlot = Arc::new(Mutex::new(None));
        tokio::spawn(serve_events_component(
            component_listener,
            Arc::clone(&slot),
        ));

        let state = Arc::new(AppState::new(dir.path()).unwrap());
        state
            .registry
            .register(
                "mockns",
                running_component(Endpoint::Tcp {
                    addr: component_addr,
                }),
            )
            .await
            .unwrap();
        let shell_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let shell_addr = shell_listener.local_addr().unwrap();
        let router_state = Arc::clone(&state);
        tokio::spawn(async move {
            axum::serve(shell_listener, crate::server::build_router(router_state))
                .await
                .unwrap();
        });
        // Leak the tempdir on purpose: the tasks above outlive this fixture
        // (the data dir must stay alive for the whole test; the OS reclaims
        // it at process exit).
        std::mem::forget(dir);
        (
            Endpoint::Tcp { addr: shell_addr },
            Endpoint::Tcp {
                addr: component_addr,
            },
            state,
            slot,
        )
    }

    #[tokio::test]
    async fn subscriptions_fan_out_component_lines_to_every_ws_subscriber() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (shell, _component, _state, slot) = shell_and_component().await;
            let shell_token = _state.shell_token.as_str().to_string();

            let (tx, rx) = broadcast::channel::<String>(16);
            *slot.lock().await = Some(rx);

            // Two ws subscribers; auth via the ?token= query (the browser
            // ws channel — header-carried auth is require_bearer's other
            // half and is covered by the auth/gateway suites).
            let mut ws_a = tokio::net::TcpStream::connect(match &shell {
                Endpoint::Tcp { addr } => *addr,
                _ => unreachable!("fixture always uses tcp"),
            })
            .await
            .unwrap();
            let head = ws_handshake(
                &mut ws_a,
                &format!("/subscriptions/mockns/surface?token={shell_token}"),
                None,
            )
            .await;
            assert!(
                head.starts_with("HTTP/1.1 101"),
                "expected the upgrade to succeed: {head}"
            );

            let mut ws_b = tokio::net::TcpStream::connect(match &shell {
                Endpoint::Tcp { addr } => *addr,
                _ => unreachable!("fixture always uses tcp"),
            })
            .await
            .unwrap();
            let head = ws_handshake(
                &mut ws_b,
                &format!("/subscriptions/mockns/surface?token={shell_token}"),
                None,
            )
            .await;
            assert!(
                head.starts_with("HTTP/1.1 101"),
                "second subscriber should upgrade too: {head}"
            );

            // Wait until the bridge has connected to the component stream
            // (it takes the feed out of the slot on connect); the ws
            // subscribers are on the shell-side feed, guaranteed by the
            // upgrade handshakes above.
            let _ = poll_until(
                || async { slot.lock().await.is_none() },
                "bridge connected to the component event stream",
            )
            .await;

            // One component line fans out to both subscribers verbatim.
            let line = json!({
                "version": "v0.9.1",
                "updateDataModel": { "surfaceId": "s1", "value": 1 }
            })
            .to_string();
            tx.send(line.clone()).unwrap();
            assert_eq!(read_text_frame(&mut ws_a).await, line);
            assert_eq!(read_text_frame(&mut ws_b).await, line);
        })
        .await
        .expect("the fan-out test should finish within 10 seconds");
    }

    #[tokio::test]
    async fn subscriptions_ws_stays_connected_across_a_component_stream_drop() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (shell, _component, state, slot) = shell_and_component().await;
            let shell_token = state.shell_token.as_str().to_string();
            let shell_addr = match &shell {
                Endpoint::Tcp { addr } => *addr,
                _ => unreachable!("fixture always uses tcp"),
            };

            // First event plane generation.
            let (tx1, rx1) = broadcast::channel::<String>(16);
            *slot.lock().await = Some(rx1);

            let mut ws = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
            let head = ws_handshake(
                &mut ws,
                &format!("/subscriptions/mockns/surface?token={shell_token}"),
                None,
            )
            .await;
            assert!(head.starts_with("HTTP/1.1 101"), "{head}");

            poll_until(
                || async { slot.lock().await.is_none() },
                "bridge attached to the first generation",
            )
            .await;
            tx1.send(r#"{"n":1}"#.to_string()).unwrap();
            assert_eq!(read_text_frame(&mut ws).await, r#"{"n":1}"#);

            // Drop the sender: the component stream ends mid-subscription.
            // The ws subscriber must stay connected while the bridge backs
            // off and reconnects.
            drop(tx1);

            // Second event plane generation: the component "restarted" its
            // /events with a fresh feed.
            let (tx2, rx2) = broadcast::channel::<String>(16);
            *slot.lock().await = Some(rx2);
            poll_until(
                || async { slot.lock().await.is_none() },
                "bridge attached to the restarted feed",
            )
            .await;
            tx2.send(r#"{"n":2}"#.to_string()).unwrap();
            assert_eq!(
                read_text_frame(&mut ws).await,
                r#"{"n":2}"#,
                "the same ws subscription should receive lines from the new generation"
            );
        })
        .await
        .expect("the stream-drop test should finish within 10 seconds");
    }

    #[tokio::test]
    async fn subscriptions_last_subscriber_leaving_tears_down_the_feed() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (shell, _component, state, slot) = shell_and_component().await;
            let shell_token = state.shell_token.as_str().to_string();
            let shell_addr = match &shell {
                Endpoint::Tcp { addr } => *addr,
                _ => unreachable!("fixture always uses tcp"),
            };

            // The sender is held alive for the whole test: dropping it would
            // close the mock channel and end the stream early.
            let (_tx, rx) = broadcast::channel::<String>(16);
            *slot.lock().await = Some(rx);

            let mut ws_a = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
            ws_handshake(
                &mut ws_a,
                &format!("/subscriptions/mockns/s?token={shell_token}"),
                None,
            )
            .await;
            let mut ws_b = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
            ws_handshake(
                &mut ws_b,
                &format!("/subscriptions/mockns/s?token={shell_token}"),
                None,
            )
            .await;

            poll_until(
                || async { slot.lock().await.is_none() },
                "bridge connected to the component event stream",
            )
            .await;
            poll_until(
                || async { state.subscriptions.feed_count().await == 1 },
                "exactly one live feed",
            )
            .await;

            // Subscribers leave; the last release tears the feed (and the
            // bridge) down.
            drop(ws_a);
            drop(ws_b);
            poll_until(
                || async { state.subscriptions.feed_count().await == 0 },
                "feed torn down after the last subscriber left",
            )
            .await;
        })
        .await
        .expect("the teardown test should finish within 10 seconds");
    }

    #[tokio::test]
    async fn subscriptions_handshake_without_token_is_401() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        let shell_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let shell_addr = shell_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(shell_listener, crate::server::build_router(state))
                .await
                .unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
        let head = ws_handshake(&mut stream, "/subscriptions/mockns/surface", None).await;
        assert!(
            head.starts_with("HTTP/1.1 401"),
            "a subscription handshake without a token should be rejected: {head}"
        );
    }

    #[tokio::test]
    async fn subscriptions_unregistered_namespace_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path()).unwrap());
        let shell_token = state.shell_token.as_str().to_string();
        let shell_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let shell_addr = shell_listener.local_addr().unwrap();
        let router_state = Arc::clone(&state);
        tokio::spawn(async move {
            axum::serve(shell_listener, crate::server::build_router(router_state))
                .await
                .unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(shell_addr).await.unwrap();
        let head = ws_handshake(
            &mut stream,
            &format!("/subscriptions/nowhere/surface?token={shell_token}"),
            None,
        )
        .await;
        assert!(
            head.starts_with("HTTP/1.1 404"),
            "an unregistered namespace has nothing to bridge: {head}"
        );
    }
}
