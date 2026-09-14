// Command catalog for the tile dialog, ported from the original renderer's
// `utils/contants/commands`. Field model:
//   { key, label, placeholder? }        - text input ("" key = raw command)
//   { key, kind: "textarea" }           - multiline text
//   { key, kind: "number" }             - numeric, stored as JSON number
//   { key, kind: "select", options }    - dropdown, stored raw
//   { key, showIf: { key, value } }     - rendered only when another field has a value
// Entry extras:
//   stepEditor - the tile command is an array of steps edited as rows
//   options    - show the "program arguments" input (DB `options` column)

export const CATALOG = [
  { header: "Deckboard" },
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
  { header: "Integrations (extensions / native bridges)" },
  {
    value: "custom-value",
    label: "Variable Value (display)",
    icon: "tag",
    color: "#171A21",
    fields: [{ key: "", label: "Variable key", placeholder: "set by the Variables & Logic extension" }],
  },
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

// Grid geometry of the original editor: 96 px cell, 100 px row.
export const CELL_W = 96;
export const ROW_H = 100;
