# Protocol v2 specification

Pulpit protocol between the desktop server and its clients (tablet,
future remote surfaces). Status: **implemented for M1** in `crates/proto`
(types), `crates/v2` (transport), generated TypeScript in
`crates/proto/bindings/`. Decisions: `docs/decisions.md` (ADR-004/006/008).

The legacy socket.io v2 protocol (`/socket.io/`, stock Android app) is
frozen and documented by its code (`crates/legacy`); this document only
covers v2.

## 1. Transport

- Plain WebSocket, JSON text frames, endpoint `/v2/ws` on the same port as
  the legacy server (8500 long-term; 8501 while the original app owns the
  port). One TCP connection per client session.
- Max inbound frame: 1 MiB. Larger frames get an `error` frame
  (`code: "too-large"`) and close the connection.
- Keepalive: the server sends a WebSocket protocol-level ping every 60 s.
  The client's runtime pongs automatically. A client that sees no inbound
  frames (including pongs) for 90 s reconnects with backoff. There are no
  application-level ping/pong frames.
- Ordering: frames from the server arrive in the order produced (TCP).
  Server → client pushes and acks share one FIFO per connection.
- Backpressure: each session drains its outbound queue through one pump.
  The queue is bounded; a client that stops reading while the producer
  keeps pushing gets the connection closed on overflow - a plain
  WebSocket close that may come without a preceding `error` frame (the
  queue that would carry it is the full one). The client reconnects and
  recovers from the snapshot. A unicast ack can be dropped in the same
  situation, so clients must treat interactions as fire-and-forget
  unless they implement their own confirmation.
- Silence watchdog: a session with no inbound frames (data or pongs) for
  3 x the ping interval is torn down like an overflow, even if TCP has
  not noticed the dead peer yet.

## 2. Frame envelope

```json
{ "v": 2, "id": "c1", "ack": null, "type": "interaction", "payload": {} }
```

- `v`: protocol version constant (`PROTOCOL_VERSION = 2`). Bumped only on a
  breaking envelope change, never for additive message/field additions.
- `id`: set by the requester (client → server) on request frames. The
  server echoes it back in `ack` on the response frame.
- `ack`: set on response frames to the request's `id`. Server pushes have
  no `id`/`ack`. Request-response exists only in the client → server
  direction.
- `type`: dotted, kebab-case names. Reserved control types: `hello`,
  `welcome`, `error`. Domain types: `boards.sync`, `boards.delta`,
  `board.open`, `state.sync`, `state.patch`, `interaction`. Reserved for
  future use: `widget.event`, `boards.write`. Control, server-to-client
  only: `server.shutdown` (see section 9).
- `payload`: omitted when empty.
- Unknown inbound `type`: if the frame carries an `id`, answer
  `error {code: "unknown-type"}`; otherwise ignore and log.
- Malformed frame (bad JSON, wrong shape): `error {code: "bad-frame"}`;
  connection stays up unless the error is fatal (see error codes).

Error frame:

```json
{ "v": 2, "ack": "c1", "type": "error", "payload": { "code": "unknown-tile", "message": "no tile 42" } }
```

Codes: `unauthorized`, `pair-invalid`, `pair-expired`, `outdated-client`,
`unknown-type`, `bad-frame`, `too-large`, `unknown-tile`,
`unsupported-interaction`, `internal`. Fatal codes close the connection:
`unauthorized`, `pair-invalid`, `pair-expired`, `outdated-client`,
`too-large`.

## 3. Handshake and pairing

### Authenticated connect

`GET /v2/ws?token=<device token>` - the token is checked at the upgrade.
Invalid token: HTTP 401, no WebSocket. Devices and their tokens live in
`~/pulpitApp/devices.json` (array of `{id, name, token, created,
last_seen}`; `id` and `token` are random hex, 16 and 32 bytes).

### Pairing a new device

1. Desktop generates a one-time code (8 chars from an unambiguous
   alphabet - A-Z minus I/O plus digits 2-7, so no 0/O, 1/I look-alikes;
   valid 5 minutes) via `POST /v2/pair` (loopback callers only).
   Response: `{"code": "ABCD2345", "expires_in": 300}`. The M1 headless
   server logs the QR-able URL (`pulpit://<lan-ip>:<port>?pair=<CODE>`);
   the desktop UI prompt ships with the editor.
2. Tablet connects `/v2/ws?pair=<CODE>` and sends `hello` within 5 s.
   Unknown or burned codes are answered on the socket with
   `pair-invalid` (expired: `pair-expired`) and closed - only unknown
   device *tokens* get an HTTP 401 at the upgrade.
3. The desktop shows "Trust this device?" using the `hello` name. M1:
   auto-accept with a warning log (no UI yet).
4. On trust: a device entry is created and `welcome` carries the new
   `token` - the only time the secret travels on the wire, and only to the
   connection that just presented a valid pairing code. The tablet stores
   it (Android: `EncryptedSharedPreferences`).
5. Every later connect uses `?token=...`; no prompt. A non-empty
   `hello.name` may rename the paired device - the change is persisted to
   `devices.json` so the welcome and the desktop device list agree.
   Revoking a device (removing it from `devices.json` via future desktop
   UI) makes the next connect fail with `unauthorized`.

QR payload: `pulpit://<host>:<port>?pair=<CODE>`.

### hello / welcome

`hello` (client → server, first frame, required within 5 s of connect):

```json
{ "v": 2, "id": "h1", "type": "hello",
  "payload": { "client": "pulpit-mobile", "version": "0.2.0",
               "name": "Tablet salon",
               "capabilities": ["kinds:button", "series", "gestures:long-press"] } }
```

`welcome` (server → client, replies to `hello`):

```json
{ "v": 2, "ack": "h1", "type": "welcome",
  "payload": { "protocol": 2, "desktop_version": "0.1.0",
               "min_client": "0.0.0", "generation": 7,
               "device": { "id": "9ab...", "name": "Tablet salon" },
               "token": "64-hex-chars...",
               "channels": { "ext.si-cpu-usage": { "shape": "series", "cap": 120 } },
               "capabilities": ["series", "state.patch", "assets", "assets2", "gestures"] } }
```

- `channels` is the full catalog of live state channels: name →
  `{shape, cap?}` (`cap` only for `series`; the server's ring-buffer size).
- `min_client`: below this client version the server closes with
  `outdated-client` after `hello` (enforcement is a server config; M1
  ships `0.0.0` = never block - we publish both ends).
- Version mismatch without a block: degrade by capability, never guess.
- Capabilities (M5): free-form `name` or `name:value` strings. The
  client declares what it renders/accepts (`kinds:*`, `series`,
  `state.patch`, `assets`, `assets2`, `gestures:*`), the server echoes
  its own set in `welcome.capabilities`. Both sides log the sets; no
  behavior is gated on them yet. Old servers never send the field, old
  clients ignore it.

After `welcome` the server immediately pushes `boards.sync` and then
`state.sync` (sections 4-5). There is no client-pull variant; a confused
client reconnects.

### Pair-request (M8, Bluetooth-style)

For a tablet that discovered the desktop over mDNS (`_pulpit._tcp.`,
advertised while the server runs; TXT `proto=v2`, `version`, `host`):

1. `POST /v2/pair-request` `{"name": "SM-T561"}` - LAN only, loopback is
   refused (the desktop has its own dialog), browser `Origin` refused
   like everywhere (B1). One request may be live at a time (else `409`).
   The response carries the **verification code**
   `{"request_id", "code", "expires_in_secs"}` - the tablet displays it.
2. The desktop shows its gate dialog with the SAME code
   (`pairing.set_pair_request_gate`); approving marks the code
   pre-approved (single-use), denying sets a rejection.
3. The tablet polls `GET /v2/pair-request/:id` -
   `{"status": "pending" | "approved" | "rejected" | "expired"}`.
   On `approved` it opens `/v2/ws?pair=<code>` + `hello` as usual; the
   pre-approval replaces the operator dialog on that path, the code
   burns, the token is issued in `welcome.token`.

The verification code is a numeric-comparison: pairing completes only
when the same number is visible on both screens and a human on each side
proceeds. Headless builds (no gate) auto-accept with a warning log.
Manual pairing (desktop mints, tablet types) keeps working unchanged.

## 4. Boards

Boards are data. One board:

```json
{ "id": 3, "name": "Media", "width": 6, "height": 5, "order": 0,
  "background": { "kind": "color", "color": "#2c3e50" },
  "tiles": [ { "id": 17, "x": 0, "y": 0, "w": 1, "h": 1,
               "kind": "button", "params": {},
               "state": { "channel": "ext.speaker-muted", "shape": "scalar" },
               "interactions": ["tap"],
               "style": { "color": "#F5AB35", "color2": "#ED4245",
                          "icon": "\uf026", "icon2": "\uf028", "icon_family": "fas",
                          "title": "Mute", "title_color": "#ffcc00",
                          "border_color": "#101010", "icon_color": "#ffe0e0" },
               "asset_hash": "<sha-256 hex>", "asset_hash2": "<sha-256 hex>" } ] }
```

- Placement is on the 96px cell grid (`x`,`y`,`w`,`h`, integers), tiles may
  be any rectangle (3x1, 3x3, ...). Non-rectangular shapes are out of scope.
- `background`: `{"kind":"color","color":...}` or
  `{"kind":"asset","hash":...}` (sha-256 hex, see section 7).
- `params`: free JSON (widget options, e.g. `hold.repeat`).
- `asset_hash`: content hash of the tile's image asset (button image,
  photo, video). Legacy `img`/`img2` data URLs are converted to store
  entries on the fly when the server builds a sync; tiles whose image
  cannot be converted simply omit it.
- `asset_hash2`: content hash of the active-state image (legacy `img2`),
  shown instead of `asset_hash` while the tile's channel reports its
  active value. Optional; absent means the tile has no second image.
- `style`: `color`/`color2`/`icon`/`icon2`/`icon_family` (`fas`|`fab`,
  resolved glyph fonts)/`title`/`shape` - all optional, resolved
  server-side the same way the legacy mapper resolves them (DB value →
  type default → fallback). `color2`/`icon2` are the active-state pair:
  the client swaps to them while the tile's channel reports its active
  value (e.g. `"ON"`). An unset `color2` is filled with the type's
  default color server-side (the legacy chain) before it goes on the
  wire; types without a default omit it and the §4 client fallback
  applies.
- Style parity fields (added 2026-10, 012 C5): `border_color`,
  `icon_color`, `title_color` and their `*_color2` active-state pairs.
  All optional; a client that does not know them keeps its defaults
  (no border, white glyph/title).
- Style parity fields (added 2026-10, round 4): `title_position`,
  `title_position2` (numbers: 0 = bottom - the default, omitted from
  the wire -, 1 = center, 2 = top), `title_box_color`,
  `title_box_color2` (background strip behind the title) and `shape2`
  (active-state shape, stringified like `shape`: `"1"` renders round
  while active). All optional with the same §4 fallback; the legacy
  wire has carried all five since the original app.
- **State-2 fallback rule (one rule for every field)**: while the tile
  is in its active state, each state-2 field (`color2`, `icon2`,
  `border_color2`, `icon_color2`, `title_color2`, `title_position2`,
  `title_box_color2`, `shape2`, `asset_hash2`, ...) falls back to its
  state-1 counterpart **per field** when absent -
  `active ? (field2 || field1) : field1`. A fully absent state-2 set
  leaves the tile visually unchanged between states; a partially set
  one changes only the fields that are set.
- `interactions` lists the gestures the tile accepts (section 6).

### boards.sync (server → client, full snapshot)

```json
{ "v": 2, "type": "boards.sync", "payload": { "generation": 7, "boards": [ ... ] } }
```

Sent after `welcome`, after board-changing writes (imports), and on the
editor's request channel (internal - the editor writes in-process, not via
WebSocket, see ADR-001/004).

### boards.delta (server → client, live change)

```json
{ "v": 2, "type": "boards.delta",
  "payload": { "generation": 8, "ops": [
      { "op": "tile-set", "board": 3, "tile": { ... } } ] } }
```

Ops: `board-set` (create/update, carries `board`), `board-remove`,
`tile-set` (add/update/move/resize, carries full `tile`),
`tile-remove`, `tile-clear` (all tiles of a board). Every committed write
batch bumps `generation` by one and produces one `boards.delta` per batch.
Deltas are broadcast to all clients; a client that falls behind or
reconnects recovers via `boards.sync`. A full snapshot after import beats
inventing a mega-delta.

### board.open (server → client, directive)

```json
{ "v": 2, "type": "board.open", "payload": { "board": 3 } }
```

Emitted when an executed command switches boards (multiaction `board`
step / `board` command) - the v2 equivalent of the legacy `change_board`
broadcast. Purely a client-UI directive; board data flows only through
`boards.sync`/`boards.delta`.

## 5. State

- Channels are flat, namespaced strings owned by their producer:
  `sys.*` (native watchers), `ext.<key>` (extension/native pushes, key =
  the legacy watch key, e.g. `ext.speaker-muted`, `ext.si-cpu-usage`),
  `vm.*`, `discord.*`, `obs.*` (M7). Producers push; the server fans out.
- Shapes: `scalar` (string/number/bool), `series` (numeric points),
  `toggle` (bool), `list` (array). Unknown shape degrades to scalar
  rendering client-side.
- No subscriptions, no filtering: the server pushes every channel to every
  authenticated client. LAN scale makes this a non-problem; revisit only
  with evidence.
- Series history lives server-side: ring buffer of the last `cap` (120)
  points per series channel. Clients render the window from `state.sync`
  and append points from `state.patch`; they keep no own history.

### state.sync (server → client, once after welcome)

```json
{ "v": 2, "type": "state.sync",
  "payload": { "values": { "ext.speaker-muted": "OFF" },
               "series": { "ext.si-cpu-usage": [0.1, 0.42] } } }
```

Series arrays run oldest → newest.

### state.patch (server → client, live)

```json
{ "v": 2, "type": "state.patch",
  "payload": { "changes": [ { "channel": "ext.si-cpu-usage", "value": 0.44 } ] } }
```

The server coalesces per connection and flushes every 100 ms, latest wins
per channel (a series flush carries its newest point). Producers push only
on change. No `state.get`, no `state.subscribe`.

## 6. Interaction

Client → server, one frame per user gesture:

```json
{ "v": 2, "id": "i9", "type": "interaction",
  "payload": { "board": 3, "tile": 17, "interaction": "tap", "args": {} } }
```

- Kinds: `tap`, `press-start`, `press-end`, `slide` (`args.value`, 0..1),
  `long-press`, `double-tap`, `swipe-left`, `swipe-right` (M5 custom
  gestures, no args), `wheel` (`args.delta`), `drag` (`args.dx`,
  `args.dy`). `press-start` / `press-end` replace the legacy `isTapStart`
  bool pair.
- Clients send only gestures the tile declares in `interactions`.
  Declarations: plain buttons declare `tap` (fire once on release);
  key-style commands and tiles with `params.hold.repeat` declare
  `tap` + `press-start` + `press-end` (down/up semantics, hold-to-repeat);
  sliders/knobs declare `slide`; displays declare none. A tile may add
  M5 custom gestures through its options JSON:
  `{"gestures": ["long-press", "double-tap", "swipe-left", "swipe-right"]}` -
  the closed set travels into `interactions` and each declared gesture
  fires the tile's action once on completion (alternative triggers, not
  press modes: no key-hold, no repeat; never on slider/knob, where the
  drag surface belongs to the value). The server rejects undeclared
  gestures with `unsupported-interaction`.
- The server validates the tile exists and answers
  `ack {ok: true}` (payload `{}`) or `error` (`unknown-tile`,
  `unsupported-interaction` for gestures the tile/backend cannot serve,
  e.g. wheel/drag in M1). Execution is asynchronous; its effects surface
  through `state.patch` / `board.open` / `boards.delta`, never through the
  ack. The ack ordering is FIFO per connection.
- Hold-to-repeat: if the tile's `params.hold.repeat` =
  `{"delay_ms": 400, "interval_ms": 120}`, `press-start` starts a
  server-side loop re-executing the tile's command (first run immediately,
  then after `delay_ms`, then every `interval_ms`); `press-end` stops it.
  A repeat loop is capped at 120 s and released when the connection dies.
  Push-to-talk-style holds (command active while held, no repeat) run from
  `press-start` to `press-end` with no timeout. Key semantics stay with
  the command engine (`press-start` = legacy `isTapStart: true`).

## 7. Assets

- Store: `~/pulpitApp/assets/<sha256-hex>.<ext>` (env
  `PULPIT_ASSETS`). Content-addressed, so imports are idempotent.
- Endpoint: `GET /assets/<sha256-hex>?token=<device token>` -
  `Cache-Control: immutable, max-age=31536000`, content type from the
  stored extension. Wrong/missing token: 401. Unknown hash: 404.
- Desktop-side import (editor picks a file): in-process call; M1 ships it
  as `AssetStore::import_bytes`/`import_data_url` used by board builds.
- Legacy conversion: when building `boards.sync`, data-URL images from the
  DB (`img`/`img2`, ~KBs of base64) are decoded, hashed into the store,
  and referenced by `asset_hash`. Unreadable values are skipped (tile
  renders without image).

## 8. widget.event (reserved)

Two-step widget flows (confirm, prompt) and web-widget messaging (M6) will
use `widget.event` frames with a `request_id` correlation. Nothing before
M6 needs it; the name is reserved so M1 clients can safely ignore it.

## 9. server.shutdown (server -> client, on exit)

When the server exits on purpose (user quits the app, or the machine is
shutting down), it sends one `server.shutdown` frame to every attached
session, immediately followed by a WebSocket close:

```json
{ "v": 2, "type": "server.shutdown" }
```

The frame carries no payload (the envelope omits it when empty). "Every
attached session" means authenticated ones: a socket still inside its
handshake misses the goodbye and sees a bare drop.

The frame is the signal that the exit is deliberate: a conforming client
stops reconnecting (it may show an idle/offline state instead) and can
drop any keep-awake behavior. A plain disconnect without the frame keeps
its usual meaning - transient loss, retry. Clients that predate the type
ignore the unknown frame and behave as before.

The frame is a courtesy, not a guarantee: if the process is killed hard
or the flush loses the race with process teardown, the client sees a bare
disconnect and retries as usual.

## 10. Versioning and evolution

- Additive changes (new message types, new optional fields, new enum
  values) never bump `v` and never break a conforming client: unknown
  message types are ignored (or `unknown-type` on request), unknown fields
  dropped, unknown enum values degrade (`WidgetKind::Other`,
  `StateShape::Other`).
- `PROTOCOL_VERSION` bumps only for breaking envelope changes.
- Change log: 2026-10 (012 C5) added the optional style parity fields
  `border_color`/`icon_color`/`title_color` (+ `*_color2` pairs) to
  `style` and `asset_hash2` to the tile manifest. Purely additive: old
  servers never send them and old clients drop unknown keys, so `v`
  stays 2.
- Change log: 2026-10 (round 4) added the optional style fields
  `title_position`/`title_position2` (numbers),
  `title_box_color`/`title_box_color2` and `shape2` to `style`.
  Additive optional fields per §10 - `v` stays 2.
- Wire compatibility is pinned by golden fixtures
  (`crates/proto/tests/fixtures/*.json`): Rust round-trips them and the
  Kotlin unit test parses the same files. Both must stay green.

## 11. Codegen

- Types are defined once in Rust (`crates/proto`, serde) - ADR-004.
- TypeScript: generated with ts-rs into `crates/proto/bindings/` by
  `cargo test -p pulpit-proto` (committed so the editor/Vue app can
  consume them without a build step).
- Kotlin: hand-written `@Serializable` mirrors in `apps/mobile`
  (`proto/Models.kt` grows v2 types with the M4 client), validated against
  the same fixtures by a JVM unit test.

## 12. Coexistence with legacy

Both protocols live on one port: legacy under `/socket.io/` (frozen,
stock client) and health on `/`, v2 under `/v2/ws` and `/assets/`,
pairing under `POST /v2/pair`. The DB is single-writer (the editor or the
headless server, never both - ADR-001).
