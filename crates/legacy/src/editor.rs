//! Editor mutations pushed to connected stock clients. The original desktop
//! app emits `refresh_board` after every tile mutation and a full
//! `get_shortcuts` broadcast after board-level changes - replicated here so
//! tablets stay in sync while the Rust editor is the writer (ADR-002/004).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::hub::Hub;
use crate::mapping::Mapper;
use crate::service::Backend;

pub struct EditorBroadcaster {
    hub: Arc<Hub>,
    backend: Arc<dyn Backend>,
}

impl EditorBroadcaster {
    pub fn new(hub: Arc<Hub>, backend: Arc<dyn Backend>) -> Self {
        EditorBroadcaster { hub, backend }
    }

    /// After add/edit/delete/move/resize/clear of a tile on `board_id` every
    /// room receives ONE object: BASIC clients read `basic`, PRO clients
    /// read `pro`.
    pub async fn refresh_board(&self, board_id: i64) {
        let Some(board) = self.backend.get_board(board_id) else {
            return;
        };
        let buttons = self.backend.get_buttons_by_board(board_id);
        let mapper = Mapper::new();
        let payload = json!({
            "basic": mapper.board_payload(&board, &buttons, false),
            "pro": mapper.board_payload(&board, &buttons, true),
        });
        self.hub
            .broadcast("refresh_board", Some(&payload.to_string()))
            .await;
    }

    /// Board-level changes (create/delete/rename/resize/import) broadcast
    /// the full shortcut list, per room as `{basic: [...], pro: [...]}`.
    pub async fn sync_boards(&self) {
        let boards = self.backend.get_boards();
        // one grouped read for every board's shortcuts (NET-09)
        let buttons = self.backend.all_buttons_by_board();
        let mapper = Mapper::new();
        let payload = json!({
            "basic": boards_payload(&boards, &buttons, &mapper, false),
            "pro": boards_payload(&boards, &buttons, &mapper, true),
        });
        self.hub
            .broadcast("get_shortcuts", Some(&payload.to_string()))
            .await;
    }
}

fn boards_payload(
    boards: &[pulpit_db::BoardRow],
    buttons: &std::collections::HashMap<i64, Vec<pulpit_db::ButtonRow>>,
    mapper: &Mapper,
    pro: bool,
) -> Vec<Value> {
    boards
        .iter()
        .map(|b| {
            let rows = buttons.get(&b.id);
            mapper.board_payload(b, rows.map(Vec::as_slice).unwrap_or(&[]), pro)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct OneBoard;

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

    impl Backend for OneBoard {
        fn get_boards(&self) -> Vec<pulpit_db::BoardRow> {
            vec![board(1, "Main"), board(2, "Empty")]
        }
        fn get_board(&self, id: i64) -> Option<pulpit_db::BoardRow> {
            (id == 1).then(|| board(1, "Main"))
        }
        fn get_buttons_by_board(&self, board_id: i64) -> Vec<pulpit_db::ButtonRow> {
            if board_id != 1 {
                return Vec::new();
            }
            vec![pulpit_db::ButtonRow {
                id: 3,
                board_id: 1,
                kind: "url".into(),
                title: Some("Docs".into()),
                x: Some(1),
                y: Some(1),
                w: 1,
                h: 1,
                mode: "button".into(),
                ..pulpit_db::ButtonRow::default()
            }]
        }
        fn get_button(&self, _id: i64) -> Option<pulpit_db::ButtonRow> {
            None
        }
        fn exec(
            &self,
            _button: pulpit_db::ButtonRow,
            _is_tap_start: bool,
            _sink: &mut dyn pulpit_actions::EventSink,
        ) {
        }
        fn slider(&self, _button: pulpit_db::ButtonRow, _value: f64) {}
    }

    #[tokio::test]
    async fn refresh_of_an_unknown_board_sends_nothing() {
        let hub = Arc::new(Hub::new());
        let session = hub.create(Arc::new(OneBoard), false).await;
        EditorBroadcaster::new(hub.clone(), Arc::new(OneBoard))
            .refresh_board(99)
            .await;
        assert_eq!(session.poll(1).await, "");
    }

    #[tokio::test]
    async fn refresh_reaches_polling_sessions_too() {
        let hub = Arc::new(Hub::new());
        let session = hub.create(Arc::new(OneBoard), true).await;
        EditorBroadcaster::new(hub.clone(), Arc::new(OneBoard))
            .refresh_board(1)
            .await;
        let packet = session.poll(1).await;
        assert!(packet.starts_with(r#"42["refresh_board",{"#), "{packet}");
    }

    #[tokio::test]
    async fn sync_boards_includes_boards_without_tiles() {
        let hub = Arc::new(Hub::new());
        let session = hub.create(Arc::new(OneBoard), false).await;
        EditorBroadcaster::new(hub.clone(), Arc::new(OneBoard))
            .sync_boards()
            .await;
        let packet = session.poll(1).await;
        let body = packet
            .strip_prefix(r#"42["get_shortcuts","#)
            .and_then(|p| p.strip_suffix(']'))
            .unwrap();
        let payload: Value = serde_json::from_str(body).unwrap();
        for room in ["basic", "pro"] {
            let boards = payload[room].as_array().unwrap();
            assert_eq!(boards.len(), 2, "{room}");
            assert_eq!(boards[0]["name"], "Main");
            assert_eq!(boards[1]["name"], "Empty");
        }
    }

    #[tokio::test]
    async fn broadcasts_without_sessions_are_harmless() {
        let hub = Arc::new(Hub::new());
        let bc = EditorBroadcaster::new(hub, Arc::new(OneBoard));
        bc.refresh_board(1).await;
        bc.sync_boards().await;
    }

    #[test]
    fn boards_payload_keeps_board_order_and_tolerates_missing_rows() {
        let boards = vec![board(2, "B"), board(1, "A")];
        let buttons: HashMap<i64, Vec<pulpit_db::ButtonRow>> = HashMap::new();
        let mapper = Mapper::new();
        let out = boards_payload(&boards, &buttons, &mapper, true);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], "B");
        assert_eq!(out[1]["name"], "A");
        assert!(boards_payload(&[], &buttons, &mapper, false).is_empty());
    }
}
