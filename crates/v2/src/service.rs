//! Axum routes for protocol v2: `/v2/ws` (auth checked at the upgrade),
//! `POST /v2/pair` (loopback-only one-time codes) and the asset store
//! `GET /assets/:hash` (docs/protocol-v2.md §3, §7).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use deckboard_proto::Frame;
use serde_json::json;

use deckboard_legacy::Backend;

use crate::assets::AssetStore;
use crate::devices::{DeviceEntry, DeviceStore, Pairing};
use crate::hub::V2Hub;
use crate::state::StateEngine;
use crate::{session, Generation};

/// Tunables; tests shrink the timers.
pub struct V2Config {
    pub desktop_version: String,
    pub min_client: String,
    pub patch_interval: Duration,
    pub ping_interval: Duration,
    pub hello_timeout: Duration,
    /// Close a connection silent longer than this (covers the zombie case).
    pub watchdog_timeout: Duration,
    pub hold_cap: Duration,
}

impl Default for V2Config {
    fn default() -> Self {
        V2Config {
            desktop_version: env!("CARGO_PKG_VERSION").to_string(),
            min_client: "0.0.0".to_string(),
            patch_interval: Duration::from_millis(100),
            ping_interval: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(5),
            watchdog_timeout: Duration::from_secs(90),
            hold_cap: Duration::from_secs(120),
        }
    }
}

pub struct V2State {
    pub hub: Arc<V2Hub>,
    pub backend: Arc<dyn Backend>,
    pub devices: Arc<DeviceStore>,
    pub pairing: Arc<Pairing>,
    pub assets: Arc<AssetStore>,
    pub engine: Arc<StateEngine>,
    pub generation: Generation,
    pub config: V2Config,
}

/// Authenticated at the upgrade, before any frame flows.
pub enum Auth {
    Device(DeviceEntry),
    /// One-time pairing code, consumed on successful hello.
    Pair(String),
}

pub fn router(state: Arc<V2State>) -> Router {
    Router::new()
        .route("/v2/ws", get(ws_connect))
        .route("/v2/pair", post(pair_create))
        .route("/assets/:hash", get(asset_get))
        .with_state(state)
}

#[derive(Default, serde::Deserialize)]
struct WsQuery {
    token: Option<String>,
    pair: Option<String>,
}

async fn ws_connect(
    State(state): State<Arc<V2State>>,
    Query(q): Query<WsQuery>,
    ws: Option<WebSocketUpgrade>,
) -> Response {
    let auth = if let Some(token) = q.token {
        match state.devices.verify(&token) {
            Some(device) => Some(Auth::Device(device)),
            None => return (StatusCode::UNAUTHORIZED, "unknown device token").into_response(),
        }
    } else if let Some(code) = q.pair {
        match state.pairing.peek(&code) {
            Ok(()) => Some(Auth::Pair(code)),
            Err(_) => return (StatusCode::UNAUTHORIZED, "invalid or expired pairing code").into_response(),
        }
    } else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };
    let Some(ws) = ws else {
        return (StatusCode::BAD_REQUEST, "websocket required").into_response();
    };
    let auth = auth.expect("auth resolved above");
    ws.on_upgrade(move |socket| session::run(state, socket, auth))
}

/// Mints a one-time pairing code. Loopback callers only: the server binds
/// all interfaces, but codes are a local desktop decision. Building the
/// QR payload (`deckboard://<host>:<port>?pair=<code>`) is a desktop-UI
/// concern - it knows the address the client should reach.
async fn pair_create(
    State(state): State<Arc<V2State>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    if !addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "pairing codes are local-only").into_response();
    }
    let code = state.pairing.new_code();
    Json(json!({
        "code": code,
        "expires_in": crate::devices::PAIR_CODE_TTL.as_secs(),
    }))
    .into_response()
}

async fn asset_get(
    State(state): State<Arc<V2State>>,
    Path(hash): Path<String>,
    Query(q): Query<WsQuery>,
) -> Response {
    let Some(token) = q.token.as_deref() else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };
    if state.devices.verify(token).is_none() {
        return (StatusCode::UNAUTHORIZED, "unknown device token").into_response();
    }
    if !crate::assets::is_valid_hash(&hash) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(content_type) = state.assets.content_type(&hash) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match state.assets.get(&hash) {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CACHE_CONTROL, "immutable, max-age=31536000".to_string()),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

impl V2State {
    /// Board generation at last read (starts at 1).
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Publishes one committed write batch: bumps the generation and
    /// broadcasts `boards.delta`. Synchronous on purpose - the editor's
    /// write path calls this in-process right after its DB commit.
    pub fn publish_delta(&self, ops: Vec<deckboard_proto::BoardOp>) -> u64 {
        let generation = self.generation.bump();
        let frame = Frame::push(
            deckboard_proto::TYPE_BOARDS_DELTA,
            serde_json::to_value(&deckboard_proto::BoardsDelta { generation, ops })
                .unwrap_or(serde_json::Value::Null),
        );
        self.hub.broadcast_frame(&frame);
        generation
    }
}
