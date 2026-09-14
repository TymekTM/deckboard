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
});
const emit = defineEmits(["tile-open", "tile-moved", "tile-add", "tile-exec", "tile-slider", "ctx-tile", "ctx-empty"]);

function metaOf(tile) {
  return props.typeMeta?.[tile.type] || {};
}

// dual-state tiles flip to their second state when tapped, like the
// tablet client; the toggle lives for the editor session only
const activeTiles = reactive(new Set());
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
function tileIcon(tile) {
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
            v-if="tileIcon(tile)"
            class="tile-icon"
            :class="'fas fa-' + tileIcon(tile)"
            :style="{ color: tileIconColor(tile) }"
          ></i>
          <span
            v-if="tile.title"
            class="tile-title"
            :class="`pos-${tileTitlePos(tile)}`"
            :style="{
              color: tileTitleColor(tile) || '#ffffff',
              background: tileTitleBox(tile) || 'transparent',
            }"
            >{{ tile.title }}</span
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
  text-overflow: ellipsis;
  pointer-events: none;
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
