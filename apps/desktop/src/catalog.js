// Command catalog for "add tile", ported from the original renderer's
// `utils/contants/commands` (subset - every type remains editable as text).
// `fields` write into the `command` column; key "" is the command itself,
// other keys become JSON properties ({"speaker": ...}).

export const CATALOG = [
  { header: "Deckboard" },
  { value: "board", label: "Switch Board", boardSelect: true },
  { value: "multiaction", label: "Multi Actions", multiaction: true },
  { divider: true },
  { header: "General" },
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
    fields: [
      {
        key: "",
        label: "Steps JSON",
        placeholder: '[{"action":"down","value":"CTRL"}]',
      },
    ],
  },
  {
    value: "mouse-ctrl",
    label: "Mouse Control",
    icon: "mouse",
    color: "#2c3e50",
    fields: [
      { key: "", label: "Action JSON", placeholder: '{"action":"move","x":100,"y":100}' },
    ],
  },
  {
    value: "type",
    label: "Type Text",
    icon: "font",
    color: "#16a085",
    fields: [{ key: "", label: "Text", multiline: true }],
  },
  {
    value: "url",
    label: "Open URL",
    icon: "link",
    color: "#2980b9",
    fields: [{ key: "", label: "URL", placeholder: "https://..." }],
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
    fields: [{ key: "speaker", label: "Device id" }],
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
  { header: "Integrations (executed by extensions / native bridges)" },
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
  {
    value: "vm-toggle-voice",
    label: "Voicemeeter toggle (vm-*)",
    icon: "sliders-h",
    color: "#e67e22",
    dual: true,
    fields: [{ key: "", label: "Strip/bus JSON" }],
  },
];

export const MULTIACTION_STEPS = [
  { value: "delay", label: "Delay (ms)" },
  { value: "board", label: "Switch board" },
  { value: "key", label: "Keystroke" },
  { value: "type", label: "Type text" },
  { value: "url", label: "Open URL" },
];

// Dual-state types from the original button.js - state 2 styling matters.
export const DUAL_STATE_TYPES = new Set([
  "speaker-device",
  "obs-scene",
  "obs-source",
  "obs-device-audio",
  "obs-filter",
  "obs-studio-mode",
  "slobs-scene",
  "slobs-source",
  "slobs-device-audio",
  "xsplit-scene",
  "twitch-slow",
  "twitch-follow-only",
  "twitch-subs-only",
  "twitch-emote-only",
  "discord-voice-channel",
  "discord-toggle-mute",
  "discord-toggle-deaf",
  "vmod-voice",
  "vmod-hearmyself",
  "vmod-voicechanger",
  "vmod-background",
  "custom-value",
]);

// Grid geometry of the original editor: 96 px cell, 100 px row.
export const CELL_W = 96;
export const ROW_H = 100;
