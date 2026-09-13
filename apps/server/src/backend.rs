//! SQLite-backed implementation of the legacy [`Backend`] trait.

use std::sync::Mutex;

use deckboard_actions::{Command, EventSink};
use deckboard_db::{ButtonRow, Db};
use deckboard_legacy::service::Backend;

/// Runs commands on a dedicated thread pool slot so a slow action never
/// blocks the socket handlers. M0 is synchronous by design (the original
/// robotjs dispatch was synchronous too).
pub struct SqlBackend {
    db: Mutex<Db>,
}

impl SqlBackend {
    pub fn new(db: Db) -> Self {
        SqlBackend { db: Mutex::new(db) }
    }
}

impl Backend for SqlBackend {
    fn get_boards(&self) -> Vec<deckboard_db::BoardRow> {
        self.db.lock().map(|db| db.get_boards().unwrap_or_default()).unwrap_or_default()
    }

    fn get_buttons_by_board(&self, board_id: i64) -> Vec<ButtonRow> {
        self.db
            .lock()
            .map(|db| db.get_buttons_by_board(board_id).unwrap_or_default())
            .unwrap_or_default()
    }

    fn get_button(&self, id: i64) -> Option<ButtonRow> {
        self.db.lock().ok()?.get_button(id).ok().flatten()
    }

    fn exec(&self, button: ButtonRow, is_tap_start: bool, sink: &mut dyn EventSink) {
        let cmd = Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        let mut input = match deckboard_actions::EnigoInput::new() {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("input backend init failed: {e}");
                return;
            }
        };
        let _ = deckboard_actions::run_command(&mut input, sink, &cmd, is_tap_start);
    }

    fn slider(&self, button: ButtonRow, value: f64) {
        let cmd = Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        let mut input = match deckboard_actions::EnigoInput::new() {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("input backend init failed: {e}");
                return;
            }
        };
        let _ = deckboard_actions::run_slider_command(&mut input, &cmd, value);
    }
}
