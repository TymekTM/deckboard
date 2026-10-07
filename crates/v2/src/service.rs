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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pulpit_db::{BoardRow, ButtonRow};
    use tokio::sync::mpsc;

    use crate::session::WsOut;

    #[derive(Default)]
    struct Mock {
        boards: Mutex<Vec<BoardRow>>,
        buttons: Mutex<Vec<ButtonRow>>,
        board_reads: AtomicUsize,
    }

    impl Backend for Mock {
        fn get_boards(&self) -> Vec<BoardRow> {
            self.board_reads.fetch_add(1, Ordering::Relaxed);
            self.boards.lock().unwrap().clone()
        }
        fn get_board(&self, id: i64) -> Option<BoardRow> {
            self.boards
                .lock()
                .unwrap()
                .iter()
                .find(|b| b.id == id)
                .cloned()
        }
        fn get_buttons_by_board(&self, board_id: i64) -> Vec<ButtonRow> {
            self.buttons
                .lock()
                .unwrap()
                .iter()
                .filter(|b| b.board_id == board_id)
                .cloned()
                .collect()
        }
        fn get_button(&self, id: i64) -> Option<ButtonRow> {
            self.buttons
                .lock()
                .unwrap()
                .iter()
                .find(|b| b.id == id)
                .cloned()
        }
        fn exec(&self, _: ButtonRow, _: bool, _: &mut dyn pulpit_actions::EventSink) {}
        fn slider(&self, _: ButtonRow, _: f64) {}
    }

    fn board(id: i64, width: i64, height: i64) -> BoardRow {
        BoardRow {
            id,
            name: format!("Board {id}"),
            background: "#112233".into(),
            layout: 6,
            image: String::new(),
            sort: 0,
            kind: "buttons".into(),
            args: None,
            order: id,
            width,
            height,
            converted: 1,
        }
    }

    fn tile(id: i64, board_id: i64, x: i64, y: i64, w: i64, h: i64) -> ButtonRow {
        ButtonRow {
            id,
            board_id,
            kind: "url".into(),
            title: Some(format!("t{id}")),
            command: Some("https://example.com".into()),
            x: Some(x),
            y: Some(y),
            w,
            h,
            mode: "button".into(),
            ..ButtonRow::default()
        }
    }

    struct Fixture {
        state: Arc<V2State>,
        backend: Arc<Mock>,
        _dir: tempfile::TempDir,
    }

    fn fixture_with(pair_requests: crate::devices::PairRequests) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(Mock::default());
        backend.boards.lock().unwrap().push(board(1, 4, 3));
        backend
            .buttons
            .lock()
            .unwrap()
            .push(tile(10, 1, 0, 0, 1, 1));
        let state = Arc::new(V2State {
            hub: Arc::new(V2Hub::new()),
            backend: backend.clone(),
            devices: Arc::new(DeviceStore::load(dir.path().join("devices.json")).unwrap()),
            pairing: Arc::new(Pairing::new()),
            assets: Arc::new(AssetStore::open(dir.path().join("assets")).unwrap()),
            engine: Arc::new(StateEngine::new(120)),
            generation: Generation::starting_at(1),
            boards_cache: Default::default(),
            pair_requests,
            config: V2Config::default(),
        });
        Fixture {
            state,
            backend,
            _dir: dir,
        }
    }

    fn fixture() -> Fixture {
        fixture_with(Default::default())
    }

    fn lan() -> ConnectInfo<SocketAddr> {
        ConnectInfo("192.168.1.20:50000".parse().unwrap())
    }

    fn loopback() -> ConnectInfo<SocketAddr> {
        ConnectInfo("127.0.0.1:50000".parse().unwrap())
    }

    fn host(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, value.parse().unwrap());
        h
    }

    fn browser() -> HeaderMap {
        let mut h = host("192.168.1.5:8611");
        h.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        h
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn token_query(token: Option<&str>) -> Query<WsQuery> {
        Query(WsQuery {
            token: token.map(str::to_string),
            pair: None,
        })
    }

    // -- defaults and helpers ----------------------------------------------

    #[test]
    fn config_defaults_match_the_protocol() {
        let c = V2Config::default();
        assert_eq!(c.desktop_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(c.min_client, "0.0.0");
        assert_eq!(c.patch_interval, Duration::from_millis(100));
        assert_eq!(c.ping_interval, Duration::from_secs(60));
        assert_eq!(c.hello_timeout, Duration::from_secs(5));
        assert_eq!(c.hold_cap, Duration::from_secs(120));
    }

    #[test]
    fn request_ids_are_32_lowercase_hex_chars() {
        let a = hex_string_16();
        assert_eq!(a.len(), 32);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_ne!(a, hex_string_16());
    }

    #[test]
    fn minted_message_is_static_and_code_free() {
        let msg = pair_minted_message();
        assert!(msg.contains("suppressed"));
        // the only digit in it is the TTL ("5 minutes")
        assert_eq!(msg.chars().filter(char::is_ascii_digit).count(), 1);
    }

    // -- /v2/ws upgrade gate (no upgrade available in a unit test) ---------

    #[tokio::test]
    async fn ws_connect_rejects_browsers_before_auth() {
        let f = fixture();
        let resp = ws_connect(State(f.state), token_query(None), browser(), None).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ws_connect_needs_a_known_token_or_a_pair_code() {
        let f = fixture();
        let resp = ws_connect(
            State(f.state.clone()),
            token_query(None),
            host("10.0.0.2:8611"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let resp = ws_connect(
            State(f.state),
            token_query(Some("bogus")),
            host("10.0.0.2:8611"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn ws_connect_with_valid_auth_but_no_upgrade_is_a_bad_request() {
        let f = fixture();
        let (_, token) = f.state.devices.create("Tab");
        let resp = ws_connect(
            State(f.state.clone()),
            token_query(Some(&token)),
            host("10.0.0.2:8611"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // pair codes are checked on the socket, so any code passes the gate
        let resp = ws_connect(
            State(f.state),
            Query(WsQuery {
                token: None,
                pair: Some("000000".into()),
            }),
            host("10.0.0.2:8611"),
            None,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // -- POST /v2/pair -----------------------------------------------------

    #[tokio::test]
    async fn pair_create_is_loopback_only() {
        let f = fixture();
        let resp = pair_create(State(f.state), lan(), host("192.168.1.5:8611")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn pair_create_refuses_browser_pages_on_loopback() {
        let f = fixture();
        let mut h = host("127.0.0.1:8611");
        h.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        let resp = pair_create(State(f.state), loopback(), h).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn pair_create_mints_a_usable_code() {
        let f = fixture();
        let resp = pair_create(State(f.state.clone()), loopback(), host("127.0.0.1:8611")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert_eq!(body["expires_in"], crate::devices::PAIR_CODE_TTL.as_secs());
        let code = body["code"].as_str().unwrap();
        assert!(f.state.pairing.peek(code).is_ok());
    }

    // -- /v2/pair-request --------------------------------------------------

    fn pair_body(name: &str) -> Json<PairRequestBody> {
        Json(PairRequestBody { name: name.into() })
    }

    async fn poll_status(state: &Arc<V2State>, id: &str) -> (StatusCode, serde_json::Value) {
        let resp = pair_request_status(State(state.clone()), Path(id.to_string())).await;
        let status = resp.status();
        (status, body_json(resp).await)
    }

    async fn poll_until_decided(state: &Arc<V2State>, id: &str) -> serde_json::Value {
        for _ in 0..400 {
            let (status, body) = poll_status(state, id).await;
            assert_eq!(status, StatusCode::OK);
            if body["status"] != "pending" {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("pair request never decided");
    }

    #[tokio::test]
    async fn pair_request_refuses_loopback_and_browsers_without_taking_the_slot() {
        let f = fixture();
        let resp = pair_request_create(
            State(f.state.clone()),
            loopback(),
            host("127.0.0.1:8611"),
            pair_body("Tab"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let resp =
            pair_request_create(State(f.state.clone()), lan(), browser(), pair_body("Tab")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let resp = pair_request_create(
            State(f.state),
            lan(),
            host("192.168.1.5:8611"),
            pair_body("Tab"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pair_request_flow_with_an_approving_gate() {
        let f = fixture();
        let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let seen_gate = seen.clone();
        f.state.pairing.set_pair_request_gate(move |name, code| {
            seen_gate
                .lock()
                .unwrap()
                .push((name.to_string(), code.to_string()));
            true
        });
        let resp = pair_request_create(
            State(f.state.clone()),
            lan(),
            host("192.168.1.5:8611"),
            pair_body("Kitchen\ntablet"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp).await;
        let id = body["request_id"].as_str().unwrap().to_string();
        let code = body["code"].as_str().unwrap().to_string();
        assert_eq!(id.len(), 32);
        assert_eq!(
            body["expires_in_secs"],
            crate::devices::PAIR_CODE_TTL.as_secs()
        );

        // a second request while the first is live conflicts
        let resp = pair_request_create(
            State(f.state.clone()),
            lan(),
            host("192.168.1.5:8611"),
            pair_body("Other"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(resp).await["code"], "request-in-flight");

        assert_eq!(
            poll_until_decided(&f.state, &id).await["status"],
            "approved"
        );
        // the gate saw a sanitized name and the code the tablet shows
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert!(!seen[0].0.contains('\n'), "{:?}", seen[0].0);
        assert_eq!(seen[0].1, code);
        // the approved code skips the second trust prompt
        assert!(f.state.pairing.take_pre_approved(&code));
        // the terminal answer freed the slot
        let (status, body) = poll_status(&f.state, &id).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], "unknown");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pair_request_rejected_by_the_operator() {
        let f = fixture();
        f.state.pairing.set_pair_request_gate(|_, _| false);
        let resp = pair_request_create(
            State(f.state.clone()),
            lan(),
            host("192.168.1.5:8611"),
            pair_body("Tab"),
        )
        .await;
        let body = body_json(resp).await;
        let id = body["request_id"].as_str().unwrap().to_string();
        let code = body["code"].as_str().unwrap().to_string();
        assert_eq!(
            poll_until_decided(&f.state, &id).await["status"],
            "rejected"
        );
        assert!(!f.state.pairing.take_pre_approved(&code));
        // the slot is free again
        let resp = pair_request_create(
            State(f.state),
            lan(),
            host("192.168.1.5:8611"),
            pair_body("Tab"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn pair_request_status_reports_expiry_and_unknown_ids() {
        let f = fixture_with(crate::devices::PairRequests::with_ttl(Duration::ZERO));
        let (status, body) = poll_status(&f.state, "nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["status"], "unknown");
        f.state
            .pair_requests
            .begin("abc".into(), "123456".into(), "Tab".into())
            .unwrap();
        let (status, body) = poll_status(&f.state, "abc").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "expired");
        assert!(f.state.pair_requests.get("abc").is_none());
    }

    #[tokio::test]
    async fn pair_request_status_pending_keeps_the_slot() {
        let f = fixture();
        f.state
            .pair_requests
            .begin("abc".into(), "123456".into(), "Tab".into())
            .unwrap();
        assert_eq!(poll_status(&f.state, "abc").await.1["status"], "pending");
        assert_eq!(poll_status(&f.state, "abc").await.1["status"], "pending");
        assert!(f.state.pair_requests.get("abc").is_some());
    }

    // -- GET /assets/:hash -------------------------------------------------

    #[tokio::test]
    async fn assets_need_a_known_token() {
        let f = fixture();
        let hash = f.state.assets.import_bytes(b"png-bytes", "png").unwrap();
        let resp = asset_get(
            State(f.state.clone()),
            Path(hash.clone()),
            token_query(None),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let resp = asset_get(
            State(f.state),
            Path(hash),
            token_query(Some("bogus")),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn assets_404_for_malformed_or_unknown_hashes() {
        let f = fixture();
        let (_, token) = f.state.devices.create("Tab");
        for hash in ["../devices.json".to_string(), "abc".into(), "f".repeat(64)] {
            let resp = asset_get(
                State(f.state.clone()),
                Path(hash.clone()),
                token_query(Some(&token)),
                HeaderMap::new(),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{hash}");
        }
    }

    async fn get_range(f: &Fixture, token: &str, hash: &str, range: Option<&str>) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(range) = range {
            headers.insert(header::RANGE, range.parse().unwrap());
        }
        asset_get(
            State(f.state.clone()),
            Path(hash.to_string()),
            token_query(Some(token)),
            headers,
        )
        .await
    }

    #[tokio::test]
    async fn assets_serve_full_partial_and_unsatisfiable_bodies() {
        let f = fixture();
        let (_, token) = f.state.devices.create("Tab");
        let hash = f.state.assets.import_bytes(b"0123456789", "png").unwrap();

        let resp = get_range(&f, &token, &hash, None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(resp.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(
            resp.headers()[header::CACHE_CONTROL],
            "immutable, max-age=31536000"
        );
        assert!(resp.headers().get(header::CONTENT_RANGE).is_none());
        assert_eq!(body_bytes(resp).await, b"0123456789");

        let resp = get_range(&f, &token, &hash, Some("bytes=2-4")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 2-4/10");
        assert_eq!(body_bytes(resp).await, b"234");

        let resp = get_range(&f, &token, &hash, Some("bytes=-3")).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes 7-9/10");
        assert_eq!(body_bytes(resp).await, b"789");

        let resp = get_range(&f, &token, &hash, Some("bytes=50-")).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(resp.headers()[header::CONTENT_RANGE], "bytes */10");
    }

    #[tokio::test]
    async fn assets_removed_from_the_store_answer_404() {
        let f = fixture();
        let (_, token) = f.state.devices.create("Tab");
        let hash = f.state.assets.import_bytes(b"x", "png").unwrap();
        assert!(f.state.assets.remove(&hash));
        let resp = get_range(&f, &token, &hash, None).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // -- V2State write path ------------------------------------------------

    fn attached(state: &V2State) -> (Arc<crate::hub::V2Session>, mpsc::Receiver<WsOut>) {
        let (tx, rx) = mpsc::channel(16);
        let session = state.hub.create(tx);
        state.hub.attach(&session);
        (session, rx)
    }

    fn next_frame(rx: &mut mpsc::Receiver<WsOut>) -> Frame {
        match rx.try_recv() {
            Ok(WsOut::Text(text)) => serde_json::from_str(&text).unwrap(),
            Ok(_) => panic!("expected a text frame"),
            Err(e) => panic!("no frame queued: {e:?}"),
        }
    }

    #[test]
    fn publish_delta_bumps_the_generation_and_broadcasts() {
        let f = fixture();
        let (_session, mut rx) = attached(&f.state);
        assert_eq!(f.state.generation(), 1);
        let op = pulpit_proto::BoardOp::TileRemove { board: 1, tile: 10 };
        assert_eq!(f.state.publish_delta(vec![op.clone()]), 2);
        assert_eq!(f.state.generation(), 2);
        let frame = next_frame(&mut rx);
        assert_eq!(frame.kind, pulpit_proto::TYPE_BOARDS_DELTA);
        let delta: pulpit_proto::BoardsDelta =
            serde_json::from_value(frame.payload.unwrap()).unwrap();
        assert_eq!(delta.generation, 2);
        assert_eq!(delta.ops, vec![op]);
        assert_eq!(f.state.publish_delta(Vec::new()), 3);
    }

    #[test]
    fn publish_delta_skips_sessions_that_never_attached() {
        let f = fixture();
        let (tx, mut rx) = mpsc::channel(4);
        let _pending = f.state.hub.create(tx);
        f.state.publish_delta(Vec::new());
        assert!(rx.try_recv().is_err());
    }

    fn tile_of(op: pulpit_proto::BoardOp) -> (i64, pulpit_proto::Tile) {
        match op {
            pulpit_proto::BoardOp::TileSet { board, tile } => (board, *tile),
            other => panic!("expected tile-set, got {other:?}"),
        }
    }

    fn placement(x: u32, y: u32, w: u32, h: u32) -> pulpit_proto::Placement {
        pulpit_proto::Placement { x, y, w, h }
    }

    #[test]
    fn tile_set_op_builds_the_committed_row() {
        let f = fixture();
        let (board, tile) = tile_of(f.state.tile_set_op(1, 10).unwrap());
        assert_eq!(board, 1);
        assert_eq!(tile.id, 10);
        assert_eq!(tile.placement, placement(0, 0, 1, 1));
    }

    #[test]
    fn tile_set_op_is_none_for_a_deleted_row() {
        let f = fixture();
        assert!(f.state.tile_set_op(1, 999).is_none());
    }

    #[test]
    fn tile_set_op_clamps_off_grid_rows_to_the_board() {
        let f = fixture();
        f.backend
            .buttons
            .lock()
            .unwrap()
            .push(tile(11, 1, 9, 9, 3, 2));
        let (_, t) = tile_of(f.state.tile_set_op(1, 11).unwrap());
        // 4x3 board: a 3x2 tile can start at most at (1,1)
        assert_eq!(t.placement, placement(1, 1, 3, 2));
    }

    #[test]
    fn tile_set_op_for_a_vanished_board_bounds_by_max_dim() {
        let f = fixture();
        f.backend
            .buttons
            .lock()
            .unwrap()
            .push(tile(12, 77, 100, 100, 50, 1));
        let (board, t) = tile_of(f.state.tile_set_op(77, 12).unwrap());
        assert_eq!(board, 77);
        let max = MAX_BOARD_DIM as u32;
        assert_eq!(t.placement, placement(0, max - 1, max, 1));
    }

    #[test]
    fn board_set_op_carries_the_board_and_its_tiles() {
        let f = fixture();
        match f.state.board_set_op(1).unwrap() {
            pulpit_proto::BoardOp::BoardSet { board } => {
                assert_eq!(board.id, 1);
                assert_eq!(board.name, "Board 1");
                assert_eq!((board.width, board.height), (4, 3));
                assert_eq!(board.tiles.len(), 1);
                assert_eq!(board.tiles[0].id, 10);
            }
            other => panic!("expected board-set, got {other:?}"),
        }
        assert!(f.state.board_set_op(404).is_none());
    }

    // -- boards.sync snapshot cache ----------------------------------------

    fn sync_of(frame: &Frame) -> BoardsSync {
        assert_eq!(frame.kind, TYPE_BOARDS_SYNC);
        serde_json::from_value(frame.payload.clone().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn snapshot_is_cached_per_generation() {
        let f = fixture();
        let (first, generation) = f.state.boards_snapshot().await;
        assert_eq!(generation, 1);
        let sync = sync_of(&first);
        assert_eq!(sync.generation, 1);
        assert_eq!(sync.boards.len(), 1);
        let reads = f.backend.board_reads.load(Ordering::Relaxed);
        let (second, _) = f.state.boards_snapshot().await;
        assert!(Arc::ptr_eq(&first, &second));
        // served from the cache: no storage reads at all
        assert_eq!(f.backend.board_reads.load(Ordering::Relaxed), reads);
    }

    #[tokio::test]
    async fn snapshot_rebuilds_after_a_published_write() {
        let f = fixture();
        let (first, _) = f.state.boards_snapshot().await;
        f.backend.boards.lock().unwrap().push(board(2, 2, 2));
        f.state.publish_delta(Vec::new());
        let (second, generation) = f.state.boards_snapshot().await;
        assert_eq!(generation, 2);
        assert!(!Arc::ptr_eq(&first, &second));
        let sync = sync_of(&second);
        assert_eq!(sync.generation, 2);
        assert_eq!(
            sync.boards.iter().map(|b| b.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
