<script setup>
import { computed, onUnmounted, ref, watch } from "vue";
import { api } from "../../api";

// OBS Studio settings section (Ustawienia): host/port/password + the
// enabled switch, a live connection status line and "Testuj
// połączenie". Self-contained: the only state it reads from the parent
// is whether the wall currently focuses this tile. Everything the user
// types stays in the form until "Zapisz" - the password is sent to the
// backend once and never comes back (the status only says whether one
// is stored).
const props = defineProps({
  expanded: { type: Boolean, default: false },
});
const emit = defineEmits(["focus"]);

const status = ref({
  enabled: false,
  connected: false,
  authFailed: false,
  version: null,
  host: "127.0.0.1",
  port: 4455,
  hasPassword: false,
});

const enabledDraft = ref(false);
const hostDraft = ref("127.0.0.1");
const portDraft = ref("4455");
const passwordDraft = ref("");
const clearPassword = ref(false);

const busy = ref(false);
const error = ref("");
const note = ref("");
const testNote = ref("");
const testNoteBad = ref(false);

function flashNote(text) {
  note.value = text;
  setTimeout(() => {
    if (note.value === text) note.value = "";
  }, 4000);
}

async function refreshStatus() {
  try {
    const s = await api.obsStatus();
    const first = !status.value.loaded;
    status.value = { ...s, loaded: true };
    if (first) {
      enabledDraft.value = Boolean(s.enabled);
      hostDraft.value = s.host || "127.0.0.1";
      portDraft.value = String(s.port ?? 4455);
    }
  } catch (e) {
    error.value = e ? String(e) : "Nie udało się pobrać statusu OBS.";
  }
}

// live status while the section is open: the worker flips connected /
// auth-failed on its own, so the chip follows without reopening
let poll = null;
watch(
  () => props.expanded,
  (expanded) => {
    if (expanded) {
      refreshStatus();
      poll = setInterval(refreshStatus, 3000);
    } else if (poll) {
      clearInterval(poll);
      poll = null;
    }
  },
  { immediate: true },
);
onUnmounted(() => {
  if (poll) clearInterval(poll);
});

const chip = computed(() => {
  if (!status.value.enabled) return { text: "wył.", ok: false };
  if (status.value.connected) return { text: status.value.version || "połączony", ok: true };
  if (status.value.authFailed) return { text: "błąd logowania", ok: false };
  return { text: "brak połączenia", ok: false };
});

const statusLine = computed(() => {
  if (!status.value.enabled) {
    return "Integracja wyłączona — kafelki OBS będą ignorowane (jedno ostrzeżenie na rodzaj kafelka).";
  }
  if (status.value.connected) {
    return `Połączono z OBS${status.value.version ? ` (${status.value.version})` : ""} — stan kafelków podąża za programem na żywo.`;
  }
  if (status.value.authFailed) {
    return "OBS odrzucił hasło — sprawdź hasło z ustawień obs-websocket (Narzędzia → obs-websocket).";
  }
  return `Brak połączenia z ${status.value.host}:${status.value.port} — uruchom OBS z wtyczką obs-websocket (OBS 28+ ma ją wbudowaną).`;
});

async function save() {
  busy.value = true;
  error.value = "";
  try {
    const port = Number(portDraft.value.trim());
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new Error("Port musi być liczbą z zakresu 1–65535.");
    }
    const password = clearPassword.value ? "" : passwordDraft.value || null;
    await api.obsApplyConfig(enabledDraft.value, hostDraft.value.trim(), port, password);
    clearPassword.value = false;
    passwordDraft.value = "";
    flashNote("Zapisano — połączenie odświeża się samo, bez restartu aplikacji.");
    await refreshStatus();
  } catch (e) {
    error.value = e ? String(e) : "Nie udało się zapisać ustawień OBS.";
  } finally {
    busy.value = false;
  }
}

async function testConnection() {
  busy.value = true;
  error.value = "";
  testNote.value = "Łączę…";
  testNoteBad.value = false;
  try {
    const port = Number(portDraft.value.trim());
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new Error("Port musi być liczbą z zakresu 1–65535.");
    }
    const password = clearPassword.value ? "" : passwordDraft.value || null;
    const version = await api.obsTestConnection(hostDraft.value.trim(), port, password);
    testNote.value = `Połączono: ${version}.`;
  } catch (e) {
    testNote.value = e ? String(e) : "Połączenie nie powiodło się.";
    testNoteBad.value = true;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <article
    class="tile"
    :class="{ focused: expanded }"
    @click="!expanded && $emit('focus')"
  >
    <button
      v-if="!expanded"
      class="hit"
      aria-label="Rozwiń: OBS Studio"
      @click.stop="$emit('focus')"
    ></button>
    <header class="tile-head">
      <h3>OBS Studio</h3>
      <span class="chip" :class="chip.ok ? 'ok' : ''">{{ chip.text }}</span>
    </header>
    <p class="sum">
      Natywna integracja przez obs-websocket v5 (OBS 28+): sceny, źródła,
      audio, nagrywanie i stream na żywo. Hasło zostaje na tym komputerze.
    </p>
    <div v-show="expanded" class="detail">
      <div class="ctl">
        <div class="ctl-text">
          <span class="ctl-name">Integracja włączona</span>
          <span class="ctl-note">Gdy wyłączona, Pulpit nie łączy się z OBS-em wcale.</span>
        </div>
        <button
          class="switch"
          role="switch"
          :aria-checked="enabledDraft"
          :disabled="busy"
          @click="enabledDraft = !enabledDraft"
        ><span class="knob"></span></button>
      </div>
      <div class="ctl">
        <div class="ctl-text">
          <span class="ctl-name">Adres i port</span>
          <span class="ctl-note">Domyślnie ws://127.0.0.1:4455 (obs-websocket v5).</span>
        </div>
        <span class="addr-line">
          <input
            v-model="hostDraft"
            class="combo-input host-input"
            placeholder="127.0.0.1"
            autocomplete="off"
            spellcheck="false"
            aria-label="Adres hosta OBS"
          />
          <input
            v-model="portDraft"
            class="combo-input port-input"
            inputmode="numeric"
            placeholder="4455"
            autocomplete="off"
            aria-label="Port obs-websocket"
          />
        </span>
      </div>
      <div class="ctl">
        <div class="ctl-text">
          <span class="ctl-name">Hasło</span>
          <span class="ctl-note">
            {{ status.hasPassword ? "Hasło zapisane — wpisz nowe, aby zmienić." : "Z obs-serverów: Narzędzia → obs-websocket → Hasło." }}
          </span>
        </div>
        <span class="addr-line">
          <input
            v-model="passwordDraft"
            type="password"
            class="combo-input host-input"
            :placeholder="status.hasPassword ? '••••••••' : 'hasło (opcjonalne)'"
            autocomplete="new-password"
            aria-label="Hasło obs-websocket"
          />
          <button
            v-if="status.hasPassword"
            class="act danger"
            :disabled="busy"
            @click="clearPassword = !clearPassword"
          >
            {{ clearPassword ? "Usuwanie hasła…" : "Usuń hasło" }}
          </button>
        </span>
      </div>
      <p class="note">{{ statusLine }}</p>
      <div class="row-btns">
        <button class="act accent" :disabled="busy" @click="save">Zapisz</button>
        <button class="act" :disabled="busy" @click="testConnection">Testuj połączenie</button>
      </div>
      <p v-if="testNote" :class="testNoteBad ? 'err' : 'note pending'">{{ testNote }}</p>
      <p v-if="error" class="err">{{ error }}</p>
      <p v-else-if="note" class="note pending">{{ note }}</p>
    </div>
  </article>
</template>

<style scoped>
/* Self-contained styling: the wall's rules are scoped to
   SettingsOverlay.vue and cannot reach this component's innards, so the
   section markup (ctl rows, switch, buttons, notes) is restyled here to
   match the existing tiles. Root-node .tile/.focused still come from
   the parent's scoped sheet. */
.hit {
  position: absolute;
  inset: 0;
  border-radius: inherit;
}

.tile-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 6px;
}
.tile-head h3 {
  font-size: 15px;
  font-weight: 600;
  flex: 1;
  min-width: 0;
}
.chip {
  flex: none;
  font-size: 11px;
  padding: 3px 9px;
  border-radius: 999px;
  background: #313a44;
  color: #95a5a6;
  white-space: nowrap;
}
.chip.ok { color: var(--accent); background: rgba(26, 188, 156, 0.14); }
.sum {
  font-size: 12.5px;
  line-height: 1.55;
  color: #95a5a6;
  margin: 0;
}
.detail { padding-top: 12px; text-align: left; animation: rise 200ms ease-out; }
@keyframes rise {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: none; }
}

.ctl {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 11px 0;
}
.ctl + .ctl { border-top: 1px solid #313a44; }
.ctl-text { min-width: 0; }
.ctl-name { display: block; font-size: 13.5px; font-weight: 500; }
.ctl-note { display: block; font-size: 12px; color: #8b97a1; margin-top: 2px; line-height: 1.5; }

.addr-line { display: flex; align-items: center; gap: 8px; flex: none; }
.host-input { width: 150px; }
.port-input { width: 90px; }
.combo-input {
  padding: 7px 10px;
  font-size: 13px;
  background: #171c21;
  border: 1px solid #1d2a36;
  border-radius: 6px;
  color: #ecf0f1;
}
.combo-input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}

.switch {
  flex: none;
  position: relative;
  width: 46px;
  height: 26px;
  border-radius: 13px;
  background: #313a44;
  transition: background 160ms ease-out;
}
.switch:hover { background: #3a4753; }
.switch[aria-checked="true"] { background: var(--accent); }
.switch[aria-checked="true"] .knob { transform: translateX(20px); }
.switch .knob {
  position: absolute;
  top: 3px;
  left: 3px;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: #f2f4f6;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1);
}

.act {
  flex: none;
  font-size: 12.5px;
  padding: 7px 12px;
  border-radius: 6px;
  background: #313a44;
  color: #ecf0f1;
  transition: background 140ms ease-out;
}
.act:hover { background: #3a4753; }
.act:disabled { opacity: 0.45; }
.act.accent { color: var(--accent); font-weight: 500; }
.act.danger { color: #f19488; }
.act.danger:hover { background: rgba(231, 76, 60, 0.18); }

.note { font-size: 12px; line-height: 1.6; color: #8b97a1; margin: 10px 0 0; }
.note.pending { color: var(--accent); }
.err { font-size: 12px; color: #f19488; margin: 8px 0 0; overflow-wrap: anywhere; }
.row-btns {
  display: flex;
  gap: 8px;
  margin-top: 12px;
}
</style>
