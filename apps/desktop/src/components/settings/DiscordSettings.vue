<script setup>
// Discord settings tile (Integracje). Self-contained: markup, state and
// styles live here, SettingsOverlay only mounts the component and routes
// the wall's one-expanded-tile focus through the focused/expand pair.
// Secrets never come back from the backend - the panel shows only a
// "configured" flag, so an empty secret input means "keep the saved one".
import { computed, onMounted, ref, watch } from "vue";
import { api } from "../../api";

const props = defineProps({
  focused: { type: Boolean, default: false },
});
const emit = defineEmits(["expand"]);

const REDIRECT_URI = "https://discord.com";
const DEVELOPERS_URL = "https://discord.com/developers/applications";

const status = ref({
  configured: false,
  clientId: "",
  hasSecret: false,
  statusType: "not_configured",
  statusLine: "nie skonfigurowano",
  username: null,
});

const clientIdDraft = ref("");
const clientSecretDraft = ref("");
const showSecret = ref(false);
const busy = ref(false);
const error = ref("");
const note = ref("");
const helpOpen = ref(false);

async function refresh() {
  try {
    const res = await api.discordStatus();
    status.value = res;
    if (res.clientId) {
      clientIdDraft.value = res.clientId;
    }
  } catch (e) {
    error.value = e ? String(e) : "Nie udało się pobrać statusu Discorda.";
  }
}

const statusDisplay = computed(() => {
  if (busy.value && status.value.statusType === "authorizing") {
    return "czekam na zatwierdzenie";
  }
  return status.value.statusLine || "nie skonfigurowano";
});

const isConnected = computed(() => status.value.statusType === "connected");

const chip = computed(() => {
  if (isConnected.value) {
    return { text: status.value.username || "połączono", ok: true };
  }
  if (status.value.statusType === "needs_auth") {
    return { text: "wymaga autoryzacji", ok: false };
  }
  if (status.value.statusType === "not_running") {
    return { text: "nie działa", ok: false };
  }
  return { text: "nie skonfigurowano", ok: false };
});

function flashNote(text) {
  note.value = text;
  setTimeout(() => {
    if (note.value === text) note.value = "";
  }, 3500);
}

async function saveCredentials() {
  if (!clientIdDraft.value.trim()) {
    error.value = "Wprowadź Client ID.";
    return;
  }
  busy.value = true;
  error.value = "";
  try {
    await api.discordSaveConfig(clientIdDraft.value.trim(), clientSecretDraft.value.trim());
    clientSecretDraft.value = "";
    flashNote("Zapisano dane aplikacji Discord.");
    await refresh();
  } catch (e) {
    error.value = e ? String(e) : "Nie udało się zapisać konfiguracji Discorda.";
  } finally {
    busy.value = false;
  }
}

async function connectDiscord() {
  busy.value = true;
  error.value = "";
  status.value.statusType = "authorizing";
  status.value.statusLine = "czekam na zatwierdzenie";
  try {
    const res = await api.discordAuthorize();
    status.value.statusType = res.statusType || "connected";
    status.value.username = res.username;
    status.value.statusLine = res.username ? `połączono jako ${res.username}` : "połączono";
    flashNote("Pomyślnie połączono z aplikacją Discord!");
  } catch (e) {
    error.value = e ? String(e) : "Autoryzacja w aplikacji Discord nie powiodła się.";
    await refresh();
  } finally {
    busy.value = false;
  }
}

async function disconnectDiscord() {
  busy.value = true;
  error.value = "";
  try {
    await api.discordDisconnect();
    flashNote("Rozłączono sesję Discorda (dane aplikacji zachowane).");
    await refresh();
  } catch (e) {
    error.value = e ? String(e) : "Nie udało się rozłączyć Discorda.";
  } finally {
    busy.value = false;
  }
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    flashNote(`Skopiowano: ${text}`);
  } catch {
    error.value = "Nie udało się skopiować — skopiuj ręcznie.";
  }
}

onMounted(refresh);
// the probe dials Discord's pipe: only pay for it while the tile is open
watch(
  () => props.focused,
  (open) => {
    if (open) refresh();
  }
);
</script>

<template>
  <article class="tile" :class="{ focused, dim: !focused }">
    <button
      v-if="!focused"
      class="hit"
      tabindex="0"
      aria-label="Rozwiń: Discord"
      @click="emit('expand')"
    ></button>
    <header class="tile-head">
      <h3>Discord</h3>
      <span class="chip" :class="chip.ok ? 'ok' : ''">{{ chip.text }}</span>
    </header>
    <p class="sum">
      Sterowanie mikrofonem, odsłuchem i kanałami głosowymi przez lokalnego klienta Discord.
    </p>
    <div v-show="focused" class="detail">
      <label class="ctl">
        <span class="ctl-head">
          <span class="ctl-title">Client ID</span>
          <span class="ctl-note">ID aplikacji z discord.com/developers.</span>
        </span>
        <span class="input-line">
          <input
            v-model="clientIdDraft"
            class="combo-input"
            placeholder="np. 123456789012345678"
            autocomplete="off"
            spellcheck="false"
            aria-label="Discord Client ID"
          />
        </span>
      </label>

      <label class="ctl">
        <span class="ctl-head">
          <span class="ctl-title">Client Secret</span>
          <span class="ctl-note" v-if="status.hasSecret && !clientSecretDraft">
            Zapisany — wpisz nowy, aby zmienić.
          </span>
          <span class="ctl-note" v-else>Klucz tajny aplikacji OAuth2.</span>
        </span>
        <span class="input-line">
          <input
            :type="showSecret ? 'text' : 'password'"
            v-model="clientSecretDraft"
            class="combo-input"
            placeholder="Client Secret"
            autocomplete="off"
            spellcheck="false"
            aria-label="Discord Client Secret"
          />
          <button class="act" type="button" @click="showSecret = !showSecret">
            {{ showSecret ? "Ukryj" : "Pokaż" }}
          </button>
          <button class="act accent" type="button" :disabled="busy" @click="saveCredentials">
            Zapisz
          </button>
        </span>
      </label>

      <div class="ctl">
        <span class="ctl-head">
          <span class="ctl-title">Status połączenia</span>
          <span class="ctl-note mono">{{ statusDisplay }}</span>
        </span>
        <div class="conn-actions">
          <button v-if="isConnected" class="act danger" :disabled="busy" @click="disconnectDiscord">
            Rozłącz
          </button>
          <button
            v-else
            class="act primary"
            :disabled="busy || !clientIdDraft"
            @click="connectDiscord"
          >
            {{ busy && status.statusType === 'authorizing' ? "Czekam..." : "Połącz z Discordem" }}
          </button>
        </div>
      </div>

      <p v-if="error" class="err">{{ error }}</p>
      <p v-else-if="note" class="note pending">{{ note }}</p>

      <button class="act help-toggle" type="button" @click="helpOpen = !helpOpen">
        <i
          class="fas"
          :class="helpOpen ? 'fa-chevron-down' : 'fa-chevron-right'"
          aria-hidden="true"
        ></i>
        {{ helpOpen ? "Ukryj instrukcję konfiguracji" : "Jak utworzyć aplikację Discord?" }}
      </button>

      <ol v-if="helpOpen" class="help-steps">
        <li>
          Przejdź do portalu deweloperskiego Discord:
          <span class="mono">{{ DEVELOPERS_URL }}</span>
          <button class="act mini" type="button" @click="copyText(DEVELOPERS_URL)">
            Kopiuj URL
          </button>
        </li>
        <li>
          Utwórz nową aplikację (przycisk <b>New Application</b>) i nadaj jej dowolną nazwę
          (np. <i>Pulpit</i>).
        </li>
        <li>
          W zakładce <b>OAuth2</b> dodaj adres przekierowania (Redirects):
          <span class="mono">{{ REDIRECT_URI }}</span>
          <button class="act mini" type="button" @click="copyText(REDIRECT_URI)">
            Kopiuj Redirect
          </button>
        </li>
        <li>
          W zakładce <b>OAuth2</b> skopiuj <b>Client ID</b> oraz wygeneruj/skopiuj
          <b>Client Secret</b> i wklej powyżej.
        </li>
        <li>
          Uruchom klienta Discord na komputerze, kliknij <b>Zapisz</b>, a następnie
          <b>Połącz z Discordem</b> i zatwierdź w oknie aplikacji Discord.
        </li>
      </ol>
    </div>
  </article>
</template>

<style scoped>
.tile {
  position: relative;
  flex: 1 1 300px;
  min-width: 240px;
  background: var(--tile);
  border: 1px solid var(--line);
  border-radius: 12px;
  padding: 18px 20px;
  transition:
    flex-grow 240ms cubic-bezier(0.2, 0, 0, 1),
    opacity 170ms ease-out,
    border-color 170ms ease-out,
    box-shadow 200ms ease-out;
}
.tile.focused {
  flex-grow: 2.4;
  border-color: color-mix(in srgb, var(--accent) 40%, var(--line));
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.5);
}
.tile.dim { opacity: 0.42; }
.tile.dim:hover { opacity: 0.62; border-color: #43505c; }

/* invisible hit zone over an unexpanded tile; steps aside once focused */
.hit {
  position: absolute;
  inset: 0;
  border-radius: inherit;
}
.tile.focused .hit { display: none; }

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
  background: var(--tile-2);
  color: var(--ink-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 60%;
}
.chip.ok { color: var(--ok); background: rgba(26, 188, 156, 0.14); }

.sum {
  font-size: 12.5px;
  line-height: 1.55;
  color: var(--ink-2);
  margin: 0;
}

.detail { padding-top: 12px; animation: rise 200ms ease-out; }
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
.ctl + .ctl { border-top: 1px solid var(--tile-2); }
.ctl-head { min-width: 0; }
.ctl-title { display: block; font-size: 13.5px; font-weight: 500; }
.ctl-note { display: block; font-size: 12px; color: var(--ink-3); margin-top: 2px; line-height: 1.5; }

.input-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
}
.combo-input {
  width: 240px;
  padding: 7px 10px;
  font-size: 13px;
  background: #171c21;
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--ink);
}
.combo-input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}

.conn-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.act {
  flex: none;
  font-size: 12.5px;
  padding: 7px 12px;
  border-radius: 6px;
  background: var(--tile-2);
  color: var(--ink);
  transition: background 140ms ease-out;
}
.act:hover { background: #3a4753; }
.act:disabled { opacity: 0.45; }
.act.accent { color: var(--accent); font-weight: 500; }
.act.primary {
  background: var(--accent);
  color: #0c2a24;
  font-weight: 600;
  padding: 9px 14px;
}
.act.primary:hover { background: var(--accent-2); }
.act.danger { color: var(--bad-ink); }
.act.danger:hover { background: rgba(231, 76, 60, 0.18); }
.help-toggle { margin-top: 12px; }
.mini {
  margin-left: 6px;
  padding: 1px 6px;
  font-size: 11px;
}

.err { font-size: 12px; color: var(--bad-ink); margin: 8px 0 0; overflow-wrap: anywhere; }
.note { font-size: 12px; line-height: 1.6; color: var(--ink-3); margin: 10px 0 0; }
.note.pending { color: var(--ok); }

.help-steps {
  margin: 10px 0 0;
  padding-left: 20px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--ink-2);
}
.help-steps li { margin-bottom: 6px; }
.help-steps b { color: var(--ink); font-weight: 600; }

.mono {
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 11.5px;
  background: var(--tile-2);
  padding: 1px 5px;
  border-radius: 4px;
  overflow-wrap: anywhere;
}
</style>
