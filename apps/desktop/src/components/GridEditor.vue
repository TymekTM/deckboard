<script setup>
import { computed, ref } from "vue";
import { CELL_W, ROW_H } from "../catalog";

// Edit-mode grid: drag to move, corner handle to resize, double-click to
// edit, empty-cell click to add. Touch mode: tap runs the tile.
const props = defineProps({
  board: { type: Object, required: true },
  touch: { type: Boolean, default: false },
});
const emit = defineEmits(["tile-open", "tile-moved", "tile-add", "tile-add-default", "tile-exec"]);

const drag = ref(null); // {tile, mode:'move'|'resize', dx, dy, pointerId}

const gridStyle = computed(() => ({
  width: `${props.board.width * CELL_W}px`,
  height: `${props.board.height * ROW_H}px`,
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
    left: `${clamp(x, 0, props.board.width - 1) * CELL_W}px`,
    top: `${clamp(y, 0, props.board.height - 1) * ROW_H}px`,
    width: `${clamp(w, 1, props.board.width) * CELL_W}px`,
    height: `${clamp(h, 1, props.board.height) * ROW_H}px`,
    zIndex: d ? 10 : 1,
  };
}

function clamp(v, min, max) {
  return Math.min(max, Math.max(min, v));
}

function startDrag(tile, mode, event) {
  if (props.touch) return;
  event.preventDefault();
  const startX = event.clientX;
  const startY = event.clientY;
  drag.value = {
    tile,
    mode,
    startX,
    startY,
    cellW: CELL_W,
    cellH: ROW_H,
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
  if (props.touch) emit("tile-exec", tile);
}

function onGridClick(event) {
  if (props.touch) return;
  if (event.target !== event.currentTarget) return;
  // empty-cell click opens the add-tile flow at that cell
  const rect = event.currentTarget.getBoundingClientRect();
  const x = clamp(Math.floor((event.clientX - rect.left) / CELL_W), 0, props.board.width - 1);
  const y = clamp(Math.floor((event.clientY - rect.top) / ROW_H), 0, props.board.height - 1);
  const occupied = props.board.buttons.some(
    (t) =>
      x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h
  );
  if (!occupied) emit("tile-add", { x, y });
}
</script>

<template>
  <div class="grid-wrap">
    <div class="toolbar">
      <span class="board-name">{{ board.name }}</span>
      <span class="dim">double-click a tile to edit - drag to move - corner to resize</span>
      <button v-if="!touch" class="add-btn" @click="$emit('tile-add-default')">
        <i class="fas fa-plus"></i> Add tile
      </button>
    </div>
    <div class="board-scroll">
      <div
        class="board-grid"
        :class="{ touch }"
        :style="gridStyle"
        @click="onGridClick"
      >
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
              background: tile.color || '#2c3e50',
              borderColor: tile.border_color || 'transparent',
              borderRadius: tile.shape === 1 ? '50%' : '6px',
            }"
            @dblclick="!touch && $emit('tile-open', tile)"
            @pointerdown="startDrag(tile, 'move', $event)"
            @click.stop="onTileTap(tile)"
          >
            <img v-if="tile.img" class="tile-img" :src="tile.img" alt="" />
            <i
              v-if="tile.icon"
              class="tile-icon"
              :class="'fas fa-' + tile.icon"
              :style="{ color: tile.icon_color || '#ffffff' }"
            ></i>
            <span
              v-if="tile.title"
              class="tile-title"
              :style="{
                color: tile.title_color || '#ffffff',
                background: tile.title_box_color || 'transparent',
              }"
              >{{ tile.title }}</span
            >
            <span v-if="tile.mode === 'slider'" class="tile-badge">
              <i class="fas fa-sliders-h"></i>
            </span>
            <span
              v-if="!touch"
              class="resize-handle"
              @pointerdown.stop="startDrag(tile, 'resize', $event)"
            ></span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.grid-wrap { display: flex; flex-direction: column; height: 100%; }
.toolbar {
  display: flex;
  gap: 12px;
  align-items: baseline;
  padding: 10px 16px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
}
.board-name { font-size: 16px; font-weight: 600; }
.dim { font-size: 11.5px; color: var(--text-muted); }
.board-scroll { flex: 1; overflow: auto; padding: 16px; }
.board-grid {
  position: relative;
  display: block;
  background-color: var(--surface);
  background-size: cover;
  outline: 1px solid var(--border);
}
.tile-slot { position: absolute; padding: 5px; }
.tile {
  position: relative;
  width: 100%;
  height: 100%;
  border: 2px solid transparent;
  border-radius: 6px;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: grab;
}
.touch .tile { cursor: pointer; }
.tile.dragging { opacity: 0.75; cursor: grabbing; box-shadow: 0 6px 18px rgba(0,0,0,0.5); }
.tile-img {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.tile-icon { font-size: 30px; pointer-events: none; }
.tile-title {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  padding: 2px 5px;
  font-size: 11.5px;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  pointer-events: none;
}
.tile-badge {
  position: absolute;
  top: 4px;
  right: 6px;
  font-size: 12px;
  opacity: 0.85;
}
.resize-handle {
  position: absolute;
  right: 0;
  bottom: 0;
  width: 16px;
  height: 16px;
  cursor: nwse-resize;
  background: linear-gradient(135deg, transparent 50%, rgba(255, 255, 255, 0.55) 50%);
}
</style>
