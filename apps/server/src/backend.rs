//! SQLite-backed implementation of the legacy [`Backend`] trait.

use std::sync::{Arc, Mutex};

use deckboard_actions::{Command, EnigoInput, EventSink};
use deckboard_db::{ButtonRow, Db};
use deckboard_discord::DiscordConfig;
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
    /// Discord local-RPC credentials (the original app's saved OAuth token)
    /// plus the settings path, so fresh tokens can be persisted.
    discord: Mutex<Option<DiscordConfig>>,
    discord_settings_path: Option<std::path::PathBuf>,
    /// Default-playback control (volume, mute, device switch), built on
    /// first use - the original's speaker service.
    speaker: Mutex<Option<Box<dyn deckboard_os::Speaker>>>,
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
            speaker: Mutex::new(None),
        }
    }

    pub fn with_discord(mut self, config: Option<DiscordConfig>, settings_path: std::path::PathBuf) -> Self {
        self.discord = Mutex::new(config);
        self.discord_settings_path = Some(settings_path);
        self
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

    /// Native system-info actions (si-cpu, si-ram). The replaced JS
    /// extension's execute() was an empty body - a claimed no-op keeps
    /// tile presses succeeding the same way.
    fn exec_sysinfo(&self, cmd: &Command) -> bool {
        if !deckboard_sysinfo::is_sysinfo_action(&cmd.kind) {
            return false;
        }
        deckboard_sysinfo::execute(&cmd.kind);
        true
    }

    /// Native url-to-call: a fire-and-forget GET, exactly what the JS
    /// package's `fetch(args.urlToCall)` did (response ignored).
    fn exec_callurl(&self, cmd: &Command) -> bool {
        if cmd.kind != "url-to-call" {
            return false;
        }
        let url = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| v.get("urlToCall").and_then(|u| u.as_str()).map(str::to_string));
        match url {
            Some(url) => {
                let agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(10)))
                    .build()
                    .new_agent();
                if let Err(e) = agent.get(&url).call() {
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
        f: impl FnOnce(&mut (dyn deckboard_os::Speaker + 'static)) -> R,
    ) -> Option<R> {
        let mut slot = self.speaker.lock().unwrap();
        if slot.is_none() {
            *slot = Some(Box::new(deckboard_os::platform_speaker()));
        }
        slot.as_deref_mut().map(f)
    }

    /// `speaker-device` command: `{"speaker": "<endpoint id>"}` switches
    /// the default output, like the original `setActiveOutputDevice`.
    /// Returns true when the kind belongs to the speaker service.
    fn exec_speaker(&self, cmd: &Command, sink: &mut dyn EventSink) -> bool {
        if cmd.kind != "speaker-device" {
            return false;
        }
        let id = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
            .and_then(|v| v.get("speaker").and_then(|s| s.as_str()).map(str::to_string));
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
    fn exec_speaker_volume(&self, value: f64) -> bool {
        if !self.speaker_claims("speaker-volume") {
            return false;
        }
        self.with_speaker(|sp| {
            if let Err(e) = sp.set_volume(value as f32 * 100.0) {
                tracing::warn!(error = %e, "speaker-volume set failed");
            }
        });
        true
    }

    fn speaker_claims(&self, kind: &str) -> bool {
        matches!(kind, "speaker-device" | "speaker-volume")
    }

    /// Watcher snapshots: master volume in percent / muted flag / default
    /// device endpoint id. None where the platform has no support.
    pub fn speaker_status(&self) -> (Option<f32>, Option<bool>) {
        self.with_speaker(|sp| (sp.volume().ok(), sp.muted().ok()))
            .unwrap_or((None, None))
    }

    pub fn speaker_device_id(&self) -> Option<String> {
        self.with_speaker(|sp| sp.active_device().ok()).flatten()
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

    /// Run one Discord action over the local RPC pipe. On an expired token
    /// a silent refresh is tried first; without a refresh token Discord's
    /// consent popup shows on the desktop and the new tokens are saved to
    /// settings.json. Returns true when the action kind belongs to Discord.
    fn exec_discord(
        &self,
        cmd: &Command,
        sink: &mut dyn EventSink,
    ) -> bool {
        if !deckboard_discord::is_discord_action(&cmd.kind) {
            return false;
        }
        let Some(config) = self.discord.lock().unwrap().clone() else {
            tracing::warn!(kind = %cmd.kind, "discord not configured (no client id in settings)");
            return true;
        };
        let args = cmd
            .command
            .as_deref()
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or(serde_json::Value::Null);
        let result = deckboard_discord::execute(&config, &cmd.kind, &args, |key, value| {
            sink.app_value(&key, &value);
        });
        let result = match result {
            Err(deckboard_discord::DiscordError::AuthRejected) => {
                match self.reauthorize_discord(&config) {
                    Ok(fresh) => {
                        tracing::info!("discord re-authorized, retrying action");
                        deckboard_discord::execute(&fresh, &cmd.kind, &args, |key, value| {
                            sink.app_value(&key, &value);
                        })
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
    ) -> Result<DiscordConfig, deckboard_discord::DiscordError> {
        let tokens = deckboard_discord::refresh(config).or_else(|_| {
            tracing::info!("discord token refresh unavailable - showing consent popup (confirm it on the desktop)");
            deckboard_discord::authorize(config, std::time::Instant::now() + std::time::Duration::from_secs(180))
        })?;
        let fresh = DiscordConfig {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            ..config.clone()
        };
        if let Some(path) = &self.discord_settings_path {
            if let Err(e) = deckboard_discord::save_tokens(path, &deckboard_discord::AuthTokens {
                access_token: fresh.access_token.clone(),
                refresh_token: fresh.refresh_token.clone(),
            }) {
                tracing::warn!(error = %e, "could not save discord tokens to settings.json");
            }
        }
        *self.discord.lock().unwrap() = Some(fresh.clone());
        Ok(fresh)
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
    fn speaker_status(&self) -> (Option<f32>, Option<bool>) {
        SqlBackend::speaker_status(self)
    }

    fn speaker_device_id(&self) -> Option<String> {
        SqlBackend::speaker_device_id(self)
    }

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
        if self.exec_extension(&cmd, None)
            || self.exec_sysinfo(&cmd)
            || self.exec_callurl(&cmd)
            || self.exec_voicemeeter(&cmd)
            || self.exec_discord(&cmd, sink)
            || self.exec_speaker(&cmd, sink)
        {
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
        if self.exec_extension(&cmd, Some(value))
            || self.exec_sysinfo(&cmd)
            || self.exec_callurl(&cmd)
            || self.exec_voicemeeter(&cmd)
            || self.exec_speaker_volume(value)
        {
            return;
        }
        self.with_input(|input| {
            let _ = deckboard_actions::run_slider_command(input, &cmd, value);
        });
    }
}
