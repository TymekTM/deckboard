//! SQLite-backed implementation of the legacy [`Backend`] trait.

use std::sync::{Arc, Mutex};

use deckboard_actions::{Command, EnigoInput, EventSink};
use deckboard_db::{ButtonRow, Db};
use deckboard_ext::ExtManager;
use deckboard_legacy::service::Backend;
use deckboard_vm::VoicemeeterState;

/// SQLite-backed backend. Executions are synchronous (the original robotjs
/// dispatch was too) and run inside `spawn_blocking` on the caller's side;
/// the OS input handle is shared behind a mutex instead of being rebuilt
/// per tap.
pub struct SqlBackend {
    db: Mutex<Db>,
    input: Mutex<Option<EnigoInput>>,
    /// Original Deckboard extensions; action types the builtin dispatcher
    /// does not know are handed to whichever extension declared them.
    extensions: Option<Arc<ExtManager>>,
    /// Native Voicemeeter remote (replaces the ffi-napi based extension,
    /// which cannot load in our JS host).
    voicemeeter: Mutex<VoicemeeterState>,
}

impl SqlBackend {
    pub fn new(db: Db) -> Self {
        SqlBackend {
            db: Mutex::new(db),
            input: Mutex::new(None),
            extensions: None,
            voicemeeter: Mutex::new(VoicemeeterState::new()),
        }
    }

    pub fn with_extensions(mut self, extensions: Arc<ExtManager>) -> Self {
        self.extensions = Some(extensions);
        self
    }

    /// Run the action through the extension host if one declared it.
    /// Returns true when handled (the builtin dispatcher is skipped,
    /// mirroring the original `runCommand` default case). Slider taps pass
    /// `{"value": v}` - that is what original slider extensions receive.
    fn exec_extension(&self, cmd: &Command, slider_value: Option<f64>) -> bool {
        let Some(ext) = &self.extensions else { return false };
        if !ext.has_action(&cmd.kind) {
            return false;
        }
        let command = match slider_value {
            Some(v) => Some(format!(r#"{{"value":{v}}}"#)),
            None => cmd.command.clone(),
        };
        match ext.execute(&cmd.kind, command.as_deref()) {
            Ok(()) => {}
            // the original shows a dialog and stops; we log and stop too
            Err(e) => tracing::warn!(kind = %cmd.kind, error = %e, "extension execute failed"),
        }
        true
    }

    /// Run `vm-*` actions against the Voicemeeter remote DLL. Only tried
    /// when no loaded JS extension claimed the action (the original
    /// voicemeeter-control extension cannot load in our host).
    fn exec_voicemeeter(&self, cmd: &Command) -> bool {
        if !deckboard_vm::is_vm_action(&cmd.kind) {
            return false;
        }
        let args = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or(serde_json::Value::Null);
        let result = self
            .voicemeeter
            .lock()
            .unwrap()
            .execute(&cmd.kind, &args);
        if let Err(e) = &result {
            tracing::warn!(kind = %cmd.kind, error = %e, "voicemeeter action failed");
        }
        true
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
        if self.exec_extension(&cmd, None) || self.exec_voicemeeter(&cmd) {
            return;
        }
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
        if self.exec_extension(&cmd, Some(value)) || self.exec_voicemeeter(&cmd) {
            return;
        }
        self.with_input(|input| {
            let _ = deckboard_actions::run_slider_command(input, &cmd, value);
        });
    }
}
