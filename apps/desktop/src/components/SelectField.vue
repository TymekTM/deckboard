<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";

// select field renderer for catalog + extension-declared fields
// ("input:select"). Hybrid: short sets render as inline chips (all choices
// visible, one click to change), anything longer or with long labels falls
// back to a trigger + popover listbox styled after the modal theme.
const props = defineProps({
  modelValue: { type: [String, Number], default: "" },
  options: { type: Array, default: () => [] }, // [{value, label}]
  label: { type: String, default: "" },
});
const emit = defineEmits(["update:modelValue"]);

const CHIP_MAX = 6;
const CHIP_LABEL_MAX = 16;
const asChips = computed(
  () =>
    props.options.length >= 2 &&
    props.options.length <= CHIP_MAX &&
    props.options.every((o) => String(o.label).length <= CHIP_LABEL_MAX)
);

const currentLabel = computed(() => {
  const cur = props.options.find((o) => o.value === props.modelValue);
  return cur ? cur.label : props.modelValue === "" ? "" : String(props.modelValue);
});

// ---- popover branch --------------------------------------------------------

const root = ref(null);
const trigger = ref(null);
const open = ref(false);
const optionEls = ref([]);

function select(value) {
  emit("update:modelValue", value);
}
function openPop() {
  if (!props.options.length) return;
  open.value = true;
  // focus lands on the selected option (or the first one)
  nextTick(() => {
    const idx = props.options.findIndex((o) => o.value === props.modelValue);
    optionEls.value[idx >= 0 ? idx : 0]?.focus();
  });
}
function closePop(refocus = true) {
  if (!open.value) return;
  open.value = false;
  if (refocus) trigger.value?.focus();
}
function onTriggerKeydown(e) {
  if (open.value && e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    closePop();
    return;
  }
  if (!open.value && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
    e.preventDefault();
    openPop();
  }
}
function onOptionKeydown(e, idx) {
  const last = props.options.length - 1;
  if (e.key === "ArrowDown") {
    e.preventDefault();
    optionEls.value[idx < last ? idx + 1 : 0]?.focus();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    optionEls.value[idx > 0 ? idx - 1 : last]?.focus();
  } else if (e.key === "Home") {
    e.preventDefault();
    optionEls.value[0]?.focus();
  } else if (e.key === "End") {
    e.preventDefault();
    optionEls.value[last]?.focus();
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    closePop();
  } else if (e.key === "Tab") {
    open.value = false;
  }
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

// ---- chips branch ----------------------------------------------------------

const chipEls = ref([]);
function onChipKeydown(e, idx) {
  const last = props.options.length - 1;
  let next = -1;
  if (e.key === "ArrowRight" || e.key === "ArrowDown") next = idx < last ? idx + 1 : 0;
  else if (e.key === "ArrowLeft" || e.key === "ArrowUp") next = idx > 0 ? idx - 1 : last;
  if (next < 0) return;
  e.preventDefault();
  chipEls.value[next]?.focus();
  select(props.options[next].value);
}
</script>

<template>
  <div v-if="asChips" class="chips" role="radiogroup" :aria-label="label">
    <button
      v-for="(o, i) in options"
      :key="o.value"
      ref="chipEls"
      class="chip"
      role="radio"
      :aria-checked="o.value === modelValue"
      :class="{ on: o.value === modelValue }"
      @click="select(o.value)"
      @keydown="onChipKeydown($event, i)"
    >
      <i v-if="o.value === modelValue" class="fas fa-check"></i>{{ o.label }}
    </button>
  </div>

  <div v-else ref="root" class="sel">
    <button
      ref="trigger"
      class="trigger"
      :disabled="!options.length"
      aria-haspopup="listbox"
      :aria-expanded="open"
      @click="open ? closePop() : openPop()"
      @keydown="onTriggerKeydown"
    >
      <span class="tval">{{ currentLabel }}</span>
      <i class="fas fa-chevron-down caret" :class="{ up: open }"></i>
    </button>
    <Transition name="pop">
      <div v-if="open" class="pop" role="listbox" :aria-label="label">
        <button
          v-for="(o, i) in options"
          :key="o.value"
          ref="optionEls"
          class="opt"
          role="option"
          :aria-selected="o.value === modelValue"
          :class="{ sel: o.value === modelValue }"
          @click="select(o.value); closePop()"
          @keydown="onOptionKeydown($event, i)"
        >
          <span class="olabel">{{ o.label }}</span>
          <i v-if="o.value === modelValue" class="fas fa-check ocheck"></i>
        </button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
/* chips: the whole choice space on the surface */
.chips {
  margin-top: 5px;
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
.chip {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 7px 13px;
  border-radius: 8px;
  background: var(--modal-field);
  font-size: 13px;
  line-height: 1.25;
  transition: background 110ms ease-out, color 110ms ease-out;
}
.chip:hover { background: #e6e6e6; }
.chip:focus-visible { outline-offset: 2px; }
/* darker teal than --accent so white labels keep AA contrast */
.chip.on { background: #0f7e69; color: #fff; font-weight: 500; }

/* popover: one-line trigger + modal-styled listbox */
.sel { position: relative; }
.trigger {
  margin-top: 5px;
  width: 100%;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 12px;
  background: #ffffff;
  border: 1px solid #d6d6d6;
  border-radius: 3px;
  font-size: 14px;
  text-align: left;
  transition: border-color 120ms ease-out;
}
.trigger:hover:not(:disabled) { border-color: #bdbdbd; }
.trigger:disabled { opacity: 0.45; cursor: default; }
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

.pop {
  position: absolute;
  left: 0;
  right: 0;
  top: calc(100% + 5px);
  z-index: 30;
  background: #ffffff;
  border-radius: 4px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.28);
  padding: 5px;
  max-height: 262px;
  overflow-y: auto;
}
.opt {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border-radius: 4px;
  font-size: 13.5px;
  line-height: 1.35;
  text-align: left;
  transition: background 100ms ease-out;
}
.opt:hover { background: var(--modal-field); }
.olabel { flex: 1; min-width: 0; }
.ocheck { flex: none; font-size: 12px; color: #0f7e69; }
.opt.sel { background: #e6f7f2; font-weight: 500; }

.pop-enter-active { transition: opacity 130ms ease-out, transform 130ms cubic-bezier(0.2, 0, 0, 1); }
.pop-leave-active { transition: opacity 90ms ease-out; }
.pop-enter-from { opacity: 0; transform: translateY(-4px); }
.pop-leave-to { opacity: 0; }
</style>
