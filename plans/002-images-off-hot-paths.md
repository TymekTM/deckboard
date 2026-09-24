# Plan 002: Get base64 tile images off the interaction and editor hot paths

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c1e0086..HEAD -- crates/db crates/v2 crates/legacy apps/desktop`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `c1e0086`, 2026-09-24

## Why this matters

Tile images are stored in the `Shortcuts` table as full
`data:image/...;base64,...` strings (the original app's schema - the format
stays). Today those strings ride every read path:

- `Db::get_button` (crates/db/src/lib.rs:191-204) SELECTs `img`/`img2` -
  and `crates/v2/src/session.rs:318` calls it for **every** v2
  interaction event: each tile tap, and each `Slide` event during a drag
  (session.rs:393-394), i.e. tens of SQLite reads per second during a
  slider drag, each materializing a multi-MB base64 string (33% larger
  than the image) plus ~30 String allocations for the other columns.
- `allowed_interactions` (crates/v2/src/boards.rs:98-101) then builds a
  full legacy-Mapper JSON payload (~30-field serde_json Map) just to read
  one `app` field - per interaction event.
- Every boards rebuild (crates/v2/src/boards.rs:64-68 → assets.rs:56-65)
  base64-decodes and sha256-hashes **every** image again; there is no
  data-URL → hash memoization.
- The editor's `list_boards` command ships every board with every button
  including images over Tauri IPC on every edit (apps/desktop/src-tauri/
  src/lib.rs:860-874), the Vue app keeps them all resident
  (apps/desktop/src/App.vue:215-227 `load()` replaces `boards.value`
  wholesale), and `EditTileModal.vue:22-23` deep-clones the whole button
  (`JSON.parse(JSON.stringify(...))`) - images included - on every modal
  open.

This plan removes images from the per-gesture path, memoizes the hash
mapping, and trims the editor's refetch storm. It deliberately does NOT
change the storage format or the wire format of `boards.sync` (legacy
mapper payloads still carry `img` - the stock Android client renders
exactly those fields, see `crates/legacy/src/mapping.rs`; that contract is
frozen per ADR-002 in `docs/decisions.md`).

## Current state

Files:

- `crates/db/src/lib.rs` - `get_button` (191-204) and
  `get_buttons_by_board` (177-189) share a 31-column SELECT including
  `img, img2`. `get_boards` (152-162) and `get_board` (164-175) use
  `prepare` (recompiled per call) while the button queries use
  `prepare_cached`.
- `crates/v2/src/session.rs` - `handle_interaction` (306-351): the
  `spawn_blocking(get_button)` at 318-321, the
  `allowed_interactions(&button)` check at 344.
- `crates/v2/src/boards.rs` - `allowed_interactions` (98-101),
  `widget_kind` (105-124, only consults `legacy.get("app")` for the
  `"custom-value"` comparison at line 114), `build_tile` (60-94) with the
  `assets.import_data_url(img)` call at 64-68.
- `crates/v2/src/assets.rs` - `import_data_url` (56-65): decodes + hashes
  every call; `exts: Mutex<HashMap<String, String>>` (hash → ext) is the
  existing interior-mutability pattern to copy.
- `crates/legacy/src/mapping.rs` - `Mapper::new().shortcut_payload(row)`
  builds the ~30-field payload; `props.rs` has
  `register_extension_input`/`ExtInput` - the registry of known custom
  value keys (the `command` column values that mean "custom-value").
- `apps/desktop/src-tauri/src/lib.rs` - `list_boards` (860-874, N+1);
  `read_image_data` (699-715) is where data URLs are born.
- `apps/desktop/src/App.vue` - `load()` (215-227) re-fetches
  serverStatus + getSettings + listKnownInputs + listAudioDevices +
  listBoards on every edit action (callers at lines 175, 190, 294, 316,
  322, 344, 356, 368, 403).
- `apps/desktop/src/components/EditTileModal.vue` - line 22-23 deep clone.

Excerpts at `c1e0086`:

```rust
// crates/v2/src/boards.rs:98
pub fn allowed_interactions(row: &ButtonRow) -> Vec<Interaction> {
    let legacy = Mapper::new().shortcut_payload(row);
    widget_kind(row, &legacy).1
}
// widget_kind only reads `legacy.get("app")` - and only to compare
// against "custom-value" (line 114). The app value for a tile is
// `row.type` for catalog/custom-value tiles (see mapping.rs), NOT
// something the Mapper computes from images.
```

```rust
// crates/v2/src/assets.rs:56
pub fn import_data_url(&self, url: &str) -> Option<String> {
    ...
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload).ok()?;
    self.import_bytes(&bytes, ext).ok()
}
```

Conventions:

- Rust: interior mutability via `Mutex<HashMap<..>>` with
  `.expect("asset store poisoned")` (see assets.rs:47-50). Errors degrade
  silently (`.ok()?`) - match it.
- Tests: `crates/v2/tests/integration.rs` is the integration harness;
  unit tests live in-file under `#[cfg(test)]`. `crates/db/src/lib.rs`
  has in-file tests using `Db::open_or_create` with temp dirs.
- Frontend: plain Vue 3 `<script setup>`, no test framework; verification
  is `npx vite build` plus the manual smoke listed in Test plan.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Rust tests | `cargo test --workspace` | all pass |
| Rust lint | `cargo clippy --workspace --all-targets` | exit 0, no warnings |
| Frontend build | `cd apps/desktop && npx vite build` | exit 0 |

## Scope

**In scope**:
- `crates/db/src/lib.rs`
- `crates/v2/src/session.rs`, `crates/v2/src/boards.rs`, `crates/v2/src/assets.rs`
- `crates/v2/tests/integration.rs` (new/adjusted tests)
- `apps/desktop/src-tauri/src/lib.rs` (list_boards trimming only)
- `apps/desktop/src/App.vue`, `apps/desktop/src/components/EditTileModal.vue`

**Out of scope**:
- The on-disk schema and the `.boardjson` import/export format - the data
  URL stays in the DB row; only read paths change.
- The legacy wire payload (`Mapper::shortcut_payload` keeps producing
  `img`) - stock-client contract, ADR-002.
- The Vue rendering of images (it keeps using `row.img` data URLs) - the
  full "serve editor images by hash URL" migration is a separate, deferred
  finding.
- `crates/legacy/src/mapping.rs`.

## Git workflow

- Branch: `perf/images-off-hot-paths`
- Commit per step; message style matches
  `ext: reject path traversal and overflow in asar headers`
  (area prefix, lowercase imperative).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: `Db::get_button_meta` - an image-less interaction read

In `crates/db/src/lib.rs`, add next to `get_button`:

```rust
/// The interaction-relevant subset of a button row, without the image
/// columns: taps and slider slides hit this per event and must not
/// materialize multi-MB base64 strings.
pub fn get_button_meta(&self, id: i64) -> Result<Option<ButtonRow>, rusqlite::Error> {
    let mut stmt = self.conn.prepare_cached(
        "SELECT id, board_id, type, command, title, title_position, title_color, \
         title_box_color, color, icon_color, icon_color2, border_color, shape, icon, \
         '' AS img, '' AS img2, icon2, color2, shape2, border_color2, title_position2, \
         title_box_color2, title_color2, position, position2, mode, x, y, w, h, options \
         FROM Shortcuts WHERE id = ?1",
    )?;
    // same query_map + map_button_row as get_button
}
```

(`ButtonRow.img`/`img2` are `Option<String>`-like empty-means-none in the
v2 layer - check `map_button_row` and keep whatever empty-string → None
normalization it already does so the row is internally consistent.)

Switch `crates/v2/src/session.rs:318` to `get_button_meta`. Confirm no
other consumer of that `button` value reads `img`: `exec_once` and
`backend.slider` receive the row - check `SqlBackend::exec`/`slider` in
`crates/backend/src/lib.rs` for `img` usage (grep `\.img` there). If the
backend path reads `img` for styling, keep `get_button` for the exec path
and only the gesture-check path uses the meta read - but per recon the
exec/slider paths consume `command`/`type`/`options`, not `img`.

**Verify**: `cargo test -p deckboard-v2` → all pass (integration tests
exercise taps and slides).

### Step 2: `allowed_interactions` without the Mapper payload

In `crates/v2/src/boards.rs`, replace the payload build:

```rust
pub fn allowed_interactions(row: &ButtonRow) -> Vec<Interaction> {
    widget_kind(row, &legacy_app_marker(row)).1
}
```

Determine how `legacy.get("app")` is produced for the `"custom-value"`
comparison: read `crates/legacy/src/mapping.rs` `shortcut_payload` and
`crates/legacy/src/props.rs` (`register_extension_input`, the
`ExtInput.command` values). The app value equals the tile's `command`
column for extension/custom-value tiles. Implement
`fn legacy_app_marker(row: &ButtonRow) -> Value` producing just that one
string (a `serde_json::json!(...)` of `row.command`/`row.type` exactly as
the mapper does - verify against mapping.rs, do not guess) so
`widget_kind`'s line 114 comparison is byte-identical in outcome.

Add a unit test in `boards.rs` asserting `allowed_interactions` returns
`[Tap]` for a row whose `command`/`type` is a registered custom-value key
and `[Tap, PressStart, PressEnd]` for a plain button - mirroring what
`widget_kind` already does via the full Mapper (keep one test that builds
the row through `Mapper::new().shortcut_payload` as the oracle).

**Verify**: `cargo test -p deckboard-v2` → all pass, including the new
oracle test.

### Step 3: Memoize data-URL → hash in `AssetStore`

In `crates/v2/src/assets.rs`, add next to `exts`:

```rust
/// data URL -> hash. Board rebuilds re-import the same URL strings on
/// every generation bump; decode+sha256 once per distinct image.
url_hashes: Mutex<HashMap<String, String>>,
```

In `import_data_url`, before decoding: check the map; on miss, run the
existing decode+`import_bytes`, insert `url -> hash`. Bound it: when the
map exceeds 512 entries, clear it (distinct images per install are far
below that; the clear is a one-line safety valve, and the entries are
small relative to the images themselves).

**Verify**: `cargo test -p deckboard-v2` → all pass. Add one unit test:
import the same data URL twice, assert the store returns the same hash and
(add a `#[cfg(test)]` counter or check `self.url_hashes.lock().unwrap().len() == 1`).

### Step 4: Trim the editor's per-edit refetch

1. `apps/desktop/src/App.vue`: split `load()` (line 215) into
   `loadCore()` (serverStatus + getSettings + listKnownInputs +
   listAudioDevices - the static-ish panel data) and `loadBoards()`
   (listBoards + currentId fixup + touchBoardId default). Replace the
   edit-action call sites (lines 294, 316, 322 - tile created/edited/
   deleted, and 344/356/368 - import/clear/delete board) with
   `loadBoards()`; keep `loadCore()` for startup and window-show paths
   (175, 190, 403 - verify each of those three is a startup/reconnect
   context before deciding; when unsure, `loadCore()` there too is safe).
2. `apps/desktop/src/components/EditTileModal.vue:22-23`: replace
   `JSON.parse(JSON.stringify(props.button))` with a shallow clone plus
   explicit nested copies ONLY of the small fields the form mutates
   (inspect which fields the form v-models; images are only displayed,
   never edited in place - `img`/`img2` can be passed by reference).
3. `crates/db/src/lib.rs`: switch `get_boards` (153) and `get_board`
   (165) from `prepare` to `prepare_cached`.

**Verify**: `cd apps/desktop && npx vite build` → exit 0. Manual smoke
(optional if no display): open the editor, edit a tile title, confirm the
grid updates and the audio-device dropdown still populates on reopen of
the app.

### Step 5: `list_boards` - drop the N+1 (optional polish, keep if trivial)

In `apps/desktop/src-tauri/src/lib.rs:860-874`, replace the per-board
`get_buttons_by_board` loop with one `SELECT ... FROM Shortcuts WHERE
board_id IN (...)` helper `Db::get_buttons_for_boards(&[i64])` in
`crates/db/src/lib.rs`, grouping in Rust. If the IN-clause building gets
awkward, a single `SELECT <button columns> FROM Shortcuts ORDER BY
rowid` filtered in Rust by the board set is equally fine at this scale
(<20 boards). Skip this step entirely if Step 1-4 are already green and
this one fights you - it is the lowest-value item in the plan.

**Verify**: `cargo test --workspace && cargo clippy --workspace --all-targets`
→ all pass, no warnings.

## Test plan

- New unit tests: boards.rs oracle test (Step 2), assets.rs memoization
  test (Step 3), db `get_button_meta` returns the same non-image fields as
  `get_button` for a fixture row (Step 1).
- Existing `crates/v2/tests/integration.rs` tap/slide flows must stay
  green - they are the regression net for the interaction-path change.
- Manual smoke (editor): tile with an image renders, tap in touch mode
  executes, slider drag works.

## Done criteria

- [ ] `cargo test --workspace` exits 0
- [ ] `cargo clippy --workspace --all-targets` exits 0, no warnings
- [ ] `grep -n "shortcut_payload" crates/v2/src/boards.rs` matches only
      the oracle test, not `allowed_interactions`
- [ ] `grep -n "get_button(" crates/v2/src/session.rs` returns nothing
      (replaced by `get_button_meta`)
- [ ] `grep -n "JSON.parse(JSON.stringify" apps/desktop/src/components/EditTileModal.vue`
      returns nothing
- [ ] `cd apps/desktop && npx vite build` exits 0
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above do not match the live code (drift).
- `SqlBackend::exec`/`slider` (crates/backend/src/lib.rs) turn out to read
  `button.img` - then the interaction path needs the full row and Step 1's
  premise is wrong; report instead of hacking around it.
- The `app` field derivation in `mapping.rs` is not reconstructible from
  `row.command`/`row.type` alone (Step 2's oracle test will tell you).
- `integration.rs` fails on a tap/slide test after Step 2 and the failure
  is not a fixture/expectation mismatch you can trace to the new marker.

## Maintenance notes

- When editor writes start publishing v2 `boards.delta` (the currently
  unwired `V2State::publish_delta`, service.rs:201, has no production
  caller), every write bumps the generation and rebuilds boards - the
  Step-3 memoization is what keeps that rebuild cheap; revisit its bound
  then.
- The deferred follow-up "serve editor images by hash URL instead of data
  URLs over IPC" would delete the `read_image_data` data-URL pipeline
  entirely; do not start it inside this plan.
- Reviewer: the riskiest hunk is the `legacy_app_marker` derivation -
  diff its outputs against `Mapper::shortcut_payload()["app"]` for a few
  real rows from `~/deckboard/database.db` if in doubt.
