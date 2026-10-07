// Command catalog for the tile dialog, ported from the original renderer's
// `utils/contants/commands`. Field model:
//   { key, label, placeholder? }        - text input ("" key = raw command)
//   { key, kind: "textarea" }           - multiline text
//   { key, kind: "number" }             - numeric, stored as JSON number
//   { key, kind: "select", options }    - dropdown, stored raw; option
//                                         values may be numbers and are
//                                         then stored as JSON numbers
//   { key, showIf: { key, value } }     - rendered only when another field has a value
// Entry extras:
//   stepEditor - the tile command is an array of steps edited as rows
//   options    - show the "program arguments" input (DB `options` column)

// Voicemeeter parameter choices, mirroring the original voicemeeter-control
// extension's INPUTS. Index values stay numbers: the backend reads them with
// as_i64, a string would fail the tile press.
const vmSelect = (values) => values.map((v) => ({ value: v, label: String(v) }));
const VM_INDEX = vmSelect([0, 1, 2, 3, 4, 5, 6, 7]);
const VM_SET_STRIP_PARAMS = vmSelect([
  "Mono", "Mute", "Solo", "MC", "Gain", "Pan_x", "Pan_y", "Color_x", "Color_y",
  "fx_x", "fx_y", "Audibility", "Comp", "Gate", "EqGain1", "EqGain2", "EqGain3",
  "Label", "A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "FadeTo",
  "Reverb", "Delay", "Fx1", "Fx2", "PostReverb", "PostDelay", "PostFx1", "PostFx2",
]);
const VM_TOGGLE_STRIP_PARAMS = vmSelect(["Mono", "Mute", "Solo", "A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3"]);
const VM_SLIDER_STRIP_PARAMS = vmSelect(["Gain", "Comp", "Gate"]);
const VM_SET_BUS_PARAMS = vmSelect([
  "Mono", "Mute", "EQ.on", "Gain", "mode.normal", "mode.Amix", "mode.Bmix",
  "mode.Repeat", "mode.Composite", "FadeTo", "Label",
  "Sel", "ReturnReverb", "ReturnDelay", "ReturnFx1", "ReturnFx2",
]);
const VM_TOGGLE_BUS_PARAMS = vmSelect(["Mono", "Mute", "EQ.on"]);
const VM_SLIDER_BUS_PARAMS = vmSelect(["Gain"]);
// Bus indices map straight onto Voicemeeter's A/B output channels
// (A1..A5, then B1..B3 on Potato); the numbers stay the stored value
const VM_BUS_INDEX = [
  "0 (A1)", "1 (A2)", "2 (A3)", "3 (A4)", "4 (A5)", "5 (B1)", "6 (B2)", "7 (B3)",
].map((label, i) => ({ value: i, label }));

// Voicemeeter gain fader range in dB; vm sliders map 0..1 onto it. Must
// match pulpit_vm::GAIN_MIN / GAIN_MAX on the backend. Only the reset
// position below leaves this file.
const VM_GAIN_MIN = -60;
const VM_GAIN_MAX = 12;
// double-tap reset position: 0 dB unity gain on the fader
export const VM_SLIDER_RESET = (0 - VM_GAIN_MIN) / (VM_GAIN_MAX - VM_GAIN_MIN);

export const CATALOG = [
  { header: "Pulpit" },
  {
    value: "board",
    label: "Switch Board",
    boardSelect: true,
    icon: "th",
    color: "#2c3e50",
  },
  {
    value: "multiaction",
    label: "Multi Actions",
    icon: "th-large",
    color: "#2c3e50",
    stepEditor: {
      addDefaults: { type: "delay", command: "100" },
      types: [
        { value: "delay", label: "Delay (ms)" },
        { value: "board", label: "Switch board", board: true },
        { value: "key", label: "Keystroke" },
        { value: "type", label: "Type text" },
        { value: "url", label: "Open URL" },
      ],
    },
  },
  { divider: true },
  { header: "Keyboard & Mouse" },
  {
    value: "key",
    label: "Keyboard Macro",
    icon: "keyboard",
    color: "#2c3e50",
    fields: [{ key: "", label: "Keystroke", placeholder: "CTRL + SHIFT + K" }],
  },
  {
    value: "advance-key",
    label: "Advance Keyboard Macro",
    icon: "keyboard",
    color: "#2c3e50",
    stepEditor: {
      addDefaults: { type: "down", command: "" },
      types: [
        { value: "down", label: "Key down" },
        { value: "up", label: "Key up" },
        { value: "delay", label: "Delay (ms)", number: true },
        { value: "type", label: "Type text" },
      ],
    },
  },
  {
    value: "mouse-ctrl",
    label: "Mouse Control",
    icon: "mouse",
    color: "#2c3e50",
    fields: [
      {
        key: "action",
        kind: "select",
        label: "Action",
        options: [
          { value: "move", label: "Move cursor" },
          { value: "lclick", label: "Left click" },
          { value: "rclick", label: "Right click" },
        ],
      },
      { key: "x", kind: "number", label: "X", showIf: { key: "action", value: "move" } },
      { key: "y", kind: "number", label: "Y", showIf: { key: "action", value: "move" } },
    ],
  },
  {
    value: "type",
    label: "Type Text",
    icon: "font",
    color: "#16a085",
    fields: [{ key: "", label: "Text", kind: "textarea" }],
  },
  { divider: true },
  { header: "System" },
  {
    value: "url",
    label: "Open URL",
    icon: "link",
    color: "#2980b9",
    fields: [{ key: "", label: "URL", placeholder: "https://..." }],
  },
  {
    // native replacement of the deckboard-callurl extension; the backend
    // fires a plain GET on urlToCall. commandAction is not executed but
    // kept in the JSON so hand-added payloads survive an edit.
    value: "url-to-call",
    label: "URL to Call",
    icon: "link",
    color: "#ff29df",
    fields: [
      { key: "urlToCall", label: "URL to call", placeholder: "http://..." },
      { key: "commandAction", label: "Command action", kind: "textarea", placeholder: "optional, stored with the tile" },
    ],
  },
  {
    value: "dir",
    label: "Open Folder",
    icon: "folder",
    color: "#f39c12",
    fields: [{ key: "", label: "Directory path" }],
  },
  {
    value: "app",
    label: "Run Program",
    icon: "cog",
    color: "#7f8c8d",
    fields: [{ key: "", label: "Executable path" }],
    options: true,
  },
  {
    value: "file",
    label: "Open File",
    icon: "file",
    color: "#7f8c8d",
    fields: [{ key: "", label: "File path" }],
  },
  {
    value: "screenshot",
    label: "Capture Screenshot",
    icon: "camera",
    color: "#8e44ad",
    fields: [{ key: "", label: "Save to folder" }],
  },
  { divider: true },
  { header: "Multimedia" },
  {
    value: "vol",
    label: "Multimedia",
    init: "play",
    icon: "play",
    color: "#27ae60",
    select: [
      { value: "play", label: "Play / Pause" },
      { value: "prev", label: "Prev" },
      { value: "next", label: "Next" },
      { value: "vol_up", label: "Increase Volume" },
      { value: "vol_down", label: "Decrease Volume" },
      { value: "vol_mute", label: "Mute" },
    ],
  },
  {
    value: "speaker-device",
    label: "Set Audio Device",
    icon: "volume-up",
    color: "#27ae60",
    dual: true,
    fields: [{ key: "speaker", label: "Device", devices: "audio" }],
  },
  { value: "speaker-volume", label: "Volume Control", mode: "slider", icon: "sliders-h", color: "#27ae60" },
  {
    value: "play",
    label: "Play Audio",
    icon: "play-circle",
    color: "#27ae60",
    fields: [{ key: "", label: "Audio file path" }],
  },
  { divider: true },
  { header: "Voicemeeter" },
  // native pulpit_vm actions; the JS extension that declared these
  // fields fails to load in the embedded runtime, so they are declared here
  {
    value: "vm-set-strip",
    label: "Set Strip Parameter",
    icon: "headphones",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SET_STRIP_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-toggle-strip",
    label: "Toggle Strip Parameter",
    icon: "microphone-slash",
    color: "#171A21",
    dual: true,
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_TOGGLE_STRIP_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_INDEX },
    ],
  },
  {
    value: "vm-increase-strip",
    label: "Increase Strip Parameter",
    icon: "volume-up",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_STRIP_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-decrease-strip",
    label: "Decrease Strip Parameter",
    icon: "volume-down",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_STRIP_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-set-bus",
    label: "Set Bus Parameter",
    icon: "headphones",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SET_BUS_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_BUS_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-toggle-bus",
    label: "Toggle Bus Parameter",
    icon: "volume-mute",
    color: "#171A21",
    dual: true,
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_TOGGLE_BUS_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_BUS_INDEX },
    ],
  },
  {
    value: "vm-increase-bus",
    label: "Increase Bus Parameter",
    icon: "volume-up",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_BUS_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_BUS_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-decrease-bus",
    label: "Decrease Bus Parameter",
    icon: "volume-down",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_BUS_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_BUS_INDEX },
      { key: "value", label: "Value" },
    ],
  },
  {
    value: "vm-slider-bus",
    label: "Volume Slider (Bus)",
    mode: "slider",
    icon: "sliders-h",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_BUS_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_BUS_INDEX },
    ],
  },
  {
    value: "vm-slider-strip",
    label: "Volume Slider (Strip)",
    mode: "slider",
    icon: "sliders-h",
    color: "#171A21",
    fields: [
      { key: "param", kind: "select", label: "Parameter", options: VM_SLIDER_STRIP_PARAMS },
      { key: "number", kind: "select", label: "Index", options: VM_INDEX },
    ],
  },
  {
    value: "vm-restart",
    label: "Restart Audio Engine",
    icon: "sync",
    color: "#171A21",
  },
  { divider: true },
  { header: "OBS Studio" },
  { value: "obs-scene", label: "Switch Scene", icon: "video", color: "#2980b9", dual: true,
    fields: [{ key: "scene", label: "Scene name" }] },
  { value: "obs-source", label: "Toggle Source", icon: "photo-video", color: "#2980b9", dual: true,
    fields: [{ key: "source", label: "Source name" }] },
  { value: "obs-device-audio", label: "Toggle Audio Source", icon: "volume-up", color: "#2980b9", dual: true,
    fields: [{ key: "device", label: "Source name" }] },
  { value: "obs-filter", label: "Toggle Filter", icon: "filter", color: "#2980b9", dual: true,
    fields: [{ key: "filter", label: "Filter name" }] },
  { value: "obs-studio-mode", label: "Toggle Studio Mode", icon: "columns", color: "#2980b9", dual: true },
  { divider: true },
  { header: "Streamlabs & XSplit" },
  { value: "slobs-scene", label: "SLOBS: Switch Scene", icon: "video", color: "#3070b0", dual: true,
    fields: [{ key: "scene", label: "Scene name" }] },
  { value: "slobs-source", label: "SLOBS: Toggle Source", icon: "photo-video", color: "#3070b0", dual: true,
    fields: [{ key: "source", label: "Source name" }] },
  { value: "slobs-device-audio", label: "SLOBS: Toggle Audio Source", icon: "volume-up", color: "#3070b0", dual: true,
    fields: [{ key: "device", label: "Source name" }] },
  { value: "xsplit-scene", label: "XSplit: Switch Scene", icon: "video", color: "#2d7dd2", dual: true,
    fields: [{ key: "scene", label: "Scene name" }] },
  { divider: true },
  { header: "Twitch" },
  { value: "twitch-slow", label: "Slow Mode", icon: "hourglass-half", color: "#9146ff", dual: true },
  { value: "twitch-follow-only", label: "Followers-Only Mode", icon: "user-plus", color: "#9146ff", dual: true },
  { value: "twitch-subs-only", label: "Subs-Only Mode", icon: "star", color: "#9146ff", dual: true },
  { value: "twitch-emote-only", label: "Emote-Only Mode", icon: "smile", color: "#9146ff", dual: true },
  { divider: true },
  { header: "AI dev work" },
  {
    value: "ai-plan-limits",
    label: "AI Plan Limits (display)",
    icon: "tachometer-alt",
    color: "#171A21",
    mode: "status",
  },
  {
    value: "ai-agent-status",
    label: "Agent Progress (display)",
    icon: "robot",
    color: "#171A21",
    mode: "status",
  },
  {
    value: "ai-tokens-today",
    label: "AI Tokens Today (graph)",
    icon: "coins",
    color: "#171A21",
    mode: "graph",
  },
  {
    value: "ai-tokens-hour",
    label: "AI Tokens Per Hour (graph)",
    icon: "clock",
    color: "#171A21",
    mode: "graph",
  },
  { divider: true },
  { header: "Spotify" },
  // native Spotify integration (design §3); styling/icon fallbacks come
  // from pulpit_spotify::input_declarations via list_known_inputs, these
  // rows make the fields editable in the dialog
  {
    value: "spotify-playback",
    label: "Playback (Play / Next / Vol)",
    icon: "play",
    color: "#1DB954",
    select: [
      { value: "play", label: "Play / Pause" },
      { value: "next", label: "Next" },
      { value: "prev", label: "Previous" },
      { value: "vol_up", label: "Increase Volume" },
      { value: "vol_down", label: "Decrease Volume" },
      { value: "vol_mute", label: "Mute" },
    ],
  },
  {
    value: "spotify-shuffle",
    label: "Shuffle",
    icon: "random",
    color: "#1DB954",
    dual: true,
  },
  {
    value: "spotify-repeat",
    label: "Repeat",
    icon: "repeat",
    color: "#1DB954",
    dual: true,
  },
  {
    value: "spotify-like",
    label: "Like Current Track",
    icon: "heart",
    color: "#1DB954",
    dual: true,
  },
  {
    value: "spotify-add",
    label: "Add Track to Playlist",
    icon: "plus",
    color: "#1DB954",
    fields: [
      { key: "playlist", label: "Playlist", devices: "spotify-playlists", placeholder: "playlist id or spotify:playlist:..." },
    ],
  },
  {
    value: "spotify-tracks",
    label: "Play Playlist / Album / Track",
    icon: "record-vinyl",
    color: "#1DB954",
    fields: [
      { key: "uri", label: "URI", devices: "spotify-playlists", placeholder: "spotify:playlist:... (also album/track/artist)" },
    ],
  },
  {
    value: "spotify-device",
    label: "Transfer Playback to Device",
    icon: "tv",
    color: "#1DB954",
    fields: [
      { key: "device", label: "Device", devices: "spotify", placeholder: "device name" },
    ],
  },
  {
    value: "spotify-volume",
    label: "Volume Control",
    mode: "slider",
    icon: "volume-up",
    color: "#1DB954",
  },
  {
    value: "spotify-seek",
    label: "Seek",
    mode: "slider",
    icon: "clock",
    color: "#1DB954",
  },
  {
    value: "spotify-now-playing",
    label: "Now Playing (display)",
    mode: "status",
    icon: "music",
    color: "#1DB954",
  },
  { divider: true },
  { header: "Multimedia (system)" },
  // natywna integracja SMTC (Windows global media transport): działa dla
  // dowolnego odtwarzacza widocznego w systemie (przeglądarka, Tidal,
  // VLC...), nie tylko przez Web API Spotify. Opcjonalny cel "Aplikacja"
  // edytuje EditTileModal (options column), nie pola command.
  {
    value: "media-now-playing",
    label: "Teraz odtwarzane (wyświetlanie)",
    mode: "status",
    icon: "music",
    color: "#8E44AD",
  },
  {
    value: "media-control",
    label: "Sterowanie multimediami",
    icon: "play",
    color: "#8E44AD",
    select: [
      { value: "play-pause", label: "Play / Pause" },
      { value: "next", label: "Next" },
      { value: "previous", label: "Previous" },
      { value: "stop", label: "Stop" },
    ],
  },
  {
    value: "media-seek",
    label: "Przewijanie utworu",
    mode: "slider",
    icon: "clock",
    color: "#8E44AD",
  },
  { divider: true },
  { header: "Variables & Logic" },
  {
    value: "custom-value",
    label: "Variable Value (display)",
    icon: "tag",
    color: "#171A21",
    fields: [{ key: "", label: "Variable key", placeholder: "set by the Variables & Logic extension" }],
  },
  { divider: true },
  { header: "Discord" },
  {
    value: "discord-voice-channel",
    label: "Discord: Join Voice Channel",
    icon: "headphones",
    color: "#5865f2",
    dual: true,
    fields: [{ key: "channel", label: "Channel id" }],
  },
  {
    value: "discord-toggle-mute",
    label: "Discord: Toggle Mute",
    icon: "microphone-slash",
    color: "#5865f2",
    dual: true,
  },
  {
    value: "discord-toggle-deaf",
    label: "Discord: Toggle Deaf",
    icon: "headphones",
    color: "#5865f2",
    dual: true,
  },
];

// State bindings, ported from the original's buttonStyles + TOGGLE_BUTTONS
// tables (crates/legacy/assets/buttonprops.json is the authoritative copy
// of the toggle keys): which live value decides whether a tile shows its
// second state. `watch` keys into customValues (APP_CUSTOM_VALUE pushes),
// optional `cmd` names the command-JSON field compared against the pushed
// value (defaults to `key`).
// The original's per-app state lane (APP_OBS/APP_TWITCH/APP_VMOD/...
// events feeding `app`-scoped bindings) is deliberately gone: no host
// ever emits those events - extensions, sysinfo, aidev and discord all
// push per-key custom values, the `watch` style - so every `app` binding
// evaluated against an empty map and its dual tiles only ever used the
// session tap-flip (DESK-09). check-catalog.mjs rejects `app` bindings
// so the lane does not sneak back without an emitter.
// Every dual-flagged catalog entry MUST have a binding here - the empty
// binding is a conscious "tap flip until the integration pushes state"
// decision, and scripts/check-catalog.mjs enforces the pairing.
export const STATE_BINDINGS = {
  "speaker-device": { watch: "speaker-device", key: "speaker" },
  // Spotify live toggles: the poller pushes ON/OFF strings under the
  // design-§4 keys (repeat cycles off/context/track, its ON/OFF label
  // rides the derived spotify-repeat-on key)
  "spotify-shuffle": { watch: "spotify-shuffle" },
  "spotify-repeat": { watch: "spotify-repeat-on" },
  "spotify-like": { watch: "spotify-liked" },
  // obs/slobs/xsplit/twitch/discord integrations have no live push in
  // Pulpit (the JS-app extensions that owned the app-state lane are
  // gone; exec pushes at most a per-key custom value): their dual tiles
  // fall back to the session tap flip, like the vm toggles below.
  "obs-studio-mode": {},
  "obs-scene": {},
  "obs-source": {},
  "obs-device-audio": {},
  "obs-filter": {},
  "slobs-scene": {},
  "slobs-source": {},
  "slobs-device-audio": {},
  "xsplit-scene": {},
  "twitch-slow": {},
  "twitch-follow-only": {},
  "twitch-subs-only": {},
  "twitch-emote-only": {},
  "discord-voice-channel": {},
  "discord-toggle-mute": {},
  "discord-toggle-deaf": {},
  // vmod-* tiles arrive as runtime extension inputs, not catalog rows;
  // same tap-flip decision
  "vmod-voice": {},
  "vmod-hearmyself": {},
  "vmod-voicechanger": {},
  // Voicemeeter strip/bus parameter toggles have no live push yet
  // (pulpit_vm is fire-and-forget), so the tap flip decides until a
  // state source exists - an empty binding records that decision.
  "vm-toggle-strip": {},
  "vm-toggle-bus": {},
};

// null = the live state is unknown (nothing pushed yet), so the tile keeps
// its current visual state; otherwise boolean. Mirrors the original
// ToggleButton isActive(): boolean state wins, arrays/strings compare
// against the command payload when the binding names a field. Bindings
// without a comparison field mean the pushed value itself carries the
// state and follow the mobile isActiveValue (MOB-10): "ON"/"1" light the
// second state, "OFF"/"0"/"" and every other push read inactive.
// `cmd` is the pre-parsed tile.command - the caller owns the parse (once
// per command change, not per call).
export function stateActive(tile, cmd, customValues, typeMeta) {
  const binding = STATE_BINDINGS[tile.type];
  let value;
  if (tile.type === "vol") {
    if (tile.command !== "vol_mute") return null;
    value = customValues["speaker-muted"];
  } else if (tile.type === "spotify-playback") {
    // command-scoped like vol_mute: only the play/pause half of the
    // select tracks the pushed playing state; next/prev/vol_* never do
    if (tile.command !== "play") return null;
    value = customValues["spotify-playing"];
  } else if (binding?.watch) {
    value = customValues[binding.watch];
  } else if (typeMeta?.[tile.type]?.mode === "custom-value") {
    // extension tiles declared as custom-value follow their variable,
    // keyed by the action value (the original's toggle_key fallback)
    value = customValues[tile.type];
  } else {
    return null;
  }
  // the command field to compare lives under `cmd` when the state key
  // differs from it (speaker-device: watched device id vs command
  // {speaker: ...})
  const cmdValue = cmd[binding?.cmd || binding?.key];
  if (typeof value === "boolean") return value;
  if (value == null) return null;
  if (cmdValue !== undefined) {
    // bound comparison: arrays/strings compare against the command
    // payload, an emptied value reads unknown
    if (value === false || value === "") return null;
    if (Array.isArray(value)) return value.includes(cmdValue);
    if (typeof value === "string") return value === cmdValue;
    return Boolean(value);
  }
  // unbound (vol_mute, custom-value toggles): same push must light the
  // tile on both surfaces, so mirror the mobile isActiveValue - only
  // the strings "ON"/"1" count as active. Today's producers push
  // booleans (speaker-muted) and ON/OFF strings (discord's
  // _labelMuteDeaf); numbers/objects never represent toggle state.
  if (typeof value === "string") return value === "ON" || value === "1";
  return false;
}

// Grid geometry of the original editor: 96 px cell, 100 px row.
export const CELL_W = 96;
export const ROW_H = 100;

// Board width/height cap (012 C4). Matches the backend's
// MAX_BOARD_DIM = 32 bound on board dimensions (the Rust import-side
// bounds live in the backend crate); keep the two in sync so the editor
// can never create a board the backend would reject.
export const MAX_BOARD_DIM = 32;

// Clamp a board dimension input to the backend's integer bounds; the
// fallback (the form's own default for that field) applies only when the
// input is not a number at all.
export function boardDim(value, fallback) {
  const n = Math.trunc(Number(value));
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_BOARD_DIM, Math.max(1, n));
}

// Clamp to inclusive bounds. Used to live as private copies in App.vue
// and GridEditor.vue; one home here so the bounds logic cannot drift
// (DESK-10).
export function clamp(v, min, max) {
  return Math.min(max, Math.max(min, v));
}

// ---- status-payload playback progress (spotify-now-playing et al) -----------

// The displayed playback position in ms, extrapolated client-side (design
// §4): the payload carries no server timestamp because clocks differ, so
// the receiver stamps its local arrival time and this advances
// position_ms by the elapsed wall time while `playing` is true - clamped
// to the track - and freezes at the reported position when paused.
// Returns null when the payload carries no usable progress.
export function statusProgressAt(progress, receivedAtMs, nowMs) {
  if (!progress || typeof progress !== "object") return null;
  const position = Number(progress.position_ms);
  const duration = Number(progress.duration_ms);
  if (!Number.isFinite(position) || !Number.isFinite(duration) || duration <= 0) {
    return null;
  }
  const base = clamp(position, 0, duration);
  if (progress.playing !== true) return base;
  const elapsed = Math.max(0, nowMs - receivedAtMs);
  return Math.min(duration, base + elapsed);
}

// m:ss without hours - playback clocks stay under an hour in practice
// and the text has to fit a tile row.
export function mmss(ms) {
  const total = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

// ---- plan usage windows (ai-plan-limits tiles) ------------------------------

// Which usage windows a plan tile renders, stored in its options column as
// a "windows:5h,week" token (no token = both windows). The edit dialog
// writes the token (checkbox rows) and the status tile reads it (row
// filter), so both sides parse through this one function - the regex and
// the want-list used to be duplicated in EditTileModal.vue and
// TileCell.vue and could drift silently (DESK-10).
export function parsePlanWindows(options) {
  const match = String(options || "").match(/(?:^|;)windows:([^;]*)/);
  const want = match
    ? match[1].split(",").map((s) => s.trim()).filter(Boolean)
    : null;
  return {
    five: !want || want.includes("5h"),
    week: !want || want.includes("week"),
  };
}

// Rewrite (or append) the windows token in an options string, preserving
// any sibling tokens: "flag:x;windows:5h" + {week only} -> "flag:x;windows:week".
export function setPlanWindows(options, windows) {
  const rest = String(options || "")
    .replace(/(^|;)windows:[^;]*/g, "")
    .replace(/^;+|;+$/g, "")
    .replace(/;;+/g, ";");
  const parts = [];
  if (windows.five) parts.push("5h");
  if (windows.week) parts.push("week");
  const token = `windows:${parts.join(",")}`;
  return rest ? `${rest};${token}` : token;
}
