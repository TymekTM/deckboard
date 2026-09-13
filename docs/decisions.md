# Architecture decisions

Short, reviewable records of the choices that shape the codebase.
Newest at the bottom.

## ADR-001: Desktop is the single source of truth; one writer owns the DB

The desktop owns `~/deckboard/database.db` and all board/command state.
Tablets are live renderers with an offline cache; they never write.

Consequence: **the original Deckboard desktop app must be closed while
deckboard-server runs** (both would write the same file and both want port
8500). M0 additionally opens the DB in SQLite read-only mode, so a careless
double-start can corrupt nothing - the server just reads stale-but-valid
data while the original app runs.

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

`deckboard-proto` defines frames, widget manifests and state channels with
serde; TypeScript and Kotlin types are generated from it (codegen lands
with M1). JSON on the wire for debuggability; assets (photos, videos, web
widget bundles) are served by hash from the desktop instead of being
inlined as data URLs (the original inlined everything).

## ADR-005: Web widgets are trusted code in v1 - honestly named

All web widgets of a board render in ONE WebView (one JS context) to keep
memory flat. That means widgets can touch each other; the sandbox protects
the OS and the desktop, not widget-vs-widget. `deckboard.fetch` goes
through the desktop proxy, which is why the proxy only allows configured
hosts per widget (manifest `net` permission) - this closes the LAN-SSRF
hole but does not make widgets mutually isolated. If widgets are ever
shared with third parties, upgrade to origin isolation (one iframe/origin
per widget) before publishing that. Revisit at M6.

## ADR-006: Reconnect falls back to full snapshots

The client tracks a generation counter from `welcome`/`presence`. If it
reconnects with a generation gap greater than N (start with: any gap, i.e.
any disconnect) or a delta request times out after 5 s, it requests a full
`boards.sync` snapshot instead of deltas. Deltas are an optimization, never
a correctness requirement.

## ADR-007: Tablet connectivity requires a foreground service

Android Doze kills idle WebSockets. The Compose client (M4) runs its
connection loop in a foreground service and prompts the user to exempt the
app from battery optimization on first run. Without this the "live
controller" silently dies with the screen off.

## ADR-008: LAN trust with a pairing token on v2

The legacy layer stays unauthenticated (LAN trust, like the original) - do
not tunnel it through the internet. Protocol v2 adds a pairing token
carried in the QR (`deckboard://host:port?token=...`) and checked at the
WebSocket upgrade; the desktop shows a "trust this device?" prompt.

## ADR-009: Structured logging from day one

`tracing` with env-filter (`RUST_LOG=debug`) everywhere; WS, protocol and
dispatch events are spans/fields, not string interpolation. Debugging
WS + Compose + WebView interactions across three codebases without
structured logs was the reviewer's pointed warning - accepted.

## ADR-010: ffmpeg is opportunistic

Media normalization (downscale, H.264 baseline transcode) runs only when a
`ffmpeg` binary is found in PATH. Without it, assets are served as-is and
clients use their native decoders. No build-time dependency, ever.
