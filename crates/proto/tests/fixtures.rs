//! Golden fixture contract (docs/protocol-v2.md §10): every wire message
//! has one committed example that must (a) parse into its typed payload
//! and (b) re-serialize to JSON equal to the file, envelope included. The
//! Kotlin unit test parses the same files, so wire drift fails both builds.

use pulpit_proto::*;
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
    assert_eq!(hello.client, "pulpit-mobile");
    assert_eq!(hello.name.as_deref(), Some("Tablet salon"));
    assert_eq!(
        hello.capabilities,
        vec!["graph".to_string(), "list".to_string()]
    );
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
        ChannelInfo {
            shape: StateShape::Series,
            cap: Some(SERIES_CAP),
            title: Some("CPU Load".to_string()),
            suffix: Some("%".to_string()),
        }
    );
    assert_eq!(w.channels["ext.speaker-muted"].shape, StateShape::Scalar);
    assert_eq!(w.channels["ext.speaker-muted"].cap, None);
    assert_eq!(w.channels["ext.speaker-muted"].title, None);
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
    let [board] = &sync.boards[..] else {
        panic!("one board")
    };
    assert_eq!(
        board.background,
        Some(Background::Color {
            color: "#2c3e50".into()
        })
    );
    let button = &board.tiles[0];
    assert_eq!(button.id, 17);
    assert_eq!(
        button.placement,
        Placement {
            x: 0,
            y: 0,
            w: 1,
            h: 1
        }
    );
    assert_eq!(button.manifest.kind, WidgetKind::Button);
    assert_eq!(
        button.manifest.state.as_ref().unwrap().channel,
        "ext.speaker-muted"
    );
    let slider = &board.tiles[1];
    assert_eq!(slider.manifest.kind, WidgetKind::Slider);
    assert_eq!(slider.manifest.interactions, vec![Interaction::Slide]);

    // system media tiles (SMTC): the now-playing display tile is a List
    // over the pushed payload with the tap toggle, the seek slider reads
    // the pushed progress fraction
    let media = &board.tiles[3];
    assert_eq!(media.id, 22);
    assert_eq!(media.manifest.kind, WidgetKind::List);
    assert_eq!(media.manifest.interactions, vec![Interaction::Tap]);
    assert_eq!(
        media.manifest.state.as_ref().unwrap().channel,
        "ext.media-now-playing"
    );
    let seek = &board.tiles[4];
    assert_eq!(seek.id, 23);
    assert_eq!(seek.manifest.kind, WidgetKind::Slider);
    assert_eq!(
        seek.manifest.state.as_ref().unwrap().channel,
        "ext.media-progress"
    );
    // style parity fields (012 C5): border/icon/title colors travel as
    // optional pairs; state 2 falls back to state 1 per field client-side
    let style = button.manifest.style.as_ref().unwrap();
    assert_eq!(style.border_color.as_deref(), Some("#101010"));
    assert_eq!(style.border_color2.as_deref(), Some("#f0f0f0"));
    assert_eq!(style.icon_color.as_deref(), Some("#ffe0e0"));
    assert_eq!(style.icon_color2.as_deref(), Some("#1db954"));
    assert_eq!(style.title_color.as_deref(), Some("#ffcc00"));
    assert_eq!(style.title_color2.as_deref(), Some("#00ffcc"));
    // second style-parity wave (round 4): title pinning/box and the
    // active-state shape travel as optional fields; 0/absent keeps the
    // client default (bottom-pinned, unboxed, resting shape)
    assert_eq!(style.title_position, Some(2));
    assert_eq!(style.title_position2, Some(1));
    assert_eq!(style.title_box_color.as_deref(), Some("#1c1c1c"));
    assert_eq!(style.title_box_color2.as_deref(), Some("#2c2c2c"));
    assert_eq!(style.shape2.as_deref(), Some("1"));
    assert_eq!(style.color2.as_deref(), Some("#ED4245"));
    assert_eq!(style.icon2.as_deref(), Some("\u{f028}"));
    let hash2 = button.manifest.asset_hash2.as_deref().expect("img2 hash");
    assert_eq!(hash2.len(), 64);
    // utility tools (round 5): the widget hint names the tool kind, the
    // tile config rides params, and the state channel is the per-id
    // `tool-<id>` key the manager pushes compact state under
    let tool = &board.tiles[2];
    assert_eq!(tool.id, 30);
    assert_eq!(tool.manifest.kind, WidgetKind::Button);
    assert_eq!(tool.manifest.params["widget"], "tool-timer");
    assert_eq!(tool.manifest.params["duration"], "05:00");
    assert_eq!(tool.manifest.params["finish_action"], "play");
    assert_eq!(tool.manifest.state.as_ref().unwrap().channel, "ext.tool-30");
    assert_eq!(
        tool.manifest.interactions,
        vec![
            Interaction::Tap,
            Interaction::LongPress,
            Interaction::DoubleTap
        ]
    );
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
                vec![
                    Interaction::Tap,
                    Interaction::PressStart,
                    Interaction::PressEnd
                ]
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
    assert_eq!(
        sync.values["ext.tool-timer"],
        serde_json::json!({
            "durationMs": 300000,
            "elapsedMs": 0,
            "finished": false,
            "running": false,
            "startedAtMs": 0,
        })
    );
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

/// M5 custom gestures ride the ordinary interaction frame; the fixture
/// pins the wire shape of a declared gesture (empty args, kebab name).
#[test]
fn interaction_longpress() {
    let (frame, i) = pinned::<InteractionPayload>("interaction-longpress", TYPE_INTERACTION);
    assert_eq!(frame.id.as_deref(), Some("i10"));
    assert_eq!(i.interaction, Interaction::LongPress);
    assert_eq!(i.args, InteractionArgs::default());
}

#[test]
fn server_shutdown() {
    // A pure control push: no id/ack and an empty payload object - there
    // is no typed message to pin, so round-trip the envelope itself.
    let (frame, raw) = fixture("server.shutdown");
    assert_eq!(frame.kind, TYPE_SERVER_SHUTDOWN);
    assert_eq!(frame.id, None);
    assert_eq!(frame.ack, None);
    let round: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&frame).unwrap()).unwrap();
    assert_eq!(round, raw);
}
