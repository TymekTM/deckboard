//! Tile action parsing (design §3). [`plan`] is pure - no I/O - so the
//! per-kind command grammar is unit-testable; [`crate::Spotify::exec`]
//! applies the plan over HTTP.

use crate::error::{Result, SpotifyError};

/// One parsed tile action.
#[derive(Debug, PartialEq, Clone)]
pub enum Plan {
    /// `spotify-playback` "play": toggle play/pause (apply reads the
    /// current state and decides which half to run).
    PlayPause,
    Next,
    Previous,
    /// +10 % volume.
    VolumeUp,
    /// -10 % volume.
    VolumeDown,
    /// Mute, remembering the previous level; restore when already muted.
    VolumeMute,
    /// Toggle shuffle.
    Shuffle,
    /// Cycle off -> context -> track -> off.
    Repeat,
    /// Save/unsave the current track.
    Like,
    /// `spotify-add` `{"playlist": "<id or uri>"}`.
    AddToPlaylist(String),
    /// `spotify-tracks` `{"uri": "spotify:playlist:...|album:|track:|artist:"}`.
    PlayUri(String),
    /// `spotify-device` `{"device": "<name or id>"}` (match by name first).
    TransferDevice(String),
    /// `spotify-volume` slider, 0..1 -> volume_percent.
    SetVolume(f64),
    /// `spotify-seek` slider, 0..1 -> position within the track.
    Seek(f64),
    /// `spotify-now-playing`: read-only display tile, claimed so taps
    /// never fall through to the macro dispatcher.
    Noop,
}

/// Parse the tile kind + command (+ slider value for slider tiles) into
/// a [`Plan`]. The command grammar:
///
/// - `spotify-playback`: bare string `play` / `next` / `prev` /
///   `vol_up` / `vol_down` / `vol_mute` (the editor `select` field),
/// - `spotify-add` / `spotify-tracks` / `spotify-device`: a JSON object
///   with one string field,
/// - sliders take the 0..1 value from the exec/slider plumbing.
pub fn plan(kind: &str, command: &str, slider_value: Option<f64>) -> Result<Plan> {
    let value = |clamp: fn(f64) -> Plan| -> Result<Plan> {
        slider_value
            .map(clamp)
            .ok_or_else(|| SpotifyError::BadPayload("slider", "slider value missing".into()))
    };
    let field = |key: &str| -> Result<String> {
        let parsed: serde_json::Value = serde_json::from_str(command).map_err(|e| {
            SpotifyError::BadPayload("command", format!("command is not JSON: {e}"))
        })?;
        let text = parsed
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                SpotifyError::BadPayload("command", format!("missing \"{key}\" string"))
            })?;
        Ok(text.to_string())
    };
    let bare = command.trim().trim_matches('"');
    match kind {
        "spotify-playback" => match bare {
            "play" => Ok(Plan::PlayPause),
            "next" => Ok(Plan::Next),
            "prev" => Ok(Plan::Previous),
            "vol_up" => Ok(Plan::VolumeUp),
            "vol_down" => Ok(Plan::VolumeDown),
            "vol_mute" => Ok(Plan::VolumeMute),
            other => Err(SpotifyError::BadPayload(
                "spotify-playback",
                format!("unknown playback command {other:?}"),
            )),
        },
        "spotify-shuffle" => Ok(Plan::Shuffle),
        "spotify-repeat" => Ok(Plan::Repeat),
        "spotify-like" => Ok(Plan::Like),
        "spotify-add" => Ok(Plan::AddToPlaylist(field("playlist")?)),
        "spotify-tracks" => Ok(Plan::PlayUri(field("uri")?)),
        "spotify-device" => Ok(Plan::TransferDevice(field("device")?)),
        "spotify-volume" => value(|v| Plan::SetVolume(v.clamp(0.0, 1.0))),
        "spotify-seek" => value(|v| Plan::Seek(v.clamp(0.0, 1.0))),
        "spotify-now-playing" => Ok(Plan::Noop),
        other => Err(SpotifyError::BadPayload(
            "kind",
            format!("not a spotify kind: {other}"),
        )),
    }
}

/// Does this action kind belong to the native Spotify integration?
/// Exact matches only: unknown `spotify-*` strings stay unclaimed
/// instead of silently swallowing future kinds.
pub fn is_spotify_action(kind: &str) -> bool {
    matches!(
        kind,
        "spotify-playback"
            | "spotify-shuffle"
            | "spotify-repeat"
            | "spotify-like"
            | "spotify-add"
            | "spotify-tracks"
            | "spotify-device"
            | "spotify-volume"
            | "spotify-seek"
            | "spotify-now-playing"
    )
}

/// Tile inputs this integration serves: (value, icon, color, mode).
/// `custom-value` marks live dual-state tiles (the pushed key drives
/// both wires' toggle faces); `slider`/`status` pick the editor's tile
/// shape for the non-button kinds.
pub fn input_declarations() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    vec![
        ("spotify-playback", "play", "#1DB954", None),
        ("spotify-shuffle", "random", "#1DB954", Some("custom-value")),
        ("spotify-repeat", "repeat", "#1DB954", Some("custom-value")),
        ("spotify-like", "heart", "#1DB954", Some("custom-value")),
        ("spotify-add", "plus", "#1DB954", None),
        ("spotify-tracks", "record-vinyl", "#1DB954", None),
        ("spotify-device", "tv", "#1DB954", None),
        ("spotify-volume", "volume-up", "#1DB954", Some("slider")),
        ("spotify-seek", "clock", "#1DB954", Some("slider")),
        ("spotify-now-playing", "music", "#1DB954", Some("status")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_commands_parse_per_bare_string() {
        assert_eq!(
            plan("spotify-playback", "play", None).unwrap(),
            Plan::PlayPause
        );
        assert_eq!(plan("spotify-playback", "next", None).unwrap(), Plan::Next);
        assert_eq!(
            plan("spotify-playback", "prev", None).unwrap(),
            Plan::Previous
        );
        assert_eq!(
            plan("spotify-playback", "vol_up", None).unwrap(),
            Plan::VolumeUp
        );
        assert_eq!(
            plan("spotify-playback", "vol_down", None).unwrap(),
            Plan::VolumeDown
        );
        assert_eq!(
            plan("spotify-playback", "vol_mute", None).unwrap(),
            Plan::VolumeMute
        );
        // whitespace tolerated, a JSON-quoted select value too
        assert_eq!(
            plan("spotify-playback", " \"play\" ", None).unwrap(),
            Plan::PlayPause
        );
        assert!(plan("spotify-playback", "bogus", None).is_err());
        assert!(plan("spotify-playback", "", None).is_err());
    }

    #[test]
    fn toggles_need_no_command() {
        assert_eq!(plan("spotify-shuffle", "", None).unwrap(), Plan::Shuffle);
        assert_eq!(plan("spotify-repeat", "", None).unwrap(), Plan::Repeat);
        assert_eq!(plan("spotify-like", "", None).unwrap(), Plan::Like);
        assert_eq!(plan("spotify-now-playing", "", None).unwrap(), Plan::Noop);
    }

    #[test]
    fn json_kinds_parse_their_single_field() {
        assert_eq!(
            plan(
                "spotify-add",
                r#"{"playlist":"37i9dQZF1DXcBWIGoYBM5M"}"#,
                None
            )
            .unwrap(),
            Plan::AddToPlaylist("37i9dQZF1DXcBWIGoYBM5M".into())
        );
        // a playlist URI is equally valid
        assert_eq!(
            plan(
                "spotify-add",
                r#"{"playlist":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"}"#,
                None
            )
            .unwrap(),
            Plan::AddToPlaylist("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".into())
        );
        assert_eq!(
            plan(
                "spotify-tracks",
                r#"{"uri":"spotify:album:5z7tK5aXmnS7jlU6iv4Z7e"}"#,
                None
            )
            .unwrap(),
            Plan::PlayUri("spotify:album:5z7tK5aXmnS7jlU6iv4Z7e".into())
        );
        assert_eq!(
            plan("spotify-device", r#"{"device":"Kitchen"}"#, None).unwrap(),
            Plan::TransferDevice("Kitchen".into())
        );
        // missing / non-string / non-JSON commands are BadPayload
        assert!(plan("spotify-tracks", r#"{"uri":""}"#, None).is_err());
        assert!(plan("spotify-add", "nope", None).is_err());
        assert!(plan("spotify-device", "{}", None).is_err());
    }

    #[test]
    fn sliders_take_the_0_to_1_value() {
        assert_eq!(
            plan("spotify-volume", "", Some(0.55)).unwrap(),
            Plan::SetVolume(0.55)
        );
        assert_eq!(
            plan("spotify-seek", "", Some(1.0)).unwrap(),
            Plan::Seek(1.0)
        );
        // out-of-range values clamp instead of erroring
        assert_eq!(
            plan("spotify-volume", "", Some(7.0)).unwrap(),
            Plan::SetVolume(1.0)
        );
        assert_eq!(
            plan("spotify-seek", "", Some(-1.0)).unwrap(),
            Plan::Seek(0.0)
        );
        // a slider tile pressed as a button has no value: that is a
        // payload error the dispatcher surfaces
        assert!(matches!(
            plan("spotify-volume", "", None),
            Err(SpotifyError::BadPayload("slider", _))
        ));
    }

    #[test]
    fn kinds_claimed_exactly() {
        for (kind, ..) in input_declarations() {
            assert!(is_spotify_action(kind), "{kind} must be claimable");
        }
        assert!(!is_spotify_action("spotify"));
        assert!(!is_spotify_action("spotify-unknown-future"));
        assert!(!is_spotify_action("obs-start"));
        // declarations stay in sync with the claim list
        assert_eq!(input_declarations().len(), 10);
    }
}
