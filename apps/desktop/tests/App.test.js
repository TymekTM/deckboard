import { describe, it, expect, afterEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import App from "../src/App.vue";
import GridEditor from "../src/components/GridEditor.vue";
import { calls, handle, emit, dialog, listenerCount } from "./tauri.js";

const tile = (id, boardId, x, y, extra = {}) => ({
  id,
  board_id: boardId,
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

function seed({ boards, dbOk = true, pendingToggle = false } = {}) {
  let list = boards ?? [
    { id: 1, name: "Main", width: 4, height: 3, background: "#101010", buttons: [tile(11, 1, 0, 0)] },
    { id: 2, name: "Stream", width: 2, height: 2, background: "#202020", buttons: [] },
  ];
  handle("server_status", () => ({ dbOk, port: 8500, clients: 2, version: "1.4.0" }));
  handle("list_known_inputs", () => [{ value: "ext-x", label: "Ext X", icon: "bolt", source: "extension" }]);
  handle("list_audio_devices", () => [{ id: "{a}", name: "Speakers" }]);
  handle("list_boards", () => list);
  handle("take_pending_touch_toggle", () => pendingToggle);
  handle("spotify_devices", () => []);
  handle("spotify_playlists", () => []);
  return { setBoards: (b) => (list = b) };
}

async function mountApp() {
  const w = mount(App, { attachTo: document.body });
  await flushPromises();
  return w;
}

const cmds = (name) => calls.filter((c) => c.cmd === name);

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("App startup", () => {
  it("loads status, inputs, devices and boards and selects the first board", async () => {
    seed();
    const w = await mountApp();
    expect(w.vm.currentId).toBe(1);
    expect(w.find(".board-title").text()).toBe("Main");
    expect(w.findAll(".board-entry").map((b) => b.text())).toEqual(["Main", "Stream"]);
    expect(w.find(".canvas-overlay.version").text()).toContain("Version 1.4.0");
    expect(listenerCount("toggle-touch-mode")).toBe(1);
    expect(listenerCount("change-board")).toBe(1);
    expect(listenerCount("app-status-update")).toBe(1);
  });

  it("explains a locked database and never lists boards", async () => {
    seed({ dbOk: false });
    const w = await mountApp();
    expect(w.find(".banner").exists()).toBe(true);
    expect(cmds("list_boards")).toHaveLength(0);
    expect(w.find(".rail-alert").exists()).toBe(true);
  });

  it("stays responsive to events when the initial load fails", async () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    handle("server_status", () => {
      throw "ipc down";
    });
    const w = await mountApp();
    expect(err).toHaveBeenCalled();
    expect(listenerCount("toggle-touch-mode")).toBe(1);
    emit("toggle-touch-mode");
    expect(w.vm.touchMode).toBe(true);
  });

  it("applies a touch toggle that arrived before mount", async () => {
    seed({ pendingToggle: true });
    const w = await mountApp();
    expect(w.vm.touchMode).toBe(true);
    expect(w.vm.touchBoardId).toBe(1);
  });

  it("merges extension inputs into the type metadata", async () => {
    seed();
    const w = await mountApp();
    expect(w.vm.typeMeta["ext-x"]).toEqual({ label: "Ext X", icon: "bolt", color: "", mode: "" });
    expect(w.vm.typeMeta.url.icon).toBe("link");
  });

  it("drops its listeners on unmount", async () => {
    seed();
    const w = await mountApp();
    w.unmount();
    expect(listenerCount("app-status-update")).toBe(0);
  });
});

describe("App live state", () => {
  async function pushed(w, app, data) {
    emit("app-status-update", { app, data });
    await flushPromises();
    return w.vm.customValues;
  }

  it("stores scalar custom values and device ids", async () => {
    seed();
    const w = await mountApp();
    await pushed(w, "APP_CUSTOM_VALUE", { cpu: 42, "speaker-muted": true });
    await pushed(w, "THIRD_PARTY_APP", { "speaker-device": "{a}" });
    expect(w.vm.customValues).toMatchObject({ cpu: 42, "speaker-muted": true, "speaker-device": "{a}" });
  });

  it("ignores unknown apps and malformed payloads", async () => {
    seed();
    const w = await mountApp();
    await pushed(w, "APP_OBS", { scene: "x" });
    emit("app-status-update", { app: "APP_CUSTOM_VALUE", data: "nope" });
    emit("app-status-update", null);
    await flushPromises();
    expect(w.vm.customValues).toEqual({});
  });

  it("keeps the last ten graph samples", async () => {
    seed();
    const w = await mountApp();
    for (let i = 1; i <= 12; i++) {
      await pushed(w, "APP_CUSTOM_VALUE", { g: { value: i, suffix: "%" } });
    }
    expect(w.vm.customValues.g.values).toEqual([3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    expect(w.vm.customValues.g.suffix).toBe("%");
  });

  it("keeps graph identity while only samples change", async () => {
    seed();
    const w = await mountApp();
    await pushed(w, "APP_CUSTOM_VALUE", { g: { value: 1, title: "T" } });
    const first = w.vm.customValues.g;
    await pushed(w, "APP_CUSTOM_VALUE", { g: { value: 1, title: "T" } });
    expect(w.vm.customValues.g).toBe(first);
    expect(first.values).toEqual([1, 1]);
    // a meta change replaces the entry but keeps the history
    await pushed(w, "APP_CUSTOM_VALUE", { g: { value: 2, title: "New" } });
    expect(w.vm.customValues.g).not.toBe(first);
    expect(w.vm.customValues.g.values).toEqual([1, 1, 2]);
  });

  it("replaces status snapshots only when they change", async () => {
    seed();
    const w = await mountApp();
    await pushed(w, "APP_CUSTOM_VALUE", { s: { rows: [{ label: "a" }] } });
    const first = w.vm.customValues.s;
    await pushed(w, "APP_CUSTOM_VALUE", { s: { rows: [{ label: "a" }] } });
    expect(w.vm.customValues.s).toBe(first);
    await pushed(w, "APP_CUSTOM_VALUE", { s: { rows: [{ label: "b" }] } });
    expect(w.vm.customValues.s.rows[0].label).toBe("b");
  });

  it("buffers pushes while hidden and applies them on show", async () => {
    seed();
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    const w = await mountApp();
    document.dispatchEvent(new Event("visibilitychange"));
    await pushed(w, "APP_CUSTOM_VALUE", { cpu: 1 });
    await pushed(w, "APP_CUSTOM_VALUE", { cpu: 2, ram: 3 });
    expect(w.vm.customValues).toEqual({});
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));
    await flushPromises();
    expect(w.vm.customValues).toEqual({ cpu: 2, ram: 3 });
  });
});

describe("App boards and tiles", () => {
  it("switches boards from the sidebar", async () => {
    seed();
    const w = await mountApp();
    await w.findAll(".board-entry")[1].trigger("click");
    expect(w.find(".board-title").text()).toBe("Stream");
    expect(w.find(".canvas-hint").exists()).toBe(true);
  });

  it("follows change-board events into the editor outside touch mode", async () => {
    seed();
    const w = await mountApp();
    emit("change-board", 2);
    await flushPromises();
    expect(w.vm.currentId).toBe(2);
    expect(w.vm.touchBoardId).toBe(2);
  });

  it("only moves the touch board while in touch mode", async () => {
    seed();
    const w = await mountApp();
    emit("toggle-touch-mode");
    emit("change-board", 2);
    await flushPromises();
    expect(w.vm.currentId).toBe(1);
    expect(w.vm.touchBoardId).toBe(2);
    expect(w.findComponent(GridEditor).props("board").name).toBe("Stream");
  });

  it("runs tiles on tap in touch mode and leaves via the exit button", async () => {
    seed();
    const w = await mountApp();
    await w.find(".rail-btn[title='Touch mode']").trigger("click");
    expect(w.find(".rail").exists()).toBe(false);
    await w.find('[data-tile-id="11"] .tile').trigger("click");
    await flushPromises();
    expect(cmds("exec_button")).toEqual([{ cmd: "exec_button", args: { id: 11 } }]);
    await w.find(".touch-exit").trigger("click");
    expect(w.vm.touchMode).toBe(false);
  });

  it("flashes a failed tile run", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    seed();
    handle("exec_button", () => {
      throw "extension crashed";
    });
    const w = await mountApp();
    w.vm.runTileNow(11);
    await flushPromises();
    expect(w.find(".flash").text()).toBe("Running the tile failed: extension crashed");
  });

  it("forwards slider values", async () => {
    seed();
    const w = await mountApp();
    w.vm.tileSlider({ id: 11 }, 0.4);
    await flushPromises();
    expect(cmds("exec_slider")[0].args).toEqual({ id: 11, value: 0.4 });
  });

  it("persists a move and applies it locally", async () => {
    seed();
    const w = await mountApp();
    const t = w.vm.boards[0].buttons[0];
    await w.vm.tileMoved(t, 2, 1, 2, 1);
    expect(cmds("move_button")[0].args).toEqual({ id: 11, boardId: 1, x: 2, y: 1, w: 2, h: 1 });
    expect(t).toMatchObject({ x: 2, y: 1, w: 2, h: 1 });
  });

  it("reloads boards when a move is refused", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    seed();
    handle("move_button", () => {
      throw "overlap";
    });
    const w = await mountApp();
    const t = w.vm.boards[0].buttons[0];
    await w.vm.tileMoved(t, 2, 1, 1, 1);
    expect(t.x).toBe(0);
    expect(cmds("list_boards")).toHaveLength(2);
    expect(w.vm.flash).toContain("Przesuwanie kafla nie powiodło się");
  });

  it("creates a tile in two steps and reopens it for editing", async () => {
    seed();
    let created = false;
    handle("create_button", () => {
      created = true;
      return 99;
    });
    handle("list_boards", () =>
      created
        ? [{ id: 1, name: "Main", width: 4, height: 3, buttons: [tile(99, 1, 1, 1)] }]
        : [{ id: 1, name: "Main", width: 4, height: 3, buttons: [] }],
    );
    const w = await mountApp();
    w.vm.createFlow = { x: 1, y: 1, boardId: 1 };
    await w.vm.tileCreated({ board_id: 1, type: "url", mode: "", x: 1, y: 1, command: "https://x", w: 0 });
    expect(cmds("create_button")[0].args).toEqual({ boardId: 1, kind: "url", mode: "button", x: 1, y: 1 });
    expect(cmds("update_button")[0].args.button).toMatchObject({ id: 99, board_id: 1, command: "https://x", w: 1, h: 1 });
    expect(w.vm.createFlow).toBeNull();
    expect(w.vm.editingTile.id).toBe(99);
  });

  it("keeps the create dialog open when creation fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    seed();
    handle("create_button", () => {
      throw "board full";
    });
    const w = await mountApp();
    w.vm.createFlow = { x: 1, y: 1, boardId: 1 };
    await w.vm.tileCreated({ board_id: 1, type: "key", x: 1, y: 1 });
    expect(w.vm.createFlow).not.toBeNull();
    expect(w.vm.flash).toBe("Creating the tile failed: board full");
  });

  it("deletes a tile only after confirmation", async () => {
    seed();
    dialog.ask.mockResolvedValueOnce(false);
    const w = await mountApp();
    const t = w.vm.boards[0].buttons[0];
    await w.vm.tileDeleted(t);
    expect(cmds("delete_button")).toHaveLength(0);
    await w.vm.tileDeleted(t);
    expect(dialog.ask).toHaveBeenLastCalledWith('Delete tile "t11"?', {
      title: "Delete tile",
      okLabel: "Delete",
      kind: "warning",
    });
    expect(cmds("delete_button")[0].args).toEqual({ id: 11, boardId: 1 });
  });

  it("saves an edited tile and closes the dialog", async () => {
    seed();
    const w = await mountApp();
    w.vm.editingTile = w.vm.boards[0].buttons[0];
    await w.vm.tileEdited({ id: 11, title: "x" });
    expect(cmds("update_button")[0].args).toEqual({ button: { id: 11, title: "x" } });
    expect(w.vm.editingTile).toBeNull();
  });

  it("fetches Spotify pickers when a tile dialog opens", async () => {
    seed();
    handle("spotify_devices", () => [{ name: "Desk" }]);
    handle("spotify_playlists", () => {
      throw "not logged in";
    });
    const w = await mountApp();
    w.vm.createFlow = { x: 0, y: 1, boardId: 1 };
    await flushPromises();
    expect(w.vm.spotifyDevices).toEqual([{ name: "Desk" }]);
    expect(w.vm.spotifyPlaylists).toEqual([]);
  });

  it("falls back to the current board when the touch board disappears", async () => {
    const s = seed();
    const w = await mountApp();
    emit("change-board", 2);
    s.setBoards([{ id: 1, name: "Main", width: 4, height: 3, buttons: [] }]);
    await w.vm.loadBoards();
    expect(w.vm.currentId).toBe(1);
    expect(w.vm.touchBoardId).toBe(1);
  });
});

describe("App clipboard and context menus", () => {
  it("copies a tile and pastes it clamped onto the current board", async () => {
    seed();
    handle("create_button", () => 50);
    const w = await mountApp();
    const source = { ...w.vm.boards[0].buttons[0], w: 2, h: 2, color: "#abcdef" };
    w.vm.copyTile(source);
    source.color = "#000000";
    await w.vm.pasteTile(w.vm.tileClipboard, { x: 3, y: 2 });
    expect(cmds("create_button")[0].args).toEqual({ boardId: 1, kind: "key", mode: "button", x: 2, y: 1 });
    expect(cmds("update_button")[0].args.button).toMatchObject({ id: 50, x: 2, y: 1, w: 2, h: 2, color: "#abcdef" });
  });

  it("offers paste in the empty-cell menu only with a copied tile", async () => {
    seed();
    const w = await mountApp();
    const ev = { clientX: 10, clientY: 10 };
    w.vm.emptyContextMenu({ x: 1, y: 1 }, ev);
    await flushPromises();
    expect(w.findAll(".ctx-menu .menu-item").map((b) => b.text())).toEqual(["New button here"]);
    w.vm.copyTile(w.vm.boards[0].buttons[0]);
    w.vm.emptyContextMenu({ x: 1, y: 1 }, ev);
    await flushPromises();
    expect(w.findAll(".ctx-menu .menu-item").map((b) => b.text())).toEqual(["New button here", "Paste button here"]);
    await w.findAll(".ctx-menu .menu-item")[0].trigger("click");
    expect(w.vm.createFlow).toEqual({ x: 1, y: 1, boardId: 1 });
    expect(w.find(".ctx-menu").exists()).toBe(false);
  });

  it("runs tile menu items", async () => {
    seed();
    const w = await mountApp();
    const t = w.vm.boards[0].buttons[0];
    await w.find('[data-tile-id="11"] .tile').trigger("contextmenu", { clientX: 5, clientY: 5 });
    expect(w.findAll(".ctx-menu .menu-item").map((b) => b.text())).toEqual([
      "Edit tile",
      "Run now",
      "Copy",
      "Duplikuj",
      "Przenieś do tablicy...",
      "Kopiuj do tablicy...",
      "Delete",
    ]);
    await w.findAll(".ctx-menu .menu-item")[1].trigger("click");
    await flushPromises();
    expect(cmds("exec_button")[0].args).toEqual({ id: 11 });
    w.vm.tileContextMenu(t, { clientX: 5, clientY: 5 });
    await flushPromises();
    await w.findAll(".ctx-menu .menu-item")[2].trigger("click");
    expect(w.vm.tileClipboard).toEqual(t);
  });

  it("keeps the menu inside the window", async () => {
    seed();
    const w = await mountApp();
    w.vm.openContextMenu({ clientX: 99999, clientY: 99999 }, [{ label: "a", run() {} }]);
    expect(w.vm.contextMenu.x).toBe(window.innerWidth - 216);
    expect(w.vm.contextMenu.y).toBe(window.innerHeight - 44 - 38);
  });

  it("closes the menu on Escape and on outside mousedown", async () => {
    seed();
    const w = await mountApp();
    w.vm.openContextMenu({ clientX: 1, clientY: 1 }, [{ label: "a", run() {} }]);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.vm.contextMenu).toBeNull();
    w.vm.openContextMenu({ clientX: 1, clientY: 1 }, [{ label: "a", run() {} }]);
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(w.vm.contextMenu).toBeNull();
  });

  it("clears and deletes boards from the board menu after confirmation", async () => {
    seed();
    const w = await mountApp();
    const board = w.vm.boards[1];
    w.vm.boardContextMenu(board, { clientX: 1, clientY: 1 });
    await w.vm.contextMenu.items.find((i) => i.label === "Clear tiles").run();
    w.vm.boardContextMenu(board, { clientX: 1, clientY: 1 });
    await w.vm.contextMenu.items.find((i) => i.label === "Delete board").run();
    expect(cmds("clear_board")[0].args).toEqual({ boardId: 2 });
    expect(cmds("delete_board")[0].args).toEqual({ boardId: 2 });
    w.vm.boardContextMenu(board, { clientX: 1, clientY: 1 });
    w.vm.contextMenu.items.find((i) => i.label === "Set current").run();
    expect(w.vm.currentId).toBe(2);
  });
});

describe("App shell", () => {
  it("zooms in 10% steps between 50% and 150%", async () => {
    seed();
    const w = await mountApp();
    for (let i = 0; i < 10; i++) w.vm.bumpZoom(1);
    expect(w.vm.zoom).toBe(1.5);
    for (let i = 0; i < 20; i++) w.vm.bumpZoom(-1);
    expect(w.vm.zoom).toBe(0.5);
    w.vm.bumpZoom(1);
    await flushPromises();
    expect(w.find(".zoom-value").text()).toBe("60%");
  });

  it("exports every board to the picked path", async () => {
    seed();
    dialog.save.mockResolvedValueOnce("C:/out.boardjson");
    const w = await mountApp();
    await w.vm.doExport();
    expect(cmds("export_boards")[0].args).toEqual({ ids: [1, 2], path: "C:/out.boardjson" });
  });

  it("imports a picked file and reloads", async () => {
    seed();
    dialog.open.mockResolvedValueOnce("C:/in.boardjson");
    const w = await mountApp();
    await w.vm.doImport();
    expect(cmds("import_boards")[0].args).toEqual({ path: "C:/in.boardjson" });
    expect(cmds("list_boards")).toHaveLength(2);
  });

  it("flashes an import failure and clears it after six seconds", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    seed();
    dialog.open.mockResolvedValueOnce("C:/bad.boardjson");
    handle("import_boards", () => {
      throw "too many tiles";
    });
    const w = await mountApp();
    vi.useFakeTimers();
    await w.vm.doImport();
    expect(w.vm.flash).toBe("Import tablic nie powiódł się: too many tiles");
    vi.advanceTimersByTime(6000);
    expect(w.vm.flash).toBeNull();
  });

  it("skips export and import when the dialogs are cancelled", async () => {
    seed();
    const w = await mountApp();
    await w.vm.doExport();
    await w.vm.doImport();
    expect(cmds("export_boards")).toHaveLength(0);
    expect(cmds("import_boards")).toHaveLength(0);
  });

  it("opens the status popover with server details", async () => {
    seed();
    const w = await mountApp();
    await w.find(".rail-btn[title='Server status']").trigger("click");
    expect(w.find(".rail-pop").text()).toContain("Server running");
    expect(w.find(".rail-pop").text()).toContain("0.0.0.0:8500");
    expect(w.find(".rail-pop").text()).toContain("2 client(s) connected");
    await w.find(".click-away").trigger("click");
    expect(w.find(".rail-pop").exists()).toBe(false);
  });

  it("hides the sidebar from the rail logo", async () => {
    seed();
    const w = await mountApp();
    await w.find(".rail-logo").trigger("click");
    expect(w.find(".sidebar").exists()).toBe(false);
  });
});
