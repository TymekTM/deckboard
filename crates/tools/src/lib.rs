//! Native utility tiles (tools): clock, timer, stopwatch, counter.
//!
//! - State lives on the server so every tablet and desktop touch mode agree.
//! - State is persisted to `~/pulpitApp/tools.json` via [`pulpit_db::write_atomic`].
//! - Compact state (`{running, startedAtMs, elapsedMs, durationMs, count}`) is
//!   pushed through `pulpit_host::forward_producer` on change only.
//! - For the legacy client, formatted labels are pushed at 1 Hz only while a
//!   timer/stopwatch runs and a legacy client is connected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub mod state;
#[cfg(test)]
mod tests;

pub use state::{
    parse_duration_ms, Clock, CounterState, FinishAction, StopwatchState, SystemClock, TimerState,
    ToolKind, ToolState,
};

/// Tool action gestures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGesture {
    Tap,
    DoubleTap,
    LongPress,
    Reset,
}

impl std::str::FromStr for ToolGesture {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "double-tap" => ToolGesture::DoubleTap,
            "long-press" => ToolGesture::LongPress,
            "reset" => ToolGesture::Reset,
            _ => ToolGesture::Tap,
        })
    }
}

/// The persistent tools store on disk: `~/pulpitApp/tools.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DiskStore {
    #[serde(default)]
    pub tools: HashMap<String, ToolState>,
}

impl DiskStore {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => DiskStore::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        pulpit_db::write_atomic(path, &bytes)
    }
}

/// Shared tools manager.
pub struct ToolManager {
    store_path: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<HashMap<i64, ToolState>>,
    change_tx: UnboundedSender<Value>,
    board_tx: Mutex<Option<UnboundedSender<i64>>>,
}

impl ToolManager {
    pub fn new(
        store_path: PathBuf,
        clock: Arc<dyn Clock>,
        change_tx: UnboundedSender<Value>,
    ) -> Self {
        let loaded = DiskStore::load(&store_path);
        let mut map = HashMap::new();
        for (k, v) in loaded.tools {
            if let Ok(id) = k.parse::<i64>() {
                map.insert(id, v);
            }
        }
        Self {
            store_path,
            clock,
            state: Mutex::new(map),
            change_tx,
            board_tx: Mutex::new(None),
        }
    }

    pub fn set_board_sink(&self, tx: UnboundedSender<i64>) {
        *self.board_tx.lock().unwrap() = Some(tx);
    }

    /// Persist current tools state to disk.
    pub fn persist(&self) {
        let snapshot = {
            let guard = self.state.lock().unwrap();
            let mut store = DiskStore::default();
            for (&id, tool) in guard.iter() {
                store.tools.insert(id.to_string(), tool.clone());
            }
            store
        };
        if let Err(e) = snapshot.save(&self.store_path) {
            tracing::warn!(path = %self.store_path.display(), error = %e, "failed to save tools.json");
        }
    }

    /// Lazily drop entries for deleted buttons.
    pub fn clean_deleted(&self, known_button_ids: &[i64]) {
        let mut changed = false;
        {
            let mut guard = self.state.lock().unwrap();
            let before = guard.len();
            guard.retain(|id, _| known_button_ids.contains(id));
            if guard.len() != before {
                changed = true;
            }
        }
        if changed {
            self.persist();
        }
    }

    /// Execute an interaction gesture on a tool tile.
    pub fn execute(
        &self,
        button_id: i64,
        kind: &str,
        command: Option<&str>,
        gesture: ToolGesture,
    ) -> bool {
        let now = self.clock.now_ms();
        let push_val: Option<Value>;

        {
            let mut guard = self.state.lock().unwrap();
            let tool = guard
                .entry(button_id)
                .or_insert_with(|| ToolState::from_config(kind, command));

            // update configuration if changed in editor
            tool.update_config(kind, command);

            match tool {
                ToolState::Timer(t) => {
                    match gesture {
                        ToolGesture::Tap => {
                            t.tap(now);
                        }
                        ToolGesture::DoubleTap | ToolGesture::LongPress | ToolGesture::Reset => {
                            t.reset();
                        }
                    }
                    push_val = Some(t.compact_state(now));
                }
                ToolState::Stopwatch(s) => {
                    match gesture {
                        ToolGesture::Tap => {
                            s.tap(now);
                        }
                        ToolGesture::DoubleTap | ToolGesture::LongPress | ToolGesture::Reset => {
                            s.reset();
                        }
                    }
                    push_val = Some(s.compact_state(now));
                }
                ToolState::Counter(c) => {
                    match gesture {
                        ToolGesture::Tap => {
                            c.tap();
                        }
                        ToolGesture::DoubleTap => {
                            c.double_tap();
                        }
                        ToolGesture::LongPress | ToolGesture::Reset => {
                            c.reset();
                        }
                    }
                    push_val = Some(c.compact_state());
                }
                ToolState::Clock => {
                    return true;
                }
            }
        }

        self.persist();

        if let Some(val) = push_val {
            let key = format!("tool-{button_id}");
            let _ = self.change_tx.send(json!({ key: val }));
        }

        true
    }

    /// Get current compact state for a button.
    pub fn get_state(&self, button_id: i64) -> Option<Value> {
        let now = self.clock.now_ms();
        let guard = self.state.lock().unwrap();
        guard.get(&button_id).map(|t| t.compact_state(now))
    }

    /// Background tick for checking running timers and finishing them.
    pub fn tick_timers(&self) {
        let now = self.clock.now_ms();
        let mut finished_actions = Vec::new();
        let mut finished_pushes = Vec::new();

        {
            let mut guard = self.state.lock().unwrap();
            for (&id, tool) in guard.iter_mut() {
                if let ToolState::Timer(t) = tool {
                    if let Some(action) = t.tick(now) {
                        finished_actions.push(action);
                        finished_pushes.push((id, t.compact_state(now)));
                    }
                }
            }
        }

        if !finished_pushes.is_empty() {
            self.persist();
            for (id, val) in finished_pushes {
                let key = format!("tool-{id}");
                let _ = self.change_tx.send(json!({ key: val }));
            }
        }

        for action in finished_actions {
            self.handle_finish_action(action);
        }
    }

    /// Check if any timer or stopwatch is currently running.
    pub fn has_running_tools(&self) -> bool {
        let guard = self.state.lock().unwrap();
        guard.values().any(|tool| match tool {
            ToolState::Timer(t) => t.running,
            ToolState::Stopwatch(s) => s.running,
            _ => false,
        })
    }

    /// Produce legacy 1 Hz formatted labels for all running timers/stopwatches.
    pub fn legacy_formatted_labels(&self) -> HashMap<String, String> {
        let now = self.clock.now_ms();
        let guard = self.state.lock().unwrap();
        let mut map = HashMap::new();
        for (&id, tool) in guard.iter() {
            match tool {
                ToolState::Timer(t) if t.running => {
                    map.insert(format!("tool-{id}"), t.format_legacy(now));
                }
                ToolState::Stopwatch(s) if s.running => {
                    map.insert(format!("tool-{id}"), s.format_legacy(now));
                }
                _ => {}
            }
        }
        map
    }

    fn handle_finish_action(&self, action: FinishAction) {
        match action {
            FinishAction::None => {}
            FinishAction::Play(path) => {
                if !path.is_empty() {
                    if let Err(e) = pulpit_os::play_audio(&path) {
                        tracing::warn!(path = %path, error = %e, "timer sound playback failed");
                    }
                }
            }
            FinishAction::Board(board_id) => {
                if let Some(sink) = self.board_tx.lock().unwrap().as_ref() {
                    let _ = sink.send(board_id);
                }
            }
        }
    }
}

/// Spawns the tool manager service and its background loops.
/// Returns `(Arc<ToolManager>, UnboundedReceiver<Value>)`.
pub fn spawn_tools(
    store_path: PathBuf,
    legacy_hub: Arc<pulpit_legacy::Hub>,
) -> (Arc<ToolManager>, UnboundedReceiver<Value>) {
    let (tx, rx) = unbounded_channel();
    let clock = Arc::new(SystemClock);
    let manager = Arc::new(ToolManager::new(store_path, clock, tx));

    // Background timer tick loop (every 100 ms to catch timer finish promptly)
    let tick_mgr = manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            tick_mgr.tick_timers();
        }
    });

    // Background legacy 1 Hz push loop
    let legacy_mgr = manager.clone();
    let legacy_hub_clone = legacy_hub.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            // Only while a timer/stopwatch runs AND a legacy client is connected
            let legacy_connected = legacy_hub_clone.try_len().unwrap_or(0) > 0;
            if legacy_connected && legacy_mgr.has_running_tools() {
                let labels = legacy_mgr.legacy_formatted_labels();
                if !labels.is_empty() {
                    let data = serde_json::to_value(&labels).unwrap_or(Value::Null);
                    let payload = json!({
                        "app": "APP_CUSTOM_VALUE",
                        "data": data,
                    })
                    .to_string();
                    legacy_hub_clone
                        .broadcast("app_status_update", Some(&payload))
                        .await;
                }
            }
        }
    });

    (manager, rx)
}

/// Checks if an action kind is a native tool kind.
pub fn is_tool_action(kind: &str) -> bool {
    matches!(
        kind,
        "tool-clock" | "tool-timer" | "tool-stopwatch" | "tool-counter"
    )
}

/// Declarations for legacy mapper inputs.
pub fn input_declarations() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    vec![
        ("tool-clock", "clock", "#34495e", Some("custom-value")),
        (
            "tool-timer",
            "hourglass-half",
            "#34495e",
            Some("custom-value"),
        ),
        (
            "tool-stopwatch",
            "stopwatch",
            "#34495e",
            Some("custom-value"),
        ),
        (
            "tool-counter",
            "calculator",
            "#34495e",
            Some("custom-value"),
        ),
    ]
}
