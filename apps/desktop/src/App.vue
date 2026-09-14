<script setup>
import { computed, onMounted, onUnmounted, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { save, open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { CELL_W, ROW_H } from "./catalog";
import GridEditor from "./components/GridEditor.vue";
import EditTileModal from "./components/EditTileModal.vue";
import BoardModal from "./components/BoardModal.vue";
import AddTileModal from "./components/AddTileModal.vue";

const boards = ref([]);
const currentId = ref(null);
const status = ref({ dbOk: false, port: 0, clients: 0 });
const touchMode = ref(false);
const touchBoardId = ref(null);

const editingTile = ref(null); // button being edited
const boardModal = ref(null); // {mode: 'create'|'edit', board?}
const addFlow = ref(null); // {x, y} position for the new tile

const currentBoard = computed(() =>
  boards.value.find((b) => b.id === currentId.value) || null
);

async function load() {
  status.value = await api.serverStatus();
  if (status.value.dbOk) {
    boards.value = await api.listBoards();
    if (!boards.value.some((b) => b.id === currentId.value)) {
      currentId.value = boards.value[0]?.id ?? null;
    }
  }
  if (touchBoardId.value === null) touchBoardId.value = currentId.value;
}

async function newBoard() {
  boardModal.value = { mode: "create" };
}

async function editBoard() {
  if (currentBoard.value) boardModal.value = { mode: "edit", board: currentBoard.value };
}

async function addTile(kind, at) {
  const board = currentBoard.value;
  if (!board) return;
  const spot = at || firstFreeCell(board);
  const id = await api.createButton(
    board.id,
    kind,
    kind === "speaker-volume" ? "slider" : "button",
    spot.x,
    spot.y
  );
  addFlow.value = null;
  await load();
  const tile = boards.value
    .find((b) => b.id === board.id)
    ?.buttons.find((b) => b.id === id);
  if (tile) editingTile.value = tile;
}

function firstFreeCell(board) {
  const taken = new Set(board.buttons.map((t) => `${t.x},${t.y}`));
  for (let y = 0; y < board.height; y++) {
    for (let x = 0; x < board.width; x++) {
      if (!taken.has(`${x},${y}`)) return { x, y };
    }
  }
  return { x: 0, y: 0 };
}

async function tileMoved(tile, x, y, w, h) {
  await api.moveButton(tile.id, tile.board_id, x, y, w, h);
  tile.x = x;
  tile.y = y;
  tile.w = w;
  tile.h = h;
}

async function tileEdited(button) {
  await api.updateButton(button);
  editingTile.value = null;
  await load();
}

async function tileDeleted(tile) {
  await api.deleteButton(tile.id, tile.board_id);
  editingTile.value = null;
  await load();
}

async function doExport() {
  const path = await save({
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  const ids = boards.value.map((b) => b.id);
  await api.exportBoards(ids, path);
}

async function doImport() {
  const path = await open({
    multiple: false,
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  await api.importBoards(path);
  await load();
}

function toggleTouch() {
  touchBoardId.value = touchMode.value ? touchBoardId.value : currentId.value;
  touchMode.value = !touchMode.value;
}

let unlisteners = [];
onMounted(async () => {
  await load();
  unlisteners.push(
    await listen("toggle-touch-mode", toggleTouch),
    await listen("change-board", (e) => {
      touchBoardId.value = e.payload;
      if (!touchMode.value) currentId.value = e.payload;
    })
  );
});
onUnmounted(() => unlisteners.forEach((f) => f()));
</script>

<template>
  <div class="app">
    <aside class="sidebar">
      <div class="brand">
        <i class="fas fa-th-large"></i> Deckboard
      </div>

      <div v-if="!status.dbOk" class="banner">
        Database is locked or missing. Close the original Deckboard app and
        restart the editor.
      </div>

      <div class="boards">
        <button
          v-for="b in boards"
          :key="b.id"
          class="board-entry"
          :class="{ active: b.id === currentId }"
          @click="currentId = b.id"
        >
          {{ b.name || "Untitled" }}
          <span class="dim">{{ b.width }}x{{ b.height }}</span>
        </button>
      </div>

      <div class="side-actions">
        <button class="primary" @click="newBoard" :disabled="!status.dbOk">
          <i class="fas fa-plus"></i> Board
        </button>
        <button @click="editBoard" :disabled="!currentBoard">
          <i class="fas fa-pen"></i> Edit
        </button>
        <button @click="doExport" :disabled="!boards.length">
          <i class="fas fa-file-export"></i> Export
        </button>
        <button @click="doImport" :disabled="!status.dbOk">
          <i class="fas fa-file-import"></i> Import
        </button>
        <button class="touch" @click="toggleTouch" :disabled="!currentBoard">
          <i class="fas fa-hand-pointer"></i>
          {{ touchMode ? "Exit touch" : "Touch mode" }}
        </button>
      </div>

      <div class="status">
        <span :class="['dot', status.dbOk ? 'ok' : 'bad']"></span>
        port {{ status.port }} - {{ status.clients }} client(s)
      </div>
    </aside>

    <main class="main">
      <GridEditor
        v-if="currentBoard && !touchMode"
        :board="currentBoard"
        @tile-open="editingTile = $event"
        @tile-moved="tileMoved"
        @tile-add="addFlow = { ...$event }"
        @tile-add-default="addFlow = { ...firstFreeCell(currentBoard) }"
        @board-cleared="load"
      />
      <GridEditor
        v-else-if="touchBoardId && touchMode"
        :board="boards.find((b) => b.id === touchBoardId) || currentBoard"
        touch
        @tile-exec="api.execButton($event.id)"
      />
    </main>

    <EditTileModal
      v-if="editingTile"
      :button="editingTile"
      :boards="boards"
      @save="tileEdited"
      @delete="tileDeleted"
      @close="editingTile = null"
    />

    <AddTileModal
      v-if="addFlow"
      @pick="addTile($event, addFlow)"
      @close="addFlow = null"
    />

    <BoardModal
      v-if="boardModal"
      :mode="boardModal.mode"
      :board="boardModal.board"
      @close="boardModal = null"
      @saved="
        boardModal = null;
        load();
      "
    />
  </div>
</template>

<style scoped>
.app {
  display: flex;
  height: 100%;
}
.sidebar {
  width: 240px;
  background: var(--surface);
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  padding: 12px;
  gap: 10px;
}
.brand {
  font-weight: 600;
  font-size: 17px;
  letter-spacing: 0.3px;
}
.brand i { color: var(--accent); margin-right: 4px; }
.banner {
  background: var(--danger);
  border-radius: 6px;
  padding: 8px 10px;
  font-size: 12.5px;
  line-height: 1.4;
}
.boards {
  flex: 1;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.board-entry {
  text-align: left;
  display: flex;
  justify-content: space-between;
  align-items: center;
  background: transparent;
  border: 1px solid transparent;
}
.board-entry:hover { background: var(--surface-2); }
.board-entry.active { background: var(--surface-2); border-color: var(--accent); }
.board-entry .dim { color: var(--text-muted); font-size: 11px; }
.side-actions {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px;
}
.side-actions .touch { grid-column: 1 / -1; }
.status {
  font-size: 11.5px;
  color: var(--text-muted);
  display: flex;
  align-items: center;
  gap: 6px;
}
.dot { width: 8px; height: 8px; border-radius: 50%; display: inline-block; }
.dot.ok { background: var(--accent); }
.dot.bad { background: var(--danger); }
.main { flex: 1; overflow: auto; }
</style>
