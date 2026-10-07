import { describe, it, expect, vi, afterEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import TileCell from "../src/components/TileCell.vue";
import { VM_SLIDER_RESET } from "../src/catalog.js";
import { handle } from "./tauri.js";

const baseTile = {
  id: 1,
  type: "key",
  mode: "button",
  command: "",
  options: "",
  title: "",
  w: 1,
  h: 1,
};

function tileOf(extra = {}) {
  return { ...baseTile, ...extra };
}

function mountTile(tile, props = {}) {
  return mount(TileCell, { props: { tile: tileOf(tile), ...props }, attachTo: document.body });
}

const tileEl = (w) => w.find(".tile");
const styleOf = (w) => tileEl(w).attributes("style") || "";

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("TileCell styling", () => {
  it("renders nothing for a placeholder tile with a null id", () => {
    const w = mountTile({ id: null });
    expect(tileEl(w).exists()).toBe(false);
  });

  it("uses the tile's own colors and the default shape", () => {
    const w = mountTile({ color: "#112233", border_color: "#445566" });
    const style = styleOf(w);
    expect(style).toMatch(/background: (#112233|rgb\(17, 34, 51\))/);
    expect(style).toMatch(/border-color: (#445566|rgb\(68, 85, 102\))/);
    expect(style).toContain("border-radius: 8px");
  });

  it("falls back to the catalog color, then to the default slate", () => {
    const meta = { key: { color: "#abcdef", icon: "keyboard" } };
    expect(styleOf(mountTile({}, { typeMeta: meta }))).toMatch(/#abcdef|rgb\(171, 205, 239\)/);
    expect(styleOf(mountTile({}))).toMatch(/#2c3e50|rgb\(44, 62, 80\)/);
  });

  it("rounds shape 1 into a circle", () => {
    expect(styleOf(mountTile({ shape: 1 }))).toContain("border-radius: 50%");
  });

  it("draws the catalog icon when the tile has none", () => {
    const w = mountTile({}, { typeMeta: { key: { icon: "keyboard" } } });
    expect(w.find("i.tile-icon").classes()).toContain("fa-keyboard");
  });

  it.each([
    ["play", "fa-play"],
    ["prev", "fa-fast-backward"],
    ["next", "fa-fast-forward"],
    ["vol_up", "fa-volume-up"],
    ["vol_down", "fa-volume-down"],
    ["vol_mute", "fa-volume-off"],
  ])("gives vol %s its own icon", (command, cls) => {
    const w = mountTile({ type: "vol", command, icon: "star" });
    expect(w.find("i.tile-icon").classes()).toContain(cls);
  });

  it("shows the title at its position with its colors", () => {
    const w = mountTile({ title: "Hello", title_position: 2, title_color: "#ff0000" });
    const title = w.find(".tile-title");
    expect(title.text()).toBe("Hello");
    expect(title.classes()).toContain("pos-2");
    expect(title.attributes("style")).toMatch(/#ff0000|rgb\(255, 0, 0\)/);
  });

  it("names untitled board tiles after their target board", () => {
    const w = mountTile(
      { type: "board", command: JSON.stringify({ id: 5 }) },
      { boardNames: { 5: "Streaming" } },
    );
    expect(w.find(".tile-title").text()).toBe("Streaming");
    expect(tileEl(w).attributes("aria-label")).toBe("Streaming");
  });

  it("builds the aria label from title, then catalog label, then type", () => {
    expect(tileEl(mountTile({ title: "T" })).attributes("aria-label")).toBe("T");
    expect(
      tileEl(mountTile({}, { typeMeta: { key: { label: "Keyboard Macro" } } })).attributes(
        "aria-label",
      ),
    ).toBe("Keyboard Macro");
    expect(tileEl(mountTile({ type: "url-to-call" })).attributes("aria-label")).toBe("url to call");
  });

  it("survives a malformed command", () => {
    const w = mountTile({ type: "board", command: "{not json" }, { boardNames: { 1: "x" } });
    expect(w.find(".tile-title").exists()).toBe(false);
  });

  it("shows the image and swaps to img2 in the second state", () => {
    const w = mountTile({ img: "a.png", img2: "b.png" });
    expect(w.find("img.tile-img").attributes("src")).toBe("a.png");
    const active = mountTile({ img: "a.png", img2: "b.png" }, { active: true });
    expect(active.find("img.tile-img").attributes("src")).toBe("b.png");
  });
});

describe("TileCell dual state", () => {
  const dual = {
    type: "obs-scene",
    mode: "toggle",
    color: "#111111",
    color2: "#222222",
    icon: "video",
    icon2: "stop",
    shape: 0,
    shape2: 1,
  };

  it("uses the session flip when no live state exists", () => {
    const off = mountTile(dual);
    expect(styleOf(off)).toMatch(/#111111|rgb\(17, 17, 17\)/);
    expect(tileEl(off).attributes("aria-pressed")).toBe("false");
    const on = mountTile(dual, { active: true });
    expect(styleOf(on)).toMatch(/#222222|rgb\(34, 34, 34\)/);
    expect(styleOf(on)).toContain("border-radius: 50%");
    expect(on.find("i.tile-icon").classes()).toContain("fa-stop");
    expect(tileEl(on).attributes("aria-pressed")).toBe("true");
  });

  it("lets a pushed live value override the session flip", () => {
    const tile = { ...dual, type: "vol", command: "vol_mute" };
    const w = mountTile(tile, { active: false, customValues: { "speaker-muted": "ON" } });
    expect(styleOf(w)).toMatch(/#222222|rgb\(34, 34, 34\)/);
    const off = mountTile(tile, { active: true, customValues: { "speaker-muted": false } });
    expect(styleOf(off)).toMatch(/#111111|rgb\(17, 17, 17\)/);
  });

  it("falls back per field to state-one values when state two is unset", () => {
    const w = mountTile({ type: "obs-scene", color: "#111111", icon: "video" }, { active: true });
    expect(styleOf(w)).toMatch(/#111111|rgb\(17, 17, 17\)/);
    expect(w.find("i.tile-icon").classes()).toContain("fa-video");
  });

  it("only exposes aria-pressed on toggle tiles", () => {
    expect(tileEl(mountTile({ mode: "button" })).attributes("aria-pressed")).toBeUndefined();
  });
});

describe("TileCell live values", () => {
  it("shows a custom-value label instead of the icon", () => {
    const w = mountTile(
      { mode: "custom-value", type: "custom-value", command: "cpu", icon: "tag" },
      { customValues: { cpu: 42 } },
    );
    expect(w.find(".tile-value").text()).toBe("42");
    expect(w.find("i.tile-icon").exists()).toBe(false);
  });

  it("hides the custom-value label behind an explicit title", () => {
    const w = mountTile(
      { mode: "custom-value", type: "custom-value", command: "cpu", title: "CPU" },
      { customValues: { cpu: 42 } },
    );
    expect(w.find(".tile-value").exists()).toBe(false);
  });

  it("ignores object pushes for custom-value labels", () => {
    const w = mountTile(
      { mode: "custom-value", type: "custom-value", command: "cpu" },
      { customValues: { cpu: { values: [1] } } },
    );
    expect(w.find(".tile-value").exists()).toBe(false);
  });

  it("draws a graph with the last sample and suffix", () => {
    const w = mountTile(
      { mode: "graph", type: "ai-tokens-today" },
      { customValues: { "ai-tokens-today": { values: [1, 5, 3], suffix: "k", title: "Tokens" } } },
    );
    expect(w.find(".tile-graph-head b").text()).toBe("Tokens");
    expect(w.find(".tile-graph-val").text()).toBe("3k");
    const line = w.find("polyline").attributes("points").split(" ");
    expect(line).toHaveLength(3);
    // max sample sits at the top (y=5), min at the bottom (y=95)
    expect(line[0]).toBe("0.0,95.0");
    expect(line[1]).toBe("50.0,5.0");
    expect(w.find("polygon").attributes("points")).toMatch(/^0,100 .* 100,100$/);
  });

  it("prefers the producer's value label and centers a single sample", () => {
    const w = mountTile(
      { mode: "graph", type: "g" },
      { customValues: { g: { values: [7], value_label: "7 tok" } } },
    );
    expect(w.find(".tile-graph-val").text()).toBe("7 tok");
    expect(w.find("polyline").attributes("points")).toBe("50.0,95.0");
  });

  it("does not draw a graph for an empty sample list", () => {
    const w = mountTile({ mode: "graph", type: "g" }, { customValues: { g: { values: [] } } });
    expect(w.find(".tile-spark").exists()).toBe(false);
  });

  it("flips the hour tile between sparkline and rows on tap", async () => {
    const value = { values: [1, 2], rows: [{ label: "Claude", value: "3k", provider: "claude" }] };
    const w = mountTile({ mode: "graph", type: "ai-tokens-hour" }, { customValues: { "ai-tokens-hour": value } });
    expect(w.find(".tile-spark").exists()).toBe(true);
    await tileEl(w).trigger("click");
    expect(w.emitted("tap")).toBeUndefined();
    expect(w.find(".tile-hour-rows .status-label").text()).toBe("Claude");
    expect(w.find(".tile-hour-rows svg").exists()).toBe(true);
    await tileEl(w).trigger("click");
    expect(w.find(".tile-spark").exists()).toBe(true);
  });
});

describe("TileCell status tiles", () => {
  const status = (value, tile = {}) =>
    mountTile(
      { mode: "status", type: "ai-plan-limits", w: 2, h: 2, ...tile },
      { customValues: { "ai-plan-limits": value } },
    );

  const planRows = [
    { label: "GLM 5h", value: "40%", state: "ok", percent: 40 },
    { label: "GLM week", value: "80%", state: "warn", percent: 80 },
    { label: "Claude 5h", value: "10%", state: "ok", percent: 10 },
  ];

  it("renders rows with dots, values and clamped bars", () => {
    const w = status({ rows: [...planRows, { label: "X", percent: 150, state: "hot" }], summary: "s" });
    const rows = w.findAll(".status-row");
    expect(rows).toHaveLength(4);
    expect(rows[0].classes()).toContain("s-ok");
    expect(rows[0].find(".status-label").text()).toBe("GLM 5h");
    expect(rows[0].find(".status-bar i").attributes("style")).toContain("width: 40%");
    expect(rows[3].find(".status-bar i").attributes("style")).toContain("width: 100%");
  });

  it("recomputes the summary from the worst visible window", () => {
    expect(status({ rows: planRows, summary: "ignored" }).find(".status-summary").text()).toBe(
      "GLM week 80%",
    );
    const fiveOnly = status({ rows: planRows }, { options: "windows:5h" });
    expect(fiveOnly.findAll(".status-row")).toHaveLength(2);
    expect(fiveOnly.find(".status-summary").text()).toBe("GLM 5h 40%");
  });

  it("keeps the producer summary without percent rows and honors hide_summary", () => {
    const rows = [{ label: "agent", value: "busy", state: "working" }];
    expect(status({ rows, summary: "1 busy" }).find(".status-summary").text()).toBe("1 busy");
    expect(status({ rows: planRows, hide_summary: true }).find(".status-summary").exists()).toBe(
      false,
    );
  });

  it("does not draw a dot on header rows", () => {
    const w = status({ rows: [{ label: "Plans", state: "header" }, planRows[0]] });
    const header = w.findAll(".status-row")[0];
    expect(header.classes()).toContain("is-header");
    expect(header.find(".status-dot").exists()).toBe(false);
  });

  it("swaps labels for provider marks in logo style", () => {
    const w = status({ rows: [planRows[0], { label: "Mystery 5h", percent: 5 }], row_style: "logo" });
    const rows = w.findAll(".status-row");
    expect(rows[0].find(".provider-glyph svg").exists()).toBe(true);
    expect(rows[0].find(".status-label").exists()).toBe(false);
    // no mark known: the name stays
    expect(rows[1].find(".status-label").text()).toBe("Mystery 5h");
  });

  it("switches 1x1 plan tiles to the mini usage view", () => {
    const w = status({ rows: planRows }, { w: 1, h: 1 });
    // h=1 with no compact list: mini view
    const mini = w.findAll(".mini-usage");
    expect(mini).toHaveLength(3);
    expect(mini[1].find(".mini-val").text()).toBe("80%");
    expect(mini[0].find(".mini-glyph svg").exists()).toBe(true);
  });

  it("uses the compact provider stack when the rows do not fit", () => {
    const compact = [{ provider: "claude", count: 2, state: "working" }];
    const small = status({ rows: planRows, compact }, { w: 2, h: 1 });
    expect(small.findAll(".compact-provider")).toHaveLength(1);
    expect(small.find(".compact-count").text()).toBe("2");
    const big = status({ rows: planRows, compact }, { w: 2, h: 2 });
    expect(big.findAll(".compact-provider")).toHaveLength(0);
    const many = Array.from({ length: 9 }, (_, i) => ({ label: `r${i}`, state: "ok" }));
    expect(status({ rows: many, compact }, { w: 2, h: 2 }).findAll(".compact-provider")).toHaveLength(1);
  });

  it("falls back to the icon when the payload has no rows", () => {
    const w = status({ rows: [] }, { icon: "robot" });
    expect(w.find(".tile-status").exists()).toBe(false);
    expect(w.find("i.tile-icon").classes()).toContain("fa-robot");
  });
});

describe("TileCell media card", () => {
  const nowPlaying = (value, tile = {}) =>
    mountTile(
      { mode: "status", type: "spotify-now-playing", w: 2, h: 2, ...tile },
      { customValues: { "spotify-now-playing": value } },
    );
  const rows = [
    { label: "Track", value: "Song" },
    { label: "Artist", value: "Band" },
    { label: "Album", value: "LP" },
    { label: "Device", value: "PC" },
  ];

  it("renders art, title, artist, meta rows and progress", async () => {
    handle("asset_data_url", () => "data:image/png;base64,AAA");
    const w = nowPlaying({
      rows,
      image: "hash1",
      progress: { position_ms: 61_000, duration_ms: 180_000, playing: false },
    });
    await flushPromises();
    expect(w.find("img.status-art").attributes("src")).toBe("data:image/png;base64,AAA");
    expect(w.find(".media-title").text()).toBe("Song");
    const subs = w.findAll(".media-sub").map((s) => s.text());
    expect(subs).toEqual(["Band", "LP", "Device: PC"]);
    expect(w.find(".status-progress-time").text()).toBe("1:01 / 3:00");
    expect(w.find(".status-corner-glyph svg").exists()).toBe(true);
    expect(w.find(".tile-status").classes()).toContain("has-art");
  });

  it("drops album and extras on a one-cell-high tile", async () => {
    const w = nowPlaying(
      { rows, progress: { position_ms: 0, duration_ms: 1000, playing: false } },
      { w: 2, h: 1 },
    );
    await flushPromises();
    expect(w.findAll(".media-sub").map((s) => s.text())).toEqual(["Band"]);
    expect(w.find(".tile-status").classes()).not.toContain("has-art");
  });

  it("hides the m:ss label on a one-cell-wide tile", async () => {
    const w = nowPlaying(
      { rows, progress: { position_ms: 0, duration_ms: 1000, playing: false } },
      { w: 1, h: 2 },
    );
    await flushPromises();
    expect(w.find(".status-progress").exists()).toBe(true);
    expect(w.find(".status-progress-time").exists()).toBe(false);
  });

  it("puts wide art beside the text", async () => {
    handle("asset_data_url", () => "data:x");
    const w = nowPlaying({ rows, image: "h" }, { w: 3, h: 2 });
    await flushPromises();
    expect(w.find(".tile-status").classes()).toContain("art-beside");
  });

  it("renders text-only when the art lookup fails", async () => {
    handle("asset_data_url", () => {
      throw new Error("gone");
    });
    const w = nowPlaying({ rows, image: "missing" });
    await flushPromises();
    expect(w.find("img.status-art").exists()).toBe(false);
    // no art and no progress: plain status rows
    expect(w.find(".media-title").exists()).toBe(false);
    expect(w.findAll(".status-row")).toHaveLength(4);
  });

  it("advances the playing bar once a second", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(100_000);
    const w = nowPlaying({
      rows,
      progress: { position_ms: 0, duration_ms: 100_000, playing: true },
    });
    await flushPromises();
    expect(w.find(".status-progress-time").text()).toBe("0:00 / 1:40");
    await vi.advanceTimersByTimeAsync(5000);
    expect(w.find(".status-progress-time").text()).toBe("0:05 / 1:40");
    expect(w.find(".status-progress-track i").attributes("style")).toContain("width: 5%");
    w.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("starts no ticker while paused", async () => {
    vi.useFakeTimers();
    const w = nowPlaying({ rows, progress: { position_ms: 0, duration_ms: 1000, playing: false } });
    await flushPromises();
    expect(vi.getTimerCount()).toBe(0);
    w.unmount();
  });
});

describe("TileCell interaction", () => {
  it("emits open on double click only in edit mode", async () => {
    const edit = mountTile({});
    await tileEl(edit).trigger("dblclick");
    expect(edit.emitted("open")).toHaveLength(1);
    const touch = mountTile({}, { touch: true });
    await tileEl(touch).trigger("dblclick");
    expect(touch.emitted("open")).toBeUndefined();
  });

  it("forwards tap, context menu and pointerdown", async () => {
    const w = mountTile({});
    await tileEl(w).trigger("click");
    await tileEl(w).trigger("contextmenu");
    await tileEl(w).trigger("pointerdown");
    expect(w.emitted("tap")).toHaveLength(1);
    expect(w.emitted("ctx")).toHaveLength(1);
    expect(w.emitted("down")).toHaveLength(1);
  });

  it("shows the resize handle only in edit mode", async () => {
    const w = mountTile({});
    await w.find(".resize-handle").trigger("pointerdown");
    expect(w.emitted("resize")).toHaveLength(1);
    expect(w.emitted("down")).toBeUndefined();
    expect(mountTile({}, { touch: true }).find(".resize-handle").exists()).toBe(false);
  });

  it("activates with Enter/Space: tap in touch mode, editor in edit mode", async () => {
    const touch = mountTile({}, { touch: true });
    await tileEl(touch).trigger("keydown", { key: "Enter" });
    await tileEl(touch).trigger("keydown", { key: " " });
    expect(touch.emitted("tap")).toHaveLength(2);
    const edit = mountTile({});
    await tileEl(edit).trigger("keydown", { key: "Enter" });
    expect(edit.emitted("open")).toHaveLength(1);
  });

  it("ignores auto-repeat and modified keys", async () => {
    const w = mountTile({}, { touch: true });
    await tileEl(w).trigger("keydown", { key: "Enter", repeat: true });
    await tileEl(w).trigger("keydown", { key: "Enter", ctrlKey: true });
    await tileEl(w).trigger("keydown", { key: " ", shiftKey: true });
    expect(w.emitted("tap")).toBeUndefined();
  });
});

describe("TileCell sliders", () => {
  const slider = (extra = {}, props = {}) =>
    mountTile({ mode: "slider", type: "speaker-volume", ...extra }, { touch: true, ...props });

  it("badges sliders in edit mode and draws the fader in touch mode", () => {
    expect(mountTile({ mode: "slider" }).find(".tile-badge").exists()).toBe(true);
    const w = slider();
    expect(w.find(".tile-badge").exists()).toBe(false);
    expect(w.find(".slider-fill").exists()).toBe(true);
  });

  it("starts the volume fader at the live speaker volume", () => {
    const w = slider({}, { customValues: { "speaker-volume": "0.3" } });
    expect(w.find(".slider-fill").attributes("style")).toContain("height: 30%");
    expect(slider({}, { customValues: { "speaker-volume": 4 } }).find(".slider-fill").attributes("style")).toContain(
      "height: 100%",
    );
    expect(slider({}, { customValues: { "speaker-volume": "x" } }).find(".slider-fill").attributes("style")).toContain(
      "height: 50%",
    );
  });

  it("steps the fader with arrow keys and clamps", async () => {
    const w = slider({}, { customValues: { "speaker-volume": 0.98 } });
    await tileEl(w).trigger("keydown", { key: "ArrowUp" });
    await tileEl(w).trigger("keydown", { key: "ArrowLeft" });
    const sent = w.emitted("slider").map(([v]) => v);
    expect(sent[0]).toBe(1);
    expect(sent[1]).toBeCloseTo(0.95, 10);
  });

  it("does not step a non-slider tile or in edit mode", async () => {
    const button = mountTile({}, { touch: true });
    await tileEl(button).trigger("keydown", { key: "ArrowUp" });
    expect(button.emitted("slider")).toBeUndefined();
    const edit = mountTile({ mode: "slider" });
    await tileEl(edit).trigger("keydown", { key: "ArrowUp" });
    expect(edit.emitted("slider")).toBeUndefined();
  });

  function stubRect(w) {
    const capture = w.find(".slider-capture").element;
    capture.getBoundingClientRect = () => ({ top: 0, height: 100, left: 0, width: 100 });
    capture.setPointerCapture = () => {};
    return w.find(".slider-capture");
  }

  it("maps a drag onto 0..1 and always sends the final value on release", async () => {
    const w = slider();
    const capture = stubRect(w);
    await capture.trigger("pointerdown", { clientY: 25, pointerId: 1 });
    expect(w.emitted("slider")[0][0]).toBeCloseTo(0.75, 10);
    window.dispatchEvent(new MouseEvent("pointermove", { clientY: 90 }));
    window.dispatchEvent(new MouseEvent("pointerup", { clientY: 120 }));
    const sent = w.emitted("slider").map(([v]) => v);
    // the throttled move may or may not go out; the release always
    // re-sends the last dragged position (the up event itself does not move it)
    expect(sent.at(-1)).toBeCloseTo(0.1, 10);
    // listeners are gone after release
    window.dispatchEvent(new MouseEvent("pointermove", { clientY: 50 }));
    expect(w.emitted("slider")).toHaveLength(sent.length);
  });

  it("resets a Voicemeeter slider to 0 dB on double tap", async () => {
    const w = slider({ type: "vm-slider-bus" });
    const capture = stubRect(w);
    await capture.trigger("pointerdown", { clientY: 10, pointerId: 1 });
    window.dispatchEvent(new MouseEvent("pointerup", { clientY: 10 }));
    await capture.trigger("pointerdown", { clientY: 10, pointerId: 1 });
    window.dispatchEvent(new MouseEvent("pointerup", { clientY: 10 }));
    const sent = w.emitted("slider").map(([v]) => v);
    expect(sent[0]).toBeCloseTo(0.9, 10);
    expect(sent[2]).toBeCloseTo(VM_SLIDER_RESET, 10);
  });

  it("does not reset non-Voicemeeter sliders on double tap", async () => {
    const w = slider();
    const capture = stubRect(w);
    for (let i = 0; i < 2; i++) {
      await capture.trigger("pointerdown", { clientY: 10, pointerId: 1 });
      window.dispatchEvent(new MouseEvent("pointerup", { clientY: 10 }));
    }
    const sent = w.emitted("slider").map(([v]) => v);
    expect(sent.every((v) => Math.abs(v - 0.9) < 1e-9)).toBe(true);
  });
});
