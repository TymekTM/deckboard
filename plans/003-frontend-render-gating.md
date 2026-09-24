# Plan 003: Stop re-rendering the whole board on every state push, and pause while hidden

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c1e0086..HEAD -- apps/desktop/src apps/desktop/package.json`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Depends on**: none
- **Planned at**: commit `c1e0086`, 2026-09-24

## Why this matters

The editor window is a single Vue app that lives for days (closing hides
the window; the tray owns the real exit - `apps/desktop/src-tauri/src/lib.rs:110-118`).
The Rust side pushes live state into the WebView every few seconds around
the clock: speaker volume/mute every 5 s, sysinfo every 10 s, aidev ai-*
values every ~15 s, extension values on their own cadence. Three
structural facts turn those pushes into continuous hidden CPU:

1. **One render effect for the whole grid.** `customValues`/`appStates`
   are single `reactive({})` objects (App.vue:25-26) passed as props into
   `GridEditor.vue`, which renders ALL tiles in one component. Any pushed
   key re-runs the one render function and diffs every tile plus every
   empty cell (up to 15×15).
2. **New object identity per push, even when nothing changed.**
   `mergeCustomValues` builds `{ ...value, values }` for graph payloads
   and replaces status snapshots wholesale (App.vue:57-73); `mergeAppState`
   always spreads a new object (App.vue:75-77). Vue then re-renders even
   for identical values.
3. **No `document.hidden` gating.** There is no `visibilitychange`
   listener anywhere in `apps/desktop/src` (verified by grep at plan
   time): the merge + full vdom diff + DOM patch run all day behind a
   hidden window.

Additionally each render re-parses `JSON.parse(tile.command)` per helper
call (`catalog.js:301-306` via `stateActive`, called ~5-8× per dual tile
through `pick()`), and touch mode still renders the invisible empty-cell
nodes (hidden by CSS only).

This plan: gate merges on visibility (with flush-on-show), keep object
identity stable when values are unchanged, extract a per-tile component so
pushes re-render only affected tiles, and clean up the two small leaks
(dead `pointercancel` path, unused `@tauri-apps/plugin-fs` dependency).

## Current state

Files:

- `apps/desktop/src/App.vue` - state objects (25-26), `applyStatusUpdate`
  (28-55), `mergeCustomValues` (57-73), `mergeAppState` (75-77), Tauri
  `listen("app-status-update")` wiring (grep `listen(` - the handler feeds
  `applyStatusUpdate`), `load()` (215-227).
- `apps/desktop/src/components/GridEditor.vue` - the monolith: props
  `customValues`/`appStates`/`typeMeta` (lines ~1-45), `pick` + per-tile
  style helpers (51-94), tile `v-for` with inline helpers (~405-445),
  `emptyCells` computed (~230-246), empty-cell `v-for` (~395-408),
  status/hour row `v-for`s keyed by index (~455-515), pointer drag
  handlers (275-292, 362-368), touch-mode CSS hide (~569).
- `apps/desktop/src/catalog.js` - `stateActive` with `JSON.parse` (290-310).
- `apps/desktop/package.json` - declares `@tauri-apps/plugin-fs` (unused;
  no import anywhere in `apps/desktop/src`, and src-tauri registers no
  `tauri-plugin-fs`).

Excerpts at `c1e0086`:

```js
// App.vue:57 - new identity per push
function mergeCustomValues(data) {
  for (const [key, value] of Object.entries(data)) {
    if (typeof value !== "object" || value === null) {
      customValues[key] = value;                       // primitive: Vue skips same-value
    } else if (typeof value.value === "number") {
      const prev = customValues[key];
      const values = [...(prev?.values ?? []), value.value].slice(-10);
      customValues[key] = { ...value, values };        // NEW object every push
    } else {
      customValues[key] = value;                        // status snapshot replaced wholesale
    }
  }
}
```

```js
// catalog.js:301 - per-evaluation parse
let cmd = {};
try { cmd = JSON.parse(tile.command) || {}; } catch { cmd = {}; }
```

```js
// GridEditor.vue:290 - pointer handlers with no pointercancel path
window.addEventListener("pointermove", onMove);
window.addEventListener("pointerup", onUp);
// (second site at 366-367; removal only inside onUp at 275-276/362-363)
```

Conventions:

- Plain `<script setup>` components, props via `defineProps`, no
  TypeScript, no state library, no router. Comments are short `//` lines
  explaining original-app parity ("like the stock client").
- The drag code relies on Vue 3 same-value assignments not triggering
  reactivity - do not introduce extra reactive writes in move handlers.
- No frontend test framework exists; verification is `npx vite build` and
  the manual smoke list in Test plan. `touch mode` is a mode of the same
  window toggled by the `toggle-touch-mode` Tauri event.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Frontend build | `cd apps/desktop && npx vite build` | exit 0 |
| Backend untouched | `git status -- apps/desktop/src-tauri` | no changes |

## Scope

**In scope**:
- `apps/desktop/src/App.vue`
- `apps/desktop/src/components/GridEditor.vue`
- `apps/desktop/src/components/EditTileModal.vue` (only if a prop must change shape)
- `apps/desktop/src/catalog.js`
- `apps/desktop/package.json` (+ `package-lock.json` via `npm uninstall`)

**Out of scope**:
- Any Rust file - the emit cadences are plan 004's territory.
- `BoardModal.vue`, `api.js`, `main.js` (except nothing; leave them).
- Redesigning the tile styling model (`pick`/dual-state semantics must
  produce identical rendered output).

## Git workflow

- Branch: `perf/frontend-render-gating`
- Commit per step; message style: `editor: render tiles as isolated
  components` (area prefix, lowercase imperative).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Gate state merges on visibility, flush on show

In `App.vue`:

1. Add module state `let hidden = document.hidden;` and one
   `visibilitychange` listener registered in `onMounted` (and removed in
   the existing `onUnmounted` cleanup block at ~414-419, which already
   removes other window listeners - follow its pattern).
2. Buffer: `let pendingStatus = null;`. In the `app-status-update`
   handler: if `hidden`, store `pendingStatus = payload` (overwrite -
   latest wins, matching the Rust side's latest-value semantics) and
   return; otherwise run `applyStatusUpdate(payload)` as today.
3. On `visibilitychange` → visible: `hidden = false;` if `pendingStatus`
   { applyStatusUpdate(pendingStatus); pendingStatus = null; }`.

Note: while hidden, the sparkline `values` arrays will miss samples -
same behavior as the original client's backgrounded webview, and the
Rust emitters keep the tiles on the tablets correct. State a one-line
comment saying exactly that.

**Verify**: `cd apps/desktop && npx vite build` → exit 0.

### Step 2: Stable identity for unchanged values

In `App.vue::mergeCustomValues`:

- Graph branch: if `prev` exists, `prev.value === value.value` AND the
  non-`value` fields are unchanged (compare with a shallow
  `JSON.stringify` of `{...value, values: []}` against the same shape
  stored from the last accepted push - payloads are ≤ a few KB, so this
  is cheap), then only append to `prev.values` IN PLACE
  (`prev.values.push(value.value); if (prev.values.length > 10)
  prev.values.shift();`) and reassign `customValues[key] = prev` only if
  `value.value` changed (same-object reassignment with unchanged
  `values` triggers nothing in Vue 3 - but keep the reassignment for the
  changed case).
- Status branch: compare `JSON.stringify(value) ===
  JSON.stringify(customValues[key])`; skip the assignment when equal.
- `mergeAppState`: skip when all incoming keys equal the stored ones
  (iterate entries, compare, only assign changed keys).

Keep the code dead simple over clever - a helper
`jsonEqual(a, b)` is fine.

**Verify**: `npx vite build` → exit 0.

### Step 3: Extract the per-tile component

In `GridEditor.vue`:

1. Create `apps/desktop/src/components/TileCell.vue` with
   `defineProps(["tile", "customValues", "appStates", "typeMeta",
   "active", "editing"])` - move the per-tile template block (the tile
   `v-for` body at ~405-445: style bindings, icon, title, live value
   label, status rows, sparkline) and the helper functions that depend
   only on one tile (`tileBg`, `tileIcon`, `pick` variants,
   `customValueLabel`, `graphData`, `statusData`, `statusRows` - the
   bodies move as-is). `tileActive` needs the `activeTiles` Set for the
   editor-preview case: pass `active` (boolean, computed in the parent
   per tile from `activeTiles.has(tile.id)`) and do the live-state part
   inside `TileCell` via `stateActive(...)`.
2. The parent's tile `v-for` becomes
   `<TileCell v-for="tile in tiles" :key="tile.id" ... />`. Keep drag
   event binding (`@pointerdown` etc.) on `TileCell`'s root element via
   emits - the drag logic stays in GridEditor.
3. Memoize per-tile parses inside `TileCell`: convert the `pick` chain to
   read from `computed`s (e.g. `const cmd = computed(() => { try {
   return JSON.parse(props.tile.command) || {}; } catch { return {}; } });`
   and have `stateActive` accept the parsed `cmd` - adjust
   `catalog.js::stateActive` signature to take `(tile, cmd, customValues,
   appStates, typeMeta)` so the parse happens once per tile per command
   change, not per helper call). Update all `stateActive` callers
   (grep `stateActive(` in src/).
4. Empty cells: render the `v-for="c in emptyCells"` block only when
   editing (`v-if="editing"` - find the exact prop/flag GridEditor uses
   to know touch mode; it is the same condition the CSS class `.touch`
   keys off, ~line 569). Touch mode then has zero empty-cell nodes.

GridEditor keeps: grid geometry, drag/resize, context menu, board
background, mode switching.

**Verify**: `npx vite build` → exit 0. Manual smoke (Test plan) - this is
the step where visual regressions would appear.

### Step 4: Pointer-drag cleanup + dead dependency

1. `GridEditor.vue`: in BOTH drag sites (275-292 and 362-368), add
   `window.addEventListener("pointercancel", onUp);` alongside the
   existing move/up registrations and remove it in `onUp` next to the
   others. `onUp` already handles "drag finished" generically -
   pointercancel should route there, not to a new handler.
2. `apps/desktop`: `npm uninstall @tauri-apps/plugin-fs` (verify no
   import breaks the build).

**Verify**: `npx vite build` → exit 0;
`grep -rn "plugin-fs" apps/desktop/package.json apps/desktop/src` →
no matches.

## Test plan

No frontend test framework - verification is build + smoke:

- `npx vite build` after every step.
- Manual smoke in `npm run tauri dev` (or the installed app):
  1. Open editor with a board containing: an image tile, a dual-state
     toggle bound to `speaker-muted`, a graph tile (ai-tokens-hour), a
     status tile (ai-agent-status). All render exactly as before
     (screenshot before starting, if possible).
  2. Wait 30 s: sparkline still advances; status rows still update
     (ages change).
  3. Drag to resize a tile; drop it. No console errors.
  4. Toggle touch mode (Ctrl+Alt+D): empty grid slots are NOT rendered
     (inspect: `document.querySelectorAll(".empty-cell").length === 0`),
     tiles still execute on tap, slider drag works.
  5. Hide to tray 60 s, show again: values are current (volume tile
     matches the mixer).
  6. DevTools performance sanity (optional): with the window visible,
     the flame chart shows TileCell-scoped component updates on a push,
     not a GridEditor-wide render.

## Done criteria

- [ ] `cd apps/desktop && npx vite build` exits 0
- [ ] `grep -rn "JSON.parse" apps/desktop/src/components/GridEditor.vue apps/desktop/src/components/TileCell.vue`
      shows the parse only inside a `computed`
- [ ] `grep -c "pointercancel" apps/desktop/src/components/GridEditor.vue` ≥ 2
- [ ] `grep -rn "plugin-fs" apps/desktop/package.json` → no match
- [ ] `git status -- apps/desktop/src-tauri` clean
- [ ] Manual smoke list executed and passing
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above do not match the live code (drift).
- The tile template in GridEditor.vue is so entangled with drag state that
  extraction requires rewriting the drag logic - report the coupling you
  found instead of restructuring it.
- `stateActive` semantics cannot be preserved with a pre-parsed `cmd`
  (the catalog tests - none exist - would not catch it; the smoke test's
  dual-state toggle must).
- The editor's second-state preview (`activeTiles`) renders wrong after
  Step 3 and the cause is not the `active` prop plumbing.

## Maintenance notes

- Future widget kinds (M5 knob/list/graph templates) should land as
  TileCell internals or children - never back into the GridEditor render
  function.
- Plan 004 lands Rust-side emit gating for hidden windows; this plan's
  Step 1 remains the WebView-side belt to those suspenders (events can
  still arrive while visible-but-unfocused).
- Reviewer: diff a before/after screenshot of a mixed board; behavior
  parity is the acceptance bar, not code shape.
