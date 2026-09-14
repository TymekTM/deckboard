<script setup>
import { reactive, ref } from "vue";
import { api } from "../api";

const props = defineProps({
  mode: { type: String, required: true }, // 'create' | 'edit'
  board: { type: Object, default: null },
});
const emit = defineEmits(["close", "saved"]);

const form = reactive({
  name: props.board?.name ?? "",
  width: props.board?.width ?? 4,
  height: props.board?.height ?? 3,
  background: props.board?.background ?? "#2c3e50",
});
const error = ref("");

async function save() {
  if (props.mode === "create") {
    await api.createBoard(form.name || "New board");
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
      <h2>{{ mode === 'create' ? 'New board' : 'Edit board' }}</h2>
      <label class="field">
        Name
        <input v-model="form.name" placeholder="Board name" @keyup.enter="save" />
      </label>
      <div class="row" v-if="mode === 'edit'">
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
      <div class="actions">
        <template v-if="mode === 'edit'">
          <button class="danger left" @click="clearBoard">Clear tiles</button>
          <button class="danger left" @click="deleteBoard">Delete board</button>
        </template>
        <button @click="emit('close')">Cancel</button>
        <button class="primary" @click="save">Save</button>
      </div>
      <p v-if="error" class="error">{{ error }}</p>
    </div>
  </div>
</template>

<style scoped>
.error { color: #f08585; font-size: 13px; }
.actions { display: flex; gap: 8px; justify-content: flex-end; }
.left { margin-right: auto; }
.actions .left + .left { margin-right: 0; }
.actions .danger.left:first-child { margin-right: 0; }
.actions { justify-content: flex-end; }
</style>
