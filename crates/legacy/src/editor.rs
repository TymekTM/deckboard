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
