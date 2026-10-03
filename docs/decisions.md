# Architecture decisions

Short, reviewable records of the choices that shape the codebase.
Newest at the bottom.

## ADR-001: Desktop is the single source of truth; one writer owns the DB

The desktop owns `~/pulpitApp/database.db` and all board/command state.
Tablets are live renderers with an offline cache; they never write.

Consequence: within Pulpit, one process writes the database at a time.
Historically this also meant the original Deckboard app had to be closed
(same file, same port 8500); since the 2026-09-24 rebrand Pulpit keeps its
own copy in `~/pulpitApp` (ADR-011) and the two no longer share state. The
headless server still opens the DB in SQLite read-only mode, so a careless
double-start can corrupt nothing.

## ADR-002: Legacy protocol is a permanent, tested fallback layer

The stock Android client (Free + Pro) speaks socket.io v2 / Engine.IO v3 on
port 8500 with handshake query `access_key` (exact match
`DCKBRD_PRO_1_3_0` -> PRO room, anything else -> BASIC room, which crops
boards to 4x3). This contract is frozen and covered by integration tests.
The reply to `get_version` is byte-identical to the original
(`{"version":"1.6.0"}` - the original hardcodes it despite shipping 3.2.0).

Consequence: during the whole rewrite the user's Pro tablet keeps working;
the new Kotlin client and protocol v2 can land without breaking it.

## ADR-003: Engine.IO v3 is implemented in-house

No Rust crate speaks socket.io v2 (socketioxide is v5/EIO=4 only). The
legacy layer implements EIO=3 directly on axum: polling handshake with the
open packet, `\x1e`-separated polling batches, websocket transport with
`2probe`/`3probe` upgrade and `2`/`3` pings, socket.io packets `40`/`42`.
~600 lines, fully integration-tested.

## ADR-004: Protocol v2 is typed once, in Rust

`pulpit-proto` defines frames, widget manifests and state channels with
serde; TypeScript and Kotlin types are generated from it (codegen lands
with M1). JSON on the wire for debuggability; assets (photos, videos, web
widget bundles) are served by hash from the desktop instead of being
inlined as data URLs (the original inlined everything).

## ADR-005: Web widgets are trusted code in v1 - honestly named

All web widgets of a board render in ONE WebView (one JS context) to keep
memory flat. That means widgets can touch each other; the sandbox protects
the OS and the desktop, not widget-vs-widget. `pulpit.fetch` goes
through the desktop proxy, which is why the proxy only allows configured
hosts per widget (manifest `net` permission) - this closes the LAN-SSRF
hole but does not make widgets mutually isolated. If widgets are ever
shared with third parties, upgrade to origin isolation (one iframe/origin
per widget) before publishing that. Revisit at M6.

## ADR-006: Deltas push live; snapshots recover (supersedes "full snapshots only")

Board changes flow as `boards.delta` op batches (`board-set`, `board-remove`,
`tile-set`, `tile-remove`, `tile-clear`), each bumping a monotonic
generation; full `boards.sync` snapshots flow once after `welcome` and
whenever the editor replaces board data wholesale (import). Deltas are an
optimization, never a correctness requirement: TCP ordering means a
connected client cannot miss a delta, so any reconnect simply recovers via
snapshot - no delta requests, no gap bookkeeping, no 5 s timeout (the
earlier draft of this ADR had those; the simpler model replaced them).

## ADR-007: Tablet connectivity requires a foreground service

Android Doze kills idle WebSockets. The Compose client (M4) runs its
connection loop in a foreground service and prompts the user to exempt the
app from battery optimization on first run. Without this the "live
controller" silently dies with the screen off.

## ADR-008: LAN trust with one-time pairing codes, a trust prompt and per-device tokens

The legacy layer stays unauthenticated (LAN trust, like the original) - do
not tunnel it through the internet. Protocol v2 authenticates at the
WebSocket upgrade. Pairing mints a one-time code (8 chars, 5 min TTL,
loopback-only minting, QR `pulpit://host:port?pair=<code>`); a code burns
on first use, five wrong codes invalidate every outstanding one, and
codes are never written to the logs.

Trust is a desktop decision (built, 012 B2): converting a code into a
device first asks the operator in a native "trust this device?" dialog.
The wait is bounded by the code TTL - an unanswered prompt denies the
pairing once the code would have expired anyway - and a denial rejects
the `hello` and burns the code, so retrying needs a fresh one. Headless
builds (no operator to ask) keep the auto-accept default with a warning
log. Trusting creates a per-device entry in `~/pulpitApp/devices.json`
(`{id, name, created, last_seen}` plus a SHA-256 token digest only; the
plaintext token travels exactly once, in the pairing `welcome`).
`hello.name` is sanitized (trimmed, control characters stripped, 64-char
cap) before it reaches the registry, the logs or the desktop device list.

The desktop settings (the "Tablety" section) list paired devices with
their last-seen time; revoking a device deletes its entry and closes its
live sessions immediately. The tablet keeps its token in app-private
SharedPreferences with `allowBackup="false"`; EncryptedSharedPreferences
stays open as a hardening step (owner decision pending).

## ADR-009: Structured logging from day one

`tracing` with env-filter (`RUST_LOG=debug`) everywhere; WS, protocol and
dispatch events are spans/fields, not string interpolation. Debugging
WS + Compose + WebView interactions across three codebases without
structured logs was the reviewer's pointed warning - accepted.

## ADR-010: ffmpeg is opportunistic

Media normalization (downscale, H.264 baseline transcode) runs only when a
`ffmpeg` binary is found in PATH. Without it, assets are served as-is and
clients use their native decoders. No build-time dependency, ever.

## ADR-011: Own identity, copied data (the Pulpit rebrand)

The project ships under its own name (`Pulpit`, `pulpit-*` crates, env
prefix `PULPIT_*`, deep link `pulpit://`) with its own data directory
`~/pulpitApp`. On first start `pulpit_db::data_dir` copies a legacy
`~/deckboard` directory (database, settings, editor config, paired
devices, extensions, assets; logs excluded) instead of moving it.

Consequence: the original app keeps working from its own directory, the
upgrade cannot lose data (copy, per-item resumable), and every
wire-visible identifier the stock client or original extensions depend
on keeps the Deckboard name: the socket.io v2 protocol, `.boardjson`
format, extension ids (`deckboard-system-info`, `deckboard-callurl`,
`discord-deckboard` settings keys) and the extension JS API
(`DeckboardExtension`). The Tauri identifier change (`app.pulpit.desktop`)
means the NSIS bundle installs next to, not over, the old build - a
one-time manual uninstall.

## ADR-012: Extensions are trusted user-installed code (stub)

Status: stub recording current behavior, not a fresh decision. The
extension host (crates/ext, ADR-011's copied `~/pulpitApp/extensions`)
runs original-ecosystem packages with full user powers: unrestricted
file reads/writes, `cmd` shell execution, arbitrary HTTP and `open`.
There is no permission model and no sandbox; installing an extension is
assumed to be as deliberate as installing any desktop app. This is a
different trust boundary from ADR-005 (web widgets share one WebView but
are proxy-gated); extensions are NOT proxy-gated. Hardening so far
bounds robustness, not trust: per-call HTTP timeouts, load/dispatch
timeouts for wedged packages, private per-open extraction dirs. If a
third-party extension marketplace or sideloaded-package sharing ever
lands, revisit with a permission model before that - not after.
