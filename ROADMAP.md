# ROADMAP

Rust rewrite of Deckboard: desktop core (Tauri 2 + Vue 3, M3+) with a
Kotlin/Compose Android client (M4+), keeping the stock Android app working
through a legacy compatibility layer until the new client ships.

## Milestones

- [x] **M0 - Legacy spike** (this commit): Engine.IO v3 / socket.io v2 server
      on port 8500, reading the existing `~/deckboard/database.db`,
      dispatching `exec_shortcut`/`exec_slider` for the system-level command
      subset. Acceptance: stock Deckboard Pro client connects over QR and
      renders boards.
- [ ] **M1 - proto v2**: typed schema (crate `deckboard-proto`) + codegen to
      TypeScript and Kotlin, raw WebSocket on the same port (`/v2/ws`),
      pairing token, `hello/welcome`, `boards.sync` snapshot.
- [ ] **M2 - Action engine complete**: remaining command types (audio
      volume via windows-rs, speaker-device, screenshot, clipboard-based
      unicode typing, media info), audio/device status watchers
      (5 s / 60 s like the original). Status (2026-09-14): master audio
      landed - `speaker-volume` slider sets the endpoint volume, and a
      5 s watcher broadcasts `speaker-volume`/`speaker-muted` as
      `app_status_update` (tablets flip live; the editor mirrors it via
      the customValues store). `screenshot` saves a PNG of the primary
      screen and `type` pastes through the clipboard (unicode-safe),
      both ported from the original. Still open: speaker-device
      switching, media info, device watcher.
- [ ] **M3 - Desktop editor (MVP gate)**: Tauri 2 + Vue 3 editor: boards,
      buttons, sliders CRUD, drag/resize, dual-state, `.boardjson`
      import/export (format-compatible), touch mode, tray, hotkeys,
      autolaunch. **Definition of MVP: full behavioral parity with the
      original desktop app using the stock Android client.**
      Status (2026-09-14): editor MVP lives in `apps/desktop` - board/tile
      CRUD, drag/resize, dual-state styling, format-compatible `.boardjson`
      import/export, touch mode with local execution, tray, autostart
      toggle, configurable touch-mode hotkey (validated, persisted to
      `~/deckboard/editor.json`, re-registered at runtime), slider drag
      interaction in touch mode, visible empty grid slots in the editor,
      live second-state preview (extension/custom-value pushes broadcast
      as `app_status_update` and forwarded to the editor, which mirrors
      the original ToggleButton `isActive` over `customValues`/app state).
- [ ] **M4 - Kotlin/Compose client MVP**: boards/buttons/sliders/toggles,
      live state, offline cache, QR/USB pairing. Includes Android plumbing:
      foreground service + battery-optimization exemption prompt (WS dies
      in Doze otherwise), structured logging.
- [ ] **M5 - Widget kit**: knob/list/graph/interactive templates, custom
      gestures, widget manifest + client capabilities negotiation.
- [ ] **M6 - Web widgets + media** (gated on M0-M5 surviving in daily use):
      board-scoped WebView layer, widget SDK (state/interact/fetch
      proxy/assets), media library with hash-based asset serving, photo/video
      tiles, live edit + hot reload. ffmpeg used when present in PATH, never
      required. Event routing rule: the web layer receives touches only over
      its own regions (z-order resolved before gestures).
- [ ] **M7 - Integrations**: OBS (obws), Streamlabs, Twitch (IRC + Helix),
      Spotify (rspotify), VoiceMod. Twitter is dropped (dead in the original,
      not carried over).
- [ ] **M8 - Extras**: plugin API, mDNS discovery, APK sideload from desktop,
      desktop updater.

## Scope decisions

- **MVP = full parity with the original Deckboard** (desktop behavior +
  stock Android client, Free and Pro). Custom grids / web widgets / media
  come after MVP ships.
- **Twitter removed** everywhere (commands, services, OAuth route).
- **Single port 8500** for both protocols: legacy lives at
  `/socket.io/?EIO=3...`, protocol v2 at `/v2/ws` (plain WebSocket, JSON
  frames, see `deckboard-proto`). **Temporary:** the server binary defaults
  to **8501** while the original desktop app is still in use (it owns 8500
  and the DB); note the stock Android client hardcodes 8500, so real-tablet
  testing means closing the original app and running with
  `DECKBOARD_PORT=8500` (or after the default is flipped back).
- **ffmpeg optional**: transcoding/normalization happens only when a
  `ffmpeg` binary is found in PATH; otherwise assets are served as-is and
  the client decodes natively (H.264/VP9 in WebView/Media3).
- **Web widget foundation without full implementation**: the manifest
  schema (`WidgetKind::Web`, `web_package`, interactions, state refs) is
  defined and versioned in `deckboard-proto` now, so boards authored later
  never need a storage migration. The WebView runtime itself lands in M6.

## Testing

- Unit: payload mapping vs original behavior (fillers, 4x3 crop,
  command transformation, `extra` listener keys, icon/style defaults).
- Integration: both transports driven end-to-end (polling flow with raw
  HTTP, websocket flow incl. `2probe`/`3probe` upgrade and pings).
- Command dispatch is exercised through a mock `Input` seam; the enigo
  backend stays unexercised in CI (it touches the real OS).
