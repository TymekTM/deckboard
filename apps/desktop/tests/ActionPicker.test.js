import { describe, it, expect, afterEach } from "vitest";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import ActionPicker from "../src/components/ActionPicker.vue";

const groups = [
  {
    header: "Pulpit",
    items: [
      { value: "board", label: "Switch Board", icon: "th", color: "#2c3e50" },
      { value: "multiaction", label: "Multi Actions", icon: "th-large" },
    ],
  },
  {
    header: "System",
    items: [
      { value: "url", label: "Open URL", icon: "link", color: "#2980b9" },
      { value: "app", label: "Run Program" },
    ],
  },
];

function mountPicker(modelValue = "url") {
  return mount(ActionPicker, { props: { groups, modelValue }, attachTo: document.body });
}

async function openPicker(w) {
  await w.find(".trigger").trigger("click");
  await nextTick();
}

const items = (w) => w.findAll(".item");
const search = (w) => w.find("input");
const highlighted = (w) => w.find(".item.hl .ilabel").text();

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActionPicker trigger", () => {
  it("shows the current action with its icon and color", () => {
    const w = mountPicker("url");
    expect(w.find(".tval").text()).toBe("Open URL");
    expect(w.find(".trigger .chip i").classes()).toContain("fa-link");
    expect(w.find(".trigger .chip").attributes("style")).toMatch(/#2980b9|rgb\(41, 128, 185\)/);
  });

  it("shows an unknown stored type raw with fallback styling", () => {
    const w = mountPicker("ext-thing");
    expect(w.find(".tval").text()).toBe("ext-thing");
    expect(w.find(".trigger .chip i").classes()).toContain("fa-cog");
  });

  it("opens from the arrow keys", async () => {
    const w = mountPicker();
    await w.find(".trigger").trigger("keydown", { key: "ArrowDown" });
    await nextTick();
    expect(w.find(".pop").exists()).toBe(true);
    expect(document.activeElement).toBe(search(w).element);
  });
});

describe("ActionPicker list", () => {
  it("lists groups with headers and marks the selection", async () => {
    const w = mountPicker("url");
    await openPicker(w);
    expect(w.findAll(".ghead").map((h) => h.text())).toEqual(["Pulpit", "System"]);
    expect(items(w)).toHaveLength(4);
    const sel = w.find(".item.sel");
    expect(sel.text()).toContain("Open URL");
    expect(sel.attributes("aria-selected")).toBe("true");
    // highlight starts on the selection
    expect(highlighted(w)).toBe("Open URL");
  });

  it("pins an unknown stored type under a Custom group", async () => {
    const w = mountPicker("ext-thing");
    await openPicker(w);
    expect(w.findAll(".ghead").map((h) => h.text())).toContain("Custom");
    expect(w.find(".item.sel .ilabel").text()).toBe("ext-thing (custom)");
  });

  it("adds no Custom group for an empty value", async () => {
    const w = mountPicker("");
    await openPicker(w);
    expect(w.findAll(".ghead").map((h) => h.text())).not.toContain("Custom");
    expect(highlighted(w)).toBe("Switch Board");
  });

  it("filters by label and by value, dropping empty groups", async () => {
    const w = mountPicker();
    await openPicker(w);
    await search(w).setValue("  URL ");
    expect(items(w).map((i) => i.find(".ilabel").text())).toEqual(["Open URL"]);
    expect(w.findAll(".ghead").map((h) => h.text())).toEqual(["System"]);
    await search(w).setValue("multiaction");
    expect(items(w).map((i) => i.find(".ilabel").text())).toEqual(["Multi Actions"]);
  });

  it("says when nothing matches", async () => {
    const w = mountPicker();
    await openPicker(w);
    await search(w).setValue("zzz ");
    expect(items(w)).toHaveLength(0);
    expect(w.find(".empty").text()).toBe("No actions match “zzz”");
    // Enter with nothing highlighted picks nothing
    await search(w).trigger("keydown", { key: "Enter" });
    expect(w.emitted("update:modelValue")).toBeUndefined();
  });

  it("picks on click and closes", async () => {
    const w = mountPicker();
    await openPicker(w);
    await items(w)[0].trigger("click");
    expect(w.emitted("update:modelValue")).toEqual([["board"]]);
    expect(w.find(".pop").exists()).toBe(false);
    expect(document.activeElement).toBe(w.find(".trigger").element);
  });

  it("moves the highlight with the keyboard and wraps", async () => {
    const w = mountPicker("board");
    await openPicker(w);
    await search(w).trigger("keydown", { key: "ArrowUp" });
    expect(highlighted(w)).toBe("Run Program");
    await search(w).trigger("keydown", { key: "ArrowDown" });
    expect(highlighted(w)).toBe("Switch Board");
    await search(w).trigger("keydown", { key: "End" });
    expect(highlighted(w)).toBe("Run Program");
    await search(w).trigger("keydown", { key: "Home" });
    expect(highlighted(w)).toBe("Switch Board");
    await search(w).trigger("keydown", { key: "ArrowDown" });
    await search(w).trigger("keydown", { key: "Enter" });
    expect(w.emitted("update:modelValue")).toEqual([["multiaction"]]);
  });

  it("resets the highlight to the first match when the query changes", async () => {
    const w = mountPicker("app");
    await openPicker(w);
    await search(w).setValue("o");
    expect(highlighted(w)).toBe("Switch Board");
  });

  it("follows the mouse with the highlight", async () => {
    const w = mountPicker();
    await openPicker(w);
    await items(w)[3].trigger("mouseenter");
    expect(highlighted(w)).toBe("Run Program");
  });

  it("points aria-activedescendant at the highlighted option", async () => {
    const w = mountPicker("url");
    await openPicker(w);
    const id = search(w).attributes("aria-activedescendant");
    expect(w.find(`#${id}`).find(".ilabel").text()).toBe("Open URL");
  });

  it("clears the query on the first Escape and closes on the second", async () => {
    const w = mountPicker();
    await openPicker(w);
    await search(w).setValue("url");
    await search(w).trigger("keydown", { key: "Escape" });
    expect(search(w).element.value).toBe("");
    expect(w.find(".pop").exists()).toBe(true);
    await search(w).trigger("keydown", { key: "Escape" });
    expect(w.find(".pop").exists()).toBe(false);
  });

  it("closes on Tab and on an outside click", async () => {
    const w = mountPicker();
    await openPicker(w);
    await search(w).trigger("keydown", { key: "Tab" });
    expect(w.find(".pop").exists()).toBe(false);
    await openPicker(w);
    document.body.click();
    await nextTick();
    expect(w.find(".pop").exists()).toBe(false);
  });

  it("reopens with an empty query", async () => {
    const w = mountPicker();
    await openPicker(w);
    await search(w).setValue("url");
    await w.find(".trigger").trigger("click");
    await openPicker(w);
    expect(search(w).element.value).toBe("");
    expect(items(w)).toHaveLength(4);
  });
});
