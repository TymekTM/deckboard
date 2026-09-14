//! Board/shortcut payload mapping - a 1:1 port of the original
//! `syncAllBoards` + `mapMacroButtons` + ButtonStyle mapper (webpack
//! modules 5981/8742/2836). Field names and defaults are contractual:
//! the stock Android client renders exactly these fields.

use deckboard_db::{BoardRow, ButtonRow};
use serde_json::{json, Map, Value};

use crate::props::{Props, StyleResolver, FALLBACK_COLOR};

/// Types whose raw command string is sent to the client untouched.
const RAW_COMMAND_TYPES: &[&str] = &[
    "board",
    "obs-control",
    "slobs-control",
    "xsplit-control",
    "vol",
];

pub struct Mapper {
    resolver: &'static StyleResolver,
}

impl Mapper {
    pub fn new() -> Mapper {
        Mapper {
            resolver: StyleResolver::global(),
        }
    }

    /// Full board payload for one grid variant.
    /// `max_w`/`max_h`: pass the board dimensions for pro, (4, 3) for basic.
    pub fn board_payload(&self, board: &BoardRow, buttons: &[ButtonRow], pro: bool) -> Value {
        let shortcuts = self.shortcuts_payload(board, buttons, pro);
        json!({
            "id": board.id,
            "name": board.name,
            "background": board.background,
            "layout": board.layout,
            "image": board.image,
            "sort": board.sort,
            "type": board.kind,
            "args": board.args,
            "order": board.order,
            "width": board.width,
            "height": board.height,
            "converted": board.converted,
            "staggered": true,
            "shortcuts": shortcuts,
        })
    }

    /// Mapped buttons + filler cells, filtered to the variant's grid.
    fn shortcuts_payload(&self, board: &BoardRow, buttons: &[ButtonRow], pro: bool) -> Vec<Value> {
        let board_buttons: Vec<(i64, i64, i64, i64)> = buttons
            .iter()
            .map(|b| (b.x.unwrap_or(0), b.y.unwrap_or(0), b.w, b.h))
            .collect();

        // Fillers for every cell of the board grid not covered by a button.
        let mut mapped: Vec<Value> = Vec::with_capacity(buttons.len());
        for b in buttons {
            mapped.push(self.shortcut_payload(b));
        }
        for y in 0..board.height {
            for x in 0..board.width {
                let covered = board_buttons
                    .iter()
                    .any(|(bx, by, bw, bh)| x >= *bx && x < bx + bw && y >= *by && y < by + bh);
                if !covered {
                    mapped.push(json!({ "id": null, "x": x, "y": y, "w": 1, "h": 1 }));
                }
            }
        }

        mapped.sort_by_key(|v| {
            let x = v["x"].as_i64().unwrap_or(0);
            let y = v["y"].as_i64().unwrap_or(0);
            (y, x)
        });

        mapped
            .into_iter()
            .filter_map(|mut v| {
                let x = v["x"].as_i64().unwrap_or(0);
                let y = v["y"].as_i64().unwrap_or(0);
                if pro {
                    (x < board.width && y < board.height).then_some(v)
                } else if x >= 4 || y >= 3 {
                    None
                } else {
                    // basic variant crops oversized buttons to the 4x3 grid
                    let w = v["w"].as_i64().unwrap_or(1);
                    let h = v["h"].as_i64().unwrap_or(1);
                    if x + w - 1 >= 4 {
                        v["w"] = json!(4 - x);
                    }
                    if y + h - 1 >= 3 {
                        v["h"] = json!(3 - y);
                    }
                    Some(v)
                }
            })
            .collect()
    }

    /// One button -> client shortcut object (ButtonStyle port).
    pub fn shortcut_payload(&self, b: &ButtonRow) -> Value {
        let props = self.resolver.props_for(&b.kind, b.command.as_deref());
        let command = transform_command(&b.kind, b.command.as_deref(), &props);
        let extra = extra_listener(&b.kind, b.command.as_deref(), b.mode.as_str(), &props);
        let mut app = props.app.clone();

        // icon from db overrides the type default; prefix follows the icon
        let (unicode, unicode2, prefix) = match b.icon.as_deref() {
            Some(icon) if !icon.is_empty() => (
                self.resolver.icon_unicode(icon, "fas").unwrap_or_default(),
                b.icon2
                    .as_deref()
                    .filter(|i| !i.is_empty())
                    .and_then(|i| self.resolver.icon_unicode(i, "fas"))
                    .unwrap_or_else(|| props.unicode.clone().unwrap_or_default()),
                "fas".to_string(),
            ),
            _ => (
                props.unicode.clone().unwrap_or_default(),
                props.unicode.clone().unwrap_or_default(),
                props.prefix.clone().unwrap_or_else(|| "fas".to_string()),
            ),
        };

        // original quirk: vol_mute buttons report their state under
        // the custom-value app with the "speaker-muted" key
        let mut extra = extra;
        if b.kind == "vol" && command == "vol_mute" {
            app = Some("custom-value".to_string());
            extra = "speaker-muted".to_string();
        }

        let mut o = Map::new();
        o.insert("id".into(), json!(b.id));
        o.insert("board_id".into(), json!(b.board_id));
        o.insert("type".into(), json!(b.kind));
        o.insert("command".into(), json!(command));
        insert_opt(
            &mut o,
            "color",
            b.color.clone(),
            props.color.clone(),
            FALLBACK_COLOR,
        );
        insert_opt(&mut o, "color2", b.color2.clone(), props.color.clone(), "");
        insert_opt(&mut o, "img", b.img.clone(), None, "");
        insert_opt(&mut o, "img2", b.img2.clone(), None, "");
        insert_opt(&mut o, "icon_color", b.icon_color.clone(), None, "");
        insert_opt(&mut o, "icon_color2", b.icon_color2.clone(), None, "");
        o.insert("position".into(), json!(b.position));
        o.insert("title".into(), json!(b.title));
        o.insert("title_position".into(), json!(b.title_position));
        o.insert("title_position2".into(), json!(b.title_position2));
        insert_opt(
            &mut o,
            "title_color",
            b.title_color.clone(),
            Some("#ffffff".into()),
            "#ffffff",
        );
        insert_opt(
            &mut o,
            "title_color2",
            b.title_color2.clone(),
            Some("#ffffff".into()),
            "#ffffff",
        );
        insert_opt(
            &mut o,
            "title_box_color",
            b.title_box_color.clone(),
            None,
            "",
        );
        insert_opt(
            &mut o,
            "title_box_color2",
            b.title_box_color2.clone(),
            None,
            "",
        );
        insert_opt(&mut o, "border_color", b.border_color.clone(), None, "");
        insert_opt(&mut o, "border_color2", b.border_color2.clone(), None, "");
        o.insert("options".into(), json!(b.options));
        o.insert("shape".into(), json!(b.shape));
        o.insert("shape2".into(), json!(b.shape2));
        o.insert("action".into(), json!(""));
        o.insert("target".into(), json!(""));
        o.insert("x".into(), json!(b.x.unwrap_or(0)));
        o.insert("y".into(), json!(b.y.unwrap_or(0)));
        o.insert("w".into(), json!(b.w));
        o.insert("h".into(), json!(b.h));
        o.insert("mode".into(), json!(b.mode));
        o.insert("unicode".into(), json!(unicode));
        o.insert("unicode2".into(), json!(unicode2));
        o.insert("prefix".into(), json!(prefix));
        o.insert("extra".into(), json!(extra));
        if let Some(a) = app {
            o.insert("app".into(), json!(a));
        } else if let Some(a) = &props.app {
            o.insert("app".into(), json!(a));
        }
        if let Some(tk) = &props.toggle_key {
            o.insert("toggle_key".into(), json!(tk));
        }
        Value::Object(o)
    }
}

impl Default for Mapper {
    fn default() -> Self {
        Self::new()
    }
}

fn insert_opt(
    o: &mut Map<String, Value>,
    key: &str,
    row: Option<String>,
    fallback: Option<String>,
    default: &str,
) {
    let v = row.or(fallback).unwrap_or_else(|| default.to_string());
    o.insert(key.into(), json!(v));
}

/// Command string transformation before sending (original mapper rules).
fn transform_command(kind: &str, command: Option<&str>, props: &Props) -> String {
    let raw = command.unwrap_or_default();
    if RAW_COMMAND_TYPES.contains(&kind) {
        return raw.to_string();
    }
    if let Some(json_key) = &props.json_key {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(extracted) = v.get(json_key).and_then(Value::as_str) {
                return extracted.to_string();
            }
        }
        return String::new();
    }
    String::new()
}

/// The `extra` field: which state key the client watches for toggles/graphs.
fn extra_listener(kind: &str, command: Option<&str>, mode: &str, props: &Props) -> String {
    let raw = command.unwrap_or_default();
    if props.json_key.is_some() {
        // jsonKey types use the type as the listener key
        return kind.to_string();
    }
    if mode == "graph" {
        return if raw.is_empty() {
            kind.to_string()
        } else {
            raw.to_string()
        };
    }
    match kind {
        "obs-scene" | "slobs-scene" => raw.to_string(),
        "obs-source" => parsed(raw, "scene") + "::" + &parsed(raw, "source"),
        "obs-filter" => {
            parsed(raw, "scene") + "::" + &parsed(raw, "source") + "::" + &parsed(raw, "filter")
        }
        "slobs-source" | "obs-device-audio" | "slobs-device-audio" => parsed(raw, "source"),
        "twitch-chat-box" => raw.to_string(),
        _ => {
            if mode == "slider" {
                if !raw.is_empty() {
                    format!("{kind}_{raw}")
                } else {
                    kind.to_string()
                }
            } else if props.app.as_deref() == Some("custom-value")
                || props.app.as_deref() == Some("third-party")
            {
                if !raw.is_empty() {
                    raw.to_string()
                } else {
                    kind.to_string()
                }
            } else {
                String::new()
            }
        }
    }
}

fn parsed(raw: &str, key: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> BoardRow {
        BoardRow {
            id: 1,
            name: "My Board".into(),
            background: "#2c3e50".into(),
            layout: 6,
            image: String::new(),
            sort: 0,
            kind: "buttons".into(),
            args: None,
            order: 0,
            width: 6,
            height: 5,
            converted: 1,
        }
    }

    fn button(kind: &str, command: Option<&str>, x: i64, y: i64, w: i64, h: i64) -> ButtonRow {
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
            mode: "button".into(),
            x: Some(x),
            y: Some(y),
            w,
            h,
            options: None,
        }
    }

    #[test]
    fn graph_mode_extra_falls_back_to_type() {
        let m = Mapper::new();
        let mut b = button("twitch-follower-count", None, 0, 0, 2, 1);
        b.mode = "graph".into();
        let s = m.shortcut_payload(&b);
        assert_eq!(s["extra"], "twitch-follower-count");
        let mut b = button("custom-metric", Some("my-key"), 0, 0, 2, 1);
        b.mode = "graph".into();
        let s = m.shortcut_payload(&b);
        assert_eq!(s["extra"], "my-key");
    }

    #[test]
    fn fillers_cover_every_free_cell() {
        let m = Mapper::new();
        let b = board();
        let payload = m.board_payload(&b, &[button("url", Some("https://x.co"), 0, 0, 1, 1)], true);
        let sc = payload["shortcuts"].as_array().unwrap();
        let fillers = sc.iter().filter(|s| s["id"].is_null()).count();
        assert_eq!(fillers, (6 * 5 - 1) as usize);
        assert!(payload["staggered"].as_bool().unwrap());
        // sorted by y then x
        let first = sc.first().unwrap();
        assert_eq!(
            (first["x"].as_i64(), first["y"].as_i64()),
            (Some(0), Some(0))
        );
    }

    #[test]
    fn basic_variant_crops_to_4x3() {
        let m = Mapper::new();
        let b = board();
        let buttons = [
            button("url", Some("https://x.co"), 0, 0, 1, 1),
            button("url", Some("https://x.co"), 3, 2, 4, 4), // oversized -> cropped
            button("url", Some("https://x.co"), 5, 4, 1, 1), // outside 4x3 -> dropped
        ];
        let basic = m.board_payload(&b, &buttons, false);
        let cropped = basic["shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_i64() == Some(10) && s["x"].as_i64() == Some(3))
            .unwrap();
        assert_eq!(
            (cropped["w"].as_i64(), cropped["h"].as_i64()),
            (Some(1), Some(1))
        );
        assert_eq!(
            basic["shortcuts"].as_array().unwrap().len(),
            4 * 3 // full 4x3 grid incl. fillers
        );
    }

    #[test]
    fn command_transformation_rules() {
        let m = Mapper::new();
        // raw types pass through
        let s = m.shortcut_payload(&button("vol", Some("vol_mute"), 0, 0, 1, 1));
        assert_eq!(s["command"], "vol_mute");
        assert_eq!(s["extra"], "speaker-muted");
        assert_eq!(s["app"], "custom-value");
        // jsonKey extraction
        let s = m.shortcut_payload(&button(
            "speaker-device",
            Some(r#"{"speaker":"dev-1"}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["command"], "dev-1");
        assert_eq!(s["extra"], "speaker-device");
        // everything else is blanked
        let s = m.shortcut_payload(&button("type", Some("hello"), 0, 0, 1, 1));
        assert_eq!(s["command"], "");
        // obs-source listener key
        let s = m.shortcut_payload(&button(
            "obs-source",
            Some(r#"{"scene":"S","source":"C","group":null}"#),
            0,
            0,
            1,
            1,
        ));
        assert_eq!(s["extra"], "S::C");
    }

    #[test]
    fn defaults_match_original() {
        let m = Mapper::new();
        let s = m.shortcut_payload(&button("key", Some("CTRL + K"), 0, 0, 1, 1));
        assert_eq!(s["color"], "#EF4836");
        assert_eq!(s["title_color"], "#ffffff");
        assert_eq!(s["mode"], "button");
        assert_eq!(s["action"], "");
        assert_eq!(s["prefix"], "fas");
        assert!(!s["unicode"].as_str().unwrap().is_empty());
    }
}
