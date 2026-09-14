//! Per-type button style resolution, ported 1:1 from the original main
//! process (webpack module 8369). The JSON assets were extracted from the
//! original bundle and FontAwesome packages, so defaults are byte-identical.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

const BUTTONPROPS_JSON: &str = include_str!("../assets/buttonprops.json");
const ICONS_JSON: &str = include_str!("../assets/icons.json");

pub const FALLBACK_COLOR: &str = "#95a5a6";

/// One extension input (`this.inputs.push({...})` in extension code) that
/// can act as a button style source, registered by the extension host.
#[derive(Debug, Clone)]
pub struct ExtInput {
    pub value: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub font_icon: Option<String>,
    pub mode: Option<String>,
    pub command: Option<String>,
}

fn ext_inputs() -> &'static RwLock<HashMap<String, ExtInput>> {
    static INPUTS: OnceLock<RwLock<HashMap<String, ExtInput>>> = OnceLock::new();
    INPUTS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register one extension input so its style can back buttons of that type.
pub fn register_extension_input(input: ExtInput) {
    if input.value.is_empty() {
        return;
    }
    ext_inputs()
        .write()
        .expect("ext input registry poisoned")
        .insert(input.value.clone(), input);
}

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

    /// Port of the original `buttonProps(row, type, command)` dispatch:
    /// built-in styles first, then extension inputs (`getExtensionButton`).
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
            .unwrap_or_else(|| self.extension_props(kind))
    }

    /// Port of `getExtensionButton`: style from the extension input whose
    /// `value` equals the button type; unknown icons fall back to
    /// exclamation-circle, custom-value inputs add app + toggle_key.
    fn extension_props(&self, kind: &str) -> Props {
        let input = ext_inputs()
            .read()
            .expect("ext input registry poisoned")
            .get(kind)
            .cloned();
        let Some(input) = input else {
            return Props::default();
        };
        let prefix = input.font_icon.clone().unwrap_or_else(|| "fas".to_string());
        let icon = input.icon.clone().unwrap_or_else(|| "exclamation-circle".to_string());
        Props {
            unicode: self.icon_unicode(&icon, &prefix),
            color: input.color,
            prefix: Some(prefix),
            app: (input.mode.as_deref() == Some("custom-value"))
                .then(|| "custom-value".to_string()),
            toggle_key: (input.mode.as_deref() == Some("custom-value")).then(|| {
                input
                    .command
                    .clone()
                    .unwrap_or_else(|| kind.to_string())
            }),
            json_key: None,
        }
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
    fn extension_input_backs_button_style() {
        register_extension_input(ExtInput {
            value: "si-cpu".into(),
            icon: Some("headphones".into()),
            color: Some("#8E44AD".into()),
            font_icon: Some("fas".into()),
            mode: Some("graph".into()),
            command: None,
        });
        let r = StyleResolver::global();
        let p = r.props_for("si-cpu", None);
        assert_eq!(p.color.as_deref(), Some("#8E44AD"));
        assert_eq!(p.prefix.as_deref(), Some("fas"));
        assert_eq!(
            p.unicode.map(|u| u.chars().next().unwrap() as u32),
            Some(0xf025)
        );
        // custom-value inputs carry app + toggle_key like the original
        register_extension_input(ExtInput {
            value: "my-value".into(),
            icon: None,
            color: Some("#123456".into()),
            font_icon: None,
            mode: Some("custom-value".into()),
            command: None,
        });
        let p = r.props_for("my-value", None);
        assert_eq!(p.app.as_deref(), Some("custom-value"));
        assert_eq!(p.toggle_key.as_deref(), Some("my-value"));
        assert_eq!(p.unicode.map(|u| u.chars().next().unwrap() as u32), Some(0xf06a));
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
