<script setup>
import { computed, onMounted, onUnmounted, reactive, ref, watch } from "vue";
import { listen } from "@tauri-apps/api/event";
import { save, open, ask } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { CATALOG, clamp } from "./catalog";
import GridEditor from "./components/GridEditor.vue";
import EditTileModal from "./components/EditTileModal.vue";
import BoardModal from "./components/BoardModal.vue";
import SettingsOverlay from "./components/SettingsOverlay.vue";
import OperatorAskModal from "./components/OperatorAskModal.vue";
import {
  createEditorHistory,
  tileCreateCommand,
  tileEditCommand,
  tileDeleteCommand,
  tileMoveCommand,
  boardCreateCommand,
  boardEditCommand,
  boardDeleteCommand,
  boardClearCommand,
  tileBulkMoveCommand,
  tileBulkDeleteCommand,
  tileBulkCreateCommand,
  tileBulkEditCommand,
} from "./editorHistory";

const boards = ref([]);
const currentId = ref(null);
const status = ref({ dbOk: false, port: 0, clients: 0, version: "" });
const touchMode = ref(false);
const touchBoardId = ref(null);
const knownInputs = ref([]);
const audioDevices = ref([]);
const history = createEditorHistory(100);
const canUndo = ref(false);
const canRedo = ref(false);

function updateHistoryFlags() {
  canUndo.value = history.canUndo();
  canRedo.value = history.canRedo();
}

async function doUndo() {
  if (!history.canUndo()) return;
  try {
    await history.undo(api);
    await loadBoards();
    updateHistoryFlags();
  } catch (e) {
    flashError("Cofanie operacji nie powiodło się", e);
  }
}


const selectedTileIds = ref(new Set());
const bulkColorModal = ref(null);

watch(currentId, () => {
  selectedTileIds.value = new Set();
});

function onSelectTile({ tile, additive }) {
  const next = new Set(selectedTileIds.value);
  if (additive) {
    if (next.has(tile.id)) next.delete(tile.id);
    else next.add(tile.id);
  } else {
    next.clear();
    next.add(tile.id);
  }
  selectedTileIds.value = next;
}

function onSelectTiles({ ids, additive }) {
  const next = additive ? new Set(selectedTileIds.value) : new Set();
  for (const id of ids) next.add(id);
  selectedTileIds.value = next;
}

function onClearSelection() {
  selectedTileIds.value = new Set();
}

function onMoveRefused(reason) {
  flashError(reason);
}

async function onTilesBulkMoved(moves) {
  try {
    for (const m of moves) {
      await api.moveButton(m.tile.id, m.tile.board_id, m.nextGeom.x, m.nextGeom.y, m.nextGeom.w, m.nextGeom.h);
    }
    history.push(
      tileBulkMoveCommand({
        moves: moves.map((m) => ({
          id: m.tile.id,
          boardId: m.tile.board_id,
          prevGeom: m.prevGeom,
          nextGeom: m.nextGeom,
        })),
      })
    );
    updateHistoryFlags();
    await loadBoards();
  } catch (e) {
    flashError("Przesuwanie grupy kafli nie powiodło się", e);
    await loadBoards();
  }
}

async function deleteSelectedTiles() {
  const ids = Array.from(selectedTileIds.value);
  if (!ids.length) return;
  const board = currentBoard.value;
  if (!board) return;
  const tilesToDelete = board.buttons.filter((b) => ids.includes(b.id));
  if (!tilesToDelete.length) return;

  const count = tilesToDelete.length;
  const ok = await ask(
    count === 1
      ? `Usunąć kafel "${tilesToDelete[0].title || tilesToDelete[0].type}"?`
      : `Usunąć ${count} zaznaczonych kafli?`,
    {
      title: "Usuń kafle",
      kind: "warning",
    }
  );
  if (!ok) return;

  try {
    for (const t of tilesToDelete) {
      await api.deleteButton(t.id, t.board_id);
    }
    history.push(tileBulkDeleteCommand({ tiles: tilesToDelete }));
    updateHistoryFlags();
    selectedTileIds.value = new Set();
    await loadBoards();
  } catch (e) {
    flashError("Usuwanie kafli nie powiodło się", e);
    await loadBoards();
  }
}

function copySelectedTiles() {
  const board = currentBoard.value;
  if (!board) return;
  const ids = Array.from(selectedTileIds.value);
  const tilesToCopy = board.buttons.filter((b) => ids.includes(b.id));
  if (!tilesToCopy.length) return;
  tileClipboard.value = tilesToCopy.map((t) => ({ ...t }));
}

async function duplicateTiles(tiles) {
  const board = currentBoard.value;
  if (!board || !tiles.length) return;
  const minX = Math.min(...tiles.map((t) => t.x));
  const minY = Math.min(...tiles.map((t) => t.y));
  const maxX = Math.max(...tiles.map((t) => t.x + t.w));
  const maxY = Math.max(...tiles.map((t) => t.y + t.h));
  const groupW = maxX - minX;
  const groupH = maxY - minY;

  let offsetX = 1;
  let offsetY = 1;
  let canOffset = true;
  for (const t of tiles) {
    const tx = t.x + offsetX;
    const ty = t.y + offsetY;
    if (tx + t.w > board.width || ty + t.h > board.height) {
      canOffset = false;
      break;
    }
    for (const other of board.buttons) {
      const overlap = !(tx + t.w <= other.x || tx >= other.x + other.w || ty + t.h <= other.y || ty >= other.y + other.h);
      if (overlap) {
        canOffset = false;
        break;
      }
    }
    if (!canOffset) break;
  }

  if (!canOffset) {
    let found = false;
    for (let y = 0; y <= board.height - groupH; y++) {
      for (let x = 0; x <= board.width - groupW; x++) {
        let collides = false;
        for (const t of tiles) {
          const targetX = x + (t.x - minX);
          const targetY = y + (t.y - minY);
          for (const other of board.buttons) {
            const overlap = !(targetX + t.w <= other.x || targetX >= other.x + other.w || targetY + t.h <= other.y || targetY >= other.y + other.h);
            if (overlap) {
              collides = true;
              break;
            }
          }
          if (collides) break;
        }
        if (!collides) {
          offsetX = x - minX;
          offsetY = y - minY;
          found = true;
          break;
        }
      }
      if (found) break;
    }
  }

  const createdTiles = [];
  const newSelection = new Set();
  try {
    for (const t of tiles) {
      const tx = clamp(t.x + offsetX, 0, Math.max(0, board.width - t.w));
      const ty = clamp(t.y + offsetY, 0, Math.max(0, board.height - t.h));
      const newId = await api.createButton(
        board.id,
        t.type || "key",
        t.mode || "button",
        tx,
        ty
      );
      const snapshot = {
        ...t,
        id: newId,
        board_id: board.id,
        x: tx,
        y: ty,
      };
      await api.updateButton(snapshot);
      createdTiles.push({ id: newId, snapshot });
      newSelection.add(newId);
    }
    history.push(tileBulkCreateCommand({ createdTiles, boardId: board.id }));
    updateHistoryFlags();
    selectedTileIds.value = newSelection;
    await loadBoards();
  } catch (e) {
    flashError("Duplikowanie kafli nie powiodło się", e);
    await loadBoards();
  }
}


async function pasteClipboardTiles() {
  if (!tileClipboard.value) return;
  const board = currentBoard.value;
  if (!board) return;
  const tiles = Array.isArray(tileClipboard.value) ? tileClipboard.value : [tileClipboard.value];
  if (!tiles.length) return;

  const minX = Math.min(...tiles.map((t) => t.x));
  const minY = Math.min(...tiles.map((t) => t.y));
  const maxX = Math.max(...tiles.map((t) => t.x + t.w));
  const maxY = Math.max(...tiles.map((t) => t.y + t.h));
  const groupW = maxX - minX;
  const groupH = maxY - minY;

  let offsetX = 0;
  let offsetY = 0;
  let found = false;
  for (let y = 0; y <= Math.max(0, board.height - groupH); y++) {
    for (let x = 0; x <= Math.max(0, board.width - groupW); x++) {
      let collides = false;
      for (const t of tiles) {
        const targetX = x + (t.x - minX);
        const targetY = y + (t.y - minY);
        for (const other of board.buttons) {
          const overlap = !(targetX + t.w <= other.x || targetX >= other.x + other.w || targetY + t.h <= other.y || targetY >= other.y + other.h);
          if (overlap) {
            collides = true;
            break;
          }
        }
        if (collides) break;
      }
      if (!collides) {
        offsetX = x - minX;
        offsetY = y - minY;
        found = true;
        break;
      }
    }
    if (found) break;
  }

  const createdTiles = [];
  const newSelection = new Set();
  try {
    for (const t of tiles) {
      const tx = clamp(t.x + offsetX, 0, Math.max(0, board.width - t.w));
      const ty = clamp(t.y + offsetY, 0, Math.max(0, board.height - t.h));
      const newId = await api.createButton(
        board.id,
        t.type || "key",
        t.mode || "button",
        tx,
        ty
      );
      const snapshot = {
        ...t,
        id: newId,
        board_id: board.id,
        x: tx,
        y: ty,
      };
      await api.updateButton(snapshot);
      createdTiles.push({ id: newId, snapshot });
      newSelection.add(newId);
    }
    history.push(tileBulkCreateCommand({ createdTiles, boardId: board.id }));
    updateHistoryFlags();
    selectedTileIds.value = newSelection;
    await loadBoards();
  } catch (e) {
    flashError("Wklejanie kafli nie powiodło się", e);
    await loadBoards();
  }
}

async function nudgeSelection(dx, dy) {
  const ids = selectedTileIds.value;
  if (!ids.size) return;
  const board = currentBoard.value;
  if (!board) return;

  const selectedTiles = board.buttons.filter((b) => ids.has(b.id));
  const unselectedTiles = board.buttons.filter((b) => !ids.has(b.id));

  let invalid = false;
  for (const t of selectedTiles) {
    const nx = t.x + dx;
    const ny = t.y + dy;
    if (nx < 0 || ny < 0 || nx + t.w > board.width || ny + t.h > board.height) {
      invalid = true;
      break;
    }
    for (const u of unselectedTiles) {
      const overlap = !(nx + t.w <= u.x || nx >= u.x + u.w || ny + t.h <= u.y || ny >= u.y + u.h);
      if (overlap) {
        invalid = true;
        break;
      }
    }
    if (invalid) break;
  }

  if (invalid) {
    flashError("Nie można przesunąć: kafelki wychodziłyby poza siatkę lub nachodziły na inne");
    return;
  }

  const moves = selectedTiles.map((t) => ({
    tile: t,
    prevGeom: { x: t.x, y: t.y, w: t.w, h: t.h },
    nextGeom: { x: t.x + dx, y: t.y + dy, w: t.w, h: t.h },
  }));
  await onTilesBulkMoved(moves);
}

async function applyBulkColor(color) {
  const ids = Array.from(selectedTileIds.value);
  if (!ids.length) return;
  const board = currentBoard.value;
  if (!board) return;
  const tilesToUpdate = board.buttons.filter((b) => ids.includes(b.id));
  const edits = [];
  try {
    for (const t of tilesToUpdate) {
      const prevSnapshot = { ...t };
      const nextSnapshot = { ...t, color };
      await api.updateButton(nextSnapshot);
      edits.push({ id: t.id, prevSnapshot, nextSnapshot });
    }
    history.push(tileBulkEditCommand({ edits }));
    updateHistoryFlags();
    bulkColorModal.value = null;
    await loadBoards();
  } catch (e) {
    flashError("Zmiana koloru kafli nie powiodła się", e);
    await loadBoards();
  }
}

async function doRedo() {
  if (!history.canRedo()) return;
  try {
    await history.redo(api);
    await loadBoards();
    updateHistoryFlags();
  } catch (e) {
    flashError("Ponawianie operacji nie powiodło się", e);
  }
}


// Live state mirror of the original client: customValues holds pushed
// values. Every producer (extensions, sysinfo, aidev, the audio watcher,
// device ids via THIRD_PARTY_APP) lands here - the original's per-app
// state lane (APP_OBS etc.) has no emitter, so no appStates map exists
// (DESK-09).
const customValues = reactive({});

// While the window is hidden to tray, state pushes are buffered per app
// and applied on the next show - writing to the reactive maps would still
// run the whole render pipeline behind a hidden window. Sparklines simply
// miss the hidden samples, like the original's backgrounded webview; the
// Rust emitters keep tablet clients correct either way.
let hidden = document.hidden;
let pendingStatus = {}; // app -> data of pushes seen while hidden

function onStatusUpdate(payload) {
  const data = payload?.data;
  if (!data || typeof data !== "object") return;
  if (hidden) {
    pendingStatus[payload.app] = { ...pendingStatus[payload.app], ...data };
    return;
  }
  applyStatusUpdate(payload);
}

function onVisibilityChange() {
  hidden = document.hidden;
  if (hidden) return;
  const buffered = pendingStatus;
  pendingStatus = {};
  for (const [app, data] of Object.entries(buffered)) {
    applyStatusUpdate({ app, data });
  }
}

function applyStatusUpdate(payload) {
  const data = payload?.data;
  if (!data || typeof data !== "object") return;
  switch (payload.app) {
    case "APP_CUSTOM_VALUE":
      mergeCustomValues(data);
      break;
    case "THIRD_PARTY_APP":
      // device id strings; keep them in customValues so the existing
      // speaker-device watch binding (stateActive) picks them up
      mergeCustomValues(data);
      break;
    default:
      break;
  }
}

// Pushed payloads are a few KB at most, so a stringify compare is the
// simplest way to keep object identity stable for unchanged values.
function jsonEqual(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function mergeCustomValues(data) {
  for (const [key, value] of Object.entries(data)) {
    if (typeof value !== "object" || value === null) {
      customValues[key] = value;
    } else if (typeof value.value === "number") {
      // graph-style payload: keep the last 10 readings for the sparkline
      // and pass the rest through (title, suffix, per-provider rows)
      const prev = customValues[key];
      if (
        prev &&
        Array.isArray(prev.values) &&
        jsonEqual({ ...value, values: [] }, { ...prev, values: [] })
      ) {
        // same reading and meta: append the sample in place so the entry
        // keeps its identity and only its readers re-render
        prev.values.push(value.value);
        if (prev.values.length > 10) prev.values.shift();
      } else {
        const values = [...(prev?.values ?? []), value.value].slice(-10);
        customValues[key] = { ...value, values };
      }
    } else {
      // status displays replace their whole snapshot; sample history
      // makes no sense for a live row list
      if (!jsonEqual(value, customValues[key])) customValues[key] = value;
    }
  }
}

// type -> {icon, color, mode} fallbacks: static catalog + extensions
const typeMeta = computed(() => {
  const map = {};
  for (const e of CATALOG) {
    if (e.value) {
      map[e.value] = {
        label: e.label || e.value,
        icon: e.icon || "",
        color: e.color || "",
        mode: e.mode || "",
      };
    }
  }
  for (const i of knownInputs.value) {
    map[i.value] = {
      label: i.label || i.value,
      icon: i.icon || "",
      color: i.color || "",
      mode: i.mode || "",
    };
  }
  return map;
});

const settingsOpen = ref(false);

// custom right-click context menu, replacing the WebView2 default menu
// everywhere: {x, y, items: [{label, icon, danger, run}]}
const contextMenu = ref(null);

// transient failure feedback (012 lower-priority): api.js wrappers reject
// silently otherwise, so a failed save/import leaves the user guessing
const flash = ref(null);
let flashTimer = null;

function flashError(message, error) {
  if (error) console.error(message, error);
  flash.value = error ? `${message}: ${error}` : message;
  if (flashTimer) clearTimeout(flashTimer);
  flashTimer = setTimeout(() => (flash.value = null), 6000);
}

function openContextMenu(event, items) {
  contextMenu.value = {
    x: Math.min(event.clientX, window.innerWidth - 216),
    y: Math.min(event.clientY, window.innerHeight - 44 - items.length * 38),
    items,
  };
}

function closeContextMenu() {
  contextMenu.value = null;
}

function runContextItem(item) {
  closeContextMenu();
  item.run();
}

// tile clipboard: plain snapshot of the copied button; survives board
// switches, lives for the editor session only
const tileClipboard = ref(null);

function copyTile(tile) {
  // shallow spread is enough: button fields are all primitives, and the
  // snapshot must not follow later edits of the original
  tileClipboard.value = { ...tile };
}

// Paste a copied button at an empty cell: create the row with the copied
// type, then apply the full snapshot (styling, command, dual state) in one
// update - the same two-step the New Button flow uses.
async function pasteTile(snapshot, pos) {
  const board = boards.value.find((b) => b.id === currentId.value);
  if (!board) return;
  const w = Math.max(1, snapshot.w || 1);
  const h = Math.max(1, snapshot.h || 1);
  const x = clamp(pos.x, 0, Math.max(0, board.width - w));
  const y = clamp(pos.y, 0, Math.max(0, board.height - h));
  try {
    const id = await api.createButton(
      board.id,
      snapshot.type || "key",
      snapshot.mode || "button",
      x,
      y
    );
    const newSnapshot = {
      ...snapshot,
      id,
      board_id: board.id,
      x,
      y,
      w,
      h,
    };
    await api.updateButton(newSnapshot);
    history.push(tileCreateCommand({ tileSnapshot: newSnapshot, boardId: board.id }));
    updateHistoryFlags();
    await loadBoards();
  } catch (e) {
    flashError("Pasting the tile failed", e);
  }
}

function tileContextMenu(tile, event) {
  if (selectedTileIds.value.size > 1 && selectedTileIds.value.has(tile.id)) {
    const count = selectedTileIds.value.size;
    openContextMenu(event, [
      { label: `Kopiuj (${count} kafli)`, icon: "copy", run: copySelectedTiles },
      { label: `Duplikuj (${count} kafli)`, icon: "clone", run: () => duplicateTiles(currentBoard.value?.buttons.filter(b => selectedTileIds.value.has(b.id)) || []) },
      { label: "Zmień kolor...", icon: "palette", run: () => (bulkColorModal.value = { color: tile.color || "#ef4836" }) },
      {
        label: `Usuń (${count} kafli)`,
        icon: "trash",
        danger: true,
        run: deleteSelectedTiles,
      },
    ]);
    return;
  }
  openContextMenu(event, [
    { label: "Edit tile", icon: "pen", run: () => (editingTile.value = tile) },
    { label: "Run now", icon: "play", run: () => runTileNow(tile.id) },
    { label: "Copy", icon: "copy", run: () => copyTile(tile) },
    { label: "Duplikuj", icon: "clone", run: () => duplicateTiles([tile]) },
    {
      label: "Delete",
      icon: "trash",
      danger: true,
      run: () => tileDeleted(tile),
    },
  ]);
}

function emptyContextMenu(pos, event) {
  openContextMenu(event, [
    {
      label: "New button here",
      icon: "plus",
      run: () => (createFlow.value = { ...pos, boardId: currentId.value }),
    },
    ...(tileClipboard.value
      ? [
          {
            label: "Paste button here",
            icon: "paste",
            run: () => {
              const tiles = Array.isArray(tileClipboard.value) ? tileClipboard.value : [tileClipboard.value];
              if (tiles.length === 1) {
                pasteTile(tiles[0], pos);
              } else {
                pasteClipboardTiles();
              }
            },
          },
        ]
      : []),
  ]);
}

function boardContextMenu(board, event) {
  openContextMenu(event, [
    {
      label: "Edit board",
      icon: "pen",
      run: () => (boardModal.value = { mode: "edit", board }),
    },
    {
      label: "Set current",
      icon: "check",
      run: () => (currentId.value = board.id),
    },
    {
      label: "Clear tiles",
      icon: "eraser",
      run: async () => {
        const ok = await ask(`Clear every tile from "${board.name}"?`, {
          title: "Clear board",
          kind: "warning",
        });
        if (ok) {
          const tilesSnapshot = (board.buttons || []).map((b) => ({ ...b }));
          try {
            await api.clearBoard(board.id);
            history.push(boardClearCommand({ boardId: board.id, tilesSnapshot }));
            updateHistoryFlags();
          } catch (e) {
            flashError("Czyszczenie tablicy nie powiodło się", e);
            return;
          }
          await loadBoards();
        }
      },
    },
    {
      label: "Delete board",
      icon: "trash",
      danger: true,
      run: async () => {
        const ok = await ask(`Delete board "${board.name}"?`, {
          title: "Delete board",
          kind: "warning",
        });
        if (ok) {
          const boardSnapshot = {
            ...board,
            buttons: (board.buttons || []).map((b) => ({ ...b })),
          };
          try {
            await api.deleteBoard(board.id);
            history.push(boardDeleteCommand({ boardSnapshot }));
            updateHistoryFlags();
          } catch (e) {
            flashError("Usuwanie tablicy nie powiodło się", e);
            return;
          }
          await loadBoards();
        }
      },
    },
  ]);
}

// shell state: zoom, sidebar collapse, rail popovers, board kebab menu
const zoom = ref(1);
const sidebarVisible = ref(true);
const railPopover = ref(null); // 'status' | 'settings' | null
const kebabOpen = ref(false);

const editingTile = ref(null); // button being edited
const createFlow = ref(null); // {x, y, boardId} for the New Button dialog
const boardModal = ref(null); // {mode: 'create'|'edit', board?}

// Spotify picker lists for the edit dialog: fetched when a dialog opens
// (login state can change between opens; the command caches playlists
// for 60 s). Empty on failure - the dialog falls back to free text.
const spotifyDevices = ref([]);
const spotifyPlaylists = ref([]);
watch([editingTile, createFlow], ([edit, create]) => {
  if (edit || create) refreshSpotifyPickers();
});
async function refreshSpotifyPickers() {
  const [devices, playlists] = await Promise.all([
    api.spotifyDevices().catch(() => []),
    api.spotifyPlaylists().catch(() => []),
  ]);
  spotifyDevices.value = devices;
  spotifyPlaylists.value = playlists;
}

const currentBoard = computed(
  () => boards.value.find((b) => b.id === currentId.value) || null
);
const boardBg = computed(() => currentBoard.value?.background || "#437072");
const boardNames = computed(() =>
  Object.fromEntries(boards.value.map((b) => [b.id, b.name || "Untitled"]))
);

// Panel data that only changes on restart or settings edits: fetched at
// startup only, not after every tile edit (each call is an IPC round-trip
// and listBoards ships every board with its base64 images).
async function loadCore() {
  status.value = await api.serverStatus();
  knownInputs.value = await api.listKnownInputs();
  audioDevices.value = await api.listAudioDevices().catch(() => []);
}

// Board data: the only thing tile/board edits change.
async function loadBoards() {
  if (status.value.dbOk) {
    boards.value = await api.listBoards();
    if (!boards.value.some((b) => b.id === currentId.value)) {
      currentId.value = boards.value[0]?.id ?? null;
    }
  }
  // touchBoardId must never dangle on a deleted board - fall back to the
  // current one (null currentId keeps it null, same as before)
  if (!boards.value.some((b) => b.id === touchBoardId.value)) {
    touchBoardId.value = currentId.value;
  }
}

async function load() {
  await loadCore();
  await loadBoards();
}

// Open (or close) the server-status popover; pairing itself lives in the
// settings overlay, the popover only hands off to it.
function toggleStatusPopover() {
  railPopover.value = railPopover.value === "status" ? null : "status";
}

async function newBoard() {
  railPopover.value = null;
  boardModal.value = { mode: "create" };
}

async function editBoard() {
  kebabOpen.value = false;
  if (currentBoard.value) boardModal.value = { mode: "edit", board: currentBoard.value };
}

// New Button flow: create the row with the chosen type, then apply the
// dialog's full payload (label, styling, command) in one update.
async function tileCreated(form) {
  const boardId_ = form.board_id ?? currentId.value;
  const board = boards.value.find((b) => b.id === boardId_);
  if (!board) return;
  try {
    const id = await api.createButton(
      board.id,
      form.type,
      form.mode || "button",
      form.x,
      form.y
    );
    const fullTile = {
      ...form,
      id,
      board_id: board.id,
      w: form.w || 1,
      h: form.h || 1,
    };
    await api.updateButton(fullTile);
    history.push(tileCreateCommand({ tileSnapshot: fullTile, boardId: board.id }));
    updateHistoryFlags();
    createFlow.value = null;
    await loadBoards();
    const tile = boards.value
      .find((b) => b.id === board.id)
      ?.buttons.find((b) => b.id === id);
    if (tile) editingTile.value = tile;
  } catch (e) {
    // keep the dialog open with the entered values on a failed save
    flashError("Creating the tile failed", e);
  }
}

function tileSlider(tile, value) {
  api
    .execSlider(tile.id, value)
    .catch((e) => flashError("Slider change failed", e));
}

// "Run now" (context menu) and touch-mode taps share one failure surface:
// the exec is fire-and-forget otherwise (012 lower-priority feedback)
function runTileNow(id) {
  api.execButton(id).catch((e) => flashError("Running the tile failed", e));
}

async function tileMoved(tile, x, y, w, h) {
  const prevGeom = { x: tile.x, y: tile.y, w: tile.w, h: tile.h };
  try {
    await api.moveButton(tile.id, tile.board_id, x, y, w, h);
    history.push(tileMoveCommand({ id: tile.id, boardId: tile.board_id, prevGeom, nextGeom: { x, y, w, h } }));
    updateHistoryFlags();
  } catch (e) {
    flashError("Przesuwanie kafla nie powiodło się", e);
    await loadBoards();
    return;
  }
  tile.x = x;
  tile.y = y;
  tile.w = w;
  tile.h = h;
}

async function tileEdited(button) {
  const prevTile = boards.value
    .flatMap((b) => b.buttons)
    .find((b) => b.id === button.id);
  const prevSnapshot = prevTile ? { ...prevTile } : null;
  try {
    await api.updateButton(button);
  } catch (e) {
    // keep the dialog open so the edits are not lost
    flashError("Zapisywanie kafla nie powiodło się", e);
    return;
  }
  if (prevSnapshot) {
    history.push(tileEditCommand({ prevSnapshot, nextSnapshot: { ...button } }));
    updateHistoryFlags();
  }
  editingTile.value = null;
  await loadBoards();
}

async function tileDeleted(tile) {
  // single confirmation for both entry points: the context menu and the
  // edit dialog's Delete button (012 A9)
  const ok = await ask(`Delete tile "${tile.title || tile.type}"?`, {
    title: "Delete tile",
    kind: "warning",
  });
  if (!ok) return;
  try {
    await api.deleteButton(tile.id, tile.board_id);
    history.push(tileDeleteCommand({ tileSnapshot: { ...tile } }));
    updateHistoryFlags();
  } catch (e) {
    flashError("Usuwanie kafla nie powiodło się", e);
    return;
  }
  editingTile.value = null;
  await loadBoards();
}

async function doExport() {
  kebabOpen.value = false;
  if (!boards.value.length) return;
  const path = await save({
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  const ids = boards.value.map((b) => b.id);
  try {
    await api.exportBoards(ids, path);
  } catch (e) {
    flashError("Exporting boards failed", e);
  }
}

async function doImport() {
  kebabOpen.value = false;
  const path = await open({
    multiple: false,
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  try {
    await api.importBoards(path);
    history.clear();
    updateHistoryFlags();
    await loadBoards();
  } catch (e) {
    flashError("Import tablic nie powiódł się", e);
  }
}

async function clearCurrentBoard() {
  kebabOpen.value = false;
  if (!currentBoard.value) return;
  const ok = await ask(`Clear every tile from "${currentBoard.value.name}"?`, {
    title: "Clear board",
    kind: "warning",
  });
  if (!ok) return;
  const tilesSnapshot = (currentBoard.value.buttons || []).map((b) => ({ ...b }));
  try {
    await api.clearBoard(currentBoard.value.id);
    history.push(boardClearCommand({ boardId: currentBoard.value.id, tilesSnapshot }));
    updateHistoryFlags();
  } catch (e) {
    flashError("Czyszczenie tablicy nie powiodło się", e);
    return;
  }
  await loadBoards();
}

async function deleteCurrentBoard() {
  kebabOpen.value = false;
  if (!currentBoard.value) return;
  const ok = await ask(`Delete board "${currentBoard.value.name}"?`, {
    title: "Delete board",
    kind: "warning",
  });
  if (!ok) return;
  const boardSnapshot = {
    ...currentBoard.value,
    buttons: (currentBoard.value.buttons || []).map((b) => ({ ...b })),
  };
  try {
    await api.deleteBoard(currentBoard.value.id);
    history.push(boardDeleteCommand({ boardSnapshot }));
    updateHistoryFlags();
  } catch (e) {
    flashError("Usuwanie tablicy nie powiodło się", e);
    return;
  }
  await loadBoards();
}

function toggleTouch() {
  touchBoardId.value = touchMode.value ? touchBoardId.value : currentId.value;
  touchMode.value = !touchMode.value;
}

function bumpZoom(dir) {
  zoom.value = Math.min(1.5, Math.max(0.5, Math.round((zoom.value + dir * 0.1) * 10) / 10));
}

let unlisteners = [];
onMounted(async () => {
  // listeners register BEFORE the initial load (012 lower-priority): a
  // rejected load must not leave the app deaf to tray/hotkey/board events
  unlisteners.push(
    await listen("toggle-touch-mode", toggleTouch),
    await listen("change-board", (e) => {
      touchBoardId.value = e.payload;
      if (!touchMode.value) currentId.value = e.payload;
    }),
    await listen("app-status-update", (e) => onStatusUpdate(e.payload))
  );
  // the context menu closes on any click outside of it, or on Escape
  window.addEventListener("mousedown", onGlobalMousedown, true);
  window.addEventListener("keydown", onKeydown, true);
  document.addEventListener("visibilitychange", onVisibilityChange);
  load()
    .catch((e) => {
      // status stays dbOk:false, so the banner explains the state
      console.error("initial load failed", e);
    })
    .finally(() => {
      // a touch-mode toggle that arrived while the WebView was torn
      // down: mount into the intended mode once listeners and boards
      // are up (the original emit would have been lost pre-mount)
      api
        .takePendingTouchToggle()
        .then((pending) => {
          if (pending) toggleTouch();
        })
        .catch(() => {});
    });
});
onUnmounted(() => {
  window.removeEventListener("mousedown", onGlobalMousedown, true);
  window.removeEventListener("keydown", onKeydown, true);
  document.removeEventListener("visibilitychange", onVisibilityChange);
  unlisteners.forEach((f) => f());
});


function onBoardModalSaved(info) {
  boardModal.value = null;
  if (info) {
    if (info.mode === "create") {
      history.push(
        boardCreateCommand({
          boardId: info.id,
          name: info.name,
          background: info.background,
          width: info.width,
          height: info.height,
        })
      );
    } else if (info.mode === "edit") {
      history.push(
        boardEditCommand({
          prevSnapshot: info.prev,
          nextSnapshot: info.next,
        })
      );
    } else if (info.mode === "clear") {
      history.push(
        boardClearCommand({
          boardId: info.boardId,
          tilesSnapshot: info.buttons,
        })
      );
    } else if (info.mode === "delete") {
      history.push(boardDeleteCommand({ boardSnapshot: info.board }));
    }
    updateHistoryFlags();
  }
  load();
}

function onGlobalMousedown(event) {
  if (event.target.closest?.(".ctx-menu")) return;
  closeContextMenu();
}

function isInputTarget(target) {
  if (!target) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable
  );
}

function isModalActive() {
  return Boolean(
    editingTile.value ||
    createFlow.value ||
    boardModal.value ||
    settingsOpen.value
  );
}

function onKeydown(event) {
  if (event.key === "Escape") {
    closeContextMenu();
    if (selectedTileIds.value.size > 0) {
      selectedTileIds.value = new Set();
      event.preventDefault();
      return;
    }
  }

  if (isInputTarget(event.target) || isModalActive()) return;

  if ((event.ctrlKey || event.metaKey) && !event.altKey) {
    const k = event.key.toLowerCase();
    if (k === "z") {
      event.preventDefault();
      if (event.shiftKey) {
        doRedo();
      } else {
        doUndo();
      }
    } else if (k === "y") {
      event.preventDefault();
      doRedo();
    } else if (k === "c") {
      if (selectedTileIds.value.size > 0) {
        event.preventDefault();
        copySelectedTiles();
      }
    } else if (k === "v") {
      if (tileClipboard.value) {
        event.preventDefault();
        pasteClipboardTiles();
      }
    } else if (k === "d") {
      if (selectedTileIds.value.size > 0) {
        event.preventDefault();
        duplicateSelectedTiles();
      }
    }
  } else if (!event.ctrlKey && !event.metaKey && !event.altKey) {
    if (event.key === "Delete" || event.key === "Backspace") {
      if (selectedTileIds.value.size > 0) {
        event.preventDefault();
        deleteSelectedTiles();
      }
    } else if (["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(event.key)) {
      if (selectedTileIds.value.size > 0) {
        event.preventDefault();
        const dir = {
          ArrowUp: [0, -1],
          ArrowDown: [0, 1],
          ArrowLeft: [-1, 0],
          ArrowRight: [1, 0],
        }[event.key];
        nudgeSelection(dir[0], dir[1]);
      }
    }
  }
}
</script>

<template>
  <div class="app" @contextmenu.prevent>
    <!-- icon rail -->
    <nav v-if="!touchMode" class="rail">
      <button
        class="rail-logo"
        :class="{ dim: !sidebarVisible }"
        title="Toggle board list"
        @click="sidebarVisible = !sidebarVisible"
      >
        <i class="fas fa-th-large"></i>
      </button>

      <button
        class="rail-btn"
        :class="{ on: touchMode }"
        title="Touch mode"
        @click="toggleTouch"
        :disabled="!currentBoard"
      >
        <i class="fas fa-play"></i>
      </button>

      <div class="rail-anchor">
        <button
          class="rail-btn"
          :class="{ on: railPopover === 'status' }"
          title="Server status"
          @click="toggleStatusPopover"
        >
          <i class="fas fa-wifi"></i>
          <span v-if="!status.dbOk" class="rail-alert"></span>
        </button>
        <Transition name="pop">
          <div v-if="railPopover === 'status'" class="rail-pop">
            <div class="pop-row strong">
              <span :class="['dot', status.dbOk ? 'ok' : 'bad']"></span>
              {{ status.dbOk ? "Server running" : "Database unavailable" }}
            </div>
            <div class="pop-row tnum">0.0.0.0:{{ status.port }}</div>
            <div class="pop-row tnum">{{ status.clients }} client(s) connected</div>
            <template v-if="status.dbOk">
              <div class="pop-sep"></div>
              <button
                class="mini accent"
                @click="railPopover = null; settingsOpen = true"
              >
                <i class="fas fa-cog"></i> Ustawienia i parowanie tabletów
              </button>
            </template>
          </div>
        </Transition>
      </div>

      <button
        class="rail-btn"
        title="Ustawienia"
        @click="settingsOpen = true"
      >
        <i class="fas fa-cog"></i>
      </button>

      <div class="rail-spacer"></div>

      <button class="rail-add" title="New board" @click="newBoard" :disabled="!status.dbOk">
        <i class="fas fa-plus"></i>
      </button>
    </nav>

    <!-- board list -->
    <aside v-if="!touchMode && sidebarVisible" class="sidebar">
      <div class="side-caption">Boards</div>
      <div class="boards">
        <button
          v-for="b in boards"
          :key="b.id"
          class="board-entry"
          :class="{ active: b.id === currentId }"
          @click="currentId = b.id"
          @contextmenu.prevent="boardContextMenu(b, $event)"
        >
          {{ b.name || "Untitled" }}
        </button>
        <div v-if="!boards.length && status.dbOk" class="side-empty">
          No boards yet. Use <i class="fas fa-plus"></i> to create one.
        </div>
      </div>
    </aside>

    <!-- board canvas -->
    <main
      class="main"
      :style="{ '--board-bg': boardBg }"
    >
      <div v-if="!status.dbOk" class="banner">
        Database is locked or missing. Close the original Deckboard app and
        restart the editor.
      </div>

      <!-- transient failure feedback (save/import/export errors) -->
      <Transition name="pop">
        <div v-if="flash" class="flash" role="status">{{ flash }}</div>
      </Transition>

      <template v-if="status.dbOk">
        <header v-if="!touchMode" class="board-head" @contextmenu.prevent="currentBoard && boardContextMenu(currentBoard, $event)">
          <span class="board-title">{{ currentBoard?.name || "" }}</span>
          <div class="head-controls">
            <button
              class="tool-btn"
              :class="{ disabled: !canUndo }"
              :disabled="!canUndo"
              title="Cofnij (Ctrl+Z)"
              @click="doUndo"
            >
              <i class="fas fa-undo"></i>
            </button>
            <button
              class="tool-btn"
              :class="{ disabled: !canRedo }"
              :disabled="!canRedo"
              title="Ponów (Ctrl+Y / Ctrl+Shift+Z)"
              @click="doRedo"
            >
              <i class="fas fa-redo"></i>
            </button>
          </div>
          <div class="kebab-anchor">
            <button class="kebab" title="Board menu" @click="kebabOpen = !kebabOpen">
              <i class="fas fa-ellipsis-v"></i>
            </button>
            <Transition name="pop">
              <div v-if="kebabOpen" class="kebab-menu">
                <button class="menu-item" @click="editBoard" :disabled="!currentBoard">
                  <i class="fas fa-pen"></i> Edit board
                </button>
                <button class="menu-item" @click="doExport" :disabled="!boards.length">
                  <i class="fas fa-file-export"></i> Export boards
                </button>
                <button class="menu-item" @click="doImport" :disabled="!status.dbOk">
                  <i class="fas fa-file-import"></i> Import boards
                </button>
                <button class="menu-item" @click="clearCurrentBoard" :disabled="!currentBoard">
                  <i class="fas fa-eraser"></i> Clear tiles
                </button>
                <button class="menu-item danger" @click="deleteCurrentBoard" :disabled="!currentBoard">
                  <i class="fas fa-trash"></i> Delete board
                </button>
                <div class="menu-note">
                  double-click a tile to edit - drag to move - click an empty
                  cell to add
                </div>
              </div>
            </Transition>
          </div>
        </header>

        <div class="canvas">
          <GridEditor
            v-if="currentBoard && !touchMode"
            :board="currentBoard"
            :zoom="zoom"
            :type-meta="typeMeta"
            :custom-values="customValues"
            :board-names="boardNames"
            :selected-ids="selectedTileIds"
            @tile-open="editingTile = $event"
            @tile-moved="tileMoved"
            @tile-add="createFlow = { ...$event, boardId: currentId }"
            @ctx-tile="tileContextMenu"
            @ctx-empty="emptyContextMenu"
            @select-tile="onSelectTile"
            @select-tiles="onSelectTiles"
            @clear-selection="onClearSelection"
            @tiles-bulk-moved="onTilesBulkMoved"
            @move-refused="onMoveRefused"
          />
          <GridEditor
            v-if="touchBoardId && touchMode"
            :board="boards.find((b) => b.id === touchBoardId) || currentBoard"
            touch
            :type-meta="typeMeta"
            :custom-values="customValues"
            :board-names="boardNames"
            @tile-exec="runTileNow($event.id)"
            @tile-slider="tileSlider"
            @tile-open="editingTile = $event"
          />
          <div
            v-if="currentBoard && !touchMode && !currentBoard.buttons.length"
            class="canvas-hint"
          >
            Click an empty cell to add your first tile
          </div>

          <template v-if="!touchMode">
            <div class="canvas-overlay zoom">
              <button class="zoom-btn" title="Zoom out" @click="bumpZoom(-0.1)">
                <i class="fas fa-search-minus"></i>
              </button>
              <span class="zoom-value tnum">{{ Math.round(zoom * 100) }}%</span>
              <button class="zoom-btn" title="Zoom in" @click="bumpZoom(0.1)">
                <i class="fas fa-search-plus"></i>
              </button>
            </div>
            <div class="canvas-overlay version tnum">
              <i class="fas fa-puzzle-piece"></i>
              Version {{ status.version || "0.1.1" }}
            </div>
          </template>

          <button v-else class="touch-exit" title="Exit touch mode" @click="toggleTouch">
            <i class="fas fa-times"></i>
          </button>
        </div>
      </template>
    </main>

    <!-- backdrop that closes rail popovers and the kebab menu -->
    <div
      v-if="railPopover || kebabOpen"
      class="click-away"
      @click="railPopover = null; kebabOpen = false"
    ></div>

    <Transition name="modal">
      <EditTileModal
        v-if="editingTile"
        key="edit"
        :button="editingTile"
        :boards="boards"
        :board-background="boardBg"
        :known-inputs="knownInputs"
        :audio-devices="audioDevices"
        :spotify-devices="spotifyDevices"
        :spotify-playlists="spotifyPlaylists"
        @save="tileEdited"
        @delete="tileDeleted"
        @close="editingTile = null"
      />
      <EditTileModal
        v-else-if="createFlow"
        key="create"
        :create="createFlow"
        :boards="boards"
        :board-background="boardBg"
        :known-inputs="knownInputs"
        :audio-devices="audioDevices"
        :spotify-devices="spotifyDevices"
        :spotify-playlists="spotifyPlaylists"
        @create="tileCreated"
        @close="createFlow = null"
      />
    </Transition>

    <Transition name="modal">
      <BoardModal
        v-if="boardModal"
        :mode="boardModal.mode"
        :board="boardModal.board"
        @close="boardModal = null"
        @saved="onBoardModalSaved"
      />
    </Transition>

    <Transition name="modal">
      <SettingsOverlay
        v-if="settingsOpen"
        :status="status"
        :custom-values="customValues"
        @close="settingsOpen = false"
      />
    </Transition>

    <!-- custom right-click menu, shown anywhere via the context handlers -->
    <div
      v-if="contextMenu"
      class="ctx-overlay"
      @click="closeContextMenu"
      @contextmenu.prevent="closeContextMenu"
    ></div>
    <Transition name="pop">
      <div
        v-if="contextMenu"
        class="ctx-menu"
        :style="{ left: contextMenu.x + 'px', top: contextMenu.y + 'px' }"
      >
        <button
          v-for="item in contextMenu.items"
          :key="item.label"
          class="menu-item"
          :class="{ danger: item.danger }"
          @click="runContextItem(item)"
        >
          <i v-if="item.icon" class="fas" :class="'fa-' + item.icon"></i>{{ item.label }}
        </button>
      </div>
    </Transition>

    
    <!-- bulk color modal -->
    <Transition name="modal">
      <div v-if="bulkColorModal" class="overlay" @click.self="bulkColorModal = null">
        <div class="modal mini-modal">
          <div class="modal-head">Zmień kolor zaznaczonych kafli</div>
          <div class="modal-body">
            <label class="field">
              Wybierz kolor
              <div class="color-picker-row">
                <input v-model="bulkColorModal.color" type="color" class="color-picker-input" />
                <input v-model="bulkColorModal.color" class="hex" placeholder="#rrggbb" />
              </div>
            </label>
          </div>
          <div class="modal-actions">
            <button class="btn-text" @click="bulkColorModal = null">Anuluj</button>
            <button class="btn-text accent" @click="applyBulkColor(bulkColorModal.color)">Zastosuj</button>
          </div>
        </div>
      </div>
    </Transition>

    <!-- operator gates (B2 trust / M8 pair-request) as in-app popups -->
    <OperatorAskModal />
  </div>
</template>

<style scoped>
.app {
  display: flex;
  height: 100%;
  background: var(--canvas);
}

/* ---- rail ---- */
.rail {
  width: 60px;
  background: var(--rail);
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 11px 0;
  gap: 6px;
}
.rail-logo {
  width: 38px;
  height: 38px;
  border-radius: 10px;
  background: #fff;
  color: var(--accent-2);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  margin-bottom: 8px;
  flex: none;
  transition: opacity 120ms ease-out, transform 120ms ease-out;
}
.rail-logo:hover { opacity: 0.85; }
.rail-logo:active { transform: scale(0.96); }
.rail-logo.dim { opacity: 0.55; }
.rail-btn {
  position: relative;
  width: 44px;
  height: 44px;
  border-radius: 8px;
  color: var(--rail-icon);
  font-size: 17px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 120ms ease-out, color 120ms ease-out;
}
.rail-btn:hover { background: var(--rail-hover); color: #e8edf0; }
.rail-btn:active { transform: scale(0.96); }
.rail-btn.on { color: var(--accent); }
.rail-alert {
  position: absolute;
  top: 8px;
  right: 8px;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--danger);
}
.rail-anchor { position: relative; }
.rail-spacer { flex: 1; }
.rail-add {
  width: 38px;
  height: 38px;
  border-radius: 10px;
  background: #fff;
  color: #262e36;
  font-size: 15px;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 2px 6px rgba(0, 0, 0, 0.35);
  transition: background 120ms ease-out, transform 120ms ease-out;
}
.rail-add:hover { background: #e9edef; }
.rail-add:active { transform: scale(0.96); }

.rail-pop {
  position: absolute;
  left: 52px;
  top: 0;
  width: 264px;
  background: var(--modal);
  color: var(--modal-text);
  border-radius: 6px;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.4);
  padding: 12px 14px;
  z-index: 50;
  cursor: default;
}
.pop-row { display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 13.5px; }
.pop-row.strong { font-weight: 500; }
.pop-sep { border-top: 1px solid var(--modal-line); margin: 8px 0; }
.mini {
  font-size: 12.5px;
  padding: 5px 10px;
  border-radius: 4px;
  background: var(--modal-field);
  transition: background 120ms ease-out;
}
.mini:hover { background: #e4e4e4; }
.mini.accent { color: var(--accent-2); font-weight: 500; }
.dot { width: 9px; height: 9px; border-radius: 50%; flex: none; }
.dot.ok { background: var(--accent); }
.dot.bad { background: var(--danger); }

/* ---- sidebar ---- */
.sidebar {
  width: 200px;
  background: var(--sidebar);
  color: var(--sidebar-text);
  display: flex;
  flex-direction: column;
  z-index: 20;
}
.side-caption {
  font-size: 14px;
  color: var(--sidebar-muted);
  padding: 18px 20px 6px;
}
.boards {
  flex: 1;
  overflow-y: auto;
  overflow-x: hidden;
  display: flex;
  flex-direction: column;
  padding: 2px 8px 12px;
}
.board-entry {
  text-align: left;
  font-size: 14px;
  color: inherit;
  padding: 9px 12px;
  border-radius: 4px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  transition: background 120ms ease-out, color 120ms ease-out;
}
.board-entry:hover { background: rgba(0, 0, 0, 0.07); }
.board-entry.active {
  background: var(--accent);
  color: #fff;
  font-weight: 500;
}
.side-empty {
  font-size: 12.5px;
  color: var(--sidebar-muted);
  padding: 10px 12px;
  line-height: 1.5;
}
.side-empty i { font-size: 11px; }

/* ---- main / canvas ---- */
.main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--board-bg);
  position: relative;
}
.banner {
  position: absolute;
  top: 14px;
  left: 50%;
  transform: translateX(-50%);
  background: var(--danger);
  color: #fff;
  border-radius: 6px;
  padding: 10px 16px;
  font-size: 13px;
  line-height: 1.4;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.3);
  z-index: 25;
}
.flash {
  position: absolute;
  bottom: 16px;
  left: 50%;
  transform: translateX(-50%);
  max-width: min(640px, 90%);
  background: var(--danger);
  color: #fff;
  border-radius: 6px;
  padding: 10px 16px;
  font-size: 13px;
  line-height: 1.4;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.3);
  z-index: 26;
  pointer-events: none;
  overflow-wrap: anywhere;
}
.board-head {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 0 10px 0 22px;
  height: 56px;
  flex: none;
  background: color-mix(in srgb, var(--board-bg) 90%, white);
}
.board-title {
  font-size: 20px;
  font-weight: 500;
  color: rgba(0, 0, 0, 0.62);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
}
.head-controls {
  display: flex;
  align-items: center;
  gap: 4px;
}
.tool-btn {
  width: 34px;
  height: 34px;
  border-radius: 6px;
  background: transparent;
  color: rgba(0, 0, 0, 0.65);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 14px;
  cursor: pointer;
  transition: background 120ms ease-out, color 120ms ease-out, opacity 120ms ease-out;
}
.tool-btn:hover:not(:disabled) {
  background: rgba(0, 0, 0, 0.08);
  color: rgba(0, 0, 0, 0.9);
}
.tool-btn:disabled,
.tool-btn.disabled {
  opacity: 0.3;
  cursor: not-allowed;
}
.mini-modal { width: min(340px, 90vw); }
.color-picker-row { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
.color-picker-input { width: 44px; height: 38px; padding: 2px; border-radius: 4px; border: 1px solid var(--modal-line); cursor: pointer; }
.kebab-anchor { position: relative; }
.kebab {
  width: 44px;
  height: 44px;
  border-radius: 8px;
  font-size: 16px;
  color: rgba(0, 0, 0, 0.55);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 120ms ease-out;
}
.kebab:hover { background: rgba(0, 0, 0, 0.1); }
.kebab-menu {
  position: absolute;
  right: 0;
  top: 48px;
  width: 200px;
  background: var(--modal);
  border-radius: 6px;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.4);
  padding: 6px;
  z-index: 50;
  display: flex;
  flex-direction: column;
}
.menu-item {
  display: flex;
  align-items: center;
  gap: 10px;
  text-align: left;
  font-size: 13.5px;
  color: var(--modal-text);
  padding: 9px 12px;
  border-radius: 4px;
  transition: background 120ms ease-out;
}
.menu-item i { width: 16px; text-align: center; color: var(--modal-muted); }
.menu-item:hover { background: rgba(0, 0, 0, 0.06); }
.menu-item.danger { color: var(--danger); }
.menu-item.danger i { color: var(--danger); }
.menu-note {
  font-size: 11.5px;
  line-height: 1.5;
  color: var(--modal-muted);
  padding: 8px 12px 6px;
  border-top: 1px solid var(--modal-line);
  margin-top: 4px;
}

.canvas {
  flex: 1;
  position: relative;
  min-height: 0;
}
.canvas-hint {
  position: absolute;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  color: rgba(255, 255, 255, 0.92);
  font-size: 15px;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.35);
  pointer-events: none;
  white-space: nowrap;
}
.canvas-overlay {
  position: absolute;
  bottom: 14px;
  display: flex;
  align-items: center;
  gap: 4px;
  background: rgba(0, 0, 0, 0.42);
  color: rgba(255, 255, 255, 0.92);
  border-radius: 8px;
  padding: 4px 10px;
  font-size: 13px;
  z-index: 10;
}
.canvas-overlay.zoom { left: 14px; }
.canvas-overlay.version { right: 14px; gap: 8px; }
.zoom-btn {
  width: 30px;
  height: 30px;
  border-radius: 6px;
  color: inherit;
  font-size: 13px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 120ms ease-out;
}
.zoom-btn:hover { background: rgba(255, 255, 255, 0.15); }
.zoom-value { min-width: 42px; text-align: center; }
.touch-exit {
  position: absolute;
  top: 12px;
  right: 12px;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: rgba(0, 0, 0, 0.45);
  color: #fff;
  font-size: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 15;
  transition: background 120ms ease-out, transform 120ms ease-out;
}
.touch-exit:hover { background: rgba(0, 0, 0, 0.6); }
.touch-exit:active { transform: scale(0.96); }

.click-away {
  position: fixed;
  inset: 0;
  z-index: 35;
}

/* custom right-click menu */
.ctx-overlay {
  position: fixed;
  inset: 0;
  z-index: 60;
}
.ctx-menu {
  position: fixed;
  min-width: 200px;
  background: var(--modal);
  border-radius: 6px;
  box-shadow: 0 8px 30px rgba(0, 0, 0, 0.4);
  padding: 6px;
  z-index: 61;
  display: flex;
  flex-direction: column;
}

/* popover transitions */
.pop-enter-active { transition: opacity 150ms ease-out, transform 150ms cubic-bezier(0.2, 0, 0, 1); }
.pop-leave-active { transition: opacity 100ms ease-out; }
.pop-enter-from { opacity: 0; transform: translateY(-4px); }
.pop-leave-to { opacity: 0; }
</style>
