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
// match pulpit_vm::GAIN_MIN / GAIN_MAX on the backend.
export const VM_GAIN_MIN = -60;
export const VM_GAIN_MAX = 12;
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

// Fallback style/config for a tile type (static catalog only; extension
// inputs are merged at runtime via list_known_inputs).
export function findTypeMeta(type) {
  return CATALOG.find((c) => c.value === type) || null;
}

// State bindings, ported from the original's buttonStyles + TOGGLE_BUTTONS
// tables: which live value decides whether a tile shows its second state.
// `watch` keys into customValues (APP_CUSTOM_VALUE pushes), `app`/`key`
// into per-app state (APP_OBS etc.). Tiles without a binding fall back to
// the tap flip.
const STATE_BINDINGS = {
  "speaker-device": { watch: "speaker-device", key: "speaker" },
  "obs-studio-mode": { app: "obs", key: "studioMode" },
  "obs-scene": { app: "obs" },
  "obs-source": { app: "obs" },
  "obs-device-audio": { app: "obs" },
  "obs-filter": { app: "obs" },
  "twitch-slow": { app: "twitch", key: "slow" },
  "twitch-follow-only": { app: "twitch", key: "followerOnly" },
  "twitch-subs-only": { app: "twitch", key: "subscriberOnly" },
  "twitch-emote-only": { app: "twitch", key: "emoteOnly" },
  "discord-voice-channel": { app: "discord", key: "channel" },
  "discord-toggle-mute": { app: "discord", key: "mute" },
  "discord-toggle-deaf": { app: "discord", key: "deaf" },
  "vmod-voice": { app: "vmod", key: "voice" },
  "vmod-hearmyself": { app: "vmod", key: "hearmyself" },
  "vmod-voicechanger": { app: "vmod", key: "voicechanger" },
};

// null = the live state is unknown (nothing pushed yet), so the tile keeps
// its current visual state; otherwise boolean. Mirrors the original
// ToggleButton isActive(): boolean state wins, arrays/strings compare
// against the command payload. `cmd` is the pre-parsed tile.command - the
// caller owns the parse (once per command change, not per call).
export function stateActive(tile, cmd, customValues, appStates, typeMeta) {
  const binding = STATE_BINDINGS[tile.type];
  let value;
  if (tile.type === "vol") {
    if (tile.command !== "vol_mute") return null;
    value = customValues["speaker-muted"];
  } else if (binding?.watch) {
    value = customValues[binding.watch];
  } else if (binding?.app) {
    const state = appStates[binding.app];
    value = binding.key ? state?.[binding.key] : state;
  } else if (typeMeta?.[tile.type]?.mode === "custom-value") {
    // extension tiles declared as custom-value follow their variable,
    // keyed by the action value (the original's toggle_key fallback)
    value = customValues[tile.type];
  } else {
    return null;
  }
  if (typeof value === "boolean") return value;
  if (value == null || value === false || value === "") return null;
  if (Array.isArray(value)) return value.includes(cmd[binding.key]);
  if (typeof value === "string") return value === cmd[binding.key];
  return Boolean(value);
}

// Grid geometry of the original editor: 96 px cell, 100 px row.
export const CELL_W = 96;
export const ROW_H = 100;

// Board width/height cap (012 C4). Mirrors `MAX_BOARD_DIM = 32` in
// pulpit-backend (import-side bounds); keep the two in sync so the editor
// can never create a board the backend would reject.
export const MAX_BOARD_DIM = 32;

// Clamp a board dimension input to the backend's integer bounds.
export function boardDim(value, fallback = 6) {
  const n = Math.trunc(Number(value));
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_BOARD_DIM, Math.max(1, n));
}
