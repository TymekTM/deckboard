//! Axum routes for protocol v2: `/v2/ws` (auth checked at the upgrade),
//! `POST /v2/pair` (loopback-only one-time codes) and the asset store
//! `GET /assets/:hash` (docs/protocol-v2.md §3, §7).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use deckboard_proto::{Board, BoardsSync, Frame, MAX_FRAME_BYTES, TYPE_BOARDS_SYNC};
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
    pub hold_cap: Duration,
    /// Port clients reach this server on (QR payload / pairing logs).
    pub public_port: u16,
}

impl Default for V2Config {
    fn default() -> Self {
        V2Config {
            desktop_version: env!("CARGO_PKG_VERSION").to_string(),
            min_client: "0.0.0".to_string(),
            patch_interval: Duration::from_millis(100),
            ping_interval: Duration::from_secs(60),
            hello_timeout: Duration::from_secs(5),
            hold_cap: Duration::from_secs(120),
            public_port: 8500,
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
    /// `boards.sync` frame cached per generation: a reconnect with no board
    /// writes skips the SQLite scan and data-URL imports entirely.
    pub boards_cache: Mutex<Option<(u64, Arc<Frame>)>>,
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
        // Pairing codes are validated on the socket (pair-invalid /
        // pair-expired frames after hello), not at the upgrade.
        Some(Auth::Pair(code))
    } else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };
    let Some(ws) = ws else {
        return (StatusCode::BAD_REQUEST, "websocket required").into_response();
    };
    let auth = auth.expect("auth resolved above");
    // Cap what tungstenite buffers per message, with headroom above
    // MAX_FRAME_BYTES so frames between the protocol limit and the cap
    // still reach the app-level check that answers `error too-large`
    // (protocol-v2.md §2); beyond the cap the socket dies at the wire
    // level. Without any cap a client could park ~64 MiB per connection
    // before hearing `too-large`.
    ws.max_message_size(MAX_FRAME_BYTES + 64 * 1024)
        .on_upgrade(move |socket| session::run(state, socket, auth))
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
    // M1 has no desktop UI: log the QR-able URL so the operator can relay
    // it to the device by hand.
    let host = local_lan_ip()
        .await
        .unwrap_or_else(|| "127.0.0.1".to_string());
    tracing::info!(
        url = %format!("deckboard://{}:{}?pair={}", host, state.config.public_port, code),
        "pairing code minted - expires in 5 minutes"
    );
    Json(json!({
        "code": code,
        "expires_in": crate::devices::PAIR_CODE_TTL.as_secs(),
    }))
    .into_response()
}

/// Best-effort LAN address (the local end of the default route); never
/// sends a packet. Falls back to loopback when there is no route.
async fn local_lan_ip() -> Option<String> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok()?;
    socket.connect("8.8.8.8:80").await.ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
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
    // Asset files are arbitrarily large; the read must not run on the
    // async workers (the server runtime is single-threaded).
    let assets = state.assets.clone();
    let bytes = match tokio::task::spawn_blocking(move || assets.get(&hash)).await {
        Ok(Some(bytes)) => bytes,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CACHE_CONTROL,
                "immutable, max-age=31536000".to_string(),
            ),
        ],
        bytes,
    )
        .into_response()
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
        let frame = Frame::push_typed(
            deckboard_proto::TYPE_BOARDS_DELTA,
            &deckboard_proto::BoardsDelta { generation, ops },
        );
        self.hub.broadcast_frame(&frame);
        generation
    }

    /// The `boards.sync` frame for the current generation, built off the
    /// async workers. Serves from the per-generation cache when no board
    /// write happened since the last build (the common reconnect case).
    ///
    /// The build repeats while a write lands mid-pass, so the returned
    /// frame's generation is never older than a `boards.delta` the caller
    /// may already have queued: sessions attach to the hub before calling
    /// this, and op replay is idempotent, so a snapshot that already
    /// contains a pending delta's write is safe to deliver after it.
    pub async fn boards_snapshot(&self) -> (Arc<Frame>, u64) {
        let cached = self
            .boards_cache
            .lock()
            .expect("boards cache poisoned")
            .clone();
        if let Some((generation, frame)) = cached {
            if generation == self.generation.get() {
                return (frame, generation);
            }
        }

        let mut generation = self.generation.get();
        let mut boards = self.build_boards_blocking().await;
        // Rebuild while a write lands mid-pass. Four retries bound the
        // work under sustained editing (the M3 editor publishing batches
        // back to back); past the bound we serve the freshest build and
        // the next delta or boards.sync heals the client (§4 recovery).
        for _ in 0..4 {
            let after = self.generation.get();
            if after == generation {
                break;
            }
            generation = after;
            boards = self.build_boards_blocking().await;
        }
        let frame = Arc::new(Frame::push_typed(
            TYPE_BOARDS_SYNC,
            &BoardsSync { generation, boards },
        ));
        *self.boards_cache.lock().expect("boards cache poisoned") =
            Some((generation, frame.clone()));
        (frame, generation)
    }

    async fn build_boards_blocking(&self) -> Vec<Board> {
        let backend = self.backend.clone();
        let assets = self.assets.clone();
        let engine = self.engine.clone();
        tokio::task::spawn_blocking(move || {
            crate::boards::build_boards(backend.as_ref(), &assets, &engine)
        })
        .await
        .expect("boards build panicked")
    }
}
