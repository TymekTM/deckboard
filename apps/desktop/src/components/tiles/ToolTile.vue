<script setup>
import { computed, onBeforeUnmount, ref, watchEffect } from "vue";
import { api } from "../../api";

// One native utility tile (catalog group "Tools"): clock, countdown
// timer, stopwatch, counter. The state lives on the server
// (`~/pulpitApp/tools.json`), compact snapshots arrive through the
// APP_CUSTOM_VALUE lane under `tool-<button id>` and time is
// extrapolated locally against the epoch timestamps in the snapshot -
// the same trick the Spotify playback progress uses, so a running timer
// costs no periodic pushes. Gestures (touch mode only): tap
// start/pause/+1, double-tap reset/-1, long-press reset; the gestures
// are the same interactions the v2 manifest declares, so tablets and
// this surface stay in lockstep.
const props = defineProps({
  tile: { type: Object, required: true },
  // pre-parsed command JSON, owned by the parent (TileCell)
  cmd: { type: Object, default: () => ({}) },
  // last compact state object from the server (null = nothing pushed
  // yet: render from the tile config instead)
  state: { type: Object, default: null },
  touch: { type: Boolean, default: false },
  titleColor: { type: String, default: "#ffffff" },
});
const emit = defineEmits(["tap"]);

const kind = computed(() => String(props.tile.type || ""));

// ---- extrapolation (server epoch ms -> local wall clock) -------------------

const nowTick = ref(Date.now());
watchEffect((onCleanup) => {
  const running = props.state?.running === true;
  if (kind.value !== "tool-clock" && !running) return;
  nowTick.value = Date.now();
  const id = setInterval(() => {
    nowTick.value = Date.now();
  }, kind.value === "tool-clock" ? 1000 : 200);
  onCleanup(() => clearInterval(id));
});

function elapsedTotalMs(state) {
  if (!state) return 0;
  const extra = state.running ? Math.max(0, nowTick.value - Number(state.startedAtMs || 0)) : 0;
  return Number(state.elapsedMs || 0) + extra;
}

const timerRemainingMs = computed(() => {
  const state = props.state;
  if (!state) return parseDuration(props.cmd.duration);
  if (state.finished) return 0;
  return Math.max(0, Number(state.durationMs || 0) - elapsedTotalMs(state));
});

const timerFinished = computed(() => {
  const state = props.state;
  if (!state) return false;
  return (
    state.finished === true ||
    (state.running === true && timerRemainingMs.value <= 0)
  );
});

const stopwatchElapsedMs = computed(() => elapsedTotalMs(props.state));

const counterCount = computed(() => {
  if (props.state && Number.isFinite(Number(props.state.count))) {
    return Number(props.state.count);
  }
  return Number(props.cmd.start_value ?? 0) || 0;
});

// ---- config fallbacks (nothing pushed yet) ---------------------------------

function parseDuration(raw) {
  const parts = String(raw ?? "").split(":").map((p) => p.trim());
  if (!parts[0]) return 300000;
  const nums = parts.map(Number);
  if (nums.some((n) => !Number.isFinite(n) || n < 0)) return 300000;
  const secs = nums.length === 3
    ? nums[0] * 3600 + nums[1] * 60 + nums[2]
    : nums.length === 2
      ? nums[0] * 60 + nums[1]
      : nums[0];
  return Math.round(secs * 1000);
}

const clockTwelve = computed(() => props.cmd.format === "12h");
const clockSeconds = computed(() => props.cmd.seconds === "yes");
const clockShowDate = computed(() => props.cmd.date === "yes");
const clockZone = computed(() => {
  const tz = String(props.cmd.timezone || "").trim();
  if (!tz) return null;
  try {
    new Intl.DateTimeFormat("pl", { timeZone: tz });
    return tz;
  } catch {
    return null;
  }
});

// Plain literals in one place (translation lane extracts them); the same
// strings the Android renderer shows.
const STR = {
  running: "Działa",
  paused: "Pauza",
  ready: "Gotowy",
  finished: "KONIEC!",
};

const timerStatus = computed(() =>
  timerFinished.value ? STR.finished
    : props.state?.running ? STR.running
      : Number(props.state?.elapsedMs || 0) > 0 ? STR.paused
        : STR.ready,
);

const stopwatchStatus = computed(() =>
  props.state?.running ? STR.running
    : stopwatchElapsedMs.value > 0 ? STR.paused
      : "00:00",
);

const counterLabel = computed(() => {
  const label = String(props.cmd.label || "").trim();
  return label || props.tile.title || "Licznik";
});

const toolTitle = computed(() => props.tile.title || "");

function clockParts() {
  const now = new Date();
  const opts = {
    hour: "2-digit",
    minute: "2-digit",
    hour12: clockTwelve.value,
    ...(clockSeconds.value ? { second: "2-digit" } : {}),
    ...(clockZone.value ? { timeZone: clockZone.value } : {}),
  };
  const time = new Intl.DateTimeFormat("pl-PL", opts).format(now);
  const date = clockShowDate.value
    ? new Intl.DateTimeFormat("pl-PL", {
        weekday: "short",
        day: "numeric",
        month: "short",
        ...(clockZone.value ? { timeZone: clockZone.value } : {}),
      }).format(now)
    : "";
  return { time, date };
}

function mmss(ms) {
  const total = Math.ceil(Math.max(0, ms) / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n) => String(n).padStart(2, "0");
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

// ---- gestures (touch mode only) --------------------------------------------
// One pointer sequence fires exactly one interaction: a press held
// ~500 ms is the long-press, a second tap inside the double-tap window
// cancels the first tap's action, everything else is a tap.

const LONG_PRESS_MS = 500;
const DOUBLE_TAP_MS = 300;
const TAP_SLOP_PX = 8;

let pressTimer = null;
let tapTimer = null;
let downAt = 0;
let downY = 0;
let consumed = false;

function clearPending() {
  if (pressTimer) clearTimeout(pressTimer);
  if (tapTimer) clearTimeout(tapTimer);
  pressTimer = null;
  tapTimer = null;
}
onBeforeUnmount(clearPending);

function runGesture(name) {
  api.execButtonGesture(props.tile.id, name).catch((e) =>
    console.warn("tool gesture failed", e),
  );
}

function onPointerDown(event) {
  if (!props.touch) return;
  consumed = false;
  downAt = performance.now();
  downY = event.clientY;
  pressTimer = setTimeout(() => {
    pressTimer = null;
    consumed = true;
    runGesture("long-press");
  }, LONG_PRESS_MS);
}

function onPointerUp(event) {
  if (!props.touch) return;
  if (pressTimer) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
  if (consumed || Math.abs(event.clientY - downY) > TAP_SLOP_PX) return;
  // wait out the double-tap window before acting on a lone tap
  if (tapTimer) {
    clearTimeout(tapTimer);
    tapTimer = null;
    consumed = true;
    runGesture("double-tap");
    return;
  }
  tapTimer = setTimeout(() => {
    tapTimer = null;
    emit("tap");
  }, DOUBLE_TAP_MS);
}

// visible reset affordance (works beside the gestures): a small corner
// button, its own click never reaches the tap logic
function onResetClick(event) {
  event.stopPropagation();
  runGesture("reset");
}
</script>

<template>
  <div
    class="tool-tile"
    @pointerdown="onPointerDown"
    @pointerup="onPointerUp"
    @pointercancel="clearPending"
    @click.stop
    @dblclick="touch && $event.stopPropagation()"
  >
    <!-- clock: rendered from the device clock, the server never ticks -->
    <template v-if="kind === 'tool-clock'">
      <div class="tool-clock">
        <span class="tool-time" :class="{ small: clockSeconds }">{{
          clockParts().time
        }}</span>
        <span v-if="clockShowDate" class="tool-sub">{{ clockParts().date }}</span>
      </div>
    </template>

    <!-- countdown timer -->
    <template v-else-if="kind === 'tool-timer'">
      <div class="tool-body" :class="{ flashing: timerFinished }">
        <span class="tool-value">{{ mmss(timerRemainingMs) }}</span>
        <span v-if="toolTitle" class="tool-sub">{{ toolTitle }}</span>
        <span v-else class="tool-sub">{{ timerStatus }}</span>
      </div>
    </template>

    <!-- stopwatch -->
    <template v-else-if="kind === 'tool-stopwatch'">
      <div class="tool-body">
        <span class="tool-value">{{ mmss(stopwatchElapsedMs) }}</span>
        <span v-if="toolTitle" class="tool-sub">{{ toolTitle }}</span>
        <span v-else class="tool-sub">{{ stopwatchStatus }}</span>
      </div>
    </template>

    <!-- counter -->
    <template v-else-if="kind === 'tool-counter'">
      <div class="tool-body">
        <span v-if="toolTitle" class="tool-sub">{{ toolTitle }}</span>
        <span v-else class="tool-sub">{{ counterLabel }}</span>
        <span class="tool-value big">{{ counterCount }}</span>
      </div>
    </template>

    <button
      v-if="touch && kind !== 'tool-clock'"
      class="tool-reset"
      title="Reset"
      @click.stop="onResetClick"
      @pointerdown.stop
      @pointerup.stop
    >
      <i class="fas fa-undo"></i>
    </button>
  </div>
</template>

<style scoped>
.tool-tile {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: auto;
}
.tool-clock,
.tool-body {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 3px;
  padding: 4px;
  min-width: 0;
  pointer-events: none;
}
.tool-time {
  font-size: 26px;
  font-weight: 700;
  line-height: 1.1;
  color: v-bind("titleColor");
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
  white-space: nowrap;
}
.tool-time.small {
  font-size: 20px;
}
.tool-value {
  font-size: 24px;
  font-weight: 700;
  line-height: 1.1;
  color: v-bind("titleColor");
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
  font-variant-numeric: tabular-nums;
}
.tool-value.big {
  font-size: 32px;
}
.tool-sub {
  font-size: 10.5px;
  color: v-bind("titleColor");
  opacity: 0.75;
  max-width: 100%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
/* finish flash: the tile blinks on every surface (Android mirrors it
   with an animated overlay) */
.tool-body.flashing {
  animation: tool-flash 800ms ease-in-out infinite;
}
@keyframes tool-flash {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
.tool-reset {
  position: absolute;
  top: 3px;
  right: 4px;
  width: 22px;
  height: 22px;
  padding: 0;
  border: none;
  border-radius: 50%;
  background: rgba(0, 0, 0, 0.35);
  color: rgba(255, 255, 255, 0.85);
  font-size: 11px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
}
.tool-reset:active {
  background: rgba(0, 0, 0, 0.55);
}
</style>
