// Catalog invariant check (012 E1/C5) - plain node, no devDependencies:
//   node scripts/check-catalog.mjs
// Asserts, for the tile dialog's action catalog:
//   1. every dual-flagged entry (second-state styling editable) has a
//      STATE_BINDINGS binding, so nobody ships a toggle tile without
//      deciding what drives its second state;
//   2. binding keys point at real catalog entries (catches renames);
//   3. the plan-windows options token round-trips through
//      parsePlanWindows/setPlanWindows, the one shared by the edit
//      dialog and the status tile's row filter (DESK-10).
import {
  CATALOG,
  STATE_BINDINGS,
  parsePlanWindows,
  setPlanWindows,
} from "../src/catalog.js";

const failures = [];

const dualEntries = CATALOG.filter((c) => c.value && c.dual);
for (const entry of dualEntries) {
  if (!STATE_BINDINGS[entry.value]) {
    failures.push(`dual entry "${entry.value}" has no STATE_BINDINGS entry`);
  }
}

// binding keys must point at real catalog entries (catches renames)
for (const type of Object.keys(STATE_BINDINGS)) {
  if (!CATALOG.some((c) => c.value === type) && !type.startsWith("vmod-")) {
    // vmod-* arrive as runtime extension inputs, not static catalog rows
    failures.push(`STATE_BINDINGS["${type}"] matches no catalog entry`);
  }
}

// plan-windows token: what the dialog writes is what the tile parses
for (const [options, want] of [
  ["", { five: true, week: true }],
  ["windows:5h,week", { five: true, week: true }],
  ["windows:5h", { five: true, week: false }],
  ["windows:week", { five: false, week: true }],
  ["windows:", { five: false, week: false }],
  ["windows: 5h , week ", { five: true, week: true }],
  ["other:x;windows:week", { five: false, week: true }],
  ["windows:not-a-window", { five: false, week: false }],
]) {
  const got = parsePlanWindows(options);
  if (got.five !== want.five || got.week !== want.week) {
    failures.push(
      `parsePlanWindows(${JSON.stringify(options)}) -> {five: ${got.five}, week: ${got.week}}, want {five: ${want.five}, week: ${want.week}}`,
    );
  }
}
for (const windows of [
  { five: true, week: true },
  { five: true, week: false },
  { five: false, week: true },
  { five: false, week: false },
]) {
  const written = setPlanWindows("other:x", windows);
  const back = parsePlanWindows(written);
  if (back.five !== windows.five || back.week !== windows.week) {
    failures.push(
      `plan windows do not round-trip: ${JSON.stringify(windows)} -> ${written} -> {five: ${back.five}, week: ${back.week}}`,
    );
  }
  if (!written.includes("other:x")) {
    failures.push(`setPlanWindows dropped sibling tokens: ${written}`);
  }
}

if (failures.length) {
  console.error("catalog parity check failed:");
  for (const f of failures) console.error("  - " + f);
  process.exit(1);
}
console.log(
  `catalog parity ok: ${dualEntries.length} dual entries, all bound`,
);
