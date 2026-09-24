# Plan 005: Bound the v2 outbound queue and reap silent v2 sessions

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c1e0086..HEAD -- crates/v2 apps/desktop/src-tauri/src/lib.rs apps/server/src/main.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `c1e0086`, 2026-09-24

## Why this matters

Protocol v2 sessions have no liveness enforcement and no backpressure:

- The outbound channel is `mpsc::unbounded_channel`
  (`crates/v2/src/session.rs:32`); `V2Hub::broadcast_frame`
  (`crates/v2/src/hub.rs:59-66`) pushes into it for every session.
- The session's pump task (`session.rs:39-72`) `select!`s between the
  queue and a ping interval, and blocks on `sink.send(...).await` with no
  timeout. If a peer stalls (Android tablet sleeping through a Wi-Fi
  drop without a TCP FIN - the normal failure mode), the pump parks on
  the send forever, pings stop firing (same select loop), the socket
  never errors, and the session is never removed.
- `V2Session::silent_for_ms()` (`hub.rs:105`) exists precisely for a
  watchdog but has **zero callers** (verified by grep at plan time). The
  30 s reaper loops in both hosts (`apps/desktop/src-tauri/src/lib.rs:461-468`,
  `apps/server/src/main.rs:304-312`) only reap the LEGACY hub
  (`reap(75)`); the module doc at `session.rs:1-3` claims a
  "ping/watchdog keepalive" that was never wired.
- Result: every broadcast (`state.patch` driven by sysinfo every 10 s,
  boards deltas, pings) clones into a queue nobody drains - unbounded RAM
  growth (~MBs/day per dead connection), a leaked session entry and a
  parked pump task per corpse, for the process lifetime. A live-but-
  backgrounded client that keeps sending pongs while not reading grows
  the same way, so silence-detection alone is not enough - the queue
  bound is the actual fix.

The legacy layer already has both halves (reap(75) matching
pingInterval+pingTimeout, and send-failure cleanup); this plan gives v2
the same treatment with bounded queues.

## Current state

Files:

- `crates/v2/src/hub.rs` - `V2Hub` (15-67): `create` (29-37) takes the
  `UnboundedSender`, `broadcast_frame` (59-66) clones a serialized String
  per session, `remove` (47-51) aborts holds. `silent_for_ms` (105-107)
  with no callers. `V2Session` (69-136) with `last_seen: AtomicU64`,
  `touch()` (99-102).
- `crates/v2/src/session.rs` - `run` (30-118ish): pump spawn (39-72),
  `run_session` reads inbound frames and calls `session.touch()` (grep
  `touch()` in the file to confirm every inbound frame refreshes
  `last_seen`).
- `crates/v2/src/service.rs` - `V2State` fields (grep `pub struct V2State`),
  `ws_connect`/connection accept path (grep `hub.attach` for where a
  session joins the fan-out and where `remove` is called on disconnect).
- `crates/v2/tests/integration.rs` - the WS integration harness used by
  existing connect/pair/sync tests (see the `publish_delta` test at line
  720 for the harness pattern: spawn a V2State, connect a client, assert
  frames).
- `apps/desktop/src-tauri/src/lib.rs:461-468` and
  `apps/server/src/main.rs:304-312` - the 30 s legacy reaper loops to
  extend.
- `docs/protocol-v2.md` - §1/§3/§6 define ping/watchdog timing
  (WS-level pings 60 s + watchdog). Read these sections first; the
  numbers below must stay consistent with the doc.

Excerpts at `c1e0086`:

```rust
// crates/v2/src/session.rs:32
let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsOut>();
// :46-68 (shape)
tokio::select! {
    msg = out_rx.recv() => match msg {
        Some(WsOut::Text(text)) => {
            if sink.send(Message::Text(text)).await.is_err() { break; }
        }
        ...
    }
    _ = ping.tick() => {
        if sink.send(Message::Ping(Vec::new())).await.is_err() { break; }
    }
}
```

```rust
// crates/v2/src/hub.rs:63-66
for session in self.sessions.lock().expect("v2 hub poisoned").values() {
    let _ = session.out.send(super::session::WsOut::Text(text.clone()));
}
```

Conventions:

- Lock discipline: hub/session mutexes use `.expect("v2 hub poisoned")` /
  `.expect("session poisoned")` messages - match them.
- Error philosophy: send failures mean "dying connection; its own loop
  notices and cleans up" (hub.rs:57-58 comment) - preserve that.
- Tests: in-file `#[cfg(test)]` units (see hub.rs:138-175 for a hub test
  that uses real channels, no mock framework) + `tests/integration.rs`
  for full-WS flows.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Crate tests | `cargo test -p deckboard-v2` | all pass |
| Workspace tests | `cargo test --workspace` | all pass |
| Lint | `cargo clippy --workspace --all-targets` | exit 0, no warnings |

## Scope

**In scope**:
- `crates/v2/src/hub.rs`, `crates/v2/src/session.rs`, `crates/v2/src/service.rs`
- `crates/v2/tests/integration.rs` (new tests)
- `apps/desktop/src-tauri/src/lib.rs` (reaper extension only)
- `apps/server/src/main.rs` (reaper extension only)
- `docs/protocol-v2.md` (only if the watchdog timing needs wording synced)

**Out of scope**:
- The legacy hub's reap semantics (`reap(75)`) - frozen, tested.
- Frame/payload formats, pairing, devices store.
- The flusher cadence (separate deferred finding).

## Git workflow

- Branch: `perf/v2-outbound-bounds`
- Commit per step; message style: `v2: bound outbound queues and reap
  silent sessions` (area prefix, lowercase imperative).
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Bound the outbound channel, close on overflow

1. In `session.rs`, replace `unbounded_channel` with
   `mpsc::channel::<WsOut>(QUEUE_CAP)` where
   `const QUEUE_CAP: usize = 256;` (a generous bound: at current push
   rates a healthy client drains this in well under a second; 256 frames
   ≈ hundreds of KB at worst).
2. Sending now returns `TrySendError`: in `V2Hub::broadcast_frame` and
   `V2Session::send_frame`, on `Err(TrySendError::Full(_))` or
   `Disconnected`, mark the session dead. Since removal must not happen
   under the sessions mutex while iterating it (current code holds the
   lock across the loop), restructure:
   ```rust
   pub fn broadcast_frame(&self, frame: &Frame) {
       let Ok(text) = serde_json::to_string(frame) else { return; };
       let mut dead = Vec::new();
       {
           let sessions = self.sessions.lock().expect("v2 hub poisoned");
           for session in sessions.values() {
               if session.try_send(WsOut::Text(text.clone())).is_err() {
                   dead.push(session.id);
               }
           }
       }
       for id in dead { self.remove(id); }
   }
   ```
   Add `V2Session::try_send(&self, msg) -> Result<(), mpsc::error::TrySendError<WsOut>>`
   wrapping `self.out.try_send(msg)`. `send_frame` returns `false` on
   error as today - find its callers (grep `send_frame(`) and make sure
   a `false` eventually leads to session teardown or is at least
   non-fatal for the sender (it already is per current usage).
3. Removing from the hub aborts holds but NOT the pump task - add a
   `close()` path: when the hub removes a session, also send
   `WsOut::Close` via `try_send` (best effort; if the queue is full the
   socket is wedged anyway) so the pump flushes-what-it-can and exits,
   dropping the socket. The pump's `sink.send` on a wedged peer may
   still hang - the watchdog in Step 2 is the backstop that aborts it.

**Verify**: `cargo test -p deckboard-v2` → all pass (the hub unit test
at hub.rs:144 uses `unbounded_channel` in tests - update it to the
bounded channel and keep its assertions).

### Step 2: Wire the silence watchdog

1. In both hosts' reaper loops (desktop lib.rs:461-468, server
   main.rs:304-312), extend the 30 s tick to also reap v2:
   ```rust
   v2_hub.reap_silent(ping_interval + watchdog_grace).await;
   ```
   where `ping_interval` comes from `V2Config` (grep `ping_interval` -
   default 60 s per protocol-v2.md) and `watchdog_grace` is
   `2 * ping_interval` (a client answering pings at 60 s must not be
   culled at 61 s). If `V2State`/hub handles are not in scope at the
   reaper spawn site, clone them in (the desktop has `v2: Option<Arc<V2State>>`
   in `setup_core`; pass `v2.map(|s| s.hub.clone())` into the spawned
   task - reaping is a no-op skip when `None`).
2. Implement `V2Hub::reap_silent(&self, max_silent_ms: u64)`:
   collect ids with `silent_for_ms() > max_silent_ms` under the lock,
   then `remove(id)` each outside it (mirroring broadcast's dead-list
   pattern). `remove` should additionally abort the pump task: store the
   pump `JoinHandle` on the session (add `pump: Mutex<Option<JoinHandle<()>>>`
   set by `run` right after spawning; `remove` takes and aborts it, and
   sends `WsOut::Close` first as in Step 1).
3. Update the `session.rs:1-3` module doc to state the actual mechanism
   (bounded queue + silence watchdog numbers).

**Verify**: `cargo test -p deckboard-v2` → all pass.

### Step 3: Tests

1. Hub unit test (in-file, modeled on hub.rs:144-175): create a session
   with a bounded receiver that is NEVER awaited; `broadcast_frame` in a
   loop until `hub.count()` drops to 0 (overflow closes it); assert
   removal happened within `QUEUE_CAP + small` iterations.
2. Hub unit test: `reap_silent` removes a session whose `last_seen` is
   older than the bound (construct, manually rewind - either expose a
   test-only `last_seen` setter behind `#[cfg(test)]` or store a
   smaller `unix_millis` value by faking; prefer the `#[cfg(test)]`
   setter on V2Session) and keeps a fresh one.
3. Integration test (`tests/integration.rs`, modeled on the line-720
   harness): a client that completes hello (so it attaches), then stops
   reading AND stops responding - feed it N broadcasts exceeding the
   queue cap and assert the server eventually closes the socket /
   `hub.count()` returns 0. If driving a non-reading WS client in the
   integration harness is impractical, the two unit tests above are the
   acceptance bar - say so in the commit message rather than weakening
   the assertions.

**Verify**: `cargo test -p deckboard-v2` → all pass including the new
tests; `cargo test --workspace` → all pass;
`cargo clippy --workspace --all-targets` → exit 0.

## Test plan

Covered in Step 3: overflow-closes-session, silence-reap removes stale /
keeps fresh, reconnect-after-reap works (an existing integration test
covers reconnect via snapshot - confirm it still passes; it is the proof
that removal does not corrupt the hub).

## Done criteria

- [ ] `grep -n "unbounded_channel" crates/v2/src` returns no matches in
      the outbound path
- [ ] `grep -rn "silent_for_ms" crates/v2 apps` shows production callers
      (the reaper), not just the definition
- [ ] `cargo test --workspace` exits 0, new queue/watchdog tests included
- [ ] `cargo clippy --workspace --all-targets` exits 0, no warnings
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above do not match the live code (drift).
- `V2Session::touch()` is not called on every inbound frame (then
  `silent_for_ms` semantics differ from this plan's assumption - report
  where touch is called).
- Bounding the queue breaks an existing integration test in a way that
  points at a real ordering dependency (frames must not be DROPPED for
  healthy clients - only overflow, i.e. a wedged peer, may lose them).
- The pump handle storage conflicts with how `run` is currently
  structured (e.g. the session is moved before the pump spawns).

## Maintenance notes

- QUEUE_CAP=256 and the watchdog grace (2 × ping interval) are the two
  knobs; if tablets on poor Wi-Fi ever get culled mid-session, raise
  grace first (it is the false-positive knob), queue cap second.
- `docs/protocol-v2.md` §6 promises a watchdog; if its wording differs
  from the implemented numbers, update the doc in the same commit.
- Reviewer: the invariant to check is "no frame loss for clients that
  keep draining" - the integration suite's snapshot-on-reconnect tests
  are the proof.
