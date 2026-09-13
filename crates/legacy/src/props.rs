//! Per-type button style resolution, ported 1:1 from the original main
//! process (webpack module 8369). The JSON assets were extracted from the
//! original bundle and FontAwesome packages, so defaults are byte-identical.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

const BUTTONPROPS_JSON: &str = include_str!("../assets/buttonprops.json");
const ICONS_JSON: &str = include_str!("../assets/icons.json");

pub const FALLBACK_COLOR: &str = "#95a5a6";

/// Style defaults for one command type.
#[derive(Debug, Clone, Default)]
pub struct Props {
    pub unicode: Option<String>,
    pub color: Option<String>,
    pub prefix: Option<String>,
    pub app: Option<String>,
    pub toggle_key: Option<String>,
    pub json_key: Option<String>,
}

pub struct StyleResolver {
    basic: HashMap<String, Props>,
    playback_colors: HashMap<String, Props>, // keyed by command, color filled at lookup
    stream_control: HashMap<String, Props>,
    icons_fas: HashMap<String, String>,
    icons_fab: HashMap<String, String>,
}

fn parse_props(v: &Value) -> Props {
    Props {
        unicode: v.get("unicode").and_then(Value::as_str).map(str::to_string),
        color: v.get("color").and_then(Value::as_str).map(str::to_string),
        prefix: v.get("prefix").and_then(Value::as_str).map(str::to_string),
        app: v.get("app").and_then(Value::as_str).map(str::to_string),
        toggle_key: v
            .get("toggle_key")
            .or_else(|| v.get("toggleKey"))
            .and_then(Value::as_str)
            .map(str::to_string),
        json_key: v
            .get("jsonKey")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

impl StyleResolver {
    pub fn global() -> &'static StyleResolver {
        static RESOLVER: OnceLock<StyleResolver> = OnceLock::new();
        RESOLVER.get_or_init(StyleResolver::load)
    }

    fn load() -> StyleResolver {
        let bp: Value = serde_json::from_str(BUTTONPROPS_JSON).expect("buttonprops.json");
        let basic = bp["basic"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), parse_props(v))).collect())
            .unwrap_or_default();
        let playback: HashMap<String, Props> = bp["playback"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), parse_props(v))).collect())
            .unwrap_or_default();
        let stream_control: HashMap<String, Props> = bp["streamControl"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), parse_props(v))).collect())
            .unwrap_or_default();
        let icons: Value = serde_json::from_str(ICONS_JSON).expect("icons.json");
        let to_map = |v: &Value| {
            v.as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        StyleResolver {
            basic,
            playback_colors: playback,
            stream_control,
            icons_fas: to_map(&icons["fas"]),
            icons_fab: to_map(&icons["fab"]),
        }
    }

    /// Port of the original `buttonProps(row, type, command)` dispatch.
    pub fn props_for(&self, kind: &str, command: Option<&str>) -> Props {
        if kind == "vol" {
            return self.playback_lookup("#F5AB35", command);
        }
        if kind.contains("spotify-playback") {
            return self.playback_lookup("#1DB954", command);
        }
        if kind.starts_with("slobs") && kind.contains("slobs-control") {
            return self.stream_lookup("#128079", command);
        }
        if kind.starts_with("obs") && kind.contains("obs-control") {
            return self.stream_lookup("#2C3E50", command);
        }
        self.basic
            .get(kind)
            .cloned()
            .unwrap_or_else(Props::default)
    }

    fn playback_lookup(&self, color: &str, command: Option<&str>) -> Props {
        let mut p = self
            .playback_colors
            .get(command.unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        p.color = Some(color.to_string());
        p
    }

    fn stream_lookup(&self, color: &str, command: Option<&str>) -> Props {
        let mut p = self
            .stream_control
            .get(command.unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        p.color = Some(color.to_string());
        p
    }

    /// Port of `getIconUnicode(icon, fontIcon)`: name -> FA codepoint char.
    pub fn icon_unicode(&self, icon: &str, prefix: &str) -> Option<String> {
        let map = if prefix == "fab" { &self.icons_fab } else { &self.icons_fas };
        map.get(icon).map(|code| {
            // original pads to 4 hex digits then evals "\uXXXX"
            let padded = if code.len() < 4 {
                format!("{:0>4}", code)
            } else {
                code.clone()
            };
            u32::from_str_radix(&padded, 16)
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string())
                .unwrap_or_default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vol_resolves_playback_style() {
        let r = StyleResolver::global();
        let p = r.props_for("vol", Some("vol_mute"));
        assert_eq!(p.color.as_deref(), Some("#F5AB35"));
        let uni = p.unicode.expect("vol_mute has unicode");
        assert!(!uni.is_empty());
        assert_eq!(uni.chars().count(), 1);
        assert!(uni.chars().next().unwrap() as u32 >= 0xf000);
    }

    #[test]
    fn obs_control_resolves_stream_style() {
        let r = StyleResolver::global();
        let p = r.props_for("obs-control", Some("stream"));
        assert_eq!(p.color.as_deref(), Some("#2C3E50"));
        assert_eq!(p.prefix.as_deref(), Some("fab"));
    }

    #[test]
    fn known_static_types() {
        let r = StyleResolver::global();
        let p = r.props_for("key", None);
        assert_eq!(p.color.as_deref(), Some("#EF4836"));
        let p = r.props_for("url", None);
        assert_eq!(p.color.as_deref(), Some("#22A7F0"));
    }

    #[test]
    fn unknown_type_falls_back_to_gray() {
        let r = StyleResolver::global();
        let p = r.props_for("totally-custom", None);
        assert!(p.unicode.is_none());
        assert!(p.color.is_none());
    }

    #[test]
    fn icon_lookup_with_aliases() {
        let r = StyleResolver::global();
        // FA5 name kept as alias in FA6 packages
        let uni = r.icon_unicode("angle-double-right", "fas").unwrap();
        assert_eq!(uni.chars().next().unwrap() as u32, 0xf101);
        assert!(r.icon_unicode("no-such-icon", "fas").is_none());
    }
}
