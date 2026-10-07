//! In-memory OBS state, the event/response reducer and the snapshot the
//! host forwards through [`pulpit_host::forward_producer`].
//!
//! The snapshot carries two families of keys, because the client lanes
//! watch different things:
//!
//! - semantic keys (`obs-scene`, `obs-source`, ...) that the desktop's
//!   `STATE_BINDINGS` compare against the tile's own command fields;
//! - one key per legacy `extra` watch string (`{"scene":"Game"}`,
//!   `::Webcam`, `::::Blur`, ...), so the v2 channels (`ext.<extra>`) and
//!   any stock client that matches `extra` against custom values light
//!   the same tiles. The exact spellings live in
//!   `crates/legacy/src/mapping.rs::extra_listener` - this module pushes
//!   both the value families those keys compare against.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Which tile argument a pending response belongs to. obs-websocket
/// echoes only some fields back (GetInputMute answers with
/// `inputMuted` but no `inputName`), so the query remembers it.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryCtx {
    None,
    /// Per-input queries (mute / volume / filter list): the input name.
    Input(String),
    /// GetSceneItemList: the scene the items belong to.
    Scene(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObsState {
    pub connected: bool,
    pub auth_failed: bool,
    pub obs_version: Option<String>,
    pub current_scene: Option<String>,
    pub scenes: Vec<String>,
    pub studio_mode: bool,
    pub recording: bool,
    pub streaming: bool,
    pub replay_buffer_active: bool,
    // (scene_name, source_name) -> is_enabled, per known scene item
    pub scene_items: HashMap<(String, String), bool>,
    // (scene_name, scene_item_id) -> source_name; the enable-state EVENT
    // carries only the numeric id, so the list responses must have run
    pub scene_item_ids: HashMap<(String, String), String>,
    // input_name -> is_muted
    pub input_mutes: HashMap<String, bool>,
    // input_name -> volume_mul (0.0..1.0)
    pub input_volumes: HashMap<String, f64>,
    // (source_name, filter_name) -> is_enabled
    pub source_filters: HashMap<(String, String), bool>,
}

impl ObsState {
    pub fn reset_connection(&mut self) {
        self.connected = false;
        self.auth_failed = false;
        self.obs_version = None;
    }

    pub fn set_connected(&mut self, version: String) {
        self.connected = true;
        self.auth_failed = false;
        self.obs_version = Some(version);
    }

    pub fn set_auth_failed(&mut self) {
        self.connected = false;
        self.auth_failed = true;
    }

    /// Drop everything learned about the OBS session (scenes, items,
    /// inputs, filters, output flags). Called when the connection goes
    /// away or OBS is disabled, so a reconnect cannot inherit stale
    /// tiles as truth.
    pub fn clear_session(&mut self) {
        self.current_scene = None;
        self.scenes.clear();
        self.studio_mode = false;
        self.recording = false;
        self.streaming = false;
        self.replay_buffer_active = false;
        self.scene_items.clear();
        self.scene_item_ids.clear();
        self.input_mutes.clear();
        self.input_volumes.clear();
        self.source_filters.clear();
    }

    /// Does the scene-item id map know this item? `false` means the
    /// enable-state event cannot be resolved and the caller must refresh
    /// the scene's item list first.
    pub fn knows_scene_item(&self, scene: &str, item_id: &str) -> bool {
        self.scene_item_ids
            .contains_key(&(scene.to_string(), item_id.to_string()))
    }

    /// Store one scene item's enabled flag (and its id mapping).
    fn set_scene_item(&mut self, scene: &str, item_id: &str, source: &str, enabled: bool) -> bool {
        let id_key = (scene.to_string(), item_id.to_string());
        let known = self.scene_item_ids.get(&id_key) == Some(&source.to_string());
        self.scene_item_ids.insert(id_key, source.to_string());
        let key = (scene.to_string(), source.to_string());
        let changed = self.scene_items.get(&key) != Some(&enabled) || !known;
        self.scene_items.insert(key, enabled);
        changed
    }

    /// Process one OBS event. Returns whether observable state changed
    /// (and therefore a snapshot should go out). Unresolvable events are
    /// dropped silently - the caller refreshes the backing list instead.
    pub fn handle_event(&mut self, event_type: &str, data: &Value) -> bool {
        match event_type {
            "CurrentProgramSceneChanged" => {
                if let Some(scene) = data.get("sceneName").and_then(Value::as_str) {
                    if self.current_scene.as_deref() != Some(scene) {
                        self.current_scene = Some(scene.to_string());
                        return true;
                    }
                }
            }
            "SceneListChanged" => {
                if let Some(scenes) = data.get("scenes").and_then(Value::as_array) {
                    let new_scenes: Vec<String> = scenes
                        .iter()
                        .filter_map(|s| s.get("sceneName").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect();
                    if self.scenes != new_scenes {
                        self.scenes = new_scenes;
                        return true;
                    }
                }
            }
            "SceneItemEnableStateChanged" => {
                if let (Some(scene), Some(id), Some(enabled)) = (
                    data.get("sceneName").and_then(Value::as_str),
                    data.get("sceneItemId").and_then(Value::as_i64),
                    data.get("sceneItemEnabled").and_then(Value::as_bool),
                ) {
                    let id = id.to_string();
                    if let Some(source) = self
                        .scene_item_ids
                        .get(&(scene.to_string(), id.clone()))
                        .cloned()
                    {
                        return self.set_scene_item(scene, &id, &source, enabled);
                    }
                }
            }
            "InputMuteStateChanged" => {
                if let (Some(input), Some(muted)) = (
                    data.get("inputName").and_then(Value::as_str),
                    data.get("inputMuted").and_then(Value::as_bool),
                ) {
                    if self.input_mutes.get(input) != Some(&muted) {
                        self.input_mutes.insert(input.to_string(), muted);
                        return true;
                    }
                }
            }
            // v5 spells this InputVolumeStateChanged (not InputVolumeChanged)
            "InputVolumeStateChanged" => {
                if let (Some(input), Some(mul)) = (
                    data.get("inputName").and_then(Value::as_str),
                    data.get("inputVolumeMul").and_then(Value::as_f64),
                ) {
                    let rounded = round_mul(mul);
                    if self.input_volumes.get(input) != Some(&rounded) {
                        self.input_volumes.insert(input.to_string(), rounded);
                        return true;
                    }
                }
            }
            "SourceFilterEnableStateChanged" => {
                if let (Some(source), Some(filter), Some(enabled)) = (
                    data.get("sourceName").and_then(Value::as_str),
                    data.get("filterName").and_then(Value::as_str),
                    data.get("filterEnabled").and_then(Value::as_bool),
                ) {
                    let key = (source.to_string(), filter.to_string());
                    if self.source_filters.get(&key) != Some(&enabled) {
                        self.source_filters.insert(key, enabled);
                        return true;
                    }
                }
            }
            "StudioModeStateChanged" => {
                if let Some(enabled) = data.get("studioModeEnabled").and_then(Value::as_bool) {
                    if self.studio_mode != enabled {
                        self.studio_mode = enabled;
                        return true;
                    }
                }
            }
            "RecordStateChanged" => {
                if let Some(active) = data.get("outputActive").and_then(Value::as_bool) {
                    if self.recording != active {
                        self.recording = active;
                        return true;
                    }
                }
            }
            "StreamStateChanged" => {
                if let Some(active) = data.get("outputActive").and_then(Value::as_bool) {
                    if self.streaming != active {
                        self.streaming = active;
                        return true;
                    }
                }
            }
            "ReplayBufferStateChanged" => {
                if let Some(active) = data.get("outputActive").and_then(Value::as_bool) {
                    if self.replay_buffer_active != active {
                        self.replay_buffer_active = active;
                        return true;
                    }
                }
            }
            _ => {}
        }
        false
    }

    /// Process one request response that carried session state. The
    /// initial sync fires the same queries after every reconnect, so
    /// both `init` and user-issued responses feed this. Returns whether
    /// a snapshot should be pushed.
    pub fn handle_response(&mut self, request_type: &str, ctx: &QueryCtx, data: &Value) -> bool {
        match request_type {
            "GetCurrentProgramScene" => {
                match data
                    .get("currentProgramSceneName")
                    .and_then(Value::as_str)
                {
                    Some(scene) if self.current_scene.as_deref() != Some(scene) => {
                        self.current_scene = Some(scene.to_string());
                        true
                    }
                    _ => false,
                }
            }
            "GetSceneList" => {
                let mut changed = false;
                if let Some(scenes) = data.get("scenes").and_then(Value::as_array) {
                    let new_scenes: Vec<String> = scenes
                        .iter()
                        .filter_map(|s| s.get("sceneName").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect();
                    if self.scenes != new_scenes {
                        self.scenes = new_scenes;
                        changed = true;
                    }
                }
                if let Some(scene) = data.get("currentProgramSceneName").and_then(Value::as_str) {
                    if self.current_scene.as_deref() != Some(scene) {
                        self.current_scene = Some(scene.to_string());
                        changed = true;
                    }
                }
                changed
            }
            "GetSceneItemList" => {
                let QueryCtx::Scene(scene) = ctx else {
                    return false;
                };
                let mut changed = false;
                if let Some(items) = data.get("sceneItems").and_then(Value::as_array) {
                    for item in items {
                        if let (Some(id), Some(source)) = (
                            item.get("sceneItemId").and_then(Value::as_i64),
                            item.get("sourceName").and_then(Value::as_str),
                        ) {
                            let enabled = item
                                .get("sceneItemEnabled")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            changed |= self.set_scene_item(scene, &id.to_string(), source, enabled);
                        }
                    }
                }
                changed
            }
            "GetInputMute" => {
                let QueryCtx::Input(input) = ctx else {
                    return false;
                };
                let muted = data
                    .get("inputMuted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.input_mutes.get(input) != Some(&muted) {
                    self.input_mutes.insert(input.clone(), muted);
                    return true;
                }
                false
            }
            "GetInputVolume" => {
                let QueryCtx::Input(input) = ctx else {
                    return false;
                };
                if let Some(mul) = data.get("inputVolumeMul").and_then(Value::as_f64) {
                    let rounded = round_mul(mul);
                    if self.input_volumes.get(input) != Some(&rounded) {
                        self.input_volumes.insert(input.clone(), rounded);
                        return true;
                    }
                }
                false
            }
            "GetSourceFilterList" => {
                let QueryCtx::Input(source) = ctx else {
                    return false;
                };
                let mut changed = false;
                if let Some(filters) = data.get("filters").and_then(Value::as_array) {
                    for flt in filters {
                        if let (Some(name), Some(enabled)) = (
                            flt.get("filterName").and_then(Value::as_str),
                            flt.get("filterEnabled").and_then(Value::as_bool),
                        ) {
                            let key = (source.clone(), name.to_string());
                            if self.source_filters.get(&key) != Some(&enabled) {
                                self.source_filters.insert(key, enabled);
                                changed = true;
                            }
                        }
                    }
                }
                changed
            }
            "GetStudioModeEnabled" => {
                let enabled = data
                    .get("studioModeEnabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.studio_mode != enabled {
                    self.studio_mode = enabled;
                    return true;
                }
                false
            }
            "GetRecordStatus" => {
                let active = data
                    .get("outputActive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.recording != active {
                    self.recording = active;
                    return true;
                }
                false
            }
            "GetStreamStatus" => {
                let active = data
                    .get("outputActive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.streaming != active {
                    self.streaming = active;
                    return true;
                }
                false
            }
            // GetInputList only seeds the per-input queries (the worker
            // fans out); it stores nothing itself.
            _ => false,
        }
    }

    /// The live-state snapshot. Keys are documented at the module head.
    pub fn to_snapshot(&self) -> Value {
        let mut map = serde_json::Map::new();

        // semantic keys: what the desktop STATE_BINDINGS compare
        let current = self.current_scene.clone().unwrap_or_default();
        map.insert("obs-scene".into(), Value::String(current.clone()));
        map.insert("obs-studio-mode".into(), Value::Bool(self.studio_mode));
        map.insert("obs-record".into(), Value::Bool(self.recording));
        map.insert("obs-stream".into(), Value::Bool(self.streaming));

        let mut enabled_sources = Vec::new();
        if let Some(cur) = self.current_scene.as_deref() {
            for ((scene, source), enabled) in &self.scene_items {
                if scene == cur && *enabled {
                    enabled_sources.push(Value::String(source.clone()));
                }
            }
        }
        map.insert("obs-source".into(), Value::Array(enabled_sources));

        let mut muted_inputs = Vec::new();
        for (input, muted) in &self.input_mutes {
            if *muted {
                muted_inputs.push(Value::String(input.clone()));
            }
        }
        map.insert("obs-device-audio".into(), Value::Array(muted_inputs));

        let mut enabled_filters: Vec<String> = Vec::new();
        for ((_, filter), enabled) in &self.source_filters {
            if *enabled && !enabled_filters.contains(filter) {
                enabled_filters.push(filter.clone());
            }
        }
        map.insert(
            "obs-filter".into(),
            Value::Array(enabled_filters.into_iter().map(Value::String).collect()),
        );

        // per-tile watch keys: the exact legacy `extra` strings
        for scene in &self.scenes {
            let is_current = Some(scene.as_str()) == self.current_scene.as_deref();
            // obs-scene tiles watch the raw command JSON (mapping.rs)
            map.insert(json_scene_arg(scene), Value::Bool(is_current));
        }
        if let Some(cur) = self.current_scene.as_deref() {
            for ((scene, source), enabled) in &self.scene_items {
                if scene == cur {
                    // obs-source tiles watch "<scene>::<source>", scene
                    // empty in the stored payloads -> "::Webcam"
                    map.insert(format!("::{source}"), Value::Bool(*enabled));
                }
            }
        }
        for (input, muted) in &self.input_mutes {
            // obs-device-audio tiles watch the device name (mapping.rs
            // falls back to the `device` field)
            map.insert(input.clone(), Value::Bool(*muted));
        }
        for (input, volume) in &self.input_volumes {
            // the desktop slider reads the simple form...
            map.insert(format!("obs-audio-slider_{input}"), serde_json::json!(volume));
            // ...the v2 channel is the slider kind's extra spelling,
            // "<kind>_<raw command JSON>"
            map.insert(
                format!("obs-audio-slider_{}", json_device_arg(input)),
                serde_json::json!(volume),
            );
        }
        for ((source, filter), enabled) in &self.source_filters {
            // obs-filter tiles watch "<scene>::<source>::<filter>";
            // stored payloads leave scene (and often source) empty, so
            // both spellings go out
            map.insert(format!("::::{filter}"), Value::Bool(*enabled));
            map.insert(format!("::{source}::{filter}"), Value::Bool(*enabled));
        }

        Value::Object(map)
    }

    /// The "OBS is gone" snapshot: every key this state might have
    /// pushed goes out with its inactive value, so no client keeps a
    /// stale lit tile. Clears the cached session afterwards.
    pub fn take_reset_snapshot(&mut self) -> Value {
        let mut map = self.to_snapshot();
        if let Some(obj) = map.as_object_mut() {
            obj.insert("obs-scene".into(), Value::String(String::new()));
            obj.insert("obs-source".into(), Value::Array(Vec::new()));
            obj.insert("obs-device-audio".into(), Value::Array(Vec::new()));
            obj.insert("obs-filter".into(), Value::Array(Vec::new()));
            obj.insert("obs-studio-mode".into(), Value::Bool(false));
            obj.insert("obs-record".into(), Value::Bool(false));
            obj.insert("obs-stream".into(), Value::Bool(false));
            for key in obj.keys().cloned().collect::<Vec<_>>() {
                // every remaining per-tile key is a boolean or a volume;
                // volumes drop to 0 like a dead fader
                let value = obj.get(&key).cloned().unwrap_or(Value::Null);
                match value {
                    Value::Bool(_) => {
                        obj.insert(key, Value::Bool(false));
                    }
                    Value::Number(_) => {
                        obj.insert(key, serde_json::json!(0.0));
                    }
                    _ => {}
                }
            }
        }
        self.clear_session();
        map
    }
}

/// The stored command JSON of an obs-scene tile as the editor writes it
/// (`{"scene":"Game"}`) - the legacy extra watch string for those tiles.
fn json_scene_arg(scene: &str) -> String {
    serde_json::json!({ "scene": scene }).to_string()
}

/// Same for the audio slider kind (`{"device":"Mic"}`).
fn json_device_arg(input: &str) -> String {
    serde_json::json!({ "device": input }).to_string()
}

/// Volume multiples are rounded to 3 decimals like the wire values, so
/// a repeated identical push stays identical for the change gate.
fn round_mul(mul: f64) -> f64 {
    (mul * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed() -> ObsState {
        let mut state = ObsState {
            scenes: vec!["Intro".into(), "Game".into()],
            current_scene: Some("Game".into()),
            ..Default::default()
        };
        // GetSceneItemList answer for the current scene: Webcam visible,
        // Overlay hidden, ids recorded for the event path
        state.handle_response(
            "GetSceneItemList",
            &QueryCtx::Scene("Game".into()),
            &json!({
                "sceneName": "Game",
                "sceneItems": [
                    { "sceneItemId": 7, "sourceName": "Webcam", "sceneItemEnabled": true },
                    { "sceneItemId": 8, "sourceName": "Overlay", "sceneItemEnabled": false }
                ]
            }),
        );
        state
    }

    #[test]
    fn state_reducer_recorded_samples() {
        let mut state = seed();

        // CurrentProgramSceneChanged (recorded event sample)
        assert!(state.handle_event("CurrentProgramSceneChanged", &json!({ "sceneName": "Intro" })));
        assert_eq!(state.current_scene.as_deref(), Some("Intro"));
        assert!(!state.handle_event("CurrentProgramSceneChanged", &json!({ "sceneName": "Intro" })));

        // StudioModeStateChanged
        assert!(state.handle_event("StudioModeStateChanged", &json!({ "studioModeEnabled": true })));
        assert!(state.studio_mode);

        // RecordStateChanged (v5 outputActive + outputState)
        assert!(state.handle_event(
            "RecordStateChanged",
            &json!({ "outputActive": true, "outputState": "OBS_WEBSOCKET_OUTPUT_STARTED" })
        ));
        assert!(state.recording);

        // InputMuteStateChanged
        assert!(state.handle_event(
            "InputMuteStateChanged",
            &json!({ "inputName": "Mic/Aux", "inputMuted": true })
        ));
        assert_eq!(state.input_mutes.get("Mic/Aux"), Some(&true));

        // InputVolumeStateChanged (the v5 spelling) with the mul rounding
        assert!(state.handle_event(
            "InputVolumeStateChanged",
            &json!({ "inputName": "Mic/Aux", "inputVolumeMul": 0.75, "inputVolumeDb": -2.5 })
        ));
        assert_eq!(state.input_volumes.get("Mic/Aux"), Some(&0.75));

        // SourceFilterEnableStateChanged
        assert!(state.handle_event(
            "SourceFilterEnableStateChanged",
            &json!({ "sourceName": "Webcam", "filterName": "ColorCorrection", "filterEnabled": true })
        ));
        assert_eq!(
            state.source_filters.get(&("Webcam".into(), "ColorCorrection".into())),
            Some(&true)
        );

        // SceneItemEnableStateChanged resolves through the recorded ids
        assert!(state.knows_scene_item("Game", "7"));
        assert!(state.handle_event(
            "SceneItemEnableStateChanged",
            &json!({ "sceneName": "Game", "sceneItemId": 7, "sceneItemEnabled": false })
        ));
        assert_eq!(state.scene_items.get(&("Game".into(), "Webcam".into())), Some(&false));

        // unknown id: dropped, caller refreshes
        assert!(!state.knows_scene_item("Game", "99"));
        assert!(!state.handle_event(
            "SceneItemEnableStateChanged",
            &json!({ "sceneName": "Game", "sceneItemId": 99, "sceneItemEnabled": true })
        ));
    }

    #[test]
    fn snapshot_carries_semantic_and_per_tile_keys() {
        let mut state = seed();
        state.current_scene = Some("Game".into());
        state.studio_mode = true;
        state.recording = true;
        state.input_mutes.insert("Mic/Aux".into(), true);
        state.input_volumes.insert("Mic/Aux".into(), 0.75);
        state
            .source_filters
            .insert(("Webcam".into(), "Blur".into()), true);

        let snap = state.to_snapshot();
        // semantic keys (desktop bindings)
        assert_eq!(snap["obs-scene"], "Game");
        assert_eq!(snap["obs-studio-mode"], true);
        assert_eq!(snap["obs-record"], true);
        assert_eq!(snap["obs-stream"], false);
        assert_eq!(snap["obs-source"], json!(["Webcam"]));
        assert_eq!(snap["obs-device-audio"], json!(["Mic/Aux"]));
        assert_eq!(snap["obs-filter"], json!(["Blur"]));
        // per-tile keys (legacy extra spellings)
        assert_eq!(snap[json!({"scene": "Game"}).to_string()], true);
        assert_eq!(snap[json!({"scene": "Intro"}).to_string()], false);
        assert_eq!(snap["::Webcam"], true);
        assert_eq!(snap["::Overlay"], false);
        assert_eq!(snap["Mic/Aux"], true);
        assert_eq!(snap["obs-audio-slider_Mic/Aux"], 0.75);
        assert_eq!(
            snap[format!("obs-audio-slider_{}", json!({"device": "Mic/Aux"}))],
            0.75
        );
        assert_eq!(snap["::::Blur"], true);
        assert_eq!(snap["::Webcam::Blur"], true);
    }

    #[test]
    fn reset_snapshot_zeroes_every_pushed_key_and_clears_the_session() {
        let mut state = seed();
        state.input_mutes.insert("Mic".into(), true);
        state.input_volumes.insert("Mic".into(), 0.4);
        state.recording = true;

        let snap = state.take_reset_snapshot();
        assert_eq!(snap["obs-scene"], "");
        assert_eq!(snap["obs-source"], json!([]));
        assert_eq!(snap["obs-device-audio"], json!([]));
        assert_eq!(snap["obs-filter"], json!([]));
        assert_eq!(snap["obs-record"], false);
        assert_eq!(snap[json!({"scene": "Game"}).to_string()], false);
        assert_eq!(snap["::Webcam"], false);
        assert_eq!(snap["Mic"], false);
        assert_eq!(snap[format!("obs-audio-slider_{}", json!({"device": "Mic"}))], 0.0);
        // the session data is gone: a reconnect re-seeds from scratch
        assert!(state.scenes.is_empty());
        assert!(state.scene_items.is_empty());
        assert!(!state.recording);
    }

    #[test]
    fn responses_seed_the_initial_state() {
        let mut state = ObsState::default();
        assert!(state.handle_response(
            "GetCurrentProgramScene",
            &QueryCtx::None,
            &json!({ "currentProgramSceneName": "Game" })
        ));
        assert_eq!(state.current_scene.as_deref(), Some("Game"));
        assert!(state.handle_response(
            "GetSceneList",
            &QueryCtx::None,
            &json!({ "currentProgramSceneName": "Game", "scenes": [{ "sceneName": "Game" }] })
        ));
        assert_eq!(state.scenes, vec!["Game".to_string()]);
        assert!(state.handle_response(
            "GetStudioModeEnabled",
            &QueryCtx::None,
            &json!({ "studioModeEnabled": true })
        ));
        assert!(state.handle_response(
            "GetRecordStatus",
            &QueryCtx::None,
            &json!({ "outputActive": true })
        ));
        // the ctx-carrying input responses (no inputName echo on the wire)
        assert!(state.handle_response(
            "GetInputMute",
            &QueryCtx::Input("Mic/Aux".into()),
            &json!({ "inputMuted": true })
        ));
        assert_eq!(state.input_mutes.get("Mic/Aux"), Some(&true));
        assert!(state.handle_response(
            "GetInputVolume",
            &QueryCtx::Input("Mic/Aux".into()),
            &json!({ "inputVolumeMul": 0.5, "inputVolumeDb": -6.0 })
        ));
        assert_eq!(state.input_volumes.get("Mic/Aux"), Some(&0.5));
        assert!(state.handle_response(
            "GetSourceFilterList",
            &QueryCtx::Input("Webcam".into()),
            &json!({ "filters": [{ "filterName": "Blur", "filterEnabled": false }] })
        ));
        assert_eq!(
            state.source_filters.get(&("Webcam".into(), "Blur".into())),
            Some(&false)
        );
    }
}
