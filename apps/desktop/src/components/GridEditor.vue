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

// Monochrome brand marks (24x24, currentColor) for the agent providers:
// official paths from Simple Icons (Z.ai, Claude, OpenAI, OpenCode) and
// Antigravity's mark traced from its official icon asset.
const PROVIDER_GLYPHS = {
  zcode: "M12.606 1.806l-1.677 2.388c-0.258 0.374-0.697 0.606-1.161 0.606h-9.162V1.794C0.594 1.806 12.606 1.806 12.606 1.806zM24 1.806L9.6 22.206 0 22.206 14.4 1.806zM11.394 22.206l1.69-2.4c0.258-0.374 0.697-0.606 1.161-0.606h9.149v3.006H11.394z",
  claude: "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z",
  codex: "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z",
  opencode: "M22 24H2V0h20zM17 4.8H7v14.4h10z",
  antigravity: "M0.0 21.11 1.62 19.49 3.25 16.78 4.69 12.81 6.68 5.41 8.12 2.35 9.38 0.9 10.83 0.18 12.63 0.0 14.26 0.54 15.34 1.44 16.96 4.15 20.21 15.16 21.65 18.23 23.1 19.85 23.1 20.21 24.0 20.93 24.0 22.02 22.38 22.2 21.11 21.11 20.75 21.11 18.77 18.95 15.7 13.71 14.8 12.81 13.35 12.09 10.83 12.09 9.38 12.81 7.76 14.62 5.23 19.13 2.71 21.65 1.8 22.2 0.54 22.38 0.0 22.02Z",
};

function providerSvg(provider) {
  const path = PROVIDER_GLYPHS[provider];
  if (!path) return "";
  return `<svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor" aria-hidden="true"><path d="${path}"/></svg>`;
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
