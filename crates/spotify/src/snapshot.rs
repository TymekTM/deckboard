//! One poll's snapshot (design §4): every `spotify-*` key with its
//! display-ready value. Pure - the poller feeds it `/v1/me/player`
//! JSON plus the cross-poll memories (liked, last volume).

use serde_json::{json, Value};

/// Auth value for the `spotify-auth` key. "off" (no configuration at
/// all, so no poller exists) is pushed by the host, not by the poller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    Ok,
    NeedsLogin,
}

impl AuthState {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthState::Ok => "ok",
            AuthState::NeedsLogin => "needs-login",
        }
    }
}

/// ON/OFF strings for dual-state tiles: they work on the legacy wire
/// (string-typed custom values), in mobile `isActiveValue` and in the
/// desktop's `stateActive` (design §3).
fn on_off(on: bool) -> &'static str {
    if on {
        "ON"
    } else {
        "OFF"
    }
}

/// Pick the ~300 px cover URL from `album.images` (640/300/64 may be
/// absent or empty entirely - design tolerates missing art).
fn art_url(player: &Value) -> String {
    let Some(images) = player
        .pointer("/item/album/images")
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    let mut best: Option<(u32, &str)> = None;
    for image in images {
        let Some(url) = image.get("url").and_then(Value::as_str) else {
            continue;
        };
        if url.is_empty() {
            continue;
        }
        let width = image.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
        match best {
            // exact 300 wins; otherwise the closest to 300
            Some((w, _))
                if w == 300 || (w.abs_diff(300) <= width.abs_diff(300) && width != 300) => {}
            _ => best = Some((width, url)),
        }
    }
    best.map(|(_, url)| url.to_string()).unwrap_or_default()
}

fn artists(player: &Value) -> String {
    player
        .pointer("/item/artists")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn track_name(player: &Value) -> String {
    player
        .pointer("/item/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn album_name(player: &Value) -> String {
    player
        .pointer("/item/album/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn device_name(player: &Value) -> String {
    player
        .pointer("/device/name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn playing(player: &Value) -> bool {
    player
        .get("is_playing")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The `spotify-now-playing` status payload: the shape mobile/desktop
/// already render for aidev tiles (`{title, rows, compact, summary}`),
/// plus the optional `progress` object (new, ignored by old clients).
/// The `image` field (album art as an asset hash) is added by the host
/// lane - the poller instead exposes the raw art URL under the separate
/// internal `spotify-art-url` key.
fn now_playing_payload(player: Option<&Value>) -> Value {
    let mut payload = json!({
        "title": "Spotify",
        "compact": "",
        "summary": "",
        "rows": [],
    });
    let Some(player) = player else {
        payload["rows"] = json!([{ "label": "Status", "value": "Nothing playing" }]);
        payload["compact"] = json!("Nothing playing");
        payload["summary"] = json!("Nothing playing");
        return payload;
    };
    let track = track_name(player);
    let artist = artists(player);
    let mut rows = Vec::new();
    if !track.is_empty() {
        rows.push(json!({ "label": "Track", "value": track }));
    }
    if !artist.is_empty() {
        rows.push(json!({ "label": "Artist", "value": artist }));
    }
    let album = album_name(player);
    if !album.is_empty() {
        rows.push(json!({ "label": "Album", "value": album }));
    }
    let device = device_name(player);
    if !device.is_empty() {
        rows.push(json!({ "label": "Device", "value": device }));
    }
    if rows.is_empty() {
        // a player object without an item (ad breaks report no item)
        rows.push(json!({ "label": "Status", "value": "Nothing playing" }));
        payload["compact"] = json!("Nothing playing");
        payload["summary"] = json!("Nothing playing");
    } else {
        let compact = if artist.is_empty() {
            track.clone()
        } else {
            format!("{track} \u{2014} {artist}")
        };
        payload["compact"] = json!(compact);
        payload["summary"] = json!(compact);
    }
    payload["rows"] = Value::Array(rows);
    if let (Some(progress), Some(duration)) = (
        player.get("progress_ms").and_then(Value::as_u64),
        player.pointer("/item/duration_ms").and_then(Value::as_u64),
    ) {
        payload["progress"] = json!({
            "position_ms": progress,
            "duration_ms": duration,
            "playing": playing(player),
        });
    }
    payload
}

/// Build one snapshot from the poll inputs. `last_volume`/`liked` are
/// the poller's memories for values the API reports as null/absent.
///
/// Keys (design §4): `spotify-playing`, `spotify-shuffle`,
/// `spotify-repeat`, `spotify-repeat-on`, `spotify-liked`,
/// `spotify-volume`, `spotify-volume-level`, `spotify-progress`, `spotify-device`,
/// `spotify-now-playing`, `spotify-auth`, plus the internal
/// `spotify-art-url` (**host-consumed; the host must strip it before
/// forwarding the snapshot to clients**).
/// The volume as a 0..1 slider position. `spotify-volume` stays the
/// legacy percent string ("55"), but slider widgets read their live value
/// as a 0..1 fraction - the same scale `exec_slider` writes - so the
/// v2 slider tile watches this key instead.
fn volume_level(percent: u32) -> String {
    format!("{:.2}", f64::from(percent.min(100)) / 100.0)
}

pub fn build_snapshot(
    player: Option<&Value>,
    liked: Option<bool>,
    auth: AuthState,
    last_volume: Option<u32>,
) -> Value {
    if auth == AuthState::NeedsLogin {
        let mut payload = json!({
            "spotify-playing": "OFF",
            "spotify-shuffle": "OFF",
            "spotify-repeat": "off",
            "spotify-repeat-on": "OFF",
            "spotify-liked": "OFF",
            "spotify-volume": last_volume.unwrap_or(100).to_string(),
            "spotify-volume-level": volume_level(last_volume.unwrap_or(100)),
            "spotify-progress": "0.000",
            "spotify-device": "",
            "spotify-auth": auth.as_str(),
            "spotify-art-url": "",
        });
        payload["spotify-now-playing"] = json!({
            "title": "Spotify",
            "rows": [{ "label": "Status", "value": "Log in via Pulpit settings" }],
            "compact": "Log in via Pulpit settings",
            "summary": "Log in via Pulpit settings",
        });
        return payload;
    }
    let shuffle = player
        .and_then(|p| p.get("shuffle_state"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let repeat = player
        .and_then(|p| p.get("repeat_state"))
        .and_then(Value::as_str)
        .unwrap_or("off");
    let volume = player
        .and_then(|p| p.pointer("/device/volume_percent"))
        .and_then(Value::as_u64)
        .map(|v| v.min(100) as u32)
        .or(last_volume)
        .unwrap_or(100);
    let progress = match (
        player.and_then(|p| p.get("progress_ms").and_then(Value::as_u64)),
        player.and_then(|p| p.pointer("/item/duration_ms").and_then(Value::as_u64)),
    ) {
        (Some(position), Some(duration)) if duration > 0 => {
            format!("{:.3}", (position as f64 / duration as f64).clamp(0.0, 1.0))
        }
        _ => "0.000".to_string(),
    };
    let mut snapshot = json!({
        "spotify-playing": on_off(player.is_some_and(playing)),
        "spotify-shuffle": on_off(shuffle),
        "spotify-repeat": repeat,
        "spotify-repeat-on": on_off(repeat != "off"),
        "spotify-liked": on_off(liked.unwrap_or(false)),
        "spotify-volume": volume.to_string(),
        "spotify-volume-level": volume_level(volume),
        "spotify-progress": progress,
        "spotify-device": player.map(device_name).unwrap_or_default(),
        "spotify-auth": auth.as_str(),
        "spotify-art-url": player.map(art_url).unwrap_or_default(),
    });
    snapshot["spotify-now-playing"] = now_playing_payload(player);
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player_json() -> Value {
        serde_json::json!({
            "device": { "id": "d1", "name": "Kitchen", "volume_percent": 55, "is_active": true },
            "shuffle_state": true,
            "repeat_state": "context",
            "is_playing": true,
            "progress_ms": 61234,
            "currently_playing_type": "track",
            "item": {
                "uri": "spotify:track:abc",
                "name": "Song name",
                "duration_ms": 201000,
                "artists": [{ "name": "A" }, { "name": "B" }],
                "album": {
                    "name": "Best Of",
                    "images": [
                        { "url": "https://i.scdn.co/image/640", "width": 640, "height": 640 },
                        { "url": "https://i.scdn.co/image/300", "width": 300, "height": 300 },
                        { "url": "https://i.scdn.co/image/64", "width": 64, "height": 64 }
                    ]
                }
            }
        })
    }

    #[test]
    fn snapshot_carries_exactly_the_design_keys_and_values() {
        let player = player_json();
        let snapshot = build_snapshot(Some(&player), Some(true), AuthState::Ok, None);
        assert_eq!(snapshot["spotify-playing"], "ON");
        assert_eq!(snapshot["spotify-shuffle"], "ON");
        assert_eq!(snapshot["spotify-repeat"], "context");
        assert_eq!(snapshot["spotify-repeat-on"], "ON");
        assert_eq!(snapshot["spotify-liked"], "ON");
        assert_eq!(snapshot["spotify-volume"], "55");
        assert_eq!(snapshot["spotify-volume-level"], "0.55");
        assert_eq!(snapshot["spotify-progress"], "0.305");
        assert_eq!(snapshot["spotify-device"], "Kitchen");
        assert_eq!(snapshot["spotify-auth"], "ok");
        // the 300 px art goes to the internal host-consumed key only
        assert_eq!(snapshot["spotify-art-url"], "https://i.scdn.co/image/300");
        // exactly the design §4 keys, nothing more
        let mut keys: Vec<&str> = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "spotify-art-url",
                "spotify-auth",
                "spotify-device",
                "spotify-liked",
                "spotify-now-playing",
                "spotify-playing",
                "spotify-progress",
                "spotify-repeat",
                "spotify-repeat-on",
                "spotify-shuffle",
                "spotify-volume",
                "spotify-volume-level",
            ]
        );
    }

    #[test]
    fn volume_level_is_the_percent_as_a_fraction() {
        assert_eq!(volume_level(0), "0.00");
        assert_eq!(volume_level(7), "0.07");
        assert_eq!(volume_level(100), "1.00");
        // a bogus API value never leaves the slider's 0..1 range
        assert_eq!(volume_level(250), "1.00");
        let logged_out = build_snapshot(None, None, AuthState::NeedsLogin, Some(30));
        assert_eq!(logged_out["spotify-volume"], "30");
        assert_eq!(logged_out["spotify-volume-level"], "0.30");
    }

    #[test]
    fn now_playing_payload_shape() {
        let player = player_json();
        let payload =
            build_snapshot(Some(&player), None, AuthState::Ok, None)["spotify-now-playing"].clone();
        assert_eq!(payload["title"], "Spotify");
        assert_eq!(
            payload["rows"][0],
            json!({ "label": "Track", "value": "Song name" })
        );
        assert_eq!(
            payload["rows"][1],
            json!({ "label": "Artist", "value": "A, B" })
        );
        assert_eq!(
            payload["rows"][2],
            json!({ "label": "Album", "value": "Best Of" })
        );
        assert_eq!(
            payload["rows"][3],
            json!({ "label": "Device", "value": "Kitchen" })
        );
        assert_eq!(payload["compact"], "Song name \u{2014} A, B");
        assert_eq!(payload["summary"], "Song name \u{2014} A, B");
        // optional new fields: progress present, image absent (host lane)
        assert_eq!(
            payload["progress"],
            json!({ "position_ms": 61234, "duration_ms": 201000, "playing": true })
        );
        assert!(payload.get("image").is_none());
    }

    #[test]
    fn nothing_playing_and_needs_login_shapes() {
        let idle = build_snapshot(None, None, AuthState::Ok, Some(42));
        assert_eq!(idle["spotify-playing"], "OFF");
        assert_eq!(idle["spotify-volume"], "42");
        assert_eq!(idle["spotify-progress"], "0.000");
        assert_eq!(idle["spotify-art-url"], "");
        let payload = &idle["spotify-now-playing"];
        assert_eq!(payload["rows"][0]["value"], "Nothing playing");
        assert!(payload.get("progress").is_none());

        let login = build_snapshot(None, None, AuthState::NeedsLogin, None);
        assert_eq!(login["spotify-auth"], "needs-login");
        assert_eq!(
            login["spotify-now-playing"]["rows"][0]["value"],
            "Log in via Pulpit settings"
        );
        assert_eq!(
            login["spotify-now-playing"]["compact"],
            "Log in via Pulpit settings"
        );
    }

    #[test]
    fn art_picks_closest_to_300_when_no_exact_size() {
        let mut player = player_json();
        // only a 640 image available
        player["item"]["album"]["images"] = json!([{ "url": "https://x/640", "width": 640 }]);
        assert_eq!(
            build_snapshot(Some(&player), None, AuthState::Ok, None)["spotify-art-url"],
            "https://x/640"
        );
        // empty images list
        player["item"]["album"]["images"] = json!([]);
        assert_eq!(
            build_snapshot(Some(&player), None, AuthState::Ok, None)["spotify-art-url"],
            ""
        );
    }

    #[test]
    fn paused_player_reports_off_but_keeps_values() {
        let mut player = player_json();
        player["is_playing"] = json!(false);
        let snapshot = build_snapshot(Some(&player), None, AuthState::Ok, None);
        assert_eq!(snapshot["spotify-playing"], "OFF");
        // progress still reflects the paused position
        assert_eq!(snapshot["spotify-progress"], "0.305");
        assert_eq!(
            snapshot["spotify-now-playing"]["progress"]["playing"],
            false
        );
    }
}
