//! SQLite-backed implementation of the legacy [`Backend`] trait.

use std::sync::Mutex;

use deckboard_actions::{Command, EnigoInput, EventSink};
use deckboard_db::{ButtonRow, Db};
use deckboard_legacy::service::Backend;

/// SQLite-backed backend. Executions are synchronous (the original robotjs
/// dispatch was too) and run inside `spawn_blocking` on the caller's side;
/// the OS input handle is shared behind a mutex instead of being rebuilt
/// per tap.
pub struct SqlBackend {
    db: Mutex<Db>,
    input: Mutex<Option<EnigoInput>>,
}

impl SqlBackend {
    pub fn new(db: Db) -> Self {
        SqlBackend {
            db: Mutex::new(db),
            input: Mutex::new(None),
        }
    }

    fn with_input(&self, f: impl FnOnce(&mut EnigoInput)) {
        let mut guard = self.input.lock().unwrap();
        if guard.is_none() {
            match EnigoInput::new() {
                Ok(i) => *guard = Some(i),
                Err(e) => {
                    tracing::error!("input backend init failed: {e}");
                    return;
                }
            }
        }
        if let Some(input) = guard.as_mut() {
            f(input);
        }
    }
}

impl Backend for SqlBackend {
    fn get_boards(&self) -> Vec<deckboard_db::BoardRow> {
        match self.db.lock().unwrap().get_boards() {
            Ok(boards) => boards,
            Err(e) => {
                tracing::error!("get_boards failed: {e}");
                Vec::new()
            }
        }
    }

    fn get_buttons_by_board(&self, board_id: i64) -> Vec<ButtonRow> {
        match self.db.lock().unwrap().get_buttons_by_board(board_id) {
            Ok(buttons) => buttons,
            Err(e) => {
                tracing::error!("get_buttons_by_board({board_id}) failed: {e}");
                Vec::new()
            }
        }
    }

    fn get_button(&self, id: i64) -> Option<ButtonRow> {
        match self.db.lock().unwrap().get_button(id) {
            Ok(button) => button,
            Err(e) => {
                tracing::error!("get_button({id}) failed: {e}");
                None
            }
        }
    }

    fn exec(&self, button: ButtonRow, is_tap_start: bool, sink: &mut dyn EventSink) {
        let cmd = Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        self.with_input(|input| {
            let _ = deckboard_actions::run_command(input, sink, &cmd, is_tap_start);
        });
    }

    fn slider(&self, button: ButtonRow, value: f64) {
        let cmd = Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        self.with_input(|input| {
            let _ = deckboard_actions::run_slider_command(input, &cmd, value);
        });
    }
}
