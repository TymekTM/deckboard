# Pulpit

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Turn any tablet into a button board for your PC. Pulpit is a Windows desktop
app with a built-in board editor and touch surface, plus a network server
that phones and tablets connect to: buttons run commands, sliders drag,
dual-state tiles flip on live system state.

It started as a Rust rewrite of Deckboard 3.x and keeps full compatibility
with its ecosystem: the stock Deckboard Android client works out of the box,
`.boardjson` boards import and export unchanged, and original Deckboard
extensions load in a native runtime.

## Features

- Board editor: button/tile CRUD, drag and resize, dual-state styling,
  visible grid, live second-state preview.
- Touch mode: the board becomes the execution surface (configurable hotkey),
  with local execution, slider drag, and state-driven tile flips.
- Command engine: volume, mute and default-audio-device switching via WASAPI
  (no PowerShell dependency), key macros, text typing, screenshots, local
  audio playback, multi-actions, board switching.
- Live state: speaker volume, mute and device pushes; extension and
  custom-value state broadcast to every client.
- Two protocols on one port: the legacy socket.io v2 wire the stock Android
  client speaks, and Pulpit protocol v2 (plain WebSocket, typed schema,
  one-time QR pairing codes, per-device tokens, delta sync).
- Extensions: original Deckboard `.asar` packages run in an embedded JS
  engine; the heaviest ones (system info, callurl, Discord, Voicemeeter)
  have native Rust replacements.
- Tray, close-to-tray, autostart, single instance, daily-rotated logs.
- `.boardjson` import/export, format-compatible with Deckboard.

## Install

Grab the NSIS installer from [Releases](../../releases) (Windows 10/11).
On first launch Pulpit copies an existing `~/deckboard` data directory
(database, settings, paired devices, extensions, assets) into
`~/pulpitApp`, so an upgrade from the original app needs no manual steps.
The original app keeps its files and keeps working.

The stock Deckboard Android app connects out of the box (scan the QR in the
editor's "Connect a tablet" panel). The native Pulpit client for Android
(Kotlin/Compose) is in development.

## Build from source

Prerequisites: a Rust toolchain (stable, MSVC) and Node.js with npm.

```sh
# desktop editor (dev mode, live reload)
cd apps/desktop
npm install
npm run tauri dev

# standalone release binary
npx tauri build --no-bundle   # exe at ../../target/release/pulpit-desktop.exe
```

Tests and lints run from the repo root:

```sh
cargo test --workspace        # device tests (audio, capture, ...) are
                              # #[ignore]d; run them with -- --ignored
cargo clippy --workspace --all-targets
cd apps/desktop && npx vite build
```

## Data and configuration

Everything lives in `~/pulpitApp` (`pulpit_db::data_dir`): `database.db`,
`settings.json`, `editor.json`, `devices.json`, `extensions/`, `assets/`,
`logs/`. Environment overrides:

| Variable              | Default                   | Purpose                                     |
| --------------------- | ------------------------- | ------------------------------------------- |
| `PULPIT_PORT`         | `8500`                    | Server port (legacy + v2 share it)          |
| `PULPIT_DB`           | `~/pulpitApp/database.db` | Database location (profiling/hermetic runs) |
| `PULPIT_EXT_DIR`      | `~/pulpitApp/extensions`  | Extension directory                         |
| `PULPIT_AIDEV_CONFIG` | `~/pulpitApp/aidev.json`  | AI dev-work config (hermetic runs)          |

## Architecture

One Cargo workspace, thin crates with a single job each:

| Crate            | Role                                                                  |
| ---------------- | --------------------------------------------------------------------- |
| `crates/db`      | SQLite access, schema, data dir + legacy migration                    |
| `crates/actions` | Command catalog and dispatch behind a testable `Input` seam           |
| `crates/os`      | Windows integration: WASAPI volume, capture, clipboard, playback      |
| `crates/sysinfo` | Native system-info source (volume/mute/device watcher)                |
| `crates/legacy`  | Engine.IO v3 + socket.io v2 server for the stock Android client       |
| `crates/proto`   | Protocol v2 typed schema + TS bindings + golden JSON fixtures         |
| `crates/v2`      | v2 transport: sessions, pairing, devices, assets, state engine        |
| `crates/backend` | SQLite backend shared by the editor and the headless server           |
| `crates/ext`     | Extension host: original Deckboard extensions on an embedded JS engine|
| `crates/vm`      | Native Voicemeeter integration                                         |
| `crates/discord` | Native Discord local-RPC integration                                   |
| `crates/aidev`   | Native AI dev-work source: agent sessions, plan limits, token burn    |
| `apps/desktop`   | Tauri 2 + Vue 3 editor and touch surface                               |
| `apps/server`    | Headless server binary (legacy + v2)                                  |
| `apps/mobile`    | Kotlin/Compose Android client (in development)                        |

Further reading:

- `docs/protocol-v2.md` - wire protocol, pairing, state sync
- `docs/decisions.md` - architecture decision records
- `ROADMAP.md` - milestones and status

## Compatibility with Deckboard

Pulpit is an independent project, not affiliated with Deckboard. It
interoperates with the Deckboard ecosystem on purpose: the stock Android
client (Free and Pro) connects to the legacy protocol, boards round-trip
through `.boardjson`, extensions load unmodified, and first launch migrates
your data instead of holding it hostage. Naming inside `settings.json`
(such as the `discord-deckboard` package) is preserved for compatibility.

## License

[MIT](LICENSE) - Tymoteusz "TymekTM" Bielski
