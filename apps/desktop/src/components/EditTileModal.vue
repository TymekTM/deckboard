<script setup>
import { computed, reactive, ref } from "vue";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { CATALOG, DUAL_STATE_TYPES, MULTIACTION_STEPS } from "../catalog";
import { api } from "../api";

const props = defineProps({
  button: { type: Object, required: true },
  boards: { type: Array, required: true },
  boardBackground: { type: String, default: "#437072" },
});
const emit = defineEmits(["save", "delete", "close"]);

// editable copy: every column the edit dialog touches
const form = reactive(JSON.parse(JSON.stringify(props.button)));

const catalogEntry = computed(
  () => CATALOG.find((c) => c.value === form.type) || null
);
const isKnownType = computed(() => catalogEntry.value !== null);
const showDual = computed(
  () =>
    DUAL_STATE_TYPES.has(form.type) ||
    Boolean(form.color2 || form.icon2 || form.img2)
);

// ---- action dropdown groups (headers/dividers from the catalog) -----------

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
  return groups;
});

// ---- command fields --------------------------------------------------------

// field values derived from `command`: key "" is the raw command, other keys
// live inside the JSON object stored in command
const fields = reactive({});
function loadFields() {
  for (const k of Object.keys(fields)) delete fields[k];
  const entry = catalogEntry.value;
  if (!entry || !entry.fields) return;
  let obj = {};
  if (entry.fields.some((f) => f.key !== "")) {
    try {
      obj = JSON.parse(form.command || "{}") || {};
    } catch {
      obj = {};
    }
  }
  for (const f of entry.fields) {
    fields[f.key] = f.key === "" ? form.command || "" : obj[f.key] ?? "";
  }
}
function applyFields() {
  const entry = catalogEntry.value;
  if (!entry || !entry.fields) return;
  if (entry.fields.every((f) => f.key === "")) {
    form.command = fields[""] ?? "";
    return;
  }
  const obj = {};
  for (const f of entry.fields) {
    if (f.key !== "" && fields[f.key] !== "") obj[f.key] = fields[f.key];
  }
  form.command = JSON.stringify(obj);
}
loadFields();

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

// select fields (type "vol": command is the raw value)
function applySelect(event) {
  form.command = event.target.value;
}

// ---- multiaction -----------------------------------------------------------

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
  form.command = JSON.stringify(steps.value.filter((s) => s.type));
}
function addStep() {
  steps.value.push({ type: "delay", command: "100" });
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

// ---- image -----------------------------------------------------------------

async function pickImage(field) {
  const path = await open({
    multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif", "svg"] }],
  });
  if (!path) return;
  try {
    form[field] = await api.readImageData(path);
  } catch (e) {
    imageError.value = String(e);
  }
}
const imageError = ref("");

// ---- save ------------------------------------------------------------------

async function removeTile() {
  const ok = await ask(`Delete tile "${form.title || form.type}"?`, {
    title: "Delete tile",
    kind: "warning",
  });
  if (ok) emit("delete");
}

function save() {
  // each writer owns the command only for its own type - the others must
  // leave the stored command untouched
  applyFields();
  if (form.type === "board") applyBoardId();
  if (form.type === "multiaction") applySteps();
  emit("save", { ...form, ...resolveTypeMeta() });
}

// tiles added from outside the catalog keep their stored icon/color
function resolveTypeMeta() {
  const entry = catalogEntry.value;
  if (!entry) return {};
  const out = {};
  if (entry.mode) out.mode = entry.mode;
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

// preview helpers mirror GridEditor's tile rendering
const prevShape = computed(() => (form.shape === 1 ? "50%" : "8px"));
const titlePos = computed(() => form.title_position ?? 0);
function colorOr(val, fallback) {
  return val || fallback;
}
function hexOf(val) {
  return val || "";
}
function setHex(field, value) {
  form[field] = value;
}
</script>

<template>
  <div class="overlay" @click.self="emit('close')">
    <div class="modal btn-modal">
      <div class="modal-head" :style="{ '--tint': boardBackground }">
        {{ button.id ? "Edit Button" : "New Button" }}
      </div>

      <div class="btn-body">
        <!-- left column: live preview + property table -->
        <div class="left-col">
          <div class="preview" :style="{ '--tint': boardBackground }">
            <div
              class="prev-tile"
              :style="{
                background: colorOr(form.color, '#2c3e50'),
                borderColor: form.border_color || 'transparent',
                borderRadius: prevShape,
              }"
            >
              <img v-if="form.img" class="prev-img" :src="form.img" alt="" />
              <i
                v-if="form.icon"
                class="fas"
                :class="'fa-' + form.icon"
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

          <div class="props" @keydown.esc="closeProps">
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
                  <template v-else-if="row[0] === 'mode'">{{ form.mode || "button" }}</template>
                  <template v-else-if="row[0] === 'icon'">
                    <i v-if="form.icon" class="fas" :class="'fa-' + form.icon"></i>
                    <template v-else>N/A</template>
                  </template>
                  <template v-else-if="row[0] === 'image'">
                    {{ form.img ? "Replace" : "Pick" }}
                  </template>
                  <template v-else-if="row[0].endsWith('_color') || row[0] === 'color'">
                    <span
                      v-if="hexOf(form[row[0]])"
                      class="chip"
                      :style="{ background: form[row[0]] }"
                    ></span>
                    {{ hexOf(form[row[0]]) || "N/A" }}
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
                        @input="setHex(row[0], $event.target.value)"
                      />
                      <input
                        class="hex"
                        :value="form[row[0]]"
                        placeholder="#rrggbb"
                        @input="setHex(row[0], $event.target.value)"
                        @keydown.enter.prevent
                      />
                      <button class="mini" @click="setHex(row[0], ''); closeProps()">None</button>
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

          <label class="field">
            Action
            <select v-model="form.type" @change="loadFields(); closeProps()">
              <optgroup v-for="g in actionGroups" :key="g.header" :label="g.header">
                <option v-for="c in g.items" :key="c.value" :value="c.value">{{ c.label }}</option>
              </optgroup>
              <option v-if="!isKnownType" :value="form.type">{{ form.type }} (custom)</option>
            </select>
          </label>

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

          <!-- multiaction step editor -->
          <template v-else-if="form.type === 'multiaction'">
            <div class="steps">
              <div v-for="(step, i) in steps" :key="i" class="step">
                <select v-model="step.type" class="step-type">
                  <option v-for="s in MULTIACTION_STEPS" :key="s.value" :value="s.value">
                    {{ s.label }}
                  </option>
                </select>
                <select
                  v-if="step.type === 'board'"
                  :value="stepBoardId(step)"
                  @change="stepBoardChange(step, $event)"
                >
                  <option value="">- board -</option>
                  <option v-for="b in boards" :key="b.id" :value="b.id">{{ b.name }}</option>
                </select>
                <input
                  v-else
                  v-model="step.command"
                  :placeholder="step.type === 'delay' ? 'milliseconds' : 'value'"
                />
                <button class="step-del" title="Remove step" @click="removeStep(i)">
                  <i class="fas fa-times"></i>
                </button>
              </div>
              <button class="mini add-step" @click="addStep"><i class="fas fa-plus"></i> Add step</button>
            </div>
          </template>

          <!-- generic fields -->
          <template v-else-if="catalogEntry?.fields">
            <label v-for="f in catalogEntry.fields" :key="f.key" class="field">
              {{ f.label }}
              <textarea
                v-if="f.multiline"
                v-model="fields[f.key]"
                rows="3"
                :placeholder="f.placeholder"
              ></textarea>
              <input v-else v-model="fields[f.key]" :placeholder="f.placeholder" @keydown.enter.prevent />
            </label>
          </template>

          <!-- unknown / custom type -->
          <label v-else class="field">
            Command
            <input v-model="form.command" placeholder="command" @keydown.enter.prevent />
          </label>

          <label class="field">
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
        </div>
      </div>

      <div class="modal-actions">
        <button class="btn-text danger left" @click="removeTile">Delete</button>
        <button class="btn-text" @click="emit('close')">Cancel</button>
        <button class="btn-text accent" @click="save">{{ button.id ? "Save" : "Add" }}</button>
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
.steps { display: flex; flex-direction: column; gap: 6px; margin-bottom: 13px; }
.step { display: flex; gap: 6px; }
.step-type { max-width: 140px; flex: none; }
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
