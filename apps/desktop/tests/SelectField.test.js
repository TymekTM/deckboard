import { describe, it, expect, afterEach } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import SelectField from "../src/components/SelectField.vue";

const opts = (...values) => values.map((v) => ({ value: v, label: String(v) }));

function mountField(props) {
  return mount(SelectField, { props: { label: "Pick", ...props }, attachTo: document.body });
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SelectField chips", () => {
  it("renders 2..6 short options as a radio group", () => {
    const w = mountField({ options: opts("a", "b", "c"), modelValue: "b" });
    expect(w.find(".chips").attributes("role")).toBe("radiogroup");
    expect(w.find(".chips").attributes("aria-label")).toBe("Pick");
    const chips = w.findAll(".chip");
    expect(chips).toHaveLength(3);
    expect(chips[1].classes()).toContain("on");
    expect(chips[1].attributes("aria-checked")).toBe("true");
    expect(chips[0].attributes("aria-checked")).toBe("false");
    expect(chips[1].find(".fa-check").exists()).toBe(true);
  });

  it("emits the clicked value, keeping numbers numeric", async () => {
    const w = mountField({ options: opts(0, 1, 2), modelValue: 0 });
    await w.findAll(".chip")[2].trigger("click");
    expect(w.emitted("update:modelValue")).toEqual([[2]]);
  });

  it("moves the selection with arrow keys and wraps", async () => {
    const w = mountField({ options: opts("a", "b", "c"), modelValue: "a" });
    const chips = w.findAll(".chip");
    await chips[0].trigger("keydown", { key: "ArrowRight" });
    await chips[0].trigger("keydown", { key: "ArrowLeft" });
    await chips[2].trigger("keydown", { key: "ArrowDown" });
    await chips[0].trigger("keydown", { key: "ArrowUp" });
    await chips[0].trigger("keydown", { key: "Enter" });
    expect(w.emitted("update:modelValue")).toEqual([["b"], ["c"], ["a"], ["c"]]);
  });

  it.each([
    ["one option", opts("a")],
    ["seven options", opts(1, 2, 3, 4, 5, 6, 7)],
    ["a long label", [{ value: "x", label: "x" }, { value: "y", label: "A very long option label" }]],
  ])("falls back to the popover for %s", (_, options) => {
    const w = mountField({ options, modelValue: "" });
    expect(w.find(".chips").exists()).toBe(false);
    expect(w.find(".trigger").exists()).toBe(true);
  });
});

describe("SelectField popover", () => {
  const many = opts("a", "b", "c", "d", "e", "f", "g");

  it("shows the current label, or the raw value when it is not an option", () => {
    expect(mountField({ options: many, modelValue: "c" }).find(".tval").text()).toBe("c");
    expect(mountField({ options: many, modelValue: "zz" }).find(".tval").text()).toBe("zz");
    expect(mountField({ options: many, modelValue: "" }).find(".tval").text()).toBe("");
    const labeled = [{ value: 1, label: "One" }, ...many];
    expect(mountField({ options: labeled, modelValue: 1 }).find(".tval").text()).toBe("One");
  });

  it("disables the trigger without options", () => {
    const w = mountField({ options: [] });
    expect(w.find(".trigger").attributes("disabled")).toBeDefined();
  });

  it("opens, focuses the selected option and closes on pick", async () => {
    const w = mountField({ options: many, modelValue: "d" });
    await w.find(".trigger").trigger("click");
    expect(w.find(".trigger").attributes("aria-expanded")).toBe("true");
    await nextTick();
    const options = w.findAll(".opt");
    expect(options).toHaveLength(7);
    expect(document.activeElement).toBe(options[3].element);
    expect(options[3].attributes("aria-selected")).toBe("true");
    await options[5].trigger("click");
    expect(w.emitted("update:modelValue")).toEqual([["f"]]);
    expect(w.find(".pop").exists()).toBe(false);
    expect(document.activeElement).toBe(w.find(".trigger").element);
  });

  it("focuses the first option when nothing is selected", async () => {
    const w = mountField({ options: many, modelValue: "nope" });
    await w.find(".trigger").trigger("keydown", { key: "ArrowDown" });
    await nextTick();
    expect(document.activeElement).toBe(w.findAll(".opt")[0].element);
  });

  it("walks options with arrows, Home and End", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    await w.find(".trigger").trigger("click");
    await nextTick();
    const options = w.findAll(".opt");
    await options[0].trigger("keydown", { key: "ArrowUp" });
    expect(document.activeElement).toBe(options[6].element);
    await options[6].trigger("keydown", { key: "ArrowDown" });
    expect(document.activeElement).toBe(options[0].element);
    await options[0].trigger("keydown", { key: "End" });
    expect(document.activeElement).toBe(options[6].element);
    await options[6].trigger("keydown", { key: "Home" });
    expect(document.activeElement).toBe(options[0].element);
    // navigation alone never commits a value
    expect(w.emitted("update:modelValue")).toBeUndefined();
  });

  it("closes on Escape and returns focus to the trigger", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    await w.find(".trigger").trigger("click");
    await nextTick();
    await w.findAll(".opt")[0].trigger("keydown", { key: "Escape" });
    expect(w.find(".pop").exists()).toBe(false);
    expect(document.activeElement).toBe(w.find(".trigger").element);
  });

  it("closes on Tab without stealing focus back", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    await w.find(".trigger").trigger("click");
    await nextTick();
    await w.findAll(".opt")[0].trigger("keydown", { key: "Tab" });
    expect(w.find(".pop").exists()).toBe(false);
  });

  it("closes on an outside click", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    await w.find(".trigger").trigger("click");
    document.body.click();
    await nextTick();
    expect(w.find(".pop").exists()).toBe(false);
  });

  it("toggles closed from the trigger", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    await w.find(".trigger").trigger("click");
    await w.find(".trigger").trigger("click");
    expect(w.find(".pop").exists()).toBe(false);
  });

  it("drops its document listeners on unmount", async () => {
    const w = mountField({ options: many, modelValue: "a" });
    w.unmount();
    // a stale handler would throw on the unmounted root ref
    expect(() => document.body.click()).not.toThrow();
  });
});
