//! A wedged extension must not stall the manager: `execute` may block on
//! its runtime for up to the dispatch timeout, but the residence lock it
//! passes through is shared with `summary` (and lazy promotions), which
//! must stay answerable. A package that wedges while *loading* fails its
//! spawn after the load timeout, and that failure is remembered: later
//! executes error immediately instead of re-spawning.

use std::time::{Duration, Instant};

use serde_json::json;

const WEDGED_JS: &str = r#"
module.exports = {
    name: "WedgedTest",
    inputs: [{ value: "wedged-action", label: "Wedge" }],
    initExtension: function () { setInterval(function () {}, 60000); },
    execute: function () {
        __host_write_file(MARKER_PATH, "1");
        while (true) { }
    },
};
"#;

fn marker_escaped(path: &std::path::Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("escape marker path")
}

#[test]
fn summary_stays_answerable_while_a_dispatch_is_stuck() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("wedged-test");
    std::fs::create_dir_all(&pkg).expect("mkdir extension");
    let marker = dir.path().join("wedged.marker");
    std::fs::write(
        pkg.join("index.js"),
        WEDGED_JS.replace("MARKER_PATH", &marker_escaped(&marker)),
    )
    .expect("write index.js");

    let (manager, _events) = pulpit_ext::ExtManager::load(dir.path(), &json!(null), &[]);
    assert!(manager.has_action("wedged-action"));

    // the extension's execute writes a marker, then never returns: the
    // dispatch only ends at its 30 s timeout
    let stuck = std::sync::Arc::clone(&manager);
    std::thread::spawn(move || {
        let _ = stuck.execute("wedged-action", None);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(marker.exists(), "extension never started executing");

    let started = Instant::now();
    let summary = manager.summary();
    let elapsed = started.elapsed();
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].0, "wedged-test");
    assert!(
        elapsed < Duration::from_secs(5),
        "summary must not queue behind a wedged dispatch, took {elapsed:?}"
    );
}

/// A lazy package whose load wedges fails its first execute at the spawn
/// timeout, and that failure must mark the residence `Failed`: the next
/// execute errors immediately instead of paying another spawn (and
/// another timeout).
#[test]
fn wedged_spawn_marks_failed_and_later_executes_fail_fast() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pkg = dir.path().join("wedged-load-test");
    std::fs::create_dir_all(&pkg).expect("mkdir extension");
    // healthy, timer-free package: probes fine, stays lazy
    std::fs::write(
        pkg.join("index.js"),
        r#"
module.exports = {
    name: "WedgedLoadTest",
    inputs: [{ value: "wedged-load-action", label: "Wedge" }],
    execute: function () { return "ok"; },
};
"#,
    )
    .expect("write index.js");

    let (manager, _events) = pulpit_ext::ExtManager::load_with_spawn_timeout(
        dir.path(),
        &json!(null),
        &[],
        Duration::from_millis(200),
    );
    assert!(manager.has_action("wedged-load-action"));

    // replace the package with one whose top level never finishes: the
    // lazy first execute now hits the (short) spawn timeout and fails
    std::fs::write(pkg.join("index.js"), "while (true) { }").expect("rewrite index.js");

    let started = Instant::now();
    let first = manager.execute("wedged-load-action", None);
    assert!(
        first.is_err(),
        "a wedged load must fail its execute, got {first:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the first execute must wait out the spawn timeout"
    );

    let started = Instant::now();
    let second = manager.execute("wedged-load-action", None);
    let elapsed = started.elapsed();
    let message = format!("{}", second.expect_err("second execute must fail"));
    assert!(
        message.contains("is disabled"),
        "expected a disabled error, got: {message}"
    );
    assert!(
        elapsed < Duration::from_millis(200),
        "the failure must be remembered (no re-spawn), took {elapsed:?}"
    );
}
