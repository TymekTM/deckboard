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

    #[test]
    fn urldecode_handles_escapes_and_malformed_percent() {
        assert_eq!(urldecode("%41%42c"), "ABc");
        assert_eq!(urldecode("%7B%22a%22%3A1%7D"), r#"{"a":1}"#);
        // invalid hex keeps the percent literally
        assert_eq!(urldecode("%zz1"), "%zz1");
        // a truncated escape at the end stays as-is
        assert_eq!(urldecode("ab%4"), "ab%4");
        assert_eq!(urldecode("ab%"), "ab%");
        // multi-byte UTF-8 split over escapes is reassembled
        assert_eq!(urldecode("%C5%BC"), "\u{17c}");
        // invalid UTF-8 degrades lossily instead of panicking
        assert_eq!(urldecode("%FF"), "\u{fffd}");
        assert_eq!(urldecode(""), "");
    }

    #[test]
    fn arg_id_accepts_numbers_and_numeric_strings() {
        assert_eq!(arg_id(&json!({"id": 7})), Some(7));
        assert_eq!(arg_id(&json!({"id": "42"})), Some(42));
        assert_eq!(arg_id(&json!({"id": "x"})), None);
        assert_eq!(arg_id(&json!({"id": 1.5})), None);
        assert_eq!(arg_id(&json!({"id": null})), None);
        assert_eq!(arg_id(&json!({})), None);
        assert_eq!(arg_id(&json!(7)), None);
    }

    #[test]
    fn sio_query_picks_the_known_keys() {
        let q = SioQuery::from(HashMap::from([
            ("sid".to_string(), "abc".to_string()),
            ("access_key".to_string(), ACCESS_KEY_PRO.to_string()),
            ("transport".to_string(), "websocket".to_string()),
            ("EIO".to_string(), "3".to_string()),
        ]));
        assert_eq!(q.sid.as_deref(), Some("abc"));
        assert_eq!(q.access_key.as_deref(), Some(ACCESS_KEY_PRO));
        assert_eq!(q.transport.as_deref(), Some("websocket"));
        let empty = SioQuery::from(HashMap::new());
        assert!(empty.sid.is_none() && empty.access_key.is_none() && empty.transport.is_none());
    }

    #[test]
    fn host_is_direct_accepts_ip_literals_and_localhost_only() {
        for host in [
            "192.168.1.5",
            "192.168.1.5:8611",
            "127.0.0.1:1",
            "localhost",
            "LOCALHOST:8611",
            "[::1]:8611",
            "[fe80::1]",
        ] {
            assert!(host_is_direct(host), "{host} should be direct");
        }
        for host in [
            "evil.example",
            "evil.example:8611",
            "localhost.evil.example",
            "192.168.1.5.nip.io",
            "",
            "[evil]:80",
            "1.2.3.4:notaport",
            // an IPv6 Host must be bracketed (RFC 3986)
            "::1",
        ] {
            assert!(!host_is_direct(host), "{host} should be rejected");
        }
    }

    #[test]
    fn origin_authority_extracts_host_and_port() {
        assert_eq!(origin_authority("http://1.2.3.4:8611"), Some("1.2.3.4:8611"));
        assert_eq!(origin_authority("https://a.b/path?q"), Some("a.b"));
        assert_eq!(origin_authority("null"), None);
        assert_eq!(origin_authority("file://"), Some(""));
    }

    fn headers(host: Option<&str>, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(host) = host {
            h.insert(header::HOST, host.parse().unwrap());
        }
        if let Some(origin) = origin {
            h.insert(header::ORIGIN, origin.parse().unwrap());
        }
        h
    }

    #[test]
    fn origin_guard_matrix() {
        // native clients: no Origin, direct Host
        assert!(origin_host_allowed(&headers(Some("10.0.0.2:8611"), None)));
        // missing Host is never allowed
        assert!(!origin_host_allowed(&headers(None, None)));
        // DNS rebinding: a name in Host is rejected even without Origin
        assert!(!origin_host_allowed(&headers(
            Some("attacker.example:8611"),
            None
        )));
        // same-origin page served by this server
        assert!(origin_host_allowed(&headers(
            Some("10.0.0.2:8611"),
            Some("http://10.0.0.2:8611")
        )));
        // case-insensitive comparison
        assert!(origin_host_allowed(&headers(
            Some("LOCALHOST:8611"),
            Some("http://localhost:8611")
        )));
        // cross-origin browser page
        assert!(!origin_host_allowed(&headers(
            Some("10.0.0.2:8611"),
            Some("http://evil.example")
        )));
        // same host, different port is a different origin
        assert!(!origin_host_allowed(&headers(
            Some("10.0.0.2:8611"),
            Some("http://10.0.0.2:9999")
        )));
        // opaque origins (sandboxed iframes, file://) are rejected
        assert!(!origin_host_allowed(&headers(
            Some("10.0.0.2:8611"),
            Some("null")
        )));
    }

    // -- packet handling ---------------------------------------------------

    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct Recorder {
        execs: StdMutex<Vec<(i64, bool)>>,
        sliders: StdMutex<Vec<(i64, f64)>>,
        meta_reads: StdMutex<Vec<i64>>,
    }

    fn board(id: i64, name: &str) -> pulpit_db::BoardRow {
        pulpit_db::BoardRow {
            id,
            name: name.into(),
            background: "#000000".into(),
            layout: 6,
            image: String::new(),
            sort: 0,
            kind: "buttons".into(),
            args: None,
            order: 0,
            width: 2,
            height: 2,
            converted: 1,
        }
    }

    fn button(id: i64, kind: &str) -> pulpit_db::ButtonRow {
        pulpit_db::ButtonRow {
            id,
            board_id: 1,
            kind: kind.into(),
            title: Some(format!("t{id}")),
            x: Some(0),
            y: Some(0),
            w: 1,
            h: 1,
            mode: "button".into(),
            ..pulpit_db::ButtonRow::default()
        }
    }

    impl Backend for Recorder {
        fn get_boards(&self) -> Vec<pulpit_db::BoardRow> {
            vec![board(1, "One"), board(2, "Two")]
        }
        fn get_board(&self, id: i64) -> Option<pulpit_db::BoardRow> {
            (id == 1 || id == 2).then(|| board(id, "B"))
        }
        fn get_buttons_by_board(&self, board_id: i64) -> Vec<pulpit_db::ButtonRow> {
            if board_id == 1 {
                vec![button(10, "url")]
            } else {
                Vec::new()
            }
        }
        fn get_button(&self, _id: i64) -> Option<pulpit_db::ButtonRow> {
            panic!("dispatch must use the image-less meta row");
        }
        fn get_button_meta(&self, id: i64) -> Option<pulpit_db::ButtonRow> {
            self.meta_reads.lock().unwrap().push(id);
            match id {
                10 => Some(button(10, "url")),
                11 => Some(button(11, "key")),
                12 => Some(button(12, "board")),
                _ => None,
            }
        }
        fn exec(
            &self,
            button: pulpit_db::ButtonRow,
            is_tap_start: bool,
            sink: &mut dyn pulpit_actions::EventSink,
        ) {
            self.execs.lock().unwrap().push((button.id, is_tap_start));
            if button.id == 12 {
                sink.change_board(2);
                sink.app_value("k", "v");
                sink.third_party_value("tp", "1");
            }
        }
        fn slider(&self, button: pulpit_db::ButtonRow, value: f64) {
            self.sliders.lock().unwrap().push((button.id, value));
        }
    }

    async fn fixture(pro: bool) -> (Arc<AppState>, Arc<Session>, Arc<Recorder>) {
        let rec = Arc::new(Recorder::default());
        let state = Arc::new(AppState {
            hub: Arc::new(Hub::new()),
            backend: rec.clone(),
        });
        let session = state.hub.create(rec.clone(), pro).await;
        (state, session, rec)
    }

    async fn drain(session: &Session) -> Vec<String> {
        let body = session.poll(1).await;
        if body.is_empty() {
            Vec::new()
        } else {
            body.split('\u{1e}').map(str::to_string).collect()
        }
    }

    /// Detached execs finish on the blocking pool; wait for the side
    /// effects instead of sleeping a fixed time.
    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..400 {
            if done() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("condition never became true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn engine_ping_answers_pong_and_noise_is_ignored() {
        let (state, session, rec) = fixture(false).await;
        for packet in [
            "2",
            "5",
            "40",
            "4",
            "41",
            "9junk",
            "",
            "42{not json",
            r#"42["nobody_listens",{}]"#,
            "42{}",
        ] {
            handle_packet(&state, &session, packet).await;
        }
        assert_eq!(drain(&session).await, vec!["3"]);
        assert!(rec.execs.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_version_is_broadcast_to_every_session() {
        let (state, session, rec) = fixture(false).await;
        let other = state.hub.create(rec, true).await;
        handle_packet(&state, &session, r#"42["get_version"]"#).await;
        let expected = format!(r#"42["get_version",{}]"#, crate::LEGACY_VERSION_PACKET);
        assert_eq!(drain(&session).await, vec![expected.clone()]);
        assert_eq!(drain(&other).await, vec![expected]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_shortcuts_replies_only_to_the_asking_session() {
        let (state, session, rec) = fixture(true).await;
        let other = state.hub.create(rec, false).await;
        handle_packet(&state, &session, r#"42["get_shortcuts"]"#).await;
        let packets = drain(&session).await;
        assert_eq!(packets.len(), 1);
        let body = packets[0]
            .strip_prefix(r#"42["get_shortcuts","#)
            .and_then(|p| p.strip_suffix(']'))
            .unwrap();
        let boards: serde_json::Value = serde_json::from_str(body).unwrap();
        let boards = boards.as_array().unwrap();
        assert_eq!(boards.len(), 2);
        assert_eq!(boards[0]["name"], "One");
        assert_eq!(boards[1]["name"], "Two");
        assert!(drain(&other).await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exec_shortcut_runs_detached_and_forwards_sink_events() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(&state, &session, r#"42["exec_shortcut",{"id":"12"}]"#).await;
        until(|| rec.execs.lock().unwrap().len() == 1).await;
        assert_eq!(rec.execs.lock().unwrap()[0], (12, false));
        let mut packets = Vec::new();
        for _ in 0..400 {
            packets.extend(drain(&session).await);
            if packets.len() >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            packets,
            vec![
                r#"42["change_board",{"boardId":2}]"#.to_string(),
                r#"42["app_status_update",{"app":"APP_CUSTOM_VALUE","data":{"k":"v"}}]"#
                    .to_string(),
                r#"42["app_status_update",{"app":"THIRD_PARTY_APP","data":{"tp":"1"}}]"#
                    .to_string(),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exec_shortcut_ignores_missing_or_unknown_ids() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(&state, &session, r#"42["exec_shortcut"]"#).await;
        handle_packet(&state, &session, r#"42["exec_shortcut",{"id":"nope"}]"#).await;
        handle_packet(&state, &session, r#"42["exec_shortcut",{"id":999}]"#).await;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(rec.execs.lock().unwrap().is_empty());
        // only the parsable id reached storage
        assert_eq!(*rec.meta_reads.lock().unwrap(), vec![999]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn key_tap_start_is_released_when_the_session_goes_away() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(
            &state,
            &session,
            r#"42["exec_shortcut",{"id":11,"isTapStart":true}]"#,
        )
        .await;
        until(|| rec.execs.lock().unwrap().len() == 1).await;
        state.hub.remove(&session.sid).await;
        assert_eq!(*rec.execs.lock().unwrap(), vec![(11, true), (11, false)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn key_tap_end_clears_the_held_key() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(
            &state,
            &session,
            r#"42["exec_shortcut",{"id":11,"isTapStart":true}]"#,
        )
        .await;
        handle_packet(
            &state,
            &session,
            r#"42["exec_shortcut",{"id":11,"isTapStart":false}]"#,
        )
        .await;
        until(|| rec.execs.lock().unwrap().len() == 2).await;
        state.hub.remove(&session.sid).await;
        // no third (release) exec: the tap end already released it
        assert_eq!(rec.execs.lock().unwrap().len(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_key_tap_start_is_not_tracked_as_held() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(
            &state,
            &session,
            r#"42["exec_shortcut",{"id":10,"isTapStart":true}]"#,
        )
        .await;
        until(|| rec.execs.lock().unwrap().len() == 1).await;
        state.hub.remove(&session.sid).await;
        assert_eq!(*rec.execs.lock().unwrap(), vec![(10, true)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exec_slider_passes_the_value_and_defaults_to_zero() {
        let (state, session, rec) = fixture(false).await;
        handle_packet(&state, &session, r#"42["exec_slider",{"id":10,"value":0.75}]"#).await;
        handle_packet(&state, &session, r#"42["exec_slider",{"id":"10"}]"#).await;
        handle_packet(&state, &session, r#"42["exec_slider",{"id":404,"value":1}]"#).await;
        handle_packet(&state, &session, r#"42["exec_slider",{}]"#).await;
        assert_eq!(*rec.sliders.lock().unwrap(), vec![(10, 0.75), (10, 0.0)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn client_capabilities_is_accepted_silently() {
        let (state, session, _rec) = fixture(false).await;
        handle_packet(
            &state,
            &session,
            r#"42["client_capabilities",{"templates":["media"]}]"#,
        )
        .await;
        assert!(drain(&session).await.is_empty());
    }
}
