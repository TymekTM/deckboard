//! Protocol v2 types - the single source of truth for the wire format
//! (ADR-004). TypeScript bindings are generated into `bindings/` by
//! `cargo test -p pulpit-proto`; the golden fixtures in
//! `tests/fixtures/` pin the exact JSON the Kotlin client parses too.
//!
//! Boards are data, not code: every tile carries a widget manifest the
//! client renders with its built-in renderers. Unknown kinds degrade to a
//! plain button. `Web` widgets are HTML bundles rendered in the board's
//! shared WebView layer; `Photo`/`Video` reference hashed assets served by
//! the desktop (`/assets/<hash>`), never inline dataURLs.
//!
//! Evolution rules (docs/protocol-v2.md §10): additive changes never break a
//! conforming client - unknown message types are ignored, unknown fields
//! dropped, unknown enum values degrade through the `#[serde(other)]`
//! variants.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const PROTOCOL_VERSION: u32 = 2;

/// Inbound frames larger than this get `error {code: "too-large"}` and a
/// close.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Default ring-buffer size for series channels.
pub const SERIES_CAP: u32 = 120;

// Message type names. Kept as constants (not an enum) so that unknown
// future types survive a round-trip as opaque strings, per the evolution
// rules above.
pub const TYPE_HELLO: &str = "hello";
pub const TYPE_WELCOME: &str = "welcome";
pub const TYPE_ERROR: &str = "error";
pub const TYPE_BOARDS_SYNC: &str = "boards.sync";
pub const TYPE_BOARDS_DELTA: &str = "boards.delta";
pub const TYPE_BOARD_OPEN: &str = "board.open";
pub const TYPE_STATE_SYNC: &str = "state.sync";
pub const TYPE_STATE_PATCH: &str = "state.patch";
pub const TYPE_INTERACTION: &str = "interaction";
/// Reserved for M6 two-step widget flows and web-widget messaging.
pub const TYPE_WIDGET_EVENT: &str = "widget.event";
/// Reserved for a future remote editor; the editor writes in-process.
pub const TYPE_BOARDS_WRITE: &str = "boards.write";
/// Server-to-client only: the server is exiting on purpose (app quit or
/// machine shutdown). The client stops reconnecting; a WS close follows.
pub const TYPE_SERVER_SHUTDOWN: &str = "server.shutdown";

/// Error codes carried in `error` frames (docs/protocol-v2.md §2).
pub mod error_code {
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const PAIR_INVALID: &str = "pair-invalid";
    pub const PAIR_EXPIRED: &str = "pair-expired";
    pub const OUTDATED_CLIENT: &str = "outdated-client";
    pub const UNKNOWN_TYPE: &str = "unknown-type";
    pub const BAD_FRAME: &str = "bad-frame";
    pub const TOO_LARGE: &str = "too-large";
    pub const UNKNOWN_TILE: &str = "unknown-tile";
    pub const UNSUPPORTED_INTERACTION: &str = "unsupported-interaction";
    pub const INTERNAL: &str = "internal";
}

/// One event frame on the v2 WebSocket. Requests carry `id`; responses
/// echo it back as `ack`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Frame {
    pub v: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

impl Frame {
    /// Client/server request frame: carries `id`, expects an `ack`.
    pub fn request(kind: &str, id: &str, payload: serde_json::Value) -> Frame {
        Frame {
            v: PROTOCOL_VERSION,
            id: Some(id.to_string()),
            ack: None,
            kind: kind.to_string(),
            payload: Some(payload),
        }
    }

    /// Server push: no `id`/`ack`.
    pub fn push(kind: &str, payload: serde_json::Value) -> Frame {
        Frame {
            v: PROTOCOL_VERSION,
            id: None,
            ack: None,
            kind: kind.to_string(),
            payload: Some(payload),
        }
    }

    /// Server push with a typed payload, serialized to JSON.
    pub fn push_typed<T: Serialize>(kind: &str, payload: &T) -> Frame {
        Frame::push(
            kind,
            serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Server control push that carries no payload at all (the envelope
    /// omits it, per the frame-envelope rules).
    pub fn bare(kind: &str) -> Frame {
        Frame {
            v: PROTOCOL_VERSION,
            id: None,
            ack: None,
            kind: kind.to_string(),
            payload: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Hello {
    pub client: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// One live state channel in the `welcome` catalog: how to render its
/// values and (for series) the server's ring-buffer size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ChannelInfo {
    pub shape: StateShape,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap: Option<u32>,
    /// Display title captured from a producer's custom-value object
    /// (`{"title": "CPU Load", ..}`), so graph tiles can label themselves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Unit suffix captured alongside the title (e.g. "%", "GB").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Welcome {
    pub protocol: u32,
    pub desktop_version: String,
    /// Clients below this version are closed with `outdated-client`.
    pub min_client: String,
    /// Board generation; grows by one per committed write batch.
    #[ts(type = "number")]
    pub generation: u64,
    pub device: Device,
    #[serde(default, skip_serializing_if = "map_is_empty")]
    pub channels: std::collections::BTreeMap<String, ChannelInfo>,
    /// The device token, issued only in the welcome that completes a
    /// pairing (docs/protocol-v2.md §3 step 4); reconnecting devices know
    /// it already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ErrorPayload {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Board background: flat color or a hashed asset (image).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Background {
    Color { color: String },
    Asset { hash: String },
}

/// Board grid background resolution of the legacy `background` column:
/// empty string means "no color set".
pub fn background_from_legacy(raw: &str) -> Option<Background> {
    (!raw.is_empty()).then(|| Background::Color {
        color: raw.to_string(),
    })
}

/// Board-level v2 struct: a grid plus free-placement tiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Board {
    pub id: i64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub order: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<Background>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiles: Vec<Tile>,
}

/// Full snapshot pushed after `welcome` and whenever the editor replaces
/// board data wholesale (import).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct BoardsSync {
    #[ts(type = "number")]
    pub generation: u64,
    pub boards: Vec<Board>,
}

/// One committed board change. `tile-set` always carries the full tile,
/// which covers add, style edit and move/resize alike.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum BoardOp {
    BoardSet { board: Board },
    BoardRemove { board: i64 },
    TileSet { board: i64, tile: Tile },
    TileRemove { board: i64, tile: i64 },
    TileClear { board: i64 },
}

/// Live board change batch; broadcast to all clients. Clients that miss
/// deltas (reconnect) recover via `boards.sync`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct BoardsDelta {
    #[ts(type = "number")]
    pub generation: u64,
    pub ops: Vec<BoardOp>,
}

/// Server directive to show a board (a `board` command executed). Purely
/// UI: board data itself only flows through sync/delta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct BoardOpen {
    pub board: i64,
}

/// Full current state, pushed once after `welcome`. Series arrays run
/// oldest -> newest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StateSync {
    #[serde(default, skip_serializing_if = "map_is_empty")]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "map_is_empty")]
    pub series: std::collections::BTreeMap<String, Vec<f64>>,
}

/// One channel value in a patch batch. For series channels `value` is the
/// newest point (clients append).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct ChannelValue {
    pub channel: String,
    pub value: serde_json::Value,
}

/// Coalesced state updates, flushed every 100 ms, latest wins per channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StatePatch {
    pub changes: Vec<ChannelValue>,
}

/// Per-gesture arguments. Only the field matching the interaction kind is
/// meaningful (`slide` -> `value`, `wheel` -> `delta`, `drag` -> `dx/dy`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct InteractionArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dx: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dy: Option<f64>,
}

/// Client -> server user gesture on a tile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct InteractionPayload {
    pub board: i64,
    pub tile: i64,
    pub interaction: Interaction,
    #[serde(default)]
    pub args: InteractionArgs,
}

/// Empty success payload for acks (serializes as `{}`).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Ack {}

/// Widget kinds the client knows how to render. New kinds are additive:
/// older clients fall back to `Button`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum WidgetKind {
    Button,
    Toggle,
    Slider,
    Knob,
    Graph,
    List,
    Web,
    Photo,
    Video,
    /// Unknown to this client version; render as a button.
    #[serde(other)]
    Other,
}

/// User interactions a widget accepts (declared in the manifest so the
/// client knows which gestures to grab).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum Interaction {
    Tap,
    PressStart,
    PressEnd,
    Slide,
    Wheel,
    Drag,
    /// Custom gesture only known to newer clients; ignored otherwise.
    #[serde(other)]
    Other,
}

/// Declarative widget manifest stored inside a board layout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct WidgetManifest {
    pub kind: WidgetKind,
    #[serde(default)]
    pub params: serde_json::Value,
    /// State channel this widget reads (e.g. `"vm.volume"`), with shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<StateRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interactions: Vec<Interaction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<Style>,
    /// Web widgets only: package id of the HTML bundle to render.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_package: Option<String>,
    /// Content hash of the tile's image asset (button image, photo,
    /// video), served from `/assets/<hash>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_hash: Option<String>,
    /// Active-state image (legacy `img2`); shown instead of `asset_hash`
    /// while the tile's channel reports its active value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_hash2: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct StateRef {
    pub channel: String,
    pub shape: StateShape,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum StateShape {
    Scalar,
    Series,
    Toggle,
    List,
    /// Unknown to this client version; render as scalar.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// State-dependent colors: `color2` shows while the tile's channel is
    /// in its active state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color2: Option<String>,
    /// FontAwesome glyph as a unicode character (already resolved).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Active-state glyph (e.g. mic -> mic-slash); paired with `color2`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon2: Option<String>,
    /// Font family for the glyphs: `"fas"` (default) or `"fab"` (brands).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    /// Tile border color; clients render a hairline border when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    /// Active-state border (per-field fallback to `border_color`, §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color2: Option<String>,
    /// Glyph color; defaults to white client-side when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    /// Active-state glyph color (per-field fallback to `icon_color`, §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color2: Option<String>,
    /// Title text color; defaults to white client-side when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_color: Option<String>,
    /// Active-state title color (per-field fallback to `title_color`, §4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_color2: Option<String>,
}

/// Free placement inside a board grid (pixel-space of the 96px cell grid).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// One tile: placement + manifest flattened into a single object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
#[ts(type = "Placement & WidgetManifest")]
pub struct Tile {
    pub id: i64,
    #[serde(flatten)]
    pub placement: Placement,
    #[serde(flatten)]
    pub manifest: WidgetManifest,
}

fn map_is_empty<T>(m: &std::collections::BTreeMap<String, T>) -> bool {
    m.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = Frame {
            v: 2,
            id: Some("abc".into()),
            ack: None,
            kind: "interaction".into(),
            payload: Some(serde_json::json!({"widget": "17", "kind": "slide", "value": 0.5})),
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains("\"v\":2"));
        let back: Frame = serde_json::from_str(&s).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn unknown_widget_kind_degrades() {
        let m: WidgetManifest = serde_json::from_str(r#"{"kind":"party-confetti"}"#).unwrap();
        assert_eq!(m.kind, WidgetKind::Other);
    }

    #[test]
    fn web_widget_manifest() {
        let m: WidgetManifest = serde_json::from_str(
            r#"{"kind":"web","web_package":"weather-animated",
                "state":{"channel":"weather.temp","shape":"scalar"},
                "interactions":["tap"]}"#,
        )
        .unwrap();
        assert_eq!(m.kind, WidgetKind::Web);
        assert_eq!(m.web_package.as_deref(), Some("weather-animated"));
    }

    #[test]
    fn interaction_names_are_kebab() {
        assert_eq!(
            serde_json::to_value(Interaction::PressStart).unwrap(),
            "press-start"
        );
        assert_eq!(
            serde_json::to_value(Interaction::PressEnd).unwrap(),
            "press-end"
        );
        let i: Interaction = serde_json::from_str("\"press-hold\"").unwrap();
        assert_eq!(i, Interaction::Other);
    }

    #[test]
    fn unknown_state_shape_degrades() {
        let r: StateRef = serde_json::from_str(r#"{"channel":"x","shape":"spiral"}"#).unwrap();
        assert_eq!(r.shape, StateShape::Other);
    }

    #[test]
    fn board_ops_are_tagged_kebab() {
        let op = BoardOp::TileRemove { board: 3, tile: 17 };
        let v = serde_json::to_value(&op).unwrap();
        assert_eq!(v["op"], "tile-remove");
        assert_eq!(v["board"], 3);
        assert_eq!(v["tile"], 17);
    }

    #[test]
    fn background_from_legacy_column() {
        assert_eq!(
            background_from_legacy("#2c3e50"),
            Some(Background::Color {
                color: "#2c3e50".into()
            })
        );
        assert_eq!(background_from_legacy(""), None);
    }

    #[test]
    fn tile_flattens_placement_and_manifest() {
        let tile = Tile {
            id: 17,
            placement: Placement {
                x: 0,
                y: 0,
                w: 2,
                h: 1,
            },
            manifest: WidgetManifest {
                kind: WidgetKind::Button,
                params: serde_json::Value::Null,
                state: None,
                interactions: vec![Interaction::Tap],
                style: None,
                web_package: None,
                asset_hash: None,
                asset_hash2: None,
            },
        };
        let v = serde_json::to_value(&tile).unwrap();
        assert_eq!(v["id"], 17);
        assert_eq!(v["x"], 0);
        assert_eq!(v["w"], 2);
        assert_eq!(v["kind"], "button");
        assert_eq!(v["interactions"][0], "tap");
    }
}
