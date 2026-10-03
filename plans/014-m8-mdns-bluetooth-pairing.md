# 014 - M8 discovery: Bluetooth-style pairing (owner spec, 2026-10-03)

Status: SHIPPED (desktop advertisement + pair-request endpoints + gate
dialog with the verification code; tablet NsdManager browse + Auto/Manual
mode switch + poll-until-paired; ADR-013). Manual mode unchanged.

Both devices declare that they are open/looking. When they find each
other, a verification code shows on BOTH sides and each side confirms
"yes, I want to connect to this device". Keep manual pairing working
alongside; support many hosts and many clients.

## Design

- **Advertisement**: the desktop advertises `_pulpit._tcp` (mDNS, port
  8500, TXT: `proto=v2`, `version`) whenever the server runs. Pure-Rust
  stack (`mdns-sd`) - no system mDNS dependency on Windows.
- **Browse**: the tablet discovers instances with Android `NsdManager`
  (no new dependency). Many desktops may advertise; many tablets may
  browse; nothing pairs without a human on both ends.
- **Auto mode (new)**: tablet taps a discovered desktop ->
  `POST /v2/pair-request {name}`. The server mints the ordinary one-time
  pairing code, and the desktop shows its trust dialog WITH the code:
  "Urządzenie 'SM-T561' chce się połączyć. Kod: 482913 - zaufać?"
  The tablet shows the same code (from the POST response). Pairing
  completes only if the desktop user confirms; the tablet polls the
  request endpoint until trusted (token issued through the existing
  Pairing exchange, trust pre-answered) or rejected/expired.
  This is numeric-comparison, Bluetooth-style: same code on both
  screens, a confirm on each.
- **Manual mode (unchanged)**: desktop "Dodaj urządzenie" mints a code,
  the tablet types it, the B2 trust gate asks as today.
- ConnectScreen gets a mode switch: "Automatycznie (wykryte komputery)"
  vs "Ręcznie (kod z komputera)".

## Surfaces

- `crates/v2`: advertise on server start; `pair-request` endpoints;
  trust-gate callback gains the code in its message (desktop lib.rs
  dialog text only).
- `apps/mobile`: NsdManager browse + auto-mode UI + poll-until-paired.
- `apps/desktop`: trust dialog text carries the verification code.
- Docs: protocol-v2.md endpoints + ADR for the pairing modes.
