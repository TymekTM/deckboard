# Plan 004: Make the idle loops quiet - gate WebView emits, dedupe unchanged pushes, collapse the COM chain

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c1e0086..HEAD -- apps/desktop/src-tauri/src crates/os crates/aidev/src/lib.rs apps/server/src`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW-MED
- **Depends on**: none (plan 001 touches the same crate but different code)
- **Category**: perf
- **Planned at**: commit `c1e0086`, 2026-09-24

## Why this matters

The desktop app runs 24/7 in the tray, but its idle loops behave as if
someone were watching:

1. **The speaker watcher** (`apps/desktop/src-tauri/src/lib.rs:384-433`)
   fires every 5 s and runs THREE full WASAPI COM object graphs where one
   suffices: `speaker_status()` = `volume()` + `muted()` each build their
   own `CoCreateInstance(MMDeviceEnumerator)` + default endpoint +
   `IAudioEndpointVolume` activation (`crates/os/src/win.rs:184-198`), and
   `speaker_device_id()` → `active_device()` builds a third
   (`win.rs:235-237`). The device-id read runs EVERY tick on the desktop
   even though only its *push* is change-gated - the headless server
   already reads it only every 6th cycle (`apps/server/src/main.rs:251-256`).
   Volume/mute are also broadcast to the legacy hub and emitted to the
   WebView on every tick even when unchanged.
2. **All four producer loops** (extensions 311-321, sysinfo 334-343, aidev
   364-377, speaker 384-433) push to BOTH the legacy hub AND
   `app.emit(...)` into the WebView - with no visibility check and no
   change check. Hidden in tray, that is ~17,000 IPC round-trips/day into
   a WebView nobody sees, each paying double serialization
   (`payload.to_string()` for the hub + Tauri IPC serialization for the
   emit). The v2 engine dedupes identical scalars
   (`crates/v2/src/state.rs:126-133`) - only this legacy/WebView path
   doesn't.
3. **The aidev payload** is sent unconditionally every ~15 s
   (`crates/aidev/src/lib.rs:210-211`), and its agent rows embed
   tick-varying age strings ("working · 45s") so even the v2 engine's
   dedupe can never fire - >95% of idle ticks ship data identical to the
   previous tick.

The 5 s speaker cadence itself is documented parity with the original app
(`ROADMAP.md` M2: "the original's real cadence") and stays. This plan
removes only the redundant work around it. Expected result: with zero
tablets connected and the window hidden, the app does ~1 COM activation
chain per 30 s instead of 3 per 5 s, and near-zero WebView IPC.

## Current state

Files:

- `apps/desktop/src-tauri/src/lib.rs` - the four producer loops
  (extensions 307-321, sysinfo 329-343, aidev 351-377, speaker 384-433);
  all follow the same shape: `hub.broadcast("app_status_update", Some(
  &payload.to_string())).await; let _ = app.emit("app-status-update",
  &payload);`.
- `crates/os/src/win.rs` - `WinSpeaker::{volume, muted, active_device}`
  (183-237), each building its own enumerator chain via the `enumerator()`
  helper; `endpoint_volume(&default_device(&enumr)?)` at 185-186, 193-194.
- `crates/os/src/lib.rs` (or wherever the `Speaker` trait is defined -
  locate with `grep -rn "trait Speaker" crates/os/src`) - the trait both
  `volume()` and `muted()` come from.
- `crates/backend/src/lib.rs` - `speaker_status()` (463-470) =
  `sp.volume()` + `sp.muted()`; `speaker_device_id()` delegates to
  `active_device()`.
- `crates/aidev/src/lib.rs` - `push_loop` (189-217): unconditional
  `tx.send(payload)` at 210-211.
- `crates/legacy/src/hub.rs` - `Hub::broadcast` + session count (grep
  `pub fn len` / `is_empty` - the reaper at lib.rs:455-463 calls
  `reap(75)`, so the hub exposes session inspection).
- `apps/server/src/main.rs` - the headless twin of the speaker loop
  (247-291) with the 6th-cycle device-id pattern to copy.

Excerpts at `c1e0086`:

```rust
// apps/desktop/src-tauri/src/lib.rs:399 - three COM chains per 5 s tick
let snapshot = tauri::async_runtime::spawn_blocking(move || {
    let (volume, muted) = snapshot_backend.speaker_status(); // chains 1+2
    let device = snapshot_backend.speaker_device_id();       // chain 3
    (volume, muted, device)
}).await.ok();
// ... hub.broadcast(...) + app.emit(...) every tick, unchanged or not
```

```rust
// crates/os/src/win.rs:184 - one chain per getter
fn volume(&mut self) -> Result<f32> {
    let enumr = enumerator()?;
    let vol = endpoint_volume(&default_device(&enumr)?)?;
```

```rust
// crates/aidev/src/lib.rs:210 - unconditional send
let payload = assemble(&config, &http, ..., now);
if tx.send(payload).is_err() { ... }
```

Conventions:

- COM apartment rules are load-bearing here: `ROADMAP.md` M2 notes the
  switch survives COM-apartment teardown and `CoInitializeEx` S_FALSE
  means a pre-existing MTA we must NOT `CoUninitialize`. Read the
  `enumerator()` helper and its CoInitialize/CoUninitialize pairing before
  touching it; keep per-call init/teardown semantics unless the change is
  provably inside one apartment.
- Blocking COM/SQLite work always goes through `spawn_blocking` (see the
  comment at lib.rs:395-397).
- Tests: `cargo test --workspace` includes real device tests (audio, screen
  capture) - the speaker tests are the regression net for the COM change.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Rust tests | `cargo test --workspace` | all pass (audio device tests included) |
| Rust lint | `cargo clippy --workspace --all-targets` | exit 0, no warnings |
| Aidev live smoke | `cargo test -p deckboard-aidev -- --ignored live --nocapture` | passes |

## Scope

**In scope**:
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/server/src/main.rs` (same helper applied to its twin loop)
- `crates/os/src/win.rs`, `crates/os/src/lib.rs` (trait), any other
  `crates/os` file the Speaker trait lives in
- `crates/backend/src/lib.rs` (speaker_status/speaker_device_id)
- `crates/aidev/src/lib.rs` (push_loop change-detection only)

**Out of scope**:
- The 5 s / 10 s / 15 s cadences - documented parity with the original.
- `crates/v2` - its engine already dedupes; do not touch.
- `crates/legacy/src/hub.rs` broadcast internals (per-session clone is
  fine at this fan-out; plan 005 covers v2 queues).
- Plan 001's cache work in `crates/aidev/src/{agents,codex,...}.rs`.

## Git workflow

- Branch: `perf/quiet-idle-loops`
- Commit per step; message style: `speaker: one COM chain per snapshot,
  device id every 6th cycle` (matches `Complete speaker-device switching...`
  log style - area prefix, lowercase imperative).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: One COM snapshot per tick

1. In the `Speaker` trait, add:
   `fn status(&mut self) -> Result<(f32, bool, Option<String>)>` -
   volume, muted, active device id - with a default impl composed of the
   existing three calls (so any non-Windows impl is unchanged). Implement
   it in `WinSpeaker` with ONE enumerator + default device +
   endpoint-volume activation, reading volume + mute from the same
   `IAudioEndpointVolume`, and the id from the same `IMMDevice` (the
   `endpoint_id` helper already exists - see `devices()` at win.rs:211).
2. `crates/backend/src/lib.rs`: add `speaker_snapshot()` returning the
   triple, implemented via `status()`; keep `speaker_status()`/
   `speaker_device_id()` for existing callers (grep both).
3. Desktop loop (`lib.rs:399-405`): call `speaker_snapshot()` once inside
   the existing `spawn_blocking`.
4. Apply the server's 6th-cycle device-id pattern to the desktop too:
   keep a tick counter; read the device component every 6th tick by
   having `status()` take a `want_device: bool` (or split into
   `status()` + `active_device()` calls where the id is read only on the
   6th tick - mirror `apps/server/src/main.rs:251-265` exactly).
5. Mirror the same `status()` call in `apps/server/src/main.rs:257-265`
   (it currently does two chains per tick).

**Verify**: `cargo test --workspace` → all pass, including the real audio
device tests. Manual: change system volume; mute flips a bound tile within
~5 s; switch default device; the `speaker-device` value updates within
~30 s (6th cycle) on both desktop and the changed tile renders the new id.

### Step 2: Change-gate the speaker push

In the desktop speaker loop (lib.rs:406-431): keep `last_volume` /
`last_muted` locals like the existing `last_device`. Broadcast + emit the
volume/mute payload only when `volume != last_volume || muted !=
last_muted` (compare f32 with a small epsilon - the value is already
rounded to 1/1000 at lib.rs:408 - plus first-tick always sends). Keep
feeding `feed_v2.set(...)` unconditionally (it dedupes internally and is
cheap). The `speaker-device` branch already has its own gate - leave it.

**Verify**: `cargo test --workspace` → pass. Manual: with the app running
and DevTools open on the editor window (or a `console.log` temporarily in
the `app-status-update` handler), let volume sit unchanged for 30 s - no
speaker events arrive; nudge volume - one event arrives within 5 s.

### Step 3: Gate WebView emits on window visibility

Add one helper in `apps/desktop/src-tauri/src/lib.rs`:

```rust
/// Emit to the editor WebView only when it can be seen. The tray-hidden
/// window cannot render pushes; plan-mate 003 buffers them WebView-side
/// for the focused-but-obscured case.
fn emit_if_visible(app: &AppHandle, event: &str, payload: &serde_json::Value) {
    let visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        let _ = app.emit(event, payload);
    }
}
```

Use it in all four producer loops INSTEAD of the bare `app.emit(...)`
(extensions 318, sysinfo 340, aidev 375, speaker 417 and 429). The hub
broadcasts stay unconditional (tablets have no window). Note:
`toggle_main_window` (lib.rs:572-581) shows the window; the WebView will
refetch nothing automatically - that is acceptable because the Vue side
(plan 003 Step 1) keeps its own last-known values, and the next push
(≤15 s for the slowest lane) refreshes them. If plan 003 is NOT yet
landed, add to this plan: after `window.show()` in `toggle_main_window`
and in the single-instance handler (lib.rs:98-103), emit one synthetic
`app-status-update` carrying the last speaker snapshot (store the last
payloads in `DesktopState` as `Mutex<Vec<(String, Value)>>` ring of the
last payload per source) - keep this fallback simple.

**Verify**: `cargo test --workspace` → pass. Manual: hide to tray, watch
the log (`RUST_LOG=debug` or a temporary `tracing::debug!` in the helper)
- no emits while hidden; show - emits resume; volume tile is correct
within one push cycle.

### Step 4: Change-detect the aidev payload before sending

In `crates/aidev/src/lib.rs::push_loop` (210-211): keep
`let mut last_payload: Option<String> = None;` in the loop state.
Serialize the assembled payload to a String once (`serde_json::to_string`),
compare with `last_payload`:

- equal → skip `tx.send` this tick (still sleep as usual);
- different → send AND store.

Because agent rows embed changing age strings, ALSO make the comparison
robust: leave the payload as-is (do not restructure the rows here), and
instead force-send at most once per 60 s even when "unchanged"
 (`last_forced: Instant`) so a visible tile's ages stay fresh when the
 serialized form happens to be stable. This keeps the graph tiles'
 semantics: series keys (ai-tokens-hour etc.) arrive from the same payload
 object - check `assemble`'s output shape; if graph values ride the same
 keys that the 60 s keepalive serves, note that the hour sparkline will
 sample at ≤60 s granularity when idle instead of 15 s. If that
 granularity loss is unacceptable (decide by reading how
 `ai-tokens-hour` builds its series in `assemble`), then instead split
 the change-detection per top-level key (compare each `ai-*` key's
 serialization separately, send only the object of changed keys - the
 desktop/server consumers merge by key already, see
 `App.vue::mergeCustomValues` / the v2 `ext.*` feed in lib.rs:298-304).

**Verify**: `cargo test -p deckboard-aidev` → pass;
`cargo test -p deckboard-aidev -- --ignored live --nocapture` → two
consecutive ticks print and the second is not sent when identical (add a
temporary `tracing::debug!` on skip; remove before commit, or keep behind
`debug!` - hot-path logging must stay debug-level per repo convention).

### Step 5: Full-workspace gate

**Verify**: `cargo clippy --workspace --all-targets` → exit 0, no
warnings; `cargo test --workspace` → all pass.

## Test plan

- Rust unit test (crates/os): `status()` returns the same triple as the
  three legacy getters for the live default device - the workspace's real
  audio tests already exercise volume/mute; add the triple-consistency
  assertion next to them (find the existing speaker tests with
  `grep -rn "speaker" crates/os/src --include="*.rs" | grep test`).
- Backend: no new tests (pure delegation).
- Manual smoke: mute toggle reflects on a bound tile within 5 s; device
  switch within 30 s; aidev tiles still advance (graph samples keep
  arriving); hidden tray produces no WebView events (log check).

## Done criteria

- [ ] `cargo test --workspace` exits 0
- [ ] `cargo clippy --workspace --all-targets` exits 0, no warnings
- [ ] `grep -n "app.emit(\"app-status-update\"" apps/desktop/src-tauri/src/lib.rs`
      matches only inside the visibility helper
- [ ] `grep -c "enumerator()" crates/os/src/win.rs` shows the volume/muted/
      active-device getters now sharing one chain per snapshot (the three
      public getters may keep their own chains; the NEW `status()` must
      contain exactly one)
- [ ] Hidden-window log check shows zero `app-status-update` emits while
      hidden, resume on show
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above do not match the live code (drift).
- The `Speaker` trait is implemented by more than `WinSpeaker` and the new
  `status()` default impl is not obviously equivalent for it.
- COM apartment rules make a shared-chain `status()` unsound in your
  reading of `enumerator()`'s CoInitialize pairing - report the exact
  pairing you found.
- The aidev per-key split (Step 4 alternative) turns out to change what
  the tablets render for `ai-tokens-hour` sampling.

## Maintenance notes

- If a future feature adds "push on scroll/open" semantics for the editor
  (stale-while-hidden values), the show-time synthetic emit from Step 3's
  fallback is the hook to extend.
- The 6th-cycle device read and the epsilon comparison are tuned to the
  original app's observable behavior; if protocol v2 clients ever gain a
  "subscribe to device changes" channel, revisit both.
- Reviewer: watch for the classic bug where change-gating swallows the
  FIRST push after a client connects (first-tick-always-sends must hold).
