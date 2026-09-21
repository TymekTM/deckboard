//! End-to-end protocol tests: drive the axum service the way the stock
//! Android client does - socket.io v2 over polling and over websocket.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use deckboard_actions::EventSink;
use deckboard_db::{BoardRow, ButtonRow};
use deckboard_legacy::{router, AppState, Backend, Hub};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Records exec calls instead of touching the OS.
#[derive(Default)]
struct MockBackend {
    pub execs: Mutex<Vec<(i64, bool)>>,
    pub sliders: Mutex<Vec<(i64, f64)>>,
}

impl Backend for MockBackend {
    fn get_boards(&self) -> Vec<BoardRow> {
        vec![BoardRow {
            id: 1,
            name: "My Board".into(),
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
        }]
    }

    fn get_buttons_by_board(&self, _board_id: i64) -> Vec<ButtonRow> {
        vec![url_button(10, 0, 0)]
    }

    fn get_button(&self, id: i64) -> Option<ButtonRow> {
        (id == 10).then(|| url_button(10, 0, 0))
    }

    fn exec(&self, button: ButtonRow, is_tap_start: bool, _sink: &mut dyn EventSink) {
        self.execs.lock().unwrap().push((button.id, is_tap_start));
    }

    fn slider(&self, button: ButtonRow, value: f64) {
        self.sliders.lock().unwrap().push((button.id, value));
    }
}

fn url_button(id: i64, x: i64, y: i64) -> ButtonRow {
    ButtonRow {
        id,
        board_id: 1,
        kind: "url".into(),
        command: Some("https://example.com".into()),
        title: Some("Example".into()),
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
        mode: "button".into(),
        x: Some(x),
        y: Some(y),
        w: 1,
        h: 1,
        options: None,
    }
}

async fn spawn_server() -> (std::net::SocketAddr, Arc<MockBackend>) {
    let backend = Arc::new(MockBackend::default());
    let state = Arc::new(AppState {
        hub: Arc::new(Hub::new()),
        backend: backend.clone() as Arc<dyn Backend>,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    (addr, backend)
}

/// Minimal blocking HTTP/1.1 client for the polling transport.
fn http(addr: std::net::SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    let status: u16 = buf
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = buf
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

#[tokio::test(flavor = "multi_thread")]
async fn polling_full_flow() {
    let (addr, backend) = spawn_server().await;

    // 1. handshake: open packet carries sid, no upgrades info mismatch
    let (status, open) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=1", None);
    assert_eq!(status, 200);
    assert!(open.starts_with("0{"), "open packet: {open}");
    let open_json: serde_json::Value =
        serde_json::from_str(&open[1..]).unwrap();
    let sid = open_json["sid"].as_str().unwrap().to_string();
    assert_eq!(open_json["upgrades"], serde_json::json!(["websocket"]));

    // 2. next poll delivers the socket.io connect packet
    let (status, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=2&sid={sid}"),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(packets, "40");

    // 3. client asks for boards
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=3&sid={sid}"),
        Some(r#"42["get_shortcuts"]"#),
    );
    assert_eq!(status, 200);
    let (status, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=4&sid={sid}"),
        None,
    );
    assert_eq!(status, 200);
    assert!(
        packets.starts_with(r#"42["get_shortcuts","#),
        "packets: {packets}"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&packets["42[\"get_shortcuts\",".len()..packets.len() - 1]).unwrap();
    let boards = payload.as_array().unwrap();
    assert_eq!(boards.len(), 1);
    assert_eq!(boards[0]["staggered"], true);
    assert_eq!(boards[0]["width"], 4);
    let shortcuts = boards[0]["shortcuts"].as_array().unwrap();
    assert!(shortcuts.iter().any(|s| s["id"] == 10));
    assert!(shortcuts.iter().any(|s| s["id"].is_null())); // fillers

    // 4. exec_shortcut reaches the backend. Execution is detached from the
    // POST (long actions must not stall the client's pings), so the effect
    // lands shortly after the response - poll for it.
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=5&sid={sid}"),
        Some(r#"42["exec_shortcut",{"id":10,"isTapStart":false}]"#),
    );
    assert_eq!(status, 200);
    wait_for_execs(&backend, &[(10, false)]);

    // 5. get_version broadcasts the exact legacy string
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=6&sid={sid}"),
        Some(r#"42["get_version"]"#),
    );
    assert_eq!(status, 200);
    let (_, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=7&sid={sid}"),
        None,
    );
    assert_eq!(packets, r#"42["get_version",{"version":"1.6.0"}]"#);

    // 6. exec_slider reaches the backend (also detached; poll for it)
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=8&sid={sid}"),
        Some(r#"42["exec_slider",{"id":10,"value":0.5}]"#),
    );
    assert_eq!(status, 200);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let sliders = backend.sliders.lock().unwrap().clone();
        if sliders == vec![(10, 0.5)] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "exec_slider never landed: {sliders:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Polls until the detached exec task has recorded exactly `expected`.
fn wait_for_execs(backend: &MockBackend, expected: &[(i64, bool)]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let execs = backend.execs.lock().unwrap().clone();
        if execs.as_slice() == expected {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "exec_shortcut never landed: {execs:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_direct_flow() {
    let (addr, _backend) = spawn_server().await;
    let url = format!(
        "ws://{addr}/socket.io/?EIO=3&transport=websocket&access_key=DCKBRD_PRO_1_3_0"
    );
    let (mut ws, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .expect("connect timeout")
    .unwrap();

    // open packet then connect packet
    let open = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("open timeout")
        .unwrap()
        .unwrap();
    assert!(open.to_text().unwrap().starts_with("0{"));
    let connect = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("connect pkt timeout")
        .unwrap()
        .unwrap();
    assert_eq!(connect.to_text().unwrap(), "40");

    // engine.io upgrade probe
    ws.send(Message::Text("2probe".into())).await.unwrap();
    let pong = recv_text(&mut ws).await;
    assert_eq!(pong, "3probe");
    ws.send(Message::Text("5".into())).await.unwrap();

    // engine ping -> pong
    ws.send(Message::Text("2".into())).await.unwrap();
    let pong = recv_text(&mut ws).await;
    assert_eq!(pong, "3");

    // event round-trip
    ws.send(Message::Text(r#"42["get_version"]"#.into()))
        .await
        .unwrap();
    let ev = recv_text(&mut ws).await;
    assert_eq!(ev, r#"42["get_version",{"version":"1.6.0"}]"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn websocket_upgrade_from_polling_session() {
    let (addr, _backend) = spawn_server().await;

    // polling session first
    let (_, open) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=1", None);
    let sid: String = {
        let open_json: serde_json::Value = serde_json::from_str(&open[1..]).unwrap();
        open_json["sid"].as_str().unwrap().into()
    };

    // websocket upgrade for the same sid: no second open packet
    let url = format!("ws://{addr}/socket.io/?EIO=3&transport=websocket&sid={sid}");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

    // the queued 40 connect is flushed over the websocket
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(msg.to_text().unwrap(), "40");

    ws.send(Message::Text("2probe".into())).await.unwrap();
    let pong = ws.next().await.unwrap().unwrap();
    assert_eq!(pong.to_text().unwrap(), "3probe");
}

#[tokio::test(flavor = "multi_thread")]
async fn health_page_served() {
    let (addr, _) = spawn_server().await;
    let (status, body) = http(addr, "GET", "/", None);
    assert_eq!(status, 200);
    assert!(body.contains("Deckboard Server is live"));
}

async fn recv_text(ws: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>) -> String {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("ws recv timeout")
        .unwrap()
        .unwrap();
    let text = msg.to_text().unwrap().to_string();
    text
}
