<script setup>
import { computed, ref } from "vue";
import { stateActive } from "../catalog";

// One tile. Isolated so a live state push re-renders only the tiles that
// read the pushed key, not the whole board. Geometry (grid position,
// size, drag offset) stays in GridEditor and falls through onto the root
// element via class/style attrs; interaction events bubble up as emits.
const props = defineProps({
  tile: { type: Object, required: true },
  touch: { type: Boolean, default: false },
  // type -> {icon, color, mode, dual} fallbacks from the action catalog
  typeMeta: { type: Object, default: () => ({}) },
  // live state pushes (APP_CUSTOM_VALUE / APP_*), see applyStatusUpdate
  customValues: { type: Object, default: () => ({}) },
  appStates: { type: Object, default: () => ({}) },
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

// Whether the tile renders its second state. A known live state wins
// (original ToggleButton isActive); otherwise the session tap flip.
const activeState = computed(() => {
  const state = stateActive(props.tile, cmd.value, props.customValues, props.appStates, props.typeMeta);
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
function tileTitleColor() {
  // color2 only participates when it is set, like the original title style
  return props.tile.title_color2 && activeState.value ? props.tile.title_color2 : props.tile.title_color;
}
function tileTitleBox() {
  return props.tile.title_box_color2 && activeState.value
    ? props.tile.title_box_color2
    : props.tile.title_box_color;
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
// height can hold) the producer's compact per-provider counts take over -
// logo plus "N working - M done", no chat titles.
const statusRows = computed(() => {
  const data = statusData.value;
  if (!data) return [];
  const compact = data.compact;
  if (Array.isArray(compact) && compact.length && (props.tile.h <= 1 || data.rows.length > props.tile.h * 4)) {
    return compact.map((c) => ({ ...c, state: "compact" }));
  }
  return data.rows;
});

// Monochrome brand glyphs (12x12, currentColor) for the agent providers.
// Hand-drawn paths: Claude's starburst, OpenAI's knot, a Z mark, OpenCode's
// brackets and Antigravity's delta.
const PROVIDER_GLYPHS = {
  zcode: "M2 2h8v2.2L5.6 8H10v2H2V7.8L6.4 4H2V2Z",
  claude: "M6 0.8 7 4.6 10.2 2.7 8.3 6 12 7 8.3 8 10.2 11.3 7 9.4 6 13.2 5 9.4 1.8 11.3 3.7 8 0 7 3.7 6 1.8 2.7 5 4.6Z",
  codex: "M6 1a5 5 0 0 1 4.3 2.5A5 5 0 0 1 10.5 10 5 5 0 0 1 6 13a5 5 0 0 1-4.5-3A5 5 0 0 1 1.7 3.5 5 5 0 0 1 6 1Zm0 2a3 3 0 0 0-2.6 1.5A3 3 0 0 0 3.2 8.4 3 3 0 0 0 6 11a3 3 0 0 0 2.8-2.6A3 3 0 0 0 8.6 4.5 3 3 0 0 0 6 3Z",
  opencode: "M4 2 1.5 6 4 10h2.2L3.7 6 6.2 2H4Zm4 0 2.5 4L8 10H5.8L8.3 6 5.8 2H8Z",
  antigravity: "M6 1 11.3 11H0.7L6 1Zm0 4.6L3.5 10.3h5L6 5.6Z",
};

function providerSvg(provider) {
  const path = PROVIDER_GLYPHS[provider];
  if (!path) return "";
  return `<svg viewBox="0 0 12 12" width="11" height="11" fill="currentColor" aria-hidden="true"><path d="${path}"/></svg>`;
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
// pointer position, sent to the backend on release (fill previews live).
const sliderVal = ref(null);
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
  const setVal = (e) => {
    sliderVal.value = Math.min(1, Math.max(0, 1 - (e.clientY - rect.top) / rect.height));
  };
  setVal(event);
  const onMove = (e) => setVal(e);
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);
    emit("slider", sliderVal.value ?? 0.5);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
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
      @dblclick="!touch && emit('open')"
      @contextmenu.prevent="emit('ctx', $event)"
      @pointerdown="emit('down', $event)"
      @click.stop="onTap"
    >
      <img
        v-if="activeState && tile.img2"
        class="tile-img"
        :src="tile.img2"
        alt=""
      />
      <img v-else-if="tile.img" class="tile-img" :src="tile.img" alt="" />
      <i
        v-if="
          tileIcon(tile) &&
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
      <div v-if="statusData" class="tile-status">
        <div
          v-for="(row, i) in statusRows"
          :key="i"
          class="status-row"
          :class="['s-' + (row.state || 'off'), { 'is-header': row.state === 'header' }]"
        >
          <span v-if="row.state !== 'header' && row.state !== 'compact'" class="status-dot"></span>
          <span
            v-if="row.provider && providerSvg(row.provider)"
            class="provider-glyph"
            v-html="providerSvg(row.provider)"
          ></span>
          <span v-if="row.label" class="status-label">{{ row.label }}</span>
          <span v-if="row.value" class="status-val">{{ row.value }}</span>
          <span v-if="row.percent != null" class="status-bar">
            <i :style="{ width: Math.min(100, Math.max(0, row.percent)) + '%' }"></i>
          </span>
        </div>
        <div
          v-if="statusData.summary && !statusRows.some((r) => r.state === 'compact')"
          class="status-summary"
        >
          {{ statusData.summary }}
        </div>
      </div>
      <span
        v-if="boardTileTitle"
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
.provider-glyph {
  flex: none;
  display: flex;
  align-items: center;
  color: rgba(255, 255, 255, 0.72);
}
/* compact fallback rows: glyph left, counts right */
.s-compact .status-val {
  margin-left: auto;
  color: rgba(255, 255, 255, 0.85);
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
