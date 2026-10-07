use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Injected clock for deterministic unit testing.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Real system clock reading elapsed milliseconds since the Unix epoch.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Action to execute when a countdown timer reaches 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "target")]
pub enum FinishAction {
    #[serde(rename = "none")]
    #[default]
    None,
    #[serde(rename = "play")]
    Play(String),
    #[serde(rename = "board")]
    Board(i64),
}

/// Pure countdown timer state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimerState {
    pub duration_ms: u64,
    pub finish_action: FinishAction,
    pub running: bool,
    pub started_at_ms: u64,
    pub elapsed_ms: u64,
    pub finished: bool,
}

impl Default for TimerState {
    fn default() -> Self {
        Self {
            duration_ms: 300_000, // 5 min default
            finish_action: FinishAction::None,
            running: false,
            started_at_ms: 0,
            elapsed_ms: 0,
            finished: false,
        }
    }
}

impl TimerState {
    pub fn new(duration_ms: u64, finish_action: FinishAction) -> Self {
        Self {
            duration_ms: duration_ms.max(1_000),
            finish_action,
            running: false,
            started_at_ms: 0,
            elapsed_ms: 0,
            finished: false,
        }
    }

    /// Tap interaction:
    /// - If finished: restarts from beginning
    /// - If running: pauses
    /// - If paused: starts/resumes
    pub fn tap(&mut self, now_ms: u64) {
        if self.finished {
            self.finished = false;
            self.elapsed_ms = 0;
            self.started_at_ms = now_ms;
            self.running = true;
        } else if self.running {
            self.elapsed_ms += now_ms.saturating_sub(self.started_at_ms);
            self.running = false;
        } else {
            self.started_at_ms = now_ms;
            self.running = true;
        }
    }

    /// Reset interaction (long-press or double-tap).
    pub fn reset(&mut self) {
        self.running = false;
        self.started_at_ms = 0;
        self.elapsed_ms = 0;
        self.finished = false;
    }

    /// Advancing time / tick: checks if the timer has reached zero.
    /// Returns `Some(FinishAction)` if it completed on this tick.
    pub fn tick(&mut self, now_ms: u64) -> Option<FinishAction> {
        if !self.running {
            return None;
        }
        let total_elapsed = self.elapsed_ms + now_ms.saturating_sub(self.started_at_ms);
        if total_elapsed >= self.duration_ms {
            self.elapsed_ms = self.duration_ms;
            self.running = false;
            self.finished = true;
            Some(self.finish_action.clone())
        } else {
            None
        }
    }

    /// Remaining time in milliseconds.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        if self.finished {
            0
        } else if self.running {
            let total_elapsed = self.elapsed_ms + now_ms.saturating_sub(self.started_at_ms);
            self.duration_ms.saturating_sub(total_elapsed)
        } else {
            self.duration_ms.saturating_sub(self.elapsed_ms)
        }
    }

    /// Compact state pushed to clients.
    pub fn compact_state(&self, now_ms: u64) -> Value {
        json!({
            "type": "timer",
            "running": self.running,
            "startedAtMs": self.started_at_ms,
            "elapsedMs": self.elapsed_ms,
            "durationMs": self.duration_ms,
            "finished": self.finished,
            "remainingMs": self.remaining_ms(now_ms),
        })
    }

    /// Formatted label for legacy stock client (mm:ss).
    pub fn format_legacy(&self, now_ms: u64) -> String {
        let remaining_secs = self.remaining_ms(now_ms).div_ceil(1000);
        let mins = remaining_secs / 60;
        let secs = remaining_secs % 60;
        format!("{mins:02}:{secs:02}")
    }
}

/// Pure stopwatch state machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StopwatchState {
    pub running: bool,
    pub started_at_ms: u64,
    pub elapsed_ms: u64,
}

impl StopwatchState {
    /// Tap interaction: start or stop.
    pub fn tap(&mut self, now_ms: u64) {
        if self.running {
            self.elapsed_ms += now_ms.saturating_sub(self.started_at_ms);
            self.running = false;
        } else {
            self.started_at_ms = now_ms;
            self.running = true;
        }
    }

    /// Reset interaction (long-press or double-tap).
    pub fn reset(&mut self) {
        self.running = false;
        self.started_at_ms = 0;
        self.elapsed_ms = 0;
    }

    /// Total accumulated elapsed time in milliseconds.
    pub fn elapsed_total_ms(&self, now_ms: u64) -> u64 {
        if self.running {
            self.elapsed_ms + now_ms.saturating_sub(self.started_at_ms)
        } else {
            self.elapsed_ms
        }
    }

    /// Compact state pushed to clients.
    pub fn compact_state(&self, now_ms: u64) -> Value {
        json!({
            "type": "stopwatch",
            "running": self.running,
            "startedAtMs": self.started_at_ms,
            "elapsedMs": self.elapsed_ms,
            "currentElapsedMs": self.elapsed_total_ms(now_ms),
        })
    }

    /// Formatted label for legacy stock client.
    pub fn format_legacy(&self, now_ms: u64) -> String {
        let total_secs = self.elapsed_total_ms(now_ms) / 1000;
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        if hours > 0 {
            format!("{hours:02}:{mins:02}:{secs:02}")
        } else {
            format!("{mins:02}:{secs:02}")
        }
    }
}

/// Pure counter state machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CounterState {
    pub step: i64,
    pub start_value: i64,
    pub label: String,
    pub count: i64,
}

impl Default for CounterState {
    fn default() -> Self {
        Self {
            step: 1,
            start_value: 0,
            label: "Licznik".to_string(),
            count: 0,
        }
    }
}

impl CounterState {
    pub fn new(step: i64, start_value: i64, label: String) -> Self {
        Self {
            step,
            start_value,
            label,
            count: start_value,
        }
    }

    /// Tap interaction: +step.
    pub fn tap(&mut self) {
        self.count += self.step;
    }

    /// Double-tap interaction: -step.
    pub fn double_tap(&mut self) {
        self.count -= self.step;
    }

    /// Long-press / reset interaction: reset to start_value.
    pub fn reset(&mut self) {
        self.count = self.start_value;
    }

    /// Compact state pushed to clients.
    pub fn compact_state(&self) -> Value {
        json!({
            "type": "counter",
            "count": self.count,
            "step": self.step,
            "label": self.label,
        })
    }

    /// Formatted label for legacy client.
    pub fn format_legacy(&self) -> String {
        self.count.to_string()
    }
}

/// Tool kind enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Clock,
    Timer,
    Stopwatch,
    Counter,
}

impl std::str::FromStr for ToolKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tool-clock" => Ok(ToolKind::Clock),
            "tool-timer" => Ok(ToolKind::Timer),
            "tool-stopwatch" => Ok(ToolKind::Stopwatch),
            "tool-counter" => Ok(ToolKind::Counter),
            _ => Err(()),
        }
    }
}

/// Combined enum for stored state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ToolState {
    #[serde(rename = "clock")]
    Clock,
    #[serde(rename = "timer")]
    Timer(TimerState),
    #[serde(rename = "stopwatch")]
    Stopwatch(StopwatchState),
    #[serde(rename = "counter")]
    Counter(CounterState),
}

impl ToolState {
    pub fn from_config(kind: &str, command: Option<&str>) -> Self {
        let args = parse_command_json(command);
        match kind {
            "tool-timer" => {
                let duration_ms = args
                    .get("duration")
                    .and_then(parse_duration_val)
                    .unwrap_or(300_000);
                let finish_action = match args.get("finish_action").and_then(Value::as_str) {
                    Some("play") => {
                        let path = args
                            .get("sound_path")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        FinishAction::Play(path)
                    }
                    Some("board") => {
                        let id = args
                            .get("board_id")
                            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                            .unwrap_or(0);
                        FinishAction::Board(id)
                    }
                    _ => FinishAction::None,
                };
                ToolState::Timer(TimerState::new(duration_ms, finish_action))
            }
            "tool-stopwatch" => ToolState::Stopwatch(StopwatchState::default()),
            "tool-counter" => {
                let step = args
                    .get("step")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                    .unwrap_or(1);
                let start_value = args
                    .get("start_value")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                    .unwrap_or(0);
                let label = args
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("Licznik")
                    .to_string();
                ToolState::Counter(CounterState::new(step, start_value, label))
            }
            _ => ToolState::Clock,
        }
    }

    pub fn update_config(&mut self, kind: &str, command: Option<&str>) {
        let args = parse_command_json(command);
        match (self, kind) {
            (ToolState::Timer(t), "tool-timer") => {
                if let Some(dur) = args.get("duration").and_then(parse_duration_val) {
                    if !t.running && t.elapsed_ms == 0 {
                        t.duration_ms = dur;
                    }
                }
                if let Some(act) = args.get("finish_action").and_then(Value::as_str) {
                    t.finish_action = match act {
                        "play" => {
                            let path = args
                                .get("sound_path")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            FinishAction::Play(path)
                        }
                        "board" => {
                            let id = args
                                .get("board_id")
                                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                                .unwrap_or(0);
                            FinishAction::Board(id)
                        }
                        _ => FinishAction::None,
                    };
                }
            }
            (ToolState::Counter(c), "tool-counter") => {
                if let Some(step) = args
                    .get("step")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                {
                    c.step = step;
                }
                if let Some(start) = args
                    .get("start_value")
                    .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
                {
                    c.start_value = start;
                }
                if let Some(lbl) = args.get("label").and_then(Value::as_str) {
                    c.label = lbl.to_string();
                }
            }
            _ => {}
        }
    }

    pub fn compact_state(&self, now_ms: u64) -> Value {
        match self {
            ToolState::Clock => json!({ "type": "clock" }),
            ToolState::Timer(t) => t.compact_state(now_ms),
            ToolState::Stopwatch(s) => s.compact_state(now_ms),
            ToolState::Counter(c) => c.compact_state(),
        }
    }
}

fn parse_command_json(cmd: Option<&str>) -> Value {
    cmd.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null)
}

fn parse_duration_val(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n * 1000);
    }
    if let Some(s) = v.as_str() {
        return parse_duration_ms(s);
    }
    None
}

/// Parses a duration string (e.g. "05:00", "5:00", "1:23:45", "300") into milliseconds.
pub fn parse_duration_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(secs) = s.parse::<u64>() {
        return Some(secs * 1000);
    }
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let m = parts[0].trim().parse::<u64>().ok()?;
            let s = parts[1].trim().parse::<u64>().ok()?;
            Some((m * 60 + s) * 1000)
        }
        3 => {
            let h = parts[0].trim().parse::<u64>().ok()?;
            let m = parts[1].trim().parse::<u64>().ok()?;
            let s = parts[2].trim().parse::<u64>().ok()?;
            Some((h * 3600 + m * 60 + s) * 1000)
        }
        _ => None,
    }
}

