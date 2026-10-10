<script setup>
import { nextTick, onUnmounted, ref, watch } from "vue";
import { answerConfirm, confirmState } from "../confirm";

// The single in-app confirmation popup behind confirm.js `ask()`. Cancel
// holds the initial focus, so a stray Enter/Space never confirms a
// destructive action; Escape and a click on the backdrop cancel.
const cancelBtn = ref(null);
const okBtn = ref(null);
let returnFocus = null;

function onKeydown(e) {
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopImmediatePropagation();
    answerConfirm(false);
    return;
  }
  // two buttons: keep Tab inside the dialog
  if (e.key === "Tab") {
    e.preventDefault();
    e.stopImmediatePropagation();
    const next = document.activeElement === cancelBtn.value ? okBtn.value : cancelBtn.value;
    next?.focus();
    return;
  }
  // nothing behind the popup may react while it is up (Delete/Backspace
  // would re-trigger the tile delete that opened it)
  e.stopImmediatePropagation();
}

watch(
  () => confirmState.open,
  async (open) => {
    if (open) {
      returnFocus = document.activeElement;
      window.addEventListener("keydown", onKeydown, true);
      await nextTick();
      cancelBtn.value?.focus();
    } else {
      window.removeEventListener("keydown", onKeydown, true);
      if (returnFocus?.isConnected) returnFocus.focus?.();
      returnFocus = null;
    }
  },
);
onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown, true);
  answerConfirm(false);
});
</script>

<template>
  <div
    v-if="confirmState.open"
    class="veil"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="confirm-title"
    aria-describedby="confirm-message"
    @mousedown.self="answerConfirm(false)"
  >
    <div class="confirm" :class="{ warn: confirmState.kind === 'warning' }">
      <div class="confirm-icon">
        <i
          class="fas"
          :class="confirmState.kind === 'warning' ? 'fa-exclamation-triangle' : 'fa-question'"
          aria-hidden="true"
        ></i>
      </div>
      <h2 id="confirm-title">{{ confirmState.title || "Potwierdź" }}</h2>
      <p id="confirm-message" class="confirm-message">{{ confirmState.message }}</p>
      <div class="confirm-actions">
        <button ref="cancelBtn" class="act" @click="answerConfirm(false)">
          {{ confirmState.cancelLabel }}
        </button>
        <button
          ref="okBtn"
          class="act"
          :class="confirmState.kind === 'warning' ? 'danger-solid' : 'primary'"
          @click="answerConfirm(true)"
        >
          {{ confirmState.okLabel }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.veil {
  position: fixed;
  inset: 0;
  /* above every editor modal and the settings overlay */
  z-index: 120;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(10, 14, 17, 0.66);
  backdrop-filter: blur(2px);
  animation: confirm-fade 140ms ease-out;
}
@keyframes confirm-fade {
  from { opacity: 0; }
  to { opacity: 1; }
}
.confirm {
  width: min(380px, calc(100vw - 80px));
  background: #262e36;
  border: 1px solid rgba(26, 188, 156, 0.35);
  border-radius: 14px;
  padding: 26px 26px 22px;
  text-align: center;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.55);
  animation: confirm-rise 200ms ease-out;
}
.confirm.warn { border-color: rgba(231, 76, 60, 0.4); }
@keyframes confirm-rise {
  from { opacity: 0; transform: translateY(6px); }
  to { opacity: 1; transform: none; }
}
.confirm-icon {
  width: 52px;
  height: 52px;
  margin: 0 auto 12px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
  color: var(--accent, #1abc9c);
  background: rgba(26, 188, 156, 0.16);
}
.confirm.warn .confirm-icon {
  color: #e74c3c;
  background: rgba(231, 76, 60, 0.16);
}
.confirm h2 {
  font-size: 17px;
  font-weight: 600;
  color: #ecf0f1;
  margin: 0 0 6px;
}
.confirm-message {
  font-size: 13.5px;
  line-height: 1.55;
  color: #b4bec6;
  margin: 0;
  overflow-wrap: anywhere;
}
.confirm-actions {
  display: flex;
  justify-content: center;
  gap: 10px;
  margin-top: 22px;
}
.act {
  font-size: 12.5px;
  padding: 9px 16px;
  border-radius: 6px;
  background: #313a44;
  color: #ecf0f1;
  transition: background 140ms ease-out;
}
.act:hover { background: #3a4753; }
.act:focus-visible { outline: 2px solid var(--accent, #1abc9c); outline-offset: 2px; }
.act.primary {
  background: var(--accent, #1abc9c);
  color: #0c2a24;
  font-weight: 600;
}
.act.primary:hover { background: var(--accent-2, #16a085); }
.act.danger-solid {
  background: #e74c3c;
  color: #fff;
  font-weight: 600;
}
.act.danger-solid:hover { background: #c0392b; }
.act.danger-solid:focus-visible { outline-color: #e74c3c; }
</style>
