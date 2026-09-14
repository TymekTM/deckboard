<script setup>
import { computed, reactive, ref } from "vue";
import { CATALOG, DUAL_STATE_TYPES, MULTIACTION_STEPS } from "../catalog";

const props = defineProps({
  button: { type: Object, required: true },
  boards: { type: Array, required: true },
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
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({
    multiple: false,
    filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
  });
  if (!path) return;
  const { readFile } = await import("@tauri-apps/plugin-fs");
  const bytes = await readFile(path);
  let b64 = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    b64 += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  const ext = path.split(".").pop().toLowerCase().replace("jpeg", "jpg");
  form[field] = `data:image/${ext};base64,${btoa(b64)}`;
}

// ---- save ------------------------------------------------------------------

function save() {
  applyFields();
  applyBoardId();
  applySteps();
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
</script>

<template>
  <div class="overlay" @click.self="emit('close')">
    <div class="modal wide">
      <h2>Edit tile</h2>

      <div class="section">
        <div class="section-title">Action</div>
        <div class="row">
          <label class="field">
            Type
            <select v-model="form.type" @change="loadFields()">
              <option v-for="c in CATALOG.filter((c) => c.value)" :key="c.value" :value="c.value">
                {{ c.label }}
              </option>
              <option v-if="!isKnownType" :value="form.type">{{ form.type }} (custom)</option>
            </select>
          </label>
          <label class="field" style="max-width: 120px">
            Mode
            <select v-model="form.mode">
              <option value="button">button</option>
              <option value="slider">slider</option>
            </select>
          </label>
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

        <!-- multiaction step editor -->
        <template v-else-if="form.type === 'multiaction'">
          <div class="steps">
            <div v-for="(step, i) in steps" :key="i" class="step">
              <select v-model="step.type" style="max-width: 130px">
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
              <button class="mini" @click="removeStep(i)">
                <i class="fas fa-times"></i>
              </button>
            </div>
            <button class="mini" @click="addStep"><i class="fas fa-plus"></i> Add step</button>
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
            <input v-else v-model="fields[f.key]" :placeholder="f.placeholder" />
          </label>
        </template>

        <!-- unknown / custom type -->
        <label v-else class="field">
          Command
          <input v-model="form.command" placeholder="command" />
        </label>

        <label class="field">
          Options (program arguments)
          <input v-model="form.options" placeholder="--flag" />
        </label>
      </div>

      <div class="section">
        <div class="section-title">Appearance</div>
        <div class="row">
          <label class="field">
            Title
            <input v-model="form.title" />
          </label>
          <label class="field" style="max-width: 110px">
            Title position
            <select v-model.number="form.title_position">
              <option :value="0">bottom</option>
              <option :value="1">center</option>
              <option :value="2">top</option>
            </select>
          </label>
        </div>
        <div class="row">
          <label class="field">
            Background
            <input v-model="form.color" type="color" class="color-input" />
          </label>
          <label class="field">
            Border
            <input v-model="form.border_color" type="color" class="color-input" />
          </label>
          <label class="field">
            Shape
            <select v-model.number="form.shape">
              <option :value="0">square</option>
              <option :value="1">circle</option>
            </select>
          </label>
        </div>
        <div class="row">
          <label class="field">
            Icon (FontAwesome 5 name)
            <div class="icon-row">
              <input v-model="form.icon" placeholder="music" />
              <i v-if="form.icon" class="fas preview" :class="'fa-' + form.icon"></i>
            </div>
          </label>
          <label class="field" style="max-width: 90px">
            Icon color
            <input v-model="form.icon_color" type="color" class="color-input" />
          </label>
          <label class="field" style="max-width: 130px">
            Image
            <button @click="pickImage('img')">
              <i class="fas fa-image"></i> {{ form.img ? 'Replace' : 'Pick' }}
            </button>
          </label>
        </div>
      </div>

      <div class="section" v-if="showDual">
        <div class="section-title">Second state (toggle)</div>
        <div class="row">
          <label class="field">
            Background 2
            <input v-model="form.color2" type="color" class="color-input" />
          </label>
          <label class="field">
            Icon 2
            <div class="icon-row">
              <input v-model="form.icon2" placeholder="microphone-slash" />
              <i v-if="form.icon2" class="fas preview" :class="'fa-' + form.icon2"></i>
            </div>
          </label>
          <label class="field" style="max-width: 130px">
            Image 2
            <button @click="pickImage('img2')">
              <i class="fas fa-image"></i> {{ form.img2 ? 'Replace' : 'Pick' }}
            </button>
          </label>
        </div>
      </div>

      <div class="actions">
        <button class="danger left" @click="emit('delete')">
          <i class="fas fa-trash"></i> Delete
        </button>
        <button @click="emit('close')">Cancel</button>
        <button class="primary" @click="save">Save</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal.wide { width: min(680px, 94vw); }
.section { margin-bottom: 16px; }
.section-title {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.6px;
  color: var(--accent);
  margin-bottom: 8px;
}
.icon-row { display: flex; gap: 8px; align-items: center; }
.preview { font-size: 20px; color: var(--text); }
.steps { display: flex; flex-direction: column; gap: 6px; }
.step { display: flex; gap: 6px; }
button.mini { padding: 4px 8px; font-size: 12px; }
button + button { margin-left: 0; }
</style>
