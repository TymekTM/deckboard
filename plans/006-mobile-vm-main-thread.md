# Plan 006: Confine PulpitViewModel state to the main thread

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`
> If the file changed since this plan was written, compare the "Current
> state" excerpts against the live code before proceeding; on a mismatch,
> treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (plans 007 and 008 build on this one)
- **Category**: bug
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

`PulpitViewModel` (the Android tablet client's whole app state) runs its
coroutines on `CoroutineScope(Job())`. That scope has no dispatcher, so
everything defaults to `Dispatchers.Default`, a multi-threaded pool.
Meanwhile Compose calls into the same ViewModel from the main thread.
Plain `var`s, a `HashSet`, and read-modify-write updates of
`MutableStateFlow`s are touched from both sides with no synchronization.
The concrete bug: two tile images finishing their download at the same
moment both run `_bitmaps.value = _bitmaps.value + (hash to bitmap)` on
different threads, and one write is lost. Its hash stays in
`assetFetches`, so `ensureAsset` never fetches it again and the tile
shows no image until the app restarts.

Two smaller defects ride along:

- `Job()` (not `SupervisorJob()`): one uncaught exception in any child
  coroutine cancels the whole scope. After that the reconnect loop and
  the shutdown probe stop silently, and the deck hangs on "Connecting..."
  until the process dies.
- `forgetPairing()` clears `_bitmaps` but not `assetFetches`. Hashes that
  were fetched successfully stay "in flight" forever, so after re-pairing,
  those images never load again.

Confining all ViewModel state to the main thread
(`Dispatchers.Main.immediate`) removes the whole race class. Network IO
already runs inside `withContext(Dispatchers.IO)`, and JSON decoding
happens on OkHttp's reader thread in `V2Client`, so the main thread only
does small map merges.

## Current state

Files:

- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`
  holds the app state, the connection lifecycle, and asset fetching. This
  is the only file in scope.
- `apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt` is
  the WebSocket client. It decodes frames on OkHttp's thread and hands
  them over through a thread-safe `Channel` and a `StateFlow`. Read it
  for context only; do not modify it.

Excerpts (line numbers at `f07f447`):

```kotlin
// PulpitViewModel.kt:24-32 (imports)
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// PulpitViewModel.kt:45-47
class PulpitViewModel(app: Application) : AndroidViewModel(app) {

    private val scope = CoroutineScope(Job())
```

```kotlin
// PulpitViewModel.kt:103-105
    /** Hashes with a fetch in flight or failed this process; failures are
     *  not retried - a 404 stays a 404 until the app restarts. */
    private val assetFetches = mutableSetOf<String>()
```

```kotlin
// PulpitViewModel.kt:159-195 (ensureAsset), the racy write is line 188
    fun ensureAsset(hash: String, attempt: Int = 0) {
        if (_bitmaps.value.containsKey(hash) || !assetFetches.add(hash)) return
        val cfg = _config.value
        val token = cfg.token ?: return
        scope.launch {
            val bitmap = withContext(Dispatchers.IO) {
                ...
            }
            if (bitmap != null) {
                _bitmaps.value = _bitmaps.value + (hash to bitmap.asImageBitmap())
            } else if (attempt < ASSET_RETRIES) {
                delay(30_000L * (attempt + 1))
                assetFetches.remove(hash)
                ensureAsset(hash, attempt + 1)
            }
        }
    }
```

```kotlin
// PulpitViewModel.kt:245-254
    fun forgetPairing() {
        saveConfig(_config.value.copy(token = null))
        disconnect()
        _boards.value = emptyList()
        _currentBoard.value = null
        _values.value = emptyMap()
        _series.value = emptyMap()
        _channelMeta.value = emptyMap()
        _bitmaps.value = emptyMap()
    }
```

`ensureAsset` is called from Compose `LaunchedEffect`s (main thread) in
`ui/BoardScreen.kt:294` and `:314`. `onCleared()` (line 151-154) already
calls `scope.cancel()`. Keep that.

No unit test instantiates `PulpitViewModel` (verify with the grep in
Step 1), so `Dispatchers.Main` being unavailable on the JVM test runner
does not matter.

Conventions: comments are full sentences explaining *why*, written in
the file's existing voice (see the KDoc blocks above). Keep the style.

**Three-surface note (AGENTS.md):** this changes no wire field or tile
behavior, only client-internal threading. The desktop and server need
nothing.

## Commands you will need

Gradle is not on PATH on the owner's machine. Use the full paths below
(verified at plan time; the baseline `testDebugUnitTest` passed in about
1 min). Elsewhere, any Gradle 8.7 + JDK 17 + Android SDK 34 works.

| Purpose | Command (bash, from repo root) | Expected on success |
|---|---|---|
| Unit tests | `cd apps/mobile && JAVA_HOME="C:/Users/Tymek/.jdks/openjdk-17.0.1" ANDROID_HOME="C:/android-sdk" "C:/Users/Tymek/gradle-8.7/bin/gradle" :app:testDebugUnitTest --console=plain` | `BUILD SUCCESSFUL` |
| Debug APK | same prefix, task `:app:assembleDebug` | `BUILD SUCCESSFUL` |

Two compiler warnings already exist and are not yours to fix:
`LocalLifecycleOwner` deprecation in `MainActivity.kt`, and a nullable
`ClassLoader` in `ProtoFixturesTest.kt`.

## Scope

**In scope**:
- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`

**Out of scope** (do NOT touch):
- `net/V2Client.kt`. Its threading is fine (OkHttp thread to Channel/StateFlow).
- Any reconnect, backoff, or lifecycle behavior change. Plans 007 and
  008 own those. Keep this diff purely about threading.
- Replacing the scope with `viewModelScope`. It would work, but it
  changes `onCleared` semantics and the ordering of `disconnect()`
  versus cancellation. The explicit scope keeps the diff minimal.

## Git workflow

- Branch: `mobile/vm-main-thread` (or the operator's branch if told).
- One commit, message style from `git log`: `mobile: confine view-model
  state to the main thread`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Confirm no test builds the ViewModel

**Verify**: `grep -rn "PulpitViewModel" apps/mobile/app/src/test` returns
no matches. If it returns matches, STOP. Those tests would crash without
a Main dispatcher.

### Step 2: Switch the scope to a main-thread supervisor scope

In `PulpitViewModel.kt`, replace line 47:

```kotlin
    private val scope = CoroutineScope(Job())
```

with:

```kotlin
    /** Every field below is confined to the main thread: Compose calls in
     *  from there, and this scope runs every coroutine there too. Blocking
     *  work hops to Dispatchers.IO explicitly (ensureAsset); frame decoding
     *  already happens on OkHttp's thread inside V2Client. SupervisorJob so
     *  one failed child cannot cancel the reconnect loop and the probe. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
```

Add the import `kotlinx.coroutines.SupervisorJob`, keeping the import
block alphabetical. Keep `kotlinx.coroutines.Job`, because `eventJob`
still uses that type.

**Verify**: `grep -n "SupervisorJob() + Dispatchers.Main.immediate" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` → exactly 1 match.

### Step 3: Update the assetFetches doc and clear it in forgetPairing

1. Change the KDoc above `assetFetches` (lines 103-104) to say that
   successful fetches also stay in the set, so `forgetPairing` must
   clear it. Suggested wording:
   `/** Hashes fetched, in flight, or failed this process (main thread
   only). Failures retry ASSET_RETRIES times with backoff; forgetPairing
   clears the set together with the bitmaps it guards. */`
2. In `forgetPairing()`, directly after `_bitmaps.value = emptyMap()`,
   add `assetFetches.clear()`.

**Verify**: `grep -n "assetFetches.clear()" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` → exactly 1 match, inside `forgetPairing`.

### Step 4: Build and test

**Verify**: run the Unit tests command, then the Debug APK command →
both `BUILD SUCCESSFUL`, with no new warnings beyond the two pre-existing
ones.

## Test plan

No JVM test can drive this. The ViewModel needs Android `Application`,
`SharedPreferences`, and the Main dispatcher, and the repo has no
Robolectric. The guarantee is structural: after the change, every
`scope.launch` body runs on the main thread. A reviewer checks it by
reading the diff: the only off-main code must be inside
`withContext(Dispatchers.IO)`.

Operator smoke test on a device (optional, not required for done): open
a board with several image tiles after a fresh app start (`adb shell am
force-stop app.pulpit.mobile`, then launch), five times in a row. Every
image tile should show its image each time.

## Done criteria

- [ ] `grep -rn "CoroutineScope(Job())" apps/mobile/app/src/main` → no matches
- [ ] `grep -n "SupervisorJob() + Dispatchers.Main.immediate" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` → 1 match
- [ ] `grep -n "assetFetches.clear()" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` → 1 match
- [ ] Unit tests command → `BUILD SUCCESSFUL`
- [ ] Debug APK command → `BUILD SUCCESSFUL`
- [ ] `git status` shows only `PulpitViewModel.kt` (and `plans/README.md`) modified
- [ ] `plans/README.md` row for 006 updated

## STOP conditions

- Step 1 grep finds a test that constructs `PulpitViewModel`.
- Something inside `scope.launch` turns out to do blocking IO outside
  `withContext(Dispatchers.IO)`, such as a synchronous `execute()` or a
  file read. Moving it to main would freeze the UI. Report it; do not
  wrap it on your own.
- The build fails with an error about the missing `Dispatchers.Main`
  (that would mean `kotlinx-coroutines-android` is no longer in
  `app/build.gradle.kts`).

## Maintenance notes

- New code in this ViewModel must stay main-confined. Anything blocking
  goes in `withContext(Dispatchers.IO)`, and the result is written back
  after the `withContext` returns.
- Plans 007 and 008 add state (`retryable` failures, a standby timer,
  a reconnect `Job`) that relies on this confinement. Land 006 first.
- If patch handling (`V2Event.Patch`) ever becomes heavy (thousands of
  channels), move the map merge into `withContext(Dispatchers.Default)`
  and write the result back on main, rather than reverting the scope.
