//! End-to-end protocol tests: drive the axum service the way the stock
//! Android client does - socket.io v2 over polling and over websocket.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use pulpit_actions::EventSink;
use pulpit_db::{BoardRow, ButtonRow};
use pulpit_legacy::{router, AppState, Backend, Hub};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// Records exec calls instead of touching the OS.
#[derive(Default)]
struct MockBackend {
    pub execs: Mutex<Vec<(i64, bool)>>,
    pub sliders: Mutex<Vec<(i64, f64)>>,
    /// Execs whose dispatched row still carried image columns - must
    /// stay empty: exec dispatch reads the image-less meta row (CORE-02).
    pub image_execs: Mutex<Vec<i64>>,
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

    fn get_board(&self, board_id: i64) -> Option<BoardRow> {
        (board_id == 1).then(|| self.get_boards().remove(0))
    }

    fn get_buttons_by_board(&self, _board_id: i64) -> Vec<ButtonRow> {
        vec![url_button(10, 0, 0)]
    }

    fn get_button(&self, id: i64) -> Option<ButtonRow> {
        match id {
            10 => Some(url_button(10, 0, 0)),
            11 => Some(key_button(11)),
            _ => None,
        }
    }

    /// Mimics the production `SqlBackend` meta read: the same row with
    /// the `img`/`img2` columns stubbed empty - so the exec tests below
    /// pin that dispatch (which reads meta since CORE-02) never depends
    /// on the image columns.
    fn get_button_meta(&self, id: i64) -> Option<ButtonRow> {
        let mut row = self.get_button(id)?;
        row.img = Some(String::new());
        row.img2 = Some(String::new());
        Some(row)
    }

    fn exec(&self, button: ButtonRow, is_tap_start: bool, _sink: &mut dyn EventSink) {
        if button.img.as_deref().is_some_and(|i| !i.is_empty()) {
            self.image_execs.lock().unwrap().push(button.id);
        }
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

/// Key tiles hold keys down on `isTapStart: true` and release them on
/// the tap end.
fn key_button(id: i64) -> ButtonRow {
    ButtonRow {
        kind: "key".into(),
        command: Some("A".into()),
        ..url_button(id, 0, 1)
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
    http_with_headers(addr, method, path, body, &[])
}

fn http_with_headers(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let body = body.unwrap_or("");
    let extra: String = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    let open_json: serde_json::Value = serde_json::from_str(&open[1..]).unwrap();
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
    let url =
        format!("ws://{addr}/socket.io/?EIO=3&transport=websocket&access_key=DCKBRD_PRO_1_3_0");
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
    assert!(body.contains("Pulpit server is live"));
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_requests_are_rejected_on_the_sockets() {
    let (addr, _backend) = spawn_server().await;

    // Polling handshake with a foreign Origin: a web page open on this
    // machine must not get a session. Native clients send no Origin.
    let (status, body) = http_with_headers(
        addr,
        "GET",
        "/socket.io/?EIO=3&transport=polling&t=1",
        None,
        &[("Origin", "http://evil.example")],
    );
    assert_eq!(status, 403, "foreign Origin must be rejected: {body}");

    // Same-origin (loopback page served by this very server): allowed.
    let (status, _) = http_with_headers(
        addr,
        "GET",
        "/socket.io/?EIO=3&transport=polling&t=2",
        None,
        &[("Origin", &format!("http://{addr}"))],
    );
    assert_eq!(status, 200, "same-host Origin must be allowed");

    // A non-literal Host is what DNS rebinding produces: rejected even
    // without an Origin header.
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .write_all(
            b"GET /socket.io/?EIO=3&transport=polling&t=3 HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    assert!(
        buf.starts_with("HTTP/1.1 403"),
        "rebound Host must be rejected"
    );

    // Websocket upgrade with a foreign Origin never completes.
    let mut request = (&format!("ws://{addr}/socket.io/?EIO=3&transport=websocket"))
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://evil.example".parse().unwrap());
    let result = tokio_tungstenite::connect_async(request).await;
    assert!(result.is_err(), "foreign-Origin upgrade must be rejected");

    // No Origin header at all: the stock Android client's normal shape.
    let (status, _) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=4", None);
    assert_eq!(status, 200, "Origin-less native requests must work");
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_packets_do_not_kill_the_session() {
    let (addr, backend) = spawn_server().await;

    // polling handshake: open packet + the queued connect packet
    let (_, open) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=1", None);
    let sid: String = {
        let open_json: serde_json::Value = serde_json::from_str(&open[1..]).unwrap();
        open_json["sid"].as_str().unwrap().into()
    };
    let (_, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=2&sid={sid}"),
        None,
    );
    assert_eq!(packets, "40");

    // Abuse the POST channel with every malformed shape: unknown engine
    // packet types, sio packets without a payload, non-JSON event
    // bodies, unparseable ids, bare record separators. Every POST must
    // answer 200 and the session must survive all of them.
    for body in [
        "9",
        "4",
        "42",
        "42[not-json",
        "42[\"exec_shortcut\",{\"id\":\"NaN\"}]",
        "\u{1e}",
        "\u{1e}2\u{1e}42[bad\u{1e}",
    ] {
        let (status, _) = http(
            addr,
            "POST",
            &format!("/socket.io/?EIO=3&transport=polling&t=3&sid={sid}"),
            Some(body),
        );
        assert_eq!(status, 200, "malformed body must not 5xx: {body:?}");
    }

    // The session is still alive and answering: engine ping -> pong.
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=4&sid={sid}"),
        Some("2"),
    );
    assert_eq!(status, 200);
    let (_, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=5&sid={sid}"),
        None,
    );
    // polling batches queued packets with record separators: every
    // delivered packet must be a pong (one came from the ping smuggled
    // inside the malformed batch above)
    assert!(
        packets.split('\u{1e}').all(|p| p == "3"),
        "session must answer pings with pongs, got: {packets:?}"
    );

    // And a real event still executes after the abuse.
    let (status, _) = http(
        addr,
        "POST",
        &format!("/socket.io/?EIO=3&transport=polling&t=6&sid={sid}"),
        Some(r#"42["exec_shortcut",{"id":10,"isTapStart":false}]"#),
    );
    assert_eq!(status, 200);
    wait_for_execs(&backend, &[(10, false)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn key_tiles_release_when_the_socket_drops() {
    let (addr, backend) = spawn_server().await;
    let url =
        format!("ws://{addr}/socket.io/?EIO=3&transport=websocket&access_key=DCKBRD_PRO_1_3_0");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // open + connect packets
    let _ = recv_text(&mut ws).await;
    let _ = recv_text(&mut ws).await;

    // press-start a key tile: key-down. The socket then dies without the
    // tap end - the key must still be released.
    ws.send(Message::Text(
        r#"42["exec_shortcut",{"id":11,"isTapStart":true}]"#.into(),
    ))
    .await
    .unwrap();
    wait_for_exec(&backend, (11, true)).await;
    drop(ws);
    wait_for_exec(&backend, (11, false)).await;
}

/// Polls until the detached exec task has recorded `expected`.
async fn wait_for_exec(backend: &MockBackend, expected: (i64, bool)) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if backend.execs.lock().unwrap().contains(&expected) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "exec {expected:?} never landed: {:?}",
            backend.execs.lock().unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

async fn recv_text(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> String {
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("ws recv timeout")
        .unwrap()
        .unwrap();
    let text = msg.to_text().unwrap().to_string();
    text
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_dispatch_rides_the_imageless_meta_row() {
    // CORE-02: taps and sliders read get_button_meta (img/img2 stubbed
    // empty like SqlBackend does), never the full base64 columns - and
    // dispatch itself works identically on that row.
    let (addr, backend) = spawn_server().await;
    let (status, open) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=1", None);
    assert_eq!(status, 200);
    let sid: String = serde_json::from_str::<serde_json::Value>(&open[1..]).unwrap()["sid"]
        .as_str()
        .unwrap()
        .into();
    let _ = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=2&sid={sid}"),
        None,
    );
    for body in [
        r#"42["exec_shortcut",{"id":10,"isTapStart":false}]"#,
        r#"42["exec_slider",{"id":10,"value":0.5}]"#,
    ] {
        let (status, _) = http(
            addr,
            "POST",
            &format!("/socket.io/?EIO=3&transport=polling&t=3&sid={sid}"),
            Some(body),
        );
        assert_eq!(status, 200);
    }
    wait_for_execs(&backend, &[(10, false)]);
    assert!(
        backend.sliders.lock().unwrap().contains(&(10, 0.5)),
        "slider never landed: {:?}",
        backend.sliders.lock().unwrap()
    );
    // every dispatched row carried the stubbed (empty) image columns
    assert!(
        backend.image_execs.lock().unwrap().is_empty(),
        "exec dispatch received full image rows: {:?}",
        backend.image_execs.lock().unwrap()
    );
}
