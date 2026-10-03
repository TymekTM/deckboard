<script setup>
import { onMounted, onUnmounted, ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";

// The operator gate as an in-app popup (native-dialog replacement): the
// backend emits `operator-ask` whenever a tablet wants to pair, and this
// modal answers it through `resolve_operator_ask`. Until answered, the
// asking worker stays parked and the pairing simply waits.
const ask = ref(null); // { id, kind, name, code? }
const busy = ref(false);
let unlisten = null;

onMounted(async () => {
  unlisten = await listen("operator-ask", (e) => {
    // one ask at a time (the backend enforces it for pair-requests; a
    // manual pairing ask while one is up replaces it - the stale worker
    // times out and denies on its own)
    ask.value = e.payload;
  });
});
onUnmounted(() => unlisten?.());

async function answer(approved) {
  const current = ask.value;
  if (!current || busy.value) return;
  busy.value = true;
  try {
    await api.resolveOperatorAsk(current.id, approved);
  } catch (e) {
    // already resolved or expired server-side: just close the popup
    console.error("resolve operator ask", e);
  } finally {
    busy.value = false;
    ask.value = null;
  }
}
</script>

<template>
  <div
    v-if="ask"
    class="veil"
    role="alertdialog"
    aria-modal="true"
    aria-labelledby="ask-title"
  >
    <div class="ask">
      <div class="ask-icon">
        <i class="fas fa-mobile-screen-button" aria-hidden="true"></i>
      </div>
      <h2 id="ask-title">
        {{ ask.kind === "pair-request" ? "Żądanie parowania" : "Zaufać urządzeniu?" }}
      </h2>
      <p class="ask-name">{{ ask.name }}</p>

      <template v-if="ask.kind === 'pair-request' && ask.code">
        <p class="ask-label">Kod weryfikacyjny (ten sam musi być na tablecie):</p>
        <div class="ask-code">{{ ask.code }}</div>
      </template>

      <p v-if="ask.kind === 'pair-request'" class="ask-warn">
        Zaufaj tylko wtedy, gdy kod powyżej zgadza się z kodem na tablecie.
        Zaufanie wydaje urządzeniu token; odrzucenie - lub brak odpowiedzi do
        wygaśnięcia żądania - odrzuca parowanie.
      </p>
      <p v-else class="ask-warn">
        Urządzenie używa jednorazowego kodu parowania. Zaufanie wydaje token;
        brak odpowiedzi do wygaśnięcia kodu odrzuca parowanie.
      </p>

      <div class="ask-actions">
        <button class="act" :disabled="busy" @click="answer(false)">Odrzuć</button>
        <button class="act trust" :disabled="busy" @click="answer(true)">
          <i class="fas fa-shield-halved" aria-hidden="true"></i> Zaufaj
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.veil {
  position: fixed;
  inset: 0;
  z-index: 90;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(10, 14, 17, 0.66);
  backdrop-filter: blur(2px);
  animation: ask-fade 140ms ease-out;
}
@keyframes ask-fade {
  from { opacity: 0; }
  to { opacity: 1; }
}
.ask {
  width: min(400px, calc(100vw - 80px));
  background: #262e36;
  border: 1px solid rgba(26, 188, 156, 0.35);
  border-radius: 14px;
  padding: 26px 26px 22px;
  text-align: center;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.55);
  animation: ask-rise 200ms ease-out;
}
@keyframes ask-rise {
  from { opacity: 0; transform: translateY(6px); }
  to { opacity: 1; transform: none; }
}
.ask-icon {
  width: 52px;
  height: 52px;
  margin: 0 auto 12px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 20px;
  color: #1abc9c;
  background: rgba(26, 188, 156, 0.16);
}
.ask h2 {
  font-size: 17px;
  font-weight: 600;
  color: #ecf0f1;
  margin: 0 0 4px;
}
.ask-name {
  font-size: 14.5px;
  font-weight: 600;
  color: #ecf0f1;
  margin: 0;
  overflow-wrap: anywhere;
}
.ask-label {
  font-size: 12px;
  color: #8b97a1;
  margin: 12px 0 6px;
}
.ask-code {
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 30px;
  font-weight: 700;
  letter-spacing: 6px;
  line-height: 1.1;
  color: #ecf0f1;
  background: #171c21;
  border: 1px solid #1d2a36;
  border-radius: 8px;
  padding: 12px 14px;
  user-select: all;
}
.ask-warn {
  font-size: 12.5px;
  line-height: 1.6;
  color: #95a5a6;
  margin: 12px 0 0;
}
.ask-actions {
  display: flex;
  justify-content: center;
  gap: 10px;
  margin-top: 20px;
}
.act {
  font-size: 12.5px;
  padding: 9px 14px;
  border-radius: 6px;
  background: #313a44;
  color: #ecf0f1;
  transition: background 140ms ease-out;
}
.act:hover { background: #3a4753; }
.act:disabled { opacity: 0.45; }
.act.trust {
  background: var(--accent, #1abc9c);
  color: #0c2a24;
  font-weight: 600;
}
.act.trust:hover { background: var(--accent-2, #16a085); }
</style>
