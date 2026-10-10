use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::state::*;
use super::*;

struct FakeClock {
    time: AtomicU64,
}

impl FakeClock {
    fn new(start_ms: u64) -> Self {
        Self {
            time: AtomicU64::new(start_ms),
        }
    }

    fn advance(&self, delta_ms: u64) {
        self.time.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.time.load(Ordering::SeqCst)
    }
}

#[test]
fn timer_lifecycle_tap_pause_resume_and_finish() {
    let clock = FakeClock::new(10_000);
    let mut timer = TimerState::new(60_000, FinishAction::Play("alert.mp3".into()));

    assert!(!timer.running);
    assert!(!timer.finished);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 60_000);
    assert_eq!(timer.format_legacy(clock.now_ms()), "01:00");

    // Tap to start
    timer.tap(clock.now_ms());
    assert!(timer.running);
    assert_eq!(timer.started_at_ms, 10_000);

    // Advance 15 seconds
    clock.advance(15_000);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 45_000);
    assert_eq!(timer.format_legacy(clock.now_ms()), "00:45");
    assert_eq!(timer.tick(clock.now_ms()), None);

    // Tap to pause
    timer.tap(clock.now_ms());
    assert!(!timer.running);
    assert_eq!(timer.elapsed_ms, 15_000);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 45_000);

    // Advance while paused: remaining stays 45s
    clock.advance(30_000);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 45_000);

    // Tap to resume
    timer.tap(clock.now_ms());
    assert!(timer.running);

    // Advance 40 seconds (total elapsed = 15 + 40 = 55s, 5s remaining)
    clock.advance(40_000);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 5_000);
    assert_eq!(timer.format_legacy(clock.now_ms()), "00:05");
    assert_eq!(timer.tick(clock.now_ms()), None);

    // Advance 6 seconds (reaches duration = 60s)
    clock.advance(6_000);
    let finished_action = timer.tick(clock.now_ms());
    assert_eq!(
        finished_action,
        Some(FinishAction::Play("alert.mp3".into()))
    );
    assert!(timer.finished);
    assert!(!timer.running);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 0);
    assert_eq!(timer.format_legacy(clock.now_ms()), "00:00");

    // Subsequent tick produces None
    assert_eq!(timer.tick(clock.now_ms()), None);

    // Tap while finished restarts from 0
    timer.tap(clock.now_ms());
    assert!(timer.running);
    assert!(!timer.finished);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 60_000);
}

#[test]
fn timer_reset_restores_initial_state() {
    let clock = FakeClock::new(1_000);
    let mut timer = TimerState::new(30_000, FinishAction::None);

    timer.tap(clock.now_ms());
    clock.advance(10_000);
    assert!(timer.running);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 20_000);

    timer.reset();
    assert!(!timer.running);
    assert!(!timer.finished);
    assert_eq!(timer.elapsed_ms, 0);
    assert_eq!(timer.remaining_ms(clock.now_ms()), 30_000);
}

#[test]
fn stopwatch_start_pause_resume_reset() {
    let clock = FakeClock::new(5_000);
    let mut sw = StopwatchState::default();

    assert!(!sw.running);
    assert_eq!(sw.elapsed_total_ms(clock.now_ms()), 0);
    assert_eq!(sw.format_legacy(clock.now_ms()), "00:00");

    // Start
    sw.tap(clock.now_ms());
    assert!(sw.running);

    // Advance 25 seconds
    clock.advance(25_000);
    assert_eq!(sw.elapsed_total_ms(clock.now_ms()), 25_000);
    assert_eq!(sw.format_legacy(clock.now_ms()), "00:25");

    // Pause
    sw.tap(clock.now_ms());
    assert!(!sw.running);
    assert_eq!(sw.elapsed_ms, 25_000);

    // Time passes while paused
    clock.advance(10_000);
    assert_eq!(sw.elapsed_total_ms(clock.now_ms()), 25_000);

    // Resume
    sw.tap(clock.now_ms());
    assert!(sw.running);

    // Advance 40 seconds (total 65s = 1m 5s)
    clock.advance(40_000);
    assert_eq!(sw.elapsed_total_ms(clock.now_ms()), 65_000);
    assert_eq!(sw.format_legacy(clock.now_ms()), "01:05");

    // Reset
    sw.reset();
    assert!(!sw.running);
    assert_eq!(sw.elapsed_total_ms(clock.now_ms()), 0);
    assert_eq!(sw.format_legacy(clock.now_ms()), "00:00");
}

#[test]
fn counter_tap_double_tap_reset() {
    let mut counter = CounterState::new(5, 10, "Bilety".into());
    assert_eq!(counter.count, 10);
    assert_eq!(counter.format_legacy(), "10");

    // Tap (+5)
    counter.tap();
    assert_eq!(counter.count, 15);
    counter.tap();
    assert_eq!(counter.count, 20);

    // Double-tap (-5)
    counter.double_tap();
    assert_eq!(counter.count, 15);

    // Reset (back to start_value 10)
    counter.reset();
    assert_eq!(counter.count, 10);
}

#[test]
fn parse_duration_formats() {
    assert_eq!(parse_duration_ms("05:00"), Some(300_000));
    assert_eq!(parse_duration_ms("1:30"), Some(90_000));
    assert_eq!(parse_duration_ms("45"), Some(45_000));
    assert_eq!(parse_duration_ms("1:00:00"), Some(3_600_000));
    assert_eq!(parse_duration_ms("invalid"), None);
}

#[test]
fn disk_store_persistence_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("tools.json");

    let mut store = DiskStore::default();
    store.tools.insert(
        "101".into(),
        ToolState::Timer(TimerState::new(120_000, FinishAction::None)),
    );
    store.tools.insert(
        "102".into(),
        ToolState::Counter(CounterState::new(2, 5, "Test".into())),
    );

    store.save(&file).unwrap();
    let loaded = DiskStore::load(&file);
    assert_eq!(store, loaded);
}

#[test]
fn tool_manager_gestures_and_change_stream() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("tools.json");
    let clock = Arc::new(FakeClock::new(1_000));
    let (tx, mut rx) = unbounded_channel();
    let mgr = ToolManager::new(file.clone(), clock.clone(), tx);

    // Execute tap on counter
    assert!(mgr.execute(
        42,
        "tool-counter",
        Some(r#"{"step":3,"start_value":0}"#),
        ToolGesture::Tap
    ));
    let change = rx.try_recv().expect("pushed on change");
    assert_eq!(change["tool-42"]["count"], 3);

    // Execute double-tap on counter
    assert!(mgr.execute(42, "tool-counter", None, ToolGesture::DoubleTap));
    let change2 = rx.try_recv().expect("pushed on change");
    assert_eq!(change2["tool-42"]["count"], 0);

    // Verify persisted
    let loaded = DiskStore::load(&file);
    assert!(loaded.tools.contains_key("42"));
}
