<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

// action picker for the tile dialog: searchable, grouped listbox that
// replaces the native <select> with optgroups. Groups carry
// {header, items: [{value, label, icon?, color?}]}; unknown modelValue
// (custom stored type) shows up pinned under a "Custom" header.
const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  groups: { type: Array, default: () => [] }, // [{header, items}]
});
const emit = defineEmits(["update:modelValue"]);

const uid = `ap-${Math.random().toString(36).slice(2, 8)}`;

// ---- current selection -----------------------------------------------------

const allItems = computed(() => props.groups.flatMap((g) => g.items));
const current = computed(() =>
  allItems.value.find((it) => it.value === props.modelValue) || null
);
const currentLabel = computed(() =>
  current.value ? current.value.label : props.modelValue === "" ? "" : String(props.modelValue)
);
const currentIcon = computed(() => current.value?.icon || "cog");
const currentColor = computed(() => current.value?.color || "#7f8c8d");

// unknown type gets its own trailing group so it stays reachable
const withCustom = computed(() => {
  if (current.value || props.modelValue === "") return props.groups;
  return [
    ...props.groups,
    {
      header: "Custom",
      items: [{ value: props.modelValue, label: `${props.modelValue} (custom)` }],
    },
  ];
});

// ---- search + visible items ------------------------------------------------

const query = ref("");
const visibleGroups = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return withCustom.value;
  return withCustom.value
    .map((g) => ({
      header: g.header,
      items: g.items.filter(
        (it) =>
          it.label.toLowerCase().includes(q) ||
          String(it.value).toLowerCase().includes(q)
      ),
    }))
    .filter((g) => g.items.length);
});
const flat = computed(() => visibleGroups.value.flatMap((g) => g.items));

// ---- highlight (activedescendant pattern; focus stays on the search) -------

const open = ref(false);
const hlIdx = ref(0);
const root = ref(null);
const trigger = ref(null);
const search = ref(null);
const listEl = ref(null);

function scrollHl() {
  nextTick(() => {
    listEl.value?.querySelector(".item.hl")?.scrollIntoView({ block: "nearest" });
  });
}
watch(query, () => {
  hlIdx.value = 0;
});
watch(hlIdx, scrollHl);

function moveHl(delta) {
  const n = flat.value.length;
  if (!n) return;
  hlIdx.value = (hlIdx.value + delta + n) % n;
}
function idxOfItem(it) {
  return flat.value.indexOf(it);
}

// ---- open / close / pick ---------------------------------------------------

function openPop() {
  open.value = true;
  query.value = "";
  const cur = flat.value.findIndex((it) => it.value === props.modelValue);
  hlIdx.value = cur >= 0 ? cur : 0;
  nextTick(() => {
    search.value?.focus();
    scrollHl();
  });
}
function closePop(refocus = true) {
  if (!open.value) return;
  open.value = false;
  if (refocus) trigger.value?.focus();
}
function pick(it) {
  emit("update:modelValue", it.value);
  closePop();
}
function onDocClick(e) {
  if (open.value && root.value && !root.value.contains(e.target)) closePop(false);
}
function onDocKeydown(e) {
  if (open.value && e.key === "Escape") closePop();
}
onMounted(() => {
  document.addEventListener("click", onDocClick);
  document.addEventListener("keydown", onDocKeydown);
});
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  document.removeEventListener("keydown", onDocKeydown);
});

function onTriggerKeydown(e) {
  if (open.value) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    openPop();
  }
}
function onSearchKeydown(e) {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    moveHl(1);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    moveHl(-1);
  } else if (e.key === "Home") {
    e.preventDefault();
    hlIdx.value = 0;
  } else if (e.key === "End") {
    e.preventDefault();
    hlIdx.value = Math.max(0, flat.value.length - 1);
  } else if (e.key === "Enter") {
    e.preventDefault();
    const it = flat.value[hlIdx.value];
    if (it) pick(it);
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    if (query.value) {
      query.value = "";
    } else {
      closePop();
    }
  } else if (e.key === "Tab") {
    open.value = false;
  }
}
</script>

<template>
  <div ref="root" class="ap">
    <button
      ref="trigger"
      class="trigger"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :aria-controls="uid"
      @click="open ? closePop() : openPop()"
      @keydown="onTriggerKeydown"
    >
      <span class="chip" :style="{ background: currentColor }">
        <i class="fas" :class="'fa-' + currentIcon"></i>
      </span>
      <span class="tval">{{ currentLabel }}</span>
      <i class="fas fa-chevron-down caret" :class="{ up: open }"></i>
    </button>
    <Transition name="pop">
      <div v-if="open" class="pop">
        <div class="search-row">
          <i class="fas fa-search"></i>
          <input
            ref="search"
            v-model="query"
            type="text"
            placeholder="Search actions…"
            role="combobox"
            :aria-expanded="true"
            :aria-controls="uid"
            :aria-activedescendant="`${uid}-opt-${hlIdx}`"
            @keydown="onSearchKeydown"
          />
        </div>
        <div :id="uid" ref="listEl" class="list" role="listbox" :aria-label="'Actions'">
          <template v-for="g in visibleGroups" :key="g.header">
            <div class="ghead">{{ g.header }}</div>
            <div
              v-for="it in g.items"
              :key="it.value"
              :id="`${uid}-opt-${idxOfItem(it)}`"
              class="item"
              role="option"
              :aria-selected="it.value === modelValue"
              :class="{ sel: it.value === modelValue, hl: flat[hlIdx] === it }"
              @click="pick(it)"
              @mouseenter="hlIdx = idxOfItem(it)"
            >
              <span class="chip" :style="{ background: it.color || '#7f8c8d' }">
                <i class="fas" :class="'fa-' + (it.icon || 'puzzle-piece')"></i>
              </span>
              <span class="ilabel">{{ it.label }}</span>
              <i v-if="it.value === modelValue" class="fas fa-check selcheck"></i>
            </div>
          </template>
          <div v-if="!flat.length" class="empty">
            No actions match “{{ query.trim() }}”
          </div>
        </div>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.ap { position: relative; }
.trigger {
  margin-top: 5px;
  width: 100%;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 12px;
  background: #ffffff;
  border: 1px solid #d6d6d6;
  border-radius: 3px;
  font-size: 14px;
  text-align: left;
  transition: border-color 120ms ease-out;
}
.trigger:hover { border-color: #bdbdbd; }
.tval {
  flex: 1;
  min-width: 0;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.caret {
  flex: none;
  font-size: 12px;
  color: var(--modal-muted);
  transition: transform 140ms cubic-bezier(0.2, 0, 0, 1);
}
.caret.up { transform: rotate(180deg); }

.chip {
  width: 22px;
  height: 22px;
  border-radius: 5px;
  display: grid;
  place-items: center;
  flex: none;
  color: #ffffff;
  font-size: 11px;
}

.pop {
  position: absolute;
  left: 0;
  right: 0;
  top: calc(100% + 5px);
  z-index: 31;
  background: #ffffff;
  border-radius: 6px;
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.24), 0 2px 8px rgba(0, 0, 0, 0.12);
  padding: 5px;
}
.search-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 7px 8px;
  border-bottom: 1px solid var(--modal-line);
  margin-bottom: 4px;
  transition: border-color 120ms ease-out;
}
.search-row:focus-within { border-bottom-color: var(--accent); }
.search-row i { font-size: 12px; color: var(--modal-muted); flex: none; }
.search-row input {
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  padding: 4px 0;
  font-size: 13.5px;
}
.search-row input:focus,
.search-row input:focus-visible {
  outline: none;
  border: none;
  box-shadow: none;
}

.list {
  max-height: 272px;
  overflow-y: auto;
  padding-bottom: 2px;
}
.ghead {
  font-size: 11px;
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.8px;
  color: var(--modal-muted);
  padding: 9px 8px 4px;
}
.item {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 8px;
  border-radius: 4px;
  font-size: 13.5px;
  cursor: pointer;
  user-select: none;
  transition: background 100ms ease-out;
}
.item.hl { background: var(--modal-field); }
.item.sel { background: #e6f7f2; font-weight: 500; }
.item.hl.sel { background: #dcf3ec; }
.ilabel { flex: 1; min-width: 0; }
.selcheck { flex: none; font-size: 12px; color: #0f7e69; }
.empty {
  padding: 16px 10px;
  font-size: 13px;
  color: var(--modal-muted);
  text-align: center;
}

.pop-enter-active { transition: opacity 130ms ease-out, transform 130ms cubic-bezier(0.2, 0, 0, 1); }
.pop-leave-active { transition: opacity 90ms ease-out; }
.pop-enter-from { opacity: 0; transform: translateY(-4px); }
.pop-leave-to { opacity: 0; }
</style>
