<script setup>
import { computed, onMounted, onUnmounted, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { save, open, ask } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { CATALOG } from "./catalog";
import GridEditor from "./components/GridEditor.vue";
import EditTileModal from "./components/EditTileModal.vue";
import BoardModal from "./components/BoardModal.vue";

const boards = ref([]);
const currentId = ref(null);
const status = ref({ dbOk: false, port: 0, clients: 0, version: "" });
const touchMode = ref(false);
const touchBoardId = ref(null);
const knownInputs = ref([]);

// type -> {icon, color, mode} fallbacks: static catalog + extension inputs
const typeMeta = computed(() => {
  const map = {};
  for (const e of CATALOG) {
    if (e.value) {
      map[e.value] = { icon: e.icon || "", color: e.color || "", mode: e.mode || "" };
    }
  }
  for (const i of knownInputs.value) {
    map[i.value] = { icon: i.icon || "", color: i.color || "", mode: i.mode || "" };
  }
  return map;
});

const hotkey = ref("Ctrl+Alt+D");
const hotkeyDraft = ref("");
const hotkeyError = ref("");
const editingHotkey = ref(false);
const autostart = ref(false);

// shell state: zoom, sidebar collapse, rail popovers, board kebab menu
const zoom = ref(1);
const sidebarVisible = ref(true);
const railPopover = ref(null); // 'status' | 'settings' | null
const kebabOpen = ref(false);

const editingTile = ref(null); // button being edited
const createFlow = ref(null); // {x, y, boardId} for the New Button dialog
const boardModal = ref(null); // {mode: 'create'|'edit', board?}

const currentBoard = computed(
  () => boards.value.find((b) => b.id === currentId.value) || null
);
const boardBg = computed(() => currentBoard.value?.background || "#437072");

async function load() {
  status.value = await api.serverStatus();
  hotkey.value = (await api.getSettings()).hotkey;
  knownInputs.value = await api.listKnownInputs();
  if (status.value.dbOk) {
    boards.value = await api.listBoards();
    if (!boards.value.some((b) => b.id === currentId.value)) {
      currentId.value = boards.value[0]?.id ?? null;
    }
  }
  if (touchBoardId.value === null) touchBoardId.value = currentId.value;
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
  const id = await api.createButton(
    board.id,
    form.type,
    form.mode || "button",
    form.x,
    form.y
  );
  await api.updateButton({
    ...form,
    id,
    board_id: board.id,
    w: form.w || 1,
    h: form.h || 1,
  });
  createFlow.value = null;
  await load();
  const tile = boards.value
    .find((b) => b.id === board.id)
    ?.buttons.find((b) => b.id === id);
  if (tile) editingTile.value = tile;
}

async function tileSlider(tile, value) {
  await api.execSlider(tile.id, value);
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
  kebabOpen.value = false;
  if (!boards.value.length) return;
  const path = await save({
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  const ids = boards.value.map((b) => b.id);
  await api.exportBoards(ids, path);
}

async function doImport() {
  kebabOpen.value = false;
  const path = await open({
    multiple: false,
    filters: [{ name: "Board JSON", extensions: ["boardjson"] }],
  });
  if (!path) return;
  await api.importBoards(path);
  await load();
}

async function clearCurrentBoard() {
  kebabOpen.value = false;
  if (!currentBoard.value) return;
  const ok = await ask(`Clear every tile from "${currentBoard.value.name}"?`, {
    title: "Clear board",
    kind: "warning",
  });
  if (!ok) return;
  await api.clearBoard(currentBoard.value.id);
  await load();
}

async function deleteCurrentBoard() {
  kebabOpen.value = false;
  if (!currentBoard.value) return;
  const ok = await ask(`Delete board "${currentBoard.value.name}"?`, {
    title: "Delete board",
    kind: "warning",
  });
  if (!ok) return;
  await api.deleteBoard(currentBoard.value.id);
  await load();
}

async function saveHotkey() {
  const combo = hotkeyDraft.value.trim();
  try {
    await api.setHotkey(combo);
    hotkey.value = combo;
    hotkeyError.value = "";
    editingHotkey.value = false;
  } catch (e) {
    hotkeyError.value = String(e);
  }
}

async function toggleAutostart() {
  autostart.value = !autostart.value;
  try {
    await api.setAutostart(autostart.value);
  } catch {
    autostart.value = !autostart.value;
  }
}

function toggleTouch() {
  touchBoardId.value = touchMode.value ? touchBoardId.value : currentId.value;
  touchMode.value = !touchMode.value;
}

function bumpZoom(dir) {
  zoom.value = Math.min(1.5, Math.max(0.5, Math.round((zoom.value + dir * 0.1) * 10) / 10));
}

function resetView() {
  zoom.value = 1;
}

let unlisteners = [];
onMounted(async () => {
  await load();
  autostart.value = await api.getAutostart();
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
          @click="railPopover = railPopover === 'status' ? null : 'status'"
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
          </div>
        </Transition>
      </div>

      <button class="rail-btn" title="Reset view (zoom 100%)" @click="resetView">
        <i class="fas fa-arrows-alt"></i>
      </button>

      <div class="rail-anchor">
        <button
          class="rail-btn"
          :class="{ on: railPopover === 'settings' }"
          title="Settings"
          @click="railPopover = railPopover === 'settings' ? null : 'settings'"
        >
          <i class="fas fa-cog"></i>
        </button>
        <Transition name="pop">
          <div v-if="railPopover === 'settings'" class="rail-pop">
            <div class="pop-label">Touch mode hotkey</div>
            <template v-if="!editingHotkey">
              <div class="pop-row hotkey-row">
                <kbd class="combo">{{ hotkey }}</kbd>
                <button
                  class="mini"
                  :disabled="!status.dbOk"
                  @click="editingHotkey = true; hotkeyDraft = hotkey"
                >Edit</button>
              </div>
            </template>
            <template v-else>
              <div class="pop-row">
                <input
                  v-model="hotkeyDraft"
                  class="hotkey-input"
                  placeholder="Ctrl+Alt+D"
                  @keyup.enter="saveHotkey"
                />
                <button class="mini accent" @click="saveHotkey">Set</button>
                <button class="mini" @click="editingHotkey = false">Cancel</button>
              </div>
              <div v-if="hotkeyError" class="pop-error">{{ hotkeyError }}</div>
            </template>

            <label class="pop-check">
              <input type="checkbox" :checked="autostart" @change="toggleAutostart" />
              Launch at startup
            </label>

            <div class="pop-row muted tnum">
              Editor v{{ status.version || "0.1.0" }}
            </div>
          </div>
        </Transition>
      </div>

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

      <template v-if="status.dbOk">
        <header v-if="!touchMode" class="board-head">
          <span class="board-title">{{ currentBoard?.name || "" }}</span>
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
            @tile-open="editingTile = $event"
            @tile-moved="tileMoved"
            @tile-add="createFlow = { ...$event, boardId: currentId }"
          />
          <GridEditor
            v-if="touchBoardId && touchMode"
            :board="boards.find((b) => b.id === touchBoardId) || currentBoard"
            touch
            :type-meta="typeMeta"
            @tile-exec="api.execButton($event.id)"
            @tile-slider="tileSlider"
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
              Version {{ status.version || "0.1.0" }}
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
        @saved="
          boardModal = null;
          load();
        "
      />
    </Transition>
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
.pop-row.muted { color: var(--modal-muted); font-size: 12.5px; margin-top: 6px; }
.pop-label { font-size: 12px; color: var(--modal-muted); margin-bottom: 2px; }
.pop-error { font-size: 12px; color: var(--danger); padding-top: 4px; overflow-wrap: anywhere; }
.hotkey-row { justify-content: space-between; }
.hotkey-input { flex: 1; min-width: 0; padding: 5px 8px; font-size: 13px; }
.combo {
  font-size: 12.5px;
  background: var(--modal-field);
  border: 1px solid var(--modal-line);
  border-radius: 4px;
  padding: 3px 8px;
}
.pop-check {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13.5px;
  padding: 8px 0 2px;
  cursor: pointer;
}
.pop-check input { width: auto; }
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

/* popover transitions */
.pop-enter-active { transition: opacity 150ms ease-out, transform 150ms cubic-bezier(0.2, 0, 0, 1); }
.pop-leave-active { transition: opacity 100ms ease-out; }
.pop-enter-from { opacity: 0; transform: translateY(-4px); }
.pop-leave-to { opacity: 0; }
</style>
