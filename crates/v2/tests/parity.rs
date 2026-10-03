//! Shared-field parity oracle (012 E1/C5): the same `ButtonRow` through
//! the legacy mapper (what stock Deckboard tablets render) and the v2
//! wire builder (what the native client renders) must agree field for
//! field. The legacy wire names are contractual; the v2 Style carries
//! them as optional fields, and state 2 falls back to state 1 per field
//! client-side (docs/protocol-v2.md §4).

use pulpit_db::ButtonRow;
use pulpit_legacy::Mapper;
use pulpit_proto::WidgetKind;
use pulpit_v2::{build_tile, AssetStore, StateEngine};

fn styled_row() -> ButtonRow {
    let png1 = format!("data:image/png;base64,{}", b64(b"face"));
    let png2 = format!("data:image/png;base64,{}", b64(b"face-active"));
    ButtonRow {
        id: 17,
        board_id: 3,
        kind: "toggle-headphone".into(),
        command: None,
        title: Some("Deafen".into()),
        title_position: 0,
        title_color: Some("#ffcc00".into()),
        title_box_color: None,
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
        shape2: 0,
        border_color2: Some("#f0f0f0".into()),
        title_position2: 0,
        title_box_color2: None,
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
    let tile = build_tile(&row, &assets, &engine);
    let style = tile.manifest.style.as_ref().expect("style present");

    let get = |key: &str| -> Option<String> {
        legacy
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };

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
    let tile = build_tile(&row, &assets, &engine);
    let hash = tile
        .manifest
        .asset_hash
        .as_ref()
        .expect("img -> asset_hash");
    assert_eq!(assets.get(hash).unwrap(), b"face");
    let hash2 = tile
        .manifest
        .asset_hash2
        .as_ref()
        .expect("img2 must reach the v2 wire as asset_hash2");
    assert_eq!(assets.get(hash2).unwrap(), b"face-active");
}

fn store() -> (AssetStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    (
        AssetStore::open(dir.path().to_path_buf()).expect("asset store"),
        dir,
    )
}
