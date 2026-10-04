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

// ---- dead lanes never light --------------------------------------------
eq("empty binding stays on the tap flip", stateActive(obsScene, { scene: "Game" }, { anything: "ON" }, {}), null);
eq("unbound plain type stays on the tap flip", stateActive({ type: "key", command: "" }, {}, { anything: "ON" }, {}), null);

if (failures.length) {
  console.error("stateActive check failed:");
  for (const f of failures) console.error("  - " + f);
  process.exit(1);
}
console.log("stateActive ok: unbound toggles match the mobile isActiveValue, bound comparisons unchanged");
