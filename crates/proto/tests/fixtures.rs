//! Golden fixture contract (docs/protocol-v2.md §9): every wire message
//! has one committed example that must (a) parse into its typed payload
//! and (b) re-serialize to JSON equal to the file, envelope included. The
//! Kotlin unit test parses the same files, so wire drift fails both builds.

use deckboard_proto::*;
use std::fs;
use std::path::Path;

fn fixture(name: &str) -> (Frame, serde_json::Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    let frame: Frame = serde_json::from_value(raw.clone()).unwrap();
    (frame, raw)
}

/// Parses the frame's payload into the expected typed message, then pins
/// both the full frame and the typed payload to the fixture's JSON.
fn pinned<T>(name: &str, kind: &str) -> (Frame, T)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let (frame, raw) = fixture(name);
    assert_eq!(frame.v, PROTOCOL_VERSION, "{name}: envelope version");
    assert_eq!(frame.kind, kind, "{name}: type name");
    let payload = frame.payload.clone().expect("{name}: payload present");
    let typed: T = serde_json::from_value(payload.clone())
        .unwrap_or_else(|e| panic!("{name}: payload parses to {kind}: {e}"));
    assert_eq!(
        serde_json::to_value(&typed).unwrap(),
        payload,
        "{name}: typed payload re-serializes identically"
    );
    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        raw,
        "{name}: whole frame re-serializes identically"
    );
    (frame, typed)
}

#[test]
fn hello() {
    let (frame, hello) = pinned::<Hello>("hello", TYPE_HELLO);
    assert_eq!(frame.id.as_deref(), Some("h1"));
    assert_eq!(frame.ack, None);
    assert_eq!(hello.client, "deckboard-mobile");
    assert_eq!(hello.name.as_deref(), Some("Tablet salon"));
    assert_eq!(hello.capabilities, vec!["graph".to_string(), "list".to_string()]);
}

#[test]
fn welcome() {
    let (frame, w) = pinned::<Welcome>("welcome", TYPE_WELCOME);
    assert_eq!(frame.ack.as_deref(), Some("h1"));
    assert_eq!(w.protocol, PROTOCOL_VERSION);
    assert_eq!(w.generation, 7);
    assert_eq!(w.device.name, "Tablet salon");
    assert_eq!(w.channels.len(), 2);
    assert_eq!(
        w.channels["ext.si-cpu-usage"],
        ChannelInfo { shape: StateShape::Series, cap: Some(SERIES_CAP) }
    );
    assert_eq!(w.channels["ext.speaker-muted"].shape, StateShape::Scalar);
    assert_eq!(w.channels["ext.speaker-muted"].cap, None);
}

#[test]
fn error_frame() {
    let (_, e) = pinned::<ErrorPayload>("error", TYPE_ERROR);
    assert_eq!(e.code, error_code::UNKNOWN_TILE);
    assert_eq!(e.message.as_deref(), Some("no tile 42"));
}

#[test]
fn boards_sync() {
    let (_, sync) = pinned::<BoardsSync>("boards.sync", TYPE_BOARDS_SYNC);
    assert_eq!(sync.generation, 7);
    let [board] = &sync.boards[..] else { panic!("one board") };
    assert_eq!(board.background, Some(Background::Color { color: "#2c3e50".into() }));
    let button = &board.tiles[0];
    assert_eq!(button.id, 17);
    assert_eq!(button.placement, Placement { x: 0, y: 0, w: 1, h: 1 });
    assert_eq!(button.manifest.kind, WidgetKind::Button);
    assert_eq!(
        button.manifest.state.as_ref().unwrap().channel,
        "ext.speaker-muted"
    );
    let slider = &board.tiles[1];
    assert_eq!(slider.manifest.kind, WidgetKind::Slider);
    assert_eq!(slider.manifest.interactions, vec![Interaction::Slide]);
}

#[test]
fn boards_delta() {
    let (_, delta) = pinned::<BoardsDelta>("boards.delta", TYPE_BOARDS_DELTA);
    assert_eq!(delta.generation, 8);
    assert_eq!(delta.ops.len(), 2);
    match &delta.ops[0] {
        BoardOp::TileSet { board, tile } => {
            assert_eq!(*board, 3);
            assert_eq!(tile.id, 17);
            assert_eq!(
                tile.manifest.interactions,
                vec![Interaction::Tap, Interaction::PressStart, Interaction::PressEnd]
            );
        }
        other => panic!("first op is tile-set, got {other:?}"),
    }
    assert_eq!(delta.ops[1], BoardOp::BoardRemove { board: 9 });
}

#[test]
fn board_open() {
    let (_, open) = pinned::<BoardOpen>("board.open", TYPE_BOARD_OPEN);
    assert_eq!(open.board, 3);
}

#[test]
fn state_sync() {
    let (_, sync) = pinned::<StateSync>("state.sync", TYPE_STATE_SYNC);
    assert_eq!(sync.values["ext.speaker-muted"], "OFF");
    assert_eq!(sync.values["discord.microphone-muted"], true);
    assert_eq!(sync.series["ext.si-cpu-usage"], vec![0.1, 0.42, 0.44]);
}

#[test]
fn state_patch() {
    let (_, patch) = pinned::<StatePatch>("state.patch", TYPE_STATE_PATCH);
    assert_eq!(patch.changes.len(), 3);
    assert_eq!(patch.changes[0].channel, "discord.microphone-muted");
    assert_eq!(patch.changes[0].value, false);
}

#[test]
fn interaction() {
    let (frame, i) = pinned::<InteractionPayload>("interaction", TYPE_INTERACTION);
    assert_eq!(frame.id.as_deref(), Some("i9"));
    assert_eq!((i.board, i.tile), (3, 21));
    assert_eq!(i.interaction, Interaction::Slide);
    assert_eq!(i.args.value, Some(0.5));
}
