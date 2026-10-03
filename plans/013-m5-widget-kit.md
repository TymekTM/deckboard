# 013 - M5 widget kit (capabilities + custom gestures)

Status: IN PROGRESS (2026-10-03)

ROADMAP M5: "knob/list/graph/interactive templates, custom gestures,
widget manifest + client capabilities negotiation". Knob/list/graph
templates already render and interact (aidev tiles, D4 live-state fix);
this plan closes what was actually missing.

## 1. Capabilities negotiation (finish the half-built wire)

`Hello.capabilities: Vec<String>` exists in proto and the Kotlin mirror,
but nobody sends it and the hub never reads it.

- Mobile sends its set with every hello: `kinds:button|toggle|slider|knob|
  graph|list`, `series`, `state.patch`, `assets`, `assets2`,
  `gestures:long-press|double-tap|swipe-left|swipe-right`.
- The hub parses the caps per session, logs them once, and `welcome`
  gains `capabilities` (server's own set, `#[serde(default)]` for
  back-compat). Old clients ignore it; old servers never send it.
- No behavior is gated on caps yet (negotiation = declared + logged +
  echoed); gating pushes on caps is a later, separately-audited change.

## 2. Custom gestures

- Proto `Interaction` gains `long-press`, `double-tap`, `swipe-left`,
  `swipe-right` (kebab-case like the rest; `Other` stays the fallback so
  old servers answer new clients with a typed error).
- A tile declares extra gestures in its options JSON:
  `{"gestures": ["long-press", "swipe-left"]}`. `widget_kind_for`
  appends the declared gestures to the derived set, so the manifest and
  the interaction gate stay consistent by construction (both flow
  through the same function).
- Server semantics: a declared gesture acks and fires the tile's action
  once (release semantics; no key-hold, no repeat - gestures are
  alternative triggers, not press modes).
- Android detects long-press/double-tap (Compose tap gestures) and
  swipes (drag + velocity/distance helper, pure + unit-tested) on tiles
  whose manifest declares them, and sends the named interaction.
- Editor UI to author `gestures` is deliberately out of scope; tiles are
  hand-authored or edited in `.boardjson` for now.

## 3. Acceptance

- `cargo test --workspace` + clippy clean; new fixtures parse on both
  sides (`hello` with caps, `welcome` with caps, a long-press
  interaction), Kotlin `ProtoFixturesTest` still green.
- Gradle unit tests green (gesture helper).
- Desktop surface untouched: no tile fields change, legacy mapper
  untouched (capabilities are v2-handshake-only by design).
