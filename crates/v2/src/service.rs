//! Axum routes for protocol v2: `/v2/ws` (auth checked at the upgrade),
//! `POST /v2/pair` (loopback-only one-time codes) and the asset store
//! `GET /assets/:hash` (docs/protocol-v2.md §3, §7).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use pulpit_db::MAX_BOARD_DIM;
use pulpit_proto::{Board, BoardsSync, Frame, MAX_FRAME_BYTES, TYPE_BOARDS_SYNC};
use serde_json::json;

use pulpit_legacy::Backend;

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
    /// M8 Bluetooth-style pairing requests (plan 014).
    pub pair_requests: crate::devices::PairRequests,
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
        .route("/v2/pair-request", post(pair_request_create))
        .route("/v2/pair-request/:id", get(pair_request_status))
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
    headers: HeaderMap,
    ws: Option<WebSocketUpgrade>,
) -> Response {
    if !pulpit_legacy::origin_host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "browser requests are not allowed").into_response();
    }
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
/// QR payload (`pulpit://<host>:<port>?pair=<code>`) is a desktop-UI
/// concern - it knows the address the client should reach.
async fn pair_create(
    State(state): State<Arc<V2State>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "pairing codes are local-only").into_response();
    }
    if !pulpit_legacy::origin_host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "browser requests are not allowed").into_response();
    }
    let code = state.pairing.new_code();
    // The code is delivered to the loopback caller in the response body
    // and never written to the log: logs outlive the 5-minute TTL by
    // weeks and pairing auto-accepts, so a logged code is a standing
    // invite (audit B2). The message is a static string so the secret
    // cannot be interpolated into it by accident.
    tracing::info!("{}", pair_minted_message());
    Json(json!({
        "code": code,
        "expires_in": crate::devices::PAIR_CODE_TTL.as_secs(),
    }))
    .into_response()
}

/// The `POST /v2/pair` log line - deliberately takes no code argument
/// so the secret cannot leak into it. Public so the desktop's mint
/// command logs the exact same code-free text (audit B2 step 1).
pub fn pair_minted_message() -> &'static str {
    "pairing code minted - expires in 5 minutes (code suppressed in logs)"
}

/// M8 Bluetooth-style pairing, step 1 (plan 014): a LAN tablet that
/// discovered this desktop over mDNS asks to pair. The server mints the
/// ordinary one-time code and asks the operator through the
/// pair-request gate; the verification code goes back to the TABLET in
/// the response (it shows the same number the desktop dialog shows).
/// The response never waits for the operator - the tablet polls
/// `GET /v2/pair-request/:id` (step 2) while the dialog is up. One live
/// request at a time; browser-initiated requests are refused like the
/// rest of the API (B1).
#[derive(serde::Deserialize)]
struct PairRequestBody {
    name: String,
}

async fn pair_request_create(
    State(state): State<Arc<V2State>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairRequestBody>,
) -> Response {
    if addr.ip().is_loopback() {
        // The point of the flow is a remote tablet; a loopback caller is
        // the desktop itself, which has the native dialog already.
        return (StatusCode::FORBIDDEN, "use the desktop pairing UI").into_response();
    }
    if !pulpit_legacy::origin_host_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "browser requests are not allowed").into_response();
    }
    let name =
        session::sanitize_device_name(Some(&body.name)).unwrap_or_else(|| "Device".to_string());
    let id = hex_string_16();
    let code = state.pairing.new_code();
    let Some(request) = state
        .pair_requests
        .begin(id.clone(), code.clone(), name.clone())
    else {
        return (
            StatusCode::CONFLICT,
            Json(json!({"code": "request-in-flight"})),
        )
            .into_response();
    };
    let pairing = state.pairing.clone();
    let decision_request = request.clone();
    let pairing_code = code.clone();
    tokio::spawn(async move {
        let gate = pairing.request_gate();
        let decision = tokio::task::spawn_blocking(move || match gate {
            Some(gate) => gate(&decision_request.name, &decision_request.code),
            None => {
                tracing::warn!(name = %decision_request.name, "pair-request auto-accepted (no gate installed)");
                true
            }
        })
        .await
        .unwrap_or(false);
        use crate::devices::PairDecision;
        request.set_decision(if decision {
            // the operator just compared this code on the dialog: the
            // hello that consumes it must not ask a second time
            pairing.pre_approve(&pairing_code);
            PairDecision::Approved
        } else {
            PairDecision::Rejected
        });
        tracing::info!(name = %request.name, approved = decision, "pair-request decided");
    });
    // The minted log line stays code-free, like `POST /v2/pair`.
    tracing::info!("{}", pair_minted_message());
    (
        StatusCode::CREATED,
        Json(json!({
            "request_id": id,
            "code": code,
            "expires_in_secs": crate::devices::PAIR_CODE_TTL.as_secs(),
        })),
    )
        .into_response()
}

/// M8 step 2: the tablet polls for the operator's decision. Terminal
/// decisions clear the slot so the next request can start at once; a
/// polled-approved response is the client's cue to open
/// `/v2/ws?pair=<code>` with the code it already shows.
async fn pair_request_status(
    State(state): State<Arc<V2State>>,
    Path(id): Path<String>,
) -> Response {
    let Some(request) = state.pair_requests.get(&id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"status": "unknown"}))).into_response();
    };
    if request.age() >= state.pair_requests.ttl() {
        state.pair_requests.reset(&id);
        return (StatusCode::OK, Json(json!({"status": "expired"}))).into_response();
    }
    use crate::devices::PairDecision;
    match request.decision() {
        PairDecision::Pending => Json(json!({"status": "pending"})).into_response(),
        PairDecision::Approved => {
            state.pair_requests.reset(&id);
            Json(json!({"status": "approved"})).into_response()
        }
        PairDecision::Rejected => {
            state.pair_requests.reset(&id);
            Json(json!({"status": "rejected"})).into_response()
        }
    }
}

/// Random request id: hex so it survives any logging/casing untouched.
fn hex_string_16() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| format!("{:02x}", rng.gen::<u8>()))
        .collect()
}

async fn asset_get(
    State(state): State<Arc<V2State>>,
    Path(hash): Path<String>,
    Query(q): Query<WsQuery>,
    headers: HeaderMap,
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
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // Asset files are arbitrarily large; the read must not run on the
    // async workers (the server runtime is single-threaded).
    let assets = state.assets.clone();
    let body =
        match tokio::task::spawn_blocking(move || assets.read_for_serving(&hash, range.as_deref()))
            .await
        {
            Ok(Ok(body)) => body,
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
    // Every response advertises byte ranges; a satisfied window adds its
    // Content-Range, an unsatisfiable request gets the 416 form.
    let mut head = HeaderMap::new();
    head.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    head.insert(
        header::CACHE_CONTROL,
        "immutable, max-age=31536000".parse().unwrap(),
    );
    head.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    match body {
        crate::assets::AssetBody::Full(bytes) => (head, bytes).into_response(),
        crate::assets::AssetBody::Window {
            start,
            end_incl,
            total,
            bytes,
        } => {
            head.insert(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end_incl}/{total}").parse().unwrap(),
            );
            // a satisfied range is partial content: 200 with a
            // Content-Range header is spec-invalid and clients treat the
            // body as the whole asset
            (StatusCode::PARTIAL_CONTENT, head, bytes).into_response()
        }
        crate::assets::AssetBody::Unsatisfiable(total) => (
            [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            StatusCode::RANGE_NOT_SATISFIABLE,
        )
            .into_response(),
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
    pub fn publish_delta(&self, ops: Vec<pulpit_proto::BoardOp>) -> u64 {
        let generation = self.generation.bump();
        let frame = Frame::push_typed(
            pulpit_proto::TYPE_BOARDS_DELTA,
            &pulpit_proto::BoardsDelta { generation, ops },
        );
        self.hub.broadcast_frame(&frame);
        generation
    }

    /// `tile-set` op for one committed tile write, built from the
    /// post-commit DB row. `None` when the row is already gone (delete).
    pub fn tile_set_op(&self, board_id: i64, tile_id: i64) -> Option<pulpit_proto::BoardOp> {
        let row = self.backend.get_button(tile_id)?;
        let names = crate::boards::board_names(self.backend.as_ref());
        let mut tile = crate::boards::build_tile(&row, &names, &self.assets, &self.engine);
        // Same placement bound as the boards builder (DESK-03): the delta
        // must not carry an off-grid tile after a board shrink. A board
        // row that is already gone still bounds the tile by MAX_BOARD_DIM.
        let (width, height) = match self.backend.get_board(board_id) {
            Some(board) => (board.width, board.height),
            None => (MAX_BOARD_DIM, MAX_BOARD_DIM),
        };
        crate::boards::clamp_tile_to_board(&mut tile, width, height);
        Some(pulpit_proto::BoardOp::TileSet {
            board: board_id,
            tile: Box::new(tile),
        })
    }

    /// `board-set` op for one committed board write (create, rename,
    /// resize, background), with its current tiles. `None` when the board
    /// is already gone.
    pub fn board_set_op(&self, board_id: i64) -> Option<pulpit_proto::BoardOp> {
        let board = self.backend.get_board(board_id)?;
        let buttons = self.backend.get_buttons_by_board(board_id);
        let names = crate::boards::board_names(self.backend.as_ref());
        Some(pulpit_proto::BoardOp::BoardSet {
            board: crate::boards::build_board(&board, &buttons, &names, &self.assets, &self.engine),
        })
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
