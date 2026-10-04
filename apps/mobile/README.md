# Pulpit Android client (protocol v2)

The tablet client for the Pulpit server, speaking protocol v2 only
(`docs/protocol-v2.md`): one plain WebSocket to `/v2/ws`, token pairing,
`boards.sync` + `boards.delta` for the board snapshot, `state.sync` +
`state.patch` for live values, `interaction` frames for taps/presses.

## Build

Requirements: JDK 17, an Android SDK (API 34) and Gradle 8.7 on the path.
`apps/mobile/local.properties` (gitignored) points at the SDK via
`sdk.dir=`. From `apps/mobile/`:

```
gradle :app:assembleDebug        # dev build
gradle :app:assembleRelease      # R8-minified, debug-signed
gradle :app:testDebugUnitTest    # wire/fixture/delta unit tests
```

The golden wire fixtures in `crates/proto/tests/fixtures` are parsed by
`ProtoFixturesTest`, so the Kotlin models cannot drift from the Rust ones.

## Pairing and running

1. Run the server (`apps/server`); it prints a QR URL and mints one-time
   codes on `POST /v2/pair` (loopback only).
2. In the app enter the PC address, port, and the current pairing code;
   tap Pair. The `welcome` frame carries the device token - it is stored
   and every later start reconnects with it automatically.
3. Interactions: clients send only gestures a tile declares; key tiles
   declare the `press-start`/`press-end` pair (server-side hold-to-repeat),
   everything else taps.

The device is a dedicated deck: a live board keeps the screen on and the
app starts itself after a reboot (`BootReceiver`). A foreground
keep-alive service (ROADMAP M4, `LinkService`) starts with every paired
launch and holds the process - and with it the WebSocket - through
screen-off and Doze, so a dark screen does not starve the link. The
older plan-008 behavior (close the socket the moment the app leaves the
foreground) remains as the fallback once the service stops: the
notification's "Rozłącz" action, unpairing, or swiping the app away put
it back in charge. Independently of the link policy: if the PC stays
unreachable for 3 minutes while the deck sits in front - asleep,
crashed, off the network - the app stops holding the screen on, so the
system screen timeout applies; waking the tablet reconnects (or repaints
the cached board). A deliberate server exit (`server.shutdown`) switches
to the goodbye screen right away. Pairing data survives reinstalls as
long as the app is updated with `adb install -r`.

## Notes

- Connect over the LAN address of the PC. The `adb reverse` tunnel drops
  server-to-client frames on some devices - fine for installs, not for
  running the deck.
- Asset images (tile images, board backgrounds) load from
  `/assets/<hash>?token=<device token>`.
- Release minification relies on the keep rules bundled with
  kotlinx-serialization and OkHttp; no custom proguard file is needed.
