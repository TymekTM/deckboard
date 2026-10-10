import { describe, it, expect, afterEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import ConfirmDialog from "../src/components/ConfirmDialog.vue";

// setup.js swaps `ask` for the dialog mock; the real one is what runs here
const real = await vi.importActual("../src/confirm.js");

let wrapper;
afterEach(() => {
  wrapper?.unmount();
  wrapper = null;
  real.answerConfirm(false);
});

function mountDialog() {
  wrapper = mount(ConfirmDialog, { attachTo: document.body });
  return wrapper;
}

describe("ConfirmDialog", () => {
  it("renders nothing until something asks", () => {
    mountDialog();
    expect(wrapper.find(".confirm").exists()).toBe(false);
  });

  it("shows the message, title and custom labels", async () => {
    mountDialog();
    const answer = real.ask('Delete tile "t1"?', {
      title: "Delete tile",
      kind: "warning",
      okLabel: "Delete",
      cancelLabel: "Keep",
    });
    await flushPromises();
    expect(wrapper.text()).toContain("Delete tile");
    expect(wrapper.text()).toContain('Delete tile "t1"?');
    const buttons = wrapper.findAll("button");
    expect(buttons.map((b) => b.text())).toEqual(["Keep", "Delete"]);
    expect(buttons[1].classes()).toContain("danger-solid");
    real.answerConfirm(false);
    await answer;
  });

  it("resolves true on confirm and false on cancel", async () => {
    mountDialog();
    const yes = real.ask("Sure?", { okLabel: "Yes" });
    await flushPromises();
    await wrapper.findAll("button")[1].trigger("click");
    expect(await yes).toBe(true);
    expect(wrapper.find(".confirm").exists()).toBe(false);

    const no = real.ask("Sure?");
    await flushPromises();
    await wrapper.findAll("button")[0].trigger("click");
    expect(await no).toBe(false);
  });

  it("cancels on Escape and on a backdrop click", async () => {
    mountDialog();
    const esc = real.ask("Sure?");
    await flushPromises();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(await esc).toBe(false);

    const back = real.ask("Sure?");
    await flushPromises();
    await wrapper.find(".veil").trigger("mousedown");
    expect(await back).toBe(false);
  });

  it("focuses Cancel first so a stray Enter cannot confirm", async () => {
    mountDialog();
    const answer = real.ask("Sure?", { kind: "warning" });
    await flushPromises();
    expect(document.activeElement).toBe(wrapper.findAll("button")[0].element);
    real.answerConfirm(false);
    await answer;
  });

  it("keeps keys away from listeners behind it", async () => {
    mountDialog();
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    const answer = real.ask("Sure?");
    await flushPromises();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    expect(behind).not.toHaveBeenCalled();
    window.removeEventListener("keydown", behind);
    real.answerConfirm(false);
    await answer;
  });

  it("a newer ask cancels the one still open", async () => {
    mountDialog();
    const first = real.ask("one");
    const second = real.ask("two");
    expect(await first).toBe(false);
    await flushPromises();
    expect(wrapper.text()).toContain("two");
    real.answerConfirm(true);
    expect(await second).toBe(true);
  });
});
