// stateActive semantics check (MOB-10 desktop side) - plain node, no
// devDependencies, wired like check-catalog.mjs:
//   node scripts/check-state-active.mjs
// Asserts the dual-state decision rules of catalog.js stateActive against
// the shapes producers actually push, mirroring the mobile client's
// isActiveValue (apps/mobile .../net/V2Client.kt) so one pushed value
// lights the tile on both surfaces:
//   - booleans win outright (speaker-muted pushes them);
//   - a binding that names a command field (`cmd`/`key`) keeps the
//     original comparison semantics (string equality / array membership,
//     "" reads unknown);
//   - an unbound toggle value carries the state itself: "ON"/"1" are
//     active, "OFF"/"0"/"" and anything else are not (discord's
//     _labelMuteDeaf pushes ON/OFF strings under toggle-microphone /
//     toggle-headphone);
//   - null (nothing pushed) stays unknown so the session tap-flip keeps
//     deciding, and empty bindings (obs/vmod/vm toggles) never light.
import { stateActive } from "../src/catalog.js";

const failures = [];

function eq(name, got, want) {
  const pass = got === want;
  if (!pass) failures.push(`${name}: got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
  return pass;
}

const volMute = { type: "vol", command: "vol_mute" };
const cv = { type: "my-var", command: "" };
const cvMeta = { "my-var": { mode: "custom-value" } };
const speakerCmd = { speaker: "endpoint-a" };
const speakerTile = { type: "speaker-device", command: JSON.stringify(speakerCmd) };
const obsScene = { type: "obs-scene", command: JSON.stringify({ scene: "Game" }) };

// ---- unbound toggles: the pushed value IS the state (MOB-10) -----------
eq("vol_mute ON is active", stateActive(volMute, {}, { "speaker-muted": "ON" }, {}), true);
eq("vol_mute 1 is active", stateActive(volMute, {}, { "speaker-muted": "1" }, {}), true);
eq("vol_mute true is active", stateActive(volMute, {}, { "speaker-muted": true }, {}), true);
eq("vol_mute OFF is inactive", stateActive(volMute, {}, { "speaker-muted": "OFF" }, {}), false);
eq("vol_mute 0 is inactive", stateActive(volMute, {}, { "speaker-muted": "0" }, {}), false);
eq("vol_mute empty string is inactive", stateActive(volMute, {}, { "speaker-muted": "" }, {}), false);
eq("vol_mute false is inactive", stateActive(volMute, {}, { "speaker-muted": false }, {}), false);
eq("vol_mute unpushed stays unknown", stateActive(volMute, {}, {}, {}), null);
eq(
  "vol_mute numeric push reads inactive like the mobile",
  stateActive(volMute, {}, { "speaker-muted": 1 }, {}),
  false,
);
eq("vol play tile never tracks state", stateActive({ type: "vol", command: "play" }, {}, { "speaker-muted": "ON" }, {}), null);

// ---- bound comparison (watch + cmd key) keeps its semantics ------------
eq(
  "speaker-device matching endpoint is active",
  stateActive(speakerTile, speakerCmd, { "speaker-device": "endpoint-a" }, {}),
  true,
);
eq(
  "speaker-device other endpoint is inactive",
  stateActive(speakerTile, speakerCmd, { "speaker-device": "endpoint-b" }, {}),
  false,
);
eq(
  "speaker-device array membership",
  stateActive(speakerTile, speakerCmd, { "speaker-device": ["endpoint-a", "endpoint-b"] }, {}),
  true,
);
eq(
  "speaker-device array without the endpoint",
  stateActive(speakerTile, speakerCmd, { "speaker-device": ["endpoint-b"] }, {}),
  false,
);
eq(
  "speaker-device boolean wins over comparison",
  stateActive(speakerTile, speakerCmd, { "speaker-device": false }, {}),
  false,
);
eq(
  "speaker-device unpushed stays unknown",
  stateActive(speakerTile, speakerCmd, {}, {}),
  null,
);

// ---- custom-value mode tiles follow their variable ----------------------
eq("custom-value ON is active", stateActive(cv, {}, { "my-var": "ON" }, cvMeta), true);
eq("custom-value OFF is inactive", stateActive(cv, {}, { "my-var": "OFF" }, cvMeta), false);
eq("custom-value boolean push", stateActive(cv, {}, { "my-var": true }, cvMeta), true);
eq("custom-value unpushed stays unknown", stateActive(cv, {}, {}, cvMeta), null);

// ---- spotify-playback: command-scoped like vol_mute ----------------------
const spotifyPlay = { type: "spotify-playback", command: "play" };
eq("spotify play ON is active", stateActive(spotifyPlay, {}, { "spotify-playing": "ON" }, {}), true);
eq("spotify play OFF is inactive", stateActive(spotifyPlay, {}, { "spotify-playing": "OFF" }, {}), false);
eq("spotify play unpushed stays unknown", stateActive(spotifyPlay, {}, {}, {}), null);
eq(
  "spotify next never tracks state",
  stateActive({ type: "spotify-playback", command: "next" }, {}, { "spotify-playing": "ON" }, {}),
  null,
);

// ---- spotify watch bindings (ON/OFF strings, unbound comparison) --------
const spotifyShuffle = { type: "spotify-shuffle", command: "" };
eq("spotify-shuffle ON is active", stateActive(spotifyShuffle, {}, { "spotify-shuffle": "ON" }, {}), true);
eq("spotify-shuffle OFF is inactive", stateActive(spotifyShuffle, {}, { "spotify-shuffle": "OFF" }, {}), false);
const spotifyRepeat = { type: "spotify-repeat", command: "" };
eq("spotify repeat-on ON is active", stateActive(spotifyRepeat, {}, { "spotify-repeat-on": "ON" }, {}), true);
eq("spotify repeat-off is inactive", stateActive(spotifyRepeat, {}, { "spotify-repeat-on": "OFF" }, {}), false);
const spotifyLike = { type: "spotify-like", command: "" };
eq("spotify-like ON is active", stateActive(spotifyLike, {}, { "spotify-liked": "ON" }, {}), true);
eq("spotify-like unpushed stays unknown", stateActive(spotifyLike, {}, {}, {}), null);

// ---- native OBS watch bindings (round 5) --------------------------------
// The connection worker pushes: obs-scene -> current scene name,
// obs-source/obs-filter/obs-device-audio -> name arrays, the toggles ->
// booleans (crates/obs/src/state.rs to_snapshot). The bound comparisons
// reuse the speaker-device semantics.
const obsSource = { type: "obs-source", command: JSON.stringify({ source: "Webcam" }) };
eq("obs-scene current scene is active", stateActive(obsScene, { scene: "Game" }, { "obs-scene": "Game" }, {}), true);
eq("obs-scene other scene is inactive", stateActive(obsScene, { scene: "Game" }, { "obs-scene": "Intro" }, {}), false);
eq("obs-scene empty push reads unknown", stateActive(obsScene, { scene: "Game" }, { "obs-scene": "" }, {}), null);
eq("obs-scene unpushed stays unknown", stateActive(obsScene, { scene: "Game" }, {}, {}), null);
eq(
  "obs-source enabled-in-scene is active",
  stateActive(obsSource, { source: "Webcam" }, { "obs-source": ["Webcam", "Chat"] }, {}),
  true,
);
eq(
  "obs-source not in the scene is inactive",
  stateActive(obsSource, { source: "Webcam" }, { "obs-source": ["Chat"] }, {}),
  false,
);
const obsMute = { type: "obs-device-audio", command: JSON.stringify({ device: "Mic" }) };
eq("obs-device-audio muted is active", stateActive(obsMute, { device: "Mic" }, { "obs-device-audio": ["Mic"] }, {}), true);
eq("obs-device-audio live is inactive", stateActive(obsMute, { device: "Mic" }, { "obs-device-audio": [] }, {}), false);
const obsFilter = { type: "obs-filter", command: JSON.stringify({ filter: "Blur" }) };
eq("obs-filter enabled is active", stateActive(obsFilter, { filter: "Blur" }, { "obs-filter": ["Blur"] }, {}), true);
eq("obs-studio-mode boolean push", stateActive({ type: "obs-studio-mode", command: "" }, {}, { "obs-studio-mode": true }, {}), true);
eq("obs-record off push is inactive", stateActive({ type: "obs-record", command: "" }, {}, { "obs-record": false }, {}), false);
eq("obs-stream unpushed stays unknown", stateActive({ type: "obs-stream", command: "" }, {}, {}, {}), null);

// ---- dead lanes never light --------------------------------------------
eq("empty binding stays on the tap flip", stateActive({ type: "slobs-scene", command: JSON.stringify({ scene: "Game" }) }, { scene: "Game" }, { anything: "ON" }, {}), null);
eq("unbound plain type stays on the tap flip", stateActive({ type: "key", command: "" }, {}, { anything: "ON" }, {}), null);

if (failures.length) {
  console.error("stateActive check failed:");
  for (const f of failures) console.error("  - " + f);
  process.exit(1);
}
console.log("stateActive ok: unbound toggles match the mobile isActiveValue, bound comparisons unchanged");
