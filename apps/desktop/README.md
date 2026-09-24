# Pulpit desktop editor

Tauri 2 + Vue 3 rewrite of the Deckboard 3.x desktop app: board/tile
editor, touch-mode execution surface, and the legacy socket.io v2 server
that keeps the stock Android client working. See `ROADMAP.md` for
milestone status and `docs/decisions.md` for architecture records.

## Run

Development (live reload, editor assets from vite):

```sh
cd apps/desktop
npm install
npm run tauri dev
```

Standalone binary (frontend embedded, no dev server):

```sh
cd apps/desktop
npx tauri build --no-bundle   # exe at ../../target/release/pulpit-desktop.exe
```

Run tests / lints from the repo root:

```sh
cargo test --workspace        # includes real device tests: audio, screen capture
cargo clippy --workspace --all-targets
cd apps/desktop && npx vite build
```

## Environment overrides

| Variable            | Default                    | Purpose                                   |
| ------------------- | -------------------------- | ----------------------------------------- |
| `PULPIT_PORT`    | `8500`                     | Legacy socket.io server port              |
| `PULPIT_DB`      | `~/pulpitApp/database.db`  | Database location (profiling/hermetic runs) |
| `PULPIT_EXT_DIR` | `~/pulpitApp/extensions`   | Extension directory                       |

The release build writes a daily-rotated log to `~/pulpitApp/logs/`
(`pulpit-desktop.log.YYYY-MM-DD`); `RUST_LOG=debug` raises the level.

Pulpit keeps its own copy of the data in `~/pulpitApp` (migrated from
`~/deckboard` on first run, ADR-011), so the original Deckboard app can
stay installed and running. To pin a separate database for evaluation,
point `PULPIT_DB` at a copy and pick a free port:

```sh
PULPIT_DB=/tmp/pulpit-eval.db PULPIT_PORT=8520 pulpit-desktop.exe
```

## Editor notes

- Click an empty grid cell to create a tile at that position; double-click
  (or right-click) a tile to edit it; drag/resize with the pointer.
- The wifi icon in the rail opens server status plus the "Connect a
  tablet" panel: every LAN IPv4 with its QR (the stock client scans the
  bare IP and appends port 8500, same payload the original app encoded).
- Touch mode (play icon in the rail, or the configurable hotkey) turns the
  board into the execution surface: taps run actions, sliders drag, and
  dual-state tiles flip on live state pushes (`app_status_update`) or tap.
- The touch-mode hotkey is validated and persisted to
  `~/pulpitApp/editor.json`; autostart writes the standard HKCU Run entry.
- Extensions load from the extension directory and declare their own
  inputs - the New Button dialog renders those fields (selects, text,
  folders) instead of raw command JSON.
