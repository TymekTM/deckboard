<script setup>
import { reactive } from "vue";
import { api } from "../api";

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

async function save() {
  if (props.mode === "create") {
    await api.createBoard(
      form.name || "New board",
      form.background,
      Number(form.width),
      Number(form.height)
    );
    emit("saved");
    return;
  }
  await api.updateBoard({
    ...props.board,
    name: form.name,
    width: Number(form.width),
    height: Number(form.height),
    background: form.background,
  });
  emit("saved");
}

async function clearBoard() {
  await api.clearBoard(props.board.id);
  emit("saved");
}

async function deleteBoard() {
  await api.deleteBoard(props.board.id);
  emit("saved");
}
</script>

<template>
  <div class="overlay" @click.self="emit('close')">
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
            <input v-model.number="form.width" type="number" min="1" max="15" />
          </label>
          <label class="field">
            Rows (height)
            <input v-model.number="form.height" type="number" min="1" max="15" />
          </label>
          <label class="field">
            Background
            <input v-model="form.background" type="color" class="color-input" />
          </label>
        </div>
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
</style>
