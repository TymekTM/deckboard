//! Editor broadcast tests: after a mutation the connected clients receive
//! the same `refresh_board` / `get_shortcuts` packets the original app sent.

use std::io::{Read, Write};
use std::sync::Arc;

use deckboard_db::{BoardRow, ButtonRow};
use deckboard_legacy::hub::WsOut;
use deckboard_legacy::{router, AppState, Backend, EditorBroadcaster, Hub};

struct MockBackend;

impl Backend for MockBackend {
    fn get_boards(&self) -> Vec<BoardRow> {
        vec![board_row(1)]
    }

    fn get_board(&self, board_id: i64) -> Option<BoardRow> {
        (board_id == 1).then(|| board_row(1))
    }

    fn get_buttons_by_board(&self, _board_id: i64) -> Vec<ButtonRow> {
        vec![ButtonRow {
            id: 7,
            board_id: 1,
            kind: "url".into(),
            title: Some("Example".into()),
            command: Some("https://example.com".into()),
            x: Some(0),
            y: Some(0),
            w: 1,
            h: 1,
            mode: "button".into(),
            ..ButtonRow::default()
        }]
    }

    fn get_button(&self, _id: i64) -> Option<ButtonRow> {
        None
    }

    fn exec(
        &self,
        _button: ButtonRow,
        _is_tap_start: bool,
        _sink: &mut dyn deckboard_actions::EventSink,
    ) {
    }

    fn slider(&self, _button: ButtonRow, _value: f64) {}
}

fn board_row(id: i64) -> BoardRow {
    BoardRow {
        id,
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
    }
}

fn broadcaster(hub: Arc<Hub>) -> EditorBroadcaster {
    EditorBroadcaster::new(hub, Arc::new(MockBackend))
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_board_carries_both_room_variants() {
    let hub = Arc::new(Hub::new());
    let bc = broadcaster(hub.clone());

    // a websocket client in the BASIC room
    let session = hub.create(false).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    session.upgrade_to_ws(tx).await;

    bc.refresh_board(1).await;

    let packet = match rx.recv().await {
        Some(WsOut::Packet(p)) => p,
        other => panic!("unexpected {other:?}"),
    };
    let payload: serde_json::Value = serde_json::from_str(
        packet
            .strip_prefix(r#"42["refresh_board","#)
            .unwrap()
            .strip_suffix(']')
            .unwrap(),
    )
    .unwrap();
    // one object, both rooms; BASIC gets the 4x3-cropped filler view
    assert_eq!(payload["basic"]["width"], 4);
    assert_eq!(payload["pro"]["width"], 4);
    assert!(payload["basic"]["shortcuts"].as_array().unwrap().len() >= 12);
    // pro variant carries the same board (4x3 board, no crop needed)
    assert_eq!(
        payload["pro"]["shortcuts"][0]["id"],
        payload["basic"]["shortcuts"][0]["id"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_boards_broadcasts_full_list_per_room() {
    let hub = Arc::new(Hub::new());
    let bc = broadcaster(hub.clone());
    let session = hub.create(true).await; // PRO room
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    session.upgrade_to_ws(tx).await;

    bc.sync_boards().await;

    let packet = match rx.recv().await {
        Some(WsOut::Packet(p)) => p,
        other => panic!("unexpected {other:?}"),
    };
    assert!(
        packet.starts_with(r#"42["get_shortcuts","#),
        "packet: {packet}"
    );
    let payload: serde_json::Value = serde_json::from_str(
        packet
            .strip_prefix(r#"42["get_shortcuts","#)
            .unwrap()
            .strip_suffix(']')
            .unwrap(),
    )
    .unwrap();
    assert_eq!(payload["pro"].as_array().unwrap().len(), 1);
    assert_eq!(payload["pro"][0]["name"], "My Board");
}

/// The stock client path end-to-end: mutation broadcast reaches a polling
/// session through the served router's hub.
#[tokio::test(flavor = "multi_thread")]
async fn broadcast_reaches_polling_session() {
    let hub = Arc::new(Hub::new());
    let bc = broadcaster(hub.clone());
    let state = Arc::new(AppState {
        hub: hub.clone(),
        backend: Arc::new(MockBackend) as Arc<dyn Backend>,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });

    // handshake like the Dart client
    let (mut stream, body) = http(addr, "GET", "/socket.io/?EIO=3&transport=polling&t=1", None);
    let sid: String = {
        let open_json: serde_json::Value = serde_json::from_str(&body[1..]).unwrap();
        open_json["sid"].as_str().unwrap().into()
    };
    drop(stream);
    let _ = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=2&sid={sid}"),
        None,
    );

    bc.refresh_board(1).await;

    let (_, packets) = http(
        addr,
        "GET",
        &format!("/socket.io/?EIO=3&transport=polling&t=3&sid={sid}"),
        None,
    );
    assert!(
        packets.starts_with(r#"42["refresh_board","#),
        "packets: {packets}"
    );
}

fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (std::net::TcpStream, String) {
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf);
    let body = buf
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (stream, body)
}
