//! System Media Transport Controls (SMTC) integration for Windows.
//!
//! Exposes system-wide "now playing" information and playback control
//! for any active media player (browser YouTube, Tidal, Spotify, VLC,
//! etc.) via Windows Global System Media Transport Controls
//! (Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager).

use crate::Result;
use serde_json::{json, Value};
use std::sync::Arc;

#[cfg(windows)]
mod win_media;

/// Convert an Application User Model ID (AUMID) or process name to a
/// friendly application title (e.g. "Spotify.exe" -> "Spotify",
/// "F0DC299D809B9700" / "ZenToast-..." -> "Zen Browser").
pub fn aumid_to_friendly_name(aumid: &str) -> String {
    let raw = aumid.trim();
    if raw.is_empty() {
        return String::new();
    }

    // Direct lookup for known common media players / browsers
    let lower = raw.to_lowercase();
    if lower == "spotify.exe" || lower == "spotify" {
        return "Spotify".to_string();
    }
    if lower == "chrome.exe" || lower == "google chrome" || lower.starts_with("chrome.") {
        return "Google Chrome".to_string();
    }
    if lower == "msedge.exe"
        || lower == "microsoft edge"
        || lower.starts_with("microsoft.microsoftedge")
    {
        return "Microsoft Edge".to_string();
    }
    if lower == "firefox.exe" || lower == "mozilla firefox" || lower.starts_with("firefox.") {
        return "Firefox".to_string();
    }
    if lower == "brave.exe" || lower == "brave" {
        return "Brave".to_string();
    }
    if lower == "zen.exe"
        || lower == "zen browser"
        || lower == "f0dc299d809b9700"
        || lower.starts_with("zentoast-")
    {
        return "Zen Browser".to_string();
    }
    if lower == "vlc.exe" || lower == "vlc" {
        return "VLC".to_string();
    }
    if lower == "foobar2000.exe" || lower == "foobar2000" {
        return "foobar2000".to_string();
    }
    if lower == "tidal.exe" || lower == "tidal" {
        return "TIDAL".to_string();
    }
    if lower == "applemusic.exe" || lower.starts_with("appleinc.applemusic") {
        return "Apple Music".to_string();
    }
    if lower == "itunes.exe" || lower == "itunes" {
        return "iTunes".to_string();
    }
    if lower == "musicbee.exe" || lower == "musicbee" {
        return "MusicBee".to_string();
    }
    if lower == "aimp.exe" || lower == "aimp" {
        return "AIMP".to_string();
    }
    if lower.starts_with("microsoft.zunemusic") {
        return "Media Player".to_string();
    }
    if lower.starts_with("microsoft.zunevideo") {
        return "Films & TV".to_string();
    }
    if lower.starts_with("spotifyab.spotifymusic") {
        return "Spotify".to_string();
    }

    // Windows registry check for AppUserModelId DisplayName
    #[cfg(windows)]
    {
        if let Some(name) = win_media::lookup_registry_display_name(raw) {
            return name;
        }
    }

    // Packaged UWP format: Publisher.AppName_hash!AppId
    if let Some((pkg, app)) = raw.split_once('!') {
        let app_trimmed = app.trim();
        if !app_trimmed.is_empty() && !app_trimmed.eq_ignore_ascii_case("app") {
            return humanize_identifier(app_trimmed);
        }
        if let Some((name_part, _)) = pkg.split_once('_') {
            if let Some((_, app_name)) = name_part.split_once('.') {
                return humanize_identifier(app_name);
            }
            return humanize_identifier(name_part);
        }
    }

    // Plain executable name: strip .exe and humanize
    if let Some(stem) = raw
        .strip_suffix(".exe")
        .or_else(|| raw.strip_suffix(".EXE"))
    {
        return humanize_identifier(stem);
    }

    humanize_identifier(raw)
}

fn humanize_identifier(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_is_lower = false;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            prev_is_lower = false;
        } else if ch.is_uppercase() {
            if prev_is_lower && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(ch);
            prev_is_lower = false;
        } else {
            out.push(ch);
            prev_is_lower = ch.is_lowercase();
        }
    }
    // Capitalize first letter of each word if all lowercase
    let words: Vec<String> = out
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect();
    words.join(" ")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaProperties {
    pub title: String,
    pub artist: String,
    pub album_title: String,
    pub album_artist: String,
    pub track_number: Option<i32>,
    pub genres: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Closed,
    Opened,
    Changing,
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRepeatMode {
    None,
    Track,
    List,
}

/// Tile inputs this integration serves: (value, icon, color, mode), the
/// same shape `pulpit_spotify::input_declarations` returns. The legacy
/// mapper resolves tile styling from these; the virtual-media-key
/// "Multimedia" kind stays untouched.
pub fn input_declarations() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    vec![
        ("media-now-playing", "music", "#8E44AD", Some("status")),
        ("media-control", "play", "#8E44AD", None),
        ("media-seek", "clock", "#8E44AD", Some("slider")),
    ]
}

/// Does this action kind belong to the system media integration? Exact
/// matches only, like `pulpit_spotify::is_spotify_action`.
pub fn is_media_action(kind: &str) -> bool {
    matches!(kind, "media-now-playing" | "media-control" | "media-seek")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackInfo {
    pub status: PlaybackStatus,
    pub is_playing: bool,
    pub shuffle: Option<bool>,
    pub repeat: Option<AutoRepeatMode>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_skip_next: bool,
    pub can_skip_prev: bool,
    pub can_stop: bool,
    pub can_seek: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineProperties {
    pub position_ms: u64,
    pub duration_ms: u64,
    pub min_seek_ms: u64,
    pub max_seek_ms: u64,
}

/// The internal host-consumed keys carrying thumbnail bytes and file
/// extension. Internal: stripped before any client forward, never rendered.
pub const ART_DATA_KEY: &str = "media-art-data";
pub const ART_EXT_KEY: &str = "media-art-ext";

/// Build a snapshot payload matching the design §4 contract and
/// spotify-now-playing shape so desktop and mobile renderers are shared.
pub fn build_snapshot(
    props: Option<&MediaProperties>,
    playback: Option<&PlaybackInfo>,
    timeline: Option<&TimelineProperties>,
    app_name: &str,
    art: Option<(&[u8], &'static str)>,
) -> Value {
    let is_playing = playback.map(|p| p.is_playing).unwrap_or(false);
    let shuffle = playback.and_then(|p| p.shuffle).unwrap_or(false);
    let repeat = match playback.and_then(|p| p.repeat) {
        Some(AutoRepeatMode::Track) => "track",
        Some(AutoRepeatMode::List) => "list",
        _ => "off",
    };
    let progress = match timeline {
        Some(t) if t.duration_ms > 0 => {
            format!(
                "{:.3}",
                (t.position_ms as f64 / t.duration_ms as f64).clamp(0.0, 1.0)
            )
        }
        _ => "0.000".to_string(),
    };

    let mut snapshot = json!({
        "media-playing": if is_playing { "ON" } else { "OFF" },
        "media-shuffle": if shuffle { "ON" } else { "OFF" },
        "media-repeat": repeat,
        "media-repeat-on": if repeat != "off" { "ON" } else { "OFF" },
        "media-progress": progress,
        "media-app": app_name,
    });

    if let Some((bytes, ext)) = art {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        snapshot[ART_DATA_KEY] = Value::String(b64);
        snapshot[ART_EXT_KEY] = Value::String(ext.to_string());
    }

    let mut now_playing = json!({
        "title": if !app_name.is_empty() { app_name } else { "Multimedia" },
        "compact": "",
        "summary": "",
        "rows": [],
    });

    let has_content = props.is_some_and(|p| !p.title.is_empty() || !p.artist.is_empty());
    if !has_content {
        now_playing["rows"] = json!([{ "label": "Status", "value": "Nic nie jest odtwarzane" }]);
        now_playing["compact"] = json!("Nic nie jest odtwarzane");
        now_playing["summary"] = json!("Nic nie jest odtwarzane");
    } else {
        let p = props.unwrap();
        let track = p.title.clone();
        let artist = p.artist.clone();
        let mut rows = Vec::new();
        if !track.is_empty() {
            rows.push(json!({ "label": "Track", "value": track }));
        }
        if !artist.is_empty() {
            rows.push(json!({ "label": "Artist", "value": artist }));
        }
        if !p.album_title.is_empty() {
            rows.push(json!({ "label": "Album", "value": p.album_title }));
        }
        if !app_name.is_empty() {
            rows.push(json!({ "label": "App", "value": app_name }));
        }

        let compact = if !track.is_empty() && !artist.is_empty() {
            format!("{track} \u{2014} {artist}")
        } else if !track.is_empty() {
            track
        } else {
            artist
        };
        now_playing["compact"] = json!(compact);
        now_playing["summary"] = json!(compact);
        now_playing["rows"] = Value::Array(rows);

        if let Some(t) = timeline {
            if t.duration_ms > 0 {
                now_playing["progress"] = json!({
                    "position_ms": t.position_ms,
                    "duration_ms": t.duration_ms,
                    "playing": is_playing,
                });
            }
        }
    }

    snapshot["media-now-playing"] = now_playing;
    snapshot
}

/// Execute a media playback command against the active or target session.
pub fn control(target_app: Option<&str>, action: &str, slider_value: Option<f64>) -> Result<bool> {
    #[cfg(windows)]
    {
        win_media::control(target_app, action, slider_value)
    }
    #[cfg(not(windows))]
    {
        let _ = (target_app, action, slider_value);
        Err(OsError::Unsupported("media"))
    }
}

/// List active media session titles (friendly app names).
pub fn list_sessions() -> Result<Vec<String>> {
    #[cfg(windows)]
    {
        win_media::list_sessions()
    }
    #[cfg(not(windows))]
    {
        Err(OsError::Unsupported("media"))
    }
}

/// Spawn the SMTC push poller / event loop.
pub fn spawn_push(
    consumers: Arc<dyn Fn() -> usize + Send + Sync + 'static>,
) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
    #[cfg(windows)]
    {
        win_media::spawn_push(consumers)
    }
    #[cfg(not(windows))]
    {
        let _ = consumers;
        let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aumid_friendly_name_mapping_examples() {
        assert_eq!(aumid_to_friendly_name("Spotify.exe"), "Spotify");
        assert_eq!(aumid_to_friendly_name("spotify.exe"), "Spotify");
        assert_eq!(
            aumid_to_friendly_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(aumid_to_friendly_name("chrome.exe"), "Google Chrome");
        assert_eq!(aumid_to_friendly_name("msedge.exe"), "Microsoft Edge");
        assert_eq!(aumid_to_friendly_name("firefox.exe"), "Firefox");
        assert_eq!(aumid_to_friendly_name("vlc.exe"), "VLC");
        assert_eq!(aumid_to_friendly_name("foobar2000.exe"), "foobar2000");
        assert_eq!(aumid_to_friendly_name("Tidal.exe"), "TIDAL");
        assert_eq!(aumid_to_friendly_name("AppleMusic.exe"), "Apple Music");
        assert_eq!(
            aumid_to_friendly_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(aumid_to_friendly_name("custom_player.exe"), "Custom Player");
        assert_eq!(aumid_to_friendly_name("F0DC299D809B9700"), "Zen Browser");
    }

    #[test]
    fn build_snapshot_idle_shape() {
        let snapshot = build_snapshot(None, None, None, "", None);
        assert_eq!(snapshot["media-playing"], "OFF");
        assert_eq!(snapshot["media-progress"], "0.000");
        assert_eq!(snapshot["media-repeat"], "off");
        assert_eq!(snapshot["media-repeat-on"], "OFF");
        assert_eq!(snapshot["media-shuffle"], "OFF");
        assert_eq!(snapshot["media-app"], "");
        let np = &snapshot["media-now-playing"];
        assert_eq!(np["title"], "Multimedia");
        assert_eq!(np["compact"], "Nic nie jest odtwarzane");
        assert_eq!(np["summary"], "Nic nie jest odtwarzane");
        assert_eq!(np["rows"][0]["label"], "Status");
        assert_eq!(np["rows"][0]["value"], "Nic nie jest odtwarzane");
        assert!(np.get("progress").is_none());
        assert!(snapshot.get(ART_DATA_KEY).is_none());
    }

    #[test]
    fn build_snapshot_playing_shape_with_art() {
        let props = MediaProperties {
            title: "Bohemian Rhapsody".into(),
            artist: "Queen".into(),
            album_title: "A Night at the Opera".into(),
            ..Default::default()
        };
        let playback = PlaybackInfo {
            status: PlaybackStatus::Playing,
            is_playing: true,
            shuffle: Some(true),
            repeat: Some(AutoRepeatMode::Track),
            can_play: true,
            can_pause: true,
            can_skip_next: true,
            can_skip_prev: true,
            can_stop: true,
            can_seek: true,
        };
        let timeline = TimelineProperties {
            position_ms: 60000,
            duration_ms: 354000,
            min_seek_ms: 0,
            max_seek_ms: 354000,
        };
        let art_bytes = b"fake-jpg-content";

        let snapshot = build_snapshot(
            Some(&props),
            Some(&playback),
            Some(&timeline),
            "Spotify",
            Some((art_bytes, "jpg")),
        );

        assert_eq!(snapshot["media-playing"], "ON");
        assert_eq!(snapshot["media-shuffle"], "ON");
        assert_eq!(snapshot["media-repeat"], "track");
        assert_eq!(snapshot["media-repeat-on"], "ON");
        assert_eq!(snapshot["media-progress"], "0.169");
        assert_eq!(snapshot["media-app"], "Spotify");
        assert!(snapshot.get(ART_DATA_KEY).is_some());
        assert_eq!(snapshot[ART_EXT_KEY], "jpg");

        let np = &snapshot["media-now-playing"];
        let prog = &np["progress"];
        assert_eq!(prog["position_ms"], 60000);
        assert_eq!(prog["duration_ms"], 354000);
        assert_eq!(prog["playing"], true);
    }

    #[test]
    fn input_declarations_cover_the_smtc_kinds_exactly() {
        for (kind, ..) in input_declarations() {
            assert!(is_media_action(kind), "{kind} must be claimable");
        }
        assert_eq!(input_declarations().len(), 3);
        // the virtual-media-key kind is NOT claimed by this integration
        assert!(!is_media_action("vol"));
        assert!(!is_media_action("media-unknown-future"));
    }

    #[test]
    #[ignore]
    fn live_smtc_smoke() {
        #[cfg(windows)]
        {
            let sessions = list_sessions().unwrap();
            println!("Live SMTC sessions: {sessions:?}");

            // the full push loop on real hardware: one snapshot must
            // arrive within a few seconds of someone watching
            let mut rx = spawn_push(std::sync::Arc::new(|| 1));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let snapshot = loop {
                match rx.try_recv() {
                    Ok(snapshot) => break snapshot,
                    Err(_) if std::time::Instant::now() > deadline => {
                        panic!("no snapshot within 10 s");
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
                }
            };
            println!(
                "Live snapshot: playing={} app={:?} now-playing={}",
                snapshot["media-playing"],
                snapshot["media-app"].as_str().unwrap_or_default(),
                snapshot["media-now-playing"],
            );
        }
    }
}
