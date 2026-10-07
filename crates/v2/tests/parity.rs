//! Wire parity (audit E1): the same `ButtonRow` pushed through the
//! legacy mapper (`crates/legacy` - contractual field names the stock
//! Deckboard client renders) and the v2 board builder (`crates/v2`)
//! must agree on every field both surfaces carry: press modes, dual
//! states, colors, icons, hold config. The desktop editor reads only
//! its own state, so drift here would be invisible until a tablet
//! renders something the editor never showed. Required before any C5
//! styling work touches either builder.

use pulpit_db::ButtonRow;
use pulpit_legacy::Mapper;
use pulpit_proto::{Interaction, WidgetKind};
use pulpit_v2::boards::{build_tile, hold_repeat_config};
use pulpit_v2::{AssetBody, AssetStore, StateEngine};
use std::collections::HashMap;

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
        x: Some(0),
        y: Some(0),
        w: 1,
        h: 1,
        options: None,
    }
}

/// The same row through both wire builders.
fn both(row: &ButtonRow) -> (serde_json::Value, pulpit_proto::Tile) {
    both_with(row, &HashMap::new())
}

/// `both` with a board-id -> name map for board-switch tile titles.
fn both_with(
    row: &ButtonRow,
    names: &HashMap<i64, String>,
) -> (serde_json::Value, pulpit_proto::Tile) {
    let legacy = Mapper::new().shortcut_payload(row);
    let dir = tempfile::tempdir().unwrap();
    let assets = AssetStore::open(dir.path().to_path_buf()).unwrap();
    let tile = build_tile(row, names, &assets, &StateEngine::new(120));
    (legacy, tile)
}

fn style_of(tile: &pulpit_proto::Tile) -> &pulpit_proto::Style {
    tile.manifest
        .style
        .as_ref()
        .expect("v2 tiles always carry style")
}

#[test]
fn colors_icons_and_title_agree_between_the_wires() {
    // a row with every display field explicitly set
    let mut r = row("url", "button", Some("https://x.co"));
    r.color = Some("#123456".into());
    r.color2 = Some("#654321".into());
    r.icon = Some("play".into());
    r.icon2 = Some("pause".into());
    r.title = Some(" Both wires ".into());
    let (legacy, tile) = both(&r);
    let style = style_of(&tile);

    // primary + dual-state colors
    assert_eq!(style.color.as_deref(), legacy["color"].as_str());
    assert_eq!(style.color2.as_deref(), legacy["color2"].as_str());
    // icons: unicode/unicode2/prefix
    assert_eq!(style.icon.as_deref(), legacy["unicode"].as_str());
    assert_eq!(style.icon2.as_deref(), legacy["unicode2"].as_str());
    assert_eq!(style.icon_family.as_deref(), legacy["prefix"].as_str());
    assert_eq!(style.title.as_deref(), legacy["title"].as_str());

    // unset dual-state fields: legacy fills its own defaults (the
    // client-side fallback); v2 mirrors color2's resolved default (the
    // type color, NET-03) and leaves genuinely absent pairs to the §4
    // client-side per-field fallback - pinned here so the divergence
    // stays deliberate. clock-display-time is the one basic type
    // without a default color, so its unset color2 is empty on both.
    let mut r = row("clock-display-time", "button", None);
    r.color2 = None;
    let (legacy, tile) = both(&r);
    let style = style_of(&tile);
    assert_eq!(style.color.as_deref(), legacy["color"].as_str());
    assert_eq!(legacy["color2"].as_str(), Some(""));
    assert!(
        style.color2.is_none(),
        "no type default -> absent on the wire"
    );
}

#[test]
fn color2_falls_back_to_the_type_default_like_legacy() {
    // NET-03: a type with a default color (vol), a custom resting color
    // and no explicit color2 - the legacy wire carries the type default
    // while active, and so must v2 (not the tile's own resting color).
    let mut r = row("vol", "button", Some("vol_up"));
    r.color = Some("#123456".into());
    r.color2 = None;
    let (legacy, tile) = both(&r);
    let style = style_of(&tile);
    assert_eq!(
        legacy["color2"].as_str().unwrap_or_default(),
        "#F5AB35",
        "legacy fills the vol type default"
    );
    assert_eq!(
        style.color2.as_deref(),
        legacy["color2"].as_str(),
        "v2 mirrors the legacy-resolved color2"
    );
    assert_ne!(
        style.color2.as_deref(),
        Some("#123456"),
        "the resting color is not the active-state fallback"
    );
}

#[test]
fn press_modes_follow_the_legacy_kind_and_hold_config() {
    struct Case {
        row: ButtonRow,
        expected: Vec<Interaction>,
        note: &'static str,
    }
    let hold = r#"{"hold":{"repeat":{"delay_ms":400,"interval_ms":120}}}"#;
    let cases = [
        Case {
            row: row("url", "button", Some("https://x.co")),
            expected: vec![Interaction::Tap],
            note: "plain button fires on release",
        },
        Case {
            row: row("key", "button", Some("A")),
            expected: vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd,
            ],
            note: "key tiles act on touch down/up",
        },
        Case {
            row: row("advance-key", "button", Some("SHIFT")),
            expected: vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd,
            ],
            note: "advance-key behaves like key",
        },
        Case {
            row: {
                let mut r = row("vol", "button", Some("vol_down"));
                r.options = Some(hold.into());
                r
            },
            expected: vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd,
            ],
            note: "hold-repeat config declares the press pair",
        },
        Case {
            row: row("volume", "slider", None),
            expected: vec![Interaction::Slide],
            note: "slider mode",
        },
        Case {
            row: row("volume", "knob", None),
            expected: vec![Interaction::Slide],
            note: "knob mode",
        },
        Case {
            row: row("si-cpu", "graph", None),
            expected: vec![],
            note: "graph is read-only",
        },
    ];
    for case in cases {
        let (legacy, tile) = both(&case.row);
        assert_eq!(
            tile.manifest.interactions, case.expected,
            "{} (legacy kind {}, mode {})",
            case.note, legacy["type"], legacy["mode"]
        );
    }
}

#[test]
fn dual_state_semantics_agree() {
    // vol_mute is the canonical dual-state tile: the legacy payload
    // marks it app=custom-value watching "speaker-muted"; v2 must call
    // it a Toggle reading the matching ext channel.
    let (legacy, tile) = both(&row("vol", "button", Some("vol_mute")));
    assert_eq!(legacy["app"], "custom-value");
    assert_eq!(legacy["extra"], "speaker-muted");
    assert_eq!(tile.manifest.kind, WidgetKind::Toggle);
    let state = tile
        .manifest
        .state
        .as_ref()
        .expect("toggle watches a channel");
    assert_eq!(
        state.channel,
        format!("ext.{}", legacy["extra"].as_str().unwrap())
    );
    assert_eq!(tile.manifest.interactions, vec![Interaction::Tap]);

    // a plain button is not dual-state on either wire
    let (legacy, tile) = both(&row("url", "button", Some("https://x.co")));
    assert!(legacy.get("app").is_none());
    assert_eq!(tile.manifest.kind, WidgetKind::Button);
    assert!(tile.manifest.state.is_none());

    // graph mode: legacy watches `extra` as a series, v2 shapes it
    let (legacy, tile) = both(&row("si-cpu", "graph", None));
    assert_eq!(legacy["mode"], "graph");
    assert_eq!(tile.manifest.kind, WidgetKind::Graph);
    let state = tile.manifest.state.as_ref().unwrap();
    assert_eq!(
        state.channel,
        format!("ext.{}", legacy["extra"].as_str().unwrap())
    );
    assert_eq!(state.shape, pulpit_proto::StateShape::Series);
}

#[test]
fn hold_repeat_config_parses_identically_on_both_wires() {
    // The hold block lives in the row options; the legacy payload
    // carries the same options string verbatim while v2 re-parses it
    // into manifest params. The repeat the server would run must be
    // the same through either reading.
    let mut r = row("vol", "button", Some("vol_down"));
    r.options = Some(r#"{"hold":{"repeat":{"delay_ms":400,"interval_ms":120}}}"#.into());
    let (legacy, tile) = both(&r);

    let legacy_options: serde_json::Value =
        serde_json::from_str(legacy["options"].as_str().unwrap()).unwrap();
    assert_eq!(
        hold_repeat_config(&legacy_options),
        Some((400, 120)),
        "legacy options parse"
    );
    assert_eq!(
        hold_repeat_config(&tile.manifest.params),
        Some((400, 120)),
        "v2 params parse to the same repeat"
    );

    // and a config both wires reject (zero interval is not a repeat)
    let mut r = row("vol", "button", Some("vol_down"));
    r.options = Some(r#"{"hold":{"repeat":{"delay_ms":0,"interval_ms":0}}}"#.into());
    let (legacy, tile) = both(&r);
    let legacy_options: serde_json::Value =
        serde_json::from_str(legacy["options"].as_str().unwrap()).unwrap();
    assert_eq!(hold_repeat_config(&legacy_options), None);
    assert_eq!(hold_repeat_config(&tile.manifest.params), None);
}

// ---- Wave-2 C5 additions (state-2 image + new optional style fields) ----

fn styled_row() -> ButtonRow {
    let png1 = format!("data:image/png;base64,{}", b64(b"face"));
    let png2 = format!("data:image/png;base64,{}", b64(b"face-active"));
    ButtonRow {
        id: 17,
        board_id: 3,
        kind: "toggle-headphone".into(),
        command: None,
        title: Some("Deafen".into()),
        title_position: 2,
        title_color: Some("#ffcc00".into()),
        title_box_color: Some("#1c1c1c".into()),
        color: Some("#5865f2".into()),
        icon_color: Some("#ffe0e0".into()),
        icon_color2: Some("#1db954".into()),
        border_color: Some("#101010".into()),
        shape: 1,
        icon: Some("headphones".into()),
        img: Some(png1),
        img2: Some(png2),
        icon2: Some("deaf".into()),
        color2: Some("#ED4245".into()),
        shape2: 1,
        border_color2: Some("#f0f0f0".into()),
        title_position2: 1,
        title_box_color2: Some("#2c2c2c".into()),
        title_color2: Some("#00ffcc".into()),
        position: None,
        position2: 0,
        mode: "button".into(),
        x: Some(0),
        y: Some(0),
        w: 1,
        h: 1,
        options: None,
    }
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn shared_style_fields_agree_between_legacy_and_v2() {
    let row = styled_row();
    let legacy = Mapper::new().shortcut_payload(&row);
    let (assets, _dir) = store();
    let engine = StateEngine::new(120);
    let tile = build_tile(&row, &HashMap::new(), &assets, &engine);
    let style = tile.manifest.style.as_ref().expect("style present");

    let get = |key: &str| -> Option<String> {
        legacy
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let get_int =
        |key: &str| -> Option<i64> { legacy.get(key).and_then(serde_json::Value::as_i64) };

    assert_eq!(get("color").as_deref(), style.color.as_deref(), "color");
    assert_eq!(
        get("color2").as_deref(),
        style.color2.as_deref(),
        "color2 (active-state pair)"
    );
    assert_eq!(
        get("border_color").as_deref(),
        style.border_color.as_deref(),
        "border_color must reach the v2 wire"
    );
    assert_eq!(
        get("border_color2").as_deref(),
        style.border_color2.as_deref(),
        "border_color2 (active-state pair)"
    );
    assert_eq!(
        get("icon_color").as_deref(),
        style.icon_color.as_deref(),
        "icon_color must reach the v2 wire"
    );
    assert_eq!(
        get("icon_color2").as_deref(),
        style.icon_color2.as_deref(),
        "icon_color2 (active-state pair)"
    );
    assert_eq!(
        get("title_color").as_deref(),
        style.title_color.as_deref(),
        "title_color must reach the v2 wire"
    );
    assert_eq!(
        get("title_color2").as_deref(),
        style.title_color2.as_deref(),
        "title_color2 (active-state pair)"
    );
    // Round-4 parity wave: title pinning/box and the active-state shape.
    // Legacy carries the raw int columns; v2 mirrors them as optional
    // numbers/strings (0/absent = the client default).
    assert_eq!(
        get_int("title_position").map(|v| v as u8),
        style.title_position,
        "title_position must reach the v2 wire"
    );
    assert_eq!(
        get_int("title_position2").map(|v| v as u8),
        style.title_position2,
        "title_position2 (active-state pair)"
    );
    assert_eq!(
        get("title_box_color").as_deref(),
        style.title_box_color.as_deref(),
        "title_box_color must reach the v2 wire"
    );
    assert_eq!(
        get("title_box_color2").as_deref(),
        style.title_box_color2.as_deref(),
        "title_box_color2 (active-state pair)"
    );
    assert_eq!(
        get_int("shape2").map(|v| v.to_string()),
        style.shape2,
        "shape2 (active-state pair, stringified like shape)"
    );
    assert_eq!(
        get("unicode").as_deref(),
        style.icon.as_deref(),
        "icon glyph"
    );
    assert_eq!(
        get("unicode2").as_deref(),
        style.icon2.as_deref(),
        "icon2 glyph (active-state pair)"
    );
    assert_eq!(
        get("prefix").as_deref(),
        style.icon_family.as_deref(),
        "glyph font family"
    );
    assert_eq!(get("title").as_deref(), style.title.as_deref(), "title");
    assert_eq!(
        tile.manifest.kind,
        WidgetKind::Button,
        "shared kind semantics"
    );
}

/// The zero defaults stay off the v2 wire (optional fields, §4: absent
/// means the client default) while legacy always carries the raw 0s.
#[test]
fn zero_title_position_and_shape_default_stay_off_the_v2_wire() {
    let mut r = row("url", "button", Some("https://x.co"));
    r.title_position = 0;
    r.title_position2 = 0;
    r.title_box_color = Some(String::new());
    r.shape2 = 0;
    let (legacy, tile) = both(&r);
    let style = style_of(&tile);
    assert_eq!(legacy["title_position"], 0);
    assert_eq!(legacy["title_position2"], 0);
    assert_eq!(legacy["shape2"], 0);
    assert!(style.title_position.is_none());
    assert!(style.title_position2.is_none());
    // empty strings filter like every other optional color
    assert!(style.title_box_color.is_none());
    assert!(style.shape2.is_none());
}

#[test]
fn state2_image_reaches_the_v2_wire() {
    let row = styled_row();
    let legacy = Mapper::new().shortcut_payload(&row);
    assert!(
        !legacy["img2"].as_str().unwrap_or_default().is_empty(),
        "legacy wire carries img2 (contract)"
    );
    let (assets, _dir) = store();
    let engine = StateEngine::new(120);
    let tile = build_tile(&row, &HashMap::new(), &assets, &engine);
    let hash = tile
        .manifest
        .asset_hash
        .as_ref()
        .expect("img -> asset_hash");
    assert_eq!(
        assets.read_for_serving(hash, None).unwrap(),
        AssetBody::Full(b"face".to_vec())
    );
    let hash2 = tile
        .manifest
        .asset_hash2
        .as_ref()
        .expect("img2 must reach the v2 wire as asset_hash2");
    assert_eq!(
        assets.read_for_serving(hash2, None).unwrap(),
        AssetBody::Full(b"face-active".to_vec())
    );
}

// ---- round-4 Spotify kinds (design §3/§4) ---------------------------------

#[test]
fn spotify_dual_state_kinds_agree_between_the_wires() {
    // spotify-playback with the "play" command: the legacy payload marks
    // it app=custom-value watching "spotify-playing" (the vol_mute-style
    // quirk); v2 must call it a Toggle reading the matching ext channel
    // and styling it with the playback green
    let (legacy, tile) = both(&row("spotify-playback", "button", Some("play")));
    assert_eq!(legacy["app"], "custom-value");
    assert_eq!(legacy["extra"], "spotify-playing");
    assert_eq!(tile.manifest.kind, WidgetKind::Toggle);
    assert_eq!(tile.manifest.interactions, vec![Interaction::Tap]);
    let state = tile
        .manifest
        .state
        .as_ref()
        .expect("toggle watches a channel");
    assert_eq!(state.channel, "ext.spotify-playing");
    assert_eq!(style_of(&tile).color.as_deref(), Some("#1DB954"));

    // other playback commands remain plain buttons on both wires
    let (legacy, tile) = both(&row("spotify-playback", "button", Some("next")));
    assert!(legacy.get("app").is_none());
    assert_eq!(tile.manifest.kind, WidgetKind::Button);
    assert!(tile.manifest.state.is_none());

    // shuffle watches its own kind; repeat/like watch pushed keys that
    // differ from the kind - the channel must match the legacy extra on
    // every one, or the two clients flip different tiles
    for (kind, key, color) in [
        ("spotify-shuffle", "spotify-shuffle", "#1db954"),
        ("spotify-repeat", "spotify-repeat-on", "#1db954"),
        ("spotify-like", "spotify-liked", "#1db954"),
    ] {
        let (legacy, tile) = both(&row(kind, "button", None));
        assert_eq!(legacy["app"], "custom-value", "{kind}");
        assert_eq!(legacy["extra"], key, "{kind}");
        assert_eq!(tile.manifest.kind, WidgetKind::Toggle, "{kind}");
        let state = tile
            .manifest
            .state
            .as_ref()
            .expect("{kind} watches a channel");
        assert_eq!(state.channel, format!("ext.{key}"), "{kind}");
        assert_eq!(style_of(&tile).color.as_deref(), Some(color), "{kind}");
        assert_eq!(
            legacy["toggle_key"].as_str(),
            Some(key),
            "{kind} carries the pushed key as toggle_key"
        );
    }
}

#[test]
fn spotify_button_kinds_agree_between_the_wires() {
    // tracks / add stay plain buttons; device extracts its jsonKey like
    // speaker-device (the stock client shows the chosen device name)
    for kind in ["spotify-tracks", "spotify-add"] {
        let (legacy, tile) = both(&row(kind, "button", None));
        assert!(legacy.get("app").is_none(), "{kind}");
        assert_eq!(tile.manifest.kind, WidgetKind::Button, "{kind}");
        assert!(tile.manifest.state.is_none(), "{kind}");
    }

    let (legacy, tile) = both(&row(
        "spotify-device",
        "button",
        Some(r#"{"device":"Kitchen"}"#),
    ));
    assert_eq!(legacy["command"], "Kitchen");
    assert_eq!(legacy["extra"], "spotify-device");
    assert_eq!(tile.manifest.kind, WidgetKind::Button);
    assert_eq!(tile.manifest.interactions, vec![Interaction::Tap]);
    assert_eq!(style_of(&tile).color.as_deref(), Some("#1db954"));
}

#[test]
fn spotify_slider_kinds_agree_between_the_wires() {
    // volume watches its own kind, seek watches the pushed progress key
    for (kind, key) in [
        ("spotify-volume", "spotify-volume"),
        ("spotify-seek", "spotify-progress"),
    ] {
        let (legacy, tile) = both(&row(kind, "slider", None));
        assert_eq!(legacy["mode"], "slider", "{kind}");
        assert_eq!(legacy["extra"], key, "{kind}");
        assert_eq!(tile.manifest.kind, WidgetKind::Slider, "{kind}");
        assert_eq!(
            tile.manifest.interactions,
            vec![Interaction::Slide],
            "{kind}"
        );
        let state = tile
            .manifest
            .state
            .as_ref()
            .expect("{kind} watches a channel");
        assert_eq!(state.channel, format!("ext.{key}"), "{kind}");
        assert_eq!(state.shape, pulpit_proto::StateShape::Scalar, "{kind}");
    }
}

#[test]
fn spotify_now_playing_is_a_status_tile_on_both_wires() {
    let (legacy, tile) = both(&row("spotify-now-playing", "status", None));
    assert_eq!(legacy["mode"], "status");
    assert_eq!(legacy["extra"], "");
    // v2 renders the status payload on List tiles (the aidev display
    // class) with no gestures, watching the type-named channel
    assert_eq!(tile.manifest.kind, WidgetKind::List);
    assert!(tile.manifest.interactions.is_empty());
    let state = tile
        .manifest
        .state
        .as_ref()
        .expect("status mode falls back to the kind");
    assert_eq!(state.channel, "ext.spotify-now-playing");
    assert_eq!(style_of(&tile).color.as_deref(), Some("#1db954"));
}

#[test]
fn untitled_board_tile_shows_the_target_board_name() {
    // MOB-09: an untitled board-switch tile shows the target board's
    // name on the desktop (TileCell resolves it client-side from the
    // command) but the v2 wire carried no command, so tablets showed an
    // empty face. The legacy wire keeps the empty title (the stock
    // client resolves the name itself); the v2 server resolves it, and
    // both surfaces end up labeling the tile "Media".
    let names = HashMap::from([(2, "Media".to_string())]);
    let mut r = row("board", "button", Some(r#"{"id": 2}"#));
    r.title = None;
    let (legacy, tile) = both_with(&r, &names);
    assert_eq!(legacy["title"].as_str(), None, "legacy title stays raw");
    assert_eq!(style_of(&tile).title.as_deref(), Some("Media"));

    // an explicit title always wins
    let r = row("board", "button", Some(r#"{"id": 2}"#)); // title "T"
    let (_, tile) = both_with(&r, &names);
    assert_eq!(style_of(&tile).title.as_deref(), Some("T"));

    // unknown target (board deleted): nothing to inject
    let mut r = row("board", "button", Some(r#"{"id": 9}"#));
    r.title = None;
    let (_, tile) = both_with(&r, &names);
    assert_eq!(style_of(&tile).title, None);

    // junk command JSON: defensive, no injection
    let mut r = row("board", "button", Some("not json"));
    r.title = None;
    let (_, tile) = both_with(&r, &names);
    assert_eq!(style_of(&tile).title, None);

    // non-board tiles never get a name injected
    let mut r = row("url", "button", Some("https://x.co"));
    r.title = None;
    let (_, tile) = both_with(&r, &names);
    assert_eq!(style_of(&tile).title, None);
}

fn store() -> (AssetStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    (
        AssetStore::open(dir.path().to_path_buf()).expect("asset store"),
        dir,
    )
}
