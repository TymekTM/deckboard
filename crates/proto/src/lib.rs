//! Protocol v2 types (foundation only - the v2 transport lands in M1).
//!
//! Boards are data, not code: every tile carries a widget manifest the
//! client renders with its built-in renderers. Unknown kinds degrade to a
//! plain button. `Web` widgets are HTML bundles rendered in the board's
//! shared WebView layer; `Photo`/`Video` reference hashed assets served by
//! the desktop (`/assets/<hash>`), never inline dataURLs.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 2;

/// One event frame on the v2 WebSocket. Requests carry `id`; responses
/// echo it back as `ack`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

/// Widget kinds the client knows how to render. New kinds are additive:
/// older clients fall back to `Button`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Interaction {
    Tap,
    PressHold,
    Slide,
    Wheel,
    Drag,
    /// Custom gesture only known to newer clients; ignored otherwise.
    #[serde(other)]
    Other,
}

/// Declarative widget manifest stored inside a board layout.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    /// Photo/Video widgets only: content hash of the asset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateRef {
    pub channel: String,
    pub shape: StateShape,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum StateShape {
    Scalar,
    Series,
    Toggle,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Style {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

/// Free placement inside a board grid (pixel-space of the 96px cell grid).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tile {
    pub id: String,
    #[serde(flatten)]
    pub placement: Placement,
    #[serde(flatten)]
    pub manifest: WidgetManifest,
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
            payload: Some(serde_json::json!({"widget": "t1", "kind": "slide", "value": 0.5})),
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains("\"v\":2"));
        let back: Frame = serde_json::from_str(&s).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn unknown_widget_kind_degrades() {
        let m: WidgetManifest =
            serde_json::from_str(r#"{"kind":"party-confetti"}"#).unwrap();
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
}
