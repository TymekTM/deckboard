//! SQLite-backed legacy [`Backend`] shared by the headless server binary and
//! the Tauri desktop editor: execution (exec/slider) plus the editor write
//! path and `.boardjson` import/export.
// The boardjson test fixture nests deep inside serde_json::json!.
#![recursion_limit = "256"]

use std::sync::Mutex;

use pulpit_actions::{EnigoInput, EventSink};
use pulpit_db::{BoardRow, ButtonRow, Db};
use pulpit_discord::DiscordConfig;
use pulpit_ext::ExtManager;
use pulpit_legacy::service::Backend;
use pulpit_vm::VoicemeeterState;

/// SQLite-backed backend. Executions are synchronous (the original robotjs
/// dispatch was too) and run inside `spawn_blocking` on the caller's side;
/// the OS input handle is shared behind a mutex instead of being rebuilt
/// per tap.
pub struct SqlBackend {
    db: Mutex<Db>,
    input: Mutex<Option<Box<dyn pulpit_actions::Input + Send>>>,
    /// Original Deckboard extensions; action types the builtin dispatcher
    /// does not know are handed to whichever extension declared them.
    extensions: Option<std::sync::Arc<ExtManager>>,
    /// Native Voicemeeter remote (replaces the ffi-napi based extension,
    /// which cannot load in our JS host).
    voicemeeter: Mutex<VoicemeeterState>,
    /// Discord local-RPC credentials (the original app's saved OAuth token)
    /// plus the settings path, so fresh tokens can be persisted.
    discord: Mutex<Option<DiscordConfig>>,
    discord_settings_path: Option<std::path::PathBuf>,
    /// Keep-alive Discord connection actor, built on first use: one
    /// authenticated pipe for the process lifetime instead of a fresh
    /// ~450 ms session per click.
    discord_client: Mutex<Option<pulpit_discord::DiscordClient>>,
    /// Default-playback control (volume, mute, device switch), built on
    /// first use - the original's speaker service.
    speaker: Mutex<Option<Box<dyn pulpit_os::Speaker>>>,
    /// Pooled HTTP client for fire-and-forget tiles; shared so repeat
    /// calls reuse connections instead of paying TLS setup per press.
    http_agent: ureq::Agent,
}

impl SqlBackend {
    pub fn new(db: Db) -> Self {
        SqlBackend {
            db: Mutex::new(db),
            input: Mutex::new(None),
            extensions: None,
            voicemeeter: Mutex::new(VoicemeeterState::new()),
            discord: Mutex::new(None),
            discord_settings_path: None,
            discord_client: Mutex::new(None),
            speaker: Mutex::new(None),
            http_agent: ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(10)))
                .build()
                .new_agent(),
        }
    }

    pub fn with_discord(
        mut self,
        config: Option<DiscordConfig>,
        settings_path: std::path::PathBuf,
    ) -> Self {
        self.discord = Mutex::new(config);
        self.discord_settings_path = Some(settings_path);
        self
    }

    pub fn with_extensions(mut self, extensions: std::sync::Arc<ExtManager>) -> Self {
        self.extensions = Some(extensions);
        self
    }

    // ---- editor write path -------------------------------------------------

    /// Create an empty board; `background`/`width`/`height` come from the
    /// editor dialog.
    pub fn create_board(
        &self,
        name: &str,
        background: &str,
        width: i64,
        height: i64,
    ) -> pulpit_db::Result<i64> {
        self.db
            .lock()
            .unwrap()
            .insert_board(name, background, width, height)
    }

    pub fn update_board(&self, board: &BoardRow) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().update_board(board)
    }

    /// Delete the board and its shortcuts.
    pub fn delete_board(&self, board_id: i64) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().delete_board(board_id)
    }

    /// New 1x1 button placed at (x, y) in `button` mode.
    pub fn create_button(
        &self,
        board_id: i64,
        kind: &str,
        mode: &str,
        x: i64,
        y: i64,
    ) -> pulpit_db::Result<i64> {
        let row = ButtonRow {
            board_id,
            kind: kind.to_string(),
            mode: mode.to_string(),
            x: Some(x),
            y: Some(y),
            w: 1,
            h: 1,
            ..ButtonRow::default()
        };
        self.db.lock().unwrap().insert_button(&row)
    }

    pub fn update_button(&self, button: &ButtonRow) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().update_button(button)
    }

    /// Drag/resize from the editor grid.
    pub fn move_button(&self, id: i64, x: i64, y: i64, w: i64, h: i64) -> pulpit_db::Result<()> {
        self.db
            .lock()
            .unwrap()
            .update_button_geometry(id, x, y, w, h)
    }

    pub fn delete_button(&self, id: i64) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().delete_button(id)
    }

    /// Remove every shortcut of a board ("Clear board").
    pub fn clear_board(&self, board_id: i64) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().clear_board(board_id)
    }

    // ---- .boardjson import/export -----------------------------------------

    /// Serialize boards to the original's `.boardjson` shape: an array of
    /// board objects (without `id`) each carrying `macros` - the raw button
    /// rows (without `id`). Key names follow the DB columns (`type`, not
    /// `kind`), so files written by the original app import here and vice
    /// versa.
    pub fn export_boards(&self, ids: &[i64]) -> pulpit_db::Result<Vec<serde_json::Value>> {
        let db = self.db.lock().unwrap();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(board) = db.get_board(*id)? else {
                continue;
            };
            let macros: Vec<serde_json::Value> = db
                .get_buttons_by_board(*id)?
                .iter()
                .map(button_json)
                .collect();
            let mut board_json = serde_json::to_value(&board)
                .map_err(|e| pulpit_db::DbError::Serialize(e.to_string()))?;
            if let Some(obj) = board_json.as_object_mut() {
                obj.remove("id");
            }
            board_json["macros"] = serde_json::Value::Array(macros);
            out.push(board_json);
        }
        Ok(out)
    }

    /// Import `.boardjson` board objects (as produced by `export_boards` or
    /// the original app). `converted` is forced to 1 and buttons are
    /// re-parented to the freshly inserted boards, mirroring the original
    /// import. The whole file lands atomically; new board ids are returned
    /// in input order.
    pub fn import_boards(&self, boards: &[serde_json::Value]) -> pulpit_db::Result<Vec<i64>> {
        let db = self.db.lock().unwrap();
        db.with_transaction(|conn_tx| {
            let mut ids = Vec::with_capacity(boards.len());
            for board_json in boards {
                let mut board: BoardRow = serde_json::from_value(board_json.clone())
                    .map_err(|e| pulpit_db::DbError::Corrupt(format!("bad board entry: {e}")))?;
                board.id = 0;
                board.converted = 1;
                let board_id = conn_tx.insert_board_full(&board)?;
                if let Some(serde_json::Value::Array(macros)) = board_json.get("macros") {
                    for macro_json in macros {
                        let mut button: ButtonRow = serde_json::from_value(macro_json.clone())
                            .map_err(|e| {
                                pulpit_db::DbError::Corrupt(format!("bad macro entry: {e}"))
                            })?;
                        button.id = 0;
                        button.board_id = board_id;
                        conn_tx.insert_button(&button)?;
                    }
                }
                ids.push(board_id);
            }
            Ok(ids)
        })
    }
}

/// One exported button row: the DB columns minus `id`.
fn button_json(button: &ButtonRow) -> serde_json::Value {
    let mut v = serde_json::to_value(button)
        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
    if let Some(obj) = v.as_object_mut() {
        obj.remove("id");
    }
    v
}

impl Backend for SqlBackend {
    fn speaker_status(&self) -> (Option<f32>, Option<bool>) {
        SqlBackend::speaker_status(self)
    }

    fn speaker_device_id(&self) -> Option<String> {
        SqlBackend::speaker_device_id(self)
    }

    fn speaker_snapshot(&self, want_device: bool) -> (Option<f32>, Option<bool>, Option<String>) {
        SqlBackend::speaker_snapshot(self, want_device)
    }

    fn get_boards(&self) -> Vec<BoardRow> {
        match self.db.lock().unwrap().get_boards() {
            Ok(boards) => boards,
            Err(e) => {
                tracing::error!("get_boards failed: {e}");
                Vec::new()
            }
        }
    }

    fn get_board(&self, board_id: i64) -> Option<BoardRow> {
        match self.db.lock().unwrap().get_board(board_id) {
            Ok(board) => board,
            Err(e) => {
                tracing::error!("get_board({board_id}) failed: {e}");
                None
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

    fn all_buttons_by_board(&self) -> std::collections::HashMap<i64, Vec<ButtonRow>> {
        match self.db.lock().unwrap().get_buttons_grouped() {
            Ok(grouped) => grouped,
            Err(e) => {
                tracing::error!("get_buttons_grouped failed: {e}");
                std::collections::HashMap::new()
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

    fn get_button_meta(&self, id: i64) -> Option<ButtonRow> {
        match self.db.lock().unwrap().get_button_meta(id) {
            Ok(button) => button,
            Err(e) => {
                tracing::error!("get_button_meta({id}) failed: {e}");
                None
            }
        }
    }

    fn exec(&self, button: ButtonRow, is_tap_start: bool, sink: &mut dyn EventSink) {
        let cmd = pulpit_actions::Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        // Touch-down (`is_tap_start = true`) only drives held `key` buttons
        // via run_command below; every other kind fires once on release.
        // The tablet client sends exec_shortcut on BOTH phases, so the
        // native dispatcher must honor the same filter as run_command or
        // toggles flip twice per tap and volume steps twice.
        if !is_tap_start && self.exec_native(&cmd, sink) {
            return;
        }
        self.with_input(|input| {
            let _ = pulpit_actions::run_command_dispatched(
                input,
                sink,
                &cmd,
                is_tap_start,
                &mut NativeSteps { backend: self },
            );
        });
    }

    fn slider(&self, button: ButtonRow, value: f64) {
        let cmd = pulpit_actions::Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        if self.exec_extension(&cmd, Some(value))
            || self.exec_sysinfo(&cmd)
            || self.exec_aidev(&cmd)
            || self.exec_callurl(&cmd)
            || self.exec_voicemeeter(&cmd, Some(value))
            || self.exec_speaker_volume(&cmd, value)
        {
            return;
        }
        self.with_input(|input| {
            let _ = pulpit_actions::run_slider_command(input, &cmd, value);
        });
    }
}

impl SqlBackend {
    /// The native/extension dispatch chain, shared by top-level tile
    /// presses and multiaction steps: extension, sysinfo, aidev,
    /// callurl, voicemeeter, discord, speaker, play. Returns true when
    /// one of them claimed the command (the builtin dispatcher is
    /// skipped, mirroring the original `runCommand` default case).
    fn exec_native(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        self.exec_extension(cmd, None)
            || self.exec_sysinfo(cmd)
            || self.exec_aidev(cmd)
            || self.exec_callurl(cmd)
            || self.exec_voicemeeter(cmd, None)
            || self.exec_discord(cmd, sink)
            || self.exec_speaker(cmd, sink)
            || self.exec_play(cmd)
    }

    /// Full local tap for desktop touch mode and the editor's "Run now":
    /// the same press-start/press-end sequence a tablet tap produces, so
    /// `key` tiles press AND release. Every other kind ignores the start
    /// phase (see [`Backend::exec`]) and still fires exactly once.
    pub fn exec_tap(&self, button: ButtonRow, sink: &mut dyn EventSink) {
        self.exec(button.clone(), true, sink);
        // hold duration matches EnigoInput::key_tap's synthesized tap
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.exec(button, false, sink);
    }

    /// Run the action through the extension host if one declared it.
    /// Returns true when handled (the builtin dispatcher is skipped,
    /// mirroring the original `runCommand` default case). Slider taps pass
    /// `{"value": v}` - that is what original slider extensions receive.
    fn exec_extension(&self, cmd: &pulpit_actions::Command, slider_value: Option<f64>) -> bool {
        let Some(ext) = &self.extensions else {
            return false;
        };
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

    /// Parse a command's JSON arguments; unparseable or empty -> null.
    fn command_args(cmd: &pulpit_actions::Command) -> serde_json::Value {
        cmd.command
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or(serde_json::Value::Null)
    }

    /// Native system-info actions (si-cpu, si-ram). The replaced JS
    /// extension's execute() was an empty body - a claimed no-op keeps
    /// tile presses succeeding the same way.
    fn exec_sysinfo(&self, cmd: &pulpit_actions::Command) -> bool {
        if !pulpit_sysinfo::is_sysinfo_action(&cmd.kind) {
            return false;
        }
        pulpit_sysinfo::execute(&cmd.kind);
        true
    }

    /// Native AI dev-work display tiles (plan limits, agent progress):
    /// presses are claimed as no-ops so display tiles never fall through
    /// to the macro dispatcher.
    fn exec_aidev(&self, cmd: &pulpit_actions::Command) -> bool {
        if !pulpit_aidev::is_aidev_action(&cmd.kind) {
            return false;
        }
        pulpit_aidev::execute(&cmd.kind);
        true
    }

    /// Native url-to-call: a fire-and-forget GET, exactly what the JS
    /// package's `fetch(args.urlToCall)` did (response ignored).
    fn exec_callurl(&self, cmd: &pulpit_actions::Command) -> bool {
        if cmd.kind != "url-to-call" {
            return false;
        }
        let url = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| {
                v.get("urlToCall")
                    .and_then(|u| u.as_str())
                    .map(str::to_string)
            });
        match url {
            Some(url) => {
                if let Err(e) = self.http_agent.get(&url).call() {
                    tracing::warn!(url = %url, error = %e, "url-to-call failed");
                }
            }
            None => tracing::warn!(kind = "url-to-call", "tile has no urlToCall configured"),
        }
        true
    }

    /// Run the default-playback control on the lazy speaker instance.
    /// None means the platform has no speaker support.
    fn with_speaker<R>(
        &self,
        f: impl FnOnce(&mut (dyn pulpit_os::Speaker + 'static)) -> R,
    ) -> Option<R> {
        let mut slot = self.speaker.lock().unwrap();
        if slot.is_none() {
            *slot = Some(Box::new(pulpit_os::platform_speaker()));
        }
        slot.as_deref_mut().map(f)
    }

    /// `speaker-device` command: `{"speaker": "<endpoint id>"}` switches
    /// the default output, like the original `setActiveOutputDevice`.
    /// Returns true when the kind belongs to the speaker service.
    fn exec_speaker(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        if cmd.kind != "speaker-device" {
            return false;
        }
        let id = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| {
                v.get("speaker")
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            });
        let Some(id) = id else {
            tracing::warn!(kind = "speaker-device", "tile has no speaker id configured");
            return true;
        };
        self.with_speaker(|sp| {
            if let Err(e) = sp.set_active_device(&id) {
                tracing::warn!(error = %e, "speaker-device switch failed");
                return;
            }
            // the original pushes the new id right after a successful switch
            sink.third_party_value("speaker-device", &id);
            sink.app_value("speaker-device", &id);
        });
        true
    }

    /// Master volume from a `speaker-volume` slider, percent 0..=100 like
    /// the original `setVolume(100 * e)`.
    fn exec_speaker_volume(&self, cmd: &pulpit_actions::Command, value: f64) -> bool {
        if cmd.kind != "speaker-volume" {
            return false;
        }
        self.with_speaker(|sp| {
            if let Err(e) = sp.set_volume(value as f32 * 100.0) {
                tracing::warn!(error = %e, "speaker-volume set failed");
            }
        });
        true
    }

    /// `play` command: start a local audio file, like the original's HTML5
    /// Audio element (restart on every press). Returns true when claimed.
    fn exec_play(&self, cmd: &pulpit_actions::Command) -> bool {
        if cmd.kind != "play" {
            return false;
        }
        let path = cmd.command.as_deref().unwrap_or_default();
        if path.is_empty() {
            return true;
        }
        if let Err(e) = pulpit_os::play_audio(path) {
            tracing::warn!(path = %path, error = %e, "play failed");
        }
        true
    }

    /// Watcher snapshots: master volume in percent / muted flag.
    /// None where the platform has no support.
    pub fn speaker_status(&self) -> (Option<f32>, Option<bool>) {
        self.with_speaker(|sp| (sp.volume().ok(), sp.muted().ok()))
            .unwrap_or((None, None))
    }

    /// Watcher snapshot in one COM pass: volume, mute and (optionally)
    /// the default device id. The per-tick loops use this instead of the
    /// three separate getters.
    pub fn speaker_snapshot(
        &self,
        want_device: bool,
    ) -> (Option<f32>, Option<bool>, Option<String>) {
        self.with_speaker(|sp| sp.status(want_device).ok())
            .flatten()
            .map(|(volume, muted, device)| (Some(volume), Some(muted), device))
            .unwrap_or((None, None, None))
    }

    pub fn speaker_device_id(&self) -> Option<String> {
        self.with_speaker(|sp| sp.active_device().ok()).flatten()
    }

    /// Playback endpoints for the editor's device picker: .
    pub fn speaker_devices(&self) -> Vec<(String, String)> {
        self.with_speaker(|sp| {
            sp.devices()
                .unwrap_or_default()
                .into_iter()
                .map(|d| (d.id, d.name))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Run `vm-*` actions against the Voicemeeter remote DLL. Only tried
    /// when no loaded JS extension claimed the action (the original
    /// voicemeeter-control extension cannot load in our host). Slider
    /// positions arrive as `slider_value` and are injected into the action
    /// arguments as `value` (vm-slider-* fader actions read it from there).
    fn exec_voicemeeter(&self, cmd: &pulpit_actions::Command, slider_value: Option<f64>) -> bool {
        if !pulpit_vm::is_vm_action(&cmd.kind) {
            return false;
        }
        let mut args = Self::command_args(cmd);
        if let Some(v) = slider_value {
            if !args.is_object() {
                args = serde_json::json!({});
            }
            args["value"] = serde_json::json!(v);
        }
        let result = self.voicemeeter.lock().unwrap().execute(&cmd.kind, &args);
        if let Err(e) = &result {
            tracing::warn!(kind = %cmd.kind, error = %e, "voicemeeter action failed");
        }
        true
    }

    /// Run one Discord action over the kept-alive local RPC connection.
    /// On an expired token a silent refresh is tried first; without a
    /// refresh token Discord's consent popup shows on the desktop and the
    /// new tokens are saved to settings.json. Returns true when the action
    /// kind belongs to Discord.
    fn exec_discord(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        if !pulpit_discord::is_discord_action(&cmd.kind) {
            return false;
        }
        let Some(config) = self.discord.lock().unwrap().clone() else {
            tracing::warn!(kind = %cmd.kind, "discord not configured (no client id in settings)");
            return true;
        };
        let args = Self::command_args(cmd);
        // The client lock is held for the whole action on purpose: Discord
        // actions queue up instead of racing the pipe. Worst case is a
        // consent-popup re-authorization blocking later actions until it
        // resolves - the same blocking the popup itself imposes.
        let mut clients = self.discord_client.lock().unwrap();
        let client = clients.get_or_insert_with(pulpit_discord::DiscordClient::spawn);
        let mut run = |config: &DiscordConfig| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            client
                .execute(config, &cmd.kind, &args, deadline)
                .inspect(|o| {
                    if let Some(o) = o {
                        sink.app_value(&o.key, &o.label);
                    }
                })
        };
        let result = match run(&config) {
            Err(pulpit_discord::DiscordError::AuthRejected) => {
                match self.reauthorize_discord(&config) {
                    Ok(fresh) => {
                        tracing::info!("discord re-authorized, retrying action");
                        run(&fresh)
                    }
                    Err(e) => Err(e),
                }
            }
            other => other,
        };
        if let Err(e) = result {
            tracing::warn!(kind = %cmd.kind, error = %e, "discord action failed");
        }
        true
    }

    /// Refresh or re-authorize the Discord token and persist it. The
    /// interactive path blocks until the user answers Discord's popup.
    fn reauthorize_discord(
        &self,
        config: &DiscordConfig,
    ) -> Result<DiscordConfig, pulpit_discord::DiscordError> {
        let tokens = pulpit_discord::refresh(config).or_else(|_| {
            tracing::info!("discord token refresh unavailable - showing consent popup (confirm it on the desktop)");
            pulpit_discord::authorize(config, std::time::Instant::now() + std::time::Duration::from_secs(180))
        })?;
        let fresh = DiscordConfig {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            ..config.clone()
        };
        if let Some(path) = &self.discord_settings_path {
            if let Err(e) = pulpit_discord::save_tokens(
                path,
                &pulpit_discord::AuthTokens {
                    access_token: fresh.access_token.clone(),
                    refresh_token: fresh.refresh_token.clone(),
                },
            ) {
                tracing::warn!(error = %e, "could not save discord tokens to settings.json");
            }
        }
        *self.discord.lock().unwrap() = Some(fresh.clone());
        Ok(fresh)
    }

    fn with_input(&self, f: impl FnOnce(&mut dyn pulpit_actions::Input)) {
        let mut guard = self.input.lock().unwrap();
        if guard.is_none() {
            match EnigoInput::new() {
                Ok(i) => *guard = Some(Box::new(i)),
                Err(e) => {
                    tracing::error!("input backend init failed: {e}");
                    return;
                }
            }
        }
        if let Some(input) = guard.as_deref_mut() {
            f(input);
        }
    }
}

/// Multiaction step dispatcher: every step goes through the same
/// native/extension chain as a top-level tile press ([`SqlBackend::exec_native`]),
/// then falls back to the builtin dispatcher. Nested multiactions keep
/// the chain too.
struct NativeSteps<'a> {
    backend: &'a SqlBackend,
}

impl pulpit_actions::StepDispatch for NativeSteps<'_> {
    fn dispatch_step(
        &mut self,
        input: &mut dyn pulpit_actions::Input,
        sink: &mut dyn pulpit_actions::EventSink,
        cmd: &pulpit_actions::Command,
    ) -> pulpit_actions::Result<()> {
        if self.backend.exec_native(cmd, sink) {
            return Ok(());
        }
        pulpit_actions::run_command_dispatched(input, sink, cmd, false, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_backend() -> SqlBackend {
        SqlBackend::new(Db::open_or_create(std::path::Path::new(":memory:")).unwrap())
    }

    // ---- test seams --------------------------------------------------------
    // Mirror of crates/actions' MockInput (Effect recording) and a fake
    // Speaker, both shared through Arc so tests can read what exec drove
    // after the fact (the real objects live inside the backend's locks).

    #[derive(Clone, Default)]
    struct SharedRecInput(std::sync::Arc<Mutex<Vec<pulpit_actions::Effect>>>);
    impl SharedRecInput {
        fn effects(&self) -> Vec<pulpit_actions::Effect> {
            self.0.lock().unwrap().clone()
        }
    }
    impl pulpit_actions::Input for SharedRecInput {
        fn key_down(&mut self, keys: &[pulpit_actions::KeyName]) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::KeyDown(keys.to_vec()));
            Ok(())
        }
        fn key_up(&mut self, keys: &[pulpit_actions::KeyName]) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::KeyUp(keys.to_vec()));
            Ok(())
        }
        fn key_tap(&mut self, keys: &[pulpit_actions::KeyName]) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::KeyTap(keys.to_vec()));
            Ok(())
        }
        fn text(&mut self, text: &str) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::Text(text.into()));
            Ok(())
        }
        fn mouse_move(&mut self, x: i32, y: i32) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::MouseMove(x, y));
            Ok(())
        }
        fn mouse_click(&mut self, left: bool) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::MouseClick(left));
            Ok(())
        }
        fn media(&mut self, key: pulpit_actions::MediaKey) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::Media(key));
            Ok(())
        }
        fn open_url(&mut self, url: &str) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::OpenUrl(url.into()));
            Ok(())
        }
        fn spawn(&mut self, path: &str, args: &[String]) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::Spawn(path.into(), args.to_vec()));
            Ok(())
        }
        fn sleep(&mut self, ms: u64) -> pulpit_actions::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(pulpit_actions::Effect::Sleep(ms));
            Ok(())
        }
        fn paste_text(&mut self, text: &str) -> pulpit_actions::Result<()> {
            self.text(text)
        }
    }

    #[derive(Clone, Default)]
    struct SharedFakeSpeaker(std::sync::Arc<Mutex<Vec<String>>>);
    impl pulpit_os::Speaker for SharedFakeSpeaker {
        fn volume(&mut self) -> pulpit_os::Result<f32> {
            Ok(50.0)
        }
        fn muted(&mut self) -> pulpit_os::Result<bool> {
            Ok(false)
        }
        fn set_volume(&mut self, _percent: f32) -> pulpit_os::Result<()> {
            Ok(())
        }
        fn devices(&mut self) -> pulpit_os::Result<Vec<pulpit_os::AudioDevice>> {
            Ok(Vec::new())
        }
        fn active_device(&mut self) -> pulpit_os::Result<String> {
            Err(pulpit_os::OsError::Unsupported("fake"))
        }
        fn set_active_device(&mut self, id: &str) -> pulpit_os::Result<()> {
            self.0.lock().unwrap().push(id.to_string());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecSink {
        boards: Vec<i64>,
        app_values: Vec<(String, String)>,
        third_party: Vec<(String, String)>,
    }
    impl pulpit_actions::EventSink for RecSink {
        fn change_board(&mut self, board_id: i64) {
            self.boards.push(board_id);
        }
        fn app_value(&mut self, key: &str, value: &str) {
            self.app_values.push((key.into(), value.into()));
        }
        fn third_party_value(&mut self, key: &str, value: &str) {
            self.third_party.push((key.into(), value.into()));
        }
    }

    fn button_row(kind: &str, command: Option<&str>) -> ButtonRow {
        ButtonRow {
            kind: kind.to_string(),
            command: command.map(str::to_string),
            mode: "button".to_string(),
            ..ButtonRow::default()
        }
    }

    fn inject(backend: &SqlBackend, input: SharedRecInput, speaker: SharedFakeSpeaker) {
        *backend.input.lock().unwrap() = Some(Box::new(input));
        *backend.speaker.lock().unwrap() = Some(Box::new(speaker));
    }

    #[test]
    fn multiaction_steps_run_native_and_builtin_dispatchers() {
        let backend = test_backend();
        let input = SharedRecInput::default();
        let speaker = SharedFakeSpeaker::default();
        inject(&backend, input.clone(), speaker.clone());

        let row = button_row(
            "multiaction",
            Some(
                r#"[
                    {"type":"key","command":"ENTER"},
                    {"type":"speaker-device","command":"{\"speaker\":\"fake-endpoint\"}"},
                    {"type":"delay","command":"25"}
                ]"#,
            ),
        );
        let mut sink = RecSink::default();
        backend.exec(row, false, &mut sink);

        assert_eq!(
            speaker.0.lock().unwrap().clone(),
            vec!["fake-endpoint".to_string()],
            "native speaker steps must run inside a multiaction"
        );
        assert_eq!(
            sink.third_party,
            vec![("speaker-device".to_string(), "fake-endpoint".to_string())]
        );
        assert_eq!(
            input.effects(),
            vec![
                pulpit_actions::Effect::KeyDown(vec![pulpit_actions::KeyName::Return]),
                pulpit_actions::Effect::Sleep(150),
                pulpit_actions::Effect::KeyUp(vec![pulpit_actions::KeyName::Return]),
                pulpit_actions::Effect::Sleep(25),
            ]
        );
    }

    #[test]
    fn exec_tap_presses_and_releases_keys_and_fires_other_kinds_once() {
        let backend = test_backend();
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        // key tile: the tap sequence must press AND release, like a
        // tablet tap (press-start drives key_down, release drives key_up)
        let key = button_row("key", Some("CTRL+SHIFT+P"));
        backend.exec_tap(key, &mut RecSink::default());
        let combo = vec![
            pulpit_actions::KeyName::Control,
            pulpit_actions::KeyName::Shift,
            pulpit_actions::KeyName::Char('p'),
        ];
        assert_eq!(
            input.effects(),
            vec![
                pulpit_actions::Effect::KeyDown(combo.clone()),
                pulpit_actions::Effect::KeyUp(combo),
            ]
        );

        // every other kind fires exactly once: the press-start phase is
        // a no-op for them, so a url tile must not open twice
        input.0.lock().unwrap().clear();
        let url = button_row("url", Some("https://example.com"));
        backend.exec_tap(url, &mut RecSink::default());
        assert_eq!(
            input.effects(),
            vec![pulpit_actions::Effect::OpenUrl(
                "https://example.com".into()
            )]
        );
    }

    /// A boardjson entry shaped like the original app writes it: `type`
    /// column names and no ids anywhere.
    fn original_style_board() -> serde_json::Value {
        serde_json::json!({
            "name": "Stream",
            "background": "#2c3e50",
            "layout": 6,
            "image": "",
            "sort": "",
            "type": "buttons",
            "args": null,
            "order": 0,
            "width": 4,
            "height": 3,
            "converted": 1,
            "macros": [
                {
                    "board_id": 3,
                    "type": "url",
                    "command": "https://example.com",
                    "title": "Example",
                    "title_position": 0,
                    "title_color": "#ffffff",
                    "title_box_color": "",
                    "color": "#8e44ad",
                    "icon_color": "",
                    "icon_color2": "",
                    "border_color": "",
                    "shape": 0,
                    "icon": null,
                    "img": "",
                    "img2": "",
                    "icon2": null,
                    "color2": "",
                    "shape2": 0,
                    "border_color2": "",
                    "title_position2": 0,
                    "title_box_color2": "",
                    "title_color2": "",
                    "position": null,
                    "position2": 0,
                    "mode": "button",
                    "x": 0,
                    "y": 0,
                    "w": 1,
                    "h": 1,
                    "options": null
                }
            ]
        })
    }

    #[test]
    fn editor_writes_land_in_the_database() {
        let backend = test_backend();

        let board = backend.create_board("New Board", "#2c3e50", 4, 3).unwrap();
        let button = backend.create_button(board, "url", "button", 2, 1).unwrap();

        let mut row = backend.get_button(button).unwrap();
        row.title = Some("Edited".into());
        row.color = Some("#c0392b".into());
        backend.update_button(&row).unwrap();
        let stored = backend.get_button(button).unwrap();
        assert_eq!(stored.title.as_deref(), Some("Edited"));
        assert_eq!(stored.color.as_deref(), Some("#c0392b"));

        backend.move_button(button, 0, 2, 2, 2).unwrap();
        let moved = backend.get_button(button).unwrap();
        assert_eq!(
            (moved.x.unwrap(), moved.y.unwrap(), moved.w, moved.h),
            (0, 2, 2, 2)
        );

        backend.clear_board(board).unwrap();
        assert!(backend.get_buttons_by_board(board).is_empty());

        let other = backend.create_board("Keeper", "#2c3e50", 4, 3).unwrap();
        backend.create_button(other, "key", "button", 0, 0).unwrap();
        backend.delete_board(board).unwrap();
        assert_eq!(backend.get_boards().len(), 1);
    }

    #[test]
    fn boardjson_import_export_roundtrip() {
        let backend = test_backend();

        let ids = backend.import_boards(&[original_style_board()]).unwrap();
        assert_eq!(ids.len(), 1);

        // macros were re-parented to the new board and ids assigned
        let buttons = backend.get_buttons_by_board(ids[0]);
        assert_eq!(buttons.len(), 1);
        let button = &buttons[0];
        assert_eq!(button.kind, "url");
        assert_eq!(button.board_id, ids[0]);
        assert_eq!(button.title.as_deref(), Some("Example"));

        // export produces the original's file shape: no ids, macros key
        let exported = backend.export_boards(&[ids[0]]).unwrap();
        assert_eq!(exported.len(), 1);
        let board = &exported[0];
        assert!(board.get("id").is_none());
        assert_eq!(board["type"], "buttons");
        let macros = board["macros"].as_array().unwrap();
        assert_eq!(macros.len(), 1);
        assert!(macros[0].get("id").is_none());
        assert_eq!(macros[0]["type"], "url");
        assert_eq!(macros[0]["title"], "Example");

        // re-importing the export doubles the boards with intact content
        let ids2 = backend.import_boards(&exported).unwrap();
        assert_ne!(ids2[0], ids[0]);
        assert_eq!(backend.get_buttons_by_board(ids2[0]).len(), 1);
    }

    #[test]
    fn import_is_atomic_on_bad_entry() {
        let backend = test_backend();
        let bad = serde_json::json!({ "name": 42, "macros": "nope" });
        let err = backend
            .import_boards(&[original_style_board(), bad])
            .unwrap_err();
        assert!(matches!(err, pulpit_db::DbError::Corrupt(_)));
        // the good board before the bad one was rolled back
        assert!(backend.get_boards().is_empty());
    }
}
