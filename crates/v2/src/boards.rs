//! DB rows -> protocol v2 boards. Style and watch-key resolution reuse the
//! legacy `Mapper` (single source of truth for those rules, tested against
//! the original client); this module reshapes its output into manifests,
//! derives state channels and converts legacy data-URL images into the
//! asset store.

use deckboard_db::{BoardRow, ButtonRow};
use deckboard_legacy::{Backend, Mapper};
use deckboard_proto::{
    background_from_legacy, Board, Interaction, Placement, StateRef, StateShape, Style, Tile,
    WidgetKind, WidgetManifest,
};
use serde_json::Value;

use crate::assets::AssetStore;
use crate::state::{ext_channel, StateEngine};

/// All boards with their tiles, in legacy `order`.
pub fn build_boards(
    backend: &dyn Backend,
    assets: &AssetStore,
    engine: &StateEngine,
) -> Vec<Board> {
    backend
        .get_boards()
        .iter()
        .map(|board| {
            let tiles = backend
                .get_buttons_by_board(board.id)
                .iter()
                .map(|row| build_tile(row, assets, engine))
                .collect();
            Board {
                id: board.id,
                name: board.name.clone(),
                width: board.width.max(1) as u32,
                height: board.height.max(1) as u32,
                order: board.order.max(0) as u32,
                background: board_background(board, assets),
                tiles,
            }
        })
        .collect()
}

fn board_background(board: &BoardRow, assets: &AssetStore) -> Option<deckboard_proto::Background> {
    // A board image (data URL in the legacy column) wins over the color,
    // mirroring how the original app renders the image over the background.
    if !board.image.is_empty() {
        if let Some(hash) = assets.import_data_url(&board.image) {
            return Some(deckboard_proto::Background::Asset { hash });
        }
    }
    background_from_legacy(&board.background)
}

/// One row -> one tile. The legacy payload supplies resolved style fields
/// and the watch key (`extra`); state channels are namespaced `ext.<key>`
/// so pushes land on the channel the tile reads.
pub fn build_tile(row: &ButtonRow, assets: &AssetStore, engine: &StateEngine) -> Tile {
    let legacy = Mapper::new().shortcut_payload(row);
    let (kind, interactions) = widget_kind(row, &legacy);
    let state = state_ref(row, &legacy, engine);
    let asset_hash = row
        .img
        .as_deref()
        .filter(|img| !img.is_empty())
        .and_then(|img| assets.import_data_url(img));

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
            params: row
                .options
                .as_deref()
                .and_then(|o| serde_json::from_str(o).ok())
                .unwrap_or(Value::Null),
            state,
            interactions,
            style: Some(style(row, &legacy)),
            web_package: None,
            // Dual-state `img2` is empty in every known DB; if it ever
            // matters, extend the manifest instead of guessing.
            asset_hash,
        },
    }
}

/// Gestures a tile declares - the same set the manifest carries, derived
/// so the interaction handler can enforce it. Runs per interaction event
/// (taps, every slide tick), so it must not build the full legacy payload;
/// only the `app` marker feeds the kind decision.
pub fn allowed_interactions(row: &ButtonRow) -> Vec<Interaction> {
    let app = Mapper::new().app_value(row);
    widget_kind_for(row, app.as_deref()).1
}

/// Widget kind from the legacy `mode`/`app` columns: rendering modes map
/// 1:1, custom-value buttons are toggles, everything else is a button.
fn widget_kind(row: &ButtonRow, legacy: &Value) -> (WidgetKind, Vec<Interaction>) {
    widget_kind_for(row, legacy.get("app").and_then(Value::as_str))
}

fn widget_kind_for(row: &ButtonRow, app: Option<&str>) -> (WidgetKind, Vec<Interaction>) {
    match row.mode.as_str() {
        "slider" => (WidgetKind::Slider, vec![Interaction::Slide]),
        "knob" => (WidgetKind::Knob, vec![Interaction::Slide]),
        "graph" => (WidgetKind::Graph, vec![]),
        "list" => (WidgetKind::List, vec![]),
        // ai dev-work display tiles: a read-only row list, no gestures
        "status" => (WidgetKind::List, vec![]),
        _ if app == Some("custom-value") => (WidgetKind::Toggle, vec![Interaction::Tap]),
        _ => (
            WidgetKind::Button,
            vec![
                Interaction::Tap,
                Interaction::PressStart,
                Interaction::PressEnd,
            ],
        ),
    }
}

/// State channel + shape, registered with the engine as a side effect so
/// the channel shows up in `welcome` even before the first push.
fn state_ref(row: &ButtonRow, legacy: &Value, engine: &StateEngine) -> Option<StateRef> {
    let key = legacy.get("extra").and_then(Value::as_str)?;
    if key.is_empty() {
        return None;
    }
    let shape = if row.mode == "graph" {
        StateShape::Series
    } else {
        StateShape::Scalar
    };
    let channel = ext_channel(key);
    engine.register(&channel, shape, None);
    Some(StateRef { channel, shape })
}

fn style(row: &ButtonRow, legacy: &Value) -> Style {
    let color = legacy.get("color").and_then(Value::as_str);
    let unicode = legacy.get("unicode").and_then(Value::as_str);
    Style {
        color: non_empty(color),
        color2: non_empty(row.color2.as_deref()),
        icon: non_empty(unicode),
        title: non_empty(row.title.as_deref()),
        // Legacy shape column is an int (0 = default); pass non-defaults
        // through so the client can render them.
        shape: (row.shape != 0).then(|| row.shape.to_string()),
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|v| !v.is_empty()).map(str::to_string)
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
                widget_kind(&r, &legacy).1,
                "{kind}/{mode}/{command:?}"
            );
        }
    }

    #[test]
    fn vol_mute_maps_to_toggle_with_channel() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&row("vol", "button", Some("vol_mute")), &assets, &engine);
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
    fn plain_button_press_interactions() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&row("key", "button", Some("CTRL + K")), &assets, &engine);
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
    fn graph_mode_becomes_series_channel() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&row("si-cpu", "graph", Some("cpu-key")), &assets, &engine);
        assert_eq!(tile.manifest.kind, WidgetKind::Graph);
        let state = tile.manifest.state.unwrap();
        assert_eq!(state.channel, "ext.cpu-key"); // raw command wins when set
        assert_eq!(state.shape, StateShape::Series);
        assert_eq!(engine.catalog()["ext.cpu-key"].cap, Some(120));
    }

    #[test]
    fn slider_and_custom_value_kinds() {
        let (assets, _dir) = asset_store();
        let engine = StateEngine::new(120);
        let tile = build_tile(&row("speaker-volume", "slider", None), &assets, &engine);
        assert_eq!(tile.manifest.kind, WidgetKind::Slider);
        assert_eq!(tile.manifest.interactions, vec![Interaction::Slide]);
        assert_eq!(tile.manifest.state.unwrap().channel, "ext.speaker-volume");

        let tile = build_tile(&row("toggle-microphone", "button", None), &assets, &engine);
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
        let tile = build_tile(&r, &assets, &engine);
        assert!(tile.manifest.asset_hash.is_some());
        assert_eq!(
            assets
                .get(tile.manifest.asset_hash.as_ref().unwrap())
                .unwrap(),
            b"img-bytes"
        );
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
