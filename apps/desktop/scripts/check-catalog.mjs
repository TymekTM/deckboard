// Catalog invariant check (012 E1/C5) - plain node, no devDependencies:
//   node scripts/check-catalog.mjs
// Asserts, for the tile dialog's action catalog:
//   1. every dual-flagged entry (second-state styling editable) has a
//      STATE_BINDINGS binding, so nobody ships a toggle tile without
//      deciding what drives its second state;
//   2. every `app` binding names the state key it reads (`key`) - a keyless
//      app binding would truthiness-test the whole app state object and
//      light every tile of that integration on any push (the OBS bug).
import { CATALOG, STATE_BINDINGS } from "../src/catalog.js";

const failures = [];

const dualEntries = CATALOG.filter((c) => c.value && c.dual);
for (const entry of dualEntries) {
  const binding = STATE_BINDINGS[entry.value];
  if (!binding) {
    failures.push(`dual entry "${entry.value}" has no STATE_BINDINGS entry`);
    continue;
  }
  if (binding.app && !binding.key && !binding.watch) {
    failures.push(
      `binding "${entry.value}" reads the whole "${binding.app}" state object; add the state key it should compare`,
    );
  }
}

// binding keys must point at real catalog entries (catches renames)
for (const type of Object.keys(STATE_BINDINGS)) {
  if (!CATALOG.some((c) => c.value === type) && !type.startsWith("vmod-")) {
    // vmod-* arrive as runtime extension inputs, not static catalog rows
    failures.push(`STATE_BINDINGS["${type}"] matches no catalog entry`);
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
