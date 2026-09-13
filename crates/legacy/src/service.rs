//! Axum service exposing the legacy protocol: Express-style health page on
//! `/` and the socket.io v2 endpoint on `/socket.io/` (polling + websocket).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
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
    fn get_boards(&self) -> Vec<deckboard_db::BoardRow>;
    fn get_buttons_by_board(&self, board_id: i64) -> Vec<deckboard_db::ButtonRow>;
    fn get_button(&self, id: i64) -> Option<deckboard_db::ButtonRow>;
    fn exec(&self, button: deckboard_db::ButtonRow, is_tap_start: bool, sink: &mut dyn deckboard_actions::EventSink);
    fn slider(&self, button: deckboard_db::ButtonRow, value: f64);
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
        "<html><body><h3>Deckboard Server is live.</h3>\
         <p>Legacy socket.io v2 endpoint on /socket.io/ (port 8500).</p>\
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
    ws: Option<WebSocketUpgrade>,
) -> Response {
    let q = SioQuery::from(q);
    match (q.transport.as_deref(), ws) {
        (Some("websocket"), Some(ws)) => ws.on_upgrade(move |socket| ws_loop(state, socket, q)),
        _ => polling_get(state, q).await,
    }
}

async fn polling_get(state: Arc<AppState>, q: SioQuery) -> Response {
    match &q.sid {
        None => {
            let session = state.hub.create(q.access_key.as_deref() == Some(ACCESS_KEY_PRO)).await;
            // socket.io connect packet, delivered on the next poll
            session.send("40".into()).await;
            let open = json!({
                "sid": session.sid,
                "upgrades": ["websocket"],
                "pingInterval": 25000,
                "pingTimeout": 60000,
            });
            ([(header::CONTENT_TYPE, "text/plain; charset=UTF-8")], format!("0{open}")).into_response()
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
    body: String,
) -> Response {
    let q = SioQuery::from(q);
    let Some(sid) = q.sid.clone() else {
        return (StatusCode::BAD_REQUEST, "missing sid").into_response();
    };
    let Some(session) = state.hub.get(&sid).await else {
        return (StatusCode::BAD_REQUEST, "unknown sid").into_response();
    };
    let raw = decode_post_body(&body);
    for packet in raw.split('\u{1e}').filter(|p| !p.is_empty()) {
        handle_packet(&state, &session, packet).await;
    }
    StatusCode::OK.into_response()
}

/// Engine.IO v3 polling POSTs may arrive raw or form-encoded as `d=...`.
fn decode_post_body(body: &str) -> String {
    let body = body.strip_prefix("d=").unwrap_or(body);
    urldecode(body)
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
                            let name = v.as_array().and_then(|a| a.first()).and_then(|e| e.as_str()).unwrap_or("").to_string();
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

async fn handle_event(
    state: &Arc<AppState>,
    session: &Arc<Session>,
    name: &str,
    args: Vec<serde_json::Value>,
) {
    match name {
        "get_version" => {
            state
                .hub
                .broadcast("get_version", Some(r#"{"version":"1.6.0"}"#))
                .await;
        }
        "get_shortcuts" => {
            let mapper = Mapper::new();
            let boards: Vec<serde_json::Value> = state
                .backend
                .get_boards()
                .iter()
                .map(|b| {
                    let buttons = state.backend.get_buttons_by_board(b.id);
                    mapper.board_payload(b, &buttons, session.is_pro)
                })
                .collect();
            let payload = serde_json::to_string(&boards).unwrap_or_else(|_| "[]".into());
            session.send(event_packet("get_shortcuts", Some(&payload))).await;
        }
        "exec_shortcut" => {
            let arg = args.first().cloned().unwrap_or(json!({}));
            let id = arg.get("id").and_then(|v| {
                v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let is_tap_start = arg.get("isTapStart").and_then(|v| v.as_bool()).unwrap_or(false);
            let Some(id) = id else { return };
            let Some(button) = state.backend.get_button(id) else {
                tracing::debug!(id, "exec_shortcut: unknown id");
                return;
            };
            let (tx, mut rx) = mpsc::unbounded_channel::<i64>();
            struct Sink(tokio::sync::mpsc::UnboundedSender<i64>);
            impl deckboard_actions::EventSink for Sink {
                fn change_board(&mut self, board_id: i64) {
                    let _ = self.0.send(board_id);
                }
            }
            let mut sink = Sink(tx);
            state.backend.exec(button, is_tap_start, &mut sink);
            while let Ok(board_id) = rx.try_recv() {
                state
                    .hub
                    .broadcast("change_board", Some(&format!(r#"{{"boardId":{board_id}}}"#)))
                    .await;
            }
        }
        "exec_slider" => {
            let arg = args.first().cloned().unwrap_or(json!({}));
            let id = arg.get("id").and_then(|v| {
                v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            });
            let value = arg.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let Some(id) = id else { return };
            let Some(button) = state.backend.get_button(id) else {
                tracing::debug!(id, "exec_slider: unknown id");
                return;
            };
            state.backend.slider(button, value);
        }
        other => tracing::debug!(event = other, "unhandled client event"),
    }
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
            let s = state.hub.create(q.access_key.as_deref() == Some(ACCESS_KEY_PRO)).await;
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
