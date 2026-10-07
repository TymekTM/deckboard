<script setup>
import { computed, reactive, ref, watch } from "vue";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { CATALOG, parsePlanWindows, setPlanWindows } from "../catalog";
import { api, vmDevicesState, refreshVmDevices } from "../api";
import SelectField from "./SelectField.vue";
import ActionPicker from "./ActionPicker.vue";

const props = defineProps({
  button: { type: Object, default: null }, // null = create mode
  create: { type: Object, default: null }, // {x, y, boardId} for create mode
  boards: { type: Array, required: true },
  boardBackground: { type: String, default: "#437072" },
  knownInputs: { type: Array, default: () => [] },
  // active audio endpoints ({id, name}) for the Set Audio Device select
  audioDevices: { type: Array, default: () => [] },
  // Spotify pickers ({name} devices, {uri, name} playlists); empty when
  // not logged in or the list failed - picker fields then fall back to
  // free text
  spotifyDevices: { type: Array, default: () => [] },
  spotifyPlaylists: { type: Array, default: () => [] },
});
const emit = defineEmits(["save", "create", "delete", "close"]);

refreshVmDevices();
const isCreate = computed(() => !props.button);

// editable copy: every column the dialog touches. Shallow spread, not a
// JSON deep clone: button fields are all primitives, and the base64 img
// strings are immutable - spreading them shares instead of duplicating
// multi-MB payloads on every dialog open.
const form = reactive(
  props.button
    ? { ...props.button }
    : {
        id: null,
        board_id: props.create?.boardId ?? null,
        type: "key",
        mode: "button",
        command: "",
        title: "",
        color: "#ef4836",
        border_color: "",
        shape: 0,
        title_position: 0,
        title_color: "",
        title_box_color: "",
        icon: "",
        icon_color: "",
        img: "",
        color2: "",
        icon2: "",
        img2: "",
        options: "",
        x: props.create?.x ?? 0,
        y: props.create?.y ?? 0,
        w: 1,
        h: 1,
      }
);

// AI plan tile: which usage windows render, stored in the options column
// as "windows:5h,week" (no token = both). Applies only to ai-plan-limits.
// The token parses through the shared catalog helper (DESK-10) - the
// status tile filters rows with the same function.
const isPlanTile = computed(() => form.type === "ai-plan-limits");
const planWindows = computed(() => parsePlanWindows(form.options));
function setPlanWindow(key, event) {
  form.options = setPlanWindows(form.options, {
    ...planWindows.value,
    [key]: event.target.checked,
  });
}

// SMTC media tiles: the optional target app ("Aplikacja") lives in the
// options column as {"app": "<name substring>"} - the now-playing status
// tile keys its live payload by `command || type`, so the command must
// stay empty. The dropdown lists the live system playback sessions; a
// failed fetch leaves the picker empty (the current session stays the
// default), and clearing it back selects exactly that.
const isMediaTile = computed(() =>
  ["media-now-playing", "media-control", "media-seek"].includes(form.type)
);
const mediaApps = ref([]);
const mediaApp = computed(() => {
  try {
    return JSON.parse(form.options || "{}")?.app || "";
  } catch {
    return "";
  }
});
watch(
  isMediaTile,
  (yes) => {
    if (yes && !mediaApps.value.length) {
      api
        .mediaSessions()
        .then((list) => (mediaApps.value = Array.isArray(list) ? list : []))
        .catch(() => {});
    }
  },
  { immediate: true }
);
function setMediaApp(name) {
  let obj = {};
  try {
    obj = JSON.parse(form.options || "{}") || {};
  } catch {
    obj = {};
  }
  if (name) obj.app = name;
  else delete obj.app;
  form.options = Object.keys(obj).length ? JSON.stringify(obj) : "";
}

// ---- action catalog (static groups + live extension inputs) ----------------

function prettify(value) {
  const s = String(value).replace(/[-_]/g, " ").trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

const extInputs = computed(() => {
  const seen = new Set(CATALOG.filter((c) => c.value).map((c) => c.value));
  return props.knownInputs.filter(
    (i) => i.source === "extension" && !seen.has(i.value)
  );
});

// extension field declarations ("input:text" / "input:select" / ...) use the
// same shape as catalog fields so the generic renderer can handle them
function extFieldShape(f) {
  if (f.kind === "input:select") {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: (f.items || []).map((it) => ({ value: it.value, label: it.label })),
    };
  }
  if (f.kind === "input:key") {
    return { key: f.key, label: f.label, placeholder: "e.g. CTRL + K" };
  }
  if (f.kind === "input:file" || f.kind === "input:folder") {
    return {
      key: f.key,
      label: f.label,
      placeholder: f.kind === "input:file" ? "file path" : "folder path",
    };
  }
  return { key: f.key, label: f.label };
}

const actionGroups = computed(() => {
  const groups = [];
  let current = null;
  for (const entry of CATALOG) {
    if (entry.header) {
      current = { header: entry.header, items: [] };
      groups.push(current);
    } else if (!entry.divider && current) {
      current.items.push(entry);
    }
  }
  if (extInputs.value.length) {
    // one group per extension package, named after the plugin
    const byExt = new Map();
    for (const i of extInputs.value) {
      const name = i.extension || "Extensions";
      if (!byExt.has(name)) byExt.set(name, []);
      byExt.get(name).push({
        value: i.value,
        label: i.label || prettify(i.value),
        icon: i.icon || "puzzle-piece",
        color: i.color || "#7f8c8d",
        extInput: i,
        fields: (i.fields || []).map(extFieldShape),
      });
    }
    for (const [header, items] of byExt) groups.push({ header, items });
  }
  return groups;
});

// catalog fields may declare a dynamic option source; "audio" is the
// audio endpoint list for speaker-device tiles, "spotify"/
// "spotify-playlists" the Spotify device/playlist pickers. A picker
// whose list is empty or failed to load (not logged in, API error)
// falls back to the raw text field, so the URI/name can always be typed.
function catalogFieldShape(f) {
  if (f.devices === "audio") {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: props.audioDevices.map((d) => ({ value: d.id, label: d.name })),
    };
  }
  if (f.devices === "spotify" && props.spotifyDevices.length) {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: props.spotifyDevices.map((d) => ({ value: d.name, label: d.name })),
    };
  }
  if (f.devices === "spotify-playlists" && props.spotifyPlaylists.length) {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: props.spotifyPlaylists.map((p) => ({ value: p.uri, label: p.name })),
    };
  }
  if (f.devices === "vm-strip" && vmDevicesState.value.strips.length) {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: vmDevicesState.value.strips,
    };
  }
  if (f.devices === "vm-bus" && vmDevicesState.value.buses.length) {
    return {
      key: f.key,
      label: f.label,
      kind: "select",
      options: vmDevicesState.value.buses,
    };
  }
  return f;
}
// catalog entry for any action type (static catalog or extension group);
// shared by the main form and the per-gesture action overrides
function catalogEntryFor(type) {
  return (
    CATALOG.find((c) => c.value === type) ||
    actionGroups.value
      .flatMap((g) => g.items)
      .find((c) => c.value === type) ||
    null
  );
}
function fieldsShapeFor(type) {
  return (catalogEntryFor(type)?.fields || []).map(catalogFieldShape);
}
const catalogFields = computed(() => fieldsShapeFor(form.type));

const catalogEntry = computed(() => catalogEntryFor(form.type));
const showDual = computed(
  () =>
    Boolean(catalogEntry.value?.dual) ||
    form.mode === "toggle" ||
    Boolean(form.color2 || form.icon2 || form.img2)
);
const stepConfig = computed(() => catalogEntry.value?.stepEditor || null);

// ---- command fields --------------------------------------------------------

// field values derived from `command`: key "" is the raw command, other keys
// live inside the JSON object stored in command
const fields = reactive({});
function loadFields() {
  for (const k of Object.keys(fields)) delete fields[k];
  const shaped = catalogFields.value;
  if (!shaped.length) return;
  let obj = {};
  if (shaped.some((f) => f.key !== "")) {
    try {
      obj = JSON.parse(form.command || "{}") || {};
    } catch {
      obj = {};
    }
  }
  for (const f of shaped) {
    fields[f.key] = f.key === "" ? form.command || "" : obj[f.key] ?? "";
  }
  // sensible defaults for selects
  for (const f of shaped) {
    if (f.kind === "select" && fields[f.key] === "" && f.options.length) {
      fields[f.key] = f.options[0].value;
    }
  }
}
function fieldVisible(f) {
  return !f.showIf || fields[f.showIf.key] === f.showIf.value;
}
function applyFields() {
  const shaped = catalogFields.value;
  if (!shaped.length) return;
  if (shaped.every((f) => f.key === "")) {
    form.command = fields[""] ?? "";
    return;
  }
  // merge into the stored command so keys the extension declared outside
  // of its field list survive an edit
  let obj = {};
  try {
    obj = JSON.parse(form.command || "{}") || {};
  } catch {
    obj = {};
  }
  for (const f of shaped) {
    const raw = fields[f.key];
    if (f.kind === "number") {
      if (raw !== "" && raw !== null && !Number.isNaN(Number(raw))) {
        obj[f.key] = Number(raw);
      } else if (fieldVisible(f) && (raw === "" || raw === null)) {
        // a cleared visible field drops its key; skipping it would keep
        // the value pre-seeded from the old command, so the tile kept
        // targeting the old scene/value while the input looked empty
        delete obj[f.key];
      }
    } else if (raw !== "") {
      obj[f.key] = raw;
    } else if (fieldVisible(f)) {
      delete obj[f.key];
    }
  }
  form.command = JSON.stringify(obj);
}
loadFields();

function onTypeChange() {
  const entry = catalogEntry.value;
  // extension inputs carry a preconfigured command template / slider mode
  if (entry?.extInput) {
    if (entry.extInput.command) form.command = entry.extInput.command;
    if (entry.extInput.mode) form.mode = entry.extInput.mode;
    if (entry.extInput.icon && !form.icon) form.icon = entry.extInput.icon;
    if (entry.extInput.color && !form.color) form.color = entry.extInput.color;
  }
  if (entry?.mode) form.mode = entry.mode;
  loadFields();
}

// ActionPicker owns the action <select> replacement; keep the side effects
// of the old native @change in one place, in order
function onActionPicked(value) {
  form.type = value;
  onTypeChange();
  closeProps();
}

// board select (type "board": command is {"id": <boardId>})
const boardId = ref(parseBoardId(form.command));
function parseBoardId(command) {
  try {
    return JSON.parse(command || "{}").id ?? "";
  } catch {
    return "";
  }
}
function applyBoardId() {
  form.command = boardId.value === "" ? "" : JSON.stringify({ id: Number(boardId.value) });
}

// select-style command (vol): command is the raw value
function applySelect(event) {
  form.command = event.target.value;
}

// ---- step editor (multiaction / advance-key) -------------------------------

const steps = ref(parseSteps(form.command));
function parseSteps(command) {
  try {
    const arr = JSON.parse(command || "[]");
    return Array.isArray(arr) ? arr.map((s) => ({ ...s })) : [];
  } catch {
    return [];
  }
}
function applySteps() {
  const cfg = stepConfig.value;
  const numberTypes = new Set(
    (cfg?.types || []).filter((t) => t.number).map((t) => t.value)
  );
  form.command = JSON.stringify(
    steps.value
      .filter((s) => s.type)
      .map((s) => ({
        type: s.type,
        command:
          numberTypes.has(s.type) && s.command !== "" && s.command !== null
            ? Number(s.command)
            : s.command,
      }))
  );
}
function addStep() {
  steps.value.push({ ...(stepConfig.value?.addDefaults || { type: "delay", command: "100" }) });
}
function removeStep(i) {
  steps.value.splice(i, 1);
}
function stepBoardChange(step, event) {
  step.command =
    event.target.value === "" ? "" : JSON.stringify({ id: Number(event.target.value) });
}
function stepBoardId(step) {
  try {
    return JSON.parse(step.command || "{}").id ?? "";
  } catch {
    return "";
  }
}
function stepTypeMeta(type) {
  return stepConfig.value?.types.find((t) => t.value === type) || null;
}

// ---- gesture editor ("Gesty") ----------------------------------------------

// Custom gestures live in the options column as JSON, the same shape the
// server and the tablet client already read:
//   {"gestures": ["long-press"], "gesture_actions": {"long-press": {...}}}
// A declared gesture without a `gesture_actions` entry fires the tile's own
// action; an entry ({type, command}) replaces it per gesture. Only
// button/toggle tiles offer the section - sliders keep their drag surface
// and display modes have nothing to trigger.
const GESTURE_DEFS = [
  { key: "long-press", label: "Przytrzymanie" },
  { key: "double-tap", label: "Podwójne tapnięcie" },
  { key: "swipe-left", label: "Przeciągnięcie w lewo" },
  { key: "swipe-right", label: "Przeciągnięcie w prawo" },
];

const gestures = reactive({});
function loadGestures() {
  for (const k of Object.keys(gestures)) delete gestures[k];
  let obj = null;
  try {
    const parsed = JSON.parse(form.options || "");
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) obj = parsed;
  } catch {}
  const declared = new Set(
    Array.isArray(obj?.gestures) ? obj.gestures.filter((g) => typeof g === "string") : []
  );
  const actions =
    obj?.gesture_actions && typeof obj.gesture_actions === "object"
      ? obj.gesture_actions
      : {};
  for (const def of GESTURE_DEFS) {
    const action = actions[def.key];
    const g = {
      enabled: declared.has(def.key) || Boolean(action),
      type: action && typeof action === "object" ? String(action.type || "") : "",
      command: action && typeof action === "object" ? String(action.command ?? "") : "",
      fields: {},
    };
    gestures[def.key] = g;
    loadGestureFields(def.key);
  }
}

// same field machinery as the main form, scoped to one gesture's picked
// action: fields load from the stored command and merge back into it
function loadGestureFields(key) {
  const g = gestures[key];
  for (const k of Object.keys(g.fields)) delete g.fields[k];
  const shaped = fieldsShapeFor(g.type);
  if (!shaped.length) return;
  let obj = {};
  if (shaped.some((f) => f.key !== "")) {
    try {
      obj = JSON.parse(g.command || "{}") || {};
    } catch {
      obj = {};
    }
  }
  for (const f of shaped) {
    g.fields[f.key] = f.key === "" ? g.command || "" : obj[f.key] ?? "";
  }
  for (const f of shaped) {
    if (f.kind === "select" && g.fields[f.key] === "" && f.options.length) {
      g.fields[f.key] = f.options[0].value;
    }
  }
}

function gestureFieldVisible(g, f) {
  return !f.showIf || g.fields[f.showIf.key] === f.showIf.value;
}

function applyGestureFields(key) {
  const g = gestures[key];
  const shaped = fieldsShapeFor(g.type);
  if (!shaped.length) return;
  if (shaped.every((f) => f.key === "")) {
    g.command = g.fields[""] ?? "";
    return;
  }
  let obj = {};
  try {
    obj = JSON.parse(g.command || "{}") || {};
  } catch {
    obj = {};
  }
  for (const f of shaped) {
    const raw = g.fields[f.key];
    if (f.kind === "number") {
      if (raw !== "" && raw !== null && !Number.isNaN(Number(raw))) {
        obj[f.key] = Number(raw);
      } else if (gestureFieldVisible(g, f) && (raw === "" || raw === null)) {
        delete obj[f.key];
      }
    } else if (raw !== "") {
      obj[f.key] = raw;
    } else if (gestureFieldVisible(g, f)) {
      delete obj[f.key];
    }
  }
  g.command = JSON.stringify(obj);
}

function onGestureTypePicked(key, value) {
  const g = gestures[key];
  g.type = value;
  if (!value) g.command = "";
  loadGestureFields(key);
}

// picker groups for a gesture's action override: a leading entry for "run
// the tile's own action" (value "") on top of the shared action catalog
const gesturePickerGroups = computed(() => [
  {
    header: "Gest",
    items: [
      { value: "", label: "Akcja kafla (domyślnie)", icon: "hand-pointer", color: "#4a6a8a" },
    ],
  },
  ...actionGroups.value,
]);

function gestureFieldList(key) {
  return fieldsShapeFor(gestures[key].type);
}

// the tile modes gestures make sense for; slider/knob keep the drag
// surface for the value and display modes have nothing to trigger
const gesturesAvailable = computed(() =>
  ["button", "toggle"].includes(form.mode || "button")
);

const gesturesActive = computed(() =>
  GESTURE_DEFS.some((def) => gestures[def.key]?.enabled)
);

// gestures ride the options JSON object, so the field cannot simultaneously
// be the plain-string program arguments of the Run Program tile - an
// inline conflict instead of a silent data loss on save
const gestureError = computed(() => {
  if (!gesturesActive.value) return "";
  const raw = String(form.options || "").trim();
  if (!raw) return "";
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return "";
  } catch {}
  return "Pole „Opcje” zawiera już argumenty programu - gesty wymagają zapisu JSON i nie mogą współdzielić tego pola.";
});

function applyGestures() {
  const enabled = GESTURE_DEFS.filter((def) => gestures[def.key].enabled);
  const raw = String(form.options || "").trim();
  if (!enabled.length) {
    // nothing declared: strip the gesture keys from an options JSON that
    // carries them (a previous configuration), leave any other options
    // byte-identical
    if (!raw) return true;
    try {
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return true;
      if (!("gestures" in parsed) && !("gesture_actions" in parsed)) return true;
      delete parsed.gestures;
      delete parsed.gesture_actions;
      form.options = JSON.stringify(parsed);
    } catch {}
    return true;
  }
  if (gestureError.value) return false;
  let obj = {};
  if (raw) {
    try {
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        return false;
      }
      obj = parsed;
    } catch {
      return false;
    }
  }
  const list = [];
  const actions = {};
  for (const def of enabled) {
    applyGestureFields(def.key);
    list.push(def.key);
    const g = gestures[def.key];
    if (g.type) actions[def.key] = { type: g.type, command: g.command ?? "" };
  }
  obj.gestures = list;
  if (Object.keys(actions).length) obj.gesture_actions = actions;
  else delete obj.gesture_actions;
  form.options = JSON.stringify(obj);
  return true;
}
loadGestures();

// ---- image -----------------------------------------------------------------

const imageError = ref("");
// img/img2 never enter the dirty snapshot below (they are multi-MB base64
// strings); pickImage is their only writer, so it flips this flag instead
const imagesDirty = ref(false);
async function pickImage(field) {
  const path = await open({
    multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif", "svg"] }],
  });
  if (!path) return;
  try {
    const data = await api.readImageData(path);
    if (data !== form[field]) imagesDirty.value = true;
    form[field] = data;
  } catch (e) {
    imageError.value = String(e);
  }
}

// ---- save / delete ---------------------------------------------------------

// the delete confirmation lives in App.vue's tileDeleted (012 A9), the
// single choke point for both this dialog and the context menu
function removeTile() {
  emit("delete");
}

// ---- dirty guard -----------------------------------------------------------

// Overlay click only closes the dialog when nothing was edited (012 A9):
// an accidental click outside must not discard a configured tile. The
// snapshot covers every writer: the form copy plus the command-mapping
// reactives (fields, steps, boardId) that merge into form.command and the
// gesture states that merge into form.options on save.
// img/img2 are excluded - stringifying up to ~20 MB of base64 on every
// keystroke stutters the dialog (DESK-04); those fields only change
// through pickImage, which flips imagesDirty above.
function formSnapshot() {
  const plain = { ...form };
  delete plain.img;
  delete plain.img2;
  return JSON.stringify({
    form: plain,
    fields: { ...fields },
    steps: steps.value,
    boardId: boardId.value,
    gestures: JSON.parse(JSON.stringify(gestures)),
  });
}
const initialSnapshot = formSnapshot();
const dirty = computed(() => formSnapshot() !== initialSnapshot || imagesDirty.value);

async function requestClose() {
  if (!dirty.value) {
    emit("close");
    return;
  }
  const ok = await ask("Zamknąć okno i porzucić niezapisane zmiany?", {
    title: "Niezapisane zmiany",
    kind: "warning",
  });
  if (ok) emit("close");
}

function overlayClose() {
  requestClose();
}

function save() {
  // each writer owns the command only for its own type - the others must
  // leave the stored command untouched
  applyFields();
  if (form.type === "board") applyBoardId();
  if (stepConfig.value) applySteps();
  if (!applyGestures()) return; // inline gesture conflict (see gestureError)
  emit(isCreate.value ? "create" : "save", { ...form, ...resolveTypeMeta() });
}

// tiles added from outside the catalog keep their stored icon/color
function resolveTypeMeta() {
  const entry = catalogEntry.value;
  if (!entry) return {};
  const out = {};
  if (entry.mode) out.mode = entry.mode;
  if (entry.extInput?.command && !form.command) out.command = entry.extInput.command;
  if (entry.init && !form.command) out.command = entry.init;
  return out;
}

// ---- property rows (original "New Button" dialog) --------------------------

// one open popover at a time; 'image' opens the file dialog instead
const openProp = ref(null);
function toggleProp(key) {
  if (key === "image") {
    pickImage("img");
    return;
  }
  // fixed-mode catalog entries (speaker-volume slider, ai-* status/graph)
  // lock the Tile Mode row: save() forces the catalog mode back, so the
  // popover would offer choices that can never take effect (DESK-11)
  if (key === "mode" && catalogEntry.value?.mode) return;
  openProp.value = openProp.value === key ? null : key;
}
function closeProps() {
  openProp.value = null;
}

const ICONS = [
  "keyboard", "mouse", "font", "link", "folder", "cog", "file", "camera",
  "play", "pause", "stop", "forward", "backward", "volume-up", "volume-mute",
  "microphone", "microphone-slash", "headphones", "music", "video", "image",
  "star", "heart", "bell", "bookmark", "calendar", "check", "clock", "cloud",
  "comment", "compass", "copy", "download", "upload", "edit", "envelope",
  "eye", "eye-slash", "fire", "flag", "gamepad", "hand-pointer", "home",
  "lightbulb", "list", "lock", "unlock", "map-marker", "moon", "sun",
  "paper-plane", "phone", "plus", "power-off", "redo", "undo", "sync",
  "sliders-h", "trash", "user", "users", "wifi", "wrench", "bolt", "tv",
  "desktop", "mobile", "puzzle-piece", "rocket", "search", "share", "tag",
];

// preview falls back to the action's own icon/color like the original
const effIcon = computed(() => form.icon || catalogEntry.value?.icon || "");
const effColor = computed(() => form.color || catalogEntry.value?.color || "#2c3e50");
const prevShape = computed(() => (form.shape === 1 ? "50%" : "8px"));
const titlePos = computed(() => form.title_position ?? 0);
function colorOr(val, fallback) {
  return val || fallback;
}
</script>

<template>
  <div class="overlay" @click.self="overlayClose">
    <div class="modal btn-modal">
      <div class="modal-head" :style="{ '--tint': boardBackground }">
        {{ isCreate ? "New Button" : "Edit Button" }}
      </div>

      <div class="btn-body">
        <!-- left column: live preview + property table -->
        <div class="left-col">
          <div class="preview" :style="{ '--tint': boardBackground }">
            <div
              class="prev-tile"
              :style="{
                background: effColor,
                borderColor: form.border_color || 'transparent',
                borderRadius: prevShape,
              }"
            >
              <img v-if="form.img" class="prev-img" :src="form.img" alt="" />
              <i
                v-if="effIcon"
                class="fas"
                :class="'fa-' + effIcon"
                :style="{ color: colorOr(form.icon_color, '#ffffff') }"
              ></i>
              <span
                v-if="form.title"
                class="prev-title"
                :class="`pos-${titlePos}`"
                :style="{
                  color: colorOr(form.title_color, '#ffffff'),
                  background: form.title_box_color || 'transparent',
                }"
              >{{ form.title }}</span>
            </div>
          </div>

          <div class="props">
            <div v-for="row in [
                 ['shape', 'Shape'],
                 ['color', 'Background Color'],
                 ['border_color', 'Border Color'],
                 ['icon', 'Icon'],
                 ['icon_color', 'Icon Color'],
                 ['title_color', 'Text Color'],
                 ['title_box_color', 'Text Box Color'],
                 ['title_position', 'Text Position'],
                 ['mode', 'Tile Mode'],
                 ['image', 'Image'],
               ]" :key="row[0]" class="prop-wrap">
              <button class="prop-row" @click="toggleProp(row[0])">
                <span class="plabel">{{ row[1] }}</span>
                <span class="pvalue">
                  <template v-if="row[0] === 'shape'">{{ form.shape === 1 ? "Round" : "Square" }}</template>
                  <template v-else-if="row[0] === 'title_position'">
                    {{ ["Bottom", "Center", "Top"][form.title_position ?? 0] }}
                  </template>
                  <template v-else-if="row[0] === 'mode'">{{ prettify(form.mode || "button") }}</template>
                  <template v-else-if="row[0] === 'icon'">
                    <i v-if="form.icon || effIcon" class="fas" :class="'fa-' + (form.icon || effIcon)"></i>
                    <template v-else>N/A</template>
                  </template>
                  <template v-else-if="row[0] === 'image'">
                    {{ form.img ? "Replace" : "Pick" }}
                  </template>
                  <template v-else>
                    <span
                      v-if="form[row[0]]"
                      class="chip"
                      :style="{ background: form[row[0]] }"
                    ></span>
                    {{ form[row[0]] || "N/A" }}
                  </template>
                </span>
              </button>

              <Transition name="pop">
                <div v-if="openProp === row[0]" class="prop-pop">
                  <!-- shape -->
                  <div v-if="row[0] === 'shape'" class="opt-row">
                    <button class="opt" :class="{ on: form.shape !== 1 }" @click="form.shape = 0; closeProps()">Square</button>
                    <button class="opt" :class="{ on: form.shape === 1 }" @click="form.shape = 1; closeProps()">Round</button>
                  </div>

                  <!-- text position -->
                  <div v-else-if="row[0] === 'title_position'" class="opt-row">
                    <button class="opt" :class="{ on: (form.title_position ?? 0) === 2 }" @click="form.title_position = 2; closeProps()">
                      <i class="fas fa-arrow-up"></i> Top
                    </button>
                    <button class="opt" :class="{ on: (form.title_position ?? 0) === 1 }" @click="form.title_position = 1; closeProps()">
                      <i class="fas fa-dot-circle"></i> Center
                    </button>
                    <button class="opt" :class="{ on: (form.title_position ?? 0) === 0 }" @click="form.title_position = 0; closeProps()">
                      <i class="fas fa-arrow-down"></i> Bottom
                    </button>
                  </div>

                  <!-- tile mode -->
                  <div v-else-if="row[0] === 'mode'" class="opt-row">
                    <button class="opt" :class="{ on: (form.mode || 'button') === 'button' }" @click="form.mode = 'button'; closeProps()">Button</button>
                    <button class="opt" :class="{ on: form.mode === 'toggle' }" @click="form.mode = 'toggle'; closeProps()">Toggle</button>
                    <button class="opt" :class="{ on: form.mode === 'slider' }" @click="form.mode = 'slider'; closeProps()">Slider</button>
                  </div>

                  <!-- icon -->
                  <template v-else-if="row[0] === 'icon'">
                    <div class="icon-edit">
                      <input v-model="form.icon" placeholder="fontawesome 5 icon name" @keydown.enter.prevent />
                      <i v-if="form.icon" class="fas big" :class="'fa-' + form.icon"></i>
                    </div>
                    <div class="icon-grid">
                      <button
                        v-for="ic in ICONS"
                        :key="ic"
                        class="icon-cell"
                        :class="{ on: form.icon === ic }"
                        :title="ic"
                        @click="form.icon = ic"
                      >
                        <i class="fas" :class="'fa-' + ic"></i>
                      </button>
                    </div>
                  </template>

                  <!-- colors -->
                  <template v-else>
                    <div class="color-edit">
                      <input
                        type="color"
                        class="swatch"
                        :value="form[row[0]] || '#000000'"
                        @input="form[row[0]] = $event.target.value"
                      />
                      <input
                        class="hex"
                        :value="form[row[0]]"
                        placeholder="#rrggbb"
                        @input="form[row[0]] = $event.target.value"
                        @keydown.enter.prevent
                      />
                      <button class="mini" @click="form[row[0]] = ''; closeProps()">None</button>
                    </div>
                  </template>
                </div>
              </Transition>
            </div>
          </div>
        </div>

        <!-- right column: behavior -->
        <div class="right-col">
          <label class="field">
            Label
            <input v-model="form.title" placeholder="Button label" @keydown.enter.prevent />
          </label>

          <div class="field sel-field">
            <span class="sel-label">Action</span>
            <ActionPicker
              :model-value="form.type"
              :groups="actionGroups"
              @update:model-value="onActionPicked"
            />
          </div>

          <!-- board picker -->
          <label v-if="form.type === 'board'" class="field">
            Switch to
            <select v-model="boardId">
              <option value="">- pick board -</option>
              <option v-for="b in boards.filter((b) => b.id !== form.board_id)" :key="b.id" :value="b.id">
                {{ b.name }}
              </option>
            </select>
          </label>

          <!-- select-style command (vol) -->
          <label v-else-if="catalogEntry?.select" class="field">
            {{ catalogEntry.label }}
            <select :value="form.command || catalogEntry.init" @change="applySelect">
              <option v-for="o in catalogEntry.select" :key="o.value" :value="o.value">
                {{ o.label }}
              </option>
            </select>
          </label>

          <!-- step editor (multiaction / advance keyboard macro) -->
          <template v-else-if="stepConfig">
            <div class="steps">
              <div v-for="(step, i) in steps" :key="i" class="step">
                <select v-model="step.type" class="step-type">
                  <option v-for="t in stepConfig.types" :key="t.value" :value="t.value">
                    {{ t.label }}
                  </option>
                </select>
                <select
                  v-if="stepTypeMeta(step.type)?.board"
                  :value="stepBoardId(step)"
                  @change="stepBoardChange(step, $event)"
                >
                  <option value="">- board -</option>
                  <option v-for="b in boards" :key="b.id" :value="b.id">{{ b.name }}</option>
                </select>
                <input
                  v-else
                  v-model="step.command"
                  :inputmode="stepTypeMeta(step.type)?.number ? 'numeric' : undefined"
                  :placeholder="stepTypeMeta(step.type)?.number ? 'milliseconds' : 'value'"
                />
                <button class="step-del" title="Remove step" @click="removeStep(i)">
                  <i class="fas fa-times"></i>
                </button>
              </div>
              <button class="mini add-step" @click="addStep"><i class="fas fa-plus"></i> Add step</button>
            </div>
          </template>

          <!-- structured / generic fields (catalog + extension-declared) -->
          <template v-else-if="catalogFields.length">
            <template v-for="f in catalogFields.filter(fieldVisible)" :key="f.key">
              <!-- select leaves the <label>: clicking a label would forward
                   the click to the first chip / trigger inside -->
              <div v-if="f.kind === 'select'" class="field sel-field">
                <span class="sel-label">{{ f.label }}</span>
                <SelectField v-model="fields[f.key]" :options="f.options" :label="f.label" />
              </div>
              <label v-else class="field">
                {{ f.label }}
                <textarea
                  v-if="f.kind === 'textarea'"
                  v-model="fields[f.key]"
                  rows="3"
                  :placeholder="f.placeholder"
                ></textarea>
                <input
                  v-else
                  v-model="fields[f.key]"
                  :inputmode="f.kind === 'number' ? 'numeric' : undefined"
                  :placeholder="f.placeholder"
                  @keydown.enter.prevent
                />
              </label>
            </template>
          </template>

          <!-- extension action without declared options: command is fixed -->
          <div v-else-if="catalogEntry?.extInput" class="no-opts">
            This action has no options.
          </div>

          <!-- unknown / custom type -->
          <label v-else class="field">
            Command
            <input v-model="form.command" placeholder="command" @keydown.enter.prevent />
          </label>

          <template v-if="isMediaTile">
            <div class="field sel-field">
              <span class="sel-label">Aplikacja (opcjonalnie)</span>
              <SelectField
                :model-value="mediaApp"
                :options="[
                  { value: '', label: '- bieżąca sesja -' },
                  ...mediaApps.map((a) => ({ value: a, label: a })),
                ]"
                label="Aplikacja"
                @update:model-value="setMediaApp"
              />
            </div>
          </template>

          <template v-if="isPlanTile">
            <div class="field">
              Usage windows
              <div class="win-row">
                <label class="win-check">
                  <input
                    type="checkbox"
                    :checked="planWindows.five"
                    @change="setPlanWindow('five', $event)"
                  />
                  5-hour
                </label>
                <label class="win-check">
                  <input
                    type="checkbox"
                    :checked="planWindows.week"
                    @change="setPlanWindow('week', $event)"
                  />
                  Weekly
                </label>
              </div>
            </div>
          </template>

          <label v-if="catalogEntry?.options" class="field">
            Options (program arguments)
            <input v-model="form.options" placeholder="--flag" @keydown.enter.prevent />
          </label>

          <!-- second state for toggles -->
          <template v-if="showDual">
            <div class="dual-head">Second state (toggle)</div>
            <div class="dual-grid">
              <label class="field">
                Background 2
                <input
                  type="color"
                  class="color-inline"
                  :value="form.color2 || '#22313f'"
                  @input="form.color2 = $event.target.value"
                />
              </label>
              <label class="field">
                Icon 2
                <input v-model="form.icon2" placeholder="icon name" @keydown.enter.prevent />
              </label>
            </div>
            <button class="mini" @click="pickImage('img2')">
              <i class="fas fa-image"></i> {{ form.img2 ? "Replace image 2" : "Pick image 2" }}
            </button>
            <div v-if="imageError" class="dual-error">{{ imageError }}</div>
          </template>

          <!-- custom gestures (desktop touch mode + tablets) -->
          <template v-if="gesturesAvailable">
            <div class="dual-head">Gesty</div>
            <p class="gesture-note">
              Zaznacz gest i opcjonalnie wybierz akcję - bez wyboru gest uruchamia
              akcję kafla. Działa w trybie dotykowym i na tabletach.
            </p>
            <div v-for="def in GESTURE_DEFS" :key="def.key" class="gesture-row">
              <label class="gesture-check">
                <input type="checkbox" v-model="gestures[def.key].enabled" />
                {{ def.label }}
              </label>
              <template v-if="gestures[def.key].enabled">
                <div class="field sel-field gesture-action">
                  <span class="sel-label">Akcja</span>
                  <ActionPicker
                    :model-value="gestures[def.key].type"
                    :groups="gesturePickerGroups"
                    @update:model-value="onGestureTypePicked(def.key, $event)"
                  />
                </div>
                <template v-for="f in gestureFieldList(def.key).filter((f) => gestureFieldVisible(gestures[def.key], f))" :key="f.key">
                  <div v-if="f.kind === 'select'" class="field sel-field">
                    <span class="sel-label">{{ f.label }}</span>
                    <SelectField v-model="gestures[def.key].fields[f.key]" :options="f.options" :label="f.label" />
                  </div>
                  <label v-else class="field">
                    {{ f.label }}
                    <textarea
                      v-if="f.kind === 'textarea'"
                      v-model="gestures[def.key].fields[f.key]"
                      rows="2"
                      :placeholder="f.placeholder"
                    ></textarea>
                    <input
                      v-else
                      v-model="gestures[def.key].fields[f.key]"
                      :inputmode="f.kind === 'number' ? 'numeric' : undefined"
                      :placeholder="f.placeholder"
                      @keydown.enter.prevent
                    />
                  </label>
                </template>
              </template>
            </div>
            <div v-if="gestureError" class="dual-error">{{ gestureError }}</div>
          </template>
        </div>
      </div>

      <div class="modal-actions">
        <button v-if="!isCreate" class="btn-text danger left" @click="removeTile">Delete</button>
        <button class="btn-text" @click="requestClose">Cancel</button>
        <button class="btn-text accent" @click="save">{{ isCreate ? "Add" : "Save" }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.btn-modal { width: min(680px, 94vw); padding: 0; overflow: hidden; }
.modal-head {
  background: color-mix(in srgb, var(--tint) 82%, black);
  color: #f4fbfa;
  font-size: 21px;
  font-weight: 400;
  padding: 15px 24px;
  flex: none;
}

.btn-body {
  display: flex;
  gap: 0;
  min-height: 0;
  overflow: auto;
}

.left-col {
  width: 272px;
  flex: none;
  border-right: 1px solid var(--modal-line);
}
.preview {
  background: var(--tint);
  height: 168px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.prev-tile {
  position: relative;
  width: 116px;
  height: 116px;
  border: 2px solid transparent;
  display: flex;
  align-items: center;
  justify-content: center;
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.3);
  overflow: hidden;
}
.prev-img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.prev-tile > i { font-size: 38px; text-shadow: 0 1px 3px rgba(0, 0, 0, 0.3); }
.prev-title {
  position: absolute;
  left: 0;
  right: 0;
  padding: 2px 5px;
  font-size: 12px;
  font-weight: 500;
  text-align: center;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
}
.prev-title.pos-0 { bottom: 0; }
.prev-title.pos-1 { top: 50%; transform: translateY(-50%); text-shadow: none; }
.prev-title.pos-2 { top: 0; }

.props { position: relative; }
.prop-wrap { position: relative; }
.prop-row {
  display: grid;
  grid-template-columns: 1fr 1fr;
  align-items: center;
  width: 100%;
  text-align: left;
  min-height: 37px;
  padding: 4px 14px;
  border-bottom: 1px solid var(--modal-line);
  font-size: 13.5px;
  transition: background 120ms ease-out;
}
.prop-row:hover { background: #f6f6f6; }
.plabel { color: var(--modal-muted); }
.pvalue {
  justify-self: center;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  font-weight: 700;
  color: var(--modal-text);
  max-width: 120px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.chip {
  width: 13px;
  height: 13px;
  border-radius: 3px;
  border: 1px solid rgba(0, 0, 0, 0.15);
  flex: none;
}

.prop-pop {
  position: absolute;
  left: 10px;
  right: 10px;
  top: calc(100% - 6px);
  background: #fff;
  border-radius: 4px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.28);
  padding: 10px;
  z-index: 6;
}
.opt-row { display: flex; gap: 6px; }
.opt {
  flex: 1;
  padding: 8px 6px;
  border-radius: 4px;
  background: var(--modal-field);
  font-size: 13px;
  transition: background 120ms ease-out, color 120ms ease-out;
}
.opt:hover { background: #e6e6e6; }
.opt.on { background: var(--accent); color: #fff; }

.icon-edit { display: flex; align-items: center; gap: 10px; margin-bottom: 8px; }
.icon-edit input { flex: 1; padding: 6px 8px; font-size: 13px; }
.icon-edit .big { font-size: 22px; width: 26px; text-align: center; }
.icon-grid {
  display: grid;
  grid-template-columns: repeat(8, 1fr);
  gap: 2px;
  max-height: 168px;
  overflow-y: auto;
}
.icon-cell {
  height: 30px;
  border-radius: 4px;
  font-size: 14px;
  color: #4a4a4a;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 120ms ease-out, color 120ms ease-out;
}
.icon-cell:hover { background: var(--modal-field); }
.icon-cell.on { background: var(--accent); color: #fff; }

.color-edit { display: flex; align-items: center; gap: 8px; }
.color-edit .swatch {
  width: 42px;
  height: 34px;
  padding: 2px;
  flex: none;
  cursor: pointer;
}
.color-edit .hex { flex: 1; min-width: 0; padding: 6px 8px; font-size: 13px; }

.right-col {
  flex: 1;
  min-width: 0;
  padding: 16px 20px 8px;
}
.right-col .field { margin-bottom: 13px; }
.win-row {
  display: flex;
  gap: 16px;
  margin-top: 6px;
}
.win-check {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: #2c3e50;
  cursor: pointer;
}
.no-opts {
  font-size: 13px;
  color: var(--modal-muted);
  background: var(--modal-field);
  border-radius: 4px;
  padding: 10px 12px;
  margin-bottom: 13px;
}
.steps { display: flex; flex-direction: column; gap: 6px; margin-bottom: 13px; }
.step { display: flex; gap: 6px; }
.step-type { max-width: 150px; flex: none; }
.step-del {
  width: 32px;
  flex: none;
  border-radius: 4px;
  color: var(--modal-muted);
  transition: background 120ms ease-out, color 120ms ease-out;
}
.step-del:hover { background: rgba(231, 76, 60, 0.12); color: var(--danger); }
.add-step { align-self: flex-start; }
.dual-head {
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.7px;
  color: var(--accent-2);
  border-top: 1px solid var(--modal-line);
  padding-top: 12px;
  margin-top: 2px;
  margin-bottom: 8px;
}
.dual-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.color-inline { height: 36px; padding: 3px; cursor: pointer; }
.dual-error { font-size: 12px; color: var(--danger); margin-top: 6px; overflow-wrap: anywhere; }

.gesture-note {
  font-size: 12px;
  line-height: 1.5;
  color: var(--modal-muted);
  margin: 0 0 10px;
}
.gesture-row { margin-bottom: 10px; }
.gesture-check {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 13.5px;
  color: var(--modal-text);
  cursor: pointer;
}
.gesture-action { margin-top: 8px; }

.modal-actions {
  border-top: 1px solid var(--modal-line);
  padding: 10px 16px 12px;
  flex: none;
}
.mini {
  font-size: 12.5px;
  padding: 6px 10px;
  border-radius: 4px;
  background: var(--modal-field);
  transition: background 120ms ease-out;
}
.mini:hover { background: #e6e6e6; }

.pop-enter-active { transition: opacity 130ms ease-out, transform 130ms cubic-bezier(0.2, 0, 0, 1); }
.pop-leave-active { transition: opacity 90ms ease-out; }
.pop-enter-from { opacity: 0; transform: translateY(-4px); }
.pop-leave-to { opacity: 0; }
</style>
