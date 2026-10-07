//! Axum service exposing the legacy protocol: Express-style health page on
//! `/` and the socket.io v2 endpoint on `/socket.io/` (polling + websocket).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;

use crate::hub::{event_packet, Hub, Session, WsOut, ACCESS_KEY_PRO};
use crate::mapping::Mapper;

/// Storage + execution seam so the transport layer stays testable.
pub trait Backend: Send + Sync + 'static {
    fn get_boards(&self) -> Vec<pulpit_db::BoardRow>;
    fn get_board(&self, board_id: i64) -> Option<pulpit_db::BoardRow>;
    fn get_buttons_by_board(&self, board_id: i64) -> Vec<pulpit_db::ButtonRow>;
    /// Every shortcut grouped by board id, for whole-board reads. The
    /// default loops the per-board getter (fine for mocks); real backends
    /// override it with a single grouped query.
    fn all_buttons_by_board(&self) -> std::collections::HashMap<i64, Vec<pulpit_db::ButtonRow>> {
        self.get_boards()
            .iter()
            .map(|board| (board.id, self.get_buttons_by_board(board.id)))
            .collect()
    }
    fn get_button(&self, id: i64) -> Option<pulpit_db::ButtonRow>;
    /// Image-less row for per-event paths (gesture checks, exec dispatch).
    /// Defaults to the full row for backends without a cheaper query.
    fn get_button_meta(&self, id: i64) -> Option<pulpit_db::ButtonRow> {
        self.get_button(id)
    }
    fn exec(
        &self,
        button: pulpit_db::ButtonRow,
        is_tap_start: bool,
        sink: &mut dyn pulpit_actions::EventSink,
    );
    /// Execute an interaction gesture on a button (default calls `exec(button, false, sink)`).
    fn exec_gesture(
        &self,
        button: pulpit_db::ButtonRow,
        gesture: &str,
        sink: &mut dyn pulpit_actions::EventSink,
    ) {
        let _ = gesture;
        self.exec(button, false, sink);
    }
    fn slider(&self, button: pulpit_db::ButtonRow, value: f64);
    /// M2 speaker watcher snapshots: master volume percent, muted flag.
    /// Defaults suit backends without speaker support.
    fn speaker_status(&self) -> (Option<f32>, Option<bool>) {
        (None, None)
    }
    /// Endpoint id of the current default playback device.
    fn speaker_device_id(&self) -> Option<String> {
        None
    }
    /// Watcher snapshot in one platform pass: volume, mute and (when
    /// `want_device`) the default device id. Per-tick loops use this;
    /// the default composes the getters for backends without a combined
    /// read.
    fn speaker_snapshot(&self, want_device: bool) -> (Option<f32>, Option<bool>, Option<String>) {
        let (volume, muted) = self.speaker_status();
        let device = if want_device {
            self.speaker_device_id()
        } else {
            None
        };
        (volume, muted, device)
    }
}

pub struct AppState {
    pub hub: Arc<Hub>,
    pub backend: Arc<dyn Backend>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(health_page))
        .route("/socket.io/", get(socket_get).post(socket_post))
        .with_state(state)
}

async fn health_page() -> Html<String> {
    Html(
        "<html><body><h3>Pulpit server is live.</h3>\
         <p>Legacy socket.io v2 endpoint on /socket.io/.</p>\
         </body></html>"
            .into(),
    )
}

#[derive(Clone)]
struct SioQuery {
    sid: Option<String>,
    access_key: Option<String>,
    transport: Option<String>,
}

impl From<HashMap<String, String>> for SioQuery {
    fn from(mut m: HashMap<String, String>) -> Self {
        // Dart client posts the path as /socket.io (no trailing slash)
        SioQuery {
            sid: m.remove("sid"),
            access_key: m.remove("access_key"),
            transport: m.remove("transport"),
        }
    }
}

async fn socket_get(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: Option<WebSocketUpgrade>,
) -> Response {
    if !origin_host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "browser requests are not allowed").into_response();
    }
    let q = SioQuery::from(q);
    match (q.transport.as_deref(), ws) {
        (Some("websocket"), Some(ws)) => ws.on_upgrade(move |socket| ws_loop(state, socket, q)),
        _ => polling_get(state, q).await,
    }
}

async fn polling_get(state: Arc<AppState>, q: SioQuery) -> Response {
    match &q.sid {
        None => {
            let session = state
                .hub
                .create(
                    state.backend.clone(),
                    q.access_key.as_deref() == Some(ACCESS_KEY_PRO),
                )
                .await;
            // socket.io connect packet, delivered on the next poll
            session.send("40".into()).await;
            let open = json!({
                "sid": session.sid,
                "upgrades": ["websocket"],
                "pingInterval": 25000,
                "pingTimeout": 60000,
            });
            (
                [(header::CONTENT_TYPE, "text/plain; charset=UTF-8")],
                format!("0{open}"),
            )
                .into_response()
        }
        Some(sid) => match state.hub.get(sid).await {
            Some(session) => {
                let body = session.poll(20_000).await;
                ([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], body).into_response()
            }
            None => (StatusCode::BAD_REQUEST, "unknown sid").into_response(),
        },
    }
}

async fn socket_post(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !origin_host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "browser requests are not allowed").into_response();
    }
    let q = SioQuery::from(q);
    let Some(sid) = q.sid.clone() else {
        return (StatusCode::BAD_REQUEST, "missing sid").into_response();
    };
    let Some(session) = state.hub.get(&sid).await else {
        return (StatusCode::BAD_REQUEST, "unknown sid").into_response();
    };
    session.touch().await;
    let raw = decode_post_body(&body);
    for packet in raw.split('\u{1e}').filter(|p| !p.is_empty()) {
        handle_packet(&state, &session, packet).await;
    }
    StatusCode::OK.into_response()
}

/// Engine.IO v3 polling POSTs may arrive raw or form-encoded as `d=...`.
/// Only the `d=`-encoded form is urldecoded: a raw body is already plain
/// packets, and decoding it would corrupt every `+` and `%XX` it happens
/// to contain.
fn decode_post_body(body: &str) -> String {
    match body.strip_prefix("d=") {
        Some(encoded) => urldecode(encoded),
        None => body.to_string(),
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn handle_packet(state: &Arc<AppState>, session: &Arc<Session>, packet: &str) {
    let mut chars = packet.chars();
    match chars.next() {
        Some('2') => session.send("3".into()).await, // engine ping -> pong
        Some('5') => {}                              // engine upgrade over ws: no-op
        Some('4') => {
            let sio = packet[1..].to_string();
            let mut sc = sio.chars();
            match sc.next() {
                Some('0') => {} // namespace connect from client: nothing to do
                Some('2') => {
                    let data = &sio[1..];
                    match serde_json::from_str::<serde_json::Value>(data) {
                        Ok(v) => {
                            let name = v
                                .as_array()
                                .and_then(|a| a.first())
                                .and_then(|e| e.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args: Vec<serde_json::Value> = v
                                .as_array()
                                .map(|a| a.iter().skip(1).cloned().collect())
                                .unwrap_or_default();
                            handle_event(state, session, &name, args).await;
                        }
                        Err(e) => tracing::warn!(packet = data, err = %e, "bad sio event"),
                    }
                }
                Some(other) => tracing::debug!(sio_type = %other, "unhandled sio packet"),
                None => {}
            }
        }
        Some(other) => tracing::debug!(eio_type = %other, "unhandled engine packet"),
        None => {}
    }
}

/// `exec_shortcut`/`exec_slider` accept the id as number or string.
fn arg_id(arg: &serde_json::Value) -> Option<i64> {
    arg.get("id").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

/// SQLite tile lookup off the async workers; a panic in the read logs
/// instead of vanishing into a swallowed JoinError. Uses the image-less
/// [`Backend::get_button_meta`] row (CORE-02): exec dispatch never reads
/// `img`/`img2`, and every tap/slide must not materialize multi-MB
/// base64 columns.
async fn get_button_blocking(state: &Arc<AppState>, id: i64) -> Option<pulpit_db::ButtonRow> {
    let backend = state.backend.clone();
    match tokio::task::spawn_blocking(move || backend.get_button_meta(id)).await {
        Ok(button) => button,
        Err(e) => {
            tracing::error!(id, error = %e, "button lookup panicked");
            None
        }
    }
}

async fn handle_event(
    state: &Arc<AppState>,
    session: &Arc<Session>,
    name: &str,
    args: Vec<serde_json::Value>,
) {
    match name {
        "get_version" => {
            // the original answers with io.emit - a broadcast, not a reply
            state
                .hub
                .broadcast("get_version", Some(crate::LEGACY_VERSION_PACKET))
                .await;
        }
        "get_shortcuts" => {
            // Board reads hit SQLite; the server runtime is
            // single-threaded, so they run on the blocking pool. One
            // grouped query for every board's shortcuts (NET-09).
            let backend = state.backend.clone();
            let is_pro = session.is_pro;
            let boards = match tokio::task::spawn_blocking(move || {
                let mapper = Mapper::new();
                let buttons = backend.all_buttons_by_board();
                backend
                    .get_boards()
                    .iter()
                    .map(|b| {
                        let rows = buttons.get(&b.id);
                        mapper.board_payload(b, rows.map(Vec::as_slice).unwrap_or(&[]), is_pro)
                    })
                    .collect::<Vec<serde_json::Value>>()
            })
            .await
            {
                Ok(boards) => boards,
                Err(e) => {
                    tracing::error!(error = %e, "boards read panicked");
                    Vec::new()
                }
            };
            let payload = serde_json::to_string(&boards).unwrap_or_else(|_| "[]".into());
            session
                .send(event_packet("get_shortcuts", Some(&payload)))
                .await;
        }
        "exec_shortcut" => {
            let arg = args.first().cloned().unwrap_or(json!({}));
            let Some(id) = arg_id(&arg) else { return };
            let is_tap_start = arg
                .get("isTapStart")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let Some(button) = get_button_blocking(state, id).await else {
                tracing::debug!(id, "exec_shortcut: unknown id");
                return;
            };
            // tap-start holds keys down for key tiles; track it so every
            // teardown path (socket drop, poll silence) can release it
            // when no tap end arrives
            if button.kind == "key" {
                if is_tap_start {
                    session.hold_key(button.clone()).await;
                } else {
                    session.key_released(id).await;
                }
            }
            tracing::info!(id, kind = %button.kind, "exec_shortcut");
            let (tx, mut rx) = mpsc::unbounded_channel::<i64>();
            let (val_tx, mut val_rx) = mpsc::unbounded_channel::<(String, String)>();
            let (third_tx, mut third_rx) = mpsc::unbounded_channel::<(String, String)>();
            struct Sink(
                tokio::sync::mpsc::UnboundedSender<i64>,
                tokio::sync::mpsc::UnboundedSender<(String, String)>,
                tokio::sync::mpsc::UnboundedSender<(String, String)>,
            );
            impl pulpit_actions::EventSink for Sink {
                fn change_board(&mut self, board_id: i64) {
                    let _ = self.0.send(board_id);
                }
                fn app_value(&mut self, key: &str, value: &str) {
                    let _ = self.1.send((key.to_string(), value.to_string()));
                }
                fn third_party_value(&mut self, key: &str, value: &str) {
                    let _ = self.2.send((key.to_string(), value.to_string()));
                }
            }
            let mut sink = Sink(tx, val_tx, third_tx);
            // The exec runs detached, not awaited: multiaction delays,
            // url fetches and Discord re-auth can take seconds to minutes,
            // and the packet loop must keep answering engine pings in the
            // meantime or the client times out and disconnects.
            let backend = state.backend.clone();
            let hub = state.hub.clone();
            tokio::spawn(async move {
                // actions may sleep (multiaction delays): keep them off the
                // async workers
                let _ = tokio::task::spawn_blocking(move || {
                    backend.exec(button, is_tap_start, &mut sink)
                })
                .await;
                while let Ok(board_id) = rx.try_recv() {
                    hub.broadcast(
                        "change_board",
                        Some(&format!(r#"{{"boardId":{board_id}}}"#)),
                    )
                    .await;
                }
                while let Ok((key, value)) = val_rx.try_recv() {
                    let data = serde_json::json!({ key: value }).to_string();
                    let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
                    hub.broadcast("app_status_update", Some(&payload)).await;
                }
                while let Ok((key, value)) = third_rx.try_recv() {
                    let data = serde_json::json!({ key: value }).to_string();
                    let payload = format!(r#"{{"app":"THIRD_PARTY_APP","data":{data}}}"#);
                    hub.broadcast("app_status_update", Some(&payload)).await;
                }
            });
        }
        "exec_slider" => {
            let arg = args.first().cloned().unwrap_or(json!({}));
            let Some(id) = arg_id(&arg) else { return };
            let value = arg.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let Some(button) = get_button_blocking(state, id).await else {
                tracing::debug!(id, "exec_slider: unknown id");
                return;
            };
            let backend = state.backend.clone();
            let _ = tokio::task::spawn_blocking(move || backend.slider(button, value)).await;
        }
        // M5 widget kit: clients declare the templates they render; the
        // payload today is mode-driven, the manifest tailoring lands with
        // protocol v2
        "client_capabilities" => {
            tracing::info!(session = %session.sid, caps = %args.first().map(|v| v.to_string()).unwrap_or_default(), "client capabilities");
        }
        other => tracing::debug!(event = other, "unhandled client event"),
    }
}

/// Browser-origin guard for the socket endpoints (shared with protocol
/// v2; see the audit's B1). Browsers always attach an `Origin` header to
/// cross-origin WebSocket/XHR requests and cannot be told to drop it, so
/// a page open on any machine that can reach the port is identifiable:
/// reject whenever `Origin` is present and does not match the `Host` it
/// connected to. Native clients (stock Deckboard app, pulpit-mobile)
/// send no `Origin`. `Host` itself must be an IP literal or `localhost`
/// (the shapes the QR payloads and manual entry produce) - anything
/// else is what a DNS-rebinding attack produces, and is rejected even
/// without an `Origin`.
pub fn origin_host_allowed(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if !host_is_direct(host) {
        return false;
    }
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(origin) => origin_authority(origin).is_some_and(|a| a.eq_ignore_ascii_case(host)),
    }
}

/// `host` / `host:port` / `[v6]:port` with an IP literal or `localhost`.
fn host_is_direct(host: &str) -> bool {
    let bare = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        match host.rsplit_once(':') {
            Some((h, port)) if !port.is_empty() && port.parse::<u16>().is_ok() => h,
            _ => host,
        }
    };
    bare.parse::<std::net::IpAddr>().is_ok() || bare.eq_ignore_ascii_case("localhost")
}

/// The `host[:port]` part of an `Origin` value, if it has one.
fn origin_authority(origin: &str) -> Option<&str> {
    let rest = origin.split_once("://")?.1;
    let end = rest.find('/').unwrap_or(rest.len());
    Some(&rest[..end])
}

async fn ws_loop(state: Arc<AppState>, socket: WebSocket, q: SioQuery) {
    let (mut tx, mut rx) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsOut>();

    let session = match &q.sid {
        Some(sid) => match state.hub.get(sid).await {
            Some(s) => {
                s.upgrade_to_ws(out_tx.clone()).await;
                s
            }
            None => {
                tracing::warn!(sid, "ws upgrade for unknown sid");
                return;
            }
        },
        None => {
            // websocket-only session: open packet + connect go over the wire
            let s = state
                .hub
                .create(
                    state.backend.clone(),
                    q.access_key.as_deref() == Some(ACCESS_KEY_PRO),
                )
                .await;
            s.upgrade_to_ws(out_tx.clone()).await;
            let open = json!({
                "sid": s.sid,
                "upgrades": ["websocket"],
                "pingInterval": 25000,
                "pingTimeout": 60000,
            });
            s.send(format!("0{open}")).await;
            s.send("40".into()).await;
            s
        }
    };
    let sid = session.sid.clone();

    // outbound pump
    let pump = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            match msg {
                WsOut::Packet(p) => {
                    if tx.send(Message::Text(p)).await.is_err() {
                        break;
                    }
                }
                WsOut::Closed => break,
            }
        }
    });

    while let Some(msg) = rx.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                session.touch().await;
                // EIO=3 upgrade probe, then plain packets
                match text.as_str() {
                    "2probe" => {
                        let _ = out_tx.send(WsOut::Packet("3probe".into()));
                    }
                    "5" => {}
                    other => handle_packet(&state, &session, other).await,
                }
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Ok(_) => {}
            Err(_) => break,
        }
    }
    pump.abort();
    state.hub.remove(&sid).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_body_decoding_respects_the_d_prefix() {
        // Dart client form: urlencoded after `d=`
        assert_eq!(decode_post_body("d=42%5B%5D"), "42[]");
        assert_eq!(decode_post_body("d=a+b"), "a b");
        // Raw bodies pass through untouched: `+` and `%` are literal here.
        assert_eq!(
            decode_post_body(r#"42["exec","a+b"]"#),
            r#"42["exec","a+b"]"#
        );
        assert_eq!(decode_post_body("42123%+5"), "42123%+5");
    }
}
