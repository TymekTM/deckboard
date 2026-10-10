//! Command dispatch for macro buttons (M0 subset).
//!
//! The dispatch logic is platform-independent and fully testable: all OS
//! side effects go through the [`Input`] trait. `EnigoInput` is the real
//! backend; tests inject a recording mock.

use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

use enigo::{Keyboard as _, Mouse as _};

#[derive(Error, Debug)]
pub enum ActionError {
    #[error("unsupported command type: {0}")]
    Unsupported(String),
    #[error("bad command payload for {0}: {1}")]
    BadPayload(String, String),
    #[error("input backend error: {0}")]
    Input(String),
}

pub type Result<T> = std::result::Result<T, ActionError>;

/// Key names accepted in `key` commands (robotjs-style, case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyName {
    Control,
    Meta,
    Alt,
    Shift,
    Return,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Function(u8),
    Char(char),
}

pub fn parse_key_name(raw: &str) -> Option<KeyName> {
    let k = raw.trim().to_ascii_lowercase();
    Some(match k.as_str() {
        "control" | "ctrl" | "right_control" | "left_control" => KeyName::Control,
        "command" | "cmd" | "meta" | "win" | "left_meta" | "right_meta" => KeyName::Meta,
        "alt" | "option" | "left_alt" | "right_alt" => KeyName::Alt,
        "shift" | "right_shift" | "left_shift" => KeyName::Shift,
        "enter" | "return" => KeyName::Return,
        "tab" => KeyName::Tab,
        "escape" | "esc" => KeyName::Escape,
        "space" => KeyName::Space,
        "backspace" => KeyName::Backspace,
        "delete" | "del" => KeyName::Delete,
        "insert" => KeyName::Insert,
        "home" => KeyName::Home,
        "end" => KeyName::End,
        "pageup" => KeyName::PageUp,
        "pagedown" => KeyName::PageDown,
        "up" => KeyName::Up,
        "down" => KeyName::Down,
        "left" => KeyName::Left,
        "right" => KeyName::Right,
        // "+" cannot appear in a hotkey string (it is the separator),
        // so the plus key gets its own name
        "plus" => KeyName::Char('+'),
        other => {
            if let Some(rest) = other.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=24).contains(&n) {
                        return Some(KeyName::Function(n));
                    }
                }
            }
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyName::Char(c)
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    Mute,
}

/// Recorded OS effect, used by tests to assert dispatch behavior.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    KeyDown(Vec<KeyName>),
    KeyUp(Vec<KeyName>),
    KeyTap(Vec<KeyName>),
    Text(String),
    MouseMove(i32, i32),
    MouseClick(bool), // true = left
    Media(MediaKey),
    Screenshot(String),
    OpenUrl(String),
    Spawn(String, Vec<String>),
    Sleep(u64),
}

/// OS interaction seam. Implemented by `EnigoInput` (real) and test mocks.
pub trait Input {
    fn key_down(&mut self, keys: &[KeyName]) -> Result<()>;
    fn key_up(&mut self, keys: &[KeyName]) -> Result<()>;
    fn key_tap(&mut self, keys: &[KeyName]) -> Result<()>;
    fn text(&mut self, text: &str) -> Result<()>;
    fn mouse_move(&mut self, x: i32, y: i32) -> Result<()>;
    fn mouse_click(&mut self, left: bool) -> Result<()>;
    fn media(&mut self, key: MediaKey) -> Result<()>;
    fn open_url(&mut self, url: &str) -> Result<()>;
    fn spawn(&mut self, path: &str, args: &[String]) -> Result<()>;
    fn sleep(&mut self, ms: u64) -> Result<()>;
    /// Paste text through the clipboard (the original `typeString`):
    /// unicode reaches any focused app and the clipboard comes back.
    /// Default falls back to plain key synthesis.
    fn paste_text(&mut self, text: &str) -> Result<()> {
        self.text(text)
    }
    /// Capture the primary screen as PNG into `dir` (empty: the pictures
    /// folder), the original `screenshot` command.
    fn screenshot(&mut self, _dir: &str) -> Result<()> {
        Err(ActionError::Unsupported("screenshot".into()))
    }
}

/// Events pushed back to connected clients (board tiles and multiaction
/// board steps).
pub trait EventSink {
    fn change_board(&mut self, board_id: i64);
    /// Push a custom-value label (e.g. `toggle-microphone` -> "OFF") to
    /// clients as APP_CUSTOM_VALUE. Default no-op: not every action pushes.
    fn app_value(&mut self, _key: &str, _value: &str) {}
    /// Get a custom-value label if known by this sink. Default None.
    fn get_app_value(&self, _key: &str) -> Option<String> {
        None
    }
    /// Push a third-party app state (e.g. `speaker-device` -> endpoint id)
    /// to clients as THIRD_PARTY_APP. Default no-op.
    fn third_party_value(&mut self, _key: &str, _value: &str) {}
}

/// No-op sink for tests that don't care about broadcasts.
pub struct NullSink;
impl EventSink for NullSink {
    fn change_board(&mut self, _board_id: i64) {}
}

/// A shortcut row, decoupled from the db crate.
#[derive(Debug, Clone, Default)]
pub struct Command {
    pub kind: String,
    pub command: Option<String>,
    pub options: Option<String>,
    pub mode: String,
}

impl Command {
    pub fn from_row(kind: &str, command: Option<&str>, options: Option<&str>, mode: &str) -> Self {
        Command {
            kind: kind.to_string(),
            command: command.map(str::to_string),
            options: options.map(str::to_string),
            mode: mode.to_string(),
        }
    }
}

/// Command types the builtin dispatcher fully implements. Hosts that also
/// run JS extensions must dispatch these BEFORE asking extensions, so a
/// package declaring `key`, `url` or `type` cannot hijack the builtin
/// kinds (the kinds below always claim their command; the extension is
/// never consulted). Integration kinds that the builtin dispatcher only
/// warns about (`obs-*`, `play`, ...) are deliberately absent: extensions
/// own those.
pub fn is_builtin_kind(kind: &str) -> bool {
    matches!(
        kind,
        "board"
            | "key"
            | "multiaction"
            | "advance-key"
            | "type"
            | "screenshot"
            | "mouse-ctrl"
            | "vol"
            | "url"
            | "dir"
            | "app"
            | "file"
    )
}

/// Dispatch one button press. Mirrors the original `runCommand`:
/// with `is_tap_start = true` only `key` runs (keys held down);
/// `false` executes the action and releases held keys.
pub fn run_command(
    input: &mut dyn Input,
    sink: &mut dyn EventSink,
    cmd: &Command,
    is_tap_start: bool,
) -> Result<()> {
    let mut steps = BuiltinSteps;
    run_command_dispatched(input, sink, cmd, is_tap_start, &mut steps)
}

/// Multiaction step dispatcher. Built-in commands go through
/// [`run_command`] directly; a host with native/extension dispatchers
/// (see `crates/backend`) implements this trait so every multiaction
/// step runs through the same chain as a top-level tile press instead
/// of only the built-in kinds.
pub trait StepDispatch {
    fn dispatch_step(
        &mut self,
        input: &mut dyn Input,
        sink: &mut dyn EventSink,
        cmd: &Command,
    ) -> Result<()>;
}

/// Default dispatcher: built-ins only, the historical multiaction
/// behavior for callers without a native chain.
struct BuiltinSteps;
impl StepDispatch for BuiltinSteps {
    fn dispatch_step(
        &mut self,
        input: &mut dyn Input,
        sink: &mut dyn EventSink,
        cmd: &Command,
    ) -> Result<()> {
        run_command(input, sink, cmd, false)
    }
}

/// Like [`run_command`], but multiaction steps are dispatched through
/// `dispatch` instead of the built-in-only fallback.
pub fn run_command_dispatched(
    input: &mut dyn Input,
    sink: &mut dyn EventSink,
    cmd: &Command,
    is_tap_start: bool,
    dispatch: &mut dyn StepDispatch,
) -> Result<()> {
    // Sliders are driven by exec_slider, not exec_shortcut.
    if cmd.mode == "slider" {
        return run_slider_command(input, cmd, 0.0);
    }
    // Touch-down (`is_tap_start = true`) only drives held `key` buttons;
    // every other kind fires once on release. The client sends
    // exec_shortcut on BOTH phases, so without this filter urls would
    // open twice and multiactions would run twice.
    if is_tap_start && cmd.kind != "key" {
        return Ok(());
    }
    match cmd.kind.as_str() {
        // Clients are remote viewers: the switch goes out through the
        // sink as a change-board event (v2 broadcasts board.open), the
        // same path a multiaction board step takes.
        "board" => match parse_board_id(cmd.command.as_deref()) {
            Some(id) => {
                sink.change_board(id);
                Ok(())
            }
            None => {
                tracing::warn!("board switch without a parsable target id - ignored");
                Ok(())
            }
        },
        "key" => run_key(input, cmd, is_tap_start),
        k if k.starts_with("slobs")
            || k.starts_with("obs")
            || k.starts_with("xsplit")
            || k.contains("twitch")
            || k.starts_with("vmod")
            || k == "play" =>
        {
            // M0 covers the system-level subset; the remaining
            // integrations arrive in M7. Spotify kinds are NOT here:
            // the backend's native chain (crates/spotify) claims them
            // before this dispatcher, and a bare run_command call
            // without a backend lands in the unknown arm below.
            tracing::warn!(kind = k, "command type not implemented yet");
            Ok(())
        }
        "multiaction" => run_multiaction(input, sink, cmd, dispatch),
        "advance-key" => run_advance_key(input, cmd),
        "type" => {
            let text = cmd.command.as_deref().unwrap_or_default();
            input.paste_text(text)
        }
        "screenshot" => {
            let dir = cmd.command.as_deref().unwrap_or_default();
            input.screenshot(dir)
        }
        "mouse-ctrl" => run_mouse(input, cmd),
        "vol" => run_vol(input, cmd),
        "url" | "dir" => {
            let target = cmd.command.as_deref().unwrap_or_default();
            if target.is_empty() {
                return Ok(());
            }
            input.open_url(target)
        }
        "app" | "file" => run_open_file(input, cmd),
        other => {
            // Unknown types belong to extensions (M7); log and ignore.
            tracing::warn!(kind = other, "unknown command type (extension?) - ignored");
            Ok(())
        }
    }
}

/// Slider value change (`exec_slider {id, value}`), value in 0..1.
pub fn run_slider_command(_input: &mut dyn Input, cmd: &Command, _value: f64) -> Result<()> {
    match cmd.kind.as_str() {
        "wheels-volume" | "slider-obs-audio" | "slider-slobs-audio" | "obs-audio-slider"
        | "slobs-audio-slider" => {
            tracing::warn!(
                kind = cmd.kind.as_str(),
                "slider backends not implemented yet"
            );
            Ok(())
        }
        other => {
            tracing::warn!(kind = other, "unknown slider type - ignored");
            Ok(())
        }
    }
}

fn run_key(input: &mut dyn Input, cmd: &Command, is_tap_start: bool) -> Result<()> {
    let keys = parse_hotkey(cmd.command.as_deref().unwrap_or_default());
    if keys.is_empty() {
        return Ok(());
    }
    if is_tap_start {
        input.key_down(&keys)
    } else {
        input.key_up(&keys)
    }
}

/// Board switch targets arrive as the editor JSON `{"id":7}` or a bare
/// `7`.
fn parse_board_id(command: Option<&str>) -> Option<i64> {
    command
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("id").cloned())
        .and_then(|v| v.as_i64())
        .or_else(|| command.and_then(|s| s.trim().parse().ok()))
}

/// Upper bound for `delay` steps (multiaction, advance-key): the input
/// mutex is held while a macro sleeps, so one junk value must not stall
/// every later action for minutes.
const MAX_DELAY_MS: u64 = 60_000;

fn capped_delay(ms: u64) -> u64 {
    if ms > MAX_DELAY_MS {
        tracing::warn!(requested = ms, cap = MAX_DELAY_MS, "delay step capped");
        MAX_DELAY_MS
    } else {
        ms
    }
}

fn run_multiaction(
    input: &mut dyn Input,
    sink: &mut dyn EventSink,
    cmd: &Command,
    dispatch: &mut dyn StepDispatch,
) -> Result<()> {
    let raw = cmd.command.as_deref().unwrap_or("[]");
    let steps: Vec<Value> = serde_json::from_str(raw)
        .map_err(|e| ActionError::BadPayload("multiaction".into(), e.to_string()))?;
    for step in steps {
        let step_cmd = Command {
            kind: step
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            command: step
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string),
            options: step
                .get("options")
                .and_then(Value::as_str)
                .map(str::to_string),
            mode: "button".to_string(),
        };
        match step_cmd.kind.as_str() {
            "delay" => {
                let ms: u64 = step_cmd
                    .command
                    .as_deref()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                input.sleep(capped_delay(ms))?;
            }
            "board" => {
                if let Some(id) = parse_board_id(step_cmd.command.as_deref()) {
                    sink.change_board(id);
                }
            }
            "key" => {
                // Full tap inside multiaction regardless of tap state.
                let keys = parse_hotkey(step_cmd.command.as_deref().unwrap_or_default());
                if !keys.is_empty() {
                    input.key_down(&keys)?;
                    input.sleep(150)?;
                    input.key_up(&keys)?;
                }
            }
            _ => dispatch.dispatch_step(input, sink, &step_cmd)?,
        }
    }
    Ok(())
}

fn run_advance_key(input: &mut dyn Input, cmd: &Command) -> Result<()> {
    let raw = cmd.command.as_deref().unwrap_or("[]");
    let steps: Vec<Value> = serde_json::from_str(raw)
        .map_err(|e| ActionError::BadPayload("advance-key".into(), e.to_string()))?;
    for step in steps {
        let action = step
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let value = step.get("value").cloned().unwrap_or(Value::Null);
        match action {
            "down" | "up" => {
                let key = parse_key_name(value.as_str().unwrap_or_default()).ok_or_else(|| {
                    ActionError::BadPayload("advance-key".into(), "bad key".into())
                })?;
                if action == "down" {
                    input.key_down(&[key])?;
                } else {
                    input.key_up(&[key])?;
                }
            }
            "delay" => {
                let ms = value.as_u64().unwrap_or(0);
                input.sleep(capped_delay(ms))?;
            }
            "type" => {
                input.text(value.as_str().unwrap_or_default())?;
            }
            other => tracing::warn!(action = other, "unknown advance-key step"),
        }
    }
    Ok(())
}

fn run_mouse(input: &mut dyn Input, cmd: &Command) -> Result<()> {
    let raw = cmd.command.as_deref().unwrap_or("{}");
    let v: Value = serde_json::from_str(raw)
        .map_err(|e| ActionError::BadPayload("mouse-ctrl".into(), e.to_string()))?;
    match v.get("action").and_then(Value::as_str) {
        Some("move") => {
            let x = v.get("x").and_then(Value::as_i64).unwrap_or(0) as i32;
            let y = v.get("y").and_then(Value::as_i64).unwrap_or(0) as i32;
            input.mouse_move(x, y)
        }
        Some("lclick") => input.mouse_click(true),
        Some("rclick") => input.mouse_click(false),
        other => Err(ActionError::BadPayload(
            "mouse-ctrl".into(),
            format!("unknown action {other:?}"),
        )),
    }
}

fn run_vol(input: &mut dyn Input, cmd: &Command) -> Result<()> {
    let media = match cmd.command.as_deref().unwrap_or_default() {
        "play" => MediaKey::PlayPause,
        "next" => MediaKey::NextTrack,
        "prev" => MediaKey::PrevTrack,
        "vol_up" => MediaKey::VolumeUp,
        "vol_down" => MediaKey::VolumeDown,
        "vol_mute" => MediaKey::Mute,
        other => {
            return Err(ActionError::BadPayload(
                "vol".into(),
                format!("unknown control {other:?}"),
            ))
        }
    };
    input.media(media)
}

/// Split an `open` options string into argv tokens: whitespace
/// separated, with double-quoted runs kept together as one token
/// (quotes stripped). No escape handling - the editor offers no way to
/// type a literal quote.
fn split_args(raw: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn run_open_file(input: &mut dyn Input, cmd: &Command) -> Result<()> {
    let path = cmd.command.as_deref().unwrap_or_default();
    if path.is_empty() {
        return Ok(());
    }
    match &cmd.options {
        Some(opts) if !opts.trim().is_empty() => {
            let args = split_args(opts);
            input.spawn(path, &args)
        }
        _ => input.open_url(path),
    }
}

/// Parse `"CTRL + SHIFT + K"` into modifier+key lists (original hotkey format).
pub fn parse_hotkey(raw: &str) -> Vec<KeyName> {
    if raw.trim() == "+" {
        return vec![KeyName::Char('+')];
    }
    raw.split('+')
        .filter_map(|part| {
            let name = parse_key_name(part);
            if name.is_none() {
                tracing::warn!(part = part.trim(), "unknown key name in hotkey - skipped");
            }
            name
        })
        .collect()
}

/// Real OS backend.
pub struct EnigoInput {
    enigo: enigo::Enigo,
}

impl EnigoInput {
    pub fn new() -> Result<Self> {
        let enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ActionError::Input(e.to_string()))?;
        Ok(EnigoInput { enigo })
    }

    fn enigo_key(name: &KeyName) -> Result<enigo::Key> {
        use enigo::Key;
        match name {
            KeyName::Control => Ok(Key::Control),
            KeyName::Meta => Ok(Key::Meta),
            KeyName::Alt => Ok(Key::Alt),
            KeyName::Shift => Ok(Key::Shift),
            KeyName::Return => Ok(Key::Return),
            KeyName::Tab => Ok(Key::Tab),
            KeyName::Escape => Ok(Key::Escape),
            KeyName::Space => Ok(Key::Space),
            KeyName::Backspace => Ok(Key::Backspace),
            KeyName::Delete => Ok(Key::Delete),
            KeyName::Insert => Ok(Key::Insert),
            KeyName::Home => Ok(Key::Home),
            KeyName::End => Ok(Key::End),
            KeyName::PageUp => Ok(Key::PageUp),
            KeyName::PageDown => Ok(Key::PageDown),
            KeyName::Up => Ok(Key::UpArrow),
            KeyName::Down => Ok(Key::DownArrow),
            KeyName::Left => Ok(Key::LeftArrow),
            KeyName::Right => Ok(Key::RightArrow),
            KeyName::Function(n) => match n {
                1 => Ok(Key::F1),
                2 => Ok(Key::F2),
                3 => Ok(Key::F3),
                4 => Ok(Key::F4),
                5 => Ok(Key::F5),
                6 => Ok(Key::F6),
                7 => Ok(Key::F7),
                8 => Ok(Key::F8),
                9 => Ok(Key::F9),
                10 => Ok(Key::F10),
                11 => Ok(Key::F11),
                12 => Ok(Key::F12),
                13 => Ok(Key::F13),
                14 => Ok(Key::F14),
                15 => Ok(Key::F15),
                16 => Ok(Key::F16),
                17 => Ok(Key::F17),
                18 => Ok(Key::F18),
                19 => Ok(Key::F19),
                20 => Ok(Key::F20),
                21 => Ok(Key::F21),
                22 => Ok(Key::F22),
                23 => Ok(Key::F23),
                24 => Ok(Key::F24),
                // parse_key_name only accepts F1-F24; anything else is a
                // programmatic KeyName misuse and must not inject a wrong
                // key (F13-F24 used to fall back to F12 here).
                other => Err(ActionError::BadPayload(
                    "key".into(),
                    format!("F{other} is outside the supported F1-F24 range"),
                )),
            },
            KeyName::Char(c) => Ok(Key::Unicode(*c)),
        }
    }
}

impl Input for EnigoInput {
    fn key_down(&mut self, keys: &[KeyName]) -> Result<()> {
        for k in keys {
            let key = Self::enigo_key(k)?;
            self.enigo
                .key(key, enigo::Direction::Press)
                .map_err(|e| ActionError::Input(e.to_string()))?;
        }
        Ok(())
    }

    fn key_up(&mut self, keys: &[KeyName]) -> Result<()> {
        for k in keys {
            let key = Self::enigo_key(k)?;
            self.enigo
                .key(key, enigo::Direction::Release)
                .map_err(|e| ActionError::Input(e.to_string()))?;
        }
        Ok(())
    }

    fn key_tap(&mut self, keys: &[KeyName]) -> Result<()> {
        self.key_down(keys)?;
        self.sleep(50)?;
        self.key_up(keys)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.enigo
            .text(text)
            .map_err(|e| ActionError::Input(e.to_string()))
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
        self.enigo
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(|e| ActionError::Input(e.to_string()))
    }

    fn mouse_click(&mut self, left: bool) -> Result<()> {
        let button = if left {
            enigo::Button::Left
        } else {
            enigo::Button::Right
        };
        self.enigo
            .button(button, enigo::Direction::Click)
            .map_err(|e| ActionError::Input(e.to_string()))
    }

    fn media(&mut self, key: MediaKey) -> Result<()> {
        use enigo::Key as K;
        let k = match key {
            MediaKey::PlayPause => K::MediaPlayPause,
            MediaKey::NextTrack => K::MediaNextTrack,
            MediaKey::PrevTrack => K::MediaPrevTrack,
            MediaKey::VolumeUp => K::VolumeUp,
            MediaKey::VolumeDown => K::VolumeDown,
            MediaKey::Mute => K::VolumeMute,
        };
        self.enigo
            .key(k, enigo::Direction::Click)
            .map_err(|e| ActionError::Input(e.to_string()))
    }

    /// Clipboard paste: the system-level `type` command must deliver
    /// unicode (enigo's key synthesis cannot), so mirror the original
    /// `typeString`: save clipboard, write text, Ctrl+V, restore.
    #[cfg(windows)]
    fn paste_text(&mut self, text: &str) -> Result<()> {
        // Non-text clipboard content (images) cannot be restored by this
        // text-only path - the original app behaves the same; restore is
        // text-only and best-effort.
        let previous = pulpit_os::clipboard::get_text()
            .ok()
            .filter(|s| !s.is_empty());
        pulpit_os::clipboard::set_text(text).map_err(|e| ActionError::Input(e.to_string()))?;
        let seq_after_set = pulpit_os::clipboard::sequence_number();
        let pasted = self.key_tap(&[KeyName::Control, KeyName::Char('v')]);
        if let Some(previous) = previous {
            // give the focused app a beat to read the paste before the
            // user's clipboard content comes back
            self.sleep(80)?;
            // Restore only if the clipboard still holds OUR paste: when
            // the user (or any app) copied something meanwhile, that
            // content wins and must not be clobbered by the restore.
            if pulpit_os::clipboard::sequence_number() == seq_after_set {
                let _ = pulpit_os::clipboard::set_text(&previous);
            }
        }
        pasted
    }

    #[cfg(windows)]
    fn screenshot(&mut self, dir: &str) -> Result<()> {
        pulpit_os::capture::screenshot_to_dir(dir)
            .map(|_| ())
            .map_err(|e| ActionError::Input(e.to_string()))
    }

    fn open_url(&mut self, url: &str) -> Result<()> {
        open::that(url).map_err(|e| ActionError::Input(e.to_string()))
    }

    fn spawn(&mut self, path: &str, args: &[String]) -> Result<()> {
        std::process::Command::new(path)
            .args(args)
            .spawn()
            .map_err(|e| ActionError::Input(e.to_string()))?;
        Ok(())
    }

    fn sleep(&mut self, ms: u64) -> Result<()> {
        std::thread::sleep(Duration::from_millis(ms));
        Ok(())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// Records every effect instead of touching the OS.
    #[derive(Default)]
    pub struct MockInput {
        pub effects: Vec<Effect>,
    }

    impl Input for MockInput {
        fn key_down(&mut self, keys: &[KeyName]) -> Result<()> {
            self.effects.push(Effect::KeyDown(keys.to_vec()));
            Ok(())
        }
        fn key_up(&mut self, keys: &[KeyName]) -> Result<()> {
            self.effects.push(Effect::KeyUp(keys.to_vec()));
            Ok(())
        }
        fn key_tap(&mut self, keys: &[KeyName]) -> Result<()> {
            self.effects.push(Effect::KeyTap(keys.to_vec()));
            Ok(())
        }
        fn text(&mut self, text: &str) -> Result<()> {
            self.effects.push(Effect::Text(text.into()));
            Ok(())
        }
        fn paste_text(&mut self, text: &str) -> Result<()> {
            self.effects.push(Effect::Text(text.into()));
            Ok(())
        }
        fn screenshot(&mut self, dir: &str) -> Result<()> {
            self.effects.push(Effect::Screenshot(dir.into()));
            Ok(())
        }
        fn mouse_move(&mut self, x: i32, y: i32) -> Result<()> {
            self.effects.push(Effect::MouseMove(x, y));
            Ok(())
        }
        fn mouse_click(&mut self, left: bool) -> Result<()> {
            self.effects.push(Effect::MouseClick(left));
            Ok(())
        }
        fn media(&mut self, key: MediaKey) -> Result<()> {
            self.effects.push(Effect::Media(key));
            Ok(())
        }
        fn open_url(&mut self, url: &str) -> Result<()> {
            self.effects.push(Effect::OpenUrl(url.into()));
            Ok(())
        }
        fn spawn(&mut self, path: &str, args: &[String]) -> Result<()> {
            self.effects.push(Effect::Spawn(path.into(), args.to_vec()));
            Ok(())
        }
        fn sleep(&mut self, ms: u64) -> Result<()> {
            self.effects.push(Effect::Sleep(ms));
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct MockSink {
        pub boards: Vec<i64>,
    }

    impl EventSink for MockSink {
        fn change_board(&mut self, board_id: i64) {
            self.boards.push(board_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockInput, MockSink};
    use KeyName::*;

    fn cmd(kind: &str, command: Option<&str>) -> Command {
        Command::from_row(kind, command, None, "button")
    }

    #[test]
    fn hotkey_parsing() {
        let keys = parse_hotkey("CTRL + SHIFT + K");
        assert_eq!(keys, vec![Control, Shift, Char('k')]);
        assert_eq!(parse_hotkey("f5"), vec![Function(5)]);
        assert_eq!(parse_hotkey("ENTER"), vec![Return]);
    }

    #[test]
    fn builtin_kinds_are_exactly_the_implemented_ones() {
        // every kind with a real match arm in run_command_dispatched must
        // be listed - hosts dispatch these BEFORE extensions so a package
        // declaring the same action cannot hijack them
        for kind in [
            "board",
            "key",
            "multiaction",
            "advance-key",
            "type",
            "screenshot",
            "mouse-ctrl",
            "vol",
            "url",
            "dir",
            "app",
            "file",
        ] {
            assert!(is_builtin_kind(kind), "{kind} must be builtin");
        }
        // integration stubs (the dispatcher only warns) and native
        // backend kinds stay extension/native territory: spotify kinds
        // are claimed by the backend's spotify arm (crates/spotify),
        // not by the builtin dispatcher
        for kind in [
            "obs-start",
            "spotify-playback",
            "spotify-like",
            "play",
            "vmod-mute",
            "twitch-live",
            "url-to-call",
            "si-cpu",
        ] {
            assert!(!is_builtin_kind(kind), "{kind} must not be builtin");
        }
    }

    #[test]
    fn hotkey_parsing_plus_key_and_unknown_names() {
        // "+" is the separator, so the plus key needs its own name
        assert_eq!(parse_hotkey("plus"), vec![Char('+')]);
        assert_eq!(parse_hotkey("+"), vec![Char('+')]);
        assert_eq!(parse_hotkey("CTRL+plus"), vec![Control, Char('+')]);
        // unknown names are skipped (with a warning), not silently
        assert_eq!(parse_hotkey("CTRL+bogus+P"), vec![Control, Char('p')]);
    }

    #[test]
    fn f_keys_one_to_twenty_four_map_to_distinct_keys() {
        let mut seen = std::collections::HashSet::new();
        for n in 1..=24u8 {
            assert!(
                seen.insert(EnigoInput::enigo_key(&Function(n)).unwrap()),
                "F{n} collided with an earlier key"
            );
        }
        assert_eq!(seen.len(), 24);
    }

    #[test]
    fn f_keys_outside_the_range_are_an_error() {
        assert!(EnigoInput::enigo_key(&Function(0)).is_err());
        assert!(EnigoInput::enigo_key(&Function(25)).is_err());
    }

    #[test]
    fn key_press_and_release_semantics() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd("key", Some("CTRL + K"));

        run_command(&mut input, &mut sink, &c, true).unwrap();
        assert_eq!(input.effects[0], Effect::KeyDown(vec![Control, Char('k')]));

        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert_eq!(input.effects[1], Effect::KeyUp(vec![Control, Char('k')]));
    }

    #[test]
    fn tap_start_only_drives_key_buttons() {
        // The client sends exec_shortcut on touch-down AND touch-up; only
        // `key` acts on touch-down (held keys). Everything else must fire
        // exactly once, on release.
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd("url", Some("https://example.com"));
        run_command(&mut input, &mut sink, &c, true).unwrap();
        assert!(
            input.effects.is_empty(),
            "tap-start must not fire non-key commands"
        );
        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert_eq!(
            input.effects,
            vec![Effect::OpenUrl("https://example.com".into())]
        );

        let multi = cmd(
            "multiaction",
            Some(r#"[{"type":"board","command":"{\"id\":7}"}]"#),
        );
        run_command(&mut input, &mut sink, &multi, true).unwrap();
        assert!(
            sink.boards.is_empty(),
            "tap-start must not run multiactions"
        );
    }

    #[test]
    fn multiaction_and_advance_key_delays_are_capped() {
        // the input mutex is held while a macro sleeps; one junk delay
        // must not stall every later action
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd(
            "multiaction",
            Some(r#"[{"type":"delay","command":"999999"}]"#),
        );
        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert_eq!(input.effects, vec![Effect::Sleep(MAX_DELAY_MS)]);

        let adv = cmd(
            "advance-key",
            Some(r#"[{"action":"delay","value":999999}]"#),
        );
        run_command(&mut input, &mut sink, &adv, false).unwrap();
        assert_eq!(input.effects[1], Effect::Sleep(MAX_DELAY_MS));
    }

    #[test]
    fn multiaction_with_delay_board_and_key() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd(
            "multiaction",
            Some(
                r#"[
                {"type":"delay","command":"100"},
                {"type":"board","command":"{\"id\":7}"},
                {"type":"key","command":"ENTER"},
                {"type":"url","command":"https://example.com"}
            ]"#,
            ),
        );
        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert!(input.effects.contains(&Effect::Sleep(100)));
        // multiaction `key` steps perform a full tap: down, 150 ms, up
        assert!(input.effects.contains(&Effect::KeyDown(vec![Return])));
        assert!(input.effects.contains(&Effect::Sleep(150)));
        assert!(input.effects.contains(&Effect::KeyUp(vec![Return])));
        assert!(input
            .effects
            .contains(&Effect::OpenUrl("https://example.com".into())));
        assert_eq!(sink.boards, vec![7]);
    }

    /// Host dispatcher: claims everything it is asked to run.
    #[derive(Default)]
    struct ClaimingDispatch {
        claimed: Vec<String>,
    }
    impl StepDispatch for ClaimingDispatch {
        fn dispatch_step(
            &mut self,
            _input: &mut dyn Input,
            _sink: &mut dyn EventSink,
            cmd: &Command,
        ) -> Result<()> {
            self.claimed.push(cmd.kind.clone());
            Ok(())
        }
    }

    #[test]
    fn multiaction_steps_go_through_the_host_dispatcher() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd(
            "multiaction",
            Some(
                r#"[
                {"type":"key","command":"ENTER"},
                {"type":"board","command":"{\"id\":7}"},
                {"type":"delay","command":"100"},
                {"type":"speaker-device","command":"{}"}
            ]"#,
            ),
        );
        let mut dispatch = ClaimingDispatch::default();
        run_command_dispatched(&mut input, &mut sink, &c, false, &mut dispatch).unwrap();
        // only the non-special step is handed to the host dispatcher
        assert_eq!(dispatch.claimed, vec!["speaker-device".to_string()]);
        // the delay/board/key special cases keep running locally
        assert_eq!(sink.boards, vec![7]);
        assert!(input.effects.contains(&Effect::KeyDown(vec![Return])));
        assert!(input.effects.contains(&Effect::Sleep(100)));
    }

    #[test]
    fn open_options_keep_quoted_arguments_together() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = Command::from_row(
            "app",
            Some("C:\\Program Files\\tool.exe"),
            Some("--msg \"hello world\""),
            "button",
        );
        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert_eq!(
            input.effects,
            vec![Effect::Spawn(
                "C:\\Program Files\\tool.exe".into(),
                vec!["--msg".to_string(), "hello world".to_string()],
            )]
        );
    }

    #[test]
    fn mouse_move_and_click() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        run_command(
            &mut input,
            &mut sink,
            &cmd("mouse-ctrl", Some(r#"{"action":"move","x":10,"y":20}"#)),
            false,
        )
        .unwrap();
        run_command(
            &mut input,
            &mut sink,
            &cmd("mouse-ctrl", Some(r#"{"action":"lclick"}"#)),
            false,
        )
        .unwrap();
        run_command(
            &mut input,
            &mut sink,
            &cmd("mouse-ctrl", Some(r#"{"action":"rclick"}"#)),
            false,
        )
        .unwrap();
        assert_eq!(
            input.effects,
            vec![
                Effect::MouseMove(10, 20),
                Effect::MouseClick(true),
                Effect::MouseClick(false),
            ]
        );
    }

    #[test]
    fn vol_maps_to_media_keys() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        run_command(&mut input, &mut sink, &cmd("vol", Some("vol_mute")), false).unwrap();
        assert_eq!(input.effects, vec![Effect::Media(MediaKey::Mute)]);
    }

    #[test]
    fn type_and_open() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        run_command(&mut input, &mut sink, &cmd("type", Some("hello")), false).unwrap();
        run_command(
            &mut input,
            &mut sink,
            &cmd("url", Some("https://x.co")),
            false,
        )
        .unwrap();
        assert_eq!(input.effects[0], Effect::Text("hello".into()));
        assert_eq!(input.effects[1], Effect::OpenUrl("https://x.co".into()));
    }

    #[test]
    fn advance_key_sequence() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        let c = cmd(
            "advance-key",
            Some(
                r#"[{"action":"down","value":"SHIFT"},{"action":"delay","value":50},{"action":"up","value":"SHIFT"},{"action":"type","value":"hi"}]"#,
            ),
        );
        run_command(&mut input, &mut sink, &c, false).unwrap();
        assert_eq!(
            input.effects,
            vec![
                Effect::KeyDown(vec![Shift]),
                Effect::Sleep(50),
                Effect::KeyUp(vec![Shift]),
                Effect::Text("hi".into()),
            ]
        );
    }

    #[test]
    fn board_type_switches_via_sink_and_sliders_routed() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        // editor JSON form and the bare-id form both resolve
        run_command(
            &mut input,
            &mut sink,
            &cmd("board", Some(r#"{"id":3}"#)),
            false,
        )
        .unwrap();
        run_command(&mut input, &mut sink, &cmd("board", Some("7")), false).unwrap();
        assert_eq!(sink.boards, vec![3, 7]);
        assert!(input.effects.is_empty());
        // unparsable target: no switch, still no crash
        run_command(&mut input, &mut sink, &cmd("board", Some("nope")), false).unwrap();
        assert_eq!(sink.boards, vec![3, 7]);
        let slider = Command::from_row("speaker-volume", None, None, "slider");
        run_slider_command(&mut input, &slider, 0.5).unwrap();
    }

    #[test]
    fn integration_types_are_not_crashing() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        run_command(&mut input, &mut sink, &cmd("obs-scene", Some("Cam")), false).unwrap();
        run_command(
            &mut input,
            &mut sink,
            &cmd("speaker-volume", Some("")),
            false,
        )
        .unwrap();
        // spotify kinds no longer have a stub branch: hosts claim them
        // in the native chain (crates/backend -> crates/spotify) before
        // this dispatcher, so a bare run_command call treats them like
        // any other unknown kind - warn, no effects, no crash
        run_command(
            &mut input,
            &mut sink,
            &cmd("spotify-playback", Some("play")),
            false,
        )
        .unwrap();
        assert!(input.effects.is_empty());
    }
}
