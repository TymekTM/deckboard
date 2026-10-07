<script setup>
import { computed, ref, watchEffect } from "vue";
import {
  stateActive,
  VM_SLIDER_RESET,
  parsePlanWindows,
  statusProgressAt,
  mmss,
} from "../catalog";
import { resolveStatusArt, payloadReceivedAt, windowVisible } from "../statusMedia";
import ToolTile from "./tiles/ToolTile.vue";

// One tile. Isolated so a live state push re-renders only the tiles that
// read the pushed key, not the whole board. Geometry (grid position,
// size, drag offset) stays in GridEditor and falls through onto the root
// element via class/style attrs; interaction events bubble up as emits.
const props = defineProps({
  tile: { type: Object, required: true },
  touch: { type: Boolean, default: false },
  // type -> {icon, color, mode, dual} fallbacks from the action catalog
  typeMeta: { type: Object, default: () => ({}) },
  // live state pushes (APP_CUSTOM_VALUE), see applyStatusUpdate
  customValues: { type: Object, default: () => ({}) },
  // board id -> name, for board-switch tiles without a title
  boardNames: { type: Object, default: () => ({}) },
  // editor-preview dual-state flip from the parent's session Set
  active: { type: Boolean, default: false },
  dragging: { type: Boolean, default: false },
});
const emit = defineEmits(["open", "ctx", "down", "resize", "tap", "slider"]);

function metaOf(tile) {
  return props.typeMeta?.[tile.type] || {};
}

// one parse per command change; helpers below never re-parse it
const cmd = computed(() => {
  try {
    return JSON.parse(props.tile.command) || {};
  } catch {
    return {};
  }
});

// ---- utility tools (round 5) -----------------------------------------------
// Compact server state arrives under `tool-<button id>`; time is
// extrapolated inside ToolTile. While a legacy stock client is
// connected, the 1 Hz lane pushes a plain formatted label string under
// the same key - the last object view is kept so the tile does not
// flicker between the rich and the text-only rendering.
const isTool = computed(() =>
  String(props.tile.type || "").startsWith("tool-"),
);
const toolState = ref(null);
watchEffect(() => {
  if (!isTool.value) {
    toolState.value = null;
    return;
  }
  const v = props.customValues[`tool-${props.tile.id}`];
  if (v && typeof v === "object") toolState.value = v;
});

// Whether the tile renders its second state. A known live state wins
// (original ToggleButton isActive); otherwise the session tap flip.
const activeState = computed(() => {
  const state = stateActive(props.tile, cmd.value, props.customValues, props.typeMeta);
  return state !== null ? state : props.active;
});

// second-state fields only apply when they are actually set (original:
// `(isActive ? data.color2 : data.color) || style.color`)
function pick(first, second) {
  return activeState.value ? props.tile[second] || props.tile[first] : props.tile[first];
}

function tileBg(tile) {
  return pick("color", "color2") || metaOf(tile).color || "#2c3e50";
}

// the original styles each vol variant with its own icon
const VOL_ICONS = {
  play: "play",
  prev: "fast-backward",
  next: "fast-forward",
  vol_up: "volume-up",
  vol_down: "volume-down",
  vol_mute: "volume-off",
};

function tileIcon(tile) {
  if (tile.type === "vol" && VOL_ICONS[tile.command]) return VOL_ICONS[tile.command];
  return pick("icon", "icon2") || metaOf(tile).icon || "";
}
function tileIconColor() {
  return pick("icon_color", "icon_color2") || "#ffffff";
}
function tileShape() {
  const shape = pick("shape", "shape2");
  return shape === 1 ? "50%" : "8px";
}
function tileBorder() {
  return pick("border_color", "border_color2") || "transparent";
}
function tileTitlePos() {
  return pick("title_position", "title_position2") ?? 0;
}
// State-2 fallback rule (docs/protocol-v2.md §4): while active, each
// state-2 field falls back to its state-1 counterpart per field - the
// same rule the v2/Android clients apply to the optional style fields.
function tileTitleColor() {
  return pick("title_color", "title_color2");
}
function tileTitleBox() {
  return pick("title_box_color", "title_box_color2");
}

// ---- live-value tiles (custom-value / graph / board), like the stock client

// CustomValueButton: label = customValues[command || type], last sample of
// graph-style objects. Shown when the tile has no title of its own.
const customValueLabel = computed(() => {
  const v = props.customValues[props.tile.command || props.tile.type];
  if (v == null || typeof v === "object") return "";
  return String(v);
});

// GraphButton: the extension pushes {values, title?, suffix?, description?}
// under the tile's command (or type), like custom-value labels
const graphData = computed(() => {
  if (props.tile.mode !== "graph") return null;
  const v = props.customValues[props.tile.command || props.tile.type];
  if (!v || typeof v !== "object" || !Array.isArray(v.values) || !v.values.length) {
    return null;
  }
  return v;
});

// the hour tile renders either the shared sparkline or, when tapped, the
// per-provider rows the producer ships alongside the graph value; the
// toggle lives for this component's lifetime only
const hourExpanded = ref(false);
function hourShowRows() {
  return hourExpanded.value && graphData.value?.rows?.length;
}

function graphLast() {
  const g = graphData.value;
  if (g.value_label) return g.value_label;
  const last = g.values[g.values.length - 1];
  return `${last ?? ""}${g.suffix || ""}`;
}

// StatusButton: the producer pushes {title, rows:[{label, value, state,
// percent?, provider?}], compact:[{provider, value}], summary} under the
// tile's command (or type) - a live list that replaces itself wholesale
// (see mergeCustomValues). Rows arrive display-ready so this stays a dumb
// renderer.
const statusData = computed(() => {
  if (props.tile.mode !== "status") return null;
  const v = props.customValues[props.tile.command || props.tile.type];
  if (!v || typeof v !== "object" || !Array.isArray(v.rows) || !v.rows.length) {
    return null;
  }
  return v;
});

// When the tile cannot fit the detail (small tile, or more rows than the
// height can hold) the producer's compact per-provider counts take over:
// a vertical stack of logos, each with a dot and the number of active or
// waiting sessions below it.
const statusCompact = computed(() => {
  const data = statusData.value;
  if (!data) return [];
  const compact = data.compact;
  if (!Array.isArray(compact) || !compact.length) return [];
  if (props.tile.h <= 1 || data.rows.length > props.tile.h * 4) return compact;
  return [];
});

// Per-tile editor option stored in the tile's options column as
// "windows:5h,week" (missing token = both): percent rows of windows the
// user unticked drop out; agent rows carry no percent, never touched.
// The token parses through the shared catalog helper (DESK-10) - the
// edit dialog's checkboxes write the same format - and only once per
// options change instead of once per row.
const planWant = computed(() => parsePlanWindows(props.tile.options));
function planWindowHidden(label) {
  if (typeof label !== "string") return false;
  const want = planWant.value;
  if (label.endsWith(" 5h")) return !want.five;
  if (label.endsWith(" week")) return !want.week;
  return false;
}

const statusRows = computed(() => {
  const data = statusData.value;
  if (!data) return [];
  return data.rows.filter((row) => !planWindowHidden(row.label));
});

// Row identifier style from the producer config: "name" (default) keeps
// the text label, "logo" swaps it for the provider mark (falling back to
// the name when no mark exists) - never both at once.
const statusLogoRows = computed(
  () => (statusData.value?.row_style ?? "name") === "logo",
);

// The brand mark to draw instead of the label in logo style: agent rows
// carry `provider`, plan rows derive it from the label ("GLM 5h" ->
// zcode). Null when the row is in name style, is a header, or no mark
// exists - the label renders instead, so text stays the only identifier
// in name mode.
function statusGlyph(row) {
  if (!statusLogoRows.value || row.state === "header") return null;
  return providerSvg(row.provider || laneProvider(row.label));
}

// Plan rows are named "<provider> <window>" ("GLM 5h") - the lane label
// maps back to the provider for the mini view's glyph.
function laneProvider(label) {
  const first = String(label || "").split(" ")[0].toLowerCase();
  return first === "glm" ? "zcode" : first;
}

// The backend summary names the worst window, which a filtered-out row
// may no longer be: recompute from the visible percent rows when the
// tile has any, else keep the producer's line. The AI-usage settings
// can turn the line off entirely (`hide_summary` from the producer).
const statusSummary = computed(() => {
  const data = statusData.value;
  if (!data) return "";
  if (data.hide_summary) return "";
  const percentRows = data.rows.filter(
    (row) => row.percent != null && !planWindowHidden(row.label),
  );
  if (!percentRows.length) return data.summary || "";
  const worst = percentRows.reduce((a, b) => (b.percent > a.percent ? b : a));
  return `${worst.label} ${Math.round(worst.percent)}%`;
});

// 1x1 plan tiles switch to the mini usage view: one bar + percentage per
// visible window, provider glyph as the only label. Detail rows cannot
// fit that size without truncating into noise.
const statusMini = computed(() => {
  const data = statusData.value;
  if (!data || props.tile.w > 1 || props.tile.h > 1) return [];
  const rows = data.rows.filter(
    (row) => row.percent != null && !planWindowHidden(row.label),
  );
  return rows.length ? rows : [];
});

// ---- album art + playback progress on status payloads (design §4) ----------
// Generic status-payload extensions: any producer's push that carries
// `image` (an asset hash) or `progress` gets them rendered, not only
// spotify-now-playing. Old payloads without the fields render exactly as
// before.

// Album art, resolved per hash through the asset_data_url Tauri command
// and memoized in the WebView (see statusMedia.js). A dropped lookup
// leaves the url null - the tile renders text-only.
const statusArtUrl = ref(null);
watchEffect((onCleanup) => {
  const hash = statusData.value?.image;
  let live = true;
  onCleanup(() => {
    live = false;
  });
  resolveStatusArt(hash, (url) => {
    if (live) statusArtUrl.value = url;
  });
});

// The bar ticks ~1 Hz, but only while the payload says playing AND the
// window is on screen: hiding the window tears the interval down through
// windowVisible, pausing (or a payload without progress) never starts
// one, and unmounting the tile disposes the effect. nowTick starting at
// Date.now() also covers the payload-change frame - no stale position.
const nowTick = ref(Date.now());
watchEffect((onCleanup) => {
  if (statusData.value?.progress?.playing !== true || !windowVisible.value) {
    return;
  }
  nowTick.value = Date.now();
  const id = setInterval(() => {
    nowTick.value = Date.now();
  }, 1000);
  onCleanup(() => clearInterval(id));
});

// Reported position extrapolated by the local time since the payload
// arrived, clamped to the track (statusProgressAt); frozen while paused
// because then no tick runs and nowTick stays put.
const progressView = computed(() => {
  const data = statusData.value;
  const progress = data?.progress;
  if (!progress) return null;
  const at = statusProgressAt(progress, payloadReceivedAt(data), nowTick.value);
  if (at == null) return null;
  const duration = Number(progress.duration_ms);
  return {
    percent: Math.min(100, Math.max(0, (at / duration) * 100)),
    label: `${mmss(at)} / ${mmss(duration)}`,
  };
});

// Media payloads (album art and/or playback progress) get a card layout
// instead of the label/value rows: the producer's Track / Artist / Album
// rows become a stacked title-subtitle block, the duplicated summary
// drops out, and remaining rows (Device, ...) only render when the tile
// is wide enough to have room. Matched by label name so any producer's
// push benefits, not only spotify-now-playing.
const mediaView = computed(() => {
  if (!statusArtUrl.value && !progressView.value) return null;
  const rows = statusRows.value;
  if (!rows.length) return null;
  const byLabel = (name) =>
    rows.find((r) => String(r.label || "").toLowerCase() === name);
  const track = byLabel("track");
  const rest = rows.filter(
    (r) =>
      r !== track &&
      !["track", "artist", "album"].includes(
        String(r.label || "").toLowerCase(),
      ),
  );
  return {
    title: track?.value || "",
    artist: byLabel("artist")?.value || "",
    album: byLabel("album")?.value || "",
    rest,
  };
});
// Album and the producer's extra rows need room for four-plus text
// lines: they render only when BOTH dimensions have two cells. A
// one-cell-high tile keeps the title, the artist and the bar - more
// lines truncate into noise. Extras collapse to one dim "Label: value"
// line each. Mirrored in the Compose StatusMediaText.
const showMediaMeta = computed(() => props.tile.w >= 2 && props.tile.h >= 2);
const mediaTitleLines = computed(() => (props.tile.h >= 2 ? 2 : 1));

// Portrait/square tiles stack the art above the text; wide tiles put the
// art beside it. The m:ss / m:ss label only fits from two cells wide.
const artBeside = computed(() => props.tile.w > props.tile.h);
const showProgressTimes = computed(() => props.tile.w >= 2);

// Monochrome brand marks (24x24, currentColor) for the agent providers:
// official paths from Simple Icons (Z.ai, Claude, OpenAI, OpenCode) and
// Antigravity's mark traced from its official icon asset.
const PROVIDER_GLYPHS = {
  zcode: "M12.606 1.806l-1.677 2.388c-0.258 0.374-0.697 0.606-1.161 0.606h-9.162V1.794C0.594 1.806 12.606 1.806 12.606 1.806zM24 1.806L9.6 22.206 0 22.206 14.4 1.806zM11.394 22.206l1.69-2.4c0.258-0.374 0.697-0.606 1.161-0.606h9.149v3.006H11.394z",
  claude: "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z",
  codex: "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z",
  opencode: "M22 24H2V0h20zM17 4.8H7v14.4h10z",
  spotify:
    "M12 0C5.4 0 0 5.4 0 12s5.4 12 12 12 12-5.4 12-12S18.66 0 12 0zm5.521 17.34c-.24.359-.66.48-1.021.24-2.82-1.74-6.36-2.101-10.561-1.141-.418.122-.779-.179-.899-.539-.12-.421.18-.78.54-.9 4.56-1.021 8.52-.6 11.64 1.32.42.18.479.659.301 1.02zm1.44-3.3c-.301.42-.841.6-1.262.3-3.239-1.98-8.159-2.58-11.939-1.38-.479.12-1.02-.12-1.14-.6-.12-.48.12-1.021.6-1.141C9.6 9.9 15 10.561 18.72 12.84c.361.181.54.78.241 1.2zm.12-3.36C15.24 8.4 8.82 8.16 5.16 9.301c-.6.179-1.2-.181-1.38-.721-.18-.601.18-1.2.72-1.381 4.26-1.26 11.28-1.02 15.721 1.621.539.3.719 1.02.42 1.56-.299.421-1.02.599-1.559.3z",
  antigravity: "M0.0 21.11 1.62 19.49 3.25 16.78 4.69 12.81 6.68 5.41 8.12 2.35 9.38 0.9 10.83 0.18 12.63 0.0 14.26 0.54 15.34 1.44 16.96 4.15 20.21 15.16 21.65 18.23 23.1 19.85 23.1 20.21 24.0 20.93 24.0 22.02 22.38 22.2 21.11 21.11 20.75 21.11 18.77 18.95 15.7 13.71 14.8 12.81 13.35 12.09 10.83 12.09 9.38 12.81 7.76 14.62 5.23 19.13 2.71 21.65 1.8 22.2 0.54 22.38 0.0 22.02Z",
};

function providerSvg(provider) {
  const path = PROVIDER_GLYPHS[provider];
  if (!path) return "";
  return `<svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden="true"><path d="${path}"/></svg>`;
}

// polyline points over the tile, y normalized to the sample range
function sparkPoints(values, close) {
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const pts = values.map((v, i) => {
    const x = values.length === 1 ? 50 : (i / (values.length - 1)) * 100;
    const y = 95 - ((v - min) / span) * 90;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  return close ? `0,100 ${pts.join(" ")} 100,100` : pts.join(" ");
}

// board tiles without a title show the target board's name
const boardTileTitle = computed(() => {
  if (props.tile.title) return props.tile.title;
  if (props.tile.type === "board") return props.boardNames?.[cmd.value.id] || "";
  return "";
});

// Brief screen-reader label: the tile's title, else the target board's
// name (board tiles switch boards without a title), else the catalog or
// extension label for the tile's type.
const tileAriaLabel = computed(() => {
  if (props.tile.title) return props.tile.title;
  if (props.tile.type === "board") {
    const name = props.boardNames?.[cmd.value.id];
    if (name) return name;
  }
  return metaOf(props.tile).label || String(props.tile.type).replace(/-/g, " ");
});

function onTap() {
  if (props.tile.type === "ai-tokens-hour" && graphData.value?.rows?.length) {
    // the hour tile's tap flips between the shared sparkline and the
    // per-provider breakdown instead of firing an action; works in edit
    // mode too, where plain tiles have no click behavior at all
    hourExpanded.value = !hourExpanded.value;
    return;
  }
  emit("tap");
}

// Touch-mode sliders: drag vertically on the tile, value 0..1 from the
// pointer position. The value streams to the backend while dragging
// (throttled) so the fader follows in realtime; release always sends the
// final position. A second tap right after a tap resets Voicemeeter
// sliders to 0 dB.
const SLIDER_SEND_INTERVAL = 150;
const SLIDER_DOUBLE_TAP_MS = 350;
// pointer travel below this counts as a tap, not a drag
const SLIDER_TAP_SLOP = 6;
// keyboard step for touch-mode sliders, as a fraction of the full range
const SLIDER_KEY_STEP = 0.05;

const sliderVal = ref(null);
// end time of the last tap on this slider (a pointer interaction that
// barely moved); double-tapping a VM slider jumps to 0 dB
let lastTapAt = -Infinity;
function isVmSlider() {
  return props.tile.type === "vm-slider-strip" || props.tile.type === "vm-slider-bus";
}
function sliderValue() {
  if (sliderVal.value != null) return sliderVal.value;
  if (props.tile.type === "speaker-volume") {
    const live = Number(props.customValues["speaker-volume"]);
    if (Number.isFinite(live)) return Math.min(1, Math.max(0, live));
  }
  return 0.5;
}
function startSlider(event) {
  const rect = event.currentTarget.getBoundingClientRect();
  // capture the pointer so the drag survives the pointer leaving the
  // tile (or the window losing it mid-move); release is implicit on
  // pointerup, and the window-level listeners still receive every event
  try {
    event.currentTarget.setPointerCapture(event.pointerId);
  } catch {
    /* capture is best-effort - the fallback listeners keep working */
  }
  const setVal = (e) => {
    sliderVal.value = Math.min(1, Math.max(0, 1 - (e.clientY - rect.top) / rect.height));
  };
  let lastSent = 0;
  const startY = event.clientY;
  const send = (force = false) => {
    const now = performance.now();
    if (!force && now - lastSent < SLIDER_SEND_INTERVAL) return;
    lastSent = now;
    emit("slider", sliderVal.value ?? 0.5);
  };
  const now = performance.now();
  if (isVmSlider() && now - lastTapAt < SLIDER_DOUBLE_TAP_MS) {
    // double tap: jump to 0 dB instead of the tapped position
    sliderVal.value = VM_SLIDER_RESET;
  } else {
    setVal(event);
  }
  send(true);
  const onMove = (e) => {
    setVal(e);
    send();
  };
  const onUp = (e) => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);
    send(true);
    if (Math.abs(e.clientY - startY) < SLIDER_TAP_SLOP) {
      lastTapAt = performance.now();
    } else {
      lastTapAt = -Infinity;
    }
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

// Keyboard operability (012 lower-priority): tiles are tab stops and
// Enter/Space activates with the pointer's semantics - touch mode runs the
// tile (the same path a tap takes), edit mode opens the editor (the
// double-click equivalent; a plain click has no action in edit mode).
// exec_button is a one-shot full tap, so activation fires once per
// physical press (auto-repeat guard); holding the key cannot hold the
// injected key down - the same trade-off a touch tap already has.
function onTileKeydown(event) {
  if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
  if (event.key === "Enter" || event.key === " ") {
    if (event.repeat) return;
    // Space would scroll the board scroll container otherwise
    event.preventDefault();
    if (props.touch) onTap();
    else emit("open");
    return;
  }
  // touch-mode sliders: arrow keys step the fader like a drag would; the
  // grid's arrow navigator must not move focus instead (stopPropagation)
  const step = {
    ArrowUp: SLIDER_KEY_STEP,
    ArrowDown: -SLIDER_KEY_STEP,
    ArrowRight: SLIDER_KEY_STEP,
    ArrowLeft: -SLIDER_KEY_STEP,
  }[event.key];
  if (!props.touch || props.tile.mode !== "slider" || step === undefined) return;
  event.preventDefault();
  event.stopPropagation();
  sliderVal.value = Math.min(1, Math.max(0, sliderValue() + step));
  emit("slider", sliderVal.value);
}
</script>

<template>
  <!-- root carries the grid geometry from GridEditor via attr fallthrough -->
  <div>
    <div
      v-if="tile.id !== null"
      class="tile"
      :class="{ dragging }"
      :style="{
        background: tileBg(tile),
        borderColor: tileBorder(),
        borderRadius: tileShape(),
      }"
      role="button"
      tabindex="0"
      :aria-label="tileAriaLabel"
      :aria-pressed="tile.mode === 'toggle' ? activeState : undefined"
      @dblclick="!touch && emit('open')"
      @contextmenu.prevent="emit('ctx', $event)"
      @pointerdown="emit('down', $event)"
      @click.stop="onTap"
      @keydown="onTileKeydown"
    >
      <img
        v-if="activeState && tile.img2"
        class="tile-img"
        :src="tile.img2"
        alt=""
      />
      <img v-else-if="tile.img" class="tile-img" :src="tile.img" alt="" />
      <!-- tool tiles (clock/timer/stopwatch/counter): the whole face is
           the tool's own renderer; gestures live there in touch mode -->
      <ToolTile
        v-if="isTool"
        :tile="tile"
        :cmd="cmd"
        :state="toolState"
        :touch="touch"
        :title-color="tileTitleColor() || '#ffffff'"
        @tap="onTap"
      />
      <i
        v-if="
          tileIcon(tile) &&
          !isTool &&
          !graphData &&
          !(tile.mode === 'status' && statusData) &&
          !(tile.mode === 'custom-value' && !tile.title && customValueLabel)
        "
        class="tile-icon"
        :class="'fas fa-' + tileIcon(tile)"
        :style="{ color: tileIconColor() }"
      ></i>
      <span
        v-if="tile.mode === 'custom-value' && !tile.title && customValueLabel"
        class="tile-value"
        >{{ customValueLabel }}</span
      >
      <template v-if="graphData">
        <div class="tile-graph-head">
          <b v-if="tile.title || graphData.title">{{
            tile.title || graphData.title
          }}</b>
          <span class="tile-graph-val">{{ graphLast() }}</span>
        </div>
        <div v-if="hourShowRows()" class="tile-status tile-hour-rows">
          <div
            v-for="(row, i) in graphData.rows"
            :key="i"
            class="status-row"
          >
            <span
              v-if="row.provider"
              class="provider-glyph"
              v-html="providerSvg(row.provider)"
            ></span>
            <span class="status-label">{{ row.label }}</span>
            <span v-if="row.value" class="status-val">{{ row.value }}</span>
          </div>
        </div>
        <svg
          v-else
          class="tile-spark"
          viewBox="0 0 100 100"
          preserveAspectRatio="none"
        >
          <polygon
            :points="sparkPoints(graphData.values, true)"
            fill="rgba(255, 255, 255, 0.35)"
          />
          <polyline
            :points="sparkPoints(graphData.values)"
            fill="none"
            stroke="rgba(255, 255, 255, 0.9)"
            stroke-width="2.5"
          />
        </svg>
      </template>
      <div
        v-if="statusData"
        class="tile-status"
        :class="{
          'is-compact': statusCompact.length,
          'is-mini': statusMini.length,
          'has-art': !!statusArtUrl,
          'art-beside': !!statusArtUrl && artBeside,
        }"
      >
        <template v-if="statusCompact.length">
          <div
            v-for="(c, i) in statusCompact"
            :key="i"
            class="compact-provider"
            :class="'s-' + (c.state || 'working')"
          >
            <span class="provider-glyph" v-html="providerSvg(c.provider)"></span>
            <span class="compact-under">
              <span class="status-dot"></span>
              <span class="compact-count">{{ c.count }}</span>
            </span>
          </div>
        </template>
        <template v-else-if="statusMini.length">
          <div
            v-for="(row, i) in statusMini"
            :key="i"
            class="mini-usage"
            :class="'s-' + row.state"
            :title="row.label"
          >
            <span class="mini-glyph" v-html="providerSvg(laneProvider(row.label))"></span>
            <span class="mini-bar">
              <i :style="{ width: Math.min(100, Math.max(0, row.percent)) + '%' }"></i>
            </span>
            <span class="mini-val">{{ Math.round(row.percent) }}%</span>
          </div>
        </template>
        <template v-else>
          <!-- Media card: a payload with album art and/or playback
               progress renders as title + artist (+ album and extra
               rows from two cells wide) below/beside the art, ending
               in the playback bar. The art stays whole: fit, aspect
               kept, nothing on top of it. -->
          <template v-if="mediaView">
            <img v-if="statusArtUrl" class="status-art" :src="statusArtUrl" alt="" />
            <div class="status-body media-body">
              <span
                class="media-title"
                :style="{ '-webkit-line-clamp': mediaTitleLines }"
                >{{ mediaView.title }}</span
              >
              <span v-if="mediaView.artist" class="media-sub">{{
                mediaView.artist
              }}</span>
              <template v-if="showMediaMeta">
                <span v-if="mediaView.album" class="media-sub dim">{{
                  mediaView.album
                }}</span>
                <span
                  v-for="(row, i) in mediaView.rest"
                  :key="i"
                  class="media-sub dim"
                  >{{ row.label ? row.label + ': ' : '' }}{{ row.value }}</span
                >
              </template>
              <div v-if="progressView" class="status-progress">
                <span class="status-progress-track">
                  <i :style="{ width: progressView.percent + '%' }"></i>
                </span>
                <span v-if="showProgressTimes" class="status-progress-time">{{
                  progressView.label
                }}</span>
              </div>
            </div>
          </template>
          <template v-else>
            <!-- album art, shown whole (fit, aspect kept, nothing on top) -->
            <img v-if="statusArtUrl" class="status-art" :src="statusArtUrl" alt="" />
            <div class="status-body">
              <div
                v-for="(row, i) in statusRows"
                :key="i"
                class="status-row"
                :class="['s-' + (row.state || 'off'), { 'is-header': row.state === 'header' }]"
              >
                <span v-if="row.state !== 'header'" class="status-dot"></span>
                <span v-if="statusGlyph(row)" class="provider-glyph" v-html="statusGlyph(row)"></span>
                <span v-else-if="row.label" class="status-label">{{ row.label }}</span>
                <span v-if="row.value" class="status-val">{{ row.value }}</span>
                <span v-if="row.percent != null" class="status-bar">
                  <i :style="{ width: Math.min(100, Math.max(0, row.percent)) + '%' }"></i>
                </span>
              </div>
              <div v-if="statusSummary" class="status-summary">
                {{ statusSummary }}
              </div>
              <!-- locally extrapolated playback progress, ticked ~1 Hz only
                   while playing and on screen -->
              <div v-if="progressView" class="status-progress">
                <span class="status-progress-track">
                  <i :style="{ width: progressView.percent + '%' }"></i>
                </span>
                <span v-if="showProgressTimes" class="status-progress-time">{{
                  progressView.label
                }}</span>
              </div>
            </div>
          </template>
          <!-- brand mark in a tile corner, outside the art -->
          <span
            v-if="statusArtUrl"
            class="status-corner-glyph"
            v-html="providerSvg('spotify')"
          ></span>
        </template>
      </div>
      <span
        v-if="boardTileTitle && !isTool"
        class="tile-title"
        :class="`pos-${tileTitlePos()}`"
        :style="{
          color: tileTitleColor() || '#ffffff',
          background: tileTitleBox() || 'transparent',
        }"
        >{{ boardTileTitle }}</span
      >
      <span v-if="tile.mode === 'slider' && !touch" class="tile-badge">
        <i class="fas fa-sliders-h"></i>
      </span>
      <template v-if="touch && tile.mode === 'slider'">
        <div
          class="slider-fill"
          :style="{ height: sliderValue() * 100 + '%' }"
        ></div>
        <div
          class="slider-thumb"
          :style="{ top: (1 - sliderValue()) * 100 + '%' }"
        ></div>
        <div class="slider-capture" @pointerdown.stop.prevent="startSlider($event)"></div>
      </template>
      <span
        v-if="!touch"
        class="resize-handle"
        @pointerdown.stop="emit('resize', $event)"
      ></span>
    </div>
  </div>
</template>

<style scoped>
.touch .tile { cursor: pointer; }
.touch .tile:active { transform: scale(0.96); }
.tile {
  position: relative;
  width: 100%;
  height: 100%;
  border: 2px solid transparent;
  border-radius: 8px;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: grab;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.25);
  transition: filter 120ms ease-out, box-shadow 120ms ease-out;
}
.tile:hover { filter: brightness(1.08); box-shadow: 0 4px 10px rgba(0, 0, 0, 0.3); }
/* keyboard focus (012 lower-priority): shown for keyboard focus only, so
   mouse clicks and touch taps stay outline-free; inset because the tile
   clips its own overflow */
.tile:focus-visible {
  outline: 2px solid rgba(255, 255, 255, 0.95);
  outline-offset: -4px;
}
.tile.dragging {
  opacity: 0.8;
  cursor: grabbing;
  filter: brightness(1.05);
  box-shadow: 0 10px 26px rgba(0, 0, 0, 0.5);
}
.tile-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.tile-icon { font-size: 30px; pointer-events: none; text-shadow: 0 1px 3px rgba(0, 0, 0, 0.3); }
.tile-title {
  position: absolute;
  left: 0;
  right: 0;
  padding: 2px 5px;
  font-size: 11.5px;
  font-weight: 500;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;  pointer-events: none;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
  line-height: 1.3;
}
.tile-title.pos-0 { bottom: 0; }
.tile-title.pos-1 {
  top: 50%;
  bottom: auto;
  transform: translateY(-50%);
  font-size: 14px;
  text-shadow: none;
}
.tile-title.pos-2 { top: 0; }
.tile-badge {
  position: absolute;
  top: 4px;
  right: 6px;
  font-size: 12px;
  opacity: 0.85;
}
/* live value pushed by extensions, shown instead of an icon */
.tile-value {
  font-size: 21px;
  font-weight: 700;
  color: #ffffff;
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
  pointer-events: none;
}
/* graph tiles: title + value block over a sparkline in the lower half */
.tile-graph-head {
  position: absolute;
  inset: 4px 6px auto;
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 1px;
  color: #ffffff;
  pointer-events: none;
}
.tile-graph-head b { font-size: 11px; font-weight: 600; opacity: 0.9; }
.tile-graph-val { font-size: 18px; font-weight: 700; }
.tile-spark {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  width: 100%;
  height: 50%;
  pointer-events: none;
}
/* status tiles: colored-dot rows with an optional thin usage bar per row */
.tile-status {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 5px;
  padding: 6px 9px;
  pointer-events: none;
}
/* detail rows get a slightly larger logo than the hour-tile breakdown */
.status-row .provider-glyph svg {
  width: 17px;
  height: 17px;
}
.status-row {
  position: relative;
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: 0;
}
.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: none;
  background: #95a5a6;
}
.s-working .status-dot,
.s-ok .status-dot {
  background: #2ecc71;
}
.s-working .status-dot {
  animation: status-pulse 1.6s ease-in-out infinite;
}
.s-attention .status-dot,
.s-warn .status-dot {
  background: #f39c12;
}
.s-high .status-dot,
.s-error .status-dot {
  background: #e74c3c;
}
.status-label {
  font-size: 11px;
  font-weight: 600;
  color: #ffffff;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.35);
}
.status-val {
  margin-left: auto;
  font-size: 10.5px;
  color: rgba(255, 255, 255, 0.85);
  white-space: nowrap;
}
/* agent states color the value text to match the dot */
.s-working .status-val {
  color: #2ecc71;
}
.s-attention .status-val {
  color: #f39c12;
}
.s-done .status-val {
  color: rgba(255, 255, 255, 0.55);
}
/* project header rows: small caps, no dot, breathing room above */
.status-row.is-header {
  margin-top: 3px;
}
.status-row.is-header:first-child {
  margin-top: 0;
}
.is-header .status-label {
  font-size: 9.5px;
  font-weight: 700;
  letter-spacing: 0.6px;
  text-transform: uppercase;
  color: rgba(255, 255, 255, 0.55);
  text-shadow: none;
}
.status-bar {
  position: absolute;
  left: 0;
  right: 0;
  bottom: -3px;
  height: 2px;
  background: rgba(255, 255, 255, 0.15);
  border-radius: 1px;
}
.status-bar i {
  display: block;
  height: 100%;
  border-radius: 1px;
  background: rgba(255, 255, 255, 0.75);
}
/* plan-limit bars take the threshold palette, like the dots */
.s-ok .status-bar i {
  background: #2ecc71;
}
.s-warn .status-bar i,
.s-attention .status-bar i {
  background: #f39c12;
}
.s-high .status-bar i,
.s-error .status-bar i {
  background: #e74c3c;
}
.status-summary {
  font-size: 10.5px;
  color: rgba(255, 255, 255, 0.8);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  border-top: 1px solid rgba(255, 255, 255, 0.14);
  margin-top: 2px;
  padding-top: 4px;
}
/* album art + playback progress (design §4): the art shows whole - fit,
   aspect kept, never cropped, tinted or overdrawn; rows, summary and the
   progress bar sit below it (portrait/square tiles) or beside it (wide
   tiles). Without art the body is layout-transparent (display: contents)
   so plain status tiles render exactly as before. */
.status-art {
  min-width: 0;
  min-height: 0;
  object-fit: contain;
}
.tile-status.has-art .status-art {
  flex: 1 1 0;
  width: 100%;
}
.tile-status.has-art .status-body {
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
.tile-status:not(.has-art) .status-body {
  display: contents;
}
.tile-status.has-art.art-beside {
  flex-direction: row;
  align-items: stretch;
}
.tile-status.has-art.art-beside .status-art {
  flex: 0 0 40%;
  width: 40%;
  height: 100%;
}
.tile-status.has-art.art-beside .status-body {
  flex: 1 1 0;
  justify-content: center;
}
/* media card text block: stacked title / artist / album instead of the
   label-value rows, each line stepping down in size so a single narrow
   cell keeps the title readable while the metadata yields */
.media-body {
  gap: 4px;
}
.media-title {
  font-size: 12.5px;
  font-weight: 700;
  line-height: 1.25;
  color: #ffffff;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.35);
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow-wrap: anywhere;
}
.media-sub {
  font-size: 10.5px;
  color: rgba(255, 255, 255, 0.65);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.media-sub.dim {
  font-size: 10px;
  color: rgba(255, 255, 255, 0.45);
}
.status-progress {
  display: flex;
  align-items: center;
  gap: 6px;
}
.status-progress-track {
  flex: 1;
  height: 2px;
  background: rgba(255, 255, 255, 0.15);
  border-radius: 1px;
  overflow: hidden;
}
.status-progress-track i {
  display: block;
  height: 100%;
  border-radius: 1px;
  background: rgba(255, 255, 255, 0.75);
}
.status-progress-time {
  font-size: 9.5px;
  color: rgba(255, 255, 255, 0.7);
  white-space: nowrap;
}
.status-corner-glyph {
  position: absolute;
  top: 5px;
  right: 6px;
  color: rgba(255, 255, 255, 0.8);
  pointer-events: none;
}
.status-corner-glyph svg {
  width: 13px;
  height: 13px;
}
.provider-glyph {
  flex: none;
  display: flex;
  align-items: center;
  color: rgba(255, 255, 255, 0.72);
}
/* compact fallback: vertical provider stack, each logo with its dot and
   the active+waiting count right below it */
.tile-status.is-compact {
  flex-direction: row;
  align-items: center;
  justify-content: center;
  gap: 20px;
}
.compact-provider {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
}
.compact-provider .provider-glyph svg {
  width: 34px;
  height: 34px;
}
.compact-provider .provider-glyph {
  color: rgba(255, 255, 255, 0.85);
}
.compact-under {
  display: flex;
  align-items: center;
  gap: 5px;
}
.compact-count {
  font-size: 12px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.9);
}
/* mini usage view (1x1 plan tiles): glyph + colored bar + percentage */
.tile-status.is-mini {
  justify-content: center;
  gap: 10px;
  padding: 8px;
}
.mini-usage {
  display: flex;
  align-items: center;
  gap: 6px;
}
.mini-usage .mini-glyph {
  display: flex;
  color: rgba(255, 255, 255, 0.8);
}
.mini-usage .mini-glyph svg {
  width: 12px;
  height: 12px;
}
.mini-bar {
  flex: 1;
  height: 8px;
  background: rgba(26, 188, 156, 0.18);
  border-radius: 2px;
  overflow: hidden;
}
.mini-bar i {
  display: block;
  height: 100%;
  border-radius: 2px;
  background: var(--accent);
}
.mini-val {
  min-width: 30px;
  font-size: 11.5px;
  font-weight: 700;
  text-align: right;
  color: var(--accent);
}
/* per-provider breakdown overlay on the hour graph tile */
.tile-hour-rows {
  position: static;
  flex: 1;
  justify-content: flex-start;
  overflow: hidden;
  padding: 0;
}
@keyframes status-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
.slider-fill {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(255, 255, 255, 0.28);
  pointer-events: none;
}
.slider-thumb {
  position: absolute;
  left: 0;
  right: 0;
  height: 3px;
  transform: translateY(-50%);
  background: rgba(255, 255, 255, 0.85);
  pointer-events: none;
}
.slider-capture {
  position: absolute;
  inset: 0;
  cursor: ns-resize;
  touch-action: none;
}
.resize-handle {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 16px;
  height: 16px;
  cursor: nwse-resize;
  background: linear-gradient(135deg, transparent 50%, rgba(255, 255, 255, 0.55) 50%);
  opacity: 0;
  transition: opacity 120ms ease-out;
}
.tile:hover .resize-handle { opacity: 1; }
</style>
