//! DB rows -> protocol v2 boards. Style and watch-key resolution reuse the
//! legacy `Mapper` (single source of truth for those rules, tested against
//! the original client); this module reshapes its output into manifests,
//! derives state channels and converts legacy data-URL images into the
//! asset store.

use pulpit_db::{BoardRow, ButtonRow, MAX_BOARD_DIM};
use pulpit_legacy::{Backend, Mapper};
use pulpit_proto::{
    background_from_legacy, Board, Interaction, Placement, StateRef, StateShape, Style, Tile,
    WidgetKind, WidgetManifest,
};
use serde_json::Value;
use std::collections::HashMap;

use crate::assets::AssetStore;
use crate::state::{ext_channel, StateEngine};

/// All boards with their tiles, in legacy `order`.
pub fn build_boards(
    backend: &dyn Backend,
    assets: &AssetStore,
    engine: &StateEngine,
) -> Vec<Board> {
    // one grouped read for every board's shortcuts (was: one SELECT per
    // board on each rebuild)
    let buttons = backend.all_buttons_by_board();
    let names = board_names(backend);
    backend
        .get_boards()
        .iter()
        .map(|board| {
            let rows = buttons.get(&board.id);
            build_board(
                board,
                rows.map(Vec::as_slice).unwrap_or(&[]),
                &names,
                assets,
                engine,
            )
        })
        .collect()
}

/// id -> name of every board. Board-switch tiles with no title of their
/// own display the target's name; the desktop touch mode and the stock
/// client resolve it client-side from the command, but the v2 wire
/// carries no command, so the server resolves it here (MOB-09).
pub fn board_names(backend: &dyn Backend) -> HashMap<i64, String> {
    backend
        .get_boards()
        .into_iter()
        .map(|board| (board.id, board.name))
        .collect()
}

/// One board row plus its shortcuts -> one protocol board.
pub fn build_board(
    board: &BoardRow,
    buttons: &[ButtonRow],
    names: &HashMap<i64, String>,
    assets: &AssetStore,
    engine: &StateEngine,
) -> Board {
    Board {
        id: board.id,
        name: board.name.clone(),
        width: board.width.clamp(1, MAX_BOARD_DIM) as u32,
        height: board.height.clamp(1, MAX_BOARD_DIM) as u32,
        order: board.order.max(0) as u32,
        background: board_background(board, assets),
        tiles: buttons
            .iter()
            .map(|row| {
                let mut tile = build_tile(row, names, assets, engine);
                clamp_tile_to_board(&mut tile, board.width, board.height);
                tile
            })
            .collect(),
    }
}

/// Pull one built tile's placement back inside its board's grid (DESK-03).
/// Wire-side defense on top of the DB clamp in `update_board`: rows written
/// by older builds still carry off-grid placements, which the stock legacy
/// client filters out and a v2 grid would render off-canvas. Shared with
/// the `tile-set` op path so deltas obey the same bound.
pub fn clamp_tile_to_board(tile: &mut Tile, board_width: i64, board_height: i64) {
    let (x, y, w, h) = pulpit_db::clamp_placement(
        i64::from(tile.placement.x),
        i64::from(tile.placement.y),
        i64::from(tile.placement.w),
        i64::from(tile.placement.h),
        board_width,
        board_height,
    );
    tile.placement = Placement {
        x: x as u32,
        y: y as u32,
        w: w as u32,
        h: h as u32,
    };
}

fn board_background(board: &BoardRow, assets: &AssetStore) -> Option<pulpit_proto::Background> {
    // A board image (data URL in the legacy column) wins over the color,
    // mirroring how the original app renders the image over the background.
    if !board.image.is_empty() {
        if let Some(hash) = assets.import_data_url(&board.image) {
            return Some(pulpit_proto::Background::Asset { hash });
        }
    }
    background_from_legacy(&board.background)
}

/// The tile's options column parsed once (SWEEP-13): `build_tile` and the
/// per-event paths all want the same JSON, and a non-JSON column (the
/// legacy `windows:` dialect) reads as Null.
fn parse_options(row: &ButtonRow) -> Value {
    row.options
        .as_deref()
        .and_then(|o| serde_json::from_str(o).ok())
        .unwrap_or(Value::Null)
}

/// One row -> one tile. The legacy payload supplies resolved style fields
/// and the watch key (`extra`); state channels are namespaced `ext.<key>`
/// so pushes land on the channel the tile reads.
pub fn build_tile(
    row: &ButtonRow,
    names: &HashMap<i64, String>,
    assets: &AssetStore,
    engine: &StateEngine,
) -> Tile {
    let legacy = Mapper::new().shortcut_payload(row);
    let mut params = parse_options(row);
    let (kind, interactions) =
        widget_kind_for(row, legacy.get("app").and_then(Value::as_str), &params);
    let state = state_ref(row, &legacy, engine);
    let asset_hash = row
        .img
        .as_deref()
        .filter(|img| !img.is_empty())
        .and_then(|img| assets.import_data_url(img));
    // Dual-state img2: rare in existing DBs, but the editor can set it
    // and the desktop preview honors it, so carry it the same way.
    let asset_hash2 = row
        .img2
        .as_deref()
        .filter(|img| !img.is_empty())
        .and_then(|img| assets.import_data_url(img));
    apply_implicit_params(row, &mut params);

    Tile {
        id: row.id,
        placement: Placement {
            x: row.x.unwrap_or(0).max(0) as u32,
            y: row.y.unwrap_or(0).max(0) as u32,
            w: row.w.max(1) as u32,
            h: row.h.max(1) as u32,
        },
        manifest: WidgetManifest {
            kind,
            params,
            state,
            interactions,
            style: Some(style(row, &legacy, names)),
            web_package: None,
            asset_hash,
            asset_hash2,
        },
    }
}

/// Gestures a tile declares - the same set the manifest carries, derived
/// so the interaction handler can enforce it. Runs per interaction event
/// (taps, every slide tick), so it must not build the full legacy payload;
/// only the `app` marker feeds the kind decision.
pub fn allowed_interactions(row: &ButtonRow) -> Vec<Interaction> {
    let app = Mapper::new().app_value(row);
    widget_kind_for(row, app.as_deref(), &parse_options(row)).1
}

/// `params.hold.repeat: {delay_ms, interval_ms}` - the server-side
/// hold-to-repeat configuration. Both values are clamped to
/// 50..=60 000 ms (NET-06): an imported options JSON is free-form, and
/// an unbounded `interval_ms: 1` would run the tile's action ~1000x/s
/// for the whole 120 s hold cap, each tick on the blocking pool.
pub fn hold_repeat_config(params: &Value) -> Option<(u64, u64)> {
    const MIN_MS: u64 = 50;
    const MAX_MS: u64 = 60_000;
    let repeat = params.get("hold")?.get("repeat")?;
    let delay_ms = repeat.get("delay_ms")?.as_u64()?;
    let interval_ms = repeat.get("interval_ms")?.as_u64()?;
    // 0 stays "not configured" (the old semantic); anything in between
    // is pulled inside the bounds instead of trusted.
    (delay_ms > 0 && interval_ms > 0).then_some((
        delay_ms.clamp(MIN_MS, MAX_MS),
        interval_ms.clamp(MIN_MS, MAX_MS),
    ))
}

/// Legacy semantics that live outside the widget manifest get an
/// explicit params hint here, so clients never need to know legacy type
/// strings: the clock display tile announces itself as a clock widget,
/// and the plan tile's `windows:` option token is normalized into a
/// params array.
fn apply_implicit_params(row: &ButtonRow, params: &mut Value) {
    if pulpit_tools::is_tool_action(&row.kind) {
        if !params.is_object() {
            *params = Value::Object(serde_json::Map::new());
        }
        let obj = params.as_object_mut().expect("just made an object");
        obj.insert("widget".into(), Value::String(row.kind.clone()));
        if let Some(cmd_val) = row.command.as_deref().and_then(|c| serde_json::from_str::<Value>(c).ok()) {
            if let Some(cmd_obj) = cmd_val.as_object() {
                for (k, v) in cmd_obj {
                    obj.entry(k.clone()).or_insert(v.clone());
                }
            }
        }
    }
    if row.kind == "clock-display-time" {
        if !params.is_object() {
            *params = Value::Object(serde_json::Map::new());
        }
        let obj = params.as_object_mut().expect("just made an object");
        obj.insert("widget".into(), Value::String("clock".into()));
        let format = if row.command.as_deref() == Some("clock-12h") {
            "12h"
        } else {
            "24h"
        };
        obj.insert("clock_format".into(), Value::String(format.into()));
    }
    // The plan window filter rides the options column in a legacy
    // dialect ("windows:5h,week") that JSON parsing drops on the floor
    // (MOB-06): lift it into params.windows so v2 clients can filter
    // rows without knowing the dialect. A params key that is already
    // there wins.
    if let Some(windows) = windows_filter(row.options.as_deref()) {
        if !params.is_object() {
            *params = Value::Object(serde_json::Map::new());
        }
        let obj = params.as_object_mut().expect("just made an object");
        obj.entry("windows").or_insert(Value::Array(
            windows.into_iter().map(Value::String).collect(),
        ));
    }
}

/// The `windows:5h,week` token out of the legacy semicolon-separated
/// options dialect (the editor's plan-tile checkboxes; no token = both
/// windows shown). `Some(windows)` lists the windows the user kept;
/// `Some([])` means both were unticked, so every plan row hides.
pub fn windows_filter(options: Option<&str>) -> Option<Vec<String>> {
    let token = options?
        .split(';')
        .find(|part| part.starts_with("windows:"))?;
    Some(
        token["windows:".len()..]
            .split(',')
            .map(str::trim)
            .filter(|window| !window.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn widget_kind_for(
    row: &ButtonRow,
    app: Option<&str>,
    params: &Value,
) -> (WidgetKind, Vec<Interaction>) {
    let (kind, mut interactions) = match row.mode.as_str() {
        "slider" => (WidgetKind::Slider, vec![Interaction::Slide]),
        "knob" => (WidgetKind::Knob, vec![Interaction::Slide]),
        "graph" => (WidgetKind::Graph, vec![]),
        "list" => (WidgetKind::List, vec![]),
        // ai dev-work display tiles: a read-only row list, no gestures
        "status" => (WidgetKind::List, vec![]),
        _ if row.kind == "tool-clock" => (WidgetKind::Button, vec![]),
        _ if matches!(row.kind.as_str(), "tool-timer" | "tool-stopwatch") => (
            WidgetKind::Button,
            vec![Interaction::Tap, Interaction::LongPress, Interaction::DoubleTap],
        ),
        _ if row.kind == "tool-counter" => (
            WidgetKind::Button,
            vec![Interaction::Tap, Interaction::DoubleTap, Interaction::LongPress],
        ),
        _ if app == Some("custom-value") => (WidgetKind::Toggle, vec![Interaction::Tap]),
        _ => {
            // Press semantics are declared, not implied: key-style
            // commands act on touch down/up and configured holds need the
            // press pair for the server-side repeat; every other button
            // fires once on release.
            let press_pair = matches!(row.kind.as_str(), "key" | "advance-key")
                || hold_repeat_config(params).is_some();
            let interactions = if press_pair {
                vec![
                    Interaction::Tap,
                    Interaction::PressStart,
                    Interaction::PressEnd,
                ]
            } else {
                vec![Interaction::Tap]
            };
            (WidgetKind::Button, interactions)
        }
    };
    // M5 custom gestures (`{"gestures": [...]}` in the options JSON):
    // alternative triggers of the tile's action, on top of the kind's
    // defaults. Slider/knob tiles keep the drag surface for the value
    // only. Deduped so a hand-edited `["tap"]` cannot double-declare.
    if !matches!(row.mode.as_str(), "slider" | "knob") {
        for gesture in declared_gestures(params) {
            if !interactions.contains(&gesture) {
                interactions.push(gesture);
            }
        }
    }
    (kind, interactions)
}

/// Gestures a tile declares in its options JSON
/// (`{"gestures": ["long-press", "swipe-left"]}`). The set is closed to
/// the four names the clients implement; unknown names are dropped so a
/// hand-edited file cannot smuggle arbitrary interaction labels.
pub fn declared_gestures(params: &Value) -> Vec<Interaction> {
    const KNOWN: &[(&str, Interaction)] = &[
        ("long-press", Interaction::LongPress),
        ("double-tap", Interaction::DoubleTap),
        ("swipe-left", Interaction::SwipeLeft),
        ("swipe-right", Interaction::SwipeRight),
    ];
    let Some(list) = params.get("gestures").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(Value::as_str)
        .filter_map(|name| {
            KNOWN
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, interaction)| *interaction)
        })
        .collect()
}

/// The action a declared gesture runs: `gesture_actions` in the options
/// JSON (`{"gesture_actions": {"long-press": {"type": "media",
/// "command": "play"}}}`) overrides the tile's own action per gesture.
/// Returns the `(kind, command)` pair to execute - `None` for the command
/// half keeps the tile's own command, matching the per-field override the
/// editor writes. `None` overall when the gesture has no override and the
/// tile's own action applies (the M5 default). Shared by the v2
/// interaction handler and the desktop touch-mode command so a gesture
/// cannot diverge between the tablet and the editor's own screen.
pub fn gesture_action_override(params: &Value, gesture: &str) -> Option<(String, Option<String>)> {
    let action = params
        .get("gesture_actions")
        .and_then(|actions| actions.get(gesture))?;
    let kind = action.get("type").and_then(Value::as_str)?;
    let command = match action.get("command") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) if !other.is_null() => Some(other.to_string()),
        _ => None,
    };
    Some((kind.to_string(), command))
}

/// State channel + shape, registered with the engine as a side effect so
/// the channel shows up in `welcome` even before the first push. The
/// legacy `extra` listener returns an empty key for display-mode tiles
/// (status): the ai-dev producers push under the tile's type, mirroring
/// the editor's `command || type` read, so fall back to the kind there.
fn state_ref(row: &ButtonRow, legacy: &Value, engine: &StateEngine) -> Option<StateRef> {
    let mut key = legacy
        .get("extra")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if key.is_empty() && row.mode == "status" {
        key = row.kind.clone();
    }
    if key.is_empty() && matches!(row.kind.as_str(), "tool-timer" | "tool-stopwatch" | "tool-counter") {
        key = format!("tool-{}", row.id);
    }
    if key.is_empty() {
        return None;
    }
    let shape = if row.mode == "graph" {
        StateShape::Series
    } else {
        StateShape::Scalar
    };
    let channel = ext_channel(&key);
    engine.register(&channel, shape, None);
    Some(StateRef { channel, shape })
}

fn style(row: &ButtonRow, legacy: &Value, names: &HashMap<i64, String>) -> Style {
    let color = legacy.get("color").and_then(Value::as_str);
    let unicode = legacy.get("unicode").and_then(Value::as_str);
    let unicode2 = legacy.get("unicode2").and_then(Value::as_str);
    let prefix = legacy.get("prefix").and_then(Value::as_str);
    Style {
        color: non_empty(color),
        // Unset color2 falls back to the TYPE default (the same chain the
        // legacy mapper resolves via the style table), not to the tile's
        // own resting color: both wires must show the same active-state
        // color, and the §4 client fallback stays a last resort for
        // types without a default.
        color2: non_empty(legacy.get("color2").and_then(Value::as_str)),
        icon: non_empty(unicode),
        icon2: non_empty(unicode2),
        icon_family: non_empty(prefix),
        title: tile_title(row, names),
        // Legacy shape column is an int (0 = default); pass non-defaults
        // through so the client can render them.
        shape: (row.shape != 0).then(|| row.shape.to_string()),
        // The editor edits these against the DB columns; legacy tablets
        // read the same values out of the mapper payload (parity oracle:
        // crates/v2/tests/parity.rs).
        border_color: non_empty(row.border_color.as_deref()),
        border_color2: non_empty(row.border_color2.as_deref()),
        icon_color: non_empty(row.icon_color.as_deref()),
        icon_color2: non_empty(row.icon_color2.as_deref()),
        title_color: non_empty(row.title_color.as_deref()),
        title_color2: non_empty(row.title_color2.as_deref()),
        // Title pinning + box color and the active-state shape: edited
        // against the same DB columns the legacy mapper reads (parity
        // oracle: crates/v2/tests/parity.rs). Zero is the legacy default
        // for positions/shapes and stays off the wire; the §4 per-field
        // state-2 fallback covers the rest client-side.
        title_position: non_zero_u8(row.title_position),
        title_position2: non_zero_u8(row.title_position2),
        title_box_color: non_empty(row.title_box_color.as_deref()),
        title_box_color2: non_empty(row.title_box_color2.as_deref()),
        shape2: (row.shape2 != 0).then(|| row.shape2.to_string()),
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|v| !v.is_empty()).map(str::to_string)
}

/// The tile's title: the row's own title, else (for board-switch tiles)
/// the target board's name. The legacy wire leaves the title empty and
/// lets the client resolve the name from the command; the v2 wire
/// carries no command, so the server resolves it (MOB-09). An unknown
/// target (deleted board) or junk command yields no injected title.
fn tile_title(row: &ButtonRow, names: &HashMap<i64, String>) -> Option<String> {
    if let Some(title) = non_empty(row.title.as_deref()) {
        return Some(title);
    }
    if row.kind != "board" {
        return None;
    }
    row.command
        .as_deref()
        .and_then(|c| serde_json::from_str::<Value>(c).ok())
        .and_then(|cmd| cmd.get("id").and_then(Value::as_i64))
        .and_then(|id| names.get(&id))
        .filter(|name| !name.is_empty())
        .cloned()
}

/// Legacy int column -> optional wire number, dropping the 0 default
/// (and junk that does not fit a u8).
fn non_zero_u8(value: i64) -> Option<u8> {
    u8::try_from(value).ok().filter(|v| *v != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateEngine;

    fn row(kind: &str, mode: &str, command: Option<&str>) -> ButtonRow {
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
            mode: mode.into(),
            x: Some(2),
            y: Some(1),
            w: 2,
            h: 1,
            options: None,
        }
    }

    #[test]
    fn allowed_interactions_match_the_full_mapper_derivation() {
        // oracle: the same rows through the full legacy payload (what
        // build_tile does) must agree with the cheap per-event path
        for (kind, mode, command) in [
            ("vol", "button", Some("vol_mute")),
            ("vol", "button", Some("vol_up")),
            ("url", "button", Some("https://example.com")),
            ("volume", "slider", None),
            ("ai-tokens-hour", "graph", None),
        ] {
            let r = row(kind, mode, command);
            let legacy = Mapper::new().shortcut_payload(&r);
            assert_eq!(
                allowed_interactions(&r),
                widget_kind_for(
                    &r,
                    legacy.get("app").and_then(Value::as_str),
                    &parse_options(&r)
                )
                .1,
                "{kind}/{mode}/{command:?}"
            );
        }
    }

    #[test]
    fn declared_gestures_extend_the_interactions_of_a_button() {
        let mut r = row("vol", "button", Some("vol_mute"));
        r.options = Some(r#"{"gestures": ["long-press", "swipe-left", "not-a-gesture"]}"#.into());
        let set = allowed_interactions(&r);
        assert!(set.contains(&Interaction::LongPress));
        assert!(set.contains(&Interaction::SwipeLeft));
        // unknown names are dropped, tap stays the base trigger
        assert!(!set.contains(&Interaction::Other));
        assert_eq!(set.first(), Some(&Interaction::Tap));
        // the manifest the client sees (build_tile) must equal the
        // per-event gate, or a client could send a gesture the server
        // answers with UNSUPPORTED_INTERACTION
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        assert_eq!(tile.manifest.interactions, set);
    }

    #[test]
    fn sliders_do_not_take_declared_gestures() {
        // the drag surface belongs to the value on slider/knob tiles
        let mut r = row("volume", "slider", None);
        r.options = Some(r#"{"gestures": ["long-press"]}"#.into());
        assert_eq!(allowed_interactions(&r), vec![Interaction::Slide]);
    }

    #[test]
    fn gesture_action_override_reads_the_editor_shape() {
        let params: Value = serde_json::from_str(
            r#"{"gesture_actions": {
                "long-press": {"type": "media", "command": "play"},
                "double-tap": {"type": "key", "command": ""},
                "swipe-left": {"type": "vol"}
            }}"#,
        )
        .unwrap();
        let (kind, command) = gesture_action_override(&params, "long-press").unwrap();
        assert_eq!(kind, "media");
        assert_eq!(command.as_deref(), Some("play"));
        // an explicit empty command replaces the tile's own (a key
        // override with no combo must not leak the tile's key)
        let (kind, command) = gesture_action_override(&params, "double-tap").unwrap();
        assert_eq!(kind, "key");
        assert_eq!(command.as_deref(), Some(""));
        // no command key: the tile's own command applies
        let (kind, command) = gesture_action_override(&params, "swipe-left").unwrap();
        assert_eq!(kind, "vol");
        assert_eq!(command, None);
        // a gesture without an override falls back to the tile's action
        assert_eq!(gesture_action_override(&params, "swipe-right"), None);
    }

    #[test]
    fn gesture_action_override_ignores_broken_options() {
        // plain-string program arguments and empty JSON are not an
        // override source: everything falls back to the tile's action
        for params in [
            Value::Null,
            Value::String("--flag".into()),
            serde_json::json!({ "windows": "5h" }),
            serde_json::json!({ "gesture_actions": {} }),
            serde_json::json!({ "gesture_actions": { "long-press": {"command": "x"} } }),
        ] {
            assert_eq!(gesture_action_override(&params, "long-press"), None);
        }
        // a non-string command keeps its JSON serialization, the way
        // numbers land in stored commands
        let params = serde_json::json!({ "gesture_actions": { "double-tap": {"type": "vol", "command": 5} } });
        let (kind, command) = gesture_action_override(&params, "double-tap").unwrap();
        assert_eq!(kind, "vol");
        assert_eq!(command.as_deref(), Some("5"));
    }

    #[test]
    fn tool_tiles_carry_widget_hint_config_and_state_channel() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row(
                "tool-timer",
                "button",
                Some(r#"{"duration":"05:00","finish_action":"play","sound_path":"alert.mp3"}"#),
            ),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(tile.manifest.kind, WidgetKind::Button);
        // the config travels as params so clients never parse legacy
        // command JSON, and the widget hint names the tool kind
        assert_eq!(tile.manifest.params["widget"], "tool-timer");
        assert_eq!(tile.manifest.params["duration"], "05:00");
        assert_eq!(tile.manifest.params["finish_action"], "play");
        // state channel: the per-id key the manager pushes compact state
        // under (the row helper's button id is 10)
        assert_eq!(
            tile.manifest.state.as_ref().unwrap().channel,
            "ext.tool-10"
        );
        // gestures: tap starts/pauses, long-press/double-tap reset
        assert_eq!(
            tile.manifest.interactions,
            vec![Interaction::Tap, Interaction::LongPress, Interaction::DoubleTap]
        );

        // the counter swaps the gesture order (+1 tap, -1 double-tap,
        // reset long-press); the clock renders client-side and takes no
        // interactions at all
        assert_eq!(
            allowed_interactions(&row("tool-counter", "button", None)),
            vec![Interaction::Tap, Interaction::DoubleTap, Interaction::LongPress]
        );
        let clock = build_tile(
            &row("tool-clock", "button", None),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(clock.manifest.params["widget"], "tool-clock");
        assert!(clock.manifest.interactions.is_empty());
    }

    #[test]
    fn vol_mute_maps_to_toggle_with_channel() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("vol", "button", Some("vol_mute")),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(
            tile.placement,
            Placement {
                x: 2,
                y: 1,
                w: 2,
                h: 1
            }
        );
        // The legacy quirk (app=custom-value, extra=speaker-muted) is
        // exactly the v2 Toggle: two states driven by a live channel.
        assert_eq!(tile.manifest.kind, WidgetKind::Toggle);
        assert_eq!(tile.manifest.interactions, vec![Interaction::Tap]);
        assert_eq!(
            tile.manifest.style.as_ref().unwrap().color.as_deref(),
            Some("#F5AB35") // vol resolves to the playback color
        );
        let state = tile.manifest.state.unwrap();
        assert_eq!(state.channel, "ext.speaker-muted");
        assert_eq!(state.shape, StateShape::Scalar);
        assert_eq!(
            engine.catalog()["ext.speaker-muted"].shape,
            StateShape::Scalar
        );
    }

    #[test]
    fn run_command_keeps_its_extension_style_on_both_wire_builders() {
        // The declaration as deckboard-commands ships it (extracted from
        // the package's metadata): the package stays loaded for its input
        // declarations even though the press itself is native now, and
        // this test pins the style those declarations must produce.
        pulpit_legacy::props::register_extension_input(pulpit_legacy::props::ExtInput {
            value: "run-command".into(),
            icon: Some("terminal".into()),
            color: Some("#34495e".into()),
            font_icon: None,
            mode: None,
            command: None,
        });
        let r = row("run-command", "button", Some(r#"{"commandAction":"calc.exe"}"#));

        // Legacy payload (stock Deckboard tablets): color/icon resolved
        // through the extension-input registry, like getExtensionButton.
        let legacy = Mapper::new().shortcut_payload(&r);
        assert_eq!(legacy["color"], "#34495e");
        assert_eq!(legacy["prefix"], "fas");
        let unicode = legacy["unicode"].as_str().unwrap();
        let glyph: Vec<u32> = unicode.chars().map(|c| c as u32).collect();
        assert_eq!(glyph, vec![0xf120], "the terminal glyph (fas f120)");

        // v2 manifest: style is derived from that same legacy payload.
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        let style = tile.manifest.style.as_ref().unwrap();
        assert_eq!(style.color.as_deref(), Some("#34495e"));
        assert_eq!(style.icon_family.as_deref(), Some("fas"));
        let icon = style.icon.as_deref().expect("terminal icon carried");
        assert_eq!(
            icon.chars().map(|c| c as u32).collect::<Vec<_>>(),
            vec![0xf120]
        );
    }

    #[test]
    fn plain_button_press_interactions() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("key", "button", Some("CTRL + K")),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(tile.manifest.kind, WidgetKind::Button);
        assert_eq!(
            tile.manifest.interactions,
            vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd
            ]
        );
        assert!(tile.manifest.state.is_none());
    }

    #[test]
    fn http_request_tile_is_a_plain_button_on_the_v2_wire() {
        // the config lives in the command column (server-side execution);
        // the client needs no kind-specific treatment, just the tap
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row(
                "http-request",
                "button",
                Some(r#"{"method":"POST","url":"http://ha.local/api"}"#),
            ),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(tile.manifest.kind, WidgetKind::Button);
        assert_eq!(tile.manifest.interactions, vec![Interaction::Tap]);
        assert!(tile.manifest.state.is_none());
    }

    #[test]
    fn graph_mode_becomes_series_channel() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("si-cpu", "graph", Some("cpu-key")),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(tile.manifest.kind, WidgetKind::Graph);
        let state = tile.manifest.state.unwrap();
        assert_eq!(state.channel, "ext.cpu-key"); // raw command wins when set
        assert_eq!(state.shape, StateShape::Series);
        assert_eq!(engine.catalog()["ext.cpu-key"].cap, Some(120));
    }

    #[test]
    fn key_and_hold_buttons_declare_press_pair() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("key", "button", Some("A")),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(
            tile.manifest.interactions,
            vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd
            ]
        );
        let mut r = row("vol", "button", Some("vol_down"));
        r.options = Some(r#"{"hold":{"repeat":{"delay_ms":400,"interval_ms":120}}}"#.into());
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        assert!(tile
            .manifest
            .interactions
            .contains(&Interaction::PressStart));
    }

    #[test]
    fn clock_tile_announces_itself_in_params() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let mut r = row("clock-display-time", "button", Some("clock-12h"));
        r.title = None;
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        let params = tile.manifest.params;
        assert_eq!(params["widget"], "clock");
        assert_eq!(params["clock_format"], "12h");
    }

    #[test]
    fn plan_windows_token_reaches_params_as_an_array() {
        // the legacy "windows:5h,week" options dialect is not JSON, so a
        // plain parse drops it; it must be lifted into params (MOB-06)
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let mut r = row("ai-plan-limits", "status", None);
        r.options = Some("windows: 5h, week".into());
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        assert_eq!(
            tile.manifest.params["windows"],
            serde_json::json!(["5h", "week"])
        );

        // no token = no params.windows key (both windows shown)
        let mut r = row("ai-plan-limits", "status", None);
        r.options = Some("{}".into());
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        assert!(tile.manifest.params.get("windows").is_none());

        // token present with everything unticked = empty filter, still
        // on the wire (hides every plan row)
        assert_eq!(
            windows_filter(Some("other:x;windows:")),
            Some(Vec::<String>::new())
        );
        assert_eq!(windows_filter(None), None);
        assert_eq!(windows_filter(Some("windows:5h")), Some(vec!["5h".into()]));
        // the token must start a part (a "xwindows:" mid-part is not one)
        assert_eq!(windows_filter(Some("xwindows:5h")), None);
    }

    #[test]
    fn style_carries_active_state_and_font_family() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let mut r = row("toggle-headphone", "button", None);
        r.icon = Some("headphones".into());
        r.icon2 = Some("deaf".into());
        r.color2 = Some("#ED4245".into());
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        let style = tile.manifest.style.unwrap();
        assert_eq!(style.icon_family.as_deref(), Some("fas"));
        assert!(
            style.icon2.is_some(),
            "icon2 resolved from the icon2 column"
        );
        assert_eq!(style.color2.as_deref(), Some("#ED4245"));
    }

    #[test]
    fn slider_and_custom_value_kinds() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("speaker-volume", "slider", None),
            &HashMap::new(),
            &assets,
            &engine,
        );
        assert_eq!(tile.manifest.kind, WidgetKind::Slider);
        assert_eq!(tile.manifest.interactions, vec![Interaction::Slide]);
        assert_eq!(tile.manifest.state.unwrap().channel, "ext.speaker-volume");

        let tile = build_tile(
            &row("toggle-microphone", "button", None),
            &HashMap::new(),
            &assets,
            &engine,
        );
        // toggle-microphone resolves through the discord extension input
        // (custom-value app), but without that input registered it stays
        // a button - extension inputs are registered by the host.
        assert_eq!(tile.manifest.kind, WidgetKind::Button);
    }

    #[test]
    fn image_data_url_becomes_asset_hash() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let mut r = row("url", "button", Some("https://x.co"));
        let png = format!("data:image/png;base64,{}", use_base64(b"img-bytes"));
        r.img = Some(png);
        let tile = build_tile(&r, &HashMap::new(), &assets, &engine);
        assert!(tile.manifest.asset_hash.is_some());
        assert_eq!(
            assets
                .get(tile.manifest.asset_hash.as_ref().unwrap())
                .unwrap(),
            b"img-bytes"
        );
    }

    #[test]
    fn status_display_tiles_watch_the_type_channel() {
        // the ai-dev producers push under the tile type; a status tile
        // with no command must still get its channel declared
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(
            &row("ai-plan-limits", "status", Some("")),
            &HashMap::new(),
            &assets,
            &engine,
        );
        let state = tile.manifest.state.expect("type channel for status mode");
        assert_eq!(state.channel, "ext.ai-plan-limits");
        assert_eq!(state.shape, StateShape::Scalar);
        assert_eq!(
            engine.catalog()["ext.ai-plan-limits"].shape,
            StateShape::Scalar
        );
    }

    #[test]
    fn oversized_board_dimensions_are_clamped_on_the_wire() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let b = board_row(1_000_000, 1_000_000);
        let board = build_board(&b, &[], &HashMap::new(), &assets, &engine);
        // v2 clients lay out a W*H grid from these numbers: a junk row
        // must not reach them at full size (audit C4)
        assert_eq!(board.width, 32);
        assert_eq!(board.height, 32);
        let degenerate = build_board(&board_row(-5, 0), &[], &HashMap::new(), &assets, &engine);
        assert_eq!((degenerate.width, degenerate.height), (1, 1));
    }

    #[test]
    fn tile_placements_are_clamped_to_the_board_grid() {
        // DESK-03: rows stored before a board shrink still reach the
        // wire; the stock legacy client drops an off-grid tile outright
        // and a v2 grid would render it off-canvas
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let b = board_row(4, 3);
        let mut hangs = row("url", "button", Some("https://example.com"));
        hangs.x = Some(5);
        hangs.y = Some(3);
        let mut huge = row("url", "button", Some("https://example.com"));
        huge.id = 11;
        huge.x = Some(2);
        huge.y = Some(2);
        huge.w = 20;
        huge.h = 2;
        let board = build_board(&b, &[hangs, huge], &HashMap::new(), &assets, &engine);
        // origin pulled back inside (x <= W-w, y <= H-h)
        assert_eq!(
            board.tiles[0].placement,
            Placement {
                x: 2,
                y: 2,
                w: 2,
                h: 1
            }
        );
        // a tile larger than the grid shrinks to it
        assert_eq!(
            board.tiles[1].placement,
            Placement {
                x: 0,
                y: 1,
                w: 4,
                h: 2
            }
        );
    }

    #[test]
    fn build_boards_labels_untitled_board_switch_tiles() {
        // the production snapshot path: an untitled `board` tile on the
        // home board must carry the target board's name in style.title
        // (MOB-09 - the v2 wire carries no command for clients to
        // resolve it themselves)
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let mut switch = row("board", "button", Some(r#"{"id": 2}"#));
        switch.title = None;
        switch.board_id = 7;
        let mut home = board_row(4, 3);
        home.id = 7;
        home.name = "Home".into();
        let mut media = board_row(2, 2);
        media.id = 2;
        media.name = "Media".into();
        let backend = NamesBackend {
            boards: vec![home, media],
            buttons: vec![switch],
        };
        let boards = build_boards(&backend, &assets, &engine);
        assert_eq!(boards[0].tiles.len(), 1);
        let style = boards[0].tiles[0].manifest.style.as_ref().unwrap();
        assert_eq!(style.title.as_deref(), Some("Media"));
    }

    /// Minimal Backend for the board-name wiring test: boards plus their
    /// rows, everything else inert.
    struct NamesBackend {
        boards: Vec<pulpit_db::BoardRow>,
        buttons: Vec<ButtonRow>,
    }

    impl Backend for NamesBackend {
        fn get_boards(&self) -> Vec<pulpit_db::BoardRow> {
            self.boards.clone()
        }
        fn get_board(&self, board_id: i64) -> Option<pulpit_db::BoardRow> {
            self.boards.iter().find(|b| b.id == board_id).cloned()
        }
        fn get_buttons_by_board(&self, board_id: i64) -> Vec<ButtonRow> {
            self.buttons
                .iter()
                .filter(|b| b.board_id == board_id)
                .cloned()
                .collect()
        }
        fn get_button(&self, _id: i64) -> Option<ButtonRow> {
            None
        }
        fn exec(&self, _button: ButtonRow, _tap_start: bool, _sink: &mut dyn pulpit_actions::EventSink) {}
        fn slider(&self, _button: ButtonRow, _value: f64) {}
    }

    fn board_row(width: i64, height: i64) -> pulpit_db::BoardRow {
        pulpit_db::BoardRow {
            id: 1,
            name: "Big".into(),
            background: "#2c3e50".into(),
            layout: 6,
            image: String::new(),
            sort: 0,
            kind: "buttons".into(),
            args: None,
            order: 0,
            width,
            height,
            converted: 1,
        }
    }

    fn asset_store() -> (AssetStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (AssetStore::open(dir.path().to_path_buf()).unwrap(), dir)
    }

    fn use_base64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}
