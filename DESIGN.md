---
name: Pulpit — ŚCIANA (Settings Wall)
description: Settings as a streaming-style tile wall — one focused tile, a dimmed wall, teal reserved for focus and primary actions only.
colors:
  ground: "#171c21"
  tile: "#262e36"
  tile-raised: "#313a44"
  line: "#1d2a36"
  well: "#171c21"
  lift: "#3a4753"
  ink: "#ecf0f1"
  ink-2: "#95a5a6"
  ink-3: "#8b97a1"
  accent: "#1abc9c"
  accent-deep: "#16a085"
  ok: "#1abc9c"
  bad: "#e74c3c"
  bad-ink: "#f19488"
typography:
  bar-title:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "18px"
    fontWeight: 600
    letterSpacing: "0.2px"
  section-label:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "12.5px"
    fontWeight: 600
    letterSpacing: "1.6px"
  tile-title:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 600
  control-label:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "13.5px"
    fontWeight: 500
  body:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: 1.55
  note:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.6
  chip:
    fontFamily: "Roboto, Segoe UI, system-ui, sans-serif"
    fontSize: "11px"
    fontWeight: 400
  data:
    fontFamily: "ui-monospace, Cascadia Mono, Consolas, monospace"
    fontSize: "13px"
    fontWeight: 400
  data-display:
    fontFamily: "ui-monospace, Cascadia Mono, Consolas, monospace"
    fontSize: "30px"
    fontWeight: 700
    lineHeight: 1.1
    letterSpacing: "6px"
rounded:
  control: "6px"
  panel: "8px"
  tile: "12px"
  chip: "999px"
spacing:
  tile-gap: "14px"
  section-gap: "30px"
  gutter: "34px"
  bar-height: "64px"
components:
  tile:
    backgroundColor: "{colors.tile}"
    textColor: "{colors.ink}"
    rounded: "{rounded.tile}"
    padding: "18px 20px"
  switch:
    backgroundColor: "{colors.tile-raised}"
    rounded: "13px"
    width: "46px"
    height: "26px"
  switch-on:
    backgroundColor: "{colors.accent}"
  chip:
    backgroundColor: "{colors.tile-raised}"
    textColor: "{colors.ink-2}"
    typography:
      fontSize: "11px"
    rounded: "{rounded.chip}"
    padding: "3px 9px"
  chip-ok:
    backgroundColor: "rgba(26, 188, 156, 0.14)"
    textColor: "{colors.ok}"
  chip-bad:
    backgroundColor: "rgba(231, 76, 60, 0.14)"
    textColor: "{colors.bad-ink}"
  act:
    backgroundColor: "{colors.tile-raised}"
    textColor: "{colors.ink}"
    typography:
      fontSize: "12.5px"
    rounded: "{rounded.control}"
    padding: "7px 12px"
  act-hover:
    backgroundColor: "{colors.lift}"
  act-accent:
    backgroundColor: "{colors.tile-raised}"
    textColor: "{colors.accent}"
    typography:
      fontSize: "12.5px"
      fontWeight: 500
    rounded: "{rounded.control}"
    padding: "7px 12px"
  act-primary:
    backgroundColor: "{colors.accent}"
    textColor: "#0c2a24"
    typography:
      fontSize: "12.5px"
      fontWeight: 600
    rounded: "{rounded.control}"
    padding: "9px 14px"
  act-primary-hover:
    backgroundColor: "{colors.accent-deep}"
  combo:
    backgroundColor: "{colors.tile-raised}"
    textColor: "{colors.ink}"
    typography:
      fontFamily: "ui-monospace, Cascadia Mono, Consolas, monospace"
      fontSize: "13px"
    rounded: "{rounded.control}"
    padding: "7px 12px"
  combo-input:
    backgroundColor: "{colors.well}"
    textColor: "{colors.ink}"
    typography:
      fontSize: "13px"
    rounded: "{rounded.control}"
    padding: "7px 10px"
  code-display:
    backgroundColor: "{colors.well}"
    textColor: "{colors.ink}"
    typography:
      fontFamily: "ui-monospace, Cascadia Mono, Consolas, monospace"
      fontSize: "30px"
      fontWeight: 700
      letterSpacing: "6px"
    rounded: "{rounded.panel}"
    padding: "14px 18px"
  qr:
    backgroundColor: "#ffffff"
    rounded: "{rounded.panel}"
    padding: "6px"
    width: "108px"
    height: "108px"
---

# Design System: Pulpit — ŚCIANA (Settings Wall)

## Overview

**Creative North Star: "ŚCIANA — the streaming wall"**

Settings is a wall of content, not a form. A full-screen dark overlay replaces the
editor window; every setting lives inside a **tile** — a live state container that
shows its status before you touch it (a chip, a summary, a QR, a pairing code).
Exactly one tile is expanded at a time: focus is the only mechanism that reveals
controls, and the rest of the wall dims in response. There is no sidebar, no row of
form fields, no wizard. The wall reads like a streaming service's shelf of title
cards: scan first, expand one thing, act, leave.

The world is deliberately quiet. Depth comes from tonal steps (ground → tile →
raised, with wells dipping back to ground) plus 1px lines, not from stacked shadows
— elevation is declared exactly once, on the focused tile. Color is rationed: a
three-stop gray ink scale does all the talking, and the teal accent appears only
where attention or life must land (focus border, switch-on state, one primary
action per tile, focus rings, and "working" status chips). Red stays out of the
accent conversation entirely — it is the error channel, with a lightened ink tint
for small text. Type is Roboto for voice and a monospace stack with tabular
numerals for every technical value: ports, IPs, hotkeys, pairing codes.

**Scope of this world.** ŚCIANA currently owns exactly one surface: the settings
overlay (`apps/desktop`, `SettingsOverlay.vue`, mounted from the rail in `App.vue`).
Everything else — the light sidebar (`#ececec`), the board-colored canvas, and the
light modal system (`#ffffff` modals, 4px radius, teal header strip) — is the
**incumbent world** inherited from the original Deckboard look. The wall's palette
is now sourced from the incumbent shell tokens (`style.css :root`: `--titlebar`,
`--rail`, `--rail-hover`, `--border`, `--text`, `--text-muted`, `--accent`,
`--danger`), so settings and shell share one color source — the earlier red and
dark-text drifts are gone by design. What remains is structural drift, recorded
here, not repaired: two modal languages (light dialogs vs this dark wall), two
radius languages (4px vs 6/8/12px), and two shadow vocabularies (the modal's
stacked shadows vs the wall's single focus ambient). Future surfaces decide world
by world; settings is the first step of the renovation, not a mandate to repaint
the editor.

**Key Characteristics:**
- Wall of tiles, one always focused; siblings dim to 0.42 opacity
- Tiles are live state containers (chips, summaries, QR, pairing code) before they are forms
- Tonal depth only; a single soft shadow marks the focused tile
- Teal accent strictly for focus, primary actions, and status-ok; red is the error channel only
- Roboto for prose, mono + tabular numerals for every technical value
- Motion budget: 140–250ms ease-out state changes, one rise reveal per detail, no page-load choreography

## Colors

An incumbent charcoal field (the shell's own dark family, not a blue-black of its
own), one teal accent, and one error red — everything else is gray discipline.
Every neutral and state value is lifted from the app shell's `:root` tokens
(`--titlebar`, `--rail`, `--rail-hover`, `--border`, `--text`, `--text-muted`,
`--accent`, `--danger` in `style.css`), so the wall and the rest of the editor
draw from a single color source; only the derived tints (`--ink-3`, `--bad-ink`)
are new here.

### Primary
- **Accent Teal** (#1abc9c, shell `--accent`): the focus and intent channel, and
  the status-ok color. Appears as: the focused tile's border (mixed 40% teal into
  the line color), the switch's on-state, the primary action button, the global
  `:focus-visible` ring, and "working" status chips (14% teal behind the text) —
  the same pairing the incumbent rail's status dots use. The hover/deep stop
  **Accent Deep** (#16a085, shell `--accent-2`) darkens those same elements on hover.

### Operate state (semantic channel)
- **Bad Red** (#e74c3c, shell `--danger`): the error channel — "broken / off /
  server down" chips (14% red behind the text) — e.g. "baza niedostępna", "serwer wył.".
- **Bad Ink** (#f19488): the lightened, text-legible tint of Bad Red used wherever
  red is small text on dark — bad chips' labels and inline validation errors — to
  hold 4.5:1 contrast. It never appears as a surface.

### Neutral
- **Ground** (#171c21, shell `--titlebar`): the overlay background; the wall's darkness floor.
- **Tile** (#262e36, shell `--rail`): the resting tile surface, one tonal step above ground.
- **Tile Raised** (#313a44, shell `--rail-hover`): second-level surface — chips,
  buttons, switch track, row separators between controls.
- **Line** (#1d2a36, shell `--border`): all 1px borders — tile outlines, kbd/code
  outlines, input borders.
- **Well** (#171c21): recessed input and code-display backgrounds; the ground value
  showing through a tile, outlining "type or read here".
- **Lift** (#3a4753): hover state of small controls (buttons, switch track) and the
  dim tile's hover border tint at 60% up (#43505c).
- **Ink** (#ecf0f1, shell `--text`): primary text — titles, values, control names.
- **Ink 2** (#95a5a6, shell `--text-muted`): secondary text — summaries, neutral chip text.
- **Ink 3** (#8b97a1): muted text — section labels, control notes, fact labels, footnotes.

### Named Rules
**The Two-Channel Rule.** Teal and red are different channels and never mix. Teal =
focus, primary actions, and status-ok (intent and life — matching the incumbent
app's teal status dots); red = error, surfaced as **Bad Ink** when it must be read
as small text. This separation is a reviewed decision (2026-09-28 settings review,
re-affirmed at the charcoal re-tint): state vocabulary stays legible even when
every accent is spent on focus.

**The Rationed Accent Rule.** On any view of the wall, teal covers a sliver of the
pixels: one border tint, at most one filled button, at most one switch. If a second
element wants teal, the design is wrong, not the budget.

## Typography

**Display/Data Font:** ui-monospace, "Cascadia Mono", Consolas, monospace
**Body Font:** Roboto (via @fontsource, weights 400/500/700), fallback "Segoe UI", system-ui, sans-serif

**Character:** One humanist sans speaks, one terminal mono proves. Roboto carries
all voice and labels at small sizes; the mono stack appears only where the computer
is the author — addresses, ports, shortcuts, codes — always with tabular numerals
(global `.tnum`, `font-variant-numeric: tabular-nums`) so live-updating readouts
never shift.

### Hierarchy
- **Bar title** (600, 18px, +0.2px): the single "Ustawienia" heading in the top bar.
- **Section label** (600, 12.5px, +1.6px tracking, uppercase, Ink 3): row headers
  "OGÓLNE / SERWER / TABLETY" — the wall's only uppercase.
- **Tile title** (600, 15px): tile names.
- **Control label** (500, 13.5px): names inside expanded controls; fact values share the size.
- **Body** (400, 12.5px, 1.55, Ink 2): tile summaries — the always-visible voice.
- **Note** (400, 12px, 1.6, Ink 3): explanatory footnotes inside details.
- **Chip** (400, 11px): status chips.
- **Data** (mono, 13px): hotkey combos, IP:port addresses, fact values.
- **Data display** (mono, 700, 30px, line-height 1.1): the listening port and the
  pairing code; the code adds +6px letter-spacing to read as digits, not words.

### Named Rules
**The Mono-Values Rule.** Every technical value — port, IP, hotkey combo, pairing
code, log path — is monospace with tabular numerals. Prose never imitates it, mono
never carries prose.

## Layout

A fixed, opaque full-screen overlay (`inset: 0`, z-index 45 — above the incumbent
light modal system at 40), entered and left on the editor's modal transition
(180ms rise in, 120ms out). A 64px top bar holds the title and the close button
(ESC does the same); below it, the wall is a centered column **max-width 1120px**
with 34px side gutters that scrolls vertically when content exceeds the window.

The wall is three labeled rows (Ogólne, Serwer, Tablety), each 30px below the last.
Tiles flex-wrap inside a row: `flex: 1 1 300px`, `min-width: 240px`, 14px gaps —
two per row on desktop, single column when narrow, no hard breakpoints. Row height
is driven by the focused tile: expansion is horizontal (flex-grow), so rows grow
organically instead of opening a fixed accordion. Inside a focused tile, control
rows stack with 1px separators; the QR panel and address list sit side by side
(14px gap); the facts list is a two-column grid (12px × 24px gaps).

## Elevation & Depth

Depth is tonal first, shadow last. Ascending surface stops (ground → tile →
tile-raised) plus 1px lines do all the structural work, and wells dip back to the
ground value; the only true elevation on the wall is reserved for the moment of
focus.

### Shadow Vocabulary
- **Focus ambient** (`box-shadow: 0 14px 40px rgba(0, 0, 0, 0.5)`): declared once,
  on the focused tile only — the wall's single elevation. It lifts the expanded tile
  off the dimming field.
- **Knob micro-shadow** (`0 1px 3px rgba(0, 0, 0, 0.4)`): part of the switch knob
  itself, not a wall token; it keeps the 20px knob readable on both track colors.

### Named Rules
**The One-Shadow Rule.** Elevation is declared exactly once in the world, on the
focused tile. No hover shadows, no stacked modal shadows, no shadow escalation —
if something new needs depth, it takes a tonal step, not a shadow.

**The Dim Wall Rule.** Depth of attention is expressed as opacity: siblings of the
focused tile sit at 0.42 and rise only to 0.62 on hover. Dead tiles (server off)
are pinned at 0.42 and cannot be hovered back.

## Shapes

Rounded but disciplined. Tiles carry the largest radius (12px); interactive controls
sit two steps down (6px buttons, inputs, kbd, address rows); content plates — QR
frame, pairing-code box — take the middle 8px; chips are full pills (999px). The
close button rounds at 10px and the switch at its own half-height (13px) with a
perfectly circular 20px knob. Every tile and technical plate is outlined with a 1px
line; controls are borderless fills except where a recessed well needs an outline
(kbd, combo input, code box). One deliberate shape exception: the QR code renders
on a flat white 108px plate (6px inner padding) — the only white surface in the
world, because scanners demand it.

## Components

Motion grammar for the whole wall: state changes run 140–250ms ease-out; focus
expansion and the switch knob use `cubic-bezier(0.2, 0, 0, 1)`; a detail reveals
with a single 200ms rise (opacity 0→1, translateY 4px→0). Nothing animates on page
load — the wall is simply there. Icons are inline Font Awesome (`fa-times`,
`fa-lock`, `fa-sync-alt`, `fa-key`) at 12–15px, never decorative color.

### Tile (signature component)
- **Anatomy:** invisible hit-zone button covering the whole tile → header (title +
  optional chip) → summary line → detail region (revealed only while focused).
- **Shape:** 12px radius, 1px line border, 18px × 20px padding, tonal `tile` fill.
- **States:** exactly one tile is `.focused` — flex-grow 2.4 over 240ms
  `cubic-bezier(0.2, 0, 0, 1)`, border becomes 40% teal mixed into line, focus
  ambient shadow, detail rises in. All others `.dim` at 0.42, hover lifting to 0.62
  with a neutral border hint (#43505c). Tiles whose backend is down are `.dead`:
  pinned at 0.42, hit-zone removed (no hit target, no fake interactivity), detail
  suppressed; their chip says so ("serwer wył.").
- **Interaction contract:** the hit-zone is a real `<button>` (aria-label
  "Rozwiń: …") so any click anywhere on a dim tile moves focus; once focused it
  steps aside (`display: none`) for the actual controls. Keyboard: dim tiles are
  tabbable, the focused tile's controls take over; the overlay traps Tab (opaque —
  focus must never wander into the editor behind it), ESC closes, `role="dialog"`
  + `aria-modal`.

### Switch
- **Shape:** 46 × 26px track, 13px radius; 20px circular knob (#f2f4f6) with micro-shadow, 3px inset.
- **States:** off = tile-raised track (hover: lift); on = teal track (hover:
  accent-deep), knob travels +20px over 160ms `cubic-bezier(0.2, 0, 0, 1)`.
  Native `role="switch"` + `aria-checked`; disabled while the backend call is in
  flight, rolling back on failure.

### Act buttons (three ranks)
- **Shape:** 6px radius, 12.5px text, 7px × 12px padding.
- **Default:** tile-raised fill, ink text; hover: lift. The workhorse ("Zmień",
  "Anuluj", "Nowy kod", "Odśwież").
- **Accent:** same body, teal text, weight 500 — the confirming step inside a
  flow ("Ustaw"). Still one per context.
- **Primary:** teal fill, near-black-teal text (#0c2a24), weight 600, slightly
  larger (9px × 14px) — the one main action of a tile ("Generuj kod parowania").
  Hover: accent-deep. Disabled: 0.45 opacity.

### Chips
- **Style:** pill (999px), 11px text, 3px × 9px padding; neutral = tile-raised fill
  + Ink 2 text; **ok** = Accent Teal text on 14% teal (the status-ok voice, matching
  the incumbent status dots); **bad** = Bad Ink text on 14% red.
- **State:** chips are read-only live status, never filters or actions. They render
  state honestly: "autostart wł./wył.", "działa / baza niedostępna", "gotowy do
  parowania / brak sieci", and the live countdown "kod żyje m:ss".

### Hotkey combo (kbd + input)
- **Display:** `<kbd>`-style plate — mono 13px, tile-raised fill, 1px line border,
  6px radius, 7px × 12px padding.
- **Edit:** recessed well input (well fill, 6px radius); focus = teal border + 1px
  teal ring (`box-shadow: 0 0 0 1px`), no glow. Validation error renders below in
  Bad Ink 12px, wrapping anywhere.

### Code display (pairing code)
- **Style:** the data-display role made physical — mono 700 30px, +6px letter-spacing,
  well fill, 1px line border, 8px radius, 14px × 18px padding. The tile's chip runs
  the green countdown; expiry burns the code back to the minting state.

### QR panel + address list
- **Pair:** flex row, 14px gap — white 108px QR plate (8px radius, 6px padding) on
  the left; vertical address list on the right.
- **Address rows:** 6px radius buttons, 13px mono tabular `ip:port` over an 11.5px
  Ink 3 interface name (ellipsis on overflow); selected and hover share the
  tile-raised fill, the selected address's mono turns 700. A default-rank
  "Odśwież" button closes the list.

### Facts list (dl)
- **Style:** two-column grid (12px × 24px gaps) of dt/dd pairs — 11.5px Ink 3
  labels ("Adres", "Klienci", "Wersja", "Logi") over 13.5px values; technical
  values in mono + tabular numerals. The read-only twin of the form row: same
  rhythm, zero controls.

### Supporting roles
Section labels (uppercase Ink 3, 12.5px/600/+1.6px), control rows (13.5px/500 name
over 12px Ink 3 note, 11px vertical padding, 1px tile-raised separators), locked
rows (lock icon + "stałe" in Ink 3 — a fact, not a disabled control), the 40px
close button (10px radius, Ink 2 → ink on hover, tile-raised hover fill, 0.96
scale on press), and 12px inline `code` chips (4px radius, tile-raised fill).

## Do's and Don'ts

### Do:
- **Do** keep exactly one tile expanded per wall; focus is exclusive (`flex-grow: 2.4`, 240ms).
- **Do** dim siblings to 0.42 (hover 0.62) and mute dead tiles at 0.42 with no hit zone.
- **Do** put every port, IP, hotkey, code and path in mono with tabular numerals (`.tnum`).
- **Do** reserve teal (#1abc9c) for the focus border tint, switch-on, one primary button, focus rings, and status-ok chips.
- **Do** use teal ok-chips and Bad Ink red as live operate-state ("działa", "serwer wył.", countdowns).
- **Do** step depth with the tonal scale (ground/well #171c21 → tile #262e36 → raised #313a44) before reaching for a shadow.
- **Do** keep transitions in the 140–250ms ease-out family and reveal a detail with one 200ms rise.
- **Do** render QR codes on the white plate — the single sanctioned white surface.

### Don't:
- **Don't** introduce a second `box-shadow` elevation; the focus ambient (0 14px 40px rgba(0,0,0,.5)) is the only one.
- **Don't** spend red as decoration, hover tints, or accent-adjacent color — red is the error channel only (as Bad Ink when small text), separate from teal by reviewed decision.
- **Don't** animate the wall on load; motion begins only as a response to interaction.
- **Don't** place real controls under the invisible hit zone, or keep a hit zone on a dead tile.
- **Don't** widen ŚCIANA into the sidebar, canvas, or the light modal system without an explicit world decision — those remain the incumbent world (light modals, teal #1abc9c, Roboto). Color is now shared with the shell; the unrepaired drift is structural: modal language, radii (4px vs 6/8/12px), and shadow vocabulary.
- **Don't** import incumbent modal idioms into the wall (uppercase flat text buttons, 4px radius, white dialogs) or wall idioms into them.
- **Don't** fake state: a chip that can't read the backend stays honest ("brak sieci"), and a dead tile never pretends to be clickable.
