<script setup>
import { computed, reactive, ref } from "vue";
import { ask } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { MAX_BOARD_DIM, boardDim } from "../catalog";

const props = defineProps({
  mode: { type: String, required: true }, // 'create' | 'edit'
  board: { type: Object, default: null },
});
const emit = defineEmits(["close", "saved"]);

const form = reactive({
  name: props.board?.name ?? "",
  width: props.board?.width ?? 6,
  height: props.board?.height ?? 4,
  background: props.board?.background ?? "#437072",
});

// Overlay click only closes when nothing was edited (012 A9); the same
// ask()-confirmation the board context menu uses guards Clear/Delete.
const initialSnapshot = JSON.stringify({ ...form });
const dirty = computed(() => JSON.stringify({ ...form }) !== initialSnapshot);

function overlayClose() {
  if (!dirty.value) emit("close");
}

// the backend reasons are shown as-is (duplicate name, refused dimensions,
// locked db); the dialog stays open so the entered values survive
const error = ref("");

async function save() {
  // clamp dimensions to the backend's integer bounds (012 C4): the
  // number input's min/max only steer the spinner, they do not validate
  // pasted or typed values
  const width = boardDim(form.width, props.board?.width ?? 6);
  const height = boardDim(form.height, props.board?.height ?? 4);
  error.value = "";
  try {
    if (props.mode === "create") {
      await api.createBoard(form.name || "New board", form.background, width, height);
    } else {
      await api.updateBoard({
        ...props.board,
        name: form.name,
        width,
        height,
        background: form.background,
      });
    }
  } catch (e) {
    error.value = e ? String(e) : "Saving the board failed.";
    return;
  }
  emit("saved");
}

async function clearBoard() {
  const ok = await ask(`Clear every tile from "${props.board.name}"?`, {
    title: "Clear board",
    kind: "warning",
  });
  if (!ok) return;
  error.value = "";
  try {
    await api.clearBoard(props.board.id);
  } catch (e) {
    error.value = e ? String(e) : "Clearing the board failed.";
    return;
  }
  emit("saved");
}

async function deleteBoard() {
  const ok = await ask(`Delete board "${props.board.name}"?`, {
    title: "Delete board",
    kind: "warning",
  });
  if (!ok) return;
  error.value = "";
  try {
    await api.deleteBoard(props.board.id);
  } catch (e) {
    error.value = e ? String(e) : "Deleting the board failed.";
    return;
  }
  emit("saved");
}
</script>

<template>
  <div class="overlay" @click.self="overlayClose">
    <div class="modal">
      <div class="modal-head" :style="{ '--canvas-head': form.background }">
        {{ mode === "create" ? "New board" : "Edit board" }}
      </div>
      <div class="modal-body">
        <label class="field">
          Name
          <input v-model="form.name" placeholder="Board name" @keyup.enter="save" />
        </label>
        <div class="row">
          <label class="field">
            Columns (width)
            <input v-model.number="form.width" type="number" min="1" :max="MAX_BOARD_DIM" step="1" />
          </label>
          <label class="field">
            Rows (height)
            <input v-model.number="form.height" type="number" min="1" :max="MAX_BOARD_DIM" step="1" />
          </label>
          <label class="field">
            Background
            <input v-model="form.background" type="color" class="color-input" />
          </label>
        </div>
        <p v-if="error" class="modal-error" role="alert">{{ error }}</p>
      </div>
      <div class="modal-actions">
        <template v-if="mode === 'edit'">
          <button class="btn-text danger left" @click="clearBoard">Clear tiles</button>
          <button class="btn-text danger" @click="deleteBoard">Delete board</button>
        </template>
        <button class="btn-text" @click="emit('close')">Cancel</button>
        <button class="btn-text accent" @click="save">Save</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.modal-head { background: color-mix(in srgb, var(--canvas-head) 82%, black); color: #f4fbfa; }
.row { display: flex; gap: 10px; }
.row > * { flex: 1; }
.color-input { height: 38px; padding: 3px; cursor: pointer; }
.modal-error {
  font-size: 12px;
  color: var(--danger);
  margin: 10px 0 0;
  overflow-wrap: anywhere;
}
</style>
