# Plan 001: Stop re-reading immutable transcript facts on every aidev poll

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c1e0086..HEAD -- crates/aidev`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `c1e0086`, 2026-09-24

## Why this matters

`crates/aidev` polls every 15 s (`poll_secs`, default 15 - a documented,
configurable cadence, out of scope here) and re-derives facts from disk that
never change between polls:

- Claude: for **every** project dir under `~/.claude/projects` (no age
  filter - includes years-old projects), it stats every `.jsonl`, then reads
  and JSON-parses up to 256 KB of the freshest transcript's head - every
  tick, even though the head's `cwd` and opening prompt are immutable.
- Codex: `limits()` (called on **every** 15 s push via `plan_rows`) walks
  the whole `~/.codex/sessions` tree and reads+parses the last 64 KB of
  every file ≤ 8 days old; `sessions()` reads **two separate 256 KB heads**
  of every fresh rollout (one for `session_meta.cwd`, one for the first
  user message) - both immutable facts of an append-only file.
- Antigravity: `Usage::scan` opens **every** conversation `.db` with no age
  filter (the `sessions()` lane in the same file does filter).
- The codex tree is walked 3× per tick (sessions, limits, usage scanner),
  `local_usage::Scanner::scan` does an O(files × live) `Vec::contains`
  inside `retain`, and every live file is `stat()`ed twice per pass.

With ~50 Claude project dirs and ~40 codex rollouts this is tens of MB of
read + DOM-parse churn every 15 s (~70 GB/day) in a 24/7 tray app, for
answers that are unchanged in ~240 of every 240 ticks. This plan adds
mtime-keyed caches and freshness cutoffs so a tick only touches files that
actually changed.

## Current state

Files (all under `crates/aidev/src/`):

- `lib.rs` - `push_loop` (lines 189–217): the poll loop. `Scanner`s for
  zcode/claude/codex, `opencode::Usage`, `antigravity::Usage` are locals
  that live for the thread's lifetime - this is where the new cache state
  must live. `assemble()` at line 221 builds the payload from them.
- `agents.rs` - `claude_sessions` (351–388), `freshest_transcript`
  (391–406), `transcript_head_info` (412–452).
- `codex.rs` - `sessions` (21–54), `session_meta_cwd` (58–73),
  `first_user_message` (78–98), `limits` (134–160), `tail_rate_limits`
  (164–201). `const TAIL_BYTES: u64 = 64 * 1024` at line 18.
- `antigravity.rs` - `Usage::scan` (76–89, no cutoff), `scan_db` (91–111),
  `sessions` (27–55, HAS a cutoff - the pattern to copy).
- `local_usage.rs` - `Scanner::scan` (79–91, the quadratic retain + double
  stat), `file_age_ok` (171–183), `collect_jsonl` (185–200).
- `util.rs` - `mtime(path)` helper used across the crate.

Key excerpts as they exist at `c1e0086`:

```rust
// agents.rs:351  - no age cutoff anywhere in claude_sessions
fn claude_sessions(root: &Path, out: &mut Vec<AgentSession>) {
    ...
    let Some((last_ts, transcript)) = freshest_transcript(&project_dir) else { continue; };
    let (cwd_project, prompt) = transcript_head_info(&transcript);   // 256 KB read+parse
```

```rust
// codex.rs:58 + 78 - two separate 256 KB head reads of the same file per tick
fn session_meta_cwd(path: &Path) -> Option<String> {
    let mut head = vec![0u8; 256 * 1024]; ... }
fn first_user_message(path: &Path) -> Option<String> {
    let mut head = vec![0u8; 256 * 1024]; ... }
```

```rust
// antigravity.rs:76 - no mtime cutoff, unlike sessions() at line 40
pub fn scan(&mut self, conversations_dir: &Path, now: i64) {
    for entry in entries.flatten() {
        ...
        self.scan_db(&path, now);   // opens the DB regardless of age
```

```rust
// local_usage.rs:83 - quadratic retain, double stat
let live: Vec<PathBuf> = files.into_iter().filter(|p| file_age_ok(p, cutoff)).collect();
self.files.retain(|path, _| live.contains(path));
```

Repo conventions to match:

- Error handling: silent `let Ok(..) = .. else { return }` degradation
  everywhere in this crate - a missing/unreadable source is "tool not
  installed", never an error. Match that; no new error types.
- Comment style: `//` full-sentence comments explaining the *why* (see the
  torn-line comment at `local_usage.rs:124`).
- Tests live in the same file under `#[cfg(test)] mod tests` - see
  `agents.rs:678-691` (the SDK-wrapper head fixture) and
  `local_usage.rs` tests for the pattern. The live smoke test is
  `cargo test -p deckboard-aidev -- --ignored live --nocapture`.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Crate tests | `cargo test -p deckboard-aidev` | all pass |
| Lint | `cargo clippy -p deckboard-aidev --all-targets` | exit 0, no warnings |
| Live smoke (optional, real dirs) | `cargo test -p deckboard-aidev -- --ignored live --nocapture` | passes, output unchanged in shape |

## Scope

**In scope** (the only files you should modify):
- `crates/aidev/src/lib.rs`
- `crates/aidev/src/agents.rs`
- `crates/aidev/src/codex.rs`
- `crates/aidev/src/antigravity.rs`
- `crates/aidev/src/local_usage.rs`
- `crates/aidev/src/util.rs` (only if a shared cache type belongs there)

**Out of scope** (do NOT touch):
- `apps/desktop/src-tauri/src/lib.rs` and `apps/server/src/main.rs` - the
  spawn wiring is plan 004's territory.
- Poll cadences (`poll_secs`, `http_poll_secs`) and the config schema -
  documented behavior.
- `crates/aidev/src/local_usage.rs::ingest` read strategy (offset cursors,
  torn-line handling) - the restart re-read burst is a separate deferred
  finding; do not restructure `ingest` here.
- `opencode.rs` - its usage lane is already incremental and bounded.

## Git workflow

- Branch: `perf/aidev-mtime-caches`
- Commit per step; message style: lowercase imperative summary line, e.g.
  `aidev: cache codex rollout heads by mtime` (matches `git log --oneline`
  style: "legacy: detach long execs from the packet loop").
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: A shared mtime-keyed head cache, used by the codex lanes

Add to `codex.rs`:

```rust
/// Immutable head facts of one rollout, keyed by (mtime, size). Rollouts
/// are append-only, so a head can only change when the file is rewritten;
/// (mtime, len) is the rewrite signal already trusted by the truncation
/// reset in local_usage::ingest.
struct HeadCache {
    entries: HashMap<PathBuf, (u64, u64, Option<String>, Option<String>)>,
}
// key: path -> (len, mtime_secs, cwd, first_user_message)
```

Rework `session_meta_cwd` + `first_user_message` into one
`fn head_facts(path: &Path, cache: &mut HeadCache) -> (Option<String>, Option<String>)`
that:

1. stats the file; if `(len, mtime)` matches the cache entry, returns it;
2. otherwise does ONE `vec![0u8; 256 * 1024]` head read and extracts both
   fields from the same buffer (keep the exact parsing logic of the two
   current functions - first line `session_meta` → `/payload/cwd`, then
   first `event_msg`/`user_message` → `/payload/message`);
3. stores and returns the result (including `None`s, so bad heads are not
   re-parsed either).

Thread the cache through: `codex::sessions` currently is a free function
called from `agents::snapshot`-style code with `(dir, config, now, out)`.
Change it to take `&mut HeadCache` (same call-site signature change in
`lib.rs::assemble` / wherever `sessions` is invoked - find callers with
`grep -rn "codex::sessions" crates/aidev/src`). The cache lives as a field
next to the `Scanner`s in `push_loop` (`lib.rs:191-196`) and is passed down
through `assemble`.

**Verify**: `cargo test -p deckboard-aidev` → all pass (existing codex
fixture tests must still pass - they exercise the same parsing through the
new entry point).

### Step 2: Apply the same cache to `codex::limits` tails, and stop re-tailing unchanged files

In `codex.rs`, add a parallel `LimitsCache`:
`HashMap<PathBuf, (u64 /*len*/, u64 /*mtime*/, Option<(i64, Limits)>)>`.

In `limits()` (line 134): the walk already stats each file for the mtime
cutoff - capture `len` too from the same `metadata()` call. If
`(len, mtime)` matches the cached entry, reuse the parsed
`Option<(i64, Limits)>`; otherwise call `tail_rate_limits` and store the
result. Keep the "newest wins" fold exactly as is.

`limits()` is called from `limits.rs:95` inside `plan_rows`, which
`assemble` calls every push. The cache must therefore also live in
`push_loop` state and be passed through `assemble` → `plan_rows` (add a
parameter; `plan_rows` is `pub` - check for other callers with
`grep -rn "plan_rows" crates apps`; adjust all of them, they are in-crate).

**Verify**: `cargo test -p deckboard-aidev` → all pass.

### Step 3: Claude lanes - freshness cutoff for project dirs, cached head facts

In `agents.rs`:

1. `claude_sessions` (line 351): after `freshest_transcript` returns
   `(last_ts, transcript)`, skip the dir when
   `last_ts < now - config.agent_done_secs - util::DONE_GRACE_SECS`
   - the exact cutoff `codex::sessions` (codex.rs:22) and
   `antigravity::sessions` (antigravity.rs:28) already use. This requires
   passing `config`/`now` into `claude_sessions` (check its callers with
   `grep -rn "claude_sessions" crates/aidev/src` and update them).
   Note: the state classification (`working`/`attention`/`done`) happens
   later from `last_ts`; rows older than the cutoff can never render, so
   skipping them cannot change any tile.
2. `transcript_head_info`: route it through the Step-1-style head cache
   (either reuse `HeadCache` by making it crate-visible in `codex.rs`, or
   duplicate a small cache struct in `agents.rs` - prefer moving `HeadCache`
   to `util.rs` and sharing it). Claude heads are keyed the same way:
   `(len, mtime) -> (cwd, prompt)`. The cache instance lives in `push_loop`
   state next to Step 1's.

**Verify**: `cargo test -p deckboard-aidev` → all pass, including the
SDK-wrapper head fixture test at `agents.rs:678-691`.

### Step 4: Antigravity - mtime cutoff + watermark pruning

In `antigravity.rs::Usage::scan` (line 76):

1. Before `scan_db`, `mtime(&path)` and skip when older than
   `now - self.history_secs` (the history window is the right cutoff here
   because usage samples, not session rows, are collected; `sessions()`
   keeps its own tighter cutoff). Copy the null-handling style of
   `sessions()` at lines 37–42.
2. Prune `self.watermark` entries for paths not seen in the current scan:
   collect seen paths into a `HashSet<&Path>` during the loop, then
   `self.watermark.retain(|p, _| seen.contains(p))` - a deleted
   conversation must not pin its cursor forever.

**Verify**: `cargo test -p deckboard-aidev` → all pass.

### Step 5: `local_usage::Scanner::scan` - de-quadratic retain, single stat

In `local_usage.rs` (lines 79–91):

1. Change `file_age_ok` to return the metadata it already fetched:
   `fn file_meta_ok(path: &Path, cutoff: i64) -> Option<(u64, i64)>`
   returning `(len, mtime_secs)` (same false → skip semantics).
2. Build `let live: HashSet<PathBuf> = ...` from the filter, use it for
   `self.files.retain(|p, _| live.contains(p))`.
3. Pass the `(len, mtime)` you already have into `ingest` (change its
   signature to accept them) so `ingest`'s own
   `std::fs::metadata` call (line 94) is dropped.

**Verify**: `cargo test -p deckboard-aidev` → all pass
(`local_usage` has tests covering offsets/truncation - they must stay green
unchanged).

### Step 6: Fold the three codex-tree walks into one

`collect_jsonl(sessions_dir, 0, &mut files)` currently runs in
`codex::sessions` (line 24), `codex::limits` (line 137) and
`local_usage::Scanner::scan` via `push_loop` (lib.rs:203). Do **one** walk
per tick in `push_loop` and pass `&[PathBuf]` into all three consumers:

- `push_loop`: `let mut codex_files = Vec::new(); collect_jsonl(...)` once,
  then hand slices to `codex.scan_files(...)`, the limits lane, and
  `sessions`.
- Rename/adjust the three entry points to accept the pre-collected list
  (keep their cutoff filtering internal).
- `local_usage::Scanner::scan` is also used for zcode and claude roots;
  give it the same `scan_files(&mut self, format, files: &[PathBuf])`
  signature and do the `collect_jsonl` at the `push_loop` call site for all
  three roots - one walk each, as before, but no duplicate walks for codex.

**Verify**: `cargo test -p deckboard-aidev && cargo clippy -p deckboard-aidev --all-targets`
→ tests pass, clippy clean.

## Test plan

New tests (in-file, matching the crate's `#[cfg(test)] mod tests` style):

- `codex.rs`: a test that builds a temp rollout file, calls the new
  `head_facts` twice, and asserts the second call does NOT re-read the file
  - e.g. delete the file between calls and assert the cached values still
  come back (the cache must be consulted before `File::open`). Same shape
  for the limits tail cache.
- `agents.rs`: `claude_sessions` with a fixture dir whose freshest
  transcript mtime is older than `agent_done_secs` produces no rows; a
  fresh one still does.
- `antigravity.rs`: after `scan`, `watermark` contains no entry for a DB
  removed from the directory.
- `local_usage.rs`: existing tests green; add one asserting a file whose
  metadata is fetched once per scan still ingests correctly (behavioral -
  no double-stat observable, so assert on outcomes only).

Pattern to model after: the temp-dir fixtures in `local_usage.rs` tests and
`agents.rs:678-691`.

Verification: `cargo test -p deckboard-aidev` → all pass, including the new
tests.

## Done criteria

- [ ] `cargo test -p deckboard-aidev` exits 0, new cache/cutoff tests included
- [ ] `cargo clippy -p deckboard-aidev --all-targets` exits 0 with no warnings
- [ ] `grep -n "256 \* 1024" crates/aidev/src/codex.rs` shows at most ONE
      allocation site in the head path (the cache miss branch), zero in the
      hit branch
- [ ] `grep -n "live.contains(path)" crates/aidev/src/local_usage.rs`
      operates on a `HashSet`, not a `Vec`
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The excerpts above do not match the live code (drift).
- `plan_rows` or `codex::sessions` turn out to have callers outside
  `crates/aidev` - the signature threading would leave the crate, which
  this plan does not authorize.
- A cache test cannot be made deterministic after two attempts.
- You find the claude/codex lanes already cache something by mtime (the
  plan's premise is then stale).

## Maintenance notes

- The caches grow with the number of distinct files seen; entries for files
  that left the history window are only pruned via the `Scanner` retain.
  If unbounded growth of the cache maps ever shows up in profiling, add the
  same retain-by-live-set to `HeadCache`/`LimitsCache`.
- The deferred follow-up "persist scanner offsets to survive restarts"
  (restart re-read burst) will want a serializable state file; the caches
  added here should be excluded from that serialization (they rebuild from
  one walk).
- Reviewer: the semantic guarantee to check is that a tile's rendered rows
  are identical before/after - run the live smoke test and diff two ticks
  of output if in doubt.
