import { describe, it, expect, afterEach } from "vitest";
import { mount } from "@vue/test-utils";
import GridEditor from "../src/components/GridEditor.vue";

const tile = (id, x, y, extra = {}) => ({
  id,
  x,
  y,
  w: 1,
  h: 1,
  type: "key",
  mode: "button",
  command: "",
  title: `t${id}`,
  ...extra,
});

function boardOf(buttons, extra = {}) {
  return { id: 1, name: "B", width: 4, height: 3, buttons, ...extra };
}

function mountGrid(buttons, props = {}, board = {}) {
  return mount(GridEditor, {
    props: { board: boardOf(buttons, board), ...props },
    attachTo: document.body,
  });
}

const slot = (w, id) => w.find(`[data-tile-id="${id}"]`);
const tileOf = (w, id) => slot(w, id).find(".tile");

// happy-dom has no layout: pin the grid's rect so pointer math is exact
function pinGridRect(w) {
  w.find(".board-grid").element.getBoundingClientRect = () => ({ left: 0, top: 0, width: 384, height: 300 });
}

function pointer(type, init) {
  window.dispatchEvent(new MouseEvent(type, init));
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("GridEditor layout", () => {
  it("sizes the grid by board dimensions and zoom", () => {
    const style = mountGrid([], { zoom: 1.5 }).find(".board-grid").attributes("style");
    expect(style).toContain("width: 576px");
    expect(style).toContain("height: 450px");
  });

  it("uses the board image as background", () => {
    const style = mountGrid([], {}, { image: "bg.png" }).find(".board-grid").attributes("style");
    expect(style).toContain("bg.png");
  });

  it("places tiles on the cell grid", () => {
    const w = mountGrid([tile(1, 2, 1, { w: 2 })]);
    const style = slot(w, 1).attributes("style");
    expect(style).toContain("left: 192px");
    expect(style).toContain("top: 100px");
    expect(style).toContain("width: 192px");
    expect(style).toContain("height: 100px");
  });

  it("pulls tiles that overhang the grid back inside", () => {
    const w = mountGrid([tile(1, 9, 9, { w: 2, h: 2 })]);
    const style = slot(w, 1).attributes("style");
    expect(style).toContain("left: 192px");
    expect(style).toContain("top: 100px");
  });

  it("renders an empty slot for every free cell in edit mode", () => {
    const w = mountGrid([tile(1, 0, 0, { w: 2, h: 2 })]);
    expect(w.findAll(".empty-cell")).toHaveLength(12 - 4);
  });

  it("renders no empty slots in touch mode", () => {
    expect(mountGrid([], { touch: true }).findAll(".empty-cell")).toHaveLength(0);
  });

  it.each([
    [{ width: 33, height: 2 }],
    [{ width: 2.5, height: 2 }],
    [{ width: 0, height: 2 }],
    [{ width: "x", height: 2 }],
  ])("skips empty slots for a junk board %j", (dims) => {
    expect(mountGrid([], {}, dims).findAll(".empty-cell")).toHaveLength(0);
  });

  it("bounds junk tile sizes when marking occupied cells", () => {
    const w = mountGrid([tile(1, 3, 2, { w: 1e9, h: 1e9 })]);
    // only (3,2) is inside the grid
    expect(w.findAll(".empty-cell")).toHaveLength(11);
  });
});

describe("GridEditor edit-mode interaction", () => {
  it("adds a tile from an empty slot", async () => {
    const w = mountGrid([tile(1, 0, 0)]);
    await w.findAll(".tile-slot").find((s) => !s.attributes("data-tile-id")).trigger("click");
    expect(w.emitted("tile-add")[0][0]).toEqual({ x: 1, y: 0 });
  });

  it("adds a tile at the clicked grid position", async () => {
    const w = mountGrid([tile(1, 0, 0)]);
    pinGridRect(w);
    await w.find(".board-grid").trigger("click", { clientX: 96 * 2 + 5, clientY: 100 * 1 + 5 });
    await w.find(".board-grid").trigger("click", { clientX: 5, clientY: 5 });
    expect(w.emitted("tile-add")).toEqual([[{ x: 2, y: 1 }]]);
  });

  it("opens the empty-cell context menu from the grid background", async () => {
    const w = mountGrid([tile(1, 0, 0)]);
    pinGridRect(w);
    await w.find(".board-grid").trigger("contextmenu", { clientX: 5, clientY: 5 });
    expect(w.emitted("ctx-empty")).toBeUndefined();
    await w.find(".board-grid").trigger("contextmenu", { clientX: 9999, clientY: 9999 });
    expect(w.emitted("ctx-empty")[0][0]).toEqual({ x: 3, y: 2 });
  });

  it("opens the tile context menu and the editor on double click", async () => {
    const t = tile(1, 0, 0);
    const w = mountGrid([t]);
    await tileOf(w, 1).trigger("contextmenu");
    await tileOf(w, 1).trigger("dblclick");
    expect(w.emitted("ctx-tile")[0][0]).toStrictEqual(t);
    expect(w.emitted("tile-open")[0][0]).toStrictEqual(t);
  });

  it("does not execute tiles on click in edit mode", async () => {
    const w = mountGrid([tile(1, 0, 0)]);
    await tileOf(w, 1).trigger("click");
    expect(w.emitted("tile-exec")).toBeUndefined();
  });

  it("moves a tile by whole cells and clamps to the board", async () => {
    const t = tile(1, 0, 0, { w: 2 });
    const w = mountGrid([t]);
    await tileOf(w, 1).trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 96 * 5, clientY: 100 * 1 + 20 });
    await w.vm.$nextTick();
    expect(slot(w, 1).attributes("style")).toContain("z-index: 10");
    expect(tileOf(w, 1).classes()).toContain("dragging");
    pointer("pointerup", {});
    await w.vm.$nextTick();
    expect(w.emitted("tile-moved")[0].slice(1)).toEqual([2, 1, 2, 1]);
    expect(tileOf(w, 1).classes()).not.toContain("dragging");
  });

  it("does not emit a move that lands on the same cell", async () => {
    const w = mountGrid([tile(1, 1, 1)]);
    await tileOf(w, 1).trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 20, clientY: 20 });
    pointer("pointerup", {});
    expect(w.emitted("tile-moved")).toBeUndefined();
  });

  it("ignores right-button drags", async () => {
    const w = mountGrid([tile(1, 0, 0)]);
    await tileOf(w, 1).trigger("pointerdown", { button: 2, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 300, clientY: 0 });
    pointer("pointerup", {});
    expect(w.emitted("tile-moved")).toBeUndefined();
  });

  it("resizes from the handle and keeps at least one cell", async () => {
    const w = mountGrid([tile(1, 1, 1), tile(2, 0, 0)]);
    await slot(w, 1).find(".resize-handle").trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 96 * 10, clientY: 100 });
    pointer("pointerup", {});
    // width clamps to board.width - x = 3, height grows by one
    expect(w.emitted("tile-moved")[0].slice(1)).toEqual([1, 1, 3, 2]);
    await slot(w, 2).find(".resize-handle").trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: -500, clientY: -500 });
    pointer("pointerup", {});
    // already 1x1: shrinking further changes nothing
    expect(w.emitted("tile-moved")).toHaveLength(1);
  });

  it("never persists a non-positive size for a tile outside a shrunken grid", async () => {
    const w = mountGrid([tile(1, 6, 0)]);
    await slot(w, 1).find(".resize-handle").trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 0, clientY: 100 });
    pointer("pointerup", {});
    const [, , , width, height] = w.emitted("tile-moved")[0];
    expect(width).toBeGreaterThanOrEqual(1);
    expect(height).toBe(2);
  });

  it("moves keyboard focus to the nearest tile in the arrow direction", async () => {
    const w = mountGrid([tile(1, 0, 0), tile(2, 2, 0), tile(3, 1, 1), tile(4, 0, 2)]);
    tileOf(w, 1).element.focus();
    await tileOf(w, 1).trigger("keydown", { key: "ArrowRight" });
    expect(document.activeElement).toBe(tileOf(w, 2).element);
    await tileOf(w, 2).trigger("keydown", { key: "ArrowDown" });
    expect(document.activeElement).toBe(tileOf(w, 3).element);
    await tileOf(w, 3).trigger("keydown", { key: "ArrowLeft" });
    // (0,2) and (0,0) are equally diagonal; straight-ahead score ties go to the first
    expect([tileOf(w, 1).element, tileOf(w, 4).element]).toContain(document.activeElement);
  });

  it("keeps focus when no tile lies in the arrow direction or a modifier is held", async () => {
    const w = mountGrid([tile(1, 0, 0), tile(2, 1, 0)]);
    tileOf(w, 1).element.focus();
    await tileOf(w, 1).trigger("keydown", { key: "ArrowLeft" });
    await tileOf(w, 1).trigger("keydown", { key: "ArrowUp" });
    await tileOf(w, 1).trigger("keydown", { key: "ArrowRight", shiftKey: true });
    expect(document.activeElement).toBe(tileOf(w, 1).element);
  });
});

describe("GridEditor touch mode", () => {
  it("executes button tiles on tap", async () => {
    const t = tile(1, 0, 0);
    const w = mountGrid([t], { touch: true });
    await tileOf(w, 1).trigger("click");
    expect(w.emitted("tile-exec")[0][0]).toStrictEqual(t);
  });

  it("flips toggle tiles locally and still executes them", async () => {
    const t = tile(1, 0, 0, { mode: "toggle", color: "#111111", color2: "#222222" });
    const w = mountGrid([t], { touch: true });
    expect(tileOf(w, 1).attributes("aria-pressed")).toBe("false");
    await tileOf(w, 1).trigger("click");
    expect(tileOf(w, 1).attributes("aria-pressed")).toBe("true");
    await tileOf(w, 1).trigger("click");
    expect(tileOf(w, 1).attributes("aria-pressed")).toBe("false");
    expect(w.emitted("tile-exec")).toHaveLength(2);
  });

  it("does not execute sliders on tap and forwards slider values", async () => {
    const t = tile(1, 0, 0, { mode: "slider" });
    const w = mountGrid([t], { touch: true });
    await tileOf(w, 1).trigger("click");
    expect(w.emitted("tile-exec")).toBeUndefined();
    await tileOf(w, 1).trigger("keydown", { key: "ArrowUp" });
    expect(w.emitted("tile-slider")[0][0]).toStrictEqual(t);
    expect(w.emitted("tile-slider")[0][1]).toBeCloseTo(0.55, 10);
  });

  it("opens settings on long-press/right-click instead of the menu", async () => {
    const w = mountGrid([tile(1, 0, 0)], { touch: true });
    await tileOf(w, 1).trigger("contextmenu");
    expect(w.emitted("tile-open")).toHaveLength(1);
    expect(w.emitted("ctx-tile")).toBeUndefined();
  });

  it("never drags, adds or opens the empty-cell menu", async () => {
    const w = mountGrid([tile(1, 0, 0)], { touch: true });
    pinGridRect(w);
    await tileOf(w, 1).trigger("pointerdown", { button: 0, clientX: 0, clientY: 0 });
    pointer("pointermove", { clientX: 300, clientY: 300 });
    pointer("pointerup", {});
    await w.find(".board-grid").trigger("click", { clientX: 300, clientY: 5 });
    await w.find(".board-grid").trigger("contextmenu", { clientX: 300, clientY: 5 });
    expect(w.emitted("tile-moved")).toBeUndefined();
    expect(w.emitted("tile-add")).toBeUndefined();
    expect(w.emitted("ctx-empty")).toBeUndefined();
  });
});
