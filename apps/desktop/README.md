# Deckboard desktop editor

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
npx tauri build --no-bundle   # exe at ../../target/release/deckboard-desktop.exe
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
| `DECKBOARD_PORT`    | `8500`                     | Legacy socket.io server port              |
| `DECKBOARD_DB`      | `~/deckboard/database.db`  | Database location (profiling/hermetic runs) |
| `DECKBOARD_EXT_DIR` | `~/deckboard/extensions`   | Extension directory                       |

The release build writes a daily-rotated log to `~/deckboard/logs/`
(`deckboard-desktop.log.YYYY-MM-DD`); `RUST_LOG=debug` raises the level.

The original Deckboard app must be closed while this one runs on the real
database (ADR-001: single writer). To evaluate side by side with the
original, point `DECKBOARD_DB` at a copy and pick a free port:

```sh
DECKBOARD_DB=/tmp/deckboard-eval.db DECKBOARD_PORT=8520 deckboard-desktop.exe
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
  `~/deckboard/editor.json`; autostart writes the standard HKCU Run entry.
- Extensions load from the extension directory and declare their own
  inputs - the New Button dialog renders those fields (selects, text,
  folders) instead of raw command JSON.
