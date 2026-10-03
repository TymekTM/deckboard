<script setup>
import { computed, onMounted, onUnmounted, ref } from "vue";
import { ask } from "@tauri-apps/plugin-dialog";
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
let pairingTimer = null;

// trusted v2 devices (the backend's devices.json, minus token material)
const devices = ref([]);
const devicesBusy = ref(false);

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
    console.error("pairing code", e);
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

async function revokeDevice(device) {
  const ok = await ask(
    `Cofnąć zaufanie urządzeniu "${device.name}"? ` +
      "Jego token przestaje działać, a połączenie zostanie natychmiast zamknięte.",
    { title: "Cofnij zaufanie", kind: "warning" },
  );
  if (!ok) return;
  devicesBusy.value = true;
  try {
    await api.revokeDevice(device.id);
  } catch (e) {
    console.error("revoke device", e);
  } finally {
    devicesBusy.value = false;
    refreshDevices();
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
  const next = !autostart.value;
  autostart.value = next;
  try {
    await api.setAutostart(next);
  } catch {
    autostart.value = !next;
  } finally {
    autostartBusy.value = false;
  }
}

function onKeydown(e) {
  if (e.key === "Escape") {
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
    hotkey.value = (await api.getSettings()).hotkey;
  } catch {
    /* keep the default combo when the backend is unreachable */
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
              <div class="big mono tnum">{{ status.port || 8500 }}</div>
              <p class="note">
                W tym wydzeniu port jest tylko do odczytu — ustawia go zmienna
                środowiskowa <code class="mono">PULPIT_PORT</code> (domyślnie 8500).
              </p>
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
              </template>
            </div>
          </article>
        </div>
      </section>
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
</style>
