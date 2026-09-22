//! End-to-end protocol tests: a real axum server + real WebSocket clients
//! (tokio-tungstenite) against a mock backend. Covers auth/pairing, the
//! welcome -> boards.sync -> state.sync handshake, patches, interactions,
//! hold-to-repeat, board.open, deltas, pings and assets.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use deckboard_actions::EventSink;
use deckboard_db::{BoardRow, ButtonRow};
use deckboard_legacy::Backend;
use deckboard_proto::*;
use deckboard_v2::{
    AssetStore, DeviceStore, Generation, Pairing, StateEngine, V2Config, V2Hub, V2State,
};

// ---------------------------------------------------------------------------
// fixtures

#[derive(Clone, Default)]
struct MockBackend {
    boards: Vec<BoardRow>,
    buttons: Vec<ButtonRow>,
    execs: Arc<Mutex<Vec<(i64, bool)>>>,
    sliders: Arc<Mutex<Vec<(i64, f64)>>>,
}

impl MockBackend {
    fn exec_count(&self) -> usize {
        self.execs.lock().unwrap().len()
    }
}

impl Backend for MockBackend {
    fn get_boards(&self) -> Vec<BoardRow> {
        self.boards.clone()
    }
    fn get_board(&self, board_id: i64) -> Option<BoardRow> {
        self.boards.iter().find(|b| b.id == board_id).cloned()
    }
    fn get_buttons_by_board(&self, board_id: i64) -> Vec<ButtonRow> {
        self.buttons
            .iter()
            .filter(|b| b.board_id == board_id)
            .cloned()
            .collect()
    }
    fn get_button(&self, id: i64) -> Option<ButtonRow> {
        self.buttons.iter().find(|b| b.id == id).cloned()
    }
    fn exec(&self, button: ButtonRow, is_tap_start: bool, sink: &mut dyn EventSink) {
        self.execs.lock().unwrap().push((button.id, is_tap_start));
        if button.kind == "board" {
            sink.change_board(9);
        }
        if button.kind == "value-pusher" {
            sink.app_value("fake-key", "42");
        }
    }
    fn slider(&self, button: ButtonRow, value: f64) {
        self.sliders.lock().unwrap().push((button.id, value));
    }
}

fn board_row() -> BoardRow {
    BoardRow {
        id: 3,
        name: "Media".into(),
        background: "#2c3e50".into(),
        layout: 6,
        image: String::new(),
        sort: 0,
        kind: "buttons".into(),
        args: None,
        order: 0,
        width: 4,
        height: 3,
        converted: 1,
    }
}

fn button_row(
    id: i64,
    kind: &str,
    mode: &str,
    command: Option<&str>,
    options: Option<String>,
) -> ButtonRow {
    ButtonRow {
        id,
        board_id: 3,
        kind: kind.into(),
        command: command.map(str::to_string),
        title: Some("T".into()),
        title_position: 0,
        title_color: None,
        title_box_color: None,
        color: None,
        icon_color: None,
        icon_color2: None,
        border_color: None,
        shape: 0,
        icon: None,
        img: None,
        img2: None,
        icon2: None,
        color2: None,
        shape2: 0,
        border_color2: None,
        title_position2: 0,
        title_box_color2: None,
        title_color2: None,
        position: None,
        position2: 0,
        mode: mode.into(),
        x: Some(0),
        y: Some(0),
        w: 1,
        h: 1,
        options,
    }
}

fn sample_backend() -> MockBackend {
    MockBackend {
        boards: vec![board_row()],
        buttons: vec![
            button_row(17, "vol", "button", Some("vol_mute"), None),
            button_row(21, "speaker-volume", "slider", None, None),
            button_row(22, "si-cpu", "graph", None, None),
            button_row(
                23,
                "key",
                "button",
                Some("A"),
                Some(r#"{"hold":{"repeat":{"delay_ms":20,"interval_ms":20}}}"#.into()),
            ),
            button_row(24, "board", "button", Some(r#"{"id":2}"#), None),
        ],
        ..Default::default()
    }
}

fn test_state(
    backend: MockBackend,
    tune: impl FnOnce(&mut V2Config),
) -> (Arc<V2State>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = V2Config {
        patch_interval: Duration::from_millis(20),
        ping_interval: Duration::from_millis(300),
        hello_timeout: Duration::from_secs(2),
        ..Default::default()
    };
    tune(&mut config);
    let state = Arc::new(V2State {
        hub: Arc::new(V2Hub::new()),
        backend: Arc::new(backend),
        devices: Arc::new(DeviceStore::load(dir.path().join("devices.json")).unwrap()),
        pairing: Arc::new(Pairing::new()),
        assets: Arc::new(AssetStore::open(dir.path().join("assets")).unwrap()),
        engine: Arc::new(StateEngine::new(120)),
        generation: Generation::starting_at(1),
        boards_cache: Default::default(),
        config,
    });
    (state, dir)
}

async fn spawn_server(state: Arc<V2State>) -> SocketAddr {
    tokio::spawn(deckboard_v2::run_flusher(
        state.engine.clone(),
        state.hub.clone(),
        state.config.patch_interval,
    ));
    let app = deckboard_v2::router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn ws_open(url: &str) -> Ws {
    let (stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .unwrap_or_else(|e| panic!("connect {url}: {e}"));
    stream
}

async fn send_frame(ws: &mut Ws, frame: &Frame) {
    ws.send(tungstenite_text(frame)).await.unwrap();
}

fn tungstenite_text(frame: &Frame) -> tokio_tungstenite::tungstenite::Message {
    tokio_tungstenite::tungstenite::Message::Text(serde_json::to_string(frame).unwrap())
}

async fn next_frame(ws: &mut Ws) -> Frame {
    loop {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                break serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("bad frame {text}: {e}"))
            }
            Some(Ok(_)) => continue, // pings, pongs
            other => panic!("ws stream ended: {other:?}"),
        }
    }
}

/// Sends hello and consumes the full post-welcome burst: welcome +
/// boards.sync + state.sync, in that order.
async fn handshake(ws: &mut Ws, client: &str, version: &str) -> (Welcome, BoardsSync, StateSync) {
    send_frame(
        ws,
        &Frame::request(
            TYPE_HELLO,
            "h1",
            serde_json::json!({"client": client, "version": version, "name": "Test tablet"}),
        ),
    )
    .await;
    let welcome: Welcome = typed(next_frame(ws).await, TYPE_WELCOME);
    let sync: BoardsSync = typed(next_frame(ws).await, TYPE_BOARDS_SYNC);
    assert_eq!(sync.boards.len(), 1);
    let state_sync: StateSync = typed(next_frame(ws).await, TYPE_STATE_SYNC);
    (welcome, sync, state_sync)
}

fn typed<T: serde::de::DeserializeOwned>(frame: Frame, kind: &str) -> T {
    assert_eq!(frame.kind, kind, "unexpected frame kind");
    serde_json::from_value(frame.payload.unwrap()).unwrap()
}

async fn http_request(
    addr: SocketAddr,
    request: String,
) -> (u16, HashMap<String, String>, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    (status, headers, buf[pos + 4..].to_vec())
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, HashMap<String, String>, Vec<u8>) {
    http_request(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn http_post_json(
    addr: SocketAddr,
    path: &str,
    body: &str,
) -> (u16, HashMap<String, String>, Vec<u8>) {
    http_request(
        addr,
        format!(
            "POST {path} HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

// ---------------------------------------------------------------------------
// tests

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_ws_is_rejected() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let addr = spawn_server(state).await;
    let result = tokio_tungstenite::connect_async(format!("ws://{addr}/v2/ws")).await;
    assert!(result.is_err(), "no token must not upgrade");
}

#[tokio::test(flavor = "multi_thread")]
async fn pre_auth_session_receives_no_broadcasts() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let addr = spawn_server(state.clone()).await;

    // A pairing-code socket upgrades but stays silent: no hello, so no
    // auth. Broadcasts made while it lingers in the hello window must not
    // reach it (state patches, deltas, anything).
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?pair=GARBAGE")).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    state.hub.broadcast_frame(&Frame::push_typed(
        TYPE_BOARDS_DELTA,
        &BoardsDelta {
            generation: 99,
            ops: vec![BoardOp::TileRemove { board: 3, tile: 21 }],
        },
    ));
    // Protocol-level pings are keepalive and fine; only data frames leak.
    let leaked = tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            match ws.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => break text,
                Some(Ok(_)) => continue,
                other => panic!("ws stream ended: {other:?}"),
            }
        }
    })
    .await;
    assert!(
        leaked.is_err(),
        "unauthenticated session must not receive broadcasts"
    );

    // The same socket still completes the handshake path when it finally
    // speaks: a bad code yields the typed pair error, proving the session
    // was alive all along - just outside the fan-out.
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_HELLO,
            "h1",
            serde_json::json!({"client": "deckboard-mobile", "version": "0.2.0"}),
        ),
    )
    .await;
    let err = next_frame(&mut ws).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::PAIR_INVALID);
}

#[tokio::test(flavor = "multi_thread")]
async fn pairing_flow_mints_welcome_and_device() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let addr = spawn_server(state.clone()).await;

    let (status, _, body) = http_post_json(addr, "/v2/pair", "{}").await;
    assert_eq!(status, 200);
    let pair: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let code = pair["code"].as_str().unwrap().to_string();
    assert_eq!(pair["expires_in"].as_u64(), Some(300));

    // Bad codes upgrade but die with a typed error frame after hello.
    let mut bad = ws_open(&format!("ws://{addr}/v2/ws?pair=WRONG123")).await;
    send_frame(
        &mut bad,
        &Frame::request(
            TYPE_HELLO,
            "h9",
            serde_json::json!({"client": "deckboard-mobile", "version": "0.2.0"}),
        ),
    )
    .await;
    let err = next_frame(&mut bad).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(
        (err.kind.as_str(), err.ack.as_deref(), payload.code.as_str()),
        (TYPE_ERROR, Some("h9"), error_code::PAIR_INVALID)
    );
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(msg) = bad.next().await {
            if msg.is_err() || matches!(msg, Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
            {
                break;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "socket must close after pair-invalid");

    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?pair={code}")).await;
    let (welcome, _sync, _state_sync) = handshake(&mut ws, "deckboard-mobile", "0.2.0").await;
    assert_eq!(welcome.device.name, "Test tablet");
    assert_eq!(welcome.protocol, PROTOCOL_VERSION);
    // The code burned on use: a second pairing with it fails on the socket.
    let mut burned = ws_open(&format!("ws://{addr}/v2/ws?pair={code}")).await;
    send_frame(
        &mut burned,
        &Frame::request(
            TYPE_HELLO,
            "h9",
            serde_json::json!({"client": "deckboard-mobile", "version": "0.2.0"}),
        ),
    )
    .await;
    let err = next_frame(&mut burned).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::PAIR_INVALID);
    // A device entry exists now.
    assert!(state
        .devices
        .list()
        .iter()
        .any(|d| d.id == welcome.device.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn token_connect_delivers_full_snapshot() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet salon");
    state
        .engine
        .set("ext.speaker-muted", serde_json::json!("OFF"));
    state.engine.set("ext.si-cpu", serde_json::json!(0.5));
    let addr = spawn_server(state.clone()).await;

    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    let (welcome, sync, state_sync) = handshake(&mut ws, "deckboard-mobile", "0.2.0").await;
    assert_eq!(welcome.device.id, device.id);
    assert_eq!(welcome.generation, 1);
    // Tile channels registered during the boards build land in the catalog.
    assert_eq!(
        welcome.channels["ext.speaker-muted"].shape,
        StateShape::Scalar
    );
    assert_eq!(welcome.channels["ext.si-cpu"].shape, StateShape::Series);
    assert_eq!(welcome.channels["ext.si-cpu"].cap, Some(120));

    let board = &sync.boards[0];
    assert_eq!(
        board.background,
        Some(Background::Color {
            color: "#2c3e50".into()
        })
    );
    let vol = board.tiles.iter().find(|t| t.id == 17).unwrap();
    assert_eq!(vol.manifest.kind, WidgetKind::Toggle);
    assert_eq!(
        vol.manifest.state.as_ref().unwrap().channel,
        "ext.speaker-muted"
    );
    let slider = board.tiles.iter().find(|t| t.id == 21).unwrap();
    assert_eq!(slider.manifest.kind, WidgetKind::Slider);
    assert_eq!(slider.manifest.interactions, vec![Interaction::Slide]);
    let graph = board.tiles.iter().find(|t| t.id == 22).unwrap();
    assert_eq!(graph.manifest.kind, WidgetKind::Graph);

    assert_eq!(state_sync.values["ext.speaker-muted"], "OFF");
    assert_eq!(state_sync.series["ext.si-cpu"], vec![0.5]);
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_rename_lands_in_welcome_and_registry() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Old name");
    let addr = spawn_server(state.clone()).await;

    // hello.name renames the paired device; the welcome of THIS connection
    // must already carry the new name, and so must the persisted registry.
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    let (welcome, _sync, _state_sync) = handshake(&mut ws, "deckboard-mobile", "0.2.0").await;
    assert_eq!(
        welcome.device.name, "Test tablet",
        "welcome must carry the renamed entry"
    );
    assert_eq!(state.devices.list()[0].name, "Test tablet");
}

#[tokio::test(flavor = "multi_thread")]
async fn state_changes_flow_as_patches() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state.clone()).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    state.engine.set("ext.late", serde_json::json!("hello"));
    let frame = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let f = next_frame(&mut ws).await;
            if f.kind == TYPE_STATE_PATCH {
                break f;
            }
        }
    })
    .await
    .expect("patch within timeout");
    let patch: StatePatch = serde_json::from_value(frame.payload.unwrap()).unwrap();
    assert_eq!(patch.changes[0].channel, "ext.late");
    assert_eq!(patch.changes[0].value, "hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn interaction_acks_execs_and_reports_unknown_tiles() {
    let backend = sample_backend();
    let (state, _dir) = test_state(backend.clone(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    // Tap: ack with the request id, then the exec lands in the backend.
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "i1",
            serde_json::json!({"board": 3, "tile": 17, "interaction": "tap"}),
        ),
    )
    .await;
    let ack = next_frame(&mut ws).await;
    assert_eq!(ack.ack.as_deref(), Some("i1"));
    assert_eq!(ack.kind, TYPE_INTERACTION);
    assert_eq!(ack.payload.unwrap(), serde_json::json!({}));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(backend.execs.lock().unwrap().contains(&(17, true)));

    // Slide reaches the slider backend with the value.
    send_frame(&mut ws, &Frame::request(TYPE_INTERACTION, "i2", serde_json::json!({"board": 3, "tile": 21, "interaction": "slide", "args": {"value": 0.75}}))).await;
    let ack = next_frame(&mut ws).await;
    assert_eq!(ack.ack.as_deref(), Some("i2"));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(backend.sliders.lock().unwrap().last(), Some(&(21, 0.75)));

    // Unknown tile -> typed error, connection stays up.
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "i3",
            serde_json::json!({"board": 3, "tile": 999, "interaction": "tap"}),
        ),
    )
    .await;
    let err = next_frame(&mut ws).await;
    assert_eq!(err.kind, TYPE_ERROR);
    assert_eq!(err.ack.as_deref(), Some("i3"));
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::UNKNOWN_TILE);

    // Unsupported gesture -> typed error.
    send_frame(&mut ws, &Frame::request(TYPE_INTERACTION, "i4", serde_json::json!({"board": 3, "tile": 17, "interaction": "wheel", "args": {"delta": 1.0}}))).await;
    let err = next_frame(&mut ws).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::UNSUPPORTED_INTERACTION);
}

#[tokio::test(flavor = "multi_thread")]
async fn hold_repeat_runs_until_press_end() {
    let backend = sample_backend();
    let (state, _dir) = test_state(backend.clone(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "h1",
            serde_json::json!({"board": 3, "tile": 23, "interaction": "press-start"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;
    tokio::time::sleep(Duration::from_millis(250)).await; // delay 20ms + interval 20ms
    let during = backend.exec_count();
    assert!(during >= 3, "hold repeats: got {during}");

    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "h2",
            serde_json::json!({"board": 3, "tile": 23, "interaction": "press-end"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = backend.exec_count();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let settled = backend.exec_count();
    assert!(
        settled <= after + 1,
        "repeat stops after press-end ({after} -> {settled})"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_press_start_does_not_leak_a_repeat_loop() {
    let backend = sample_backend();
    let (state, _dir) = test_state(backend.clone(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    // Two press-starts for one tile: the second must replace (not join)
    // the first loop, so a single press-end stops everything.
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "d1",
            serde_json::json!({"board": 3, "tile": 23, "interaction": "press-start"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "d2",
            serde_json::json!({"board": 3, "tile": 23, "interaction": "press-start"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "d3",
            serde_json::json!({"board": 3, "tile": 23, "interaction": "press-end"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;

    tokio::time::sleep(Duration::from_millis(250)).await;
    let after = backend.exec_count();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let settled = backend.exec_count();
    assert!(
        settled <= after + 1,
        "leaked repeat loop kept firing ({after} -> {settled})"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn undeclared_interactions_are_rejected() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    // The slider tile (21) declares only `slide`; press-start is not in
    // its manifest, so it must not reach the backend.
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "u1",
            serde_json::json!({"board": 3, "tile": 21, "interaction": "press-start"}),
        ),
    )
    .await;
    let err = next_frame(&mut ws).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::UNSUPPORTED_INTERACTION);
}

#[tokio::test(flavor = "multi_thread")]
async fn board_switch_pushes_board_open() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "b1",
            serde_json::json!({"board": 3, "tile": 24, "interaction": "tap"}),
        ),
    )
    .await;
    let ack = next_frame(&mut ws).await;
    assert_eq!(ack.ack.as_deref(), Some("b1"));
    let open: BoardOpen = typed(next_frame(&mut ws).await, TYPE_BOARD_OPEN);
    assert_eq!(open.board, 9);
}

#[tokio::test(flavor = "multi_thread")]
async fn published_deltas_reach_clients() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state.clone()).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    let op = BoardOp::TileRemove { board: 3, tile: 21 };
    let generation = state.publish_delta(vec![op]);
    assert_eq!(generation, 2);

    let frame = next_frame(&mut ws).await;
    assert_eq!(frame.kind, TYPE_BOARDS_DELTA);
    let delta: BoardsDelta = serde_json::from_value(frame.payload.unwrap()).unwrap();
    assert_eq!(delta.generation, 2);
    assert_eq!(delta.ops[0], BoardOp::TileRemove { board: 3, tile: 21 });
}

#[tokio::test(flavor = "multi_thread")]
async fn assets_serve_with_token_and_cache_headers() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let png_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(b"raw-png-bytes")
    );
    let hash = state.assets.import_data_url(&png_url).unwrap();
    let addr = spawn_server(state).await;

    // No token -> 401, even for a valid hash.
    let (status, _, _) = http_get(addr, &format!("/assets/{hash}")).await;
    assert_eq!(status, 401);
    // Bad hash with a good token -> 404.
    let zeros = "0".repeat(64);
    let (status, _, _) = http_get(addr, &format!("/assets/{zeros}?token={}", device.token)).await;
    assert_eq!(status, 404);
    // Happy path: bytes + immutable caching.
    let (status, headers, body) =
        http_get(addr, &format!("/assets/{hash}?token={}", device.token)).await;
    assert_eq!(status, 200);
    assert_eq!(body, b"raw-png-bytes");
    assert_eq!(headers["content-type"].as_str(), "image/png");
    assert_eq!(
        headers["cache-control"].as_str(),
        "immutable, max-age=31536000"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn server_pings_idle_clients() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    let ping = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(msg) = ws.next().await {
            if matches!(msg, Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) {
                break;
            }
        }
    })
    .await;
    assert!(ping.is_ok(), "expected a protocol-level ping");
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_frame_gets_typed_error_and_close() {
    let (state, _dir) = test_state(sample_backend(), |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    // 1 MiB + slack: above the protocol limit but below the wire cap, so
    // the app-level check (not tungstenite) classifies it - the spec
    // promises a typed error, then a close.
    let big = "x".repeat(MAX_FRAME_BYTES + 16);
    ws.send(tokio_tungstenite::tungstenite::Message::Text(big))
        .await
        .unwrap();
    let err = next_frame(&mut ws).await;
    assert_eq!(err.kind, TYPE_ERROR);
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::TOO_LARGE);
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(msg) = ws.next().await {
            if msg.is_err() || matches!(msg, Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
            {
                break;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "connection must close after too-large");
}

#[tokio::test(flavor = "multi_thread")]
async fn outdated_clients_are_closed_after_hello() {
    let (state, _dir) = test_state(sample_backend(), |cfg| {
        cfg.min_client = "9.9.9".into();
    });
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_HELLO,
            "h1",
            serde_json::json!({"client": "deckboard-mobile", "version": "0.2.0"}),
        ),
    )
    .await;
    let err = next_frame(&mut ws).await;
    let payload: ErrorPayload = serde_json::from_value(err.payload.unwrap()).unwrap();
    assert_eq!(payload.code, error_code::OUTDATED_CLIENT);
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(msg) = ws.next().await {
            if msg.is_err() || matches!(msg, Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
            {
                break;
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "connection must close after outdated-client"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_side_values_land_on_ext_channels() {
    let mut backend = sample_backend();
    backend
        .buttons
        .push(button_row(25, "value-pusher", "button", None, None));
    let (state, _dir) = test_state(backend, |_| {});
    let device = state.devices.create("Tablet");
    let addr = spawn_server(state.clone()).await;
    let mut ws = ws_open(&format!("ws://{addr}/v2/ws?token={}", device.token)).await;
    handshake(&mut ws, "deckboard-mobile", "0.2.0").await;

    send_frame(
        &mut ws,
        &Frame::request(
            TYPE_INTERACTION,
            "v1",
            serde_json::json!({"board": 3, "tile": 25, "interaction": "tap"}),
        ),
    )
    .await;
    let _ack = next_frame(&mut ws).await;
    let frame = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let f = next_frame(&mut ws).await;
            if f.kind == TYPE_STATE_PATCH {
                break f;
            }
        }
    })
    .await
    .expect("state patch from exec app_value");
    let patch: StatePatch = serde_json::from_value(frame.payload.unwrap()).unwrap();
    assert_eq!(patch.changes[0].channel, "ext.fake-key");
    assert_eq!(patch.changes[0].value, "42");
}
