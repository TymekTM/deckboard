<script setup>
import { computed, reactive, ref } from "vue";
import { CELL_W, ROW_H, MAX_BOARD_DIM, clamp } from "../catalog";
import TileCell from "./TileCell.vue";

// Edit-mode grid: drag to move, corner handle to resize, double-click to
// edit, empty-cell click to add. Touch mode: tap runs the tile. Tiles
// themselves render in TileCell so live state pushes re-render only the
// affected tiles, not the whole board.
const props = defineProps({
  board: { type: Object, required: true },
  touch: { type: Boolean, default: false },
  zoom: { type: Number, default: 1 },
  // type -> {icon, color, mode, dual} fallbacks from the action catalog
  typeMeta: { type: Object, default: () => ({}) },
  // live state pushes (APP_CUSTOM_VALUE / APP_*), see applyStatusUpdate
  customValues: { type: Object, default: () => ({}) },
  appStates: { type: Object, default: () => ({}) },
  // board id -> name, for board-switch tiles without a title
  boardNames: { type: Object, default: () => ({}) },
});
const emit = defineEmits(["tile-open", "tile-moved", "tile-add", "tile-exec", "tile-slider", "ctx-tile", "ctx-empty"]);

// tiles whose stored mode is "toggle" flip to their second state when
// tapped, exactly like the tablet client; the flip lives for the editor
// session only (live state pushes win once they arrive)
const activeTiles = reactive(new Set());

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
    // clamp to W-w / H-h so a multi-cell tile never hangs over the edge
    left: `${clamp(x, 0, Math.max(0, props.board.width - w)) * cell.value}px`,
    top: `${clamp(y, 0, Math.max(0, props.board.height - h)) * row.value}px`,
    width: `${clamp(w, 1, props.board.width) * cell.value}px`,
    height: `${clamp(h, 1, props.board.height) * row.value}px`,
    zIndex: d ? 10 : 1,
  };
}

// every unoccupied grid position renders as a visible empty slot, like the
// original editor; touch mode skips them entirely - they are edit
// affordances, and the CSS used to merely hide them
const emptyCells = computed(() => {
  const w = Number(props.board.width);
  const h = Number(props.board.height);
  // allocation guard (012 C4): a junk board (import/hand-edit) with huge
  // or fractional dimensions must not allocate W*H cells; bounds match
  // the backend's MAX_BOARD_DIM
  if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || h < 1) return [];
  if (w > MAX_BOARD_DIM || h > MAX_BOARD_DIM) return [];
  const occupied = new Set();
  for (const t of props.board.buttons) {
    // per-tile loops are bounded to the grid as well - junk tile w/h
    // must not blow the set up either
    const tw = Math.max(1, Math.min(Number(t.w) || 1, w));
    const th = Math.max(1, Math.min(Number(t.h) || 1, h));
    for (let dy = 0; dy < th && t.y + dy < h; dy++) {
      for (let dx = 0; dx < tw && t.x + dx < w; dx++) {
        if (t.x + dx >= 0 && t.y + dy >= 0) occupied.add(`${t.x + dx},${t.y + dy}`);
      }
    }
  }
  const cells = [];
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
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
    window.removeEventListener("pointercancel", onUp);
    const d = drag.value;
    drag.value = null;
    if (!d) return;
    if (d.mode === "move") {
      // clamp to W-w / H-h (012 C4): the top-left cell alone must stay in
      // bounds even for multi-cell tiles
      const x = clamp(tile.x + d.cx, 0, Math.max(0, props.board.width - tile.w));
      const y = clamp(tile.y + d.cy, 0, Math.max(0, props.board.height - tile.h));
      if (x !== tile.x || y !== tile.y) emit("tile-moved", tile, x, y, tile.w, tile.h);
    } else {
      const w = clamp(tile.w + d.cw, 1, props.board.width - tile.x);
      const h = clamp(tile.h + d.ch, 1, props.board.height - tile.y);
      if (w !== tile.w || h !== tile.h) emit("tile-moved", tile, tile.x, tile.y, w, h);
    }
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

function onTileTap(tile) {
  if (!props.touch) return;
  if (tile.mode === "slider") return;
  if (tile.mode === "toggle") {
    // flip to the second state like the tablet client does
    if (activeTiles.has(tile.id)) activeTiles.delete(tile.id);
    else activeTiles.add(tile.id);
  }
  emit("tile-exec", tile);
}

// Arrow keys move focus to the nearest tile in the pressed direction
// (keyboard operability, 012 lower-priority); Enter/Space activation lives
// in TileCell. Candidates are compared by cell-center distance so
// multi-cell tiles navigate sensibly, and tiles straight ahead win over
// diagonal ones.
const ARROW_DIRS = {
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
};
const gridEl = ref(null);

function onGridKeydown(event) {
  const dir = ARROW_DIRS[event.key];
  if (!dir) return;
  if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
  const slot = event.target.closest?.(".tile-slot");
  if (!slot || slot.dataset.tileId === undefined) return;
  const current = props.board.buttons.find(
    (t) => t.id !== null && String(t.id) === slot.dataset.tileId
  );
  if (!current) return;
  const [dx, dy] = dir;
  const cx = current.x + current.w / 2;
  const cy = current.y + current.h / 2;
  let best = null;
  let bestScore = Infinity;
  for (const t of props.board.buttons) {
    if (t.id === null || t.id === current.id) continue;
    const tx = t.x + t.w / 2 - cx;
    const ty = t.y + t.h / 2 - cy;
    if (dx && tx * dx <= 0) continue; // not in the pressed direction
    if (dy && ty * dy <= 0) continue;
    const along = dx ? Math.abs(tx) : Math.abs(ty);
    const across = dx ? Math.abs(ty) : Math.abs(tx);
    const score = along + across * 2;
    if (score < bestScore) {
      bestScore = score;
      best = t;
    }
  }
  if (!best) return;
  // keep the scroll container still; focus() scrolls the target into view
  event.preventDefault();
  gridEl.value?.querySelector(`[data-tile-id="${best.id}"] .tile`)?.focus();
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
      ref="gridEl"
      class="board-grid"
      :class="{ touch }"
      :style="gridStyle"
      @click="onGridClick"
      @contextmenu.prevent="onGridContext"
      @keydown="onGridKeydown"
    >
      <template v-if="!touch">
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
          @click.stop="$emit('tile-add', { x: c.x, y: c.y })"
          @contextmenu.prevent.stop="onEmptyContext({ x: c.x, y: c.y }, $event)"
        >
          <div class="empty-cell"></div>
        </div>
      </template>
      <TileCell
        v-for="tile in board.buttons"
        :key="tile.id ?? `fill-${tile.x}-${tile.y}`"
        class="tile-slot"
        :style="tileStyle(tile)"
        :data-tile-id="tile.id"
        :tile="tile"
        :touch="touch"
        :type-meta="typeMeta"
        :custom-values="customValues"
        :app-states="appStates"
        :board-names="boardNames"
        :active="activeTiles.has(tile.id)"
        :dragging="Boolean(drag && drag.tile.id === tile.id)"
        @open="$emit('tile-open', tile)"
        @ctx="onTileContext(tile, $event)"
        @down="startDrag(tile, 'move', $event)"
        @tap="onTileTap(tile)"
        @slider="(value) => $emit('tile-slider', tile, value)"
        @resize="startDrag(tile, 'resize', $event)"
      />
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
/* lands on each TileCell root via attr fallthrough */
.tile-slot { position: absolute; padding: 5px; }
.tile-slot:has(> .empty-cell) { cursor: pointer; }
.touch .tile-slot { cursor: default; }
.empty-cell {
  width: 100%;
  height: 100%;
  border-radius: 8px;
  background: rgba(0, 0, 0, 0.09);
  transition: background 120ms ease-out;
}
.tile-slot:hover > .empty-cell { background: rgba(0, 0, 0, 0.16); }
</style>
