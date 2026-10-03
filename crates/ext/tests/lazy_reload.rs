//! Regression tests for the lazy runtime model.
//!
//! Stateless extensions must release their interpreter between actions, so
//! each execute re-runs the entry module and module-level state starts
//! fresh. Extensions that registered timers keep one live interpreter and
//! their module state persists across executes.

use std::time::Duration;

use serde_json::json;

const STATELESS_JS: &str = r#"
var __calls = 0;
module.exports = {
    name: "StatelessTest",
    inputs: [{ value: "lazy-echo", label: "Echo" }],
    execute: function (action, args) {
        __calls += 1;
        __pending_set_values.push({ calls: __calls });
        return "ok";
    },
};
"#;

const LIVE_JS: &str = r#"
var __calls = 0;
module.exports = {
    name: "LiveTest",
    inputs: [{ value: "live-echo", label: "Echo" }],
    initExtension: function () {
        setInterval(function () {}, 1000);
    },
    execute: function (action, args) {
        __calls += 1;
        __pending_set_values.push({ calls: __calls });
        return "ok";
    },
};
"#;

fn write_ext(dir: &std::path::Path, name: &str, js: &str) {
    std::fs::create_dir(dir.join(name)).expect("mkdir extension");
    std::fs::write(dir.join(name).join("index.js"), js).expect("write index.js");
}

/// Read the next `SetValue` payload from the manager's event stream.
fn next_push(events: &mut tokio::sync::mpsc::UnboundedReceiver<pulpit_ext::ExtEvent>) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match events.try_recv() {
            Ok(pulpit_ext::ExtEvent::SetValue(v)) => {
                return v
                    .get("calls")
                    .and_then(serde_json::Value::as_u64)
                    .expect("calls key") as u32;
            }
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => panic!("no event arrived in time: {e}"),
        }
    }
}

#[test]
fn stateless_extension_reloads_between_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ext(dir.path(), "stateless-test", STATELESS_JS);

    let (manager, mut events) = pulpit_ext::ExtManager::load(dir.path(), &json!(null), &[]);
    assert!(manager.has_action("lazy-echo"), "action not registered");

    manager.execute("lazy-echo", None).expect("first execute");
    assert_eq!(next_push(&mut events), 1, "first execute runs fresh module");

    // the interpreter was released after the run, so the entry module is
    // re-executed and the counter starts over
    manager.execute("lazy-echo", None).expect("second execute");
    assert_eq!(
        next_push(&mut events),
        1,
        "module state resets after reload"
    );
}

#[test]
fn timed_extension_keeps_state_between_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_ext(dir.path(), "live-test", LIVE_JS);

    let (manager, mut events) = pulpit_ext::ExtManager::load(dir.path(), &json!(null), &[]);
    assert!(manager.has_action("live-echo"), "action not registered");

    manager.execute("live-echo", None).expect("first execute");
    assert_eq!(next_push(&mut events), 1, "first execute on live runtime");

    // timers keep the interpreter resident, so module state persists
    manager.execute("live-echo", None).expect("second execute");
    assert_eq!(
        next_push(&mut events),
        2,
        "module state persists on live runtime"
    );
}
