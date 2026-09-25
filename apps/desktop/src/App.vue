<script setup>
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
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
const audioDevices = ref([]);
const lanAddresses = ref([]); // {name, ipv4, qr} - "Connect a tablet" popover
const activeAddress = ref(0);
const pairMode = ref("legacy"); // 'legacy' (stock client) | 'v2' (new client)
const pairingOffer = ref(null); // {code, expires_in_secs, addresses} | null

// Live state mirrors of the original client: customValues holds pushed
// values (APP_CUSTOM_VALUE), appStates per-integration status (APP_OBS...).
const customValues = reactive({});
const appStates = reactive({});

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
    case "APP_OBS":
      mergeAppState("obs", data);
      break;
    case "APP_TWITCH":
      mergeAppState("twitch", data);
      break;
    case "APP_VMOD":
      mergeAppState("vmod", data);
      break;
    case "APP_DISCORD":
      mergeAppState("discord", data);
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

function mergeAppState(name, data) {
  const prev = appStates[name];
  const next = { ...(prev || {}) };
  let changed = false;
  for (const [k, v] of Object.entries(data)) {
    if (prev && jsonEqual(prev[k], v)) continue;
    next[k] = v;
    changed = true;
  }
  if (changed) appStates[name] = next;
}

// type -> {icon, color, mode} fallbacks: static catalog + extensions
const typeMeta = computed(() => {
  const map = {};
  for (const e of CATALOG) {
    if (e.value) {
      map[e.value] = {
        icon: e.icon || "",
        color: e.color || "",
        mode: e.mode || "",
      };
    }
  }
  for (const i of knownInputs.value) {
    map[i.value] = {
      icon: i.icon || "",
      color: i.color || "",
      mode: i.mode || "",
    };
  }
  return map;
});

const hotkey = ref("Ctrl+Alt+D");
const hotkeyDraft = ref("");
const hotkeyError = ref("");
const editingHotkey = ref(false);
const autostart = ref(false);

// custom right-click context menu, replacing the WebView2 default menu
// everywhere: {x, y, items: [{label, icon, danger, run}]}
const contextMenu = ref(null);

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

function tileContextMenu(tile, event) {
  openContextMenu(event, [
    { label: "Edit tile", icon: "pen", run: () => (editingTile.value = tile) },
    { label: "Run now", icon: "play", run: () => api.execButton(tile.id) },
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
          await api.clearBoard(board.id);
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
          await api.deleteBoard(board.id);
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
  hotkey.value = (await api.getSettings()).hotkey;
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
  if (touchBoardId.value === null) touchBoardId.value = currentId.value;
}

async function load() {
  await loadCore();
  await loadBoards();
}

// Open (or close) the server-status popover; on open, refresh the LAN
// addresses so the tablet pairing QR never shows a stale interface.
function toggleStatusPopover() {
  const opening = railPopover.value !== "status";
  railPopover.value = opening ? "status" : null;
  if (opening) {
    activeAddress.value = 0;
    api
      .listLanAddresses()
      .then((a) => (lanAddresses.value = a))
      .catch(() => (lanAddresses.value = []));
  }
}

function refreshLanAddresses() {
  api
    .listLanAddresses()
    .then((a) => {
      lanAddresses.value = a;
      activeAddress.value = 0;
    })
    .catch(() => {});
}

// Mint a one-time pairing code for the new (protocol v2) client; codes
// live 5 minutes and are burned on first use.
async function generatePairingCode() {
  try {
    pairingOffer.value = await api.createPairingCode();
  } catch (e) {
    console.error("pairing code", e);
  }
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
  await loadBoards();
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
  await loadBoards();
}

async function tileDeleted(tile) {
  await api.deleteButton(tile.id, tile.board_id);
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
  await loadBoards();
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
  await api.deleteBoard(currentBoard.value.id);
  await loadBoards();
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

let unlisteners = [];
onMounted(async () => {
  await load();
  autostart.value = await api.getAutostart();
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
});
onUnmounted(() => {
  window.removeEventListener("mousedown", onGlobalMousedown, true);
  window.removeEventListener("keydown", onKeydown, true);
  document.removeEventListener("visibilitychange", onVisibilityChange);
  unlisteners.forEach((f) => f());
});

function onGlobalMousedown(event) {
  if (event.target.closest?.(".ctx-menu")) return;
  closeContextMenu();
}

function onKeydown(event) {
  if (event.key === "Escape") closeContextMenu();
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
              <div class="pop-label">Connect a tablet (same Wi-Fi/LAN)</div>
              <div class="pair-tabs">
                <button
                  class="pair-tab"
                  :class="{ on: pairMode === 'legacy' }"
                  @click="pairMode = 'legacy'"
                >Stock client</button>
                <button
                  class="pair-tab"
                  :class="{ on: pairMode === 'v2' }"
                  @click="pairMode = 'v2'"
                >New client (v2)</button>
              </div>

              <template v-if="pairMode === 'legacy'">
                <template v-if="lanAddresses.length">
                  <div class="lan-row">
                    <img
                      class="lan-qr"
                      :src="lanAddresses[activeAddress]?.qr"
                      alt="QR code with the desktop IP address"
                    />
                    <div class="lan-list">
                      <button
                        v-for="(a, i) in lanAddresses"
                        :key="a.ipv4"
                        class="lan-addr"
                        :class="{ sel: i === activeAddress }"
                        @click="activeAddress = i"
                      >
                        <span class="lan-ip tnum">{{ a.ipv4 }}:{{ status.port }}</span>
                        <span class="lan-name">{{ a.name }}</span>
                      </button>
                      <button class="mini" @click="refreshLanAddresses">
                        <i class="fas fa-sync-alt"></i> Refresh
                      </button>
                    </div>
                  </div>
                  <div class="pop-row muted">
                    Scan the QR in the Deckboard app, or type the address.
                  </div>
                </template>
                <div v-else class="pop-row muted">
                  Not connected to any local network.
                </div>
              </template>

              <template v-else>
                <div v-if="!pairingOffer" class="pair-empty">
                  <button class="mini accent" @click="generatePairingCode">
                    <i class="fas fa-key"></i> Generate pairing code
                  </button>
                  <div class="pop-row muted">One-time code, valid 5 minutes.</div>
                </div>
                <template v-else>
                  <div class="lan-row">
                    <img
                      class="lan-qr"
                      :src="pairingOffer.addresses[activeAddress % pairingOffer.addresses.length]?.qr"
                      alt="QR code with the pairing payload"
                    />
                    <div class="lan-list">
                      <div class="pair-code tnum">{{ pairingOffer.code }}</div>
                      <div class="lan-name">valid {{ Math.round(pairingOffer.expires_in_secs / 60) }} min, one device</div>
                      <button class="mini" @click="generatePairingCode">
                        <i class="fas fa-sync-alt"></i> New code
                      </button>
                    </div>
                  </div>
                  <div class="pop-row muted">
                    Scan the QR in the new Pulpit client, or enter the code
                    with the address {{ pairingOffer.addresses[0]?.ipv4 }}.
                  </div>
                </template>
              </template>
            </template>
          </div>
        </Transition>
      </div>

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
              Editor v{{ status.version || "0.1.1" }}
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

      <template v-if="status.dbOk">
        <header v-if="!touchMode" class="board-head" @contextmenu.prevent="currentBoard && boardContextMenu(currentBoard, $event)">
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
            :custom-values="customValues"
            :app-states="appStates"
            :board-names="boardNames"
            @tile-open="editingTile = $event"
            @tile-moved="tileMoved"
            @tile-add="createFlow = { ...$event, boardId: currentId }"
            @ctx-tile="tileContextMenu"
            @ctx-empty="emptyContextMenu"
          />
          <GridEditor
            v-if="touchBoardId && touchMode"
            :board="boards.find((b) => b.id === touchBoardId) || currentBoard"
            touch
            :type-meta="typeMeta"
            :custom-values="customValues"
            :app-states="appStates"
            :board-names="boardNames"
            @tile-exec="api.execButton($event.id)"
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
.pop-sep { border-top: 1px solid var(--modal-line); margin: 8px 0; }
.lan-row { display: flex; gap: 10px; align-items: flex-start; margin-top: 4px; }
.lan-qr {
  width: 96px;
  height: 96px;
  flex: none;
  border-radius: 4px;
  background: #fff;
}
.lan-list { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.lan-addr {
  text-align: left;
  padding: 3px 6px;
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  transition: background 120ms ease-out;
}
.lan-addr:hover { background: var(--modal-field); }
.lan-addr.sel { background: var(--modal-field); }
.lan-ip { font-size: 13px; }
.lan-addr.sel .lan-ip { font-weight: 700; }
.lan-name {
  font-size: 11.5px;
  color: var(--modal-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pair-tabs { display: flex; gap: 4px; margin: 2px 0 6px; }
.pair-tab {
  font-size: 12px;
  padding: 4px 8px;
  border-radius: 4px;
  color: var(--modal-muted);
  background: transparent;
  transition: background 120ms ease-out, color 120ms ease-out;
}
.pair-tab:hover { background: var(--modal-field); }
.pair-tab.on { background: var(--modal-field); color: var(--modal-text); font-weight: 500; }
.pair-empty { display: flex; flex-direction: column; gap: 2px; }
.pair-code {
  font-size: 21px;
  font-weight: 700;
  letter-spacing: 2px;
  line-height: 1.1;
}
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
