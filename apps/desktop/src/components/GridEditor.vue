<script setup>
import { computed, reactive, ref } from "vue";
import { CELL_W, ROW_H, stateActive } from "../catalog";

// Edit-mode grid: drag to move, corner handle to resize, double-click to
// edit, empty-cell click to add. Touch mode: tap runs the tile.
const props = defineProps({
  board: { type: Object, required: true },
  touch: { type: Boolean, default: false },
  zoom: { type: Number, default: 1 },
  // type -> {icon, color} fallbacks from the action catalog + extensions
  typeMeta: { type: Object, default: () => ({}) },
  // live state pushes (APP_CUSTOM_VALUE / APP_*), see applyStatusUpdate
  customValues: { type: Object, default: () => ({}) },
  appStates: { type: Object, default: () => ({}) },
  // board id -> name, for board-switch tiles without a title
  boardNames: { type: Object, default: () => ({}) },
});
const emit = defineEmits(["tile-open", "tile-moved", "tile-add", "tile-exec", "tile-slider", "ctx-tile", "ctx-empty"]);

function metaOf(tile) {
  return props.typeMeta?.[tile.type] || {};
}

// dual-state tiles flip to their second state when tapped, like the
// tablet client; the toggle lives for the editor session only
const activeTiles = reactive(new Set());
// ai-tokens-hour tiles currently showing the per-provider breakdown
const hourExpanded = reactive(new Set());

// the hour tile renders either the shared sparkline or, when tapped, the
// per-provider rows the producer ships alongside the graph value
function hourShowRows(tile) {
  return hourExpanded.has(tile.id) && graphData(tile)?.rows?.length;
}
function isDual(tile) {
  return Boolean(
    tile.color2 || tile.icon2 || tile.img2 || metaOf(tile).dual
  );
}

// Whether the tile renders its second state. A known live state wins
// (original ToggleButton isActive); otherwise the session tap flip.
function tileActive(tile) {
  const state = stateActive(tile, props.customValues, props.appStates, props.typeMeta);
  return state !== null ? state : activeTiles.has(tile.id);
}

// second-state fields only apply when they are actually set (original:
// `(isActive ? data.color2 : data.color) || style.color`)
function pick(tile, first, second) {
  return tileActive(tile) ? tile[second] || tile[first] : tile[first];
}

function tileBg(tile) {
  return pick(tile, "color", "color2") || metaOf(tile).color || "#2c3e50";
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
  return pick(tile, "icon", "icon2") || metaOf(tile).icon || "";
}
function tileIconColor(tile) {
  return pick(tile, "icon_color", "icon_color2") || "#ffffff";
}
function tileShape(tile) {
  const shape = pick(tile, "shape", "shape2");
  return shape === 1 ? "50%" : "8px";
}
function tileBorder(tile) {
  return pick(tile, "border_color", "border_color2") || "transparent";
}
function tileTitlePos(tile) {
  return pick(tile, "title_position", "title_position2") ?? 0;
}
function tileTitleColor(tile) {
  // color2 only participates when it is set, like the original title style
  return tile.title_color2 && tileActive(tile) ? tile.title_color2 : tile.title_color;
}
function tileTitleBox(tile) {
  return tile.title_box_color2 && tileActive(tile)
    ? tile.title_box_color2
    : tile.title_box_color;
}

// ---- live-value tiles (custom-value / graph / board), like the stock client

// CustomValueButton: label = customValues[command || type], last sample of
// graph-style objects. Shown when the tile has no title of its own.
function customValueLabel(tile) {
  const v = props.customValues[tile.command || tile.type];
  if (v == null || typeof v === "object") return "";
  return String(v);
}

// GraphButton: the extension pushes {values, title?, suffix?, description?}
// under the tile's command (or type), like custom-value labels
function graphData(tile) {
  if (tile.mode !== "graph") return null;
  const v = props.customValues[tile.command || tile.type];
  if (!v || typeof v !== "object" || !Array.isArray(v.values) || !v.values.length) {
    return null;
  }
  return v;
}

function graphLast(tile) {
  const g = graphData(tile);
  if (g.value_label) return g.value_label;
  const last = g.values[g.values.length - 1];
  return `${last ?? ""}${g.suffix || ""}`;
}

// StatusButton: the producer pushes {title, rows:[{label, value, state,
// percent?, provider?}], compact:[{provider, value}], summary} under the
// tile's command (or type) - a live list that replaces itself wholesale
// (see mergeCustomValues). Rows arrive display-ready so this stays a dumb
// renderer.
function statusData(tile) {
  if (tile.mode !== "status") return null;
  const v = props.customValues[tile.command || tile.type];
  if (!v || typeof v !== "object" || !Array.isArray(v.rows) || !v.rows.length) {
    return null;
  }
  return v;
}

// When the tile cannot fit the detail (small tile, or more rows than the
// height can hold) the producer's compact per-provider counts take over -
// logo plus "N working · M done", no chat titles.
function statusRows(tile) {
  const data = statusData(tile);
  if (!data) return [];
  const compact = data.compact;
  if (Array.isArray(compact) && compact.length && (tile.h <= 1 || data.rows.length > tile.h * 4)) {
    return compact.map((c) => ({ ...c, state: "compact" }));
  }
  return data.rows;
}

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
function boardTileTitle(tile) {
  if (tile.title) return tile.title;
  if (tile.type === "board") {
    try {
      return props.boardNames?.[JSON.parse(tile.command || "{}").id] || "";
    } catch {
      return "";
    }
  }
  return "";
}

const drag = ref(null); // {tile, mode:'move'|'resize', dx, dy, pointerId}

// zoomed cell size keeps the grid proportional at 50-150%
const cell = computed(() => CELL_W * props.zoom);
const row = computed(() => ROW_H * props.zoom);

const gridStyle = computed(() => ({
  width: `${props.board.width * cell.value}px`,
  height: `${props.board.height * row.value}px`,
  backgroundImage: props.board.image
    ? `url(${props.board.image})`
    : "none",
  backgroundSize: "cover",
}));

function tileStyle(tile) {
  const d = drag.value && drag.value.tile.id === tile.id ? drag.value : null;
  const x = d && d.mode === "move" ? tile.x + d.cx : tile.x;
  const y = d && d.mode === "move" ? tile.y + d.cy : tile.y;
  const w = d && d.mode === "resize" ? Math.max(1, tile.w + d.cw) : tile.w;
  const h = d && d.mode === "resize" ? Math.max(1, tile.h + d.ch) : tile.h;
  return {
    left: `${clamp(x, 0, props.board.width - 1) * cell.value}px`,
    top: `${clamp(y, 0, props.board.height - 1) * row.value}px`,
    width: `${clamp(w, 1, props.board.width) * cell.value}px`,
    height: `${clamp(h, 1, props.board.height) * row.value}px`,
    zIndex: d ? 10 : 1,
  };
}

function clamp(v, min, max) {
  return Math.min(max, Math.max(min, v));
}

// every unoccupied grid position renders as a visible empty slot, like the
// original editor and tablet client
const emptyCells = computed(() => {
  const occupied = new Set();
  for (const t of props.board.buttons) {
    for (let dy = 0; dy < Math.max(1, t.h); dy++) {
      for (let dx = 0; dx < Math.max(1, t.w); dx++) {
        occupied.add(`${t.x + dx},${t.y + dy}`);
      }
    }
  }
  const cells = [];
  for (let y = 0; y < props.board.height; y++) {
    for (let x = 0; x < props.board.width; x++) {
      if (!occupied.has(`${x},${y}`)) cells.push({ x, y });
    }
  }
  return cells;
});

function startDrag(tile, mode, event) {
  if (props.touch) return;
  if (event.button !== 0) return; // right/middle click must not drag
  event.preventDefault();
  const startX = event.clientX;
  const startY = event.clientY;
  drag.value = {
    tile,
    mode,
    startX,
    startY,
    cellW: cell.value,
    cellH: row.value,
    cx: 0,
    cy: 0,
    cw: 0,
    ch: 0,
  };
  const onMove = (e) => {
    const d = drag.value;
    if (!d) return;
    d.cx = Math.round((e.clientX - d.startX) / d.cellW);
    d.cy = Math.round((e.clientY - d.startY) / d.cellH);
    d.cw = Math.round((e.clientX - d.startX) / d.cellW);
    d.ch = Math.round((e.clientY - d.startY) / d.cellH);
  };
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    const d = drag.value;
    drag.value = null;
    if (!d) return;
    if (d.mode === "move") {
      const x = clamp(tile.x + d.cx, 0, props.board.width - 1);
      const y = clamp(tile.y + d.cy, 0, props.board.height - 1);
      if (x !== tile.x || y !== tile.y) emit("tile-moved", tile, x, y, tile.w, tile.h);
    } else {
      const w = clamp(tile.w + d.cw, 1, props.board.width - tile.x);
      const h = clamp(tile.h + d.ch, 1, props.board.height - tile.y);
      if (w !== tile.w || h !== tile.h) emit("tile-moved", tile, tile.x, tile.y, w, h);
    }
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}

function onTileTap(tile) {
  if (tile.type === "ai-tokens-hour" && graphData(tile)?.rows?.length) {
    // the hour tile's tap flips between the shared sparkline and the
    // per-provider breakdown instead of firing an action; works in edit
    // mode too, where plain tiles have no click behavior at all
    if (hourExpanded.has(tile.id)) hourExpanded.delete(tile.id);
    else hourExpanded.add(tile.id);
    return;
  }
  if (!props.touch) return;
  if (tile.mode === "slider") return;
  if (isDual(tile)) {
    // flip to the second state like the tablet client does
    if (activeTiles.has(tile.id)) activeTiles.delete(tile.id);
    else activeTiles.add(tile.id);
  }
  emit("tile-exec", tile);
}

// right-click (long-press on touch devices) over a tile: touch mode keeps
// the old "open settings" behavior, edit mode opens the custom menu
function onTileContext(tile, event) {
  if (props.touch) {
    emit("tile-open", tile);
    return;
  }
  emit("ctx-tile", tile, event);
}

// right-click on an empty cell (edit mode): context menu with "add here"
function onEmptyContext(pos, event) {
  if (props.touch) return;
  emit("ctx-empty", pos, event);
}

// right-click on the grid padding / background resolves the cell under
// the pointer, like onGridClick
function onGridContext(event) {
  if (props.touch) return;
  if (event.target !== event.currentTarget) return;
  const rect = event.currentTarget.getBoundingClientRect();
  const x = clamp(Math.floor((event.clientX - rect.left) / cell.value), 0, props.board.width - 1);
  const y = clamp(Math.floor((event.clientY - rect.top) / row.value), 0, props.board.height - 1);
  const occupied = props.board.buttons.some(
    (t) => x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h
  );
  if (!occupied) emit("ctx-empty", { x, y }, event);
}

// Touch-mode sliders: drag vertically on the tile, value 0..1 from the
// pointer position, sent to the backend on release (fill previews live).
const sliderVals = reactive({});
function sliderValue(tile) {
  if (sliderVals[tile.id] != null) return sliderVals[tile.id];
  if (tile.type === "speaker-volume") {
    const live = Number(props.customValues["speaker-volume"]);
    if (Number.isFinite(live)) return clamp(live, 0, 1);
  }
  return 0.5;
}
function startSlider(tile, event) {
  const rect = event.currentTarget.getBoundingClientRect();
  const setVal = (e) => {
    sliderVals[tile.id] = clamp(1 - (e.clientY - rect.top) / rect.height, 0, 1);
  };
  setVal(event);
  const onMove = (e) => setVal(e);
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    emit("tile-slider", tile, sliderVals[tile.id] ?? 0.5);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}

function onGridClick(event) {
  if (props.touch) return;
  if (event.target !== event.currentTarget) return;
  // empty-cell click opens the add-tile flow at that cell
  const rect = event.currentTarget.getBoundingClientRect();
  const x = clamp(Math.floor((event.clientX - rect.left) / cell.value), 0, props.board.width - 1);
  const y = clamp(Math.floor((event.clientY - rect.top) / row.value), 0, props.board.height - 1);
  const occupied = props.board.buttons.some(
    (t) =>
      x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h
  );
  if (!occupied) emit("tile-add", { x, y });
}
</script>

<template>
  <div class="board-scroll">
    <div
      class="board-grid"
      :class="{ touch }"
      :style="gridStyle"
      @click="onGridClick"
      @contextmenu.prevent="onGridContext"
      >
      <div
        v-for="c in emptyCells"
        :key="`empty-${c.x}-${c.y}`"
        class="tile-slot"
        :style="{
          left: `${c.x * cell}px`,
          top: `${c.y * row}px`,
          width: `${cell}px`,
          height: `${row}px`,
        }"
        @click.stop="!touch && $emit('tile-add', { x: c.x, y: c.y })"
        @contextmenu.prevent.stop="onEmptyContext({ x: c.x, y: c.y }, $event)"
      >
        <div class="empty-cell"></div>
      </div>
      <div
        v-for="tile in board.buttons"
        :key="tile.id ?? `fill-${tile.x}-${tile.y}`"
        class="tile-slot"
        :style="tileStyle(tile)"
      >
        <div
          v-if="tile.id !== null"
          class="tile"
          :class="{ dragging: drag && drag.tile.id === tile.id }"
          :style="{
            background: tileBg(tile),
            borderColor: tileBorder(tile),
            borderRadius: tileShape(tile),
          }"
          @dblclick="!touch && $emit('tile-open', tile)"
          @contextmenu.prevent="onTileContext(tile, $event)"
          @pointerdown="startDrag(tile, 'move', $event)"
          @click.stop="onTileTap(tile)"
        >
          <img
            v-if="tileActive(tile) && tile.img2"
            class="tile-img"
            :src="tile.img2"
            alt=""
          />
          <img v-else-if="tile.img" class="tile-img" :src="tile.img" alt="" />
          <i
            v-if="
              tileIcon(tile) &&
              !(tile.mode === 'graph' && graphData(tile)) &&
              !(tile.mode === 'status' && statusData(tile)) &&
              !(tile.mode === 'custom-value' && !tile.title && customValueLabel(tile))
            "
            class="tile-icon"
            :class="'fas fa-' + tileIcon(tile)"
            :style="{ color: tileIconColor(tile) }"
          ></i>
          <span
            v-if="tile.mode === 'custom-value' && !tile.title && customValueLabel(tile)"
            class="tile-value"
            >{{ customValueLabel(tile) }}</span
          >
          <template v-if="tile.mode === 'graph' && graphData(tile)">
            <div class="tile-graph-head">
              <b v-if="tile.title || graphData(tile).title">{{
                tile.title || graphData(tile).title
              }}</b>
              <span class="tile-graph-val">{{ graphLast(tile) }}</span>
            </div>
            <div v-if="hourShowRows(tile)" class="tile-status tile-hour-rows">
              <div
                v-for="(row, i) in graphData(tile).rows"
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
                :points="sparkPoints(graphData(tile).values, true)"
                fill="rgba(255, 255, 255, 0.35)"
              />
              <polyline
                :points="sparkPoints(graphData(tile).values)"
                fill="none"
                stroke="rgba(255, 255, 255, 0.9)"
                stroke-width="2.5"
              />
            </svg>
          </template>
          <div v-if="tile.mode === 'status' && statusData(tile)" class="tile-status">
            <div
              v-for="(row, i) in statusRows(tile)"
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
              v-if="statusData(tile).summary && !statusRows(tile).some((r) => r.state === 'compact')"
              class="status-summary"
            >
              {{ statusData(tile).summary }}
            </div>
          </div>
          <span
            v-if="boardTileTitle(tile)"
            class="tile-title"
            :class="`pos-${tileTitlePos(tile)}`"
            :style="{
              color: tileTitleColor(tile) || '#ffffff',
              background: tileTitleBox(tile) || 'transparent',
            }"
            >{{ boardTileTitle(tile) }}</span
          >
          <span v-if="tile.mode === 'slider' && !touch" class="tile-badge">
            <i class="fas fa-sliders-h"></i>
          </span>
          <template v-if="touch && tile.mode === 'slider'">
            <div
              class="slider-fill"
              :style="{ height: sliderValue(tile) * 100 + '%' }"
            ></div>
            <div
              class="slider-thumb"
              :style="{ top: (1 - sliderValue(tile)) * 100 + '%' }"
            ></div>
            <div class="slider-capture" @pointerdown.stop.prevent="startSlider(tile, $event)"></div>
          </template>
          <span
            v-if="!touch"
            class="resize-handle"
            @pointerdown.stop="startDrag(tile, 'resize', $event)"
          ></span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.board-scroll {
  position: absolute;
  inset: 0;
  overflow: auto;
  display: flex;
  padding: 20px;
}
.board-grid {
  position: relative;
  display: block;
  margin: auto;
  background-size: cover;
}
.tile-slot { position: absolute; padding: 5px; }
.tile-slot:has(> .empty-cell) { cursor: pointer; }
.touch .empty-cell { display: none; }
.touch .tile-slot { cursor: default; }
.empty-cell {
  width: 100%;
  height: 100%;
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.09);
  transition: background 120ms ease-out;
}
.tile-slot:hover > .empty-cell { background: rgba(0, 0, 0, 0.16); }
.touch .tile-slot:hover > .empty-cell { background: rgba(0, 0, 0, 0.09); }
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
.touch .tile { cursor: pointer; }
.touch .tile:active { transform: scale(0.96); }
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
