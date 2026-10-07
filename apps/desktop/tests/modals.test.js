import { describe, it, expect, afterEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import BoardModal from "../src/components/BoardModal.vue";
import OperatorAskModal from "../src/components/OperatorAskModal.vue";
import { calls, handle, dialog, emit, listenerCount } from "./tauri.js";

afterEach(() => {
  document.body.innerHTML = "";
});

const board = { id: 3, name: "Main", width: 5, height: 3, background: "#101010", extra: "kept" };
const button = (w, text) => w.findAll("button").find((b) => b.text() === text);

describe("BoardModal create", () => {
  it("creates a board with defaults when the name is empty", async () => {
    const w = mount(BoardModal, { props: { mode: "create" } });
    expect(w.find(".modal-head").text()).toBe("New board");
    expect(button(w, "Delete board")).toBeUndefined();
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(calls).toEqual([
      { cmd: "create_board", args: { name: "New board", background: "#437072", width: 6, height: 4 } },
    ]);
    expect(w.emitted("saved")).toHaveLength(1);
  });

  it("clamps typed dimensions before saving", async () => {
    const w = mount(BoardModal, { props: { mode: "create" } });
    const [name, width, height] = w.findAll("input");
    await name.setValue("Stream");
    await width.setValue("99");
    await height.setValue("0");
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(calls[0].args).toMatchObject({ name: "Stream", width: 32, height: 1 });
  });

  it("saves on Enter in the name field", async () => {
    const w = mount(BoardModal, { props: { mode: "create" } });
    await w.find("input").trigger("keyup", { key: "Enter" });
    await flushPromises();
    expect(calls[0].cmd).toBe("create_board");
  });

  it("keeps the dialog open and shows the backend error", async () => {
    handle("create_board", () => {
      throw "duplicate board name";
    });
    const w = mount(BoardModal, { props: { mode: "create" } });
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(w.find(".modal-error").text()).toBe("duplicate board name");
    expect(w.emitted("saved")).toBeUndefined();
  });

  it("closes from the overlay only while untouched", async () => {
    const w = mount(BoardModal, { props: { mode: "create" } });
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
    await w.find("input").setValue("edited");
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
    await button(w, "Cancel").trigger("click");
    expect(w.emitted("close")).toHaveLength(2);
  });
});

describe("BoardModal edit", () => {
  it("prefills and updates the board, keeping its other fields", async () => {
    const w = mount(BoardModal, { props: { mode: "edit", board } });
    expect(w.find(".modal-head").text()).toBe("Edit board");
    expect(w.find("input").element.value).toBe("Main");
    await w.find("input").setValue("Renamed");
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(calls).toEqual([
      {
        cmd: "update_board",
        args: { board: { ...board, name: "Renamed" } },
      },
    ]);
  });

  it("truncates fractional dimensions", async () => {
    const w = mount(BoardModal, { props: { mode: "edit", board } });
    await w.findAll("input")[1].setValue("7.6");
    await button(w, "Save").trigger("click");
    await flushPromises();
    expect(calls[0].args.board.width).toBe(7);
  });

  it("clears tiles only after confirmation", async () => {
    dialog.ask.mockResolvedValueOnce(false);
    const w = mount(BoardModal, { props: { mode: "edit", board } });
    await button(w, "Clear tiles").trigger("click");
    await flushPromises();
    expect(calls).toHaveLength(0);
    await button(w, "Clear tiles").trigger("click");
    await flushPromises();
    expect(dialog.ask).toHaveBeenLastCalledWith('Clear every tile from "Main"?', {
      title: "Clear board",
      kind: "warning",
    });
    expect(calls).toEqual([{ cmd: "clear_board", args: { boardId: 3 } }]);
    expect(w.emitted("saved")).toHaveLength(1);
  });

  it("deletes after confirmation and reports failures", async () => {
    handle("delete_board", () => {
      throw "cannot delete the last board";
    });
    const w = mount(BoardModal, { props: { mode: "edit", board } });
    await button(w, "Delete board").trigger("click");
    await flushPromises();
    expect(calls).toEqual([{ cmd: "delete_board", args: { boardId: 3 } }]);
    expect(w.find(".modal-error").text()).toBe("cannot delete the last board");
    expect(w.emitted("saved")).toBeUndefined();
  });

  it("uses a generic message when the backend error is empty", async () => {
    handle("clear_board", () => {
      throw "";
    });
    const w = mount(BoardModal, { props: { mode: "edit", board } });
    await button(w, "Clear tiles").trigger("click");
    await flushPromises();
    expect(w.find(".modal-error").text()).toBe("Clearing the board failed.");
  });
});

describe("OperatorAskModal", () => {
  async function mountAsk() {
    const w = mount(OperatorAskModal);
    await flushPromises();
    return w;
  }

  it("stays hidden until the backend asks", async () => {
    const w = await mountAsk();
    expect(w.find(".veil").exists()).toBe(false);
    expect(listenerCount("operator-ask")).toBe(1);
  });

  it("shows a pair request with its verification code", async () => {
    const w = await mountAsk();
    emit("operator-ask", { id: 7, kind: "pair-request", name: "Tablet", code: "123456" });
    await flushPromises();
    expect(w.find("h2").text()).toBe("Żądanie parowania");
    expect(w.find(".ask-name").text()).toBe("Tablet");
    expect(w.find(".ask-code").text()).toBe("123456");
  });

  it("shows the manual-code variant without a code block", async () => {
    const w = await mountAsk();
    emit("operator-ask", { id: 8, kind: "pairing-code", name: "Phone" });
    await flushPromises();
    expect(w.find("h2").text()).toBe("Zaufać urządzeniu?");
    expect(w.find(".ask-code").exists()).toBe(false);
  });

  it.each([
    ["Zaufaj", true],
    ["Odrzuć", false],
  ])("answers %s with approved=%s and closes", async (label, approved) => {
    const w = await mountAsk();
    emit("operator-ask", { id: 9, kind: "pair-request", name: "T", code: "1" });
    await flushPromises();
    await w.findAll("button").find((b) => b.text().includes(label)).trigger("click");
    await flushPromises();
    expect(calls).toEqual([{ cmd: "resolve_operator_ask", args: { id: 9, approved } }]);
    expect(w.find(".veil").exists()).toBe(false);
  });

  it("closes even when the ask already expired server-side", async () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    handle("resolve_operator_ask", () => {
      throw "expired";
    });
    const w = await mountAsk();
    emit("operator-ask", { id: 1, kind: "pair-request", name: "T" });
    await flushPromises();
    await w.find(".act.trust").trigger("click");
    await flushPromises();
    expect(w.find(".veil").exists()).toBe(false);
    expect(err).toHaveBeenCalled();
  });

  it("answers only once while a resolve is in flight", async () => {
    let release;
    handle("resolve_operator_ask", () => new Promise((r) => (release = r)));
    const w = await mountAsk();
    emit("operator-ask", { id: 2, kind: "pair-request", name: "T" });
    await flushPromises();
    await w.find(".act.trust").trigger("click");
    await w.find(".act").trigger("click");
    expect(calls).toHaveLength(1);
    expect(w.find(".act").attributes("disabled")).toBeDefined();
    release();
    await flushPromises();
  });

  it("replaces a pending ask with a newer one", async () => {
    const w = await mountAsk();
    emit("operator-ask", { id: 1, kind: "pair-request", name: "Old" });
    emit("operator-ask", { id: 2, kind: "pair-request", name: "New" });
    await flushPromises();
    expect(w.find(".ask-name").text()).toBe("New");
  });

  it("unsubscribes on unmount", async () => {
    const w = await mountAsk();
    w.unmount();
    expect(listenerCount("operator-ask")).toBe(0);
  });
});
