# AGENTS.md

Notes for coding agents working in this repo. Architecture, build setup
and protocol details live in `README.md` and `docs/protocol-v2.md`; this
file covers only what those documents cannot tell you.

## One change, three surfaces

Pulpit is three apps that must behave identically, connected by the
payloads the server sends. The same tile behavior exists in all three at
once:

| Surface | Where it lives | Renders tiles from |
| --- | --- | --- |
| Desktop (editor + touch mode) | `apps/desktop` (Tauri + Vue) | backend JSON via Tauri commands |
| Server (embedded in desktop, headless `apps/server`) | `crates/legacy`, `crates/v2` | DB rows mapped to wire payloads |
| Android client | `apps/mobile` (Kotlin/Compose) | v2 payload; `proto/V2.kt` mirrors `crates/proto`, checked against `crates/proto/tests/fixtures` |

Tile behavior and styling - press modes (`button` / `toggle` / `slider`),
dual states, live-state fields - must be implemented in every surface
that consumes them, and each surface needs its own verification before
the work counts as done:

- Desktop: `npx vite build` in `apps/desktop`, plus exercising the editor
  or touch mode when the change is visible in the UI.
- Rust crates (backend, mappers, server): `cargo test --workspace` and
  `cargo clippy --workspace --all-targets` from the repo root.
- Android: Gradle unit tests in `apps/mobile` whenever its code moved
  (`gradle :app:testDebugUnitTest` from `apps/mobile`, JDK 17; Gradle is
  not always on PATH - on the owner's machine it lives in
  `C:/Users/Tymek/gradle-8.7/bin`, the JDK in
  `C:/Users/Tymek/.jdks/openjdk-17.0.1`, the SDK in `C:/android-sdk`).

Passing one surface is not completion. A change that edits only the layer
where the bug was found is suspect by default: check what the other two
surfaces do with the same field before assuming they need nothing.

## Building the desktop app

A distributable `pulpit-desktop.exe` must come from `npx tauri build`
(--no-bundle for the raw exe) run in `apps/desktop`. A plain
`cargo build --release -p pulpit-desktop` compiles fine but bakes the
dev frontend URL (`http://localhost:5173`) as the active page - the
window opens with `ERR_CONNECTION_REFUSED` because nothing serves the
dev server. The string is present in every binary either way, so greping
the exe for it proves nothing; verify by running it. Side-by-side
profiling runs opt out of the single-instance guard with
`PULPIT_NO_SINGLE_INSTANCE=1` (plus `PULPIT_PORT` and `PULPIT_DB`), and
`PULPIT_NO_DISCOVERY=1` so the second instance does not announce a
duplicate "Pulpit on <host>" over mDNS. The desktop still reads
`editor.json`, `settings.json` and `devices.json` from `~/pulpitApp` and
appends to its `logs/` - there is no data-dir override.

## Carry fields through both wire builders

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

## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
