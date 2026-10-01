# Plan 009: Fresh gesture callbacks after live tile edits, and stable tile identity in the grid

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- apps/mobile/app/src/main/java/app/pulpit/mobile/ui`
> If any of `Tile.kt`, `Widgets.kt`, `BoardScreen.kt` changed, compare the
> excerpts below with the live code; on a mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (independent of 006-008; plan 010 builds on this one)
- **Category**: bug
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

The desktop now pushes tile edits to tablets live (`board.delta`, wired
via `publish_delta` in `apps/desktop/src-tauri/src/lib.rs`). The tablet
applies them to its board list, and Compose recomposes with a new `Tile`
object that has the same `id`. Two defects make the tablet act on stale
data after such an edit:

1. **Stale gesture callbacks.** `ButtonTile`, `SliderTile`, and
   `KnobTile` install their gesture handlers with
   `Modifier.pointerInput(tile.id) { ... }`. A `pointerInput` block is
   started once per key and keeps the lambdas it captured at that moment.
   The callbacks (`onPressStart`, `onPressEnd`, `onSlider`) close over the
   **old** `Tile`, and `PulpitViewModel.pressStart/pressEnd/slider` decide
   what to send from `tile.interacts(...)`. So after the owner changes a
   tile's interaction on the desktop (for example tap → hold-to-repeat
   `press-start`/`press-end`, or enables slide), the tablet keeps sending
   the old gestures, or none at all, until the board is left and reopened
   or the app restarts. Nothing is logged; the tile simply "does nothing"
   or does the wrong thing.
2. **Positional identity in the grid.** `BoardGrid` renders
   `board.tiles.forEach { t -> ... }` without `key(t.id)`. Compose then
   matches tiles to their remembered state by position in the list. When
   a delta adds or removes a tile in the middle, every later tile inherits
   its neighbor's animation state (`animate*AsState` in `Tile`),
   `LaunchedEffect`s restart, and `pointerInput` blocks are torn down and
   rebuilt for tiles that did not change. The visible symptom is tiles
   briefly animating from another tile's color or press state after an
   edit, plus wasted work on the tablet's slow CPU.

Both fixes are standard Compose idioms: `rememberUpdatedState` for
callbacks read inside long-lived gesture blocks, and `key(id)` for list
items.

**Three-surface note (AGENTS.md):** client-only rendering fix. The
desktop editor and touch mode are Vue (no `pointerInput` semantics), and
the server already sends the correct deltas. Nothing changes on the wire.

## Current state

Files (under `apps/mobile/app/src/main/java/app/pulpit/mobile/ui/`):

```kotlin
// Tile.kt:239-272 (ButtonTile), the gesture block at 258-271
@Composable
private fun ButtonTile(
    tile: Tile,
    ...
    onPressStart: () -> Unit,
    onPressEnd: () -> Unit,
) {
    ...
    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(tile.id) {
                detectTapGestures(
                    onPress = {
                        onPressStart()
                        try {
                            awaitRelease()
                        } finally {
                            onPressEnd()
                        }
                    },
                )
            },
```

`Tile` passes `ButtonTile` wrapper lambdas (Tile.kt:222-229) that set the
local `pressed` flag and then call the outer callbacks. These wrappers are
also re-created when the outer callbacks change. That is fine as long as
`ButtonTile` reads the *current* one.

```kotlin
// Tile.kt:315-345 (SliderTile)
private fun SliderTile(
    tile: Tile, baseColor: Color, icon: String, iconFamily: FontFamily,
    iconColor: Color,
    onSlider: (Float) -> Unit,
) {
    var value by remember(tile.id) { mutableFloatStateOf(0.5f) }
    val slide = remember(tile.id) { SlideThrottle() }
    ...
            .pointerInput(tile.id) {
                detectDragGestures(
                    onDragStart = { offset -> ...; slide.push(value, force = true, send = onSlider) },
                    onDrag = { change, _ -> ...; slide.push(value, send = onSlider) },
                    onDragEnd = { slide.push(value, force = true, send = onSlider) },
                )
            },
```

```kotlin
// Widgets.kt:335-374 (KnobTile), same shape: pointerInput(tile.id) with
// detectDragGestures whose onDrag and onDragEnd call
// slide.push(value, ..., send = onSlider)
```

```kotlin
// BoardScreen.kt:311-341 (BoardGrid)
        board.tiles.forEach { t ->
            val watchChannel = t.state?.channel
            val live = liveValues[watchChannel]
            t.assetHash?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
            ...
            Box(Modifier.offset(...).width(...).height(...)) {
                Tile(
                    tile = t,
                    ...
                    onPressStart = { vm.pressStart(board.id, t) },
                    onPressEnd = { vm.pressEnd(board.id, t) },
                    onSlider = { v -> vm.slider(board.id, t, v) },
                )
            }
        }
```

```kotlin
// state/PulpitViewModel.kt:427-444 - why the captured Tile matters
    fun pressStart(boardId: Long, tile: Tile) {
        if (tile.interacts(V2.INT_PRESS_START)) {
            client?.pressStart(boardId, tile.id)
        }
    }
    fun pressEnd(boardId: Long, tile: Tile) {
        when {
            tile.interacts(V2.INT_PRESS_END) -> client?.pressEnd(boardId, tile.id)
            tile.interacts(V2.INT_TAP) -> client?.tap(boardId, tile.id)
        }
    }
```

Leave `ListTile` alone. It uses `Modifier.clickable(onClick = ...)`,
which always calls the latest lambda.

Conventions: lowercase `//` comments explaining *why*. Imports are
grouped loosely, not strictly sorted. Add new imports next to their
siblings (`androidx.compose.runtime.*`).

## Commands you will need

| Purpose | Command (bash, from repo root) | Expected on success |
|---|---|---|
| Unit tests | `cd apps/mobile && JAVA_HOME="C:/Users/Tymek/.jdks/openjdk-17.0.1" ANDROID_HOME="C:/android-sdk" "C:/Users/Tymek/gradle-8.7/bin/gradle" :app:testDebugUnitTest --console=plain` | `BUILD SUCCESSFUL` |
| Debug APK | same prefix, task `:app:assembleDebug` | `BUILD SUCCESSFUL` |

Elsewhere: any Gradle 8.7 + JDK 17 + Android SDK 34. There are two
pre-existing warnings (`LocalLifecycleOwner` deprecation,
`ProtoFixturesTest` nullable ClassLoader); they are not yours to fix.

## Scope

**In scope**:
- `apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Tile.kt`
- `apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Widgets.kt`
- `apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` (only the `forEach` in `BoardGrid` + one import)

**Out of scope** (do NOT touch):
- Changing the `pointerInput` keys to include the callbacks or the
  `Tile` itself. That restarts the gesture detector on every patch and
  can cut an in-progress hold or drag. `rememberUpdatedState` is the
  intended fix.
- Restructuring `BoardGrid`'s state reads. Plan 010 does that.
- Slider/knob starting value (`mutableFloatStateOf(0.5f)`, never synced
  from the server). That is a known feature gap, tracked in `plans/README.md`.
- `ListTile`, `PulpitViewModel`.

## Git workflow

- Branch: `mobile/gesture-lambdas-tile-keys` (or the operator's branch).
- One commit: `mobile: read current gesture callbacks, key grid tiles by id`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: ButtonTile reads the current callbacks

In `Tile.kt`:

1. Add `import androidx.compose.runtime.rememberUpdatedState` next to
   the other `androidx.compose.runtime` imports.
2. At the top of `ButtonTile`'s body (before `val title = ...`), add:

```kotlin
    // the gesture block below lives as long as tile.id; a live tile edit
    // (board.delta) swaps the callbacks underneath it, so read the newest
    val pressStart by rememberUpdatedState(onPressStart)
    val pressEnd by rememberUpdatedState(onPressEnd)
```

3. Inside the `detectTapGestures(onPress = { ... })` block, replace
   `onPressStart()` with `pressStart()` and `onPressEnd()` with
   `pressEnd()`. Keep the `try`/`finally` shape unchanged.

`getValue` is already imported in `Tile.kt` (line 29).

**Verify**: `grep -n "rememberUpdatedState" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Tile.kt`
shows the import plus 2 uses. `grep -n "onPressStart()\|onPressEnd()" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Tile.kt`
shows only the wrapper calls in `Tile` (around lines 222-229). There
should be no match inside `ButtonTile`'s `pointerInput`.

### Step 2: SliderTile reads the current onSlider

In `SliderTile` (`Tile.kt`), next to `val slide = remember(tile.id) { ... }`, add:

```kotlin
    // see ButtonTile: the drag block outlives a live tile edit
    val send by rememberUpdatedState(onSlider)
```

In its `detectDragGestures` block, replace each `send = onSlider` with
`send = send` (3 occurrences: onDragStart, onDrag, onDragEnd). If the
named-argument-equals-local reads awkwardly, name the local `sendSlide`
and use `send = sendSlide`. Either is fine; be consistent with Step 3.

### Step 3: KnobTile reads the current onSlider

In `Widgets.kt`:

1. Add `import androidx.compose.runtime.rememberUpdatedState` next to
   the other runtime imports.
2. In `KnobTile`, next to `val slide = remember(tile.id) { ... }`, add the
   same `val ... by rememberUpdatedState(onSlider)` line, using the name
   you chose in Step 2, with the comment `// see ButtonTile (Tile.kt): the drag block outlives a live tile edit`.
3. Replace each `send = onSlider` in its `detectDragGestures` block (2
   occurrences: onDrag, onDragEnd).

**Verify**: `grep -rn "send = onSlider" apps/mobile/app/src/main` gives no matches.
Debug APK command → `BUILD SUCCESSFUL`.

### Step 4: Key the grid tiles by id

In `BoardScreen.kt`:

1. Add `import androidx.compose.runtime.key` next to the other
   `androidx.compose.runtime` imports (lines 37-45).
2. Wrap the body of `board.tiles.forEach { t -> ... }` (BoardGrid, line
   311) in `key(t.id) { ... }`:

```kotlin
        board.tiles.forEach { t ->
            // identity by tile id, not list position: a delta that adds or
            // removes a tile must not hand its neighbors' animation and
            // gesture state to the wrong tile
            key(t.id) {
                val watchChannel = t.state?.channel
                ...unchanged body...
            }
        }
```

Re-indent the moved body by 4 spaces. Change nothing else inside it.

**Verify**: `grep -n "key(t.id)" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` gives 1 match.
Unit tests command and Debug APK command → `BUILD SUCCESSFUL`.

## Test plan

No JVM test is possible: the repo has no Compose UI test or Robolectric
setup (`app/src` has only `main/` and `test/`, and `test/` holds pure
logic tests). Adding that infrastructure is out of scope. The fix is
verified by review and on the device.

Operator device check (optional for done):
1. On the desktop, make a key tile with a single tap action and open its
   board on the tablet. A tap fires the action.
2. Without touching the tablet, change that tile on the desktop to a
   hold-to-repeat key (press-start/press-end), and save.
3. Hold the tile on the tablet. It should now repeat. Before this fix it
   kept sending a single tap until the board was reopened.
4. Add a tile in the middle of the board on the desktop. The tiles after
   it on the tablet should not flash or animate.

## Done criteria

- [ ] `grep -rn "send = onSlider" apps/mobile/app/src/main` → no matches
- [ ] `grep -c "rememberUpdatedState" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Tile.kt` → `4` (import + 2 in ButtonTile + 1 in SliderTile)
- [ ] `grep -c "rememberUpdatedState" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/Widgets.kt` → `2`
- [ ] `grep -n "key(t.id)" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` → 1 match
- [ ] Unit tests and Debug APK commands → `BUILD SUCCESSFUL`
- [ ] `git status`: only the 3 in-scope files (+ `plans/README.md`) changed

## STOP conditions

- A `pointerInput` in these files uses a key other than `tile.id`, or
  more gesture blocks exist than the three named here (`grep -rn
  "pointerInput(" apps/mobile/app/src/main/java/app/pulpit/mobile/ui`).
  The BoardScreen overlay's `pointerInput(Unit)` that swallows taps is
  expected and needs nothing. Any *other* one calling a parameter
  lambda: report it rather than guessing.
- The compiler rejects `send = send` shadowing. Use the `sendSlide` name
  from Step 2 instead; this is not a STOP. Any other compile error that
  the steps do not explain is a STOP.

## Maintenance notes

- Rule for this codebase: any lambda *parameter* used inside
  `pointerInput`, `LaunchedEffect`, or another long-lived block keyed on
  an id must go through `rememberUpdatedState`. Reviewers should check
  new widgets for it.
- Plan 010 extracts the per-tile body of `BoardGrid` into its own
  composable. The `key(t.id)` stays at the call site.
