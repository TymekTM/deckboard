# Audit findings handoff - 2026-10-01

Written against commit `f07f447` (branch `t3code/5c2592ce`). This is a
**findings list with fix sketches**, not step-by-step plans: each item says
where the problem is, what to change and how to verify it. It continues the
numbering after `plans/001`-`005` (perf round, done/deferred) and
`plans/006`-`011` (mobile battery/bugs round, step-by-step plans written in
parallel with this audit). Where this file overlaps 006-011 the step-by-step
plan wins; overlaps are marked `SUPERSEDED BY 0xx` below.

Audit scope: all Rust crates, `apps/desktop` (Vue + Tauri), `apps/server`,
`apps/mobile` (Kotlin), CI and docs. Not done: no `cargo test`,
`cargo clippy`, `cargo audit` or Gradle run (read-only audit); no runtime
reproduction of anything. `npm audit --omit=dev` on `apps/desktop`: 0
vulnerabilities.

## Rules for the executor (read first)

1. **Re-verify every finding against the current code before changing it.**
   Line numbers are from `f07f447` and may have drifted. Items marked
   `VERIFIED` were opened and confirmed by the advisor; items marked
   `REPORTED` came from an audit subagent and were not opened separately -
   if the code does not match the description, STOP on that item and note it
   in the status table rather than improvising.
2. **One change, three surfaces** (`AGENTS.md`). Tile behavior must be
   handled in desktop, both wire builders (`crates/legacy/src/mapping.rs`,
   `crates/v2/src/boards.rs`) and Android. Before calling a tile-related fix
   done, check what the other two surfaces do with the same field.
3. Verification gates (run what applies to the surfaces you touched):
   - Rust: `cargo test --workspace` and `cargo clippy --workspace --all-targets`
     from the repo root, plus `cargo fmt --all -- --check` (CI runs fmt; the
     recent history has several "fmt drift" fixes).
   - Desktop: `npx vite build` in `apps/desktop`; exercise the UI when the
     change is visible.
   - Android: Gradle unit tests in `apps/mobile` when its code changed.
4. The release profile is `panic = "abort"` - any panic in any thread kills
   the whole resident tray app. Do not introduce `unwrap`/`expect`/unchecked
   indexing on file, network or DB data.
5. Legacy wire field names are contractual (stock Deckboard Android client).
   Do not rename or drop fields in `mapping.rs`.
6. Never write secret values (tokens, pairing codes, keys) into code,
   tests, logs, commit messages or docs. Reference file and credential type
   only.
7. One commit per package below, message style: `<area>: <what>` (see
   `git log --oneline`). Do not merge or push.

## Suggested order

- **Package A** (independent small fixes, do first): A1-A9.
- **Package B** (security hardening): B1-B3. B1 needs a manual check with the
  stock Deckboard Android client; do not merge it without that.
- **Package C** (behavior/robustness, medium): C1-C5.
- **Package D** (Android): D1-D4.
- **Package E** (test/CI/docs): E1-E3. Do E1 (parity tests) before touching
  tile styling fields (C5).

---

## Package A - small, confirmed fixes

### A1. Desktop touch mode and "Run now" never press key tiles - VERIFIED, HIGH
- Where: `apps/desktop/src-tauri/src/lib.rs` `exec_button` (~1262-1285)
  calls `backend.exec(button, false, ..)`. `crates/actions/src/lib.rs`
  `run_command`/`run_key` (~183-286): with `is_tap_start=false` a `key` tile
  only calls `key_up` (enigo `Release`), never `Press`. Tablets are fine
  because the v2 session sends press-start (`is_tap_start=true`) and
  press-end (`crates/v2/src/session.rs` ~394-413).
- Effect: key-macro tiles do nothing from desktop touch mode or the editor's
  "Run now". Called from `App.vue` `api.execButton` (touch tap + "Run now").
- Fix: in `exec_button`, run the same sequence a tablet tap produces for
  keys: `exec(button.clone(), true, ..)` then a short pause (the v2 tap path
  uses release-phase only for non-key kinds, so `true` is a no-op for them)
  then `exec(button, false, ..)`. Keep it inside the existing
  `spawn_blocking`. Do not change `run_command` semantics (tablet clients
  rely on them). Keep multiaction/board behavior unchanged.
- Test: add a test in `crates/backend` or `crates/actions` using the
  existing `MockInput` seam (`Effect::KeyDown/KeyUp`) proving a `key` tile
  executed through the new helper yields `KeyDown` then `KeyUp`, and a `url`
  tile fires exactly once. If `exec_button` is hard to unit-test, extract
  the sequence into a function in `crates/backend` and test that.
- Done when: new test passes; `cargo test --workspace` green.

### A2. F13-F24 inject F12 - VERIFIED
- Where: `crates/actions/src/lib.rs` `enigo_key`, `KeyName::Function(n)`
  match (~472-486): `_ => Key::F12` for 13..=24. `parse_key_name`
  (~75-80) accepts F1-F24.
- Fix: map 13..=24 to `Key::F13`..`Key::F24` if the pinned `enigo` 0.2
  exposes them (check with `cargo doc`/source in `~/.cargo/registry`); if
  it does not, use `Key::Other(vk)` with VK_F13 = 0x7C .. VK_F24 = 0x87 on
  Windows. Never fall back to F12 - make the unknown case an error.
- Test: table-driven test over `1..=24` asserting every `Function(n)` maps
  to a distinct key.

### A3. Claude OAuth usage lane reads the wrong key - VERIFIED on this machine
- Where: `crates/aidev/src/limits.rs` `claude_oauth_usage` (~598-612) reads
  `oauth.get("access_token")`. The real credentials file
  (`~/.claude/.credentials.json`) nests under `claudeAiOauth` with camelCase
  keys: `accessToken`, `refreshToken`, `expiresAt`, `scopes`, ... (verified
  by printing key names only - never print values).
- Effect: the "Claude OAuth usage" lane in `docs/aidev.md` never works and
  silently falls back to local sums.
- Fix: accept `accessToken` first, `access_token` as fallback. If
  `expiresAt` (ms since epoch) is present and in the past, skip the call.
  Log HTTP failures at `debug` with status only (never the token or header).
- Test: unit test feeding a fixture JSON with camelCase keys (fake token
  string like `"test-token"`) through the extraction helper - extract the
  parsing into a small pure function to make this testable.

### A4. Hotkey change can leave the user with no hotkey and a success UI - VERIFIED
- Where: `apps/desktop/src-tauri/src/lib.rs` `set_touch_mode_hotkey`
  (~1324-1351) unregisters the old shortcut first, then
  `register_touch_mode_hotkey` (~858-877) only logs a failure; the command
  returns `Ok(())` and persists the new combo anyway.
- Fix: make `register_touch_mode_hotkey` return `Result<(), String>`.
  In `set_touch_mode_hotkey`: register the new combo first; only on success
  unregister the old one, update state and persist; on failure keep the old
  hotkey registered and return `Err(msg)`. Startup call site keeps its
  fallback-to-default behavior. In `apps/desktop/src/components/SettingsOverlay.vue`
  `saveHotkey` (~84-94) show the returned error text next to the field.
- Verify: `npx vite build`; manually try setting a combo already owned by
  another app and confirm the old one still works and the error is shown.

### A5. `settings.json` / `editor.json` can be wiped or torn - VERIFIED (discord), REPORTED (editor.json)
- Where: `crates/discord/src/lib.rs` `save_tokens` (~270-307):
  `read_to_string(..).unwrap_or_else(|_| "{}")` + `serde_json ... unwrap_or_else(empty object)`
  then non-atomic `std::fs::write`. `settings.json` also holds every
  extension's config. Similar non-atomic write for `editor.json`
  (`crates/backend/src/lib.rs` ~78-145) and hotkey persistence
  (`lib.rs` `std::fs::write(path, json)` in `set_touch_mode_hotkey`).
- Fix: (1) if the file EXISTS but cannot be read or parsed, return an error
  without writing (a missing file is the only case that starts from `{}`);
  (2) add one shared helper (e.g. `write_atomic(path, bytes)` in
  `crates/db`, which `discord` and `backend` already can depend on - check
  dependency direction first) that writes `<path>.tmp` in the same dir and
  renames; use it at the three sites. Existing test
  `save_tokens_errors_on_unmergeable_settings` must still pass.
- Test: new test: unreadable/corrupt settings file -> `save_tokens` returns
  an error and the file's bytes are unchanged.

### A6. Extension `fetch` shim passes an object where the native reads a string - VERIFIED
- Where: `crates/ext/src/prelude.js` `__fetch` (~471-505) calls
  `__host_http(spec)` with a JS object; `crates/ext/src/host.rs` `host_http`
  (~424) does `arg_str(args, 0)` which uses `as_string()` -> empty string for
  an object, so the URL is empty and every call fails. `__http_request`
  (~712) correctly passes `JSON.stringify(...)`. `host_http` also ignores
  `headers` and `timeout_ms`, and `do_http` (~464+) uses an agent with no
  timeout.
- Fix: `JSON.stringify(spec)` in `__fetch`; make `host_http` read and apply
  `headers` (object of string -> string) and `timeout_ms` (default 15000, cap
  e.g. 60000); give the ureq agent a timeout. Keep the return shape.
- Test: Rust test that starts a throwaway local `TcpListener` HTTP server,
  runs a tiny script through the ext runtime calling `fetch(url, {headers})`
  and asserts status/body/header arrival. If the runtime makes that
  impractical, test `host_http`'s JSON handling directly via the native
  function with a `Context`.
- Note: this makes network calls that currently fail actually go out; read
  `crates/ext/src/host.rs` `do_http` for redirect behavior and keep it.

### A7. Hold-repeat loop runs blocking exec on the async runtime - VERIFIED
- Where: `crates/v2/src/session.rs` `start_hold` (~432-460): the repeat loop
  calls `exec_blocking(...)` directly inside `tokio::spawn`. Tap/press paths
  correctly use `exec_once` -> `spawn_blocking`. The headless server runtime
  is `current_thread` (`apps/server/src/main.rs` ~18), so one slow tile
  (multiaction sleep, callurl, Discord) stalls every socket and ping.
- Fix: inside the loop use
  `tokio::task::spawn_blocking(...).await` for each tick and skip/serialize
  so a slow tick is never overlapped by the next. The existing `timeout(cap)`
  wrapper keeps bounding total hold time.
- Test: existing hold tests in `crates/v2/tests/integration.rs` must pass;
  add one asserting another session still gets a pong while a hold tile's
  action is slow (use a mock backend with a sleeping exec).

### A8. `delete_board` is two non-transactional statements - VERIFIED
- Where: `crates/db/src/lib.rs` `delete_board` (~326-332). Use the existing
  `with_transaction` helper (see other multi-write functions in that file).
- Test: small test that both rows disappear; no partial state is observable
  by construction.

### A9. Destructive UI actions have no confirmation - VERIFIED (modal close), REPORTED (details)
- Where: `apps/desktop/src/App.vue` tile Delete (~222-227);
  `apps/desktop/src/components/BoardModal.vue` Clear/Delete (~39-47,77-80);
  overlay click closes modals and discards edits (`BoardModal.vue` ~51,
  `EditTileModal.vue` ~413).
- Fix: confirm before delete/clear (use the project's existing modal/dialog
  pattern - look at how other confirmations or `SettingsOverlay.vue` do it;
  `@tauri-apps/plugin-dialog` is already permitted via `dialog:default`).
  Overlay click only closes when the form is unchanged ("dirty" flag).
- Verify: `npx vite build`; click through delete tile, delete board, clear
  board, edit-then-click-outside.

---

## Package B - security hardening

### B1. No Origin/Host validation on the sockets; any web page can drive tiles - VERIFIED (absence), exploit not run
- Where: `crates/legacy/src/service.rs` (upgrade ~113-127, ~422),
  `crates/v2/src/service.rs` `ws_connect` (~88-117) and `pair_create`
  (~123-130). Grep for `origin`/`Host` handling across `crates/legacy`,
  `crates/v2`, `apps/server` found none. The server binds `0.0.0.0`
  (`apps/desktop/src-tauri/src/lib.rs` ~597, `apps/server/src/main.rs`
  ~298). Browsers allow cross-origin WebSocket connections, so a web page
  open on the PC can connect to `ws://127.0.0.1:8500/socket.io/...`, read
  boards and fire `exec_shortcut` (app launchers, `type`, `key`, `url`).
  `POST /v2/pair` trusts only `addr.ip().is_loopback()`, which a browser on
  the same machine satisfies. ADR-008 (LAN trust) does not cover this: a
  web page is not a LAN peer.
- Fix: a small axum middleware / check on the upgrade and `/v2/pair`:
  reject when an `Origin` header is present and is not same-host (browser
  clients send it; native tablet clients and the stock Deckboard app are
  expected to send none - VERIFY this assumption); reject when `Host` is not
  an IP literal or `localhost`. Consider dropping the HTTP `POST /v2/pair`
  route entirely, since the desktop mints codes via the Tauri command
  `create_pairing_code` (`lib.rs` ~1070) - check `apps/server` and docs for
  other users of that route first.
- STOP condition: if the stock Deckboard Android client (or the native
  client) sends an `Origin` header and would be rejected, STOP and report;
  do not guess a weaker rule. A manual test with the real tablet is required
  before this is considered done.
- Test: integration tests in `crates/legacy/tests` and
  `crates/v2/tests/integration.rs`: upgrade with a foreign `Origin` is
  rejected; upgrade without `Origin` still works; with loopback `Origin`
  works as decided.

### B2. Pairing and device trust (ADR-008 drift) - REPORTED
- Where: `crates/v2/src/session.rs` ~174-181 auto-accepts pairing ("no UI
  yet"); `crates/v2/src/devices.rs` `revoke` (~125-135) has no caller
  outside tests and the file is loaded once (hand edits are ignored and
  overwritten); no failed-attempt limit on codes (`devices.rs` ~15-16,
  ~209-222); pairing code logged at info (`crates/v2/src/service.rs`
  ~136-139, `apps/desktop/src-tauri/src/lib.rs` ~1086, logs kept 14 days);
  `hello.name` unbounded and logged raw (`session.rs` ~164-177);
  `touch`/`rename` returning `None` (revoked device) falls through with the
  stale entry (`session.rs` ~168-171); tokens stored in clear
  (`devices.rs` ~27,111-114) although `verify` compares SHA-256 digests;
  save errors only warn (`devices.rs` ~141-152).
- Fix (split in small commits, each independently useful):
  1. Do not log pairing codes (log "code minted" only).
  2. Trim `hello.name`, cap ~64 chars, strip control chars, fallback
     "Device".
  3. Treat `None` from `touch`/`rename` as `unauthorized`.
  4. Store only token digests in `devices.json` (migrate existing plain
     tokens on load) and use `write_atomic` (see A5).
  5. Failed-attempt budget: invalidate all outstanding codes after N wrong
     codes within the TTL.
  6. Desktop: Tauri commands `list_devices` / `revoke_device` and hub
     closing that device's live sessions on revoke; UI list in the
     Settings overlay "Tablets" section (`SettingsOverlay.vue` ~278-330).
     Trust prompt before accepting a new device. This is a product feature -
     check `PRODUCT.md`/`DESIGN.md` and `docs/decisions.md` ADR-008, and
     update ADR-008 to match what you build.
- Android: `docs/decisions.md` ADR-008/`docs/protocol-v2.md` say
  EncryptedSharedPreferences but `apps/mobile/.../PulpitViewModel.kt`
  (~49,199-203) uses plain SharedPreferences, and the manifest allows backup
  and cleartext. Minimum: `android:allowBackup="false"` (or backup rules
  excluding the prefs file); either implement Keystore-backed storage or
  amend the ADR/doc to say plain storage. Decide with the owner.

### B3. Tauri WebView hardening - VERIFIED (csp null, read_image_data)
- Where: `apps/desktop/src-tauri/tauri.conf.json` (~15) `"csp": null`;
  `apps/desktop/src-tauri/src/lib.rs` `read_image_data` (~970-987) reads any
  path the WebView passes (extension-filtered only), `import_boards` /
  `export_boards` (~1354-1385) take arbitrary paths. `v-html` in
  `TileCell.vue` (~218-222,378-450) currently only renders constants.
- Fix: set a restrictive CSP (start from `default-src 'self'`; allow
  `img-src 'self' data: blob:` and whatever origin the v2 asset/LAN QR
  needs; check the built app for violations). Cap `read_image_data` size
  (e.g. 10 MiB) and file type; ideally accept only paths that came from a
  dialog result. Cap `import_boards` file size.
- Risk: a strict CSP can break inline styles/fonts. Test the built app
  (`npm run tauri dev`) in editor, touch mode and settings; if inline
  styles break, allow `style-src 'self' 'unsafe-inline'` rather than
  dropping CSP.

---

## Package C - behavior and robustness

### C1. Held keys never released when a tablet disconnects - VERIFIED
- Where: press-start does `key_down` (`crates/v2/src/session.rs`
  ~394-396 -> `crates/actions/src/lib.rs` `run_key`); teardown paths
  `hub.rs` `shutdown`/`abort_holds` (~181-185, ~237-242), session end
  (`session.rs` ~96) only abort repeat tasks. Legacy (`crates/legacy/src/service.rs`
  ~321-361) has the same gap. `docs/protocol-v2.md` (~271-275) promises
  release when the connection dies.
- Fix: per session, track tiles with an un-ended key press-start
  (`HashMap<tile, ButtonRow>`), and on EVERY teardown path (clean close,
  reaper, overflow, error) run the release-phase `exec(button, false)` /
  `key_up` for each. Same for the legacy session. Run through
  `spawn_blocking`.
- Test: integration test: pair, press-start a `key` tile with a mock
  `Input`, drop the socket, assert `KeyUp` arrives.

### C2. Hub-side teardown leaves a zombie session task - REPORTED
- Where: `crates/v2/src/hub.rs` `shutdown` (~181-185) does
  `try_send(Close)` then immediately `abort_pump`, so Close is never
  flushed; the read loop in `session.rs` (~240-272) keeps running with no
  writer; final `hub.remove` returns `None`, so cleanup is skipped
  (`session.rs` ~96); a hold started after removal is never aborted.
- Fix: give the session a cancellation signal (oneshot / `CancellationToken`)
  that `shutdown` fires and `run_session` `select!`s on; call `abort_holds`
  unconditionally at the tail of `run`. Combine with C1's release.
- Test: extend `overflowing_queue_closes_the_session` /
  `reap_silent_drops_stale_and_keeps_fresh_sessions` to assert the session
  task finishes.

### C3. Multiactions drop extension/speaker/Discord/Voicemeeter/play/sysinfo steps - VERIFIED
- Where: `crates/actions/src/lib.rs` `run_multiaction` (~298-346): the
  fallback arm `_ => run_command(..)` only handles builtin kinds; unknown
  kinds are warned "ignored" (~218-229). Extension/native dispatchers live
  only in `crates/backend/src/lib.rs` `exec` (~293-320).
- Fix: pass a step-dispatch callback (or a trait method on the sink/input
  seam) from `Backend::exec` into `run_multiaction` so each step goes
  through the same dispatch chain as a top-level tile (extension, sysinfo,
  aidev, callurl, voicemeeter, discord, speaker, play, then builtin). Keep
  the `delay`/`board`/`key` special cases.
- Test: unit test in `crates/backend` with a fake extension/speaker: a
  multiaction `[key, speaker volume, delay]` executes all three.
- Risk: MED - macros that were partly silently skipped will now fully run.
  Mention in the commit message.

### C4. Board size/import limits - VERIFIED (code path), REPORTED (impact)
- Where: `apps/desktop/src/components/BoardModal.vue` (`Number()` on
  width/height, no bounds); `crates/backend/src/lib.rs` `import_boards`
  (~182-207; no size/count/dimension checks); `crates/db/src/lib.rs`
  insert (~460-480); `crates/legacy/src/mapping.rs` filler loop (~54-105,
  cost W*H*B); `crates/v2/src/boards.rs` (~52-53, only `.max(1)`);
  `apps/desktop/src/components/GridEditor.vue` `emptyCells` (~66-82) and
  drag clamp (~118-119) ignoring tile width.
- Fix: define one constant (e.g. `MAX_BOARD_DIM = 32`) and enforce integer
  bounds in the UI modal, in `import_boards` (reject or clamp; also cap file
  size and tile count), and defensively in both wire builders. Clamp drag to
  `W-w` / `H-h`.
- Test: import test with a 1_000_000 x 1_000_000 board is rejected quickly;
  mapper test with oversized dimensions does not allocate W*H.

### C5. Style/field parity gaps between editor and v2/Android - REPORTED (needs E1 first)
- Where: `crates/v2/src/boards.rs` `style()` (~221-237) emits only 7 fields
  (`crates/proto/src/lib.rs` ~399-419); the editor
  (`apps/desktop/src/components/EditTileModal.vue`) also edits border, icon
  color and title color; `apps/mobile/.../ui/Tile.kt` hardcodes white
  (~114-131) and border 0. The state-2 image (`img2`) is dropped on the v2
  wire (`boards.rs` `build_tile` ~77-113), while the desktop preview
  (`TileCell.vue` ~47-90) has fallback rules for state 2. Also: dual-flagged
  catalog entries without a state binding in `apps/desktop/src/catalog.js`
  (`STATE_BINDINGS` ~430-447, `stateActive` ~454-477; OBS truthiness check
  tests a whole object).
- Fix: write down one fallback rule (state 2 falls back to state 1 per
  field), then extend `proto::Style`, `boards.rs`, `V2.kt`
  (`apps/mobile/.../proto/V2.kt`), `Tile.kt` and keep legacy `mapping.rs`
  unchanged unless it already drops them. Regenerate ts-rs bindings and
  golden fixtures (`crates/proto`). Per AGENTS.md all three surfaces and
  both wire builders move together. New fields must be optional so old
  clients tolerate them.
- This is a protocol change: bump/justify in `docs/protocol-v2.md`.
- Other correctness items in the same area (REPORTED): migration not
  resumable and no schema creation on clean install (`crates/db/src/lib.rs`
  ~655-718, contradicts ADR-011 "per-item resumable"; copy to a staging dir
  then rename, copy WAL/shm, per-item skip-if-present; update ADR-011);
  `map_board_row` strict on junk legacy columns, one bad row empties the
  list (`crates/db/src/lib.rs` ~422-440, `crates/backend/src/lib.rs`
  ~233-241; use the lenient `row_int` pattern from the Shortcuts path).

---

## Package D - Android (`apps/mobile`)

All REPORTED unless noted. Run the Gradle unit tests after each.
**Execute plans 006-011 first.** Overlaps: D1's stale-closure half ->
plan 009 (D1's cancelled-touch phantom tap is NOT covered there - still do
it here); D2 -> plan 006 (SUPERSEDED); D3 -> plan 007 (SUPERSEDED); D4's
slider/knob live value is listed as "Deferred" in the mobile round but
still open here; E3's AGENTS.md part -> plan 011 (SUPERSEDED).

### D1. Phantom taps on cancelled touches - VERIFIED (code)
- `ui/Tile.kt` ~261-270: `onPress = { onPressStart(); try { awaitRelease() } finally { onPressEnd() } }`.
  `onPressEnd` -> `PulpitViewModel.kt` ~433-437 becomes `tap` for tap-only
  tiles regardless of whether the touch was cancelled (`awaitRelease()`
  returns false when the finger leaves). Fix: send the tap only when
  `awaitRelease()` returns `true`; still send `press-end` when `press-start`
  was sent. Also stale closures: `pointerInput(tile.id)` blocks
  (`Tile.kt` ~261,331, `Widgets.kt` ~350) keep first-composition lambdas
  after a `tile-set` delta changes the tile - use `rememberUpdatedState`.

### D2. Asset bitmaps vanish after re-pairing; data race - SUPERSEDED BY 006
- `PulpitViewModel.kt` ~47 (scope with no dispatcher), ~105,160 (plain
  `mutableSetOf` mutated from two threads), ~188 (`_bitmaps.value = _bitmaps.value + ..`
  loses concurrent updates -> use `MutableStateFlow.update {}`), ~253
  (`forgetPairing` clears bitmaps but not `assetFetches`), ~161-162 (hash
  added before the token null-check). Fix all four.

### D3. Reconnect policy - SUPERSEDED BY 007
- `V2Client.kt` ~257-259, ~191, ~291 (`FATAL_CODES`); `PulpitViewModel.kt`
  ~295-296, ~366-370, ~405. After a fatal auth code (`unauthorized`,
  `pair-invalid`, `pair-expired`, `outdated-client`, `too-large`) enter a
  terminal "failed" state that suppresses reconnect and shows the specific
  reason; otherwise use exponential backoff with jitter and a longer cap.

### D4. Sliders/knobs ignore live state; `boards.delta` is all-or-nothing - REPORTED
- `ui/Tile.kt` ~315-330, `Widgets.kt` ~335-350 start from 0.5 and take no
  live value; `onDragCancel` unhandled. Map the channel's live value to
  0..1 when not dragging (desktop touch mode already mirrors state).
- `V2Client.kt` ~235 / `proto/V2.kt` ~212-225: one malformed op drops the
  whole batch (`runCatching` around the frame). Decode ops individually;
  on any rejected op force a resync (reconnect/snapshot).
- Also: version drift: `build.gradle.kts` versionName `0.1.0`, `V2Client.kt`
  `VERSION = "0.2.0"`, workspace/tauri/package `0.1.1`. Pick one source.

---

## Package E - tests, CI, docs

### E1. Parity tests - REPORTED (do before C5)
- Add a Rust test that builds the same `ButtonRow` through
  `crates/legacy/src/mapping.rs` and `crates/v2/src/boards.rs` and asserts
  the shared fields agree (press modes, dual states, colors, icons, hold
  config). Add vitest (or a node script run from CI) for
  `apps/desktop/src/catalog.js` `stateActive`/`STATE_BINDINGS`: every
  dual-flagged catalog entry has a binding. Add a Kotlin test over the
  golden JSON fixtures in `crates/proto` for `Tile.kt`'s field handling.
- Also missing v2/legacy tests (list from the audit): 403 loopback guard on
  `/v2/pair`, `pair-expired` over the socket, hello timeout, revoked/unknown
  token, held-key cleanup on disconnect (C1), asset Range/416 over HTTP,
  concurrent `consume` of one code, malformed legacy packets. The hold
  tests use `sleep(250ms)` windows - move to `tokio::time::pause` where
  possible. The known flaky `token_connect_delivers_full_snapshot` (see
  `plans/README.md`) is unrelated; do not "fix" it here.
- Other untested risky code: `crates/backend` exec dispatch order and
  multiaction (C3); `crates/ext` host natives and prelude shims (A6);
  `crates/vm` input validation; migration partial-copy (C5).

### E2. CI - REPORTED
- `.github/workflows/ci.yml`: `cargo clippy ... -- -D warnings` (check the
  current baseline is clean first; if not, fix or scope), Gradle job for
  `apps/mobile` (there is no Gradle wrapper committed - add `gradlew` or use
  `gradle/actions/setup-gradle`), `cache: npm`, `concurrency` group,
  `permissions: contents: read`, optional `cargo audit`/`cargo deny` (not
  installed locally, so audit result unknown). Consider a Linux job for the
  crates that should build there.

### E3. Docs drift - REPORTED/VERIFIED
- (SUPERSEDED BY 011) `AGENTS.md`: the Android row and "Models.kt mirrors mapping.rs" are stale:
  the native client speaks protocol v2 and its model file is
  `apps/mobile/.../proto/V2.kt`, which mirrors `crates/proto` (and
  `crates/v2/src/boards.rs`), not the legacy mapper. Keep the "carry fields
  through BOTH wire builders" rule - only fix the Android row.
- `docs/aidev.md`: crate is `pulpit-aidev`, config path is
  `<data_dir>/aidev.json`, env var `PULPIT_AIDEV_CONFIG` is missing from the
  README env table; README architecture table omits `crates/aidev`; README
  says `cargo test --workspace` runs real device tests but CI ignores them.
- (DONE in the index) `plans/README.md` note about `publish_delta` having
  no production caller was stale and has been struck through.
- `docs/protocol-v2.md` still mentions the 8501 split; `apps/mobile/README.md`
  and `build.gradle.kts` still say "Deckboard"; ADR-007 (foreground service)
  and ADR-008 (trust prompt, encrypted prefs) describe features that do not
  exist yet - update the ADR or build the feature (see B2).

---

## Lower-priority items (REPORTED; do only if time permits)

- `crates/aidev/src/antigravity.rs` ~194-200,274: `i + len as usize` on an
  untrusted varint can overflow and panic/loop; use `checked_add` +
  `usize::try_from` and require forward progress; test with `u64::MAX`.
- `crates/aidev/src/codex.rs` ~179-203: `read_to_string` on a tail seeked
  into the middle of a UTF-8 char returns `InvalidData` and drops the file's
  limits; read bytes + `from_utf8_lossy`; the `?` on missing
  `payload`/`timestamp` inside the reverse loop aborts the whole scan -
  use `continue`. `crates/aidev/src/local_usage.rs` ~122-125 same
  `read_to_string` risk.
- `crates/aidev/src/limits.rs` ~418-421: refuse `http://` for non-loopback
  provider hosts (the Bearer key would go in plaintext).
- `crates/vm/src/lib.rs` ~249-308, ~368-370: Voicemeeter parameter text is
  built with `format!` and sent to `VBVMR_SetParameters`, which accepts
  `;`-separated commands - validate `kind`/`number`/`param` against strict
  patterns and reject `;`, quotes, newlines in names.
- `crates/discord/src/lib.rs`: add timeouts to all agents and a frame-length
  cap (`decode_frame` ~375-385); derive `Debug` on config structs holds
  secrets (~44-50) - redact; avoid opening the OAuth consent window on
  plain network errors (`crates/backend/src/lib.rs` ~563-634).
- `crates/ext/src/manager.rs` ~447-489, ~667-713, ~210-342: `execute` holds
  `residence` across a 30 s blocking dispatch; `spawn_runtime` waits on a
  channel with no timeout; a wedged extension adds a 30 s stall per launch.
  Also: extension dispatch runs BEFORE builtin kinds in
  `crates/backend/src/lib.rs` ~305-316 (an extension can hijack `key`, `url`,
  `type`); extraction to a predictable `%TEMP%\pulpit-extensions\<pkg>`
  (`crates/ext/src/source.rs` ~74-86); `crates/ext/src/lib.rs` cites ADR-005
  for extension trust but that ADR covers web widgets only - write an ADR.
  Metadata cache is keyed by mtime-in-seconds, written non-atomically and
  named `pulpit-server` (`manager.rs` ~816-869).
- `crates/os`: `PolicyConfigVtbl` fields declared as Rust-ABI `unsafe fn`
  instead of `unsafe extern "system" fn` (`win.rs` ~95-113); clipboard
  NUL scan not bounded by `GlobalSize` (`clipboard.rs` ~46-54);
  `set_active_device` returns at the first failing role (`win.rs` ~257-273);
  screenshot file creation is check-then-write (`capture.rs` ~17-31);
  shared MCI alias across concurrent playbacks (`play.rs`).
- `crates/actions/src/lib.rs`: input mutex held across multiaction sleeps
  with unbounded `delay` (~320-327,369-372) - cap delays; hotkeys silently
  drop unknown key names (`filter_map`, ~276-286, ~434-436) and splitting on
  `+` breaks the plus key; `open` options split on whitespace (~419-431).
- `crates/v2/src/assets.rs` ~48-51: asset writes are check-then-write to the
  final name while responses are `immutable`; write to temp + rename and
  verify size at `open`.
- `apps/server/src/main.rs` ~274-283: hand-built JSON via `format!` for
  legacy broadcasts; use `serde_json::json!`.
- `crates/sysinfo/src/lib.rs` ~54-67,101-105: `push_loop` ignores the
  closed-channel result and never exits; `crates/vm/src/lib.rs` ~235-245
  `logged_in` stays true forever.
- Desktop: `App.vue` ~461-475 registers event listeners only after
  `await load()` - if load rejects the tray/touch listeners never register;
  failed saves/imports/runs have no user feedback (`api.js` thin wrappers);
  `touchBoardId` can point at a deleted board; hotkey/tray events after the
  10-minute WebView teardown (`lib.rs` ~675-721) can be lost or arrive
  before listeners register; slider drag in `TileCell.vue` ~280-319 lacks
  `setPointerCapture`; tiles are not keyboard-operable.
- aidev "today"/"week" use UTC boundaries (`local_usage.rs` ~368-377),
  which resets "today" at 02:00 in Poland - consider an optional local
  midnight setting.

## Considered and rejected (do not re-audit)

- SQL injection in `crates/db` (all queries parameterized).
- Zip-slip / traversal in asar extraction (names rejected, offsets checked,
  unit-tested).
- Path traversal on `/assets/<hash>` (hash validated as 64 hex).
- Constant-time token comparison (digests compared; moot).
- Legacy unauthenticated surface and unbounded polling queues (ADR-008).
- `callurl` arbitrary GET (by design).
- `base64`/`rand`/`dirs` duplicate versions in `Cargo.lock`: real but low
  value; move `ureq`/`dirs` into `[workspace.dependencies]` only if
  touching them anyway.

## Direction options (for the owner; not tasks)

1. Finish tablet pairing/trust on both ends (QR scan + `pulpit://` intent
   filter on Android, device list with trust/revoke on desktop) - closes B2
   and M4.
2. Settings UI for AI dev work (keys, ceilings, timezone) in the new
   settings overlay - conflicts with the owner's deferral of aidev perf
   plan 001; ask first.
3. Headless server on Linux (add a Linux CI job first to size it).
4. One hardening release: B1 + remove HTTP `/v2/pair` + code attempt limit
   + token rotation.

## Status

| Item | Status |
|------|--------|
| A1-A9 | TODO |
| B1-B3 | TODO |
| C1-C5 | TODO |
| D1 (phantom tap only), D4 | TODO |
| D2, D3 | SUPERSEDED by 006, 007 |
| E1-E3 | TODO |
