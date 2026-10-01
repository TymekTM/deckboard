# Plan 011: Correct AGENTS.md - the Android client speaks v2, not the legacy payload

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report - do not improvise. When done, update the status row for this plan
> in `plans/README.md` - unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f07f447..HEAD -- AGENTS.md apps/mobile/app/src/main/java/app/pulpit/mobile/proto`
> If either changed, compare with the excerpts below; on a mismatch, STOP.

## Status

- **Priority**: P3
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: docs
- **Planned at**: commit `f07f447`, 2026-10-01

## Why this matters

`AGENTS.md` is the first file every coding agent reads in this repo, and
it describes the Android client wrongly:

- It says the client renders tiles from the "legacy payload", and that
  `proto/Models.kt` mirrors `crates/legacy/src/mapping.rs`. Neither is
  true. `apps/mobile/app/src/main/java/app/pulpit/mobile/proto/` contains
  only `V2.kt`, which mirrors the v2 protocol types in `crates/proto`.
  The client connects to `/v2/ws` (`net/V2Client.kt`). Its contract tests
  parse the shared golden fixtures in `crates/proto/tests/fixtures`
  (`app/src/test/java/app/pulpit/mobile/ProtoFixturesTest.kt`).
- It says the native client "parses the legacy shape". So an agent adding
  a tile field is told the field reaches `apps/mobile` through
  `crates/legacy/src/mapping.rs`. In reality it reaches `apps/mobile`
  only through the v2 manifest builder (`crates/v2/src/boards.rs`) and
  `crates/proto`. An agent following the doc edits the wrong mapper,
  sees the desktop work, and ships a field the native tablet never
  receives. That is exactly the failure the section is meant to prevent.
- It does not give the exact Android test command. Gradle is not on
  PATH on the owner's machine, and agents waste a round discovering that.

The rule "carry fields through BOTH wire builders" stays correct: the
legacy mapper still serves the **stock** Deckboard Android client. Only
the attribution of which client reads which builder changes.

**Three-surface note (AGENTS.md):** docs only. No surface behavior changes.

## Current state

`AGENTS.md` (repo root), lines 12-44 at `f07f447`:

```markdown
| Surface | Where it lives | Renders tiles from |
| --- | --- | --- |
| Desktop (editor + touch mode) | `apps/desktop` (Tauri + Vue) | backend JSON via Tauri commands |
| Server (embedded in desktop, headless `apps/server`) | `crates/legacy`, `crates/v2` | DB rows mapped to wire payloads |
| Android client | `apps/mobile` (Kotlin/Compose) | legacy payload; `proto/Models.kt` mirrors `crates/legacy/src/mapping.rs` |
...
- Android: run the Gradle unit tests in `apps/mobile` whenever its code
  moved.
...
## Carry fields through both wire builders

New or changed tile fields must reach BOTH wire builders: the legacy
mapper (`crates/legacy/src/mapping.rs`) and the v2 manifest builder
(`crates/v2/src/boards.rs`). Legacy field names are contractual - the
stock Deckboard Android client renders exactly those fields - and the
native client (`apps/mobile`) parses the legacy shape, so a field that
skips the mapper silently vanishes from every tablet while the desktop
editor still looks fine.
```

Ground truth to check before editing (run these; all are read-only):

- `ls apps/mobile/app/src/main/java/app/pulpit/mobile/proto` lists exactly `V2.kt`.
- `grep -n "v2/ws" apps/mobile/app/src/main/java/app/pulpit/mobile/net/V2Client.kt` gives 1 or more matches.
- `grep -n "crates/proto/tests/fixtures" apps/mobile/app/src/test/java/app/pulpit/mobile/ProtoFixturesTest.kt` gives 1 or more matches.
- `grep -rn "legacy" apps/mobile/app/src/main` gives no protocol use. Comments mentioning the original client are fine.

Style: the file is terse, uses ASCII hyphens (` - `) and backticked
paths, and wraps at about 72 columns. Match it.

## Commands you will need

None beyond `grep`/`ls`. This is a docs-only change and needs no build.

## Scope

**In scope**:
- `AGENTS.md`

**Out of scope** (do NOT touch):
- `README.md`, `docs/protocol-v2.md`, `apps/mobile/README.md`
- The table rows for Desktop and Server, and the first section's
  three-surface rule (still correct)

## Git workflow

- Branch: `docs/agents-mobile-v2` (or the operator's branch).
- One commit: `docs: agents.md - android client speaks v2`.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Fix the table row

Replace the Android row with:

```markdown
| Android client | `apps/mobile` (Kotlin/Compose) | v2 payload; `proto/V2.kt` mirrors `crates/proto`, checked against `crates/proto/tests/fixtures` |
```

### Step 2: Give the exact Android verification command

Replace the two-line Android bullet with:

```markdown
- Android: Gradle unit tests in `apps/mobile` whenever its code moved
  (`gradle :app:testDebugUnitTest` from `apps/mobile`, JDK 17; Gradle is
  not always on PATH - on the owner's machine it lives in
  `C:/Users/Tymek/gradle-8.7/bin`, the JDK in
  `C:/Users/Tymek/.jdks/openjdk-17.0.1`, the SDK in `C:/android-sdk`).
```

### Step 3: Fix the wire-builder section

Replace the paragraph under `## Carry fields through both wire builders`
with:

```markdown
New or changed tile fields must reach BOTH wire builders: the legacy
mapper (`crates/legacy/src/mapping.rs`) and the v2 manifest builder
(`crates/v2/src/boards.rs`). Legacy field names are contractual - the
stock Deckboard Android client renders exactly those fields. The native
client (`apps/mobile`) speaks only v2: a field reaches it through
`crates/v2/src/boards.rs` and the `crates/proto` types, which
`apps/mobile/.../proto/V2.kt` mirrors by hand. A field missing from
either path silently vanishes from that group of tablets while the
desktop editor still looks fine - so add it to `crates/proto`, both
builders, and `V2.kt`, and extend a golden fixture in
`crates/proto/tests/fixtures` so `ProtoFixturesTest` proves the client
parses it.
```

**Verify**:
- `grep -n "Models.kt" AGENTS.md` gives no matches.
- `grep -n "parses the legacy shape" AGENTS.md` gives no matches.
- `grep -c "V2.kt" AGENTS.md` gives `2`.
- `git diff --stat` shows only `AGENTS.md` (+ `plans/README.md`).

## Test plan

Docs only. The ground-truth commands in "Current state" are the test:
every claim the new text makes is backed by one of them.

## Done criteria

- [ ] Verify greps in Step 3 pass
- [ ] `git status`: only `AGENTS.md` (+ `plans/README.md`) changed

## STOP conditions

- `apps/mobile/.../proto/` contains a `Models.kt`, or the client
  connects to a legacy endpoint. Then the doc may be right and this plan
  is stale; report it.
- `crates/proto/tests/fixtures` does not exist.

## Maintenance notes

- If the native client ever adds a legacy fallback, revisit the table
  row.
- The machine-specific Gradle paths in Step 2 belong to the owner's
  machine. If they move, update that bullet rather than deleting it.
