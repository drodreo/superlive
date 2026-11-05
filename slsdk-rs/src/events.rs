//! The standardized event plane of the B-side contract: `GET /events` plus the
//! [`A2uiNotifier`] push handle.
//!
//! Wire contract (pinned here, referenced by the shell bridge and slsdk-ts's
//! observe client):
//!
//! - Transport: HTTP chunked stream. `GET /events` answers `200` with
//!   `Content-Type: application/vnd.superlive.events+v1` and streams a
//!   newline-delimited JSON (NDJSON) body: **one [`ServerToClientMessage`]
//!   per `\n`-terminated line**, no SSE framing, no heartbeat frames. A
//!   trailing partial line (connection cut mid-line) is an incomplete message
//!   and is discarded by consumers — never delivered.
//! - Delivery semantics: **live feed only — no replay, no backlog**. A
//!   subscriber that connects (or reconnects) sees only messages pushed after
//!   its connection; recovery is the consumer's job (reconnect + re-pull, and
//!   components that need reliable snapshots re-push them on their own
//!   schedule). `updateDataModel` full-document writes are idempotent, which
//!   is what makes this contract workable.
//! - No-subscriber behavior: pushes are dropped, silently
//!   ([`A2uiNotifier::push`] never blocks and never buffers for absent
//!   consumers). The endpoint existing does not imply anyone is listening.
//!
//! The payload is the A2UI `ServerToClientMessage` envelope verbatim
//! (`version` + exactly one operation key; field names camelCase, aligned
//! with slsdk-ts `src/ui/a2ui/types.ts`). The shell does not translate it —
//! subscribing to a component's feed means speaking its control language
//! (A2UI).

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::Poll;
use tokio::sync::{broadcast, mpsc, watch};

/// Content-Type of the standardized event stream. The private vendor subtype
/// doubles as the protocol handshake marker: the shell bridge accepts only
/// this type, so a component answering /events with anything else is treated
/// as "no event plane" (bridge retries with backoff).
pub const EVENTS_CONTENT_TYPE: &str = "application/vnd.superlive.events+v1";

/// The A2UI protocol version stamped on every envelope this SDK builds
/// (aligned with slsdk-ts `ServerToClientMessage.version`).
pub const A2UI_VERSION: &str = "v0.9.1";

/// The in-flight capacity of a notifier's broadcast channel. A slow consumer
/// that falls more than this many messages behind is skipped forward
/// (`Lagged`) — consistent with the no-replay contract: skipping beats
/// blocking the producer.
const CHANNEL_CAPACITY: usize = 256;

/// Per-connection capacity of the bridge channel feeding an `EventStream`
/// body. When the HTTP consumer stalls, the bridge's `send` parks, the
/// broadcast receiver stops draining, and the consumer is skipped forward
/// (`Lagged`) — the same no-replay semantics as the notifier channel itself.
const BRIDGE_CAPACITY: usize = 16;

// ===== the A2UI server → client envelope (minimal authoring types) =====

/// protocol.md createSurface — surfaceId/catalogId are immutable after
/// creation (field names aligned with slsdk-ts `CreateSurfaceMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSurfaceMessage {
    pub surface_id: String,
    pub catalog_id: String,
}

/// protocol.md updateComponents — flat adjacency list; the per-component
/// shape is owned by the component's catalog, so the SDK carries instances
/// opaquely (`serde_json::Value`), exactly like slsdk-ts's wide
/// `A2uiComponentInstance`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateComponentsMessage {
    pub surface_id: String,
    pub components: Vec<serde_json::Value>,
}

/// protocol.md updateDataModel — replaces the value at `path`; `path`
/// omitted = the whole model; `value` omitted = the key at path is removed
/// (field semantics aligned with slsdk-ts `UpdateDataModelMessage`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDataModelMessage {
    pub surface_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// protocol.md deleteSurface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSurfaceMessage {
    pub surface_id: String,
}

/// Server → client envelope: `version` + exactly one operation key.
///
/// Modeled as a flat struct with optional operation fields (that is the wire
/// shape; a Rust enum cannot express "tag + sibling operation key" without a
/// custom serializer). Build instances through the [`ServerToClientMessage::create_surface`]
/// family — they stamp [`A2UI_VERSION`] and guarantee exactly one operation
/// key; hand-rolled structs with zero operation keys serialize to a
/// version-only envelope that every consumer must reject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerToClientMessage {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_surface: Option<CreateSurfaceMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_components: Option<UpdateComponentsMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_data_model: Option<UpdateDataModelMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_surface: Option<DeleteSurfaceMessage>,
}

impl ServerToClientMessage {
    /// createSurface envelope (surface instantiation).
    pub fn create_surface(surface_id: impl Into<String>, catalog_id: impl Into<String>) -> Self {
        Self {
            version: A2UI_VERSION.to_string(),
            create_surface: Some(CreateSurfaceMessage {
                surface_id: surface_id.into(),
                catalog_id: catalog_id.into(),
            }),
            update_components: None,
            update_data_model: None,
            delete_surface: None,
        }
    }

    /// updateComponents envelope (flat component table upsert).
    pub fn update_components(
        surface_id: impl Into<String>,
        components: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            version: A2UI_VERSION.to_string(),
            create_surface: None,
            update_components: Some(UpdateComponentsMessage {
                surface_id: surface_id.into(),
                components,
            }),
            update_data_model: None,
            delete_surface: None,
        }
    }

    /// updateDataModel envelope (pointer-scoped model write; `path = None`
    /// addresses the whole model — the natural "re-push my snapshot" shape).
    pub fn update_data_model(
        surface_id: impl Into<String>,
        path: Option<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            version: A2UI_VERSION.to_string(),
            create_surface: None,
            update_components: None,
            update_data_model: Some(UpdateDataModelMessage {
                surface_id: surface_id.into(),
                path,
                value: Some(value),
            }),
            delete_surface: None,
        }
    }

    /// deleteSurface envelope (surface teardown).
    pub fn delete_surface(surface_id: impl Into<String>) -> Self {
        Self {
            version: A2UI_VERSION.to_string(),
            create_surface: None,
            update_components: None,
            update_data_model: None,
            delete_surface: Some(DeleteSurfaceMessage {
                surface_id: surface_id.into(),
            }),
        }
    }

    /// Exactly one operation key is present (the constructor family's
    /// invariant; exported for consumers that deserialize third-party
    /// envelopes and want the shape checked).
    pub fn has_single_operation(&self) -> bool {
        [
            self.create_surface.is_some(),
            self.update_components.is_some(),
            self.update_data_model.is_some(),
            self.delete_surface.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
            == 1
    }
}

// ===== the push handle =====

/// The component-author notification API: `push` a
/// [`ServerToClientMessage`] and every current `/events` subscriber receives
/// it as one NDJSON line.
///
/// Cheap to clone (a broadcast sender under the hood); components hold one
/// clone in their service state and call [`A2uiNotifier::push`] wherever
/// their data changes. Pushing is synchronous and never blocks: with zero
/// subscribers the message is dropped (module doc: no replay); a consumer
/// that lags more than [`CHANNEL_CAPACITY`] messages behind is skipped
/// forward rather than allowed to stall the producer.
#[derive(Clone)]
pub struct A2uiNotifier {
    tx: broadcast::Sender<String>,
    /// The closed-plane signal: [`A2uiNotifier::close`] stamps it, and every
    /// `EventStream` bridge task watches its own clone and ends the stream
    /// the moment it fires (shutdown-time teardown). A watch (the same
    /// primitive `serve` uses for its shutdown signal) keeps `push` lock-free
    /// and lets late subscribers see the close immediately.
    closed_tx: watch::Sender<()>,
    closed_rx: watch::Receiver<()>,
}

impl Default for A2uiNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl A2uiNotifier {
    /// Creates a notifier. The matching `/events` endpoint is mounted by
    /// `serve`/`serve_with_events`; the author keeps a clone in their own
    /// router state (the handle is what makes "component data changed → UI
    /// updates" one line of code).
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        let (closed_tx, closed_rx) = watch::channel(());
        Self {
            tx,
            closed_tx,
            closed_rx,
        }
    }

    /// Pushes one message to every current subscriber. Fire-and-forget:
    /// no-subscriber and serialization outcomes are both silent by contract
    /// (serialization of these envelopes cannot fail — the payload is
    /// `serde_json::Value` all the way down).
    pub fn push(&self, message: ServerToClientMessage) {
        match serde_json::to_string(&message) {
            Ok(line) => {
                // Err(SendError) = zero live receivers: the documented
                // no-subscriber drop, not a failure.
                let _ = self.tx.send(line);
            }
            Err(error) => {
                // Unreachable for Value-based payloads; logged rather than
                // propagated so push keeps its infallible face.
                log::warn!("a2ui event serialization failed (dropped): {error}");
            }
        }
    }

    /// Number of currently connected `/events` consumers (diagnostics and
    /// tests; zero does not mean the endpoint is gone).
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// Closes the event plane: every live `/events` stream ends immediately
    /// (each bridge task's `select` watches the close signal), and later
    /// pushes have no reachable consumer (the same no-replay face as pushing
    /// with zero subscribers). Lines already buffered in the broadcast
    /// channel are dropped — the contract never promised them (lagged
    /// consumers skip them the same way). This is the shutdown-time teardown
    /// hook — `serve`'s graceful shutdown future calls it the moment the
    /// exit signal fires, so in-flight chunked responses finish and graceful
    /// shutdown is satisfiable with active subscribers.
    pub fn close(&self) {
        let _ = self.closed_tx.send(());
    }

    /// Opens a new consumer stream (the /events handler's intake; one
    /// receiver per connection).
    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// A fresh watcher of the close signal (one per spawned stream).
    fn closed_watcher(&self) -> watch::Receiver<()> {
        self.closed_rx.clone()
    }
}

// ===== the /events endpoint =====

/// The stream side of `GET /events`: NDJSON lines relayed from the notifier's
/// broadcast channel. Lagged consumers skip forward (warn + keep going —
/// dropping beats tearing the connection); the stream ends when the broadcast
/// channel closes, either because every notifier clone was dropped or because
/// [`A2uiNotifier::close`] fired (the shutdown-time teardown) — either way
/// the chunked response terminates.
struct EventStream {
    rx: mpsc::Receiver<String>,
}

impl EventStream {
    /// Opens one bridge per connection. tokio's broadcast receiver exposes
    /// no poll-based receive (mpsc's `poll_recv` is the only public poll
    /// API), so a task owns the async `recv` loop and relays lines into an
    /// mpsc channel the stream body polls. When the response body drops,
    /// the mpsc receiver goes with it, the task's next `send` fails, and the
    /// bridge unwinds.
    fn spawn(notifier: &A2uiNotifier) -> Self {
        let (tx, rx) = mpsc::channel::<String>(BRIDGE_CAPACITY);
        let mut bcast = notifier.subscribe();
        let mut closed = notifier.closed_watcher();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Terminating beats delivering: when the event plane
                    // closes (shutdown teardown) the stream ends immediately;
                    // buffered broadcast lines are dropped (the no-replay
                    // contract never promised them).
                    biased;
                    _ = closed.changed() => break,
                    line = bcast.recv() => match line {
                        Ok(line) => {
                            if tx.send(line).await.is_err() {
                                // Consumer gone (connection closed) — stop bridging.
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            // The consumer fell behind; the no-replay contract
                            // says skip, not die.
                            log::warn!("events consumer lagged, {skipped} message(s) skipped");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
        Self { rx }
    }
}

impl Stream for EventStream {
    type Item = Result<axum::body::Bytes, std::convert::Infallible>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // mpsc::Receiver is Unpin, so the get_mut projection above makes
        // poll_recv callable without further pin gymnastics.
        match this.rx.poll_recv(cx) {
            Poll::Ready(Some(mut line)) => {
                line.push('\n');
                Poll::Ready(Some(Ok(line.into_bytes().into())))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// `GET /events`: the standardized event endpoint (module doc = the wire
/// contract). Mounted by `serve`/`serve_with_events` under the same token
/// middleware as every other route; the shell bridge connects with the
/// per-component token exactly like the /api forwarding path.
pub fn events_router(notifier: A2uiNotifier) -> Router {
    Router::new()
        .route("/events", get(events_handler))
        .with_state(notifier)
}

async fn events_handler(State(notifier): State<A2uiNotifier>) -> Response {
    let stream = EventStream::spawn(&notifier);
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(EVENTS_CONTENT_TYPE),
        )],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn envelope_constructors_stamp_version_and_single_operation() {
        let msg = ServerToClientMessage::update_data_model(
            "xatodo",
            Some("/todos".into()),
            serde_json::json!([{ "title": "a" }]),
        );
        assert_eq!(msg.version, "v0.9.1");
        assert!(msg.has_single_operation());
        assert!(msg.create_surface.is_none());
        assert!(msg.delete_surface.is_none());

        let wire = serde_json::to_value(&msg).unwrap();
        // camelCase field names, aligned with slsdk-ts types.ts
        assert_eq!(
            wire,
            serde_json::json!({
                "version": "v0.9.1",
                "updateDataModel": {
                    "surfaceId": "xatodo",
                    "path": "/todos",
                    "value": [{ "title": "a" }]
                }
            })
        );
    }

    #[test]
    fn update_data_model_without_path_addresses_the_whole_model_on_the_wire() {
        let msg = ServerToClientMessage::update_data_model("s1", None, serde_json::json!({}));
        let wire = serde_json::to_value(&msg).unwrap();
        let op = wire.get("updateDataModel").unwrap();
        // path omitted = whole-model semantics (slsdk-ts applyMessage)
        assert!(op.get("path").is_none());
    }

    #[tokio::test]
    async fn push_reaches_subscriber_as_one_ndjson_line_and_drops_without_subscribers() {
        let notifier = A2uiNotifier::new();
        // No subscribers yet: push is a silent drop.
        notifier.push(ServerToClientMessage::delete_surface("s1"));
        assert_eq!(notifier.subscriber_count(), 0);

        let mut rx = notifier.subscribe();
        assert_eq!(notifier.subscriber_count(), 1);
        notifier.push(ServerToClientMessage::create_surface("s1", "superlive"));

        let line = rx.recv().await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["createSurface"]["surfaceId"], "s1");
        assert_eq!(parsed["createSurface"]["catalogId"], "superlive");

        // Dropping the receiver empties the count; dropping every clone
        // closes the channel (the stream ends — exercised below).
        drop(rx);
        assert_eq!(notifier.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn event_stream_yields_lines_and_ends_when_the_notifier_is_dropped() {
        let notifier = A2uiNotifier::new();
        // Subscribe first: pushes before any subscriber are dropped by
        // design (the no-replay contract), so the stream must exist before
        // the push can be observed.
        let mut body = axum::body::Body::from_stream(EventStream::spawn(&notifier));
        notifier.push(ServerToClientMessage::update_components(
            "s1",
            vec![serde_json::json!({ "id": "root" })],
        ));

        // One line per message, newline-terminated.
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert!(first.ends_with(b"\n"));
        let parsed: serde_json::Value = serde_json::from_slice(&first[..first.len() - 1]).unwrap();
        assert_eq!(parsed["updateComponents"]["components"][0]["id"], "root");

        // All notifier clones dropped → channel closed → bridge ends → the
        // body ends with no further frames.
        drop(notifier);
        let mut extra = 0;
        while body.frame().await.is_some() {
            extra += 1;
        }
        assert_eq!(extra, 0);
    }

    #[tokio::test]
    async fn close_ends_a_live_stream() {
        let notifier = A2uiNotifier::new();
        let mut body = axum::body::Body::from_stream(EventStream::spawn(&notifier));
        // The subscription is live: the pushed line reaches the stream while
        // the plane is open.
        notifier.push(ServerToClientMessage::delete_surface("s1"));
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .expect("the live push must reach the subscriber")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(first.ends_with(b"\n"));
        let parsed: serde_json::Value = serde_json::from_slice(&first[..first.len() - 1]).unwrap();
        assert_eq!(parsed["deleteSurface"]["surfaceId"], "s1");

        // close() fires: the stream ends promptly — without this, graceful
        // shutdown would wait for the in-flight chunked response forever.
        notifier.close();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while body.frame().await.is_some() {}
        })
        .await
        .expect("the stream must end after close");
    }

    #[tokio::test]
    async fn events_endpoint_answers_the_vendor_content_type_and_streams() {
        let notifier = A2uiNotifier::new();
        // Direct handler call: the route wiring (get + with_state) is trivial
        // and the dev-dependency set deliberately has no tower oneshot.
        let response = events_handler(State(notifier.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            EVENTS_CONTENT_TYPE
        );
        // The stream only ends when every notifier clone is dropped, so the
        // assertion reads frames as they arrive instead of collecting the
        // body to completion (which would wait forever here).
        notifier.push(ServerToClientMessage::delete_surface("s1"));
        let mut body = response.into_body();
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        let text = String::from_utf8_lossy(&first).into_owned();
        assert_eq!(
            text,
            "{\"version\":\"v0.9.1\",\"deleteSurface\":{\"surfaceId\":\"s1\"}}\n"
        );
        // Dropping the body tears down the consumer; the bridge task unwinds
        // on its next send.
    }
}
