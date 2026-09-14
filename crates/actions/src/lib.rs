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
    OpenUrl(String),
    Spawn(String, Vec<String>),
    Sleep(u64),
    ChangeBoard(i64),
    Unsupported(String),
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
}

/// Events pushed back to connected clients (e.g. multiaction board switch).
pub trait EventSink {
    fn change_board(&mut self, board_id: i64);
    /// Push a custom-value label (e.g. `toggle-microphone` -> "OFF") to
    /// clients as APP_CUSTOM_VALUE. Default no-op: not every action pushes.
    fn app_value(&mut self, _key: &str, _value: &str) {}
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

/// Dispatch one button press. Mirrors the original `runCommand`:
/// with `is_tap_start = true` only `key` runs (keys held down);
/// `false` executes the action and releases held keys.
pub fn run_command(
    input: &mut dyn Input,
    sink: &mut dyn EventSink,
    cmd: &Command,
    is_tap_start: bool,
) -> Result<()> {
    // Sliders are driven by exec_slider, not exec_shortcut.
    if cmd.mode == "slider" {
        return run_slider_command(input, cmd, 0.0);
    }
    match cmd.kind.as_str() {
        "board" => Ok(()), // board switching is client/editor-local
        "key" => run_key(input, cmd, is_tap_start),
        k if k.starts_with("spotify")
            || k.starts_with("slobs")
            || k.starts_with("obs")
            || k.starts_with("xsplit")
            || k.contains("twitch")
            || k.starts_with("vmod")
            || k == "speaker-device"
            || k == "speaker-volume"
            || k == "play"
            || k == "screenshot" =>
        {
            // M0 covers the system-level subset; integrations arrive in M7.
            tracing::warn!(kind = k, "command type not implemented yet");
            Ok(())
        }
        "multiaction" => run_multiaction(input, sink, cmd),
        "advance-key" => run_advance_key(input, cmd),
        "type" => {
            let text = cmd.command.as_deref().unwrap_or_default();
            input.text(text)
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
        "speaker-volume" | "wheels-volume" | "slider-obs-audio" | "slider-slobs-audio"
        | "obs-audio-slider" | "slobs-audio-slider" => {
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

fn run_multiaction(input: &mut dyn Input, sink: &mut dyn EventSink, cmd: &Command) -> Result<()> {
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
                input.sleep(ms)?;
            }
            "board" => {
                let id = step_cmd
                    .command
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .and_then(|v| v.get("id").cloned())
                    .and_then(|v| v.as_i64())
                    .or_else(|| step_cmd.command.as_deref().and_then(|s| s.parse().ok()));
                if let Some(id) = id {
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
            _ => run_command(input, sink, &step_cmd, false)?,
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
                input.sleep(ms)?;
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

fn run_open_file(input: &mut dyn Input, cmd: &Command) -> Result<()> {
    let path = cmd.command.as_deref().unwrap_or_default();
    if path.is_empty() {
        return Ok(());
    }
    match &cmd.options {
        Some(opts) if !opts.trim().is_empty() => {
            let args: Vec<String> = opts.split_whitespace().map(str::to_string).collect();
            input.spawn(path, &args)
        }
        _ => input.open_url(path),
    }
}

/// Parse `"CTRL + SHIFT + K"` into modifier+key lists (original hotkey format).
pub fn parse_hotkey(raw: &str) -> Vec<KeyName> {
    raw.split('+').filter_map(parse_key_name).collect()
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

    fn enigo_key(name: &KeyName) -> enigo::Key {
        use enigo::Key;
        match name {
            KeyName::Control => Key::Control,
            KeyName::Meta => Key::Meta,
            KeyName::Alt => Key::Alt,
            KeyName::Shift => Key::Shift,
            KeyName::Return => Key::Return,
            KeyName::Tab => Key::Tab,
            KeyName::Escape => Key::Escape,
            KeyName::Space => Key::Space,
            KeyName::Backspace => Key::Backspace,
            KeyName::Delete => Key::Delete,
            KeyName::Insert => Key::Insert,
            KeyName::Home => Key::Home,
            KeyName::End => Key::End,
            KeyName::PageUp => Key::PageUp,
            KeyName::PageDown => Key::PageDown,
            KeyName::Up => Key::UpArrow,
            KeyName::Down => Key::DownArrow,
            KeyName::Left => Key::LeftArrow,
            KeyName::Right => Key::RightArrow,
            KeyName::Function(n) => match n {
                1 => Key::F1,
                2 => Key::F2,
                3 => Key::F3,
                4 => Key::F4,
                5 => Key::F5,
                6 => Key::F6,
                7 => Key::F7,
                8 => Key::F8,
                9 => Key::F9,
                10 => Key::F10,
                11 => Key::F11,
                12 => Key::F12,
                _ => Key::F12,
            },
            KeyName::Char(c) => Key::Unicode(*c),
        }
    }
}

impl Input for EnigoInput {
    fn key_down(&mut self, keys: &[KeyName]) -> Result<()> {
        for k in keys {
            self.enigo
                .key(Self::enigo_key(k), enigo::Direction::Press)
                .map_err(|e| ActionError::Input(e.to_string()))?;
        }
        Ok(())
    }

    fn key_up(&mut self, keys: &[KeyName]) -> Result<()> {
        for k in keys {
            self.enigo
                .key(Self::enigo_key(k), enigo::Direction::Release)
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

    impl MockInput {
        pub fn pressed(&self) -> Vec<&Effect> {
            self.effects.iter().by_ref().collect()
        }
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
    fn board_type_is_noop_and_sliders_routed() {
        let mut input = MockInput::default();
        let mut sink = MockSink::default();
        run_command(&mut input, &mut sink, &cmd("board", Some("3")), false).unwrap();
        assert!(input.effects.is_empty());
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
        run_command(
            &mut input,
            &mut sink,
            &cmd("spotify-playback", Some("play")),
            false,
        )
        .unwrap();
    }
}
