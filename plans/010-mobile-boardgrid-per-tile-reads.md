# Plan 010: Recompose only the tiles whose channel changed

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt`
> Plan 009 is expected to have landed (it adds `key(t.id)` and one
> import). Any other change to `BoardGrid` is a STOP condition.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/009-mobile-gesture-lambdas-tile-keys.md
- **Category**: perf (battery)
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

While the deck is live, the server sends a `state.patch` up to 10 times a
second whenever any channel changes (`crates/v2/src/service.rs:44`,
100 ms flusher). It broadcasts every channel to every session,
including channels for boards that are not on screen. Each patch
replaces the whole `values` map (and the `series` map) in
`PulpitViewModel`. `BoardGrid` reads both maps directly in its own
scope, so **every patch recomposes the whole grid**. For every tile on
screen it then re-runs:

- `displayText`, `isActiveValue`
- `listItems` and `statusData`, which walk JSON and build lists (the
  ai-dev status tiles parse rows each time)
- allocates `SeriesWindow` / `TileItems` wrappers

Most `Tile` bodies then skip, because their inputs are equal. But the
grid-level work is repeated for all tiles even when only one unrelated
channel (for example a CPU graph on another board) changed. On the
target tablet (Exynos 7870-class CPU) this keeps the CPU out of idle
for a few milliseconds every 100 ms, with the screen on, all day.

After this plan, each tile reads only *its own* channel through
`derivedStateOf`. A patch to channel A re-runs a cheap map lookup per
tile, but recomposes only the tiles bound to A.

This is a smaller win than plan 008 (the radio and screen dominate
battery use), so it is P3. It is MED risk because a mistake here makes
a tile stop updating live. Follow the steps exactly and run the device
check.

**Three-surface note (AGENTS.md):** client rendering internals only.
Same inputs, same output per tile; nothing changes on the wire or on
the desktop. (Per-board channel subscription on the server would cut
the traffic itself. That is a protocol change, listed as a deferred
direction item in `plans/README.md`, not part of this plan.)

## Current state

`apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt`, as
of `f07f447` plus plan 009 (which wrapped the loop body in `key(t.id)`):

```kotlin
// BoardScreen.kt:282-342
@Composable
private fun BoardGrid(vm: PulpitViewModel, board: Board, modifier: Modifier) {
    val liveValues by vm.values.collectAsState()
    val series by vm.series.collectAsState()
    val channelMeta by vm.channelMeta.collectAsState()
    val bitmaps by vm.bitmaps.collectAsState()
    // toggles without a state channel keep client-side position state
    val positions = remember(board.id) { mutableStateMapOf<Long, Boolean>() }

    // a board image wins over the color (like the original client); the
    // color shows through while the asset loads
    val bgAsset = board.background?.hash
    LaunchedEffect(bgAsset) { bgAsset?.let { vm.ensureAsset(it) } }
    val bgBitmap = bgAsset?.let { bitmaps[it] }

    BoxWithConstraints(
        modifier.background(hex(board.background?.color, DeckColors.background)),
    ) {
        bgBitmap?.let {
            Image(
                bitmap = it,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.matchParentSize(),
            )
        }
        val tile = maxWidth / board.width.coerceAtLeast(1)
        val tileHeight = maxHeight / board.height.coerceAtLeast(1)

        board.tiles.forEach { t ->
            key(t.id) {   // added by plan 009
                val watchChannel = t.state?.channel
                val live = liveValues[watchChannel]
                t.assetHash?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
                val active = when {
                    watchChannel != null -> isActiveValue(live)
                    else -> positions[t.id] ?: false
                }
                Box(
                    Modifier
                        .offset(x = tile * t.x, y = tileHeight * t.y)
                        .width(tile * t.w)
                        .height(tileHeight * t.h),
                ) {
                    Tile(
                        tile = t,
                        tileSize = tile,
                        active = active,
                        liveText = displayText(live),
                        series = SeriesWindow(series[watchChannel] ?: emptyList()),
                        channel = watchChannel?.let { channelMeta[it] },
                        items = TileItems(listItems(t, live)),
                        status = statusData(t, live),
                        image = t.assetHash?.let { bitmaps[it] },
                        onPressStart = { vm.pressStart(board.id, t) },
                        onPressEnd = { vm.pressEnd(board.id, t) },
                        onSlider = { v -> vm.slider(board.id, t, v) },
                    )
                }
            }
        }
    }
}
```

Facts the design relies on (verified at plan time):

- `PulpitViewModel` types (`state/PulpitViewModel.kt:92-128`):
  `values: StateFlow<Map<String, JsonElement>>`,
  `series: StateFlow<Map<String, List<Double>>>`,
  `bitmaps: StateFlow<Map<String, ImageBitmap>>`,
  `channelMeta: StateFlow<Map<String, ChannelInfo>>`.
- The patch handler (`PulpitViewModel.kt:341-363`) copies the map and
  replaces only changed channels. Untouched entries keep the **same
  instance**, so an equality check on them is cheap.
- `BoardGrid` is called twice during a board-switch slide
  (`BoardScreen.kt:216` and `:223`). Nothing in this plan depends on
  that.
- Kotlin 2.0.20 with the Compose compiler plugin: strong skipping is on
  by default. A composable with unchanged arguments (same `vm` instance,
  equal `@Immutable` `Tile`, same `State` objects) is skipped.
- Name clash to be aware of: inside package `app.pulpit.mobile.ui`,
  `Tile(...)` is the composable from `Tile.kt`, and the data class is
  `app.pulpit.mobile.proto.Tile`. `BoardScreen.kt` does not import the
  data class. Write its type fully qualified in the new signature, the
  same way `Widgets.kt:479` writes `kotlinx.serialization.json.JsonElement`.
  Do **not** add `import app.pulpit.mobile.proto.Tile` to `BoardScreen.kt`.

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
- `apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt`
  (`BoardGrid` + a new private `TileCell` + imports)

**Out of scope** (do NOT touch):
- `PulpitViewModel.kt`. Do not split the maps into per-channel flows;
  the derived reads make that unnecessary.
- `Tile.kt`, `Widgets.kt`, `TileModels.kt`. Their signatures stay as
  they are.
- The `positions` map semantics. It is currently never written (a
  known gap, tracked in `plans/README.md`). Keep passing it through
  unchanged.
- Server-side patch rate or channel filtering.

## Git workflow

- Branch: `mobile/boardgrid-per-tile-reads` (or the operator's branch).
- One commit: `mobile: recompose only tiles whose channel changed`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: BoardGrid holds State objects, not map snapshots

In `BoardGrid`, replace the four delegated reads:

```kotlin
    val liveValues by vm.values.collectAsState()
    val series by vm.series.collectAsState()
    val channelMeta by vm.channelMeta.collectAsState()
    val bitmaps by vm.bitmaps.collectAsState()
```

with:

```kotlin
    // State holders, not snapshots: reading .value here would recompose
    // the whole grid on every state.patch (10 Hz while anything moves).
    // Each TileCell derives just its own channel from them.
    val values = vm.values.collectAsState()
    val series = vm.series.collectAsState()
    val channelMeta = vm.channelMeta.collectAsState()
    val bitmaps = vm.bitmaps.collectAsState()
```

Replace the background bitmap line
`val bgBitmap = bgAsset?.let { bitmaps[it] }` with:

```kotlin
    val bgBitmap by remember(bitmaps, bgAsset) {
        derivedStateOf { bgAsset?.let { bitmaps.value[it] } }
    }
```

### Step 2: Extract the per-tile body into TileCell

Replace the body inside `key(t.id) { ... }` with:

```kotlin
            key(t.id) {
                Box(
                    Modifier
                        .offset(x = tile * t.x, y = tileHeight * t.y)
                        .width(tile * t.w)
                        .height(tileHeight * t.h),
                ) {
                    TileCell(vm, board.id, t, tile, values, series, channelMeta, bitmaps, positions)
                }
            }
```

Add this private composable directly below `BoardGrid`:

```kotlin
/** One grid tile. Reads only its own channel, series window, and image,
 *  so a patch for another channel leaves it alone - the lambdas below
 *  re-run (a map lookup), the tile does not recompose. */
@Composable
private fun TileCell(
    vm: PulpitViewModel,
    boardId: Long,
    t: app.pulpit.mobile.proto.Tile,
    tileSize: Dp,
    values: State<Map<String, JsonElement>>,
    series: State<Map<String, List<Double>>>,
    channelMeta: State<Map<String, ChannelInfo>>,
    bitmaps: State<Map<String, ImageBitmap>>,
    positions: SnapshotStateMap<Long, Boolean>,
) {
    val watchChannel = t.state?.channel
    val live by remember(values, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) { watchChannel?.let { values.value[it] } }
    }
    val points by remember(series, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) {
            watchChannel?.let { series.value[it] } ?: emptyList()
        }
    }
    val meta by remember(channelMeta, watchChannel) {
        derivedStateOf(structuralEqualityPolicy()) { watchChannel?.let { channelMeta.value[it] } }
    }
    val image by remember(bitmaps, t.assetHash) {
        derivedStateOf { t.assetHash?.let { bitmaps.value[it] } }
    }
    t.assetHash?.let { hash -> LaunchedEffect(hash) { vm.ensureAsset(hash) } }
    val active = when {
        watchChannel != null -> isActiveValue(live)
        else -> positions[t.id] ?: false
    }
    Tile(
        tile = t,
        tileSize = tileSize,
        active = active,
        liveText = displayText(live),
        series = SeriesWindow(points),
        channel = meta,
        items = TileItems(listItems(t, live)),
        status = statusData(t, live),
        image = image,
        onPressStart = { vm.pressStart(boardId, t) },
        onPressEnd = { vm.pressEnd(boardId, t) },
        onSlider = { v -> vm.slider(boardId, t, v) },
    )
}
```

Notes for the executor:

- Keep every expression feeding `Tile(...)` identical in meaning to the
  old code. The only change is *where* the maps are read.
- The `image` (and `bgBitmap`) derivations pass no policy on purpose.
  `ImageBitmap` does not override `equals`, so any policy compares
  references.
- Do not wrap `displayText`, `listItems`, or `statusData` in `remember`.
  `TileCell` now recomposes only when `live` (or `t`) changes, which is
  exactly when they must re-run.

### Step 3: Imports

Add to `BoardScreen.kt` (next to their siblings; keep the file's loose
grouping):

```kotlin
import androidx.compose.runtime.State
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.runtime.structuralEqualityPolicy
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.unit.Dp
import app.pulpit.mobile.proto.ChannelInfo
import kotlinx.serialization.json.JsonElement
```

Skip any that already exist (`grep -n "^import" .../BoardScreen.kt`).
If `getValue` becomes unused after Step 1, keep it: the new `by`
delegates in `TileCell` use it.

**Verify**: Debug APK command → `BUILD SUCCESSFUL`, no new warnings
about unused variables in `BoardScreen.kt`.

### Step 4: Structural checks

**Verify**:
- `grep -n "liveValues" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` gives no matches.
- `grep -n "by vm.values.collectAsState\|by vm.series.collectAsState" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` gives no matches.
- `grep -c "derivedStateOf" apps/mobile/app/src/main/java/app/pulpit/mobile/ui/BoardScreen.kt` gives `6` (import + bg + 4 in TileCell; the file had none at `f07f447`).
- Unit tests command → `BUILD SUCCESSFUL`.

## Test plan

No JVM test can observe recomposition (no Compose UI test setup in the
repo). Verification is the build, the structural greps, and this
**required** device check by the operator before marking the plan DONE.
The reviewer must not accept a DONE without it.

1. Install the debug APK on the tablet (`adb install -r`). Open a board
   with: a toggle bound to a channel (for example Discord mute), a graph
   (CPU), a text/value tile, an image tile, and an ai-dev status tile if
   available.
2. Toggle mute on the PC. The toggle tile flips within about 1 s.
3. The graph keeps scrolling and the value tile keeps updating.
4. The image tile shows its image. Edit the tile's image on the desktop;
   the new image appears.
5. Switch boards with the switcher and back. Everything still updates.
6. Optional evidence: Android Studio Layout Inspector with
   "Show recomposition counts". The status/image tiles' counts should
   stay flat while only the CPU graph's count rises.

## Done criteria

- [ ] Structural greps in Step 4 pass
- [ ] Unit tests and Debug APK commands → `BUILD SUCCESSFUL`
- [ ] Device check steps 1-5 performed and reported by the operator
- [ ] `git status`: only `BoardScreen.kt` (+ `plans/README.md`) changed

## STOP conditions

- Plan 009's `key(t.id)` is not present in `BoardGrid`.
- `BoardGrid` contains per-tile logic not shown in "Current state" (new
  parameters to `Tile(...)`, new effects). Report it; the extraction must
  carry everything and you would be guessing.
- The compiler resolves `Tile(...)` in `TileCell` to the data-class
  constructor (an error mentioning `proto.Tile` constructor parameters).
  That means an import of `app.pulpit.mobile.proto.Tile` was added
  somewhere. Remove it and use the fully qualified type.
- In device step 2 or 3 a tile stops updating. Revert, and report which
  tile type failed.

## Maintenance notes

- New per-tile inputs derived from a ViewModel map belong in `TileCell`,
  read through `derivedStateOf`. Do not read `values.value` (or
  `series.value`, etc.) directly in `BoardGrid`; that reintroduces
  whole-grid recomposition.
- If a future tile needs *several* channels, derive a small immutable
  snapshot of exactly those channels in one `derivedStateOf`.
- The real traffic fix is server-side: subscribe a session only to the
  channels of the boards it shows. See the deferred direction items in
  `plans/README.md`.
