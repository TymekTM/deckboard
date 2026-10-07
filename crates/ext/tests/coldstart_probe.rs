//! Timing probe for the cold-start of a "run command" tile press
//! (`run-command` action from `deckboard-commands.asar`).
//!
//! Ignored by default: it needs the real published package, which is not
//! in the repo. Point `PULPIT_COLDSTART_ASAR` at a COPY of the asar (never
//! the live file) and run with:
//!
//! ```text
//! cargo test -p pulpit-ext --test coldstart_probe -- --ignored --nocapture
//! ```
//!
//! It prints per-phase timings for the first press (extract + JS load +
//! execute) and the warm second press, plus the end-to-end manager path.
//! The extraction cache is a scratch temp dir, so the cold numbers are
//! deterministic and the user's real extraction cache is never touched.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;

fn probe_asar() -> Option<PathBuf> {
    std::env::var("PULPIT_COLDSTART_ASAR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore = "set PULPIT_COLDSTART_ASAR to a copy of deckboard-commands.asar"]
fn coldstart_phases() {
    let Some(asar) = probe_asar() else {
        panic!("PULPIT_COLDSTART_ASAR not set or not a file");
    };

    // Phase A: open the package. Cold pass: extraction into a scratch
    // cache; warm pass: same cache root again (cache hit), which is what
    // the second app session (or a second lazy spawn) pays.
    let cache = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let source = pulpit_ext::PackageSource::open_with_cache_root(
        &asar,
        "deckboard-commands".into(),
        Some(cache.path()),
    )
    .unwrap();
    let extract_ms = ms(t);
    let (root, _keep) = source.into_root();
    let file_count = walk_count(&root);
    println!(
        "phase A  open cold (extract -> scratch cache): {extract_ms:8.1} ms  ({file_count} files)"
    );

    let t = Instant::now();
    let warm = pulpit_ext::PackageSource::open_with_cache_root(
        &asar,
        "deckboard-commands".into(),
        Some(cache.path()),
    )
    .unwrap();
    let warm_open_ms = ms(t);
    println!("phase A' open warm (cache hit): {warm_open_ms:8.1} ms");
    drop(warm);

    // Phase B: evaluate prelude + entry module.
    let t = Instant::now();
    let mut rt = pulpit_ext::ExtRuntime::load(&root, "deckboard-commands", &json!(null)).unwrap();
    let load_ms = ms(t);
    println!("phase B  ExtRuntime::load (prelude+entry): {load_ms:8.1} ms");

    // Phase C: first execute of run-command (harmless command).
    let t = Instant::now();
    rt.execute(
        "run-command",
        &json!({"commandAction": "cmd /c echo pulpit-probe"}),
    )
    .unwrap();
    let exec1_ms = ms(t);
    println!("phase C  execute #1 (cmd /c echo): {exec1_ms:8.1} ms");

    // Phase D: second press = fresh runtime on the SAME extraction
    // (matches the resident-thread reload for a stateless package).
    let t = Instant::now();
    let mut rt2 = pulpit_ext::ExtRuntime::load(&root, "deckboard-commands", &json!(null)).unwrap();
    let load2_ms = ms(t);
    let t = Instant::now();
    rt2.execute(
        "run-command",
        &json!({"commandAction": "cmd /c echo pulpit-probe"}),
    )
    .unwrap();
    let exec2_ms = ms(t);
    println!("phase D  load #2: {load2_ms:8.1} ms   execute #2: {exec2_ms:8.1} ms");

    println!(
        "TOTAL first press ~{:.0} ms, second press ~{:.0} ms",
        extract_ms + load_ms + exec1_ms,
        load2_ms + exec2_ms
    );
}

#[test]
#[ignore = "set PULPIT_COLDSTART_ASAR to a copy of deckboard-commands.asar"]
fn coldstart_end_to_end_manager() {
    let Some(asar) = probe_asar() else {
        panic!("PULPIT_COLDSTART_ASAR not set or not a file");
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(&asar, dir.path().join("deckboard-commands.asar")).unwrap();

    let t = Instant::now();
    let (manager, _events) = pulpit_ext::ExtManager::load(dir.path(), &json!(null), &[]);
    let scan_ms = ms(t);
    println!("manager load (scan/classify): {scan_ms:8.1} ms");
    assert!(manager.has_action("run-command"), "action registered");

    let t = Instant::now();
    manager
        .execute(
            "run-command",
            Some(r#"{"commandAction": "cmd /c echo pulpit-probe"}"#),
        )
        .expect("first execute");
    let press1_ms = ms(t);
    println!("manager press #1 (lazy spawn + execute): {press1_ms:8.1} ms");

    let t = Instant::now();
    manager
        .execute(
            "run-command",
            Some(r#"{"commandAction": "cmd /c echo pulpit-probe"}"#),
        )
        .expect("second execute");
    let press2_ms = ms(t);
    println!("manager press #2 (warm): {press2_ms:8.1} ms");
}

fn walk_count(root: &std::path::Path) -> usize {
    fn rec(dir: &std::path::Path, out: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rec(&path, out);
            } else {
                *out += 1;
            }
        }
    }
    let mut n = 0;
    rec(root, &mut n);
    n
}
