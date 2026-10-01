# Plan 008: Let the tablet sleep: close the link in background, release the screen after 3 min offline

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- apps/mobile`
> Plans 006 and 007 are expected to have landed first (see "Depends on").
> Their changes are described under "Current state". Any *other* change
> to the in-scope files is a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/006-mobile-vm-main-thread.md, plans/007-mobile-terminal-auth-failures.md
- **Category**: perf (battery)
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

The owner reports heavy battery drain on the deck tablet (Samsung
SM-T561, LineageOS, Android 7.1, LCD). Two app behaviors keep the
hardware awake for no visible benefit.

1. **The screen is held on with no PC to talk to.** `MainActivity` sets
   `FLAG_KEEP_SCREEN_ON` and drops it only when the server sends
   `server.shutdown`. The desktop sends that goodbye only on an explicit
   app exit (`apps/desktop/src-tauri/src/lib.rs:207-211`, `RunEvent::ExitRequested`),
   not when Windows sleeps, crashes, or loses Wi-Fi. So every night the
   PC sleeps, the tablet keeps its backlight on until morning and
   retries the connection every 12 s.
2. **The socket stays open in the background.** When the screen is
   turned off with the power button, or the user leaves the app, the
   WebSocket stays up. The server keeps pushing `state.patch` frames (its
   flusher runs at 10 Hz, `crates/v2/src/service.rs:44`) and pings. Each
   frame wakes the Wi-Fi radio and the CPU to decode JSON nobody sees,
   and if the link drops, the retry loop keeps running too. Today the
   background path disconnects only in shutdown standby.

After this plan:

- On `ON_STOP` (screen off or app hidden), the paired client closes the
  socket and cancels pending retries. On `ON_START` it reconnects at
  once. Every connect already receives a full snapshot
  (`boards.sync` + `state.sync`), so nothing goes stale.
- If the link stays down for **3 minutes** (owner's choice) while the
  board is on screen, the app stops holding the screen on. The system
  screen timeout then turns the display off, `ON_STOP` follows, and with
  it all network activity stops. The deck wakes when the user presses
  power or taps (the system's normal wake) and immediately looks for the
  PC again.
- Connect and goodbye screens never hold the screen on. Only a live (or
  briefly reconnecting) board does.

This mirrors a decision already made in this codebase for the shutdown
standby (`PulpitViewModel.kt` init block comment: "Background = dark
screen = no attempts at all"; `docs/protocol-v2.md` §9: the client "can
drop any keep-awake behavior"). The owner explicitly does **not** want
screen brightness changed. Do not touch brightness.

**Three-surface note (AGENTS.md):** client-only. The protocol and the
server are unchanged. A client closing its socket is a normal WS close,
which the server's session teardown already handles
(`crates/v2/src/session.rs` `run`).

## Current state

Files (all under `apps/mobile/app/src/main/java/app/pulpit/mobile/`
unless noted):

- `MainActivity.kt` owns the screen choice and the keep-screen-on flag.
- `state/PulpitViewModel.kt` owns the connection lifecycle, reconnect
  backoff, shutdown standby, and the foreground flag.
- `state/LinkPolicy.kt` is **new**: pure, unit-tested policy helpers.
- `apps/mobile/README.md` documents the "dedicated deck" behavior and
  needs a wording update.
- Test location pattern: `apps/mobile/app/src/test/java/app/pulpit/mobile/DeltasTest.kt`
  (file in the flat test dir, `package app.pulpit.mobile.state`, JUnit4).

### MainActivity.kt as of f07f447, plus plan 007's change

```kotlin
// MainActivity.kt:20-66 (plan 007 added the `refused` val and the
// `!refused &&` in the middle condition)
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // This device is the deck - keep the board visible, not dozing.
        // The shutdown overlay drops the flag again: nothing to watch, so
        // the screen may sleep (re-added when the deck comes back).
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent {
            DeckboardTheme {
                val vm: PulpitViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                val boards by vm.boards.collectAsState()
                val serverDown by vm.serverDown.collectAsState()

                DisposableEffect(LocalLifecycleOwner.current) {
                    val observer = LifecycleEventObserver { _, event ->
                        when (event) {
                            Lifecycle.Event.ON_START -> vm.onAppForeground()
                            Lifecycle.Event.ON_STOP -> vm.onAppBackground()
                            else -> {}
                        }
                    }
                    lifecycle.addObserver(observer)
                    onDispose { lifecycle.removeObserver(observer) }
                }
                DisposableEffect(serverDown) {
                    if (serverDown) {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    } else {
                        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                    onDispose {}
                }
                // (plan 007) val refused = (conn as? ...ConnState.Failed)?.retryable == false
                if (serverDown) {
                    ShutdownScreen(onTap = vm::reconnectFromShutdown)
                } else if (/* !refused && */ conn is app.pulpit.mobile.net.ConnState.Connected || boards.isNotEmpty()) {
                    BoardScreen(vm)
                } else {
                    ConnectScreen(vm, onConnected = {})
                }
            }
        }
    }
}
```

### PulpitViewModel.kt as of f07f447, plus plans 006/007

Relevant members (line numbers drift after 006/007; find them by name):

```kotlin
    // after 006: private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** Tracks the Activity's STARTED/STOPPED so the silent probe sleeps
     *  with the screen: no connect attempts while the tablet is dozing. */
    @Volatile private var foreground = false

    private var client: V2Client? = null
    private var eventJob: Job? = null
    private var reconnectAttempts = 0

    /** One pending reconnect per connect cycle: Failed and Disconnected
     *  arrive back to back, and each state flip would otherwise schedule
     *  a duplicate timer (double-counting the attempt budget). */
    private var reconnectScheduled = false

    /** Set while a pairing is in flight (no token yet). */
    private var pendingPairCode: String? = null
```

```kotlin
    init {
        if (!_config.value.token.isNullOrBlank()) {
            connect()
        }
        // The silent probe: while the shutdown overlay is up and the app is
        // foreground, poke the server every PROBE_SECONDS ...
        scope.launch {
            while (true) {
                delay(PROBE_SECONDS * 1000L)
                if (foreground && _serverDown.value) {
                    Log.i(TAG, "shutdown probe: trying the server again")
                    connect()
                }
            }
        }
    }
```

```kotlin
    private fun openClient(token: String?, pairCode: String?) {
        disconnect()
        reconnectScheduled = false
        val cfg = _config.value
        val c = V2Client(cfg.host, cfg.port, token, pairCode, cfg.name)
        client = c
        observeEvents(c)
        c.connect()
    }

    fun disconnect() {
        eventJob?.cancel()
        client?.disconnect()
        client = null
        pendingPairCode = null
        _connState.value = ConnState.Disconnected
        // The last board stays on screen ...
    }
```

```kotlin
    fun onAppForeground() {
        foreground = true
    }

    fun onAppBackground() {
        foreground = false
        // No attempts in standby while the screen is off: kill a probe
        // that is mid-flight so the socket dies with the screen.
        if (_serverDown.value) {
            client?.disconnect()
        }
    }
```

```kotlin
    // observeEvents -> client.state.collect, after plan 007:
                    when (st) {
                        is ConnState.Connected -> {
                            reconnectAttempts = 0
                            _reconnectAttempt.value = 0
                            _serverDown.value = false
                        }
                        is ConnState.ServerDown -> {
                            Log.i(TAG, "server announced shutdown - retry loop suspended")
                            _serverDown.value = true
                        }
                        is ConnState.Failed -> when {
                            !st.retryable -> pendingPairCode = null
                            !_serverDown.value -> scheduleReconnect()
                        }
                        is ConnState.Disconnected -> if (!_serverDown.value) scheduleReconnect()
                        else -> {}
                    }
```

```kotlin
    // scheduleReconnect, after plan 007:
    private fun scheduleReconnect() {
        val token = _config.value.token
        if (token.isNullOrBlank()) { ... pairing message ...; return }
        if (reconnectScheduled) return
        reconnectScheduled = true
        reconnectAttempts++
        _reconnectAttempt.value = reconnectAttempts
        scope.launch {
            delay(reconnectAttempts.coerceAtMost(6) * 2_000L)
            reconnectScheduled = false
            if (_serverDown.value) return@launch
            val st = _connState.value
            if ((st is ConnState.Failed && st.retryable) || st is ConnState.Disconnected) {
                Log.i(TAG, "reconnect attempt $reconnectAttempts")
                connect()
            }
        }
    }
```

Facts the design relies on (verified at plan time):

- With plan 006 the scope runs on `Dispatchers.Main.immediate`, and
  Compose's lifecycle callbacks run on main. All fields below are
  main-confined, so no locking is needed.
- `observeEvents` launches the state collector immediately under
  `Main.immediate`. After `connect()` returns, `_connState` already holds
  `Connecting`.
- `connect()` is non-blocking (OkHttp `newWebSocket` enqueues).
- `MainActivity` declares `configChanges` for orientation/screen size
  (`AndroidManifest.xml`), so rotation does not fire `ON_STOP`.

## Commands you will need

| Purpose | Command (bash, from repo root) | Expected on success |
|---|---|---|
| Unit tests | `cd apps/mobile && JAVA_HOME="C:/Users/Tymek/.jdks/openjdk-17.0.1" ANDROID_HOME="C:/android-sdk" "C:/Users/Tymek/gradle-8.7/bin/gradle" :app:testDebugUnitTest --console=plain` | `BUILD SUCCESSFUL` |
| One test class | same, plus `--tests 'app.pulpit.mobile.state.LinkPolicyTest'` | `BUILD SUCCESSFUL` |
| Debug APK | same prefix, task `:app:assembleDebug` | `BUILD SUCCESSFUL` |

Elsewhere: any Gradle 8.7 + JDK 17 + Android SDK 34. Pre-existing
warnings (`LocalLifecycleOwner` deprecation, `ProtoFixturesTest`
nullable ClassLoader) are not yours.

## Scope

**In scope**:
- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/LinkPolicy.kt` (create)
- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`
- `apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt`
- `apps/mobile/app/src/test/java/app/pulpit/mobile/LinkPolicyTest.kt` (create)
- `apps/mobile/README.md` (one paragraph)

**Out of scope** (do NOT touch):
- Screen brightness, `WindowManager.LayoutParams.screenBrightness`, or
  any dimming. The owner rejected it.
- `ui/BoardScreen.kt` status texts. The existing "Disconnected -
  retrying" overlay is fine during the 3-minute grace.
- Wake locks, `FLAG_TURN_SCREEN_ON`, foreground services, AlarmManager.
  The deck intentionally does *not* wake itself; it wakes on the
  user's touch or power press.
- Server, desktop, protocol docs. Nothing changes on the wire.
- The OkHttp ping interval in `V2Client`. Once the socket closes in
  background, it no longer matters.

## Git workflow

- Branch: `mobile/link-lifecycle-standby` (or the operator's branch).
- Commits: one for Steps 1-2 (`mobile: link policy helpers`), one for
  Steps 3-5 (`mobile: close the link in background, release the screen
  after 3 min offline`).
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Create the pure policy file

Create `apps/mobile/app/src/main/java/app/pulpit/mobile/state/LinkPolicy.kt`:

```kotlin
//! Deck screen policy (pure, unit-tested in LinkPolicyTest): which screen
//! the Activity shows, and when it may let the display sleep.

package app.pulpit.mobile.state

import app.pulpit.mobile.net.ConnState

/** How long the link may stay down, with the board on screen, before the
 *  deck stops holding the display awake. Long enough to ride out a Wi-Fi
 *  blip or a desktop restart; short enough that a sleeping PC does not
 *  keep the tablet lit all night. */
const val LINK_STANDBY_MS = 3 * 60_000L

/** Whether the board (not the connect or goodbye screen) is on screen. A
 *  snapshot keeps the board up through a transient drop; a refusal
 *  (revoked token, bad code) needs the connect screen and its "Forget
 *  pairing". */
fun showsBoard(conn: ConnState, hasBoards: Boolean, serverDown: Boolean): Boolean {
    if (serverDown) return false
    if (conn is ConnState.Failed && !conn.retryable) return false
    return conn is ConnState.Connected || hasBoards
}

/** FLAG_KEEP_SCREEN_ON is held only for a board that is live or still
 *  inside its reconnect grace; everything else follows the system
 *  screen timeout. */
fun keepsScreenOn(showsBoard: Boolean, linkStandby: Boolean): Boolean =
    showsBoard && !linkStandby
```

**Verify**: Debug APK command → `BUILD SUCCESSFUL`.

### Step 2: Unit-test the policy

Create `apps/mobile/app/src/test/java/app/pulpit/mobile/LinkPolicyTest.kt`,
`package app.pulpit.mobile.state`, JUnit4 like `DeltasTest.kt`. Cases:

1. `liveBoardShows`: `showsBoard(ConnState.Connected("h", 1), hasBoards = false, serverDown = false)` gives true.
2. `snapshotRidesOutADrop`: `Disconnected`, `Connecting("h", 1)`, and
   `Failed("timeout")` (retryable) with `hasBoards = true` all give true.
3. `noSnapshotMeansConnectScreen`: `Disconnected` with `hasBoards = false` gives false.
4. `goodbyeWins`: `serverDown = true` gives false even with `Connected` and boards.
5. `refusalShowsConnectScreen`: `Failed("x", retryable = false)` with boards gives false.
6. `screenHeldOnlyForBoardOutsideStandby`: `keepsScreenOn` truth table,
   4 asserts: (true, false) gives true, and the other three give false.
7. `standbyAfterThreeMinutes`: `assertEquals(180_000L, LINK_STANDBY_MS)`.

**Verify**: One test class command → `BUILD SUCCESSFUL`, 7 tests.

STOP if case 5 does not compile because `ConnState.Failed` has no
`retryable` parameter. That means plan 007 has not landed.

### Step 3: ViewModel: standby timer, cancellable retries, background close

Edit `state/PulpitViewModel.kt`. Keep the file's comment voice: full
sentences on *why*.

3a. **State.** Next to `_serverDown`, add:

```kotlin
    /** True once the link has been down for LINK_STANDBY_MS while the app
     *  was in front: the Activity stops holding the screen on and the
     *  system timeout puts the display to sleep. Cleared by a successful
     *  connect and by the next foreground (the user woke the deck). */
    private val _linkStandby = MutableStateFlow(false)
    val linkStandby: StateFlow<Boolean> = _linkStandby

    private var standbyJob: Job? = null
```

3b. **Replace `reconnectScheduled` with a job.** Delete the
`private var reconnectScheduled = false` field. Keep and adapt its KDoc.
In its place add `private var reconnectJob: Job? = null`. Then:

- In `openClient`, delete the line `reconnectScheduled = false`.
- In `scheduleReconnect`, replace
  `if (reconnectScheduled) return` + `reconnectScheduled = true` with
  `if (reconnectJob?.isActive == true) return`. Assign the launched
  coroutine: `reconnectJob = scope.launch { ... }`. Inside it, after
  the `delay(...)`, replace `reconnectScheduled = false` with
  `reconnectJob = null`. **This line must stay before any `connect()`
  call**, because `connect()` → `openClient` → `disconnect()` cancels
  `reconnectJob`, and it must not cancel the coroutine that is running.
  Change the early return to
  `if (_serverDown.value || !foreground) return@launch`.
- In `disconnect()`, add as the first two lines:
  ```kotlin
          reconnectJob?.cancel()
          reconnectJob = null
  ```

3c. **Standby timer helpers.** Add near `scheduleReconnect`:

```kotlin
    /** Start the standby countdown on the first non-connected state while
     *  in front. Idempotent: reconnect attempts do not restart it, so the
     *  3 minutes count from the moment the link was lost. */
    private fun armStandby() {
        if (!foreground || _linkStandby.value || standbyJob?.isActive == true) return
        standbyJob = scope.launch {
            delay(LINK_STANDBY_MS)
            standbyJob = null
            Log.i(TAG, "link down for ${LINK_STANDBY_MS / 60_000} min - letting the screen sleep")
            _linkStandby.value = true
        }
    }

    private fun disarmStandby() {
        standbyJob?.cancel()
        standbyJob = null
        _linkStandby.value = false
    }
```

3d. **Collector.** In `observeEvents`' `client.state.collect` `when`:

- `is ConnState.Connected ->`: add `disarmStandby()` after
  `_serverDown.value = false`.
- `is ConnState.Failed ->`: run `armStandby()` first, then the existing
  inner `when`. Shape:
  `is ConnState.Failed -> { armStandby(); when { ... } }`.
- `is ConnState.Disconnected ->`: `{ armStandby(); if (!_serverDown.value) scheduleReconnect() }`.
- Replace `else -> {}` with `is ConnState.Connecting -> armStandby()`.
  `ServerDown` keeps its own branch unchanged.

3e. **Lifecycle.** Replace `onAppForeground` and `onAppBackground` with:

```kotlin
    fun onAppForeground() {
        foreground = true
        // The user just woke the deck: hold the screen again and look for
        // the PC right away instead of waiting out a backoff.
        _linkStandby.value = false
        if (!_config.value.token.isNullOrBlank() && reconnectJob?.isActive != true) {
            val st = _connState.value
            val idle = client == null || st is ConnState.Disconnected ||
                (st is ConnState.Failed && st.retryable)
            if (idle) {
                // In shutdown standby this is the silent probe: the
                // goodbye screen stays until a welcome clears it.
                if (!_serverDown.value) {
                    reconnectAttempts = 0
                    _reconnectAttempt.value = 0
                }
                connect()
            }
        }
        if (_connState.value !is ConnState.Connected) armStandby()
    }

    fun onAppBackground() {
        foreground = false
        standbyJob?.cancel()
        standbyJob = null
        // A refusal already closed its socket and its message must stay
        // on the connect screen; a pairing in flight cannot be retried
        // (one-time code). Leave both alone.
        val refused = (_connState.value as? ConnState.Failed)?.retryable == false
        if (refused || pendingPairCode != null) return
        // A dark screen has nothing to show: close the socket so the radio
        // and the CPU can sleep instead of decoding patches nobody sees.
        // Every connect gets a full snapshot, so nothing goes stale;
        // onAppForeground reconnects.
        Log.i(TAG, "app in background - closing the link")
        disconnect()
    }
```

Note on the old `onAppBackground`: it called `client?.disconnect()`
only in shutdown standby. The new version covers that case too,
because `disconnect()` closes the client in every state.

3f. **Update the `foreground` field's KDoc** to: "Tracks the Activity's
STARTED/STOPPED: the socket lives only while the app is in front (see
onAppBackground), and retries, the probe, and the standby countdown run
only then."

**Verify**: `grep -n "reconnectScheduled" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` gives no matches.
`grep -c "armStandby()" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` gives `5`: the definition line (`private fun armStandby() {`) plus 4 call sites (Failed, Disconnected, Connecting, onAppForeground).
Debug APK command → `BUILD SUCCESSFUL`.

### Step 4: MainActivity uses the policy

In `MainActivity.kt`:

1. Delete the unconditional `window.addFlags(...FLAG_KEEP_SCREEN_ON)` in
   `onCreate` and its 3-line comment. The effect below becomes the single
   owner of the flag.
2. Add `val linkStandby by vm.linkStandby.collectAsState()` next to the
   other `collectAsState()` lines.
3. Replace the `DisposableEffect(serverDown) { ... }` block **and** plan
   007's `val refused = ...` line with:

```kotlin
                // This device is the deck: a live board keeps the display
                // awake. The connect and goodbye screens, and a link dead
                // for LINK_STANDBY_MS, let the system timeout apply - the
                // flag comes back with the next healthy board.
                val board = showsBoard(conn, boards.isNotEmpty(), serverDown)
                val keepOn = keepsScreenOn(board, linkStandby)
                DisposableEffect(keepOn) {
                    if (keepOn) {
                        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    } else {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                    onDispose {}
                }
```

4. Make the screen choice use `board`:

```kotlin
                when {
                    serverDown -> ShutdownScreen(onTap = vm::reconnectFromShutdown)
                    // With a snapshot on screen the deck stays up while the
                    // link is down (BoardScreen shows the retrying banner);
                    // the connect screen only owns the no-data and refused
                    // states.
                    board -> BoardScreen(vm)
                    else -> ConnectScreen(vm, onConnected = {})
                }
```

5. Add the imports `app.pulpit.mobile.state.showsBoard` and
   `app.pulpit.mobile.state.keepsScreenOn`. Remove any import that is
   now unused.

**Verify**: `grep -c "FLAG_KEEP_SCREEN_ON" apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt` gives `2` (add + clear inside the effect).
`grep -n "refused" apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt` gives no matches.
Unit tests command and Debug APK command → `BUILD SUCCESSFUL`.

### Step 5: README wording

In `apps/mobile/README.md`, replace the paragraph that starts "The device
is a dedicated deck: the screen is kept on ..." with:

```
The device is a dedicated deck: a live board keeps the screen on and the
app starts itself after a reboot (`BootReceiver`). When the screen goes
off (power button, or the system timeout) the app closes its connection
and reconnects the moment it is back in front. If the PC stays
unreachable for 3 minutes - asleep, crashed, off the network - the app
stops holding the screen on, so the system screen timeout applies;
waking the tablet reconnects. A deliberate server exit (`server.shutdown`)
switches to the goodbye screen right away. Pairing data survives
reinstalls as long as the app is updated with `adb install -r`.
```

**Verify**: `grep -n "3 minutes" apps/mobile/README.md` gives 1 match.

## Test plan

- New: `LinkPolicyTest` (7 tests) covers screen choice and the keep-on
  rule. This is the entire decision surface MainActivity uses.
- Not JVM-testable (no Robolectric): ViewModel timers and lifecycle. A
  reviewer checks the invariants in the diff:
  1. No `connect()` call is reachable when `foreground == false`, except
     the `init` one (which precedes `ON_START` by milliseconds).
  2. `reconnectJob = null` precedes `connect()` inside the retry coroutine.
  3. `disconnect()` cancels `reconnectJob` but **not** `standbyJob`.
     Reconnect attempts go through `openClient` → `disconnect()`, and
     must not reset the 3-minute countdown.
- Operator device checklist (owner, with `adb logcat -s PulpitViewModel V2Client`):
  1. Board live, press power. The log shows `app in background - closing
     the link`, and the desktop log shows the v2 session closed. Press
     power again: the board is live within about 2 s.
  2. Board live, kill the server hard (Task Manager, no goodbye). The
     overlay says "Disconnected - retrying". After about 3 min the log
     shows `letting the screen sleep`, and the display turns off after
     the system timeout. Then no `reconnect attempt` lines appear while
     the screen is off.
  3. Start the server and wake the tablet: it reconnects without a tap.
  4. Quit the desktop from the tray: the goodbye screen shows, and the
     screen sleeps after the system timeout (unchanged behavior).
  5. Check the tablet's system screen timeout is not "never" and that
     developer option "Stay awake" is off. Otherwise step 2 cannot turn
     the display off; that is a device setting, not a bug.

## Done criteria

- [ ] `LinkPolicy.kt` and `LinkPolicyTest.kt` exist; `LinkPolicyTest` runs 7 passing tests
- [ ] `grep -rn "reconnectScheduled" apps/mobile/app/src/main` → no matches
- [ ] `grep -c "FLAG_KEEP_SCREEN_ON" apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt` → `2`
- [ ] `grep -rn "screenBrightness" apps/mobile/app/src` → no matches
- [ ] Unit tests command and Debug APK command → `BUILD SUCCESSFUL`
- [ ] `git status`: only the 5 in-scope files (+ `plans/README.md`) changed
- [ ] `plans/README.md` row for 008 updated

## STOP conditions

- Plan 006 or 007 has not landed (no `SupervisorJob() + Dispatchers.Main.immediate`
  in the ViewModel, or no `retryable` on `ConnState.Failed`).
- The drift check shows changes to in-scope files beyond 006/007.
- You find another caller of `onAppForeground`/`onAppBackground` or of
  `disconnect()` outside `PulpitViewModel.kt` and `MainActivity.kt`
  (`grep -rn "onAppBackground\|onAppForeground\|\.disconnect()" apps/mobile/app/src/main`).
  The new semantics might break it.
- Implementing any step seems to require a wake lock, a service, or a
  brightness change.

## Maintenance notes

- `ROADMAP.md` M4 still lists "foreground service + battery-optimization
  exemption prompt (WS dies in Doze otherwise)". This plan takes the
  opposite route: no socket while the screen is dark, reconnect on wake.
  If someone revives the foreground-service idea, it undoes these
  battery savings. Raise it with the owner first.
- If the desktop ever sends a goodbye on Windows sleep (a
  `WM_POWERBROADCAST` hook), the tablet would go to the goodbye screen
  immediately instead of after the 3-minute grace. That is
  complementary and needs no client change.
- `LINK_STANDBY_MS` is the single knob. If it becomes a user setting, read
  it from prefs in the ViewModel and keep `LinkPolicy` pure.
- A reviewer should look hardest at the ordering in 3b (`reconnectJob =
  null` before `connect()`) and at `onAppBackground`'s early return for
  refused/pairing states.
