<script setup>
import { CATALOG } from "../catalog";

defineEmits(["pick", "close"]);
const groups = [];
let current = { header: "Actions", items: [] };
for (const entry of CATALOG) {
  if (entry.header) {
    if (current.items.length) groups.push(current);
    current = { header: entry.header, items: [] };
  } else if (!entry.divider) {
    current.items.push(entry);
  }
}
if (current.items.length) groups.push(current);
</script>

<template>
  <div class="overlay" @click.self="$emit('close')">
    <div class="modal">
      <h2>Add tile</h2>
      <div v-for="g in groups" :key="g.header" class="group">
        <div class="header">{{ g.header }}</div>
        <button
          v-for="item in g.items"
          :key="item.value"
          class="pick"
          @click="$emit('pick', item.value)"
        >
          <i v-if="item.icon" class="fas" :class="'fa-' + item.icon"></i>
          {{ item.label }}
        </button>
      </div>
      <div class="actions">
        <button @click="$emit('close')">Cancel</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.group { margin-bottom: 10px; }
.header {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.6px;
  color: var(--text-muted);
  margin: 6px 0;
}
.pick {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  text-align: left;
  margin-bottom: 4px;
}
.pick i { width: 18px; text-align: center; color: var(--accent); }
</style>
