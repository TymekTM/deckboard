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
      <div class="modal-head">Add tile</div>
      <div class="list">
        <div v-for="g in groups" :key="g.header" class="group">
          <div class="header">{{ g.header }}</div>
          <button
            v-for="item in g.items"
            :key="item.value"
            class="pick"
            @click="$emit('pick', item.value)"
          >
            <span class="glyph" :style="{ background: item.color || 'var(--accent-2)' }">
              <i v-if="item.icon" class="fas" :class="'fa-' + item.icon"></i>
            </span>
            {{ item.label }}
          </button>
        </div>
      </div>
      <div class="modal-actions">
        <button class="btn-text" @click="$emit('close')">Cancel</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.list { padding: 6px 10px 0; }
.group { margin-bottom: 8px; }
.header {
  font-size: 11.5px;
  text-transform: uppercase;
  letter-spacing: 0.7px;
  color: var(--modal-muted);
  margin: 8px 6px 4px;
}
.pick {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  text-align: left;
  font-size: 14px;
  color: var(--modal-text);
  padding: 7px 8px;
  border-radius: 4px;
  transition: background 120ms ease-out;
}
.pick:hover { background: var(--modal-field); }
.glyph {
  width: 26px;
  height: 26px;
  border-radius: 6px;
  color: #fff;
  font-size: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  flex: none;
}
</style>
