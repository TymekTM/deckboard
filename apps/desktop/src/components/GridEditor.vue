<script setup>
import { computed, reactive, ref } from "vue";
import { CELL_W, ROW_H, MAX_BOARD_DIM, clamp } from "../catalog";
import TileCell from "./TileCell.vue";

// Edit-mode grid: drag to move, corner handle to resize, double-click to
// edit, empty-cell click to add. Multi-select: Ctrl/Shift click and rubber-band.
// Touch mode: tap runs the tile.
const props = defineProps({
  board: { type: Object, required: true },
  touch: { type: Boolean, default: false },
  zoom: { type: Number, default: 1 },
  // type -> {icon, color, mode, dual} fallbacks from the action catalog
  typeMeta: { type: Object, default: () => ({}) },
  // live state pushes (APP_CUSTOM_VALUE), see applyStatusUpdate
  customValues: { type: Object, default: () => ({}) },
  // board id -> name, for board-switch tiles without a title
  boardNames: { type: Object, default: () => ({}) },
  selectedIds: { type: Object, default: () => new Set() },
});
const emit = defineEmits([
  "tile-open",
  "tile-moved",
  "tile-add",
  "tile-exec",
  "tile-slider",
  "ctx-tile",
  "ctx-empty",
  "select-tile",
  "select-tiles",
  "clear-selection",
  "tiles-bulk-moved",
  "move-refused",
]);

// tiles whose stored mode is "toggle" flip to their second state when
// tapped, exactly like the tablet client; the flip lives for the editor
// session only (live state pushes win once they arrive)
const activeTiles = reactive(new Set());

const drag = ref(null); // {tile, mode:'move'|'resize', isGroupDrag, startX, startY, cellW, cellH, cx, cy, cw, ch}

const rubberBand = reactive({
  active: false,
  startX: 0,
  startY: 0,
  curX: 0,
  curY: 0,
});

const rubberBandStyle = computed(() => {
  const minX = Math.min(rubberBand.startX, rubberBand.curX);
  const maxX = Math.max(rubberBand.startX, rubberBand.curX);
  const minY = Math.min(rubberBand.startY, rubberBand.curY);
  const maxY = Math.max(rubberBand.startY, rubberBand.curY);
  return {
    left: `${minX}px`,
    top: `${minY}px`,
    width: `${maxX - minX}px`,
    height: `${maxY - minY}px`,
  };
});

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

const baseStyles = computed(() => {
  const map = new Map();
  for (const t of props.board.buttons) {
    map.set(t, {
      left: `${clamp(t.x, 0, Math.max(0, props.board.width - t.w)) * cell.value}px`,
      top: `${clamp(t.y, 0, Math.max(0, props.board.height - t.h)) * row.value}px`,
      width: `${clamp(t.w, 1, props.board.width) * cell.value}px`,
      height: `${clamp(t.h, 1, props.board.height) * row.value}px`,
      zIndex: 1,
    });
  }
  return map;
});

function tileStyle(tile) {
  const d = drag.value;
  const base = baseStyles.value.get(tile);
  if (!d) return base;

  const isSelected = props.selectedIds.has(tile.id);
  const isTarget = d.tile.id === tile.id;
  const inGroup = d.isGroupDrag && isSelected;

  if (!isTarget && !inGroup) return base;

  let x = tile.x;
  let y = tile.y;
  let w = tile.w;
  let h = tile.h;

  if (d.mode === "move") {
    x += d.cx;
    y += d.cy;
  } else if (isTarget) {
    w = Math.max(1, tile.w + d.cw);
    h = Math.max(1, tile.h + d.ch);
  }

  return {
    ...base,
    left: `${(d.isGroupDrag ? x : clamp(x, 0, Math.max(0, props.board.width - w))) * cell.value}px`,
    top: `${(d.isGroupDrag ? y : clamp(y, 0, Math.max(0, props.board.height - h))) * row.value}px`,
    width: `${clamp(w, 1, props.board.width) * cell.value}px`,
    height: `${clamp(h, 1, props.board.height) * row.value}px`,
    zIndex: 10,
  };
}

const emptyCells = computed(() => {
  const w = Number(props.board.width);
  const h = Number(props.board.height);
  if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || h < 1) return [];
  if (w > MAX_BOARD_DIM || h > MAX_BOARD_DIM) return [];
  const occupied = new Set();
  for (const t of props.board.buttons) {
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
  if (event.button !== 0) return;
  event.preventDefault();

  if (mode === "move") {
    if (event.ctrlKey || event.shiftKey) {
      emit("select-tile", { tile, additive: true });
    } else if (!props.selectedIds.has(tile.id)) {
      emit("select-tile", { tile, additive: false });
    }
  }

  const isGroupDrag =
    mode === "move" && props.selectedIds.has(tile.id) && props.selectedIds.size > 1;

  const startX = event.clientX;
  const startY = event.clientY;
  drag.value = {
    tile,
    mode,
    isGroupDrag,
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
      if (d.isGroupDrag) {
        if (d.cx === 0 && d.cy === 0) {
          if (!event.ctrlKey && !event.shiftKey) {
            emit("select-tile", { tile, additive: false });
          }
          return;
        }

        const selectedTiles = props.board.buttons.filter((b) => props.selectedIds.has(b.id));
        const unselectedTiles = props.board.buttons.filter((b) => !props.selectedIds.has(b.id));

        let invalidReason = "";
        for (const t of selectedTiles) {
          const nx = t.x + d.cx;
          const ny = t.y + d.cy;
          if (nx < 0 || ny < 0 || nx + t.w > props.board.width || ny + t.h > props.board.height) {
            invalidReason = "Nie można przesunąć: kafelki wychodziłyby poza siatkę";
            break;
          }
          for (const u of unselectedTiles) {
            const overlap = !(nx + t.w <= u.x || nx >= u.x + u.w || ny + t.h <= u.y || ny >= u.y + u.h);
            if (overlap) {
              invalidReason = "Nie można przesunąć: kafelki nakładałyby się na inne kafelki";
              break;
            }
          }
          if (invalidReason) break;
        }

        if (invalidReason) {
          emit("move-refused", invalidReason);
          return;
        }

        const moves = selectedTiles.map((t) => ({
          tile: t,
          prevGeom: { x: t.x, y: t.y, w: t.w, h: t.h },
          nextGeom: { x: t.x + d.cx, y: t.y + d.cy, w: t.w, h: t.h },
        }));
        emit("tiles-bulk-moved", moves);
      } else {
        const x = clamp(tile.x + d.cx, 0, Math.max(0, props.board.width - tile.w));
        const y = clamp(tile.y + d.cy, 0, Math.max(0, props.board.height - tile.h));
        if (x !== tile.x || y !== tile.y) emit("tile-moved", tile, x, y, tile.w, tile.h);
      }
    } else {
      const w = clamp(tile.w + d.cw, 1, Math.max(1, props.board.width - tile.x));
      const h = clamp(tile.h + d.ch, 1, Math.max(1, props.board.height - tile.y));
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
    if (activeTiles.has(tile.id)) activeTiles.delete(tile.id);
    else activeTiles.add(tile.id);
  }
  emit("tile-exec", tile);
}

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
    if (dx && tx * dx <= 0) continue;
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
  event.preventDefault();
  gridEl.value?.querySelector(`[data-tile-id="${best.id}"] .tile`)?.focus();
}

function onTileContext(tile, event) {
  if (props.touch) {
    emit("tile-open", tile);
    return;
  }
  if (!props.selectedIds.has(tile.id)) {
    emit("select-tile", { tile, additive: false });
  }
  emit("ctx-tile", tile, event);
}

function onEmptyContext(pos, event) {
  if (props.touch) return;
  emit("ctx-empty", pos, event);
}

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

function onGridPointerDown(event) {
  if (props.touch || event.button !== 0) return;
  if (event.target.closest(".tile")) return;

  const rect = gridEl.value?.getBoundingClientRect();
  if (!rect) return;
  const startX = event.clientX - rect.left;
  const startY = event.clientY - rect.top;

  rubberBand.startX = startX;
  rubberBand.startY = startY;
  rubberBand.curX = startX;
  rubberBand.curY = startY;
  rubberBand.active = false;

  const onMove = (e) => {
    const curX = e.clientX - rect.left;
    const curY = e.clientY - rect.top;
    rubberBand.curX = curX;
    rubberBand.curY = curY;
    if (Math.hypot(curX - rubberBand.startX, curY - rubberBand.startY) > 6) {
      rubberBand.active = true;
    }
  };

  const onUp = (e) => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", onUp);

    if (rubberBand.active) {
      const minX = Math.min(rubberBand.startX, rubberBand.curX);
      const maxX = Math.max(rubberBand.startX, rubberBand.curX);
      const minY = Math.min(rubberBand.startY, rubberBand.curY);
      const maxY = Math.max(rubberBand.startY, rubberBand.curY);

      const intersecting = [];
      for (const t of props.board.buttons) {
        if (t.id === null) continue;
        const tLeft = t.x * cell.value;
        const tRight = (t.x + t.w) * cell.value;
        const tTop = t.y * row.value;
        const tBottom = (t.y + t.h) * row.value;
        const hit = !(tRight < minX || tLeft > maxX || tBottom < minY || tTop > maxY);
        if (hit) intersecting.push(t.id);
      }
      emit("select-tiles", {
        ids: intersecting,
        additive: e.ctrlKey || e.shiftKey,
      });
      rubberBand.active = false;
    } else {
      if (!e.ctrlKey && !e.shiftKey) {
        emit("clear-selection");
      }
    }
  };

  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("pointercancel", onUp);
}

function onGridClick(event) {
  if (props.touch) return;
  if (event.target !== event.currentTarget) return;
  const rect = event.currentTarget.getBoundingClientRect();
  const x = clamp(Math.floor((event.clientX - rect.left) / cell.value), 0, props.board.width - 1);
  const y = clamp(Math.floor((event.clientY - rect.top) / row.value), 0, props.board.height - 1);
  const occupied = props.board.buttons.some(
    (t) => x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h
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
      @pointerdown="onGridPointerDown"
      @click="onGridClick"
      @contextmenu.prevent="onGridContext"
      @keydown="onGridKeydown"
    >
      <div
        v-if="rubberBand.active"
        class="rubber-band"
        :style="rubberBandStyle"
      ></div>
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
        :board-names="boardNames"
        :selected="selectedIds.has(tile.id)"
        :active="activeTiles.has(tile.id)"
        :dragging="Boolean(drag && (drag.tile.id === tile.id || (drag.isGroupDrag && selectedIds.has(tile.id))))"
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
.rubber-band {
  position: absolute;
  border: 1.5px solid var(--accent, #1abc9c);
  background: rgba(26, 188, 156, 0.16);
  border-radius: 4px;
  pointer-events: none;
  z-index: 40;
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
