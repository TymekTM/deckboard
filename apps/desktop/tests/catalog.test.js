import { describe, it, expect } from "vitest";
import {
  CATALOG,
  STATE_BINDINGS,
  VM_SLIDER_RESET,
  CELL_W,
  ROW_H,
  MAX_BOARD_DIM,
  stateActive,
  boardDim,
  clamp,
  statusProgressAt,
  mmss,
  parsePlanWindows,
  setPlanWindows,
} from "../src/catalog.js";

const entries = CATALOG.filter((c) => c.value);

describe("CATALOG shape", () => {
  it("has unique action values", () => {
    const values = entries.map((c) => c.value);
    expect(new Set(values).size).toBe(values.length);
  });

  it("gives every action a label, icon and color", () => {
    for (const entry of entries) {
      expect(entry.label, entry.value).toBeTruthy();
      expect(entry.icon, entry.value).toBeTruthy();
      expect(entry.color, entry.value).toMatch(/^#[0-9a-fA-F]{6}$/);
    }
  });

  it("only uses known press/display modes", () => {
    for (const entry of entries) {
      if (entry.mode !== undefined) {
        expect(["slider", "status", "graph"], entry.value).toContain(entry.mode);
      }
    }
  });

  it("keeps rows as header, divider or action", () => {
    for (const row of CATALOG) {
      const kinds = [row.header, row.divider, row.value].filter(Boolean);
      expect(kinds.length, JSON.stringify(row)).toBe(1);
    }
  });

  it("opens with a header and never stacks two dividers", () => {
    expect(CATALOG[0].header).toBeTruthy();
    for (let i = 1; i < CATALOG.length; i++) {
      expect(CATALOG[i].divider && CATALOG[i - 1].divider).toBeFalsy();
    }
  });

  it("gives every select field non-empty options with unique values", () => {
    for (const entry of entries) {
      for (const field of entry.fields || []) {
        if (field.kind !== "select") continue;
        expect(field.options.length, `${entry.value}.${field.key}`).toBeGreaterThan(0);
        const values = field.options.map((o) => o.value);
        expect(new Set(values).size, `${entry.value}.${field.key}`).toBe(values.length);
      }
    }
  });

  it("points every showIf at a sibling field and a real option", () => {
    for (const entry of entries) {
      for (const field of entry.fields || []) {
        if (!field.showIf) continue;
        const target = entry.fields.find((f) => f.key === field.showIf.key);
        expect(target, `${entry.value}.${field.key}`).toBeTruthy();
        if (target.kind === "select") {
          expect(target.options.map((o) => o.value)).toContain(field.showIf.value);
        }
      }
    }
  });

  it("keeps field keys unique per entry", () => {
    for (const entry of entries) {
      const keys = (entry.fields || []).map((f) => f.key);
      expect(new Set(keys).size, entry.value).toBe(keys.length);
    }
  });

  it("only offers raw-command selects with an init present in the list", () => {
    for (const entry of entries.filter((e) => e.select)) {
      const values = entry.select.map((o) => o.value);
      expect(new Set(values).size).toBe(values.length);
      if (entry.init) expect(values).toContain(entry.init);
    }
  });

  it("gives step editors defaults that name one of their own step types", () => {
    for (const entry of entries.filter((e) => e.stepEditor)) {
      const types = entry.stepEditor.types.map((t) => t.value);
      expect(types, entry.value).toContain(entry.stepEditor.addDefaults.type);
    }
  });

  it("stores Voicemeeter indices as numbers (backend reads as_i64)", () => {
    for (const entry of entries.filter((e) => e.value.startsWith("vm-"))) {
      const index = (entry.fields || []).find((f) => f.key === "number");
      if (!index) continue;
      for (const opt of index.options) expect(typeof opt.value, entry.value).toBe("number");
      expect(index.options.map((o) => o.value)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    }
  });

  it("labels the bus index with its A/B output channel", () => {
    const bus = entries.find((e) => e.value === "vm-set-bus");
    const labels = bus.fields.find((f) => f.key === "number").options.map((o) => o.label);
    expect(labels[0]).toBe("0 (A1)");
    expect(labels[5]).toBe("5 (B1)");
    expect(labels[7]).toBe("7 (B3)");
  });

  it("marks the slider actions the backend exec_slider handles", () => {
    const sliders = entries.filter((e) => e.mode === "slider").map((e) => e.value);
    expect(sliders).toEqual(
      expect.arrayContaining([
        "speaker-volume",
        "vm-slider-bus",
        "vm-slider-strip",
        "spotify-volume",
        "spotify-seek",
      ]),
    );
  });

  it("exposes the original editor's grid geometry", () => {
    expect(CELL_W).toBe(96);
    expect(ROW_H).toBe(100);
    expect(MAX_BOARD_DIM).toBe(32);
  });
});

describe("STATE_BINDINGS", () => {
  it("binds every dual entry", () => {
    for (const entry of entries.filter((e) => e.dual)) {
      expect(STATE_BINDINGS[entry.value], entry.value).toBeDefined();
    }
  });

  it("never uses the dead app-state lane", () => {
    for (const [type, binding] of Object.entries(STATE_BINDINGS)) {
      expect(binding.app, type).toBeUndefined();
    }
  });

  it("keys only real catalog entries or runtime vmod inputs", () => {
    const values = new Set(entries.map((e) => e.value));
    for (const type of Object.keys(STATE_BINDINGS)) {
      expect(values.has(type) || type.startsWith("vmod-"), type).toBe(true);
    }
  });

  it("names a comparison field that the entry's command actually has", () => {
    for (const [type, binding] of Object.entries(STATE_BINDINGS)) {
      const field = binding.cmd || binding.key;
      if (!field) continue;
      const entry = entries.find((e) => e.value === type);
      expect(entry.fields.map((f) => f.key), type).toContain(field);
    }
  });
});

describe("stateActive", () => {
  const volMute = { type: "vol", command: "vol_mute" };
  const cv = { type: "my-var", command: "" };
  const cvMeta = { "my-var": { mode: "custom-value" } };
  const speakerCmd = { speaker: "endpoint-a" };
  const speakerTile = { type: "speaker-device", command: JSON.stringify(speakerCmd) };

  it.each([
    ["ON", true],
    ["1", true],
    [true, true],
    ["OFF", false],
    ["0", false],
    ["", false],
    [false, false],
    [1, false],
    [{ on: true }, false],
    ["on", false],
  ])("vol_mute push %j reads %j", (value, want) => {
    expect(stateActive(volMute, {}, { "speaker-muted": value }, {})).toBe(want);
  });

  it("keeps an unpushed or null mute state unknown", () => {
    expect(stateActive(volMute, {}, {}, {})).toBeNull();
    expect(stateActive(volMute, {}, { "speaker-muted": null }, {})).toBeNull();
  });

  it.each(["play", "prev", "next", "vol_up", "vol_down"])(
    "vol %s never tracks state",
    (command) => {
      expect(stateActive({ type: "vol", command }, {}, { "speaker-muted": "ON" }, {})).toBeNull();
    },
  );

  it("compares speaker-device against the command's endpoint", () => {
    const at = (value) =>
      stateActive(speakerTile, speakerCmd, { "speaker-device": value }, {});
    expect(at("endpoint-a")).toBe(true);
    expect(at("endpoint-b")).toBe(false);
    expect(at(["endpoint-b", "endpoint-a"])).toBe(true);
    expect(at(["endpoint-b"])).toBe(false);
    expect(at([])).toBe(false);
    expect(at(true)).toBe(true);
    expect(at(false)).toBe(false);
    expect(at("")).toBeNull();
    expect(at(undefined)).toBeNull();
  });

  it("treats a non-string non-array bound value as truthiness", () => {
    expect(stateActive(speakerTile, speakerCmd, { "speaker-device": 3 }, {})).toBe(true);
    expect(stateActive(speakerTile, speakerCmd, { "speaker-device": 0 }, {})).toBe(false);
  });

  it("falls back to unbound semantics when the command lacks the field", () => {
    expect(stateActive(speakerTile, {}, { "speaker-device": "ON" }, {})).toBe(true);
    expect(stateActive(speakerTile, {}, { "speaker-device": "endpoint-a" }, {})).toBe(false);
  });

  it("follows custom-value variables declared by extensions", () => {
    expect(stateActive(cv, {}, { "my-var": "ON" }, cvMeta)).toBe(true);
    expect(stateActive(cv, {}, { "my-var": "OFF" }, cvMeta)).toBe(false);
    expect(stateActive(cv, {}, { "my-var": true }, cvMeta)).toBe(true);
    expect(stateActive(cv, {}, {}, cvMeta)).toBeNull();
    // the same type without the custom-value declaration stays on tap flip
    expect(stateActive(cv, {}, { "my-var": "ON" }, {})).toBeNull();
    expect(stateActive(cv, {}, { "my-var": "ON" }, undefined)).toBeNull();
  });

  it("scopes spotify-playback to the play command", () => {
    const play = { type: "spotify-playback", command: "play" };
    expect(stateActive(play, {}, { "spotify-playing": "ON" }, {})).toBe(true);
    expect(stateActive(play, {}, { "spotify-playing": "OFF" }, {})).toBe(false);
    expect(stateActive(play, {}, {}, {})).toBeNull();
    for (const command of ["next", "prev", "vol_up", "vol_down", "vol_mute"]) {
      expect(
        stateActive({ type: "spotify-playback", command }, {}, { "spotify-playing": "ON" }, {}),
      ).toBeNull();
    }
  });

  it.each([
    ["spotify-shuffle", "spotify-shuffle"],
    ["spotify-repeat", "spotify-repeat-on"],
    ["spotify-like", "spotify-liked"],
  ])("%s watches %s", (type, key) => {
    const tile = { type, command: "" };
    expect(stateActive(tile, {}, { [key]: "ON" }, {})).toBe(true);
    expect(stateActive(tile, {}, { [key]: "OFF" }, {})).toBe(false);
    expect(stateActive(tile, {}, {}, {})).toBeNull();
    // the tile's own type key is not what it watches (repeat/like)
    if (key !== type) expect(stateActive(tile, {}, { [type]: "ON" }, {})).toBeNull();
  });

  it("leaves empty bindings and unbound types on the tap flip", () => {
    for (const type of ["obs-scene", "vm-toggle-strip", "discord-toggle-mute", "vmod-voice", "key"]) {
      expect(stateActive({ type, command: "" }, {}, { [type]: "ON", anything: "ON" }, {})).toBeNull();
    }
  });
});

describe("boardDim", () => {
  it.each([
    [5, 5],
    ["7", 7],
    [7.9, 7],
    ["7.9", 7],
    [0, 1],
    [-4, 1],
    [32, 32],
    [33, 32],
    [1e9, 32],
  ])("clamps %j to %j", (value, want) => {
    expect(boardDim(value, 4)).toBe(want);
  });

  it("uses the fallback only for non-numbers", () => {
    expect(boardDim("abc", 4)).toBe(4);
    expect(boardDim(undefined, 6)).toBe(6);
    expect(boardDim(NaN, 3)).toBe(3);
    expect(boardDim(Infinity, 3)).toBe(3);
    // Number("") is 0, a number, so it clamps instead
    expect(boardDim("", 3)).toBe(1);
  });
});

describe("clamp", () => {
  it("keeps values inside inclusive bounds", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
    expect(clamp(0, 0, 0)).toBe(0);
    expect(clamp(10, 0, 10)).toBe(10);
  });
});

describe("VM_SLIDER_RESET", () => {
  it("maps 0 dB onto the -60..12 dB fader range", () => {
    expect(VM_SLIDER_RESET).toBeCloseTo(60 / 72, 10);
    expect(-60 + VM_SLIDER_RESET * 72).toBeCloseTo(0, 10);
  });
});

describe("statusProgressAt", () => {
  const playing = { position_ms: 10_000, duration_ms: 60_000, playing: true };

  it("rejects payloads without usable progress", () => {
    expect(statusProgressAt(null, 0, 0)).toBeNull();
    expect(statusProgressAt("x", 0, 0)).toBeNull();
    expect(statusProgressAt({}, 0, 0)).toBeNull();
    expect(statusProgressAt({ position_ms: 1, duration_ms: 0 }, 0, 0)).toBeNull();
    expect(statusProgressAt({ position_ms: 1, duration_ms: -5 }, 0, 0)).toBeNull();
    expect(statusProgressAt({ position_ms: "a", duration_ms: 10 }, 0, 0)).toBeNull();
  });

  it("extrapolates by elapsed wall time while playing", () => {
    expect(statusProgressAt(playing, 1000, 1000)).toBe(10_000);
    expect(statusProgressAt(playing, 1000, 4500)).toBe(13_500);
  });

  it("clamps to the track length", () => {
    expect(statusProgressAt(playing, 0, 1e9)).toBe(60_000);
    expect(statusProgressAt({ ...playing, position_ms: 90_000 }, 0, 0)).toBe(60_000);
    expect(statusProgressAt({ ...playing, position_ms: -5 }, 0, 0)).toBe(0);
  });

  it("never runs backwards when the clock goes back", () => {
    expect(statusProgressAt(playing, 5000, 1000)).toBe(10_000);
  });

  it("freezes while paused or when playing is not literally true", () => {
    for (const p of [false, "true", 1, undefined]) {
      expect(statusProgressAt({ ...playing, playing: p }, 0, 99_999)).toBe(10_000);
    }
  });

  it("accepts numeric strings", () => {
    expect(statusProgressAt({ position_ms: "500", duration_ms: "1000" }, 0, 0)).toBe(500);
  });
});

describe("mmss", () => {
  it.each([
    [0, "0:00"],
    [499, "0:00"],
    [500, "0:01"],
    [9_000, "0:09"],
    [61_000, "1:01"],
    [600_000, "10:00"],
    [3_725_000, "62:05"],
    [-3000, "0:00"],
  ])("formats %d ms as %s", (ms, want) => {
    expect(mmss(ms)).toBe(want);
  });
});

describe("plan windows token", () => {
  it.each([
    ["", true, true],
    [null, true, true],
    [undefined, true, true],
    ["windows:5h,week", true, true],
    ["windows:5h", true, false],
    ["windows:week", false, true],
    ["windows:", false, false],
    ["windows: 5h , week ", true, true],
    ["other:x;windows:week", false, true],
    ["windows:week;other:x", false, true],
    ["windows:not-a-window", false, false],
    ["xwindows:5h", true, true],
  ])("parses %j", (options, five, week) => {
    expect(parsePlanWindows(options)).toEqual({ five, week });
  });

  it.each([
    [{ five: true, week: true }, "windows:5h,week"],
    [{ five: true, week: false }, "windows:5h"],
    [{ five: false, week: true }, "windows:week"],
    [{ five: false, week: false }, "windows:"],
  ])("writes %j", (windows, want) => {
    expect(setPlanWindows("", windows)).toBe(want);
    expect(parsePlanWindows(setPlanWindows("", windows))).toEqual(windows);
  });

  it("replaces an existing token and keeps siblings", () => {
    expect(setPlanWindows("flag:x;windows:5h", { five: false, week: true })).toBe(
      "flag:x;windows:week",
    );
    expect(setPlanWindows("windows:5h;flag:x", { five: true, week: true })).toBe(
      "flag:x;windows:5h,week",
    );
    expect(setPlanWindows("a:1;windows:5h;b:2", { five: true, week: false })).toBe(
      "a:1;b:2;windows:5h",
    );
  });

  it("collapses duplicate tokens into one", () => {
    const out = setPlanWindows("windows:5h;windows:week", { five: true, week: false });
    expect(out).toBe("windows:5h");
  });

  it("round-trips through every combination with siblings", () => {
    for (const five of [true, false]) {
      for (const week of [true, false]) {
        const written = setPlanWindows("other:x", { five, week });
        expect(written).toContain("other:x");
        expect(parsePlanWindows(written)).toEqual({ five, week });
      }
    }
  });
});
