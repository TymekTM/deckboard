<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";

const props = defineProps({
  status: { type: Object, required: true },
});
const emit = defineEmits(["close"]);

const rootEl = ref(null);
const closeBtn = ref(null);

// the wall keeps exactly one expanded tile; everything else dims
const focused = ref("launch");

const autostart = ref(false);
const autostartBusy = ref(false);
const autostartError = ref("");

const hotkey = ref("Ctrl+Alt+D");
const hotkeyDraft = ref("");
const hotkeyError = ref("");
const editingHotkey = ref(false);

const lanAddresses = ref([]);
const activeAddress = ref(0);
const lanLoading = ref(false);

const pairingOffer = ref(null); // {code, expires_in_secs, addresses}
const pairingBusy = ref(false);
const pairingLeft = ref(0);
const pairingError = ref("");
let pairingTimer = null;

// trusted v2 devices (the backend's devices.json, minus token material)
const devices = ref([]);
const devicesBusy = ref(false);

// server port editing; the value applies on the next launch because the
// sockets bind once at startup and tablets aim at host:port explicitly
const portDraft = ref("");
const portError = ref("");
const portPending = ref(null); // stored value that differs from the running one
const portLocked = ref(false); // the PULPIT_PORT env var overrides the store
const portBusy = ref(false);

// in-app revoke confirmation (replaces the native dialog popup)
const confirmRevoke = ref(null); // device awaiting confirmation
const revokeError = ref("");
const cancelBtn = ref(null);

// M8 sideload: adb device list + keep-data APK install
const adbList = ref([]);
const adbBusy = ref(false);
const adbNote = ref("");
const adbNoteBad = ref(false);

async function refreshAdb() {
  adbBusy.value = true;
  try {
    adbList.value = await api.adbDevices();
  } catch (e) {
    adbList.value = [];
    adbNote.value = String(e);
    adbNoteBad.value = true;
  } finally {
    adbBusy.value = false;
  }
}

async function installApk() {
  const path = await open({
    multiple: false,
    filters: [{ name: "APK", extensions: ["apk"] }],
  });
  if (!path) return;
  adbBusy.value = true;
  adbNote.value = "Instaluję (adb install -r, dane aplikacji zostają)...";
  adbNoteBad.value = false;
  try {
    adbNote.value = (await api.adbInstallApk(String(path))) || "Gotowe.";
    await refreshAdb();
  } catch (e) {
    adbNote.value = String(e);
    adbNoteBad.value = true;
  } finally {
    adbBusy.value = false;
  }
}

// M8 update check: reads the update_url feed (empty = off), no download
const updateBusy = ref(false);
const updateNote = ref("");
const updateNoteBad = ref(false);

async function checkUpdates() {
  updateBusy.value = true;
  updateNote.value = "Sprawdzam...";
  updateNoteBad.value = false;
  try {
    const r = await api.checkForUpdates();
    updateNote.value = r.update_available
      ? `Dostępna wersja ${r.latest} (masz ${r.current}). ${r.url || "Pobierz ze strony wydania."}`
      : `System jest aktualny (v${r.current}).`;
  } catch (e) {
    updateNote.value = String(e);
    updateNoteBad.value = true;
  } finally {
    updateBusy.value = false;
  }
}

const version = computed(() => props.status.version || "0.1.1");

function tileCls(id) {
  return { focused: focused.value === id, dim: focused.value !== id };
}

async function refreshLan() {
  lanLoading.value = true;
  try {
    lanAddresses.value = await api.listLanAddresses();
    if (activeAddress.value >= lanAddresses.value.length) activeAddress.value = 0;
  } catch {
    lanAddresses.value = [];
  } finally {
    lanLoading.value = false;
  }
}

function stopPairingClock() {
  if (pairingTimer) {
    clearInterval(pairingTimer);
    pairingTimer = null;
  }
}

async function generatePairing() {
  pairingBusy.value = true;
  pairingError.value = "";
  try {
    pairingOffer.value = await api.createPairingCode();
    pairingLeft.value = pairingOffer.value.expires_in_secs;
    stopPairingClock();
    pairingTimer = setInterval(() => {
      pairingLeft.value -= 1;
      if (pairingLeft.value <= 0) {
        // one-time code burned out: back to the minting state
        pairingOffer.value = null;
        stopPairingClock();
      }
    }, 1000);
  } catch (e) {
    pairingError.value = e
      ? String(e)
      : "Nie udało się wygenerować kodu parowania.";
  } finally {
    pairingBusy.value = false;
  }
}

const pairingClock = computed(() => {
  const s = Math.max(0, pairingLeft.value);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
});

async function refreshDevices() {
  try {
    devices.value = await api.listDevices();
  } catch {
    // v2 stack unavailable: show none rather than stale rows
    devices.value = [];
  }
}

// a pairing can complete while the overlay is open: refetch when the
// devices tile is (re)focused
watch(focused, (tile) => {
  if (tile === "devices") refreshDevices();
});

function revokeDevice(device) {
  revokeError.value = "";
  confirmRevoke.value = device;
}

async function doRevoke() {
  const device = confirmRevoke.value;
  if (!device) return;
  devicesBusy.value = true;
  revokeError.value = "";
  try {
    await api.revokeDevice(device.id);
  } catch (e) {
    // keep the popup open with the reason; the device list is stale anyway
    revokeError.value = e ? String(e) : "Nie udało się odwołać urządzenia.";
    return;
  } finally {
    devicesBusy.value = false;
  }
  confirmRevoke.value = null;
  refreshDevices();
}

// focus lands on Cancel so a reflexive Enter cannot fire the destructive
// action; Escape closes just the popup while it is up
watch(confirmRevoke, (device) => {
  if (device) nextTick(() => cancelBtn.value?.focus());
});

async function savePort() {
  const value = Number(portDraft.value.trim());
  if (!Number.isInteger(value) || value < 1024 || value > 65535) {
    portError.value = "Port musi być liczbą z zakresu 1024–65535.";
    return;
  }
  portBusy.value = true;
  try {
    await api.setServerPort(value);
    portError.value = "";
    portPending.value = value === (props.status.port || 8500) ? null : value;
  } catch (e) {
    // the backend message says why the port was refused
    portError.value = e ? String(e) : "Nie udało się zapisać portu.";
  } finally {
    portBusy.value = false;
  }
}

function lastSeenLabel(unixSecs) {
  if (!unixSecs) return "nigdy";
  const diff = Math.max(0, Date.now() / 1000 - unixSecs);
  if (diff < 90) return "teraz";
  if (diff < 3600) return `${Math.floor(diff / 60)} min temu`;
  if (diff < 48 * 3600) return `${Math.floor(diff / 3600)} godz. temu`;
  return new Date(unixSecs * 1000).toLocaleDateString();
}

async function saveHotkey() {
  const combo = hotkeyDraft.value.trim();
  try {
    await api.setHotkey(combo);
    hotkey.value = combo;
    hotkeyError.value = "";
    editingHotkey.value = false;
  } catch (e) {
    // the backend message says why the combo was refused (taken by
    // another app, unparseable) - show it instead of a generic guess
    hotkeyError.value = e
      ? String(e)
      : "Ten skrót jest zajęty lub niedozwolony — wybierz inną kombinację.";
  }
}

async function toggleAutostart() {
  autostartBusy.value = true;
  autostartError.value = "";
  const next = !autostart.value;
  autostart.value = next;
  try {
    await api.setAutostart(next);
  } catch (e) {
    // flip back and say why - a silent revert leaves the user guessing
    autostart.value = !next;
    autostartError.value = e ? String(e) : "Nie udało się zapisać autostartu.";
  } finally {
    autostartBusy.value = false;
  }
}

function onKeydown(e) {
  if (e.key === "Escape") {
    if (confirmRevoke.value) {
      confirmRevoke.value = null;
      return;
    }
    emit("close");
    return;
  }
  // light focus loop: the overlay is opaque, so Tab must never wander
  // into the editor behind it
  if (e.key === "Tab" && rootEl.value && !rootEl.value.contains(document.activeElement)) {
    closeBtn.value?.focus();
  }
}

onMounted(async () => {
  window.addEventListener("keydown", onKeydown);
  closeBtn.value?.focus();
  try {
    const settings = await api.getSettings();
    hotkey.value = settings.hotkey;
    portLocked.value = Boolean(settings.port_locked);
    portPending.value =
      settings.port && settings.port !== (props.status.port || 8500)
        ? settings.port
        : null;
    portDraft.value = String(settings.port ?? props.status.port ?? 8500);
  } catch {
    /* keep the defaults when the backend is unreachable */
  }
  autostart.value = await api.getAutostart().catch(() => false);
  refreshLan();
  refreshDevices();
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
  stopPairingClock();
});
</script>

<template>
  <div ref="rootEl" class="settings" role="dialog" aria-modal="true" aria-label="Ustawienia">
    <header class="bar">
      <h1>Ustawienia</h1>
      <button ref="closeBtn" class="close" title="Zamknij (Esc)" @click="$emit('close')">
        <i class="fas fa-times" aria-hidden="true"></i>
      </button>
    </header>

    <div class="wall">
      <section class="row" aria-labelledby="sec-general">
        <h2 id="sec-general">Ogólne</h2>
        <div class="tiles">
          <article class="tile" :class="tileCls('launch')">
            <button
              class="hit"
              :tabindex="focused === 'launch' ? -1 : 0"
              aria-label="Rozwiń: Uruchamianie"
              @click="focused = 'launch'"
            ></button>
            <header class="tile-head">
              <h3>Uruchamianie</h3>
              <span class="chip" :class="autostart ? 'ok' : ''">
                {{ autostart ? "autostart wł." : "autostart wył." }}
              </span>
            </header>
            <p class="sum">Zachowanie Pulpitu po zalogowaniu i przy zamykaniu okna.</p>
            <div v-show="focused === 'launch'" class="detail">
              <div class="ctl">
                <div class="ctl-text">
                  <span class="ctl-name">Uruchamiaj po zalogowaniu</span>
                  <span class="ctl-note">Rejestruje Pulpit w autostarcie systemu.</span>
                </div>
                <button
                  class="switch"
                  role="switch"
                  :aria-checked="autostart"
                  :disabled="autostartBusy"
                  @click="toggleAutostart"
                ><span class="knob"></span></button>
              </div>
              <p v-if="autostartError" class="err">{{ autostartError }}</p>
              <div class="ctl">
                <div class="ctl-text">
                  <span class="ctl-name">Zamykanie do zasobnika</span>
                  <span class="ctl-note">Zawsze aktywne — zamknięcie okna chowa Pulpit do zasobnika.</span>
                </div>
                <span class="locked"><i class="fas fa-lock" aria-hidden="true"></i> stałe</span>
              </div>
            </div>
          </article>

          <article class="tile" :class="tileCls('hotkey')">
            <button
              class="hit"
              :tabindex="focused === 'hotkey' ? -1 : 0"
              aria-label="Rozwiń: Hotkey trybu dotykowego"
              @click="focused = 'hotkey'"
            ></button>
            <header class="tile-head">
              <h3>Hotkey trybu dotykowego</h3>
            </header>
            <p class="sum">Skrót przełączający edytor w pełnoekranowy tryb dotykowy.</p>
            <div v-show="focused === 'hotkey'" class="detail">
              <div v-if="!editingHotkey" class="combo-line">
                <kbd class="combo">{{ hotkey }}</kbd>
                <button class="act" @click="editingHotkey = true; hotkeyDraft = hotkey">Zmień</button>
              </div>
              <template v-else>
                <div class="combo-line">
                  <input
                    v-model="hotkeyDraft"
                    class="combo-input"
                    placeholder="Ctrl+Alt+D"
                    aria-label="Nowy skrót"
                    @keyup.enter="saveHotkey"
                  />
                  <button class="act accent" @click="saveHotkey">Ustaw</button>
                  <button class="act" @click="editingHotkey = false">Anuluj</button>
                </div>
                <p v-if="hotkeyError" class="err">{{ hotkeyError }}</p>
              </template>
            </div>
          </article>
        </div>
      </section>

      <section class="row" aria-labelledby="sec-server">
        <h2 id="sec-server">Serwer</h2>
        <div class="tiles">
          <article class="tile" :class="tileCls('status')">
            <button
              class="hit"
              :tabindex="focused === 'status' ? -1 : 0"
              aria-label="Rozwiń: Status serwera"
              @click="focused = 'status'"
            ></button>
            <header class="tile-head">
              <h3>Status</h3>
              <span class="chip" :class="status.dbOk ? 'ok' : 'bad'">
                {{ status.dbOk ? "działa" : "baza niedostępna" }}
              </span>
            </header>
            <p v-if="status.dbOk" class="sum">
              Nasłuch na 0.0.0.0:{{ status.port }} — legacy i v2 dzielą jeden port.
            </p>
            <p v-else class="sum">
              Baza jest zablokowana lub nie istnieje. Zamknij oryginalną aplikację i uruchom Pulpit ponownie.
            </p>
            <div v-show="focused === 'status' && status.dbOk" class="detail">
              <dl class="facts">
                <div><dt>Adres</dt><dd class="mono tnum">0.0.0.0:{{ status.port }}</dd></div>
                <div><dt>Klienci</dt><dd class="tnum">{{ status.clients }}</dd></div>
                <div><dt>Wersja</dt><dd>Editor v{{ version }}</dd></div>
                <div><dt>Logi</dt><dd class="mono">~/pulpitApp/logs</dd></div>
              </dl>
              <div class="ctl" style="margin-top: 12px">
                <div class="ctl-text">
                  <span class="ctl-name">Aktualizacje</span>
                  <span class="ctl-note">Sprawdza feed z update_url w editor.json; nie pobiera nic sama.</span>
                </div>
                <button class="act" :disabled="updateBusy" @click="checkUpdates">Sprawdź</button>
              </div>
              <p v-if="updateNote" :class="updateNoteBad ? 'err' : 'note pending'">{{ updateNote }}</p>
            </div>
          </article>

          <article class="tile" :class="tileCls('port')">
            <button
              class="hit"
              :tabindex="focused === 'port' ? -1 : 0"
              aria-label="Rozwiń: Port nasłuchu"
              @click="focused = 'port'"
            ></button>
            <header class="tile-head">
              <h3>Port nasłuchu</h3>
            </header>
            <p class="sum">Wspólny port obu protokołów; tablety wpinają się w ten adres.</p>
            <div v-show="focused === 'port'" class="detail">
              <div class="port-line">
                <span class="big mono tnum">{{ status.port || 8500 }}</span>
                <template v-if="!portLocked">
                  <input
                    v-model="portDraft"
                    class="port-input"
                    inputmode="numeric"
                    autocomplete="off"
                    aria-label="Nowy port"
                    @keyup.enter="savePort"
                  />
                  <button class="act accent" :disabled="portBusy" @click="savePort">Zapisz</button>
                </template>
              </div>
              <p v-if="portPending" class="note pending">
                Zapisano port {{ portPending }} — zostanie użyty po ponownym uruchomieniu Pulpitu.
              </p>
              <p v-if="portLocked" class="note">
                Port narzuca zmienna środowiskowa <code class="mono">PULPIT_PORT</code> — jego
                wartość ma pierwszeństwo przed zapisem.
              </p>
              <p v-else class="note">
                Dozwolony zakres 1024–65535. Zmiana obowiązuje po ponownym uruchomieniu; tablety
                wpinają się w konkretny adres <span class="mono">host:port</span>, więc trzeba je
                wtedy skierować na nowy port.
              </p>
              <p v-if="portError" class="err">{{ portError }}</p>
            </div>
          </article>
        </div>
      </section>

      <section class="row" aria-labelledby="sec-tablets">
        <h2 id="sec-tablets">Tablety</h2>
        <div class="tiles">
          <article class="tile" :class="[tileCls('legacy'), { dead: !status.dbOk }]">
            <button
              class="hit"
              :tabindex="focused === 'legacy' && status.dbOk ? -1 : 0"
              aria-label="Rozwiń: Stock client"
              @click="focused = 'legacy'"
            ></button>
            <header class="tile-head">
              <h3>Stock client</h3>
              <span v-if="status.dbOk" class="chip" :class="lanAddresses.length ? 'ok' : ''">
                {{ lanAddresses.length ? "gotowy do parowania" : "brak sieci" }}
              </span>
              <span v-else class="chip bad">serwer wył.</span>
            </header>
            <p class="sum">Oryginalna aplikacja Deckboard na Androida łączy się przez QR.</p>
            <div v-show="focused === 'legacy' && status.dbOk" class="detail">
              <template v-if="lanAddresses.length">
                <div class="pair">
                  <img
                    class="qr"
                    :src="lanAddresses[activeAddress]?.qr"
                    alt="Kod QR z adresem pulpitu"
                  />
                  <div class="addr-list">
                    <button
                      v-for="(a, i) in lanAddresses"
                      :key="a.ipv4"
                      class="addr"
                      :class="{ sel: i === activeAddress }"
                      @click="activeAddress = i"
                    >
                      <span class="mono tnum">{{ a.ipv4 }}:{{ status.port }}</span>
                      <span class="addr-name">{{ a.name }}</span>
                    </button>
                    <button class="act" :disabled="lanLoading" @click="refreshLan">
                      <i class="fas fa-sync-alt" aria-hidden="true"></i> Odśwież
                    </button>
                  </div>
                </div>
                <p class="note">Zeskanuj QR w aplikacji Deckboard lub wpisz adres ręcznie.</p>
              </template>
              <p v-else class="note">
                Pulpit nie widzi żadnej sieci lokalnej — podłącz Wi-Fi lub Ethernet i odśwież.
              </p>
            </div>
          </article>

          <article class="tile" :class="[tileCls('v2'), { dead: !status.dbOk }]">
            <button
              class="hit"
              :tabindex="focused === 'v2' && status.dbOk ? -1 : 0"
              aria-label="Rozwiń: Nowy klient v2"
              @click="focused = 'v2'"
            ></button>
            <header class="tile-head">
              <h3>Nowy klient (v2)</h3>
              <span v-if="pairingOffer && status.dbOk" class="chip ok">kod żyje {{ pairingClock }}</span>
            </header>
            <p class="sum">Natywny klient Pulpit — jednorazowy kod zamiast otwartego QR.</p>
            <div v-show="focused === 'v2' && status.dbOk" class="detail">
              <template v-if="!pairingOffer">
                <button class="act primary" :disabled="pairingBusy" @click="generatePairing">
                  <i class="fas fa-key" aria-hidden="true"></i> Generuj kod parowania
                </button>
                <p class="note">Kod działa 5 minut i wypala się po pierwszym użyciu.</p>
              </template>
              <template v-else>
                <div class="code mono tnum">{{ pairingOffer.code }}</div>
                <p class="note">Wpisz kod w nowym kliencie razem z adresem. Wygasa za {{ pairingClock }}.</p>
                <button class="act" :disabled="pairingBusy" @click="generatePairing">Nowy kod</button>
              </template>
              <p v-if="pairingError" class="err">{{ pairingError }}</p>
            </div>
          </article>

          <article class="tile" :class="[tileCls('devices'), { dead: !status.dbOk }]">
            <button
              class="hit"
              :tabindex="focused === 'devices' && status.dbOk ? -1 : 0"
              aria-label="Rozwiń: Zaufane urządzenia"
              @click="focused = 'devices'"
            ></button>
            <header class="tile-head">
              <h3>Zaufane urządzenia</h3>
              <span v-if="status.dbOk" class="chip" :class="devices.length ? 'ok' : ''">
                {{ devices.length ? `zaufane: ${devices.length}` : "brak" }}
              </span>
              <span v-else class="chip bad">serwer wył.</span>
            </header>
            <p class="sum">
              Tablety z zapisanym tokenem. Nowe urządzenie pyta o zaufanie przy pierwszym połączeniu.
            </p>
            <div v-show="focused === 'devices' && status.dbOk" class="detail">
              <p v-if="!devices.length" class="note">
                Żadne urządzenie nie ma jeszcze zaufania — wygeneruj kod parowania powyżej.
              </p>
              <template v-else>
                <ul class="devices">
                  <li v-for="d in devices" :key="d.id" class="device">
                    <span class="device-name">{{ d.name }}</span>
                    <span class="device-seen">widziany: {{ lastSeenLabel(d.last_seen) }}</span>
                    <button class="act danger" :disabled="devicesBusy" @click="revokeDevice(d)">
                      <i class="fas fa-user-slash" aria-hidden="true"></i> Cofnij zaufanie
                    </button>
                  </li>
                </ul>
                <p class="note">
                  Cofnięcie usuwa token urządzenia i zamyka jego połączenie. Wróci tylko przez
                  nowe parowanie. Pytanie o zaufanie pozostawione bez odpowiedzi wygasa razem
                  z kodem parowania i odrzuca tablet.
                </p>
                <div class="ctl" style="margin-top: 10px">
                  <div class="ctl-text">
                    <span class="ctl-name">Instalacja APK (sideload)</span>
                    <span class="ctl-note">
                      Tablet podłączony po USB z adb; instalacja z zachowaniem danych
                      (adb install -r). Urządzenia: {{ adbList.length ? adbList.join(", ") : "brak" }}
                    </span>
                  </div>
                  <span style="display: flex; gap: 8px; flex: none">
                    <button class="act" :disabled="adbBusy" @click="refreshAdb">Odśwież</button>
                    <button class="act accent" :disabled="adbBusy" @click="installApk">Zainstaluj APK…</button>
                  </span>
                </div>
                <p v-if="adbNote" :class="adbNoteBad ? 'err' : 'note pending'">{{ adbNote }}</p>
              </template>
            </div>
          </article>
        </div>
      </section>
    </div>

    <!-- in-app revoke confirmation; replaces the native dialog popup -->
    <div
      v-if="confirmRevoke"
      class="veil"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="revoke-title"
    >
      <div class="confirm">
        <div class="confirm-icon">
          <i class="fas fa-user-slash" aria-hidden="true"></i>
        </div>
        <h2 id="revoke-title">Cofnąć zaufanie?</h2>
        <p class="confirm-name">{{ confirmRevoke.name }}</p>
        <p class="confirm-warn">
          Token urządzenia zostanie usunięty, a jego połączenie zamknięte natychmiast.
          Tablet wróci tylko przez nowe parowanie.
        </p>
        <p v-if="revokeError" class="err">{{ revokeError }}</p>
        <div class="confirm-actions">
          <button ref="cancelBtn" class="act" @click="confirmRevoke = null">Anuluj</button>
          <button class="act danger-solid" :disabled="devicesBusy" @click="doRevoke">
            <i class="fas fa-user-slash" aria-hidden="true"></i> Odwołaj urządzenie
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings {
  --wall: #171c21;
  --tile: #262e36;
  --tile-2: #313a44;
  --line: #1d2a36;
  --ink: #ecf0f1;
  --ink-2: #95a5a6;
  --ink-3: #8b97a1;
  --ok: var(--accent);
  --bad: #e74c3c;
  --bad-ink: #f19488;
  position: fixed;
  inset: 0;
  z-index: 45;
  background: var(--wall);
  color: var(--ink);
  display: flex;
  flex-direction: column;
  user-select: none;
}

.bar {
  flex: none;
  height: 64px;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 34px;
}
.bar h1 {
  font-size: 18px;
  font-weight: 600;
  letter-spacing: 0.2px;
  flex: 1;
}
.close {
  width: 40px;
  height: 40px;
  border-radius: 10px;
  color: var(--ink-2);
  font-size: 15px;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: background 140ms ease-out, color 140ms ease-out;
}
.close:hover { background: var(--tile-2); color: var(--ink); }
.close:active { transform: scale(0.96); }

.wall {
  flex: 1;
  overflow-y: auto;
  width: 100%;
  max-width: 1120px;
  margin: 0 auto;
  padding: 6px 34px 48px;
}

.row { margin-top: 30px; }
.row > h2 {
  font-size: 12.5px;
  font-weight: 600;
  letter-spacing: 1.6px;
  text-transform: uppercase;
  color: var(--ink-3);
  margin: 0 0 12px;
}

.tiles {
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  align-items: stretch;
}

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
.tile.dead,
.tile.dead:hover { opacity: 0.42; }
.tile.dead .hit { display: none; }

/* the invisible hit zone covers an unexpanded tile so a click anywhere
   moves the focus; once expanded it steps aside for the real controls */
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
}
.chip.ok { color: var(--ok); background: rgba(26, 188, 156, 0.14); }
.chip.bad { color: var(--bad-ink); background: rgba(231, 76, 60, 0.14); }

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
.ctl-text { min-width: 0; }
.ctl-name { display: block; font-size: 13.5px; font-weight: 500; }
.ctl-note { display: block; font-size: 12px; color: var(--ink-3); margin-top: 2px; line-height: 1.5; }
.locked {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11.5px;
  color: var(--ink-3);
}

.switch {
  flex: none;
  position: relative;
  width: 46px;
  height: 26px;
  border-radius: 13px;
  background: var(--tile-2);
  transition: background 160ms ease-out;
}
.switch:hover { background: #3a4753; }
.switch[aria-checked="true"] { background: var(--accent); }
.switch[aria-checked="true"]:hover { background: var(--accent-2); }
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
.switch[aria-checked="true"] .knob { transform: translateX(20px); }

.combo-line { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.combo {
  font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  font-size: 13px;
  background: var(--tile-2);
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 7px 12px;
}
.combo-input {
  flex: 1;
  min-width: 120px;
  max-width: 200px;
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

.err { font-size: 12px; color: var(--bad-ink); margin: 8px 0 0; overflow-wrap: anywhere; }

.facts {
  margin: 0;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px 24px;
}
.facts dt { font-size: 11.5px; color: var(--ink-3); margin-bottom: 2px; }
.facts dd { margin: 0; font-size: 13.5px; }

.big { font-size: 30px; font-weight: 700; line-height: 1.1; }
.note { font-size: 12px; line-height: 1.6; color: var(--ink-3); margin: 10px 0 0; }
.note code { padding: 1px 5px; border-radius: 4px; background: var(--tile-2); font-size: 11.5px; }

.pair { display: flex; gap: 14px; align-items: flex-start; }

.devices {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.device {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 7px 9px;
  border-radius: 6px;
  background: var(--tile-2);
}
.device-name {
  flex: 1;
  min-width: 0;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.device-seen { flex: none; font-size: 11.5px; color: var(--ink-3); }
.qr {
  flex: none;
  width: 108px;
  height: 108px;
  background: #fff;
  border-radius: 8px;
  padding: 6px;
}
.addr-list { display: flex; flex-direction: column; gap: 4px; flex: 1; min-width: 0; }
.addr {
  text-align: left;
  padding: 6px 9px;
  border-radius: 6px;
  display: flex;
  flex-direction: column;
  font-size: 13px;
  transition: background 140ms ease-out;
}
.addr:hover, .addr.sel { background: var(--tile-2); }
.addr.sel .mono { font-weight: 700; }
.addr-name {
  font-size: 11.5px;
  color: var(--ink-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.code {
  display: inline-block;
  font-size: 30px;
  font-weight: 700;
  letter-spacing: 6px;
  line-height: 1;
  background: #171c21;
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 14px 18px;
}

.mono { font-family: ui-monospace, "Cascadia Mono", Consolas, monospace; }

.port-line { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.port-input {
  width: 110px;
  padding: 7px 10px;
  font-size: 14px;
  background: #171c21;
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--ink);
}
.port-input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
}
.note.pending { color: var(--ok); }

.veil {
  position: absolute;
  inset: 0;
  z-index: 10;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(10, 14, 17, 0.62);
  backdrop-filter: blur(2px);
  animation: fade 140ms ease-out;
}
@keyframes fade {
  from { opacity: 0; }
  to { opacity: 1; }
}
.confirm {
  width: min(380px, calc(100vw - 80px));
  background: var(--tile);
  border: 1px solid rgba(231, 76, 60, 0.35);
  border-radius: 14px;
  padding: 26px 26px 22px;
  text-align: center;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.55);
  animation: rise 200ms ease-out;
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
  color: var(--bad-ink);
  background: rgba(231, 76, 60, 0.16);
}
.confirm h2 {
  font-size: 17px;
  font-weight: 600;
  margin: 0 0 4px;
}
.confirm-name {
  font-size: 14px;
  font-weight: 600;
  margin: 0;
  overflow-wrap: anywhere;
}
.confirm-warn {
  font-size: 12.5px;
  line-height: 1.6;
  color: var(--ink-2);
  margin: 8px 0 0;
}
.confirm-actions {
  display: flex;
  justify-content: center;
  gap: 10px;
  margin-top: 20px;
}
.act.danger-solid {
  background: var(--bad);
  color: #fff;
  font-weight: 600;
}
.act.danger-solid:hover { background: #c0392b; }
.act.danger-solid:disabled { opacity: 0.55; }
</style>
