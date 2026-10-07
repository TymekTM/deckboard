//! Board/shortcut payload mapping - a 1:1 port of the original
//! `syncAllBoards` + `mapMacroButtons` + ButtonStyle mapper (webpack
//! modules 5981/8742/2836). Field names and defaults are contractual:
//! the stock Android client renders exactly these fields.

use pulpit_db::{clamp_placement, BoardRow, ButtonRow, MAX_BOARD_DIM};
use serde_json::{json, Map, Value};

use crate::props::{Props, StyleResolver, FALLBACK_COLOR};

/// Types whose raw command string is sent to the client untouched.
/// `media-control` is a select-style kind exactly like `vol` (the raw
/// transport action is the whole command).
const RAW_COMMAND_TYPES: &[&str] = &[
    "board",
    "obs-control",
    "slobs-control",
    "xsplit-control",
    "vol",
    "media-control",
];

/// Defensive ceiling for board dimensions on the wire
/// ([`pulpit_db::MAX_BOARD_DIM`], one definition for the import bound
/// and both wire builders). The filler loop below costs W*H per board,
/// and the client lays out a W*H grid: a junk row must not reach
/// either at stored size (audit C4).
fn clamp_dim(v: i64) -> i64 {
    v.clamp(1, MAX_BOARD_DIM)
}

pub struct Mapper {
    resolver: &'static StyleResolver,
}

impl Mapper {
    pub fn new() -> Mapper {
        Mapper {
            resolver: StyleResolver::global(),
        }
    }

    /// Full board payload for one grid variant.
    /// `max_w`/`max_h`: pass the board dimensions for pro, (4, 3) for basic.
    pub fn board_payload(&self, board: &BoardRow, buttons: &[ButtonRow], pro: bool) -> Value {
        let shortcuts = self.shortcuts_payload(board, buttons, pro);
        json!({
            "id": board.id,
            "name": board.name,
            "background": board.background,
            "layout": board.layout,
            "image": board.image,
            "sort": board.sort,
            "type": board.kind,
            "args": board.args,
            "order": board.order,
            "width": clamp_dim(board.width),
            "height": clamp_dim(board.height),
            "converted": board.converted,
            "staggered": true,
            "shortcuts": shortcuts,
        })
    }

    /// Mapped buttons + filler cells, filtered to the variant's grid.
    /// Fillers and filters run on the clamped dimensions, so an
    /// oversized row cannot make this loop allocate W*H. Tile placements
    /// are pulled back inside the board grid first (DESK-03): the pro
    /// filter below drops an out-of-bounds tile entirely, and a filler
    /// must not be minted for a cell the clamped tile ends up covering.
    fn shortcuts_payload(&self, board: &BoardRow, buttons: &[ButtonRow], pro: bool) -> Vec<Value> {
        let (width, height) = (clamp_dim(board.width), clamp_dim(board.height));
        let placements: Vec<(i64, i64, i64, i64)> = buttons
            .iter()
            .map(|b| clamp_placement(b.x.unwrap_or(0), b.y.unwrap_or(0), b.w, b.h, width, height))
            .collect();
        let board_buttons: &[(i64, i64, i64, i64)] = &placements;

        // Fillers for every cell of the board grid not covered by a button.
        let mut mapped: Vec<Value> = Vec::with_capacity(buttons.len());
        for (b, (x, y, w, h)) in buttons.iter().zip(&placements) {
            let mut payload = self.shortcut_payload(b);
            payload["x"] = json!(x);
            payload["y"] = json!(y);
            payload["w"] = json!(w);
            payload["h"] = json!(h);
            mapped.push(payload);
        }
        for y in 0..height {
            for x in 0..width {
                let covered = board_buttons
                    .iter()
                    .any(|(bx, by, bw, bh)| x >= *bx && x < bx + bw && y >= *by && y < by + bh);
                if !covered {
                    mapped.push(json!({ "id": null, "x": x, "y": y, "w": 1, "h": 1 }));
                }
            }
        }

        mapped.sort_by_key(|v| {
            let x = v["x"].as_i64().unwrap_or(0);
            let y = v["y"].as_i64().unwrap_or(0);
            (y, x)
        });

        mapped
            .into_iter()
            .filter_map(|mut v| {
                let x = v["x"].as_i64().unwrap_or(0);
                let y = v["y"].as_i64().unwrap_or(0);
                if pro {
                    (x < width && y < height).then_some(v)
                } else if x >= 4 || y >= 3 {
                    None
                } else {
                    // basic variant crops oversized buttons to the 4x3 grid
                    let w = v["w"].as_i64().unwrap_or(1);
                    let h = v["h"].as_i64().unwrap_or(1);
                    if x + w > 4 {
                        v["w"] = json!(4 - x);
                    }
                    if y + h > 3 {
                        v["h"] = json!(3 - y);
                    }
                    Some(v)
                }
            })
            .collect()
    }

    /// The payload's `app` value alone: the style-table/extension lookup
    /// plus the vol_mute quirk below. Per-event paths (the v2 gesture
    /// check) use this instead of building the full payload.
    pub fn app_value(&self, b: &ButtonRow) -> Option<String> {
        let props = self.resolver.props_for(&b.kind, b.command.as_deref());
        let command = transform_command(&b.kind, b.command.as_deref(), &props);
        if b.kind == "vol" && command == "vol_mute" {
            return Some("custom-value".to_string());
        }
        // the play command of spotify-playback is a live toggle on the
        // pushed spotify-playing key (same shape as the vol_mute quirk)
        if b.kind == "spotify-playback" && b.command.as_deref() == Some("play") {
            return Some("custom-value".to_string());
        }
        // utility tools carry their live state (server-authoritative
        // timer/stopwatch/counter, key `tool-<id>`) through the same
        // custom-value lane as vol_mute
        if matches!(
            b.kind.as_str(),
            "tool-timer" | "tool-stopwatch" | "tool-counter"
        ) {
            return Some("custom-value".to_string());
        }
        props.app.clone()
    }

    /// One button -> client shortcut object (ButtonStyle port).
    pub fn shortcut_payload(&self, b: &ButtonRow) -> Value {
        let props = self.resolver.props_for(&b.kind, b.command.as_deref());
        let command = transform_command(&b.kind, b.command.as_deref(), &props);
        let mut extra = extra_listener(&b.kind, b.command.as_deref(), b.mode.as_str(), &props);
        let app = self.app_value(b);

        // icon from db overrides the type default; prefix follows the icon
        let (unicode, unicode2, prefix) = match b.icon.as_deref() {
            Some(icon) if !icon.is_empty() => (
                self.resolver.icon_unicode(icon, "fas").unwrap_or_default(),
                b.icon2
                    .as_deref()
                    .filter(|i| !i.is_empty())
                    .and_then(|i| self.resolver.icon_unicode(i, "fas"))
                    .unwrap_or_else(|| props.unicode.clone().unwrap_or_default()),
                "fas".to_string(),
            ),
            _ => (
                props.unicode.clone().unwrap_or_default(),
                props.unicode.clone().unwrap_or_default(),
                props.prefix.clone().unwrap_or_else(|| "fas".to_string()),
            ),
        };

        // original quirk: vol_mute buttons report their state under
        // the custom-value app with the "speaker-muted" key (the app half
        // lives in `app_value`, shared with the per-event paths)
        if b.kind == "vol" && command == "vol_mute" {
            extra = "speaker-muted".to_string();
        }
        if matches!(b.kind.as_str(), "tool-timer" | "tool-stopwatch" | "tool-counter") {
            extra = format!("tool-{}", b.id);
        }

        let mut o = Map::new();
        o.insert("id".into(), json!(b.id));
        o.insert("board_id".into(), json!(b.board_id));
        o.insert("type".into(), json!(b.kind));
        o.insert("command".into(), json!(command));
        insert_opt(
            &mut o,
            "color",
            b.color.clone(),
            props.color.clone(),
            FALLBACK_COLOR,
        );
        insert_opt(&mut o, "color2", b.color2.clone(), props.color.clone(), "");
        insert_opt(&mut o, "img", b.img.clone(), None, "");
        insert_opt(&mut o, "img2", b.img2.clone(), None, "");
        insert_opt(&mut o, "icon_color", b.icon_color.clone(), None, "");
        insert_opt(&mut o, "icon_color2", b.icon_color2.clone(), None, "");
        o.insert("position".into(), json!(b.position));
        o.insert("title".into(), json!(b.title));
        o.insert("title_position".into(), json!(b.title_position));
        o.insert("title_position2".into(), json!(b.title_position2));
        insert_opt(
            &mut o,
            "title_color",
            b.title_color.clone(),
            Some("#ffffff".into()),
            "#ffffff",
        );
        insert_opt(
            &mut o,
            "title_color2",
            b.title_color2.clone(),
            Some("#ffffff".into()),
            "#ffffff",
        );
        insert_opt(
            &mut o,
            "title_box_color",
            b.title_box_color.clone(),
            None,
            "",
        );
        insert_opt(
            &mut o,
            "title_box_color2",
            b.title_box_color2.clone(),
            None,
            "",
        );
        insert_opt(&mut o, "border_color", b.border_color.clone(), None, "");
        insert_opt(&mut o, "border_color2", b.border_color2.clone(), None, "");
        o.insert("options".into(), json!(b.options));
        o.insert("shape".into(), json!(b.shape));
        o.insert("shape2".into(), json!(b.shape2));
        o.insert("action".into(), json!(""));
        o.insert("target".into(), json!(""));
        o.insert("x".into(), json!(b.x.unwrap_or(0)));
        o.insert("y".into(), json!(b.y.unwrap_or(0)));
        o.insert("w".into(), json!(b.w));
        o.insert("h".into(), json!(b.h));
        o.insert("mode".into(), json!(b.mode));
        o.insert("unicode".into(), json!(unicode));
        o.insert("unicode2".into(), json!(unicode2));
        o.insert("prefix".into(), json!(prefix));
        o.insert("extra".into(), json!(extra));
        if let Some(a) = app {
            o.insert("app".into(), json!(a));
        }
        if let Some(tk) = &props.toggle_key {
            o.insert("toggle_key".into(), json!(tk));
        }
        Value::Object(o)
    }
}

impl Default for Mapper {
    fn default() -> Self {
        Self::new()
    }
}

fn insert_opt(
    o: &mut Map<String, Value>,
    key: &str,
    row: Option<String>,
    fallback: Option<String>,
    default: &str,
) {
    let v = row.or(fallback).unwrap_or_else(|| default.to_string());
    o.insert(key.into(), json!(v));
}

/// Command string transformation before sending (original mapper rules).
fn transform_command(kind: &str, command: Option<&str>, props: &Props) -> String {
    let raw = command.unwrap_or_default();
    if RAW_COMMAND_TYPES.contains(&kind) {
        return raw.to_string();
    }
    if let Some(json_key) = &props.json_key {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(extracted) = v.get(json_key).and_then(Value::as_str) {
                return extracted.to_string();
            }
        }
        return String::new();
    }
    String::new()
}

/// Spotify dual-state tiles watch pushed keys that differ from the tile
/// kind (round-4 Spotify design §4): the `play` command of
/// spotify-playback flips `spotify-playing`, `spotify-repeat` lights on
/// `spotify-repeat-on`, `spotify-like` on `spotify-liked`, and the seek
/// slider reads track `spotify-progress`. Like the vol_mute quirk, the
/// wire must name the pushed key exactly or the stock client never
/// flips the tile.
fn spotify_listener(kind: &str, command: Option<&str>) -> Option<&'static str> {
    match (kind, command.unwrap_or_default()) {
        ("spotify-playback", "play") => Some("spotify-playing"),
        ("spotify-repeat", _) => Some("spotify-repeat-on"),
        ("spotify-like", _) => Some("spotify-liked"),
        ("spotify-seek", _) => Some("spotify-progress"),
        _ => None,
    }
}

/// System-media (SMTC) tiles follow the same rule: the seek slider reads
/// the pushed `media-progress` fraction. The display and control kinds
/// watch nothing (the payload rides the status lane under the kind).
fn media_listener(kind: &str) -> Option<&'static str> {
    match kind {
        "media-seek" => Some("media-progress"),
        _ => None,
    }
}

/// The obs kinds the native integration (round 5) pushes live state
/// for: the watch strings follow the stored command JSON shapes the
/// original wrote (`scene` / `source` / `device` / `filter` fields).
/// `obs-scene` keeps the raw command text the original mapped; the
/// others compose `<scene>::<source>[::[<filter>]]` from the parsed
/// fields - the empty segments stay in, so the keys are stable.
/// obs-device-audio falls back from `source` to the catalog's `device`
/// field (it used to resolve to "" and never light anything), and the
/// argument-less obs toggles watch their kind name so the native
/// pushes (`obs-studio-mode`, `obs-record`, `obs-stream`) reach the
/// v2 state channels too.
fn obs_listener(kind: &str, raw: &str) -> Option<String> {
    match kind {
        "obs-scene" => Some(raw.to_string()),
        "obs-source" => Some(parsed(raw, "scene") + "::" + &parsed(raw, "source")),
        "obs-filter" => Some(
            parsed(raw, "scene") + "::" + &parsed(raw, "source") + "::" + &parsed(raw, "filter"),
        ),
        "obs-device-audio" => {
            let source = parsed(raw, "source");
            if source.is_empty() {
                Some(parsed(raw, "device"))
            } else {
                Some(source)
            }
        }
        "obs-studio-mode" | "obs-record" | "obs-stream" => Some(kind.to_string()),
        _ => None,
    }
}

/// The `extra` field: which state key the client watches for toggles/graphs.
fn extra_listener(kind: &str, command: Option<&str>, mode: &str, props: &Props) -> String {
    if let Some(key) = spotify_listener(kind, command) {
        return key.to_string();
    }
    if let Some(key) = media_listener(kind) {
        return key.to_string();
    }
    let raw = command.unwrap_or_default();
    if let Some(key) = obs_listener(kind, raw) {
        return key;
    }
    if props.json_key.is_some() {
        // jsonKey types use the type as the listener key
        return kind.to_string();
    }
    if mode == "graph" {
        return if raw.is_empty() {
            kind.to_string()
        } else {
            raw.to_string()
        };
    }
    match kind {
        "slobs-scene" => raw.to_string(),
        "slobs-source" | "slobs-device-audio" => parsed(raw, "source"),
        "twitch-chat-box" => raw.to_string(),
        _ => {
            if mode == "slider" {
                if !raw.is_empty() {
                    format!("{kind}_{raw}")
                } else {
                    kind.to_string()
                }
            } else if props.app.as_deref() == Some("custom-value")
                || props.app.as_deref() == Some("third-party")
            {
                if !raw.is_empty() {
                    raw.to_string()
                } else {
                    kind.to_string()
                }
            } else {
                String::new()
            }
        }
    }
}

fn parsed(raw: &str, key: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> BoardRow {
        BoardRow {
            id: 1,
            name: "My Board".into(),
            background: "#2c3e50".into(),
            layout: 6,
            image: String::new(),
            sort: 0,
            kind: "buttons".into(),
            args: None,
            order: 0,
            width: 6,
            height: 5,
            converted: 1,
        }
    }

    fn button(kind: &str, command: Option<&str>, x: i64, y: i64, w: i64, h: i64) -> ButtonRow {
        ButtonRow {
            id: 10,
            board_id: 1,
            kind: kind.into(),
            command: command.map(str::to_string),
            title: Some("T".into()),
            title_position: 0,
            title_color: None,
            title_box_color: None,
            color: None,
            icon_color: None,
            icon_color2: None,
            border_color: None,
            shape: 0,
            icon: None,
            img: None,
            img2: None,
            icon2: None,
            color2: None,
            shape2: 0,
            border_color2: None,
            title_position2: 0,
            title_box_color2: None,
            title_color2: None,
            position: None,
            position2: 0,
            mode: "button".into(),
            x: Some(x),
            y: Some(y),
            w,
            h,
            options: None,
        }
    }

    #[test]
    fn graph_mode_extra_falls_back_to_type() {
        let m = Mapper::new();
        let mut b = button("twitch-follower-count", None, 0, 0, 2, 1);
        b.mode = "graph".into();
        let s = m.shortcut_payload(&b);
        // the tablet categorizes by mode and watches customValues[extra]
        assert_eq!(s["mode"], "graph");
        assert_eq!(s["extra"], "twitch-follower-count");
        let mut b = button("custom-metric", Some("my-key"), 0, 0, 2, 1);
        b.mode = "graph".into();
        let s = m.shortcut_payload(&b);
        assert_eq!(s["mode"], "graph");
        assert_eq!(s["extra"], "my-key");
    }

    #[test]
    fn http_request_kind_passes_through_without_a_mapping_entry() {
        // the http-request tile carries its config in the command column;
        // the stock client only renders a pressable button, so the wire
        // must keep the type and not mangle anything (the command stays
        // server-side and is intentionally blanked like any unknown kind)
        let m = Mapper::new();
        let b = button(
            "http-request",
            Some(r#"{"method":"POST","url":"http://ha.local/api"}"#),
            0,
            0,
            1,
            1,
        );
        let s = m.shortcut_payload(&b);
        assert_eq!(s["type"], "http-request");
        assert_eq!(s["mode"], "button");
        assert!(s["extra"].as_str().unwrap().is_empty());
    }

    #[test]
    fn tool_tiles_watch_their_per_id_state_key() {
        let m = Mapper::new();
        for kind in ["tool-timer", "tool-stopwatch", "tool-counter"] {
            let s = m.shortcut_payload(&button(kind, None, 0, 0, 1, 1));
            // the manager pushes compact state and 1 Hz labels under
            // `tool-<button id>`; the stock client binds custom values
            // through the app+extra pair, like vol_mute
            assert_eq!(s["extra"], "tool-10", "{kind}");
            assert_eq!(s["app"], "custom-value", "{kind}");
        }
        // the clock never pushes - no listener, no app override
        let s = m.shortcut_payload(&button("tool-clock", None, 0, 0, 1, 1));
        assert_eq!(s["extra"], "");
        assert!(s.get("app").is_none());
    }

    #[test]
    fn fillers_cover_every_free_cell() {
        let m = Mapper::new();
        let b = board();
        let payload = m.board_payload(&b, &[button("url", Some("https://x.co"), 0, 0, 1, 1)], true);
        let sc = payload["shortcuts"].as_array().unwrap();
        let fillers = sc.iter().filter(|s| s["id"].is_null()).count();
        assert_eq!(fillers, (6 * 5 - 1) as usize);
        assert!(payload["staggered"].as_bool().unwrap());
        // sorted by y then x
        let first = sc.first().unwrap();
        assert_eq!(
            (first["x"].as_i64(), first["y"].as_i64()),
            (Some(0), Some(0))
        );
    }

    #[test]
    fn basic_variant_crops_to_4x3() {
        let m = Mapper::new();
        let b = board();
        let buttons = [
            button("url", Some("https://x.co"), 0, 0, 1, 1),
            // (3,2,4,4) hangs off the 6x5 board; DESK-03 pulls it to
            // (2,2,4,4) first, then the 4x3 crop trims it to (2,2,2,1)
            button("url", Some("https://x.co"), 3, 2, 4, 4),
            button("url", Some("https://x.co"), 5, 4, 1, 1), // outside 4x3 -> dropped
        ];
        let basic = m.board_payload(&b, &buttons, false);
        let cropped = basic["shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_i64() == Some(10) && s["x"].as_i64() == Some(2))
            .unwrap();
        // clamped to (2,1,4,4) inside 6x5 first, then the 4x3 crop
        // trims it to (2,1,2,2)
        assert_eq!(
            (cropped["w"].as_i64(), cropped["h"].as_i64()),
            (Some(2), Some(2))
        );
        // 4x3 grid = 12 cells: 2 buttons + 7 fillers (the clamped tile
        // covers four cells in the visible grid)
        assert_eq!(basic["shortcuts"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn pro_variant_clamps_placements_to_the_board_grid() {
        let m = Mapper::new();
        let b = board(); // 6x5
        let buttons = [
            // stored row hangs off the right/bottom edge (5+3>6, 4+2>5)
            button("url", Some("https://x.co"), 5, 4, 3, 2),
            // a board shrink left this entirely off-grid; the old pro
            // filter dropped the tile outright
            button("url", Some("https://x.co"), 9, 0, 1, 1),
        ];
        let pro = m.board_payload(&b, &buttons, true);
        let sc = pro["shortcuts"].as_array().unwrap();
        let placed: Vec<(i64, i64, i64, i64)> = sc
            .iter()
            .filter(|s| !s["id"].is_null())
            .map(|s| {
                (
                    s["x"].as_i64().unwrap(),
                    s["y"].as_i64().unwrap(),
                    s["w"].as_i64().unwrap(),
                    s["h"].as_i64().unwrap(),
                )
            })
            .collect();
        // origin first pulled left/up, then the size trimmed: nothing
        // overhangs x=6 / y=5 any more (payload order is y then x)
        assert_eq!(placed, vec![(5, 0, 1, 1), (3, 3, 3, 2)]);
        // no filler is minted for a cell the clamped tiles now cover
        let fillers = sc.iter().filter(|s| s["id"].is_null()).count();
        assert_eq!(fillers, (6 * 5 - (3 * 2 + 1)) as usize);
    }

    #[test]
    fn command_transformation_rules() {
        let m = Mapper::new();
        // raw types pass through
        let s = m.shortcut_payload(&button("vol", Some("vol_mute"), 0, 0, 1, 1));
        assert_eq!(s["command"], "vol_mute");
        assert_eq!(s["extra"], "speaker-muted");
        assert_eq!(s["app"], "custom-value");
        // jsonKey extraction
        let s = m.shortcut_payload(&button(
            "speaker-device",
            Some(r#"{"speaker":"dev-1"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["command"], "dev-1");
        assert_eq!(s["extra"], "speaker-device");
        // everything else is blanked
        let s = m.shortcut_payload(&button("type", Some("hello"), 0, 0, 1, 1));
        assert_eq!(s["command"], "");
        // obs-source listener key
        let s = m.shortcut_payload(&button(
            "obs-source",
            Some(r#"{"scene":"S","source":"C","group":null}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "S::C");
    }

    #[test]
    fn oversized_board_dimensions_are_clamped_in_the_payload() {
        let m = Mapper::new();
        let mut b = board();
        b.width = 100;
        b.height = 100;
        let payload = m.board_payload(&b, &[], true);
        // the wire never sees dimensions past the cap, and the filler
        // loop is bounded by the clamped grid - never W*H of whatever
        // junk the DB row carried (audit C4)
        assert_eq!(payload["width"], 32);
        assert_eq!(payload["height"], 32);
        let shortcuts = payload["shortcuts"].as_array().unwrap();
        assert_eq!(
            shortcuts.len(),
            32 * 32,
            "fillers cover the clamped grid only"
        );
        // basic variant still crops to 4x3 from the clamped dims
        let basic = m.board_payload(&b, &[], false);
        assert_eq!(basic["width"], 32);
        assert_eq!(basic["shortcuts"].as_array().unwrap().len(), 4 * 3);
    }

    #[test]
    fn obs_tiles_watch_the_pushed_keys() {
        let m = Mapper::new();
        // scene tiles keep the original's raw-command watch string, and
        // the new toggles watch their kind name (the keys the native
        // integration pushes - see crates/obs/src/state.rs to_snapshot)
        let s = m.shortcut_payload(&button(
            "obs-scene",
            Some(r#"{"scene":"Game"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], r#"{"scene":"Game"}"#);
        for (kind, key) in [
            ("obs-studio-mode", "obs-studio-mode"),
            ("obs-record", "obs-record"),
            ("obs-stream", "obs-stream"),
        ] {
            let s = m.shortcut_payload(&button(kind, None, 0, 0, 1, 1));
            assert_eq!(s["extra"], key, "{kind}");
        }
        // source tiles compose scene::source (empty scene stays empty)
        let s = m.shortcut_payload(&button(
            "obs-source",
            Some(r#"{"source":"Webcam"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "::Webcam");
        // filters compose scene::source::filter with the optional source
        let s = m.shortcut_payload(&button(
            "obs-filter",
            Some(r#"{"filter":"Blur"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "::::Blur");
        let s = m.shortcut_payload(&button(
            "obs-filter",
            Some(r#"{"source":"Webcam","filter":"Blur"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "::Webcam::Blur");
        // device audio falls back from the empty `source` to the
        // catalog's `device` field (used to watch "" and never light)
        let s = m.shortcut_payload(&button(
            "obs-device-audio",
            Some(r#"{"device":"Mic"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "Mic");
        // the audio slider keeps the slider spelling: kind + raw command
        let mut slider = button(
            "obs-audio-slider",
            Some(r#"{"device":"Mic"}"#),
            0,
            0,
            1,
            1,
        );
        slider.mode = "slider".into();
        let s = m.shortcut_payload(&slider);
        assert_eq!(s["extra"], format!("obs-audio-slider_{}", r#"{"device":"Mic"}"#));
        // unimplemented obs kinds stay without a watch key
        let s = m.shortcut_payload(&button("obs-transition", None, 0, 0, 1, 1));
        assert_eq!(s["extra"], "");
    }

    #[test]
    fn spotify_tiles_watch_the_pushed_keys() {
        let m = Mapper::new();
        // the play command is a live toggle on the pushed key
        let s = m.shortcut_payload(&button("spotify-playback", Some("play"), 0, 0, 1, 1));
        assert_eq!(s["app"], "custom-value");
        assert_eq!(s["extra"], "spotify-playing");
        assert_eq!(s["color"], "#1DB954"); // per-command playback style
        assert!(!s["unicode"].as_str().unwrap().is_empty());
        // other playback commands stay plain buttons (existing behavior)
        let s = m.shortcut_payload(&button("spotify-playback", Some("next"), 0, 0, 1, 1));
        assert!(s.get("app").is_none());
        assert_eq!(s["extra"], "");
        // repeat/like remap onto their pushed keys, not the kind
        for (kind, key) in [("spotify-repeat", "spotify-repeat-on"), ("spotify-like", "spotify-liked")] {
            let s = m.shortcut_payload(&button(kind, None, 0, 0, 1, 1));
            assert_eq!(s["app"], "custom-value", "{kind}");
            assert_eq!(s["extra"], key, "{kind}");
            assert_eq!(s["toggle_key"], key, "{kind}");
            assert_eq!(s["color"], "#1db954", "{kind}");
        }
        // shuffle watches its own kind name
        let s = m.shortcut_payload(&button("spotify-shuffle", None, 0, 0, 1, 1));
        assert_eq!(s["extra"], "spotify-shuffle");
        // sliders: volume watches the kind, seek remaps to progress
        let mut vol = button("spotify-volume", None, 0, 0, 1, 1);
        vol.mode = "slider".into();
        let s = m.shortcut_payload(&vol);
        assert_eq!(s["extra"], "spotify-volume");
        let mut seek = button("spotify-seek", None, 0, 0, 1, 1);
        seek.mode = "slider".into();
        let s = m.shortcut_payload(&seek);
        assert_eq!(s["extra"], "spotify-progress");
        // device: jsonKey extraction like speaker-device
        let s = m.shortcut_payload(&button(
            "spotify-device",
            Some(r#"{"device":"Kitchen"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["command"], "Kitchen");
        assert_eq!(s["extra"], "spotify-device");
        // now-playing: status display tile, no toggle plumbing
        let mut np = button("spotify-now-playing", None, 0, 0, 1, 1);
        np.mode = "status".into();
        let s = m.shortcut_payload(&np);
        assert!(s.get("app").is_none());
        assert_eq!(s["extra"], "");
    }

    #[test]
    fn media_tiles_follow_the_spotify_shape() {
        let m = Mapper::new();
        // the seek slider reads the pushed progress fraction
        let mut seek = button("media-seek", None, 0, 0, 1, 1);
        seek.mode = "slider".into();
        let s = m.shortcut_payload(&seek);
        assert_eq!(s["extra"], "media-progress");
        // transport control: plain button, no state plumbing
        let s = m.shortcut_payload(&button("media-control", Some("play-pause"), 0, 0, 1, 1));
        assert_eq!(s["command"], "play-pause");
        assert!(s.get("app").is_none());
        assert_eq!(s["extra"], "");
        // now-playing: status display tile, payload rides the kind
        let mut np = button("media-now-playing", None, 0, 0, 2, 2);
        np.mode = "status".into();
        let s = m.shortcut_payload(&np);
        assert!(s.get("app").is_none());
        assert_eq!(s["extra"], "");
        // the original virtual-media-key kind is untouched
        let s = m.shortcut_payload(&button("vol", Some("play"), 0, 0, 1, 1));
        assert_eq!(s["extra"], "");
    }

    #[test]
    fn defaults_match_original() {
        let m = Mapper::new();
        let s = m.shortcut_payload(&button("key", Some("CTRL + K"), 0, 0, 1, 1));
        assert_eq!(s["color"], "#EF4836");
        assert_eq!(s["title_color"], "#ffffff");
        assert_eq!(s["mode"], "button");
        assert_eq!(s["action"], "");
        assert_eq!(s["prefix"], "fas");
        assert!(!s["unicode"].as_str().unwrap().is_empty());
    }
}
