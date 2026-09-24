# ROADMAP

Pulpit: a Rust desktop core (Tauri 2 + Vue 3, M3+) with a Kotlin/Compose
Android client (M4+), keeping the stock Deckboard Android app working
through a legacy compatibility layer until the new client ships. Started
as a Rust rewrite of Deckboard 3.x; owns its identity and data directory
(`~/pulpitApp`) since the 2026-09-24 rebrand (ADR-011).

## Milestones

- [x] **M0 - Legacy spike** (this commit): Engine.IO v3 / socket.io v2 server
      on port 8500, reading the existing `~/deckboard/database.db`,
      dispatching `exec_shortcut`/`exec_slider` for the system-level command
      subset. Acceptance: stock Deckboard Pro client connects over QR and
      renders boards.
- [x] **M1 - proto v2**: typed schema in `pulpit-proto` (envelope, widget
      manifests, all messages) with ts-rs TypeScript bindings
      (`crates/proto/bindings/`) and golden JSON fixtures parsed by both the
      Rust tests and a Kotlin unit test. Transport in `pulpit-v2`:
      raw WebSocket `/v2/ws` on the shared port, one-time pairing codes
      (`POST /v2/pair`) + per-device tokens (`~/deckboard/devices.json`),
      `hello`/`welcome` with the live channel catalog, `boards.sync` +
      `boards.delta`, `state.sync`/`state.patch` (server-side series ring
      buffers, 100 ms coalescing), `interaction` with press-start/press-end
      and server-side hold-to-repeat, `board.open`, hashed assets on
      `/assets/<sha256>`, WS-level pings (60 s) + watchdog. Spec:
      `docs/protocol-v2.md`. The Kotlin client migrates to v2 in M4.
- [x] **M2 - Action engine complete** (`crates/os`): master volume/mute
      and default-device switching via WASAPI + IPolicyConfig (no
      PowerShell dependency, unlike the original), `speaker-device` /
      `speaker-volume` commands live, screenshot as
      `Pulpit_<UTC stamp>.png` (same-second captures get a `_N`
      suffix instead of overwriting), `type` via clipboard paste with
      restore (unicode-safe). Status watcher pushes volume + mute every
      5 s and the device id every 30 s (the original's real cadence)
      over legacy `app_status_update` and the v2 state engine. Media
      info was dropped: the original app has no such command (zero
      references in its bundle). Merge notes (2026-09-21): the desktop
      editor's watcher pushes the device on every detected change
      (5 s poll) instead of a fixed 30 s cadence; `play` (local audio
      file via MCI, restart on press) lives in `pulpit-os` and the
      shared `SqlBackend`; the switch survives COM-apartment teardown
      (`CoInitializeEx` S_FALSE means a pre-existing MTA we must not
      `CoUninitialize`). Linux support: the `Speaker` trait is
      the seam - swap in an ALSA/PipeWire implementation.
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
      Cutover (2026-09-22): the editor replaced the original app for daily
      use on this machine - installed from the NSIS bundle, HKCU Run
      autostart on, original autostart bat disabled.
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
- **Rebrand to Pulpit (2026-09-24)**: own product name and data directory
      instead of living in Deckboard's shadow. Data is copied, not moved:
      `~/deckboard` stays intact for the original app, and every
      wire-visible string the stock client or original extensions depend
      on (protocol payloads, extension ids, settings keys) keeps the old
      name. ADR-011.
- **Single port 8500** for both protocols: legacy lives at
      `/socket.io/?EIO=3...`, protocol v2 at `/v2/ws` (plain WebSocket, JSON
      frames, see `pulpit-proto`). The temporary 8501 split ended on
      2026-09-22: the original desktop app was retired from daily use (its
      `deckboard.bat` autostart removed), 8500 is the default again, and the
      desktop editor is the daily driver (single instance, logs in
      `~/deckboard/logs/`, close-to-tray).
- **ffmpeg optional**: transcoding/normalization happens only when a
  `ffmpeg` binary is found in PATH; otherwise assets are served as-is and
  the client decodes natively (H.264/VP9 in WebView/Media3).
- **Web widget foundation without full implementation**: the manifest
  schema (`WidgetKind::Web`, `web_package`, interactions, state refs) is
  defined and versioned in `pulpit-proto` now, so boards authored later
  never need a storage migration. The WebView runtime itself lands in M6.

## Testing

- Unit: payload mapping vs original behavior (fillers, 4x3 crop,
  command transformation, `extra` listener keys, icon/style defaults).
- Integration: both transports driven end-to-end (polling flow with raw
  HTTP, websocket flow incl. `2probe`/`3probe` upgrade and pings).
- Command dispatch is exercised through a mock `Input` seam; the enigo
  backend stays unexercised in CI (it touches the real OS).
