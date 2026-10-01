# Plan 007: Stop reconnecting when the desktop refuses the tablet for good

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt`
> Plan 006 is expected to have changed `PulpitViewModel.kt` (scope line,
> `assetFetches` doc, `forgetPairing`). Those hunks are fine. Any other
> difference from the excerpts below is a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/006-mobile-vm-main-thread.md
- **Category**: bug
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

When the desktop no longer knows the tablet's device token (the device
was removed in the editor, or the server's device store was reset), the
`/v2/ws` upgrade answers **HTTP 401** before any WebSocket frame is
exchanged (`crates/v2/src/service.rs:93-104`). OkHttp reports that
through `onFailure`. The client maps it to a plain, retryable
`ConnState.Failed`, and the ViewModel reconnects every 2-12 s **forever**.
That is a pointless connection attempt every few seconds for as long as
the screen is on, and it drains the battery.

It also locks the user out. If a board snapshot is already on screen,
`MainActivity` keeps showing `BoardScreen`, whose "Disconnected -
retrying" overlay swallows every tap. The connect screen, which has the
"Forget pairing" button, is unreachable without clearing the app's data.

The same retry loop follows the fatal error *frames*
(`FATAL_CODES`: `unauthorized`, `pair-invalid`, `pair-expired`,
`outdated-client`, `too-large`). `V2Client` closes the socket,
`onClosed` publishes `Disconnected`, and the ViewModel schedules a
reconnect. The ViewModel's own `ServerError` handler races that by
setting a message after the reconnect is already queued.

After this plan, a refusal is a terminal state:
`ConnState.Failed(reason, retryable = false)`. Nothing reconnects
automatically, and the app shows the connect screen with a
human-readable reason.

**Three-surface note (AGENTS.md):** client-only. The server already
answers 401 and sends the fatal codes. The desktop and server need no
change, and the wire format is untouched.

## Current state

Files:

- `apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt` holds
  the socket client and the `ConnState` sealed class.
- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`
  holds the reconnect policy.
- `apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt` picks
  the screen.

```kotlin
// V2Client.kt:40-48
sealed class ConnState {
    data object Disconnected : ConnState()
    data class Connecting(val host: String, val port: Int) : ConnState()
    data class Connected(val host: String, val port: Int) : ConnState()
    data class Failed(val reason: String) : ConnState()
    /** The server sent `server.shutdown`: the exit is deliberate, and
     *  reconnecting would be pointless until it comes back. */
    data object ServerDown : ConnState()
}
```

```kotlin
// V2Client.kt:191-209 (inside `private val listener = object : WebSocketListener()`)
        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            Log.i(TAG, "closed: $reason")
            setStateUnlessServerDown(ConnState.Disconnected)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            Log.w(TAG, "failure: ${t.message}")
            setStateUnlessServerDown(ConnState.Failed(t.message ?: "connection failed"))
        }
    }

    /** The terminal `ServerDown` state survives the close or failure that
     *  follows it: the socket going away is the expected aftermath of the
     *  goodbye, not a retryable drop. */
    private fun setStateUnlessServerDown(state: ConnState) {
        if (_state.value !is ConnState.ServerDown) {
            _state.value = state
        }
    }
```

```kotlin
// V2Client.kt:253-260 (handleFrame, error branch)
            V2.TYPE_ERROR -> {
                val error = json.decodeFromJsonElement(ErrorPayload.serializer(), payload)
                Log.w(TAG, "server error: ${error.code} ${error.message.orEmpty()}")
                _events.trySend(V2Event.ServerError(error.code, error.message))
                if (FATAL_CODES.contains(error.code)) {
                    webSocket?.close(1000, error.code)
                }
            }
```

```kotlin
// V2Client.kt:290-291 (companion object)
        /** Fatal errors close the socket; no point retrying with the same auth. */
        val FATAL_CODES = setOf("unauthorized", "pair-invalid", "pair-expired", "outdated-client", "too-large")
```

```kotlin
// PulpitViewModel.kt:281-299 (observeEvents, state collector)
                client.state.collect { st ->
                    _connState.value = st
                    when (st) {
                        is ConnState.Connected -> { ... }
                        is ConnState.ServerDown -> { ... }
                        // While the overlay is up the probe owns reconnects:
                        // failures are expected and stay invisible.
                        is ConnState.Failed, is ConnState.Disconnected ->
                            if (!_serverDown.value) scheduleReconnect()
                        else -> {}
                    }
                }
```

```kotlin
// PulpitViewModel.kt:364-371 (handleEvent)
            is V2Event.ServerError -> {
                Log.w(TAG, "server error: ${ev.code} ${ev.message.orEmpty()}")
                if (ev.code == "pair-invalid" || ev.code == "pair-expired" || ev.code == "unauthorized") {
                    // Bad auth: stop retrying; the connect screen explains.
                    client.disconnect()
                    _connState.value = ConnState.Failed(authMessage(ev.code))
                }
            }
```

```kotlin
// PulpitViewModel.kt:404-415 (scheduleReconnect, the delayed body)
        scope.launch {
            delay(reconnectAttempts.coerceAtMost(6) * 2_000L)
            reconnectScheduled = false
            if (_serverDown.value) return@launch
            val st = _connState.value
            if (st is ConnState.Failed || st is ConnState.Disconnected) {
                Log.i(TAG, "reconnect attempt $reconnectAttempts")
                connect()
            }
        }
```

```kotlin
// PulpitViewModel.kt:418-422
    private fun authMessage(code: String): String = when (code) {
        "pair-invalid" -> "invalid pairing code - generate a new one on the desktop"
        "pair-expired" -> "pairing code expired - generate a new one on the desktop"
        else -> "device revoked on the desktop - pair again"
    }
```

```kotlin
// MainActivity.kt:54-63
                if (serverDown) {
                    ShutdownScreen(onTap = vm::reconnectFromShutdown)
                } else if (conn is app.pulpit.mobile.net.ConnState.Connected || boards.isNotEmpty()) {
                    ...
                    BoardScreen(vm)
                } else {
                    ConnectScreen(vm, onConnected = {})
                }
```

Server side, for reference only (`crates/v2/src/service.rs:93-104`): an
unknown token returns `StatusCode::UNAUTHORIZED` at the upgrade. Pairing
codes are validated *after* `hello` and arrive as `pair-invalid` /
`pair-expired` error frames.

Other `when (conn)` / `ConnState` consumers that must keep compiling,
with no behavior change needed: `ui/BoardScreen.kt:238-246`
(`statusFor`, has an `else` branch) and `ui/ConnectScreen.kt:112-135`
(shows `Failed.reason`).

Test conventions: JUnit4, pure functions, one class per helper group.
Pattern to copy: `apps/mobile/app/src/test/java/app/pulpit/mobile/DisplayHelpersTest.kt`
(file in `app/src/test/java/app/pulpit/mobile/`, declared
`package app.pulpit.mobile.net`, `org.junit.Assert.*` imports).

## Commands you will need

| Purpose | Command (bash, from repo root) | Expected on success |
|---|---|---|
| Unit tests | `cd apps/mobile && JAVA_HOME="C:/Users/Tymek/.jdks/openjdk-17.0.1" ANDROID_HOME="C:/android-sdk" "C:/Users/Tymek/gradle-8.7/bin/gradle" :app:testDebugUnitTest --console=plain` | `BUILD SUCCESSFUL` |
| One test class | same, plus `--tests 'app.pulpit.mobile.net.ConnStateTest'` | `BUILD SUCCESSFUL` |
| Debug APK | same prefix, task `:app:assembleDebug` | `BUILD SUCCESSFUL` |

Elsewhere: any Gradle 8.7 + JDK 17 + Android SDK 34. Two warnings are
pre-existing (`LocalLifecycleOwner` deprecation, `ProtoFixturesTest`
nullable ClassLoader).

## Scope

**In scope**:
- `apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt`
- `apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt`
- `apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt`
- `apps/mobile/app/src/test/java/app/pulpit/mobile/ConnStateTest.kt` (create)

**Out of scope**:
- Server/desktop code. The 401 and error frames are correct as they are.
- Automatically clearing the stored token on 401. A typo'd host that
  points at a *different* Pulpit server would wipe a valid pairing. The
  user clears it with "Forget pairing".
- Background/standby behavior (plan 008) and `ui/*` files.

## Git workflow

- Branch: `mobile/terminal-auth-failures` (or the operator's branch).
- One commit: `mobile: treat auth refusals as terminal instead of retrying`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Add the terminal flag and the pure helpers in V2Client.kt

1. Change `Failed` to carry a retry flag (keep the KDoc style):

   ```kotlin
   /** [retryable] = false: the desktop refused this device or app for
    *  good (unknown token, bad pairing code, outdated client). The same
    *  credentials can never succeed, so nothing reconnects on its own. */
   data class Failed(val reason: String, val retryable: Boolean = true) : ConnState()
   ```

2. Below the `ConnState` class, add three top-level helpers:

   ```kotlin
   /** Terminal states outlive the close/failure callbacks that follow them. */
   fun ConnState.isTerminal(): Boolean =
       this is ConnState.ServerDown || (this is ConnState.Failed && !retryable)

   /** User-facing text for a fatal refusal (an error-frame code, or
    *  "unauthorized" for the upgrade's HTTP 401). */
   fun fatalReason(code: String): String = when (code) {
       "pair-invalid" -> "invalid pairing code - generate a new one on the desktop"
       "pair-expired" -> "pairing code expired - generate a new one on the desktop"
       "unauthorized" -> "device revoked on the desktop - pair again"
       "outdated-client" -> "this app is too old for the desktop - update it"
       else -> "the desktop refused the connection ($code)"
   }

   /** State for a socket failure. The `/v2/ws` upgrade answers 401 for an
    *  unknown or revoked token (crates/v2/src/service.rs), and retrying
    *  the same token cannot work. Everything else is a transient drop. */
   internal fun failureState(httpCode: Int?, message: String?): ConnState.Failed =
       if (httpCode == 401) {
           ConnState.Failed(fatalReason("unauthorized"), retryable = false)
       } else {
           ConnState.Failed(message ?: "connection failed")
       }
   ```

   The first three strings are copied verbatim from the ViewModel's
   `authMessage` so the user-visible text does not change.

**Verify**: Unit tests command → `BUILD SUCCESSFUL` (compiles; behavior unchanged so far).

### Step 2: Use the helpers inside V2Client

1. Rename `setStateUnlessServerDown` to `setStateUnlessTerminal` and make
   it check `!_state.value.isTerminal()`. Update its KDoc: terminal
   states (the goodbye, or a refusal) survive the close/failure that
   follows them.
2. `onClosed` becomes `setStateUnlessTerminal(ConnState.Disconnected)`.
3. `onFailure` becomes:
   ```kotlin
   Log.w(TAG, "failure: ${t.message} (http ${response?.code})")
   setStateUnlessTerminal(failureState(response?.code, t.message))
   ```
4. In `handleFrame`'s `V2.TYPE_ERROR` branch, mark the terminal state
   **before** emitting the event and closing, so the `onClosed` that
   follows the close cannot downgrade it:
   ```kotlin
   V2.TYPE_ERROR -> {
       val error = json.decodeFromJsonElement(ErrorPayload.serializer(), payload)
       Log.w(TAG, "server error: ${error.code} ${error.message.orEmpty()}")
       val fatal = FATAL_CODES.contains(error.code)
       if (fatal) {
           setStateUnlessTerminal(ConnState.Failed(fatalReason(error.code), retryable = false))
       }
       _events.trySend(V2Event.ServerError(error.code, error.message))
       if (fatal) {
           webSocket?.close(1000, error.code)
       }
   }
   ```

**Verify**: `grep -n "setStateUnlessServerDown" apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt` → no matches; `grep -c "setStateUnlessTerminal" apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt` → `4` (definition + onClosed + onFailure + error branch).

### Step 3: ViewModel never retries a terminal failure

In `PulpitViewModel.kt`:

1. In the `client.state.collect` block, replace the combined
   `is ConnState.Failed, is ConnState.Disconnected ->` branch with:
   ```kotlin
   // A refusal is final: stay on the connect screen with the reason.
   is ConnState.Failed -> when {
       !st.retryable -> pendingPairCode = null
       !_serverDown.value -> scheduleReconnect()
   }
   // While the overlay is up the probe owns reconnects:
   // failures are expected and stay invisible.
   is ConnState.Disconnected -> if (!_serverDown.value) scheduleReconnect()
   ```
   Keep the `else -> {}` arm. `Connecting` still lands there.
2. In `handleEvent`, reduce the `is V2Event.ServerError ->` branch to the
   log line plus a comment: `// fatal codes are already terminal in
   V2Client (ConnState.Failed, retryable = false)`. Delete the
   `client.disconnect()` and the `_connState.value = ...` lines.
3. Delete the now-unused `private fun authMessage(...)`.
4. In `scheduleReconnect`'s delayed body, change the guard to
   `if ((st is ConnState.Failed && st.retryable) || st is ConnState.Disconnected) {`.

**Verify**: `grep -n "authMessage" apps/mobile/app/src/main/java/app/pulpit/mobile/state/PulpitViewModel.kt` → no matches; Unit tests command → `BUILD SUCCESSFUL`.

STOP if the compiler reports that `handleEvent`'s `client` parameter is
now unused *and* removing it would touch other files. A warning is fine
and you may leave the parameter.

### Step 4: Route refusals to the connect screen

In `MainActivity.kt`, before the `if (serverDown)` chain add:

```kotlin
// A refusal (revoked token, bad code) needs the connect screen - its
// "Forget pairing" is the way out; the board overlay would eat every tap.
val refused = (conn as? app.pulpit.mobile.net.ConnState.Failed)?.retryable == false
```

and change the middle condition to
`} else if (!refused && (conn is app.pulpit.mobile.net.ConnState.Connected || boards.isNotEmpty())) {`.

**Verify**: Debug APK command → `BUILD SUCCESSFUL`.

### Step 5: Unit tests

Create `apps/mobile/app/src/test/java/app/pulpit/mobile/ConnStateTest.kt`,
`package app.pulpit.mobile.net`, modeled on `DisplayHelpersTest.kt`, with:

1. `failedIsRetryableByDefault`: `ConnState.Failed("x").retryable` is
   true and `isTerminal()` is false.
2. `refusalAndGoodbyeAreTerminal`: `Failed("x", retryable = false)` and
   `ServerDown` are terminal. `Disconnected`, `Connecting("h", 1)`, and
   `Connected("h", 1)` are not.
3. `http401IsTerminalRefusal`: `failureState(401, "boom")` gives
   `retryable == false` and reason `fatalReason("unauthorized")`.
4. `otherFailuresStayRetryable`: `failureState(null, "timeout")` gives
   retryable with reason `"timeout"`. `failureState(500, null)` gives
   retryable with reason `"connection failed"`.
5. `fatalReasonKeepsTheOldWording`: assert the exact strings for
   `pair-invalid`, `pair-expired`, `unauthorized`. Assert that
   `fatalReason("too-large")` contains `"too-large"`.
6. `everyFatalCodeHasAReason`: for each `V2Client.FATAL_CODES` entry,
   `fatalReason(it)` is not blank.

**Verify**: One test class command → `BUILD SUCCESSFUL`, 6 tests run.
Then the full Unit tests command → `BUILD SUCCESSFUL`.

## Test plan

- New: `ConnStateTest` (6 tests above) covers the state classification
  and the 401 mapping. Together these are the whole decision surface.
- The ViewModel wiring (no retry on terminal) is not JVM-testable (no
  Robolectric). Review covers it: the only path to `scheduleReconnect()`
  from a `Failed` must be guarded by `st.retryable`.
- Operator check on a device (optional): pair the tablet, remove the
  device in the desktop editor's Tablets view, restart the tablet app.
  Expected: the connect screen shows "device revoked on the desktop -
  pair again", and `adb logcat -s PulpitViewModel` shows **no**
  `reconnect attempt` lines over the next 60 s.

## Done criteria

- [ ] `grep -rn "setStateUnlessServerDown\|authMessage" apps/mobile/app/src/main` → no matches
- [ ] `grep -n "retryable" apps/mobile/app/src/main/java/app/pulpit/mobile/MainActivity.kt` → 1 match
- [ ] `ConnStateTest` exists with 6 passing tests
- [ ] Unit tests command and Debug APK command → `BUILD SUCCESSFUL`
- [ ] `git status`: only the 4 in-scope files (+ `plans/README.md`) changed
- [ ] `plans/README.md` row for 007 updated

## STOP conditions

- The drift check shows `V2Client.kt` or `MainActivity.kt` changed since
  `f07f447`.
- Some other code constructs `ConnState.Failed` positionally with a
  second argument, or pattern-matches it destructured
  (`grep -rn "ConnState.Failed(" apps/mobile/app/src` shows anything
  beyond the sites in this plan, `PulpitViewModel.connect`/`scheduleReconnect`
  messages, and `V2Client.connect`). Report it instead of guessing the
  retry flag.
- The server turns out to answer something other than 401 for a revoked
  token (`crates/v2/src/service.rs` around line 96 no longer says
  `StatusCode::UNAUTHORIZED`).

## Maintenance notes

- New fatal server codes: add them to `FATAL_CODES` *and* give them a
  `fatalReason` line. The `everyFatalCodeHasAReason` test only proves
  non-blank, so a reviewer should check the wording.
- Plan 008 builds its screen-routing helper on `retryable`. Keep the
  field name.
- If the server ever adds a "token expired, re-auth with refresh" flow,
  that would be a *retryable* refusal. It needs a new state, not
  `retryable = true` on a 401.
