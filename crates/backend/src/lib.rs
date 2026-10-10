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
mod http_request;

use pulpit_legacy::service::Backend;
use pulpit_spotify::SpotifyError;
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
    discord_settings_mtime: Mutex<Option<std::time::SystemTime>>,
    /// Keep-alive Discord connection actor, built on first use: one
    /// authenticated pipe for the process lifetime instead of a fresh
    /// ~450 ms session per click.
    discord_client: Mutex<Option<pulpit_discord::DiscordClient>>,
    /// Native Spotify integration handle, injected by the host when
    /// `spotify.json` provides a login; None leaves Spotify actions
    /// claimed-but-logged-out (the design's NeedsLogin surface).
    spotify: Option<pulpit_spotify::Spotify>,
    /// Native OBS websocket handle, injected by the host when it built
    /// the producer pump; None leaves OBS actions claimed-but-warned.
    obs: Option<pulpit_obs::Obs>,
    /// The user-facing message of the last failed Spotify exec/slider
    /// (design §3 error mapping). Cleared on every exec/slider entry and
    /// taken by the host's exec command so a desktop tap surfaces it as
    /// a toast; tablets keep their log-only path.
    spotify_last_error: std::sync::Mutex<Option<String>>,
    http_last_error: std::sync::Mutex<Option<String>>,
    custom_values: std::sync::Mutex<std::collections::HashMap<String, String>>,
    tools: Option<std::sync::Arc<pulpit_tools::ToolManager>>,
    /// Kinds already greeted with the one "not available in Pulpit"
    /// warning this run (SLOBS / XSplit / Twitch): the warning is per
    /// kind per run, never per press.
    unavailable_warned: std::sync::Mutex<std::collections::HashSet<String>>,
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
            discord_settings_mtime: Mutex::new(None),
            discord_client: Mutex::new(None),
            spotify: None,
            obs: None,
            spotify_last_error: std::sync::Mutex::new(None),
            http_last_error: std::sync::Mutex::new(None),
            custom_values: std::sync::Mutex::new(std::collections::HashMap::new()),
            tools: None,
            unavailable_warned: std::sync::Mutex::new(std::collections::HashSet::new()),
            speaker: Mutex::new(None),
            http_agent: pulpit_db::http_agent(std::time::Duration::from_secs(10), true),
        }
    }

    pub fn with_discord(
        mut self,
        config: Option<DiscordConfig>,
        settings_path: std::path::PathBuf,
    ) -> Self {
        let mtime = settings_path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok());
        self.discord = Mutex::new(config);
        self.discord_settings_path = Some(settings_path);
        self.discord_settings_mtime = Mutex::new(mtime);
        self
    }

    pub fn with_voicemeeter_override(self, override_path: Option<std::path::PathBuf>) -> Self {
        self.voicemeeter
            .lock()
            .unwrap()
            .set_override_path(override_path);
        self
    }

    pub fn swap_discord_config(&self, config: Option<DiscordConfig>) {
        *self.discord.lock().unwrap() = config;
        *self.discord_client.lock().unwrap() = None;
        if let Some(path) = &self.discord_settings_path {
            if let Ok(meta) = path.metadata() {
                if let Ok(mtime) = meta.modified() {
                    *self.discord_settings_mtime.lock().unwrap() = Some(mtime);
                }
            }
        }
    }

    pub fn get_discord_config(&self) -> Option<DiscordConfig> {
        self.discord.lock().unwrap().clone()
    }

    pub fn discord_settings_path(&self) -> Option<&std::path::Path> {
        self.discord_settings_path.as_deref()
    }

    pub fn vm_status(&self) -> pulpit_vm::VoicemeeterStatus {
        self.voicemeeter.lock().unwrap().status()
    }

    pub fn vm_devices(&self) -> (Vec<pulpit_vm::DeviceItem>, Vec<pulpit_vm::DeviceItem>) {
        self.voicemeeter.lock().unwrap().devices()
    }

    pub fn vm_reconnect(&self) -> pulpit_vm::Result<()> {
        self.voicemeeter.lock().unwrap().reconnect()
    }

    pub fn vm_run(&self, vm_type: Option<i32>) -> pulpit_vm::Result<()> {
        self.voicemeeter.lock().unwrap().run_voicemeeter(vm_type)
    }

    pub fn vm_set_override(&self, path: Option<std::path::PathBuf>) {
        self.voicemeeter.lock().unwrap().set_override_path(path);
    }

    /// Attach the native Spotify integration (None = no `spotify.json`
    /// / not logged in: spotify tile presses still resolve to a typed
    /// NeedsLogin instead of falling through to the macro dispatcher).
    pub fn with_spotify(mut self, spotify: Option<pulpit_spotify::Spotify>) -> Self {
        self.spotify = spotify;
        self
    }

    pub fn with_tools(mut self, tools: Option<std::sync::Arc<pulpit_tools::ToolManager>>) -> Self {
        self.tools = tools;
        self
    }

    /// Attach the native OBS integration (None = no obs.json / disabled
    /// at the file level is still Some - the handle parks itself; None
    /// means the host could not build the producer pump at all).
    pub fn with_obs(mut self, obs: Option<pulpit_obs::Obs>) -> Self {
        self.obs = obs;
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

    /// Persist a new board order (sidebar drag & drop); returns the ids
    /// whose stored order actually changed.
    pub fn reorder_boards(&self, ordered_ids: &[i64]) -> pulpit_db::Result<Vec<i64>> {
        self.db.lock().unwrap().set_board_order(ordered_ids)
    }

    /// Delete the board and its shortcuts.
    pub fn delete_board(&self, board_id: i64) -> pulpit_db::Result<()> {
        self.db.lock().unwrap().delete_board(board_id)
    }

    /// New 1x1 button placed at (x, y) in `button` mode.
    /// Ids of every button, for the tools store's lazy GC of entries
    /// whose tile was deleted. Best effort: a failed read keeps everything.
    pub fn all_button_ids(&self) -> Vec<i64> {
        match self.db.lock().unwrap().all_button_ids() {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("all_button_ids failed: {e}");
                Vec::new()
            }
        }
    }

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
    pub fn move_button_to_board(
        &self,
        id: i64,
        board_id: i64,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
    ) -> pulpit_db::Result<()> {
        self.db
            .lock()
            .unwrap()
            .update_button_board_and_geometry(id, board_id, x, y, w, h)
    }

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
    ///
    /// Inputs are bounded up front (see [`MAX_BOARD_DIM`]): a hostile or
    /// corrupt file is rejected before anything is inserted, so no import
    /// can fan out into huge grids downstream (wire-builder filler loops).
    pub fn import_boards(&self, boards: &[serde_json::Value]) -> pulpit_db::Result<Vec<i64>> {
        const MAX_BOARDS_PER_IMPORT: usize = 100;
        const MAX_BUTTONS_PER_BOARD: usize = 1024;
        if boards.len() > MAX_BOARDS_PER_IMPORT {
            return Err(pulpit_db::DbError::Corrupt(format!(
                "too many boards in one import: {} (max {MAX_BOARDS_PER_IMPORT})",
                boards.len()
            )));
        }
        let db = self.db.lock().unwrap();
        db.with_transaction(|conn_tx| {
            let mut ids = Vec::with_capacity(boards.len());
            for board_json in boards {
                let mut board: BoardRow = serde_json::from_value(board_json.clone())
                    .map_err(|e| pulpit_db::DbError::Corrupt(format!("bad board entry: {e}")))?;
                if board.width < 1
                    || board.height < 1
                    || board.width > MAX_BOARD_DIM
                    || board.height > MAX_BOARD_DIM
                {
                    return Err(pulpit_db::DbError::Corrupt(format!(
                        "board {:?} has dimensions {}x{} outside 1..={MAX_BOARD_DIM}",
                        board.name, board.width, board.height
                    )));
                }
                board.id = 0;
                board.converted = 1;
                let board_id = conn_tx.insert_board_full(&board)?;
                if let Some(serde_json::Value::Array(macros)) = board_json.get("macros") {
                    if macros.len() > MAX_BUTTONS_PER_BOARD {
                        return Err(pulpit_db::DbError::Corrupt(format!(
                            "too many macros on board {:?}: {} (max {MAX_BUTTONS_PER_BOARD})",
                            board.name,
                            macros.len()
                        )));
                    }
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

/// Largest board grid any surface will build: one definition in
/// `pulpit_db`, re-exported here for the import path (which rejects
/// wider/taller boards outright) and both wire builders (audit C4).
pub use pulpit_db::MAX_BOARD_DIM;

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

    fn exec_gesture(&self, button: ButtonRow, gesture: &str, sink: &mut dyn EventSink) {
        let cmd = pulpit_actions::Command::from_row(
            &button.kind,
            button.command.as_deref(),
            button.options.as_deref(),
            &button.mode,
        );
        if pulpit_tools::is_tool_action(&cmd.kind) {
            if let Some(tools) = &self.tools {
                let g = gesture.parse().unwrap_or(pulpit_tools::ToolGesture::Tap);
                tools.execute(button.id, &cmd.kind, cmd.command.as_deref(), g);
            }
            return;
        }
        self.exec(button, false, sink);
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
        //
        // Builtin kinds never reach the extension chain at all: the macro
        // dispatcher owns `key`, `url`, `type`, ... and always claims them,
        // so a JS extension listing the same action name cannot hijack
        // those tiles.
        let builtin = pulpit_actions::is_builtin_kind(&cmd.kind);
        self.spotify_last_error.lock().unwrap().take();
        self.http_last_error.lock().unwrap().take();
        if !builtin && !is_tap_start {
            if pulpit_tools::is_tool_action(&cmd.kind) {
                if let Some(tools) = &self.tools {
                    tools.execute(
                        button.id,
                        &cmd.kind,
                        cmd.command.as_deref(),
                        pulpit_tools::ToolGesture::Tap,
                    );
                }
                return;
            }
            if self.exec_native(&cmd, sink) {
                return;
            }
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
        self.spotify_last_error.lock().unwrap().take();
        self.http_last_error.lock().unwrap().take();
        if self.exec_runcommand(&cmd)
            || self.exec_extension(&cmd, Some(value))
            || self.exec_sysinfo(&cmd)
            || self.exec_aidev(&cmd)
            || self.exec_callurl(&cmd)
            || self.exec_voicemeeter(&cmd, Some(value))
            || self.exec_spotify(&cmd, Some(value))
            || self.exec_obs(&cmd, Some(value))
            || self.exec_speaker_volume(&cmd, value)
            || self.exec_media(&cmd, Some(value))
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
    /// presses and multiaction steps: run-command, extension, sysinfo,
    /// aidev, callurl, voicemeeter, discord, spotify, speaker, play,
    /// system media. Returns true when one of them claimed the command
    /// (the builtin dispatcher is skipped, mirroring the original
    /// `runCommand` default case).
    fn exec_native(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        self.exec_runcommand(cmd)
            || self.exec_extension(cmd, None)
            // unavailable integrations sit right behind the extensions:
            // a package that actually provides slobs-*/twitch-* must win
            || self.exec_unavailable(cmd)
            || self.exec_sysinfo(cmd)
            || self.exec_aidev(cmd)
            || self.exec_callurl(cmd)
            || self.exec_http(cmd, sink)
            || self.exec_voicemeeter(cmd, None)
            || self.exec_discord(cmd, sink)
            || self.exec_spotify(cmd, None)
            || self.exec_obs(cmd, None)
            || self.exec_speaker(cmd, sink)
            || self.exec_play(cmd)
            || self.exec_tool(cmd)
            || self.exec_media(cmd, None)
    }

    fn exec_tool(&self, cmd: &pulpit_actions::Command) -> bool {
        pulpit_tools::is_tool_action(&cmd.kind)
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

    /// Native http-request: full HTTP request action.
    fn exec_http(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        if cmd.kind != "http-request" {
            return false;
        }
        http_request::execute_http_action(self, cmd, sink);
        true
    }

    /// Native run-command: `{"commandAction": "<shell line>"}` executed
    /// through `cmd /C`, what the deckboard-commands JS package did via
    /// `child_process.exec`. Sits ahead of the extension chain so the
    /// 12 MB package is never extracted or evaluated for a press; the
    /// package stays loaded for its input metadata (the editor's action
    /// list and tile styles), only its execute is replaced.
    fn exec_runcommand(&self, cmd: &pulpit_actions::Command) -> bool {
        if cmd.kind != "run-command" {
            return false;
        }
        // mirror ExtManager::execute's arg parsing: unparsable command
        // JSON becomes a plain string, so `{commandAction}` is missing and
        // the action is a no-op (the JS `execute` destructured the same way)
        let args = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok());
        let Some(what) = args
            .as_ref()
            .and_then(|v| v.get("commandAction"))
            .and_then(serde_json::Value::as_str)
        else {
            tracing::debug!(kind = %cmd.kind, "run-command without commandAction - nothing to run");
            return true;
        };
        if what.trim().is_empty() {
            return true;
        }
        // fire and forget like child_process.exec: the press returns as
        // soon as the shell starts. No pipes either - a program the shell
        // launches (notepad, a server script) inherits piped handles, and
        // waiting for their EOF would hold the press until it exits.
        let child = shell_command(what)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let what = what.to_string();
                // the original popped a "Command Failed!" dialog; the host
                // has no dialogs, so a reaper logs the failure instead
                let reaper = std::thread::Builder::new()
                    .name("run-command-reaper".into())
                    .spawn(move || match child.wait() {
                        Ok(status) if !status.success() => {
                            tracing::warn!(command = %what, %status, "run-command failed");
                        }
                        Ok(_) => tracing::debug!(command = %what, "run-command finished"),
                        Err(e) => {
                            tracing::warn!(command = %what, error = %e, "run-command wait failed")
                        }
                    });
                if let Err(e) = reaper {
                    tracing::warn!(error = %e, "run-command reaper thread failed to start");
                }
            }
            Err(e) => tracing::warn!(command = %what, error = %e, "run-command spawn failed"),
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
        self.maybe_reload_hot_settings();
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
    /// Hot reload for settings.json edits that land behind the process's
    /// back (a hand edit, or the desktop settings panel writing the file
    /// the headless server also reads). The mtime is checked on the next
    /// voicemeeter/discord action - a cheap stat, no file watcher - and a
    /// change re-reads the Discord config and the Voicemeeter DLL override
    /// from the file. Simplest correct option over file-watch: config only
    /// matters when an action runs anyway.
    fn maybe_reload_hot_settings(&self) {
        let Some(path) = &self.discord_settings_path else {
            return;
        };
        let Ok(meta) = path.metadata() else { return };
        let Ok(mtime) = meta.modified() else { return };
        let mut cached = self.discord_settings_mtime.lock().unwrap();
        if *cached != Some(mtime) {
            *cached = Some(mtime);
            if let Ok(raw) = std::fs::read_to_string(path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let cfg = pulpit_discord::DiscordConfig::from_settings(&val);
                    *self.discord.lock().unwrap() = cfg;
                    *self.discord_client.lock().unwrap() = None;
                    // set_override_path only drops the live VM session
                    // when the override actually changed
                    self.voicemeeter
                        .lock()
                        .unwrap()
                        .set_override_path(pulpit_vm::load_dll_override(&val));
                }
            }
        }
    }

    fn exec_discord(&self, cmd: &pulpit_actions::Command, sink: &mut dyn EventSink) -> bool {
        if !pulpit_discord::is_discord_action(&cmd.kind) {
            return false;
        }
        self.maybe_reload_hot_settings();
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
    /// interactive path blocks until the user answers Discord's popup -
    /// so it must only run when the silent refresh was actually REJECTED
    /// (no refresh token, Discord refused it): a plain network failure
    /// must fail the action without popping a window the offline machine
    /// cannot act on.
    fn reauthorize_discord(
        &self,
        config: &DiscordConfig,
    ) -> Result<DiscordConfig, pulpit_discord::DiscordError> {
        let tokens = match pulpit_discord::refresh(config) {
            Ok(tokens) => tokens,
            Err(e) if refresh_failure_needs_popup(&e) => {
                tracing::info!("discord token refresh unavailable - showing consent popup (confirm it on the desktop)");
                pulpit_discord::authorize(
                    config,
                    std::time::Instant::now() + std::time::Duration::from_secs(180),
                )?
            }
            Err(e) => {
                tracing::warn!(error = %e, "discord token refresh blocked by the network - no consent popup");
                return Err(e);
            }
        };
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

    /// Run one system-media (SMTC) tile action against the active or
    /// options-targeted session (`crates/os` media). The display tile's
    /// tap is the play/pause toggle, the control tile's select value is
    /// the raw transport action, the seek slider passes its 0..1 value.
    /// Failures are logged (fire-and-forget like the Voicemeeter bridge).
    /// Returns true when the kind belongs to this integration.
    fn exec_media(&self, cmd: &pulpit_actions::Command, slider_value: Option<f64>) -> bool {
        let Some((action, target_app)) =
            media_request(&cmd.kind, cmd.command.as_deref(), cmd.options.as_deref())
        else {
            return false;
        };
        if let Err(e) = pulpit_os::media::control(target_app.as_deref(), &action, slider_value) {
            tracing::warn!(kind = %cmd.kind, action = %action, error = %e, "media control failed");
        }
        true
    }

    /// Run one Spotify tile action through the native handle
    /// (`crates/spotify`). Slider kinds arrive here a second time from
    /// [`Backend::slider`] with their 0..1 value. Failures surface as
    /// the design's typed errors (Premium required / no active device /
    /// needs login): logged for the tablet path and stored as the last
    /// error so the host's exec command can read them out for the
    /// desktop toast (see [`SqlBackend::take_last_spotify_error`]).
    /// Returns true when the kind belongs to Spotify.
    fn exec_spotify(&self, cmd: &pulpit_actions::Command, slider_value: Option<f64>) -> bool {
        if !pulpit_spotify::is_spotify_action(&cmd.kind) {
            return false;
        }
        let Some(spotify) = &self.spotify else {
            let message = pulpit_spotify::user_message(&SpotifyError::NeedsLogin);
            tracing::warn!(kind = %cmd.kind, message = %message, "spotify action skipped (not configured)");
            *self.spotify_last_error.lock().unwrap() = Some(message);
            return true;
        };
        let command = cmd.command.as_deref().unwrap_or_default();
        if let Err(e) = spotify.exec(&cmd.kind, command, slider_value) {
            let message = pulpit_spotify::user_message(&e);
            tracing::warn!(kind = %cmd.kind, error = %e, message = %message, "spotify action failed");
            *self.spotify_last_error.lock().unwrap() = Some(message);
        }
        true
    }

    /// Run one OBS tile action through the native websocket handle
    /// (`crates/obs`). The exec is fire-and-forget: the handle queues
    /// the action for its connection actor and returns immediately, so
    /// a slow OBS never blocks the input thread. Slider kinds arrive
    /// here a second time from [`Backend::slider`] with their 0..1
    /// value. Returns true when the kind belongs to OBS.
    fn exec_obs(&self, cmd: &pulpit_actions::Command, slider_value: Option<f64>) -> bool {
        if !pulpit_obs::is_obs_action(&cmd.kind) {
            return false;
        }
        let Some(obs) = &self.obs else {
            // like Spotify's NeedsLogin: claimed, told once, never
            // falling through to the macro dispatcher
            tracing::warn!(
                kind = %cmd.kind,
                "obs action skipped - OBS is not configured (enable it in the Pulpit settings)"
            );
            return true;
        };
        obs.exec(
            &cmd.kind,
            cmd.command.as_deref().unwrap_or_default(),
            slider_value,
        );
        true
    }

    /// Claim the integration kinds Pulpit does not implement (SLOBS,
    /// XSplit, Twitch): stock boards carry those tiles, so they stay
    /// loadable, but a press can only warn - once per kind per run,
    /// not per press. An installed extension that actually provides
    /// the kind wins before this arm runs (see [`SqlBackend::exec_native`]).
    /// Returns whether THIS call emitted the run's single warning.
    fn exec_unavailable(&self, cmd: &pulpit_actions::Command) -> bool {
        let owned = cmd.kind.starts_with("slobs")
            || cmd.kind.starts_with("xsplit")
            || cmd.kind.contains("twitch");
        if !owned {
            return false;
        }
        let mut warned = self.unavailable_warned.lock().unwrap();
        if warned.insert(cmd.kind.clone()) {
            tracing::warn!(
                kind = %cmd.kind,
                "integration not available in Pulpit - it needs an extension that provides it"
            );
            true
        } else {
            false
        }
    }

    /// Take (and clear) the user-facing message of the last failed
    /// Spotify exec/slider. The desktop's exec commands return it as the
    /// command error so the editor's existing flash path shows it;
    /// concurrent taps may race which tap reports, which is fine for a
    /// toast.
    pub fn take_last_spotify_error(&self) -> Option<String> {
        self.spotify_last_error.lock().unwrap().take()
    }

    pub fn take_last_http_error(&self) -> Option<String> {
        self.http_last_error.lock().unwrap().take()
    }

    pub fn set_last_http_error(&self, err: String) {
        *self.http_last_error.lock().unwrap() = Some(err);
    }

    pub fn set_custom_value(&self, key: &str, value: &str) {
        self.custom_values
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub fn get_custom_value(&self, key: &str) -> Option<String> {
        self.custom_values.lock().unwrap().get(key).cloned()
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

/// Should a failed silent token refresh fall back to the interactive
/// consent popup? Only when Discord (or the missing refresh token)
/// actually rejected it. A transport-level failure means the machine is
/// offline or Discord is down: no popup can fix that, and popping one
/// anyway blocks the action thread for its whole timeout for nothing.
fn refresh_failure_needs_popup(err: &pulpit_discord::DiscordError) -> bool {
    !matches!(err, pulpit_discord::DiscordError::Network(_))
}

/// One system-media command parsed: the transport action plus the
/// optional target app. Pure so the grammar is unit-testable without
/// touching WinRT. Grammar:
///
/// - `media-now-playing`: tap toggles play/pause (no command),
/// - `media-control`: the select's raw value (`play-pause` / `next` /
///   `previous` / `stop`; an empty command defaults to `play-pause`),
/// - `media-seek`: the slider's 0..1 value rides `slider_value`,
/// - the optional "Aplikacja" target lives in the options column as
///   `{"app": "<name substring>"}` on every kind.
fn media_request(
    kind: &str,
    command: Option<&str>,
    options: Option<&str>,
) -> Option<(String, Option<String>)> {
    if !pulpit_os::media::is_media_action(kind) {
        return None;
    }
    let target_app = options
        .and_then(|opts| serde_json::from_str::<serde_json::Value>(opts).ok())
        .and_then(|v| {
            v.get("app")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
    let action = match kind {
        "media-now-playing" => "play-pause".to_string(),
        "media-seek" => "seek".to_string(),
        _ => {
            let raw = command.unwrap_or_default().trim().trim_matches('"');
            if raw.is_empty() {
                "play-pause".to_string()
            } else {
                raw.to_string()
            }
        }
    };
    Some((action, target_app))
}

/// Shell runner for `run-command`, byte-for-byte the same invocation the
/// extension host uses (crates/ext host.rs): `cmd /C` on Windows with
/// CREATE_NO_WINDOW so no console flashes per press, `sh -c` elsewhere.
#[cfg(windows)]
fn shell_command(command: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("cmd");
    c.args(["/C", command]).creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> std::process::Command {
    let mut c = std::process::Command::new("sh");
    c.args(["-c", command]);
    c
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
        // Builtin kinds stay on the macro dispatcher even inside
        // multiactions - an extension declaring `key` must not hijack key
        // steps any more than top-level key tiles.
        if pulpit_actions::is_builtin_kind(&cmd.kind) {
            return pulpit_actions::run_command_dispatched(input, sink, cmd, false, self);
        }
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

    // ---- spotify native arm ---------------------------------------------

    /// Scripted transport shared with the Spotify handle the backend
    /// owns, so tests can queue answers and read requests afterwards.
    #[derive(Clone)]
    struct SharedSpotifyFake(std::sync::Arc<pulpit_spotify::http::FakeTransport>);

    impl pulpit_spotify::http::Transport for SharedSpotifyFake {
        fn send(
            &self,
            req: &pulpit_spotify::http::HttpRequest,
        ) -> std::result::Result<
            pulpit_spotify::http::HttpResponse,
            pulpit_spotify::http::TransportError,
        > {
            self.0.send(req)
        }
    }

    fn spotify_backend(
        fake: &SharedSpotifyFake,
    ) -> (SqlBackend, std::path::PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify.json");
        pulpit_spotify::SpotifyConfig {
            client_id: "cid".into(),
            access_token: "ACCESS".into(),
            refresh_token: Some("REFRESH".into()),
            // far-future: no refresh happens during these tests
            expires_at: Some(4_102_444_800),
            user: None,
            product: None,
        }
        .save(&path)
        .unwrap();
        let config = pulpit_spotify::SpotifyConfig::load(&path).unwrap();
        let spotify =
            pulpit_spotify::Spotify::with_transport(config, path.clone(), Box::new(fake.clone()));
        (test_backend().with_spotify(Some(spotify)), path, dir)
    }

    fn control_answer(status: u16) -> pulpit_spotify::http::HttpResponse {
        pulpit_spotify::http::HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn spotify_kinds_reach_the_native_chain() {
        let fake = SharedSpotifyFake(std::sync::Arc::new(
            pulpit_spotify::http::FakeTransport::new(),
        ));
        // playing -> pause; then the pause answer
        fake.0
            .push_json(200, serde_json::json!({ "is_playing": true }));
        fake.0.push(control_answer(204));
        let (backend, _path, _dir) = spotify_backend(&fake);
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        let row = button_row("spotify-playback", Some("play"));
        backend.exec(row, false, &mut RecSink::default());
        // the native arm claimed it: two HTTP calls happened and the
        // macro dispatcher (input effects) never ran
        assert!(input.effects().is_empty());
        let requests = fake.0.requests();
        let urls: Vec<&str> = requests.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].ends_with("/me/player"));
        assert!(urls[1].ends_with("/me/player/pause"));
    }

    #[test]
    fn spotify_slider_kinds_deliver_the_value() {
        let fake = SharedSpotifyFake(std::sync::Arc::new(
            pulpit_spotify::http::FakeTransport::new(),
        ));
        fake.0.push(control_answer(204));
        let (backend, _path, _dir) = spotify_backend(&fake);
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        let mut row = button_row("spotify-volume", None);
        row.mode = "slider".into();
        backend.slider(row, 0.55);
        assert!(input.effects().is_empty());
        assert!(fake
            .0
            .last_url()
            .ends_with("/me/player/volume?volume_percent=55"));
    }

    #[test]
    fn spotify_actions_without_configuration_stay_claimed() {
        // no with_spotify: the kind is still ours (claimed, warned about,
        // never falling through to the macro dispatcher)
        let backend = test_backend();
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());
        backend.exec(
            button_row("spotify-playback", Some("next")),
            false,
            &mut RecSink::default(),
        );
        assert!(input.effects().is_empty());
        // the NeedsLogin user message is readable for the desktop toast
        assert_eq!(
            backend.take_last_spotify_error().as_deref(),
            Some("Log in to Spotify in Pulpit settings")
        );
        // taken, not peeked: the next exec without a failure reads None
        assert_eq!(backend.take_last_spotify_error(), None);
    }

    // ---- obs native arm + unavailable integrations ----------------------

    fn plain_command(kind: &str) -> pulpit_actions::Command {
        pulpit_actions::Command::from_row(kind, None, None, "button")
    }

    #[test]
    fn obs_kinds_stay_claimed_without_configuration() {
        let backend = test_backend();
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());
        backend.exec(
            button_row("obs-scene", Some(r#"{"scene":"Game"}"#)),
            false,
            &mut RecSink::default(),
        );
        backend.exec(
            button_row("obs-studio-mode", None),
            false,
            &mut RecSink::default(),
        );
        assert!(
            input.effects().is_empty(),
            "obs kinds are claimed (warned about), never dispatched to the macro layer"
        );
        // unimplemented obs kinds are NOT claimed: they fall through to
        // the builtin dispatcher's stub arm, like before this integration
        backend.exec(
            button_row("obs-transition", None),
            false,
            &mut RecSink::default(),
        );
        assert!(input.effects().is_empty());
        // the audio slider is claimed on the slider path, with the value
        let mut row = button_row("obs-audio-slider", Some(r#"{"device":"Mic"}"#));
        row.mode = "slider".into();
        backend.slider(row, 0.4);
        assert!(input.effects().is_empty());
    }

    #[test]
    fn unavailable_integrations_warn_once_per_kind_and_stay_claimed() {
        let backend = test_backend();
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());
        for kind in ["slobs-scene", "xsplit-scene", "twitch-slow"] {
            backend.exec(
                button_row(kind, Some(r#"{"scene":"S"}"#)),
                false,
                &mut RecSink::default(),
            );
        }
        assert!(
            input.effects().is_empty(),
            "slobs/xsplit/twitch kinds stay claimed so no macro dispatcher ever fires"
        );
        // the presses above carried each kind's single warning for this
        // run: repeating them stays silent
        assert!(!backend.exec_unavailable(&plain_command("slobs-scene")));
        assert!(!backend.exec_unavailable(&plain_command("slobs-scene")));
        assert!(!backend.exec_unavailable(&plain_command("xsplit-scene")));
        // a kind not seen yet warns exactly once
        assert!(backend.exec_unavailable(&plain_command("twitch-emote-only")));
        assert!(!backend.exec_unavailable(&plain_command("twitch-emote-only")));
        assert!(backend.exec_unavailable(&plain_command("slobs-source")));
        // kinds outside the dead integrations are never claimed here
        assert!(!backend.exec_unavailable(&plain_command("key")));
        assert!(!backend.exec_unavailable(&plain_command("obs-scene")));
    }

    #[test]
    fn an_extension_providing_an_unavailable_kind_wins_over_the_stub() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("slobs-provider");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("index.js"),
            r#"module.exports = ({ setValue }) => ({
                name: "slobs provider",
                inputs: [{ value: "slobs-scene" }],
                execute: function (action) { setValue({ "slobs-taken": action }); }
            });"#,
        )
        .unwrap();
        let (manager, mut events) =
            pulpit_ext::ExtManager::load(dir.path(), &serde_json::Value::Null, &[]);
        let backend = test_backend().with_extensions(manager);
        let input = SharedRecInput::default();
        inject(&backend, input, SharedFakeSpeaker::default());

        backend.exec(
            button_row("slobs-scene", Some(r#"{"scene":"S"}"#)),
            false,
            &mut RecSink::default(),
        );
        // the extension's setValue push proves the extension executed -
        // the unavailable stub never ran for this press
        match events.try_recv() {
            Ok(pulpit_ext::ExtEvent::SetValue(v)) => assert_eq!(v["slobs-taken"], "slobs-scene"),
            other => panic!("expected the extension's setValue event, got {other:?}"),
        }
    }

    #[test]
    fn failed_spotify_exec_records_the_user_message_and_clears_on_next_exec() {
        let fake = SharedSpotifyFake(std::sync::Arc::new(
            pulpit_spotify::http::FakeTransport::new(),
        ));
        fake.0.push(pulpit_spotify::http::HttpResponse {
            status: 403,
            headers: Vec::new(),
            body: serde_json::to_vec(&serde_json::json!({
                "error": { "message": "m", "reason": "PREMIUM_REQUIRED" }
            }))
            .unwrap(),
        });
        let (backend, _path, _dir) = spotify_backend(&fake);
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        backend.exec(
            button_row("spotify-playback", Some("next")),
            false,
            &mut RecSink::default(),
        );
        assert_eq!(
            backend.take_last_spotify_error().as_deref(),
            Some("Spotify Premium required")
        );

        // the next exec starts clean even though nothing changed the
        // error slot since
        fake.0
            .push_json(200, serde_json::json!({ "is_playing": false }));
        fake.0.push(control_answer(204));
        backend.exec(
            button_row("spotify-playback", Some("play")),
            false,
            &mut RecSink::default(),
        );
        assert_eq!(backend.take_last_spotify_error(), None);
    }

    #[test]
    fn multiaction_spotify_steps_run_the_native_chain() {
        let fake = SharedSpotifyFake(std::sync::Arc::new(
            pulpit_spotify::http::FakeTransport::new(),
        ));
        fake.0.push(control_answer(204));
        let (backend, _path, _dir) = spotify_backend(&fake);
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        let row = button_row(
            "multiaction",
            Some(r#"[{"type":"spotify-playback","command":"next"}]"#),
        );
        backend.exec(row, false, &mut RecSink::default());
        assert!(input.effects().is_empty());
        assert!(fake.0.last_url().ends_with("/me/player/next"));
    }

    #[test]
    fn media_request_parses_the_smtc_grammar() {
        // tap on the display tile toggles play/pause; the control tile's
        // select value is the raw action; the slider kind seeks
        assert_eq!(
            media_request("media-now-playing", None, None),
            Some(("play-pause".into(), None))
        );
        assert_eq!(
            media_request("media-control", Some("next"), None),
            Some(("next".into(), None))
        );
        // an emptied / quoted / JSON-quoted command defaults to play-pause
        assert_eq!(
            media_request("media-control", Some(""), None),
            Some(("play-pause".into(), None))
        );
        assert_eq!(
            media_request("media-control", Some("\"stop\""), None),
            Some(("stop".into(), None))
        );
        assert_eq!(
            media_request("media-seek", None, None),
            Some(("seek".into(), None))
        );

        // the optional "Aplikacja" target rides the options column JSON
        assert_eq!(
            media_request(
                "media-control",
                Some("play-pause"),
                Some(r#"{"app":"Spotify"}"#)
            ),
            Some(("play-pause".into(), Some("Spotify".into())))
        );
        // blank app strings mean "no target", not an empty pattern
        assert_eq!(
            media_request("media-now-playing", None, Some(r#"{"app":"  "}"#)),
            Some(("play-pause".into(), None))
        );
        // non-JSON options (other tiles' dialects) parse as no target
        assert_eq!(
            media_request("media-seek", None, Some("windows:5h")),
            Some(("seek".into(), None))
        );

        // foreign kinds stay unclaimed
        assert_eq!(media_request("vol", Some("play"), None), None);
        assert_eq!(media_request("media-future", None, None), None);
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

        let created_button = backend.create_button(board, "key", "button", 0, 0).unwrap();
        backend
            .move_button_to_board(created_button, other, 1, 2, 1, 1)
            .unwrap();
        let moved_across = backend.get_button(created_button).unwrap();
        assert_eq!(moved_across.board_id, other);
        assert_eq!((moved_across.x.unwrap(), moved_across.y.unwrap()), (1, 2));
        backend.delete_board(board).unwrap();
        assert_eq!(backend.get_boards().len(), 1);
    }

    #[test]
    fn all_button_ids_spans_boards_and_tracks_deletes() {
        let backend = test_backend();
        let a = backend.create_board("A", "#2c3e50", 4, 3).unwrap();
        let b = backend.create_board("B", "#2c3e50", 4, 3).unwrap();
        let first = backend
            .create_button(a, "tool-timer", "button", 0, 0)
            .unwrap();
        let second = backend.create_button(b, "url", "button", 0, 0).unwrap();

        let mut ids = backend.all_button_ids();
        ids.sort();
        assert_eq!(ids, vec![first.min(second), first.max(second)]);

        backend.delete_button(second).unwrap();
        assert_eq!(backend.all_button_ids(), vec![first]);
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

    #[test]
    fn network_refresh_failures_skip_the_consent_popup() {
        // offline: no popup (it could not succeed anyway and would block
        // the action thread for the full timeout)
        assert!(!refresh_failure_needs_popup(
            &pulpit_discord::DiscordError::Network("oauth request")
        ));
        // Discord (or the missing refresh token) said no: the popup is
        // the only way forward
        assert!(refresh_failure_needs_popup(
            &pulpit_discord::DiscordError::AuthRejected
        ));
        assert!(refresh_failure_needs_popup(
            &pulpit_discord::DiscordError::Call("oauth error response")
        ));
    }

    /// A JS extension declaring the builtin `key` action must not own key
    /// presses: the builtin dispatcher runs first, so the combo still
    /// reaches the input backend. Without the ordering a package could
    /// swallow (or shadow) every `key`, `url` or `type` tile just by
    /// listing the action name in its inputs.
    #[test]
    fn builtin_key_tiles_are_not_hijacked_by_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("key-grabber");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("index.js"),
            r#"module.exports = {
                name: "key grabber",
                inputs: [{ value: "key" }],
                execute: function (action, args) {}
            };"#,
        )
        .unwrap();
        let (manager, _events) =
            pulpit_ext::ExtManager::load(dir.path(), &serde_json::Value::Null, &[]);
        assert!(manager.has_action("key"), "the package declares `key`");

        let backend = test_backend().with_extensions(manager);
        let input = SharedRecInput::default();
        inject(&backend, input.clone(), SharedFakeSpeaker::default());

        // top-level key tile: a full tap is press AND release
        backend.exec_tap(button_row("key", Some("CTRL+P")), &mut RecSink::default());
        let combo = vec![
            pulpit_actions::KeyName::Control,
            pulpit_actions::KeyName::Char('p'),
        ];
        assert_eq!(
            input.effects(),
            vec![
                pulpit_actions::Effect::KeyDown(combo.clone()),
                pulpit_actions::Effect::KeyUp(combo),
            ],
            "an extension declaring `key` must not swallow the key press"
        );

        // the same protection must hold for key steps inside multiactions
        input.0.lock().unwrap().clear();
        let multi = button_row("multiaction", Some(r#"[{"type":"key","command":"ENTER"}]"#));
        backend.exec(multi, false, &mut RecSink::default());
        assert_eq!(
            input.effects(),
            vec![
                pulpit_actions::Effect::KeyDown(vec![pulpit_actions::KeyName::Return]),
                pulpit_actions::Effect::Sleep(150),
                pulpit_actions::Effect::KeyUp(vec![pulpit_actions::KeyName::Return]),
            ],
            "multiaction key steps must reach the builtin dispatcher too"
        );
    }

    #[test]
    fn run_command_tile_executes_natively_without_any_extension() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.txt");
        // what the deckboard-commands package ran via child_process.exec:
        // a bare shell line; the executor wraps it in `cmd /C` (or `sh -c`)
        // (no quotes around the target: cmd /C mangles nested quotes)
        let (action, probe) = if cfg!(windows) {
            (
                format!("echo pulpit-runcmd > {}", out.display()),
                "pulpit-runcmd",
            )
        } else {
            (
                format!("printf pulpit-runcmd > '{}'", out.display()),
                "pulpit-runcmd",
            )
        };
        let backend = test_backend();
        let command = serde_json::json!({ "commandAction": action }).to_string();
        backend.exec(
            button_row("run-command", Some(&command)),
            false,
            &mut RecSink::default(),
        );
        // the executor does not wait for the shell, so poll for its output
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = String::new();
        while std::time::Instant::now() < deadline {
            seen = std::fs::read_to_string(&out).unwrap_or_default();
            if seen.trim() == probe {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(
            seen.trim(),
            probe,
            "the native run-command executor must run the shell line"
        );
    }

    #[test]
    fn run_command_press_does_not_wait_for_the_program() {
        // a long-lived program (notepad, a server script) must not hold the
        // press: the old JS used async child_process.exec
        let action = if cfg!(windows) {
            "ping -n 6 127.0.0.1"
        } else {
            "sleep 5"
        };
        let backend = test_backend();
        let command = serde_json::json!({ "commandAction": action }).to_string();
        let started = std::time::Instant::now();
        backend.exec(
            button_row("run-command", Some(&command)),
            false,
            &mut RecSink::default(),
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "press blocked for {:?} on a 5 s command",
            started.elapsed()
        );
    }

    #[test]
    fn run_command_without_commandaction_is_a_claimed_noop() {
        let backend = test_backend();
        // slider taps arrive as {"value": v}; the JS execute destructured
        // {commandAction} and did nothing - the native path claims too
        backend.exec(
            button_row("run-command", Some(r#"{"value":50}"#)),
            false,
            &mut RecSink::default(),
        );
        backend.exec(
            button_row("run-command", None),
            false,
            &mut RecSink::default(),
        );
    }

    #[test]
    fn run_command_is_claimed_before_the_extension_chain() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("js-ran.txt");
        let pkg = dir.path().join("commands-lookalike");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("index.js"),
            format!(
                r##"module.exports = {{
                    name: "commands lookalike",
                    inputs: [{{
                        value: "run-command",
                        label: "Run Command",
                        icon: "terminal",
                        color: "#34495e"
                    }}],
                    execute: function (action, args) {{
                        __host_write_file("{}", "js");
                    }}
                }};"##,
                marker.display().to_string().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let (manager, _events) =
            pulpit_ext::ExtManager::load(dir.path(), &serde_json::Value::Null, &[]);
        assert!(manager.has_action("run-command"));
        // The editor's action picker and tile styling read exactly this
        // listing (list_known_inputs / register_extension_input), so the
        // declaration must survive even though the press never runs JS.
        let input = manager
            .inputs()
            .iter()
            .find(|i| i.value == "run-command")
            .expect("run-command stays listed for the editor");
        assert_eq!(input.label.as_deref(), Some("Run Command"));
        assert_eq!(input.icon.as_deref(), Some("terminal"));
        assert_eq!(input.color.as_deref(), Some("#34495e"));

        let backend = test_backend().with_extensions(manager);
        backend.exec(
            button_row(
                "run-command",
                Some(r#"{"commandAction":"echo pulpit-runcmd"}"#),
            ),
            false,
            &mut RecSink::default(),
        );
        assert!(
            !marker.exists(),
            "the JS execute must never run: the native executor claims run-command first"
        );
    }

    // ---- editor/import edge cases ----------------------------------------

    #[test]
    fn command_args_parse_json_or_fall_back_to_null() {
        let cmd = |c: Option<&str>| pulpit_actions::Command::from_row("vm-mute", c, None, "button");
        assert_eq!(
            SqlBackend::command_args(&cmd(Some(r#"{"strip":1}"#))),
            serde_json::json!({"strip": 1})
        );
        assert_eq!(
            SqlBackend::command_args(&cmd(Some("not json"))),
            serde_json::Value::Null
        );
        assert_eq!(
            SqlBackend::command_args(&cmd(Some(""))),
            serde_json::Value::Null
        );
        assert_eq!(
            SqlBackend::command_args(&cmd(None)),
            serde_json::Value::Null
        );
    }

    #[test]
    fn exported_button_rows_drop_only_the_id() {
        let row = ButtonRow {
            id: 42,
            board_id: 7,
            kind: "url".into(),
            title: Some("T".into()),
            ..ButtonRow::default()
        };
        let json = button_json(&row);
        assert!(json.get("id").is_none());
        assert_eq!(json["board_id"], 7);
        assert_eq!(json["type"], "url");
        assert_eq!(json["title"], "T");
    }

    #[test]
    fn export_skips_unknown_ids_and_keeps_request_order() {
        let backend = test_backend();
        let a = backend.create_board("A", "#000000", 2, 2).unwrap();
        let b = backend.create_board("B", "#000000", 3, 1).unwrap();
        backend.create_button(b, "key", "button", 0, 0).unwrap();
        let out = backend.export_boards(&[b, 9_999, a]).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["name"], "B");
        assert_eq!(out[0]["macros"].as_array().unwrap().len(), 1);
        assert_eq!(out[1]["name"], "A");
        assert_eq!(out[1]["macros"], serde_json::json!([]));
        assert!(backend.export_boards(&[]).unwrap().is_empty());
    }

    #[test]
    fn import_forces_converted_and_fresh_ids() {
        let backend = test_backend();
        let mut entry = original_style_board();
        entry["id"] = serde_json::json!(555);
        entry["converted"] = serde_json::json!(0);
        entry["macros"][0]["id"] = serde_json::json!(777);
        let ids = backend.import_boards(&[entry]).unwrap();
        assert_ne!(ids[0], 555);
        let board = backend.get_board(ids[0]).unwrap();
        assert_eq!(board.converted, 1);
        let buttons = backend.get_buttons_by_board(ids[0]);
        assert_ne!(buttons[0].id, 777);
        // the stale board_id from the file is re-parented
        assert_eq!(buttons[0].board_id, ids[0]);
    }

    #[test]
    fn import_returns_ids_in_input_order() {
        let backend = test_backend();
        let mut first = original_style_board();
        first["name"] = "First".into();
        let mut second = original_style_board();
        second["name"] = "Second".into();
        let ids = backend.import_boards(&[first, second]).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(backend.get_board(ids[0]).unwrap().name, "First");
        assert_eq!(backend.get_board(ids[1]).unwrap().name, "Second");
    }

    #[test]
    fn import_without_macros_creates_an_empty_board() {
        let backend = test_backend();
        let mut entry = original_style_board();
        entry.as_object_mut().unwrap().remove("macros");
        let ids = backend.import_boards(&[entry]).unwrap();
        assert!(backend.get_buttons_by_board(ids[0]).is_empty());
        // a non-array macros value is ignored the same way
        let mut odd = original_style_board();
        odd["macros"] = serde_json::json!({"not": "a list"});
        let ids = backend.import_boards(&[odd]).unwrap();
        assert!(backend.get_buttons_by_board(ids[0]).is_empty());
    }

    #[test]
    fn import_rejects_zero_sized_boards_and_too_many_boards() {
        let backend = test_backend();
        let mut zero = original_style_board();
        zero["width"] = serde_json::json!(0);
        assert!(matches!(
            backend.import_boards(&[zero]),
            Err(pulpit_db::DbError::Corrupt(_))
        ));
        let many = vec![original_style_board(); 101];
        let err = backend.import_boards(&many).unwrap_err();
        assert!(err.to_string().contains("too many boards"), "{err}");
        assert!(backend.get_boards().is_empty());
        // exactly at the bound is fine
        let ids = backend
            .import_boards(&vec![original_style_board(); 100])
            .unwrap();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn import_accepts_the_largest_allowed_grid() {
        let backend = test_backend();
        let mut entry = original_style_board();
        entry["width"] = serde_json::json!(MAX_BOARD_DIM);
        entry["height"] = serde_json::json!(MAX_BOARD_DIM);
        assert!(backend.import_boards(&[entry.clone()]).is_ok());
        entry["height"] = serde_json::json!(MAX_BOARD_DIM + 1);
        assert!(backend.import_boards(&[entry]).is_err());
    }

    #[test]
    fn create_button_places_a_1x1_tile() {
        let backend = test_backend();
        let board = backend.create_board("B", "#000000", 4, 3).unwrap();
        let id = backend.create_button(board, "key", "toggle", 3, 2).unwrap();
        let row = backend.get_button(id).unwrap();
        assert_eq!(row.board_id, board);
        assert_eq!(row.kind, "key");
        assert_eq!(row.mode, "toggle");
        assert_eq!((row.x, row.y, row.w, row.h), (Some(3), Some(2), 1, 1));
        // the meta read sees the same row
        assert_eq!(backend.get_button_meta(id).unwrap().id, id);
        backend.delete_button(id).unwrap();
        assert!(backend.get_button(id).is_none());
    }

    #[test]
    fn grouped_reads_cover_every_board() {
        let backend = test_backend();
        let a = backend.create_board("A", "#000000", 2, 2).unwrap();
        let b = backend.create_board("B", "#000000", 2, 2).unwrap();
        backend.create_button(a, "key", "button", 0, 0).unwrap();
        backend.create_button(a, "url", "button", 1, 0).unwrap();
        backend.create_button(b, "url", "button", 0, 0).unwrap();
        let grouped = backend.all_buttons_by_board();
        assert_eq!(grouped[&a].len(), 2);
        assert_eq!(grouped[&b].len(), 1);
        assert!(backend.get_board(9_999).is_none());
    }

    // ---- native chain ----------------------------------------------------

    /// Speaker fake that records every call for the speaker tests.
    #[derive(Clone, Default)]
    struct RecSpeaker(std::sync::Arc<Mutex<Vec<String>>>);
    impl RecSpeaker {
        fn calls(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    impl pulpit_os::Speaker for RecSpeaker {
        fn volume(&mut self) -> pulpit_os::Result<f32> {
            Ok(42.0)
        }
        fn muted(&mut self) -> pulpit_os::Result<bool> {
            Ok(true)
        }
        fn set_volume(&mut self, percent: f32) -> pulpit_os::Result<()> {
            self.0.lock().unwrap().push(format!("volume:{percent}"));
            Ok(())
        }
        fn devices(&mut self) -> pulpit_os::Result<Vec<pulpit_os::AudioDevice>> {
            Ok(vec![pulpit_os::AudioDevice {
                id: "{dev-1}".into(),
                name: "Speakers".into(),
                is_default: true,
            }])
        }
        fn active_device(&mut self) -> pulpit_os::Result<String> {
            Ok("{dev-1}".into())
        }
        fn set_active_device(&mut self, id: &str) -> pulpit_os::Result<()> {
            if id == "broken" {
                return Err(pulpit_os::OsError::Unsupported("broken"));
            }
            self.0.lock().unwrap().push(format!("device:{id}"));
            Ok(())
        }
    }

    fn with_rec_speaker(backend: &SqlBackend) -> (SharedRecInput, RecSpeaker) {
        let input = SharedRecInput::default();
        let speaker = RecSpeaker::default();
        *backend.input.lock().unwrap() = Some(Box::new(input.clone()));
        *backend.speaker.lock().unwrap() = Some(Box::new(speaker.clone()));
        (input, speaker)
    }

    #[test]
    fn speaker_device_switches_and_announces_the_new_id() {
        let backend = test_backend();
        let (input, speaker) = with_rec_speaker(&backend);
        let mut sink = RecSink::default();
        backend.exec(
            button_row("speaker-device", Some(r#"{"speaker":"{dev-2}"}"#)),
            false,
            &mut sink,
        );
        assert_eq!(speaker.calls(), vec!["device:{dev-2}"]);
        assert_eq!(
            sink.third_party,
            vec![("speaker-device".to_string(), "{dev-2}".to_string())]
        );
        assert_eq!(
            sink.app_values,
            vec![("speaker-device".to_string(), "{dev-2}".to_string())]
        );
        assert!(input.effects().is_empty());
    }

    #[test]
    fn speaker_device_ignores_the_tap_start_phase() {
        let backend = test_backend();
        let (_input, speaker) = with_rec_speaker(&backend);
        let button = button_row("speaker-device", Some(r#"{"speaker":"{dev-2}"}"#));
        backend.exec(button.clone(), true, &mut RecSink::default());
        assert!(speaker.calls().is_empty());
        backend.exec(button, false, &mut RecSink::default());
        assert_eq!(speaker.calls().len(), 1);
    }

    #[test]
    fn speaker_device_without_id_or_with_a_failed_switch_announces_nothing() {
        let backend = test_backend();
        let (input, speaker) = with_rec_speaker(&backend);
        let mut sink = RecSink::default();
        backend.exec(button_row("speaker-device", Some("{}")), false, &mut sink);
        backend.exec(button_row("speaker-device", None), false, &mut sink);
        backend.exec(
            button_row("speaker-device", Some(r#"{"speaker":"broken"}"#)),
            false,
            &mut sink,
        );
        assert!(speaker.calls().is_empty());
        assert!(sink.app_values.is_empty() && sink.third_party.is_empty());
        // claimed: never fell through to the macro dispatcher
        assert!(input.effects().is_empty());
    }

    #[test]
    fn speaker_volume_slider_scales_to_percent() {
        let backend = test_backend();
        let (_input, speaker) = with_rec_speaker(&backend);
        backend.slider(button_row("speaker-volume", None), 0.25);
        backend.slider(button_row("speaker-volume", None), 1.0);
        assert_eq!(speaker.calls(), vec!["volume:25", "volume:100"]);
    }

    #[test]
    fn speaker_watchers_read_through_the_shared_instance() {
        let backend = test_backend();
        let (_input, _speaker) = with_rec_speaker(&backend);
        assert_eq!(backend.speaker_status(), (Some(42.0), Some(true)));
        assert_eq!(
            backend.speaker_snapshot(true),
            (Some(42.0), Some(true), Some("{dev-1}".to_string()))
        );
        assert_eq!(
            backend.speaker_snapshot(false),
            (Some(42.0), Some(true), None)
        );
        assert_eq!(backend.speaker_device_id().as_deref(), Some("{dev-1}"));
        assert_eq!(
            backend.speaker_devices(),
            vec![("{dev-1}".to_string(), "Speakers".to_string())]
        );
        // the trait object path (what the legacy watchers call) agrees
        let dyn_backend: &dyn Backend = &backend;
        assert_eq!(dyn_backend.speaker_status(), (Some(42.0), Some(true)));
        assert_eq!(dyn_backend.speaker_device_id().as_deref(), Some("{dev-1}"));
    }

    #[test]
    fn display_only_kinds_are_claimed_no_ops() {
        let backend = test_backend();
        let (input, speaker) = with_rec_speaker(&backend);
        for kind in ["si-cpu", "si-ram", "ai-plan-limits", "ai-agent-status"] {
            backend.exec(button_row(kind, None), false, &mut RecSink::default());
            backend.slider(button_row(kind, None), 0.5);
        }
        assert!(input.effects().is_empty());
        assert!(speaker.calls().is_empty());
    }

    #[test]
    fn play_without_a_path_is_a_claimed_no_op() {
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        backend.exec(button_row("play", None), false, &mut RecSink::default());
        backend.exec(button_row("play", Some("")), false, &mut RecSink::default());
        assert!(input.effects().is_empty());
    }

    #[test]
    fn discord_without_configuration_stays_claimed() {
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        backend.exec(
            button_row("toggle-microphone", None),
            false,
            &mut RecSink::default(),
        );
        assert!(input.effects().is_empty());
        // no keep-alive client was spawned for an unconfigured Discord
        assert!(backend.discord_client.lock().unwrap().is_none());
    }

    #[test]
    fn url_to_call_without_a_url_is_claimed() {
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        backend.exec(
            button_row("url-to-call", Some("{}")),
            false,
            &mut RecSink::default(),
        );
        backend.exec(
            button_row("url-to-call", Some("garbage")),
            false,
            &mut RecSink::default(),
        );
        assert!(input.effects().is_empty());
    }

    #[test]
    fn url_to_call_fires_a_get_at_the_configured_url() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            stream
                .write_all(b"HTTP/1.1 500 Oops\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            line
        });
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        let command = format!(r#"{{"urlToCall":"http://{addr}/hook?x=1"}}"#);
        backend.exec(
            button_row("url-to-call", Some(&command)),
            false,
            &mut RecSink::default(),
        );
        assert_eq!(server.join().unwrap().trim_end(), "GET /hook?x=1 HTTP/1.1");
        // the error status is ignored like the JS fetch() was
        assert!(input.effects().is_empty());
    }

    #[test]
    fn run_command_with_a_blank_line_is_a_claimed_no_op() {
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        backend.exec(
            button_row("run-command", Some(r#"{"commandAction":"   "}"#)),
            false,
            &mut RecSink::default(),
        );
        backend.slider(
            button_row("run-command", Some(r#"{"commandAction":""}"#)),
            0.5,
        );
        assert!(input.effects().is_empty());
    }

    #[test]
    fn unknown_slider_kinds_reach_the_slider_dispatcher_quietly() {
        let backend = test_backend();
        let (input, speaker) = with_rec_speaker(&backend);
        backend.slider(button_row("wheels-volume", None), 0.3);
        backend.slider(button_row("no-such-slider", None), 0.3);
        assert!(input.effects().is_empty());
        assert!(speaker.calls().is_empty());
    }

    #[test]
    fn key_tap_start_presses_and_release_lets_go() {
        let backend = test_backend();
        let (input, _speaker) = with_rec_speaker(&backend);
        let key = button_row("key", Some("ctrl+c"));
        backend.exec(key.clone(), true, &mut RecSink::default());
        backend.exec(key, false, &mut RecSink::default());
        let effects = input.effects();
        assert_eq!(effects.len(), 2, "{effects:?}");
        assert!(matches!(effects[0], pulpit_actions::Effect::KeyDown(_)));
        assert!(matches!(effects[1], pulpit_actions::Effect::KeyUp(_)));
    }

    #[test]
    fn board_tiles_switch_through_the_sink() {
        let backend = test_backend();
        let (_input, _speaker) = with_rec_speaker(&backend);
        let mut sink = RecSink::default();
        backend.exec(button_row("board", Some(r#"{"id":7}"#)), false, &mut sink);
        assert_eq!(sink.boards, vec![7]);
    }

    #[test]
    fn spotify_error_slot_starts_empty() {
        let backend = test_backend();
        assert_eq!(backend.take_last_spotify_error(), None);
    }
}
