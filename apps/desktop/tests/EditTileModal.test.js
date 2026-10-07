import { describe, it, expect, afterEach } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import EditTileModal from "../src/components/EditTileModal.vue";
import ActionPicker from "../src/components/ActionPicker.vue";
import SelectField from "../src/components/SelectField.vue";
import { calls, dialog, handle } from "./tauri.js";

const boards = [
  { id: 1, name: "Main" },
  { id: 2, name: "Stream" },
  { id: 3, name: "Games" },
];

const existing = (extra = {}) => ({
  id: 10,
  board_id: 1,
  type: "key",
  mode: "button",
  command: "CTRL + K",
  title: "Save",
  color: "#123456",
  border_color: "",
  shape: 0,
  title_position: 0,
  title_color: "",
  title_box_color: "",
  icon: "",
  icon_color: "",
  img: "",
  color2: "",
  icon2: "",
  img2: "",
  options: "",
  x: 0,
  y: 0,
  w: 1,
  h: 1,
  ...extra,
});

function mountEdit(button, props = {}) {
  return mount(EditTileModal, {
    props: { button, boards, ...props },
    attachTo: document.body,
  });
}

function mountCreate(props = {}) {
  return mountEdit(null, { create: { x: 2, y: 1, boardId: 1 }, ...props });
}

const fieldInput = (w, label) => {
  const field = w.findAll("label.field").find((f) => f.text().startsWith(label));
  if (!field) throw new Error(`no field "${label}"`);
  return field.find("input, textarea, select");
};
const button = (w, text) => w.findAll("button").find((b) => b.text().trim() === text);
const pickAction = async (w, value) => {
  w.findComponent(ActionPicker).vm.$emit("update:modelValue", value);
  await w.vm.$nextTick();
};
async function save(w) {
  const label = w.find(".modal-actions .accent").text();
  await w.find(".modal-actions .accent").trigger("click");
  const ev = label === "Add" ? "create" : "save";
  return w.emitted(ev).at(-1)[0];
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("EditTileModal basics", () => {
  it("creates a key tile at the requested cell", async () => {
    const w = mountCreate();
    expect(w.find(".modal-head").text()).toBe("New Button");
    expect(button(w, "Delete")).toBeUndefined();
    await fieldInput(w, "Keystroke").setValue("CTRL + SHIFT + P");
    await fieldInput(w, "Label").setValue("Palette");
    const out = await save(w);
    expect(out).toMatchObject({
      id: null,
      board_id: 1,
      type: "key",
      command: "CTRL + SHIFT + P",
      title: "Palette",
      x: 2,
      y: 1,
      w: 1,
      h: 1,
    });
  });

  it("edits an existing tile and keeps its other columns", async () => {
    const w = mountEdit(existing({ extra_col: "kept" }));
    expect(w.find(".modal-head").text()).toBe("Edit Button");
    expect(fieldInput(w, "Keystroke").element.value).toBe("CTRL + K");
    const out = await save(w);
    expect(out).toMatchObject({ id: 10, command: "CTRL + K", extra_col: "kept", color: "#123456" });
  });

  it("emits delete and close", async () => {
    const w = mountEdit(existing());
    await button(w, "Delete").trigger("click");
    await button(w, "Cancel").trigger("click");
    expect(w.emitted("delete")).toHaveLength(1);
    expect(w.emitted("close")).toHaveLength(1);
  });

  it("closes from the overlay only while untouched", async () => {
    const w = mountEdit(existing());
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
    await fieldInput(w, "Label").setValue("changed");
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toHaveLength(1);
  });

  it("counts a command-field edit as dirty", async () => {
    const w = mountEdit(existing());
    await fieldInput(w, "Keystroke").setValue("ALT + F4");
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toBeUndefined();
  });
});

describe("EditTileModal command mapping", () => {
  it("writes structured fields as a JSON object and keeps unknown keys", async () => {
    const w = mountEdit(
      existing({ type: "url-to-call", command: JSON.stringify({ urlToCall: "http://a", extra: 1 }) }),
    );
    expect(fieldInput(w, "URL to call").element.value).toBe("http://a");
    await fieldInput(w, "URL to call").setValue("http://b");
    await fieldInput(w, "Command action").setValue("note");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ urlToCall: "http://b", commandAction: "note", extra: 1 });
  });

  it("drops a cleared visible field from the command", async () => {
    const w = mountEdit(
      existing({ type: "url-to-call", command: JSON.stringify({ urlToCall: "http://a", commandAction: "x" }) }),
    );
    await fieldInput(w, "Command action").setValue("");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ urlToCall: "http://a" });
  });

  it("stores number fields as numbers and hides them behind showIf", async () => {
    const w = mountCreate();
    await pickAction(w, "mouse-ctrl");
    // select defaults to the first option ("move"), which shows X/Y
    await fieldInput(w, "X").setValue("120");
    await fieldInput(w, "Y").setValue("abc");
    let out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ action: "move", x: 120 });

    const click = mountEdit(
      existing({ type: "mouse-ctrl", command: JSON.stringify({ action: "lclick", x: 5 }) }),
    );
    expect(click.findAll("label.field").some((f) => f.text().startsWith("X"))).toBe(false);
    out = await save(click);
    // a hidden field's stored value survives untouched
    expect(JSON.parse(out.command)).toEqual({ action: "lclick", x: 5 });
  });

  it("drops a cleared visible number field", async () => {
    const w = mountEdit(
      existing({ type: "mouse-ctrl", command: JSON.stringify({ action: "move", x: 5, y: 6 }) }),
    );
    await fieldInput(w, "X").setValue("");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ action: "move", y: 6 });
  });

  it("recovers from a malformed stored command", async () => {
    const w = mountEdit(existing({ type: "url-to-call", command: "{oops" }));
    expect(fieldInput(w, "URL to call").element.value).toBe("");
    await fieldInput(w, "URL to call").setValue("http://x");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ urlToCall: "http://x" });
  });

  it("keeps Voicemeeter select values numeric", async () => {
    const w = mountCreate();
    await pickAction(w, "vm-toggle-bus");
    const selects = w.findAllComponents(SelectField);
    expect(selects).toHaveLength(2);
    selects[1].vm.$emit("update:modelValue", 5);
    await w.vm.$nextTick();
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ param: "Mono", number: 5 });
  });

  it("writes board switches as {id} and hides the own board", async () => {
    const w = mountEdit(existing({ type: "board", command: JSON.stringify({ id: 2 }) }));
    const select = fieldInput(w, "Switch to");
    const names = select.findAll("option").map((o) => o.text());
    expect(names).toEqual(["- pick board -", "Stream", "Games"]);
    expect(select.element.value).toBe("2");
    await select.setValue("3");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ id: 3 });
  });

  it("writes an empty command when no board is picked", async () => {
    const w = mountCreate();
    await pickAction(w, "board");
    const out = await save(w);
    expect(out.command).toBe("");
  });

  it("stores select-style commands raw and defaults to init", async () => {
    const w = mountCreate();
    await pickAction(w, "vol");
    let out = await save(w);
    expect(out.command).toBe("play");
    await fieldInput(w, "Multimedia").setValue("vol_mute");
    out = await save(w);
    expect(out.command).toBe("vol_mute");
  });

  it("edits multi-action steps", async () => {
    const steps = [
      { type: "key", command: "CTRL + C" },
      { type: "board", command: JSON.stringify({ id: 2 }) },
    ];
    const w = mountEdit(existing({ type: "multiaction", command: JSON.stringify(steps) }));
    expect(w.findAll(".step")).toHaveLength(2);
    await button(w, "Add step").trigger("click");
    expect(w.findAll(".step")).toHaveLength(3);
    // board step renders the board select
    const boardSelect = w.findAll(".step")[1].findAll("select")[1];
    await boardSelect.setValue("3");
    await w.findAll(".step-del")[0].trigger("click");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual([
      { type: "board", command: JSON.stringify({ id: 3 }) },
      { type: "delay", command: "100" },
    ]);
  });

  it("converts numeric advance-key delays", async () => {
    const steps = [
      { type: "down", command: "SHIFT" },
      { type: "delay", command: "250" },
      { type: "delay", command: "" },
    ];
    const w = mountEdit(existing({ type: "advance-key", command: JSON.stringify(steps) }));
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual([
      { type: "down", command: "SHIFT" },
      { type: "delay", command: 250 },
      { type: "delay", command: "" },
    ]);
  });

  it("treats a non-array step command as empty", async () => {
    const w = mountEdit(existing({ type: "multiaction", command: '{"a":1}' }));
    expect(w.findAll(".step")).toHaveLength(0);
  });

  it("offers a raw command input for unknown types", async () => {
    const w = mountEdit(existing({ type: "something-custom", command: "raw" }));
    await fieldInput(w, "Command").setValue("raw2");
    const out = await save(w);
    expect(out.command).toBe("raw2");
  });

  it("shows the program-arguments field only for actions that take them", async () => {
    const w = mountCreate();
    expect(w.findAll("label.field").some((f) => f.text().startsWith("Options"))).toBe(false);
    await pickAction(w, "app");
    await fieldInput(w, "Options").setValue("--fullscreen");
    const out = await save(w);
    expect(out.options).toBe("--fullscreen");
  });
});

describe("EditTileModal action switching", () => {
  it("forces the catalog mode for fixed-mode actions", async () => {
    const w = mountCreate();
    await pickAction(w, "speaker-volume");
    const out = await save(w);
    expect(out.mode).toBe("slider");
  });

  it("locks the Tile Mode row for fixed-mode actions", async () => {
    const w = mountCreate();
    await pickAction(w, "ai-plan-limits");
    await w.findAll(".prop-row").find((r) => r.text().startsWith("Tile Mode")).trigger("click");
    expect(w.find(".prop-pop").exists()).toBe(false);
  });

  it("lets free actions pick a mode from the property row", async () => {
    const w = mountCreate();
    await w.findAll(".prop-row").find((r) => r.text().startsWith("Tile Mode")).trigger("click");
    await button(w, "Toggle").trigger("click");
    expect(w.find(".prop-pop").exists()).toBe(false);
    const out = await save(w);
    expect(out.mode).toBe("toggle");
  });

  it("shows second-state styling for dual actions and toggle tiles", async () => {
    const w = mountCreate();
    expect(w.find(".dual-head").exists()).toBe(false);
    await pickAction(w, "obs-scene");
    expect(w.find(".dual-head").exists()).toBe(true);
    const toggle = mountEdit(existing({ mode: "toggle" }));
    expect(toggle.find(".dual-head").exists()).toBe(true);
    const styled = mountEdit(existing({ icon2: "stop" }));
    expect(styled.find(".dual-head").exists()).toBe(true);
  });

  it("toggles plan usage windows into the options token", async () => {
    const w = mountCreate();
    await pickAction(w, "ai-plan-limits");
    const [five, week] = w.findAll(".win-check input");
    expect(five.element.checked).toBe(true);
    expect(week.element.checked).toBe(true);
    await five.setValue(false);
    const out = await save(w);
    expect(out.options).toBe("windows:week");
  });
});

describe("EditTileModal dynamic pickers", () => {
  it("lists audio devices for Set Audio Device", async () => {
    const w = mountCreate({
      audioDevices: [
        { id: "{a}", name: "Speakers" },
        { id: "{b}", name: "Headset" },
      ],
    });
    await pickAction(w, "speaker-device");
    const select = w.findComponent(SelectField);
    expect(select.props("options")).toEqual([
      { value: "{a}", label: "Speakers" },
      { value: "{b}", label: "Headset" },
    ]);
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ speaker: "{a}" });
  });

  it("uses Spotify pickers when lists are loaded and free text otherwise", async () => {
    const loaded = mountCreate({ spotifyPlaylists: [{ uri: "spotify:playlist:1", name: "Mix" }] });
    await pickAction(loaded, "spotify-tracks");
    expect(loaded.findComponent(SelectField).props("options")).toEqual([
      { value: "spotify:playlist:1", label: "Mix" },
    ]);
    const empty = mountCreate();
    await pickAction(empty, "spotify-tracks");
    expect(empty.findComponent(SelectField).exists()).toBe(false);
    await fieldInput(empty, "URI").setValue("spotify:album:9");
    const out = await save(empty);
    expect(JSON.parse(out.command)).toEqual({ uri: "spotify:album:9" });
  });

  it("uses the Spotify device names as values", async () => {
    const w = mountCreate({ spotifyDevices: [{ name: "Desk" }, { name: "Phone" }] });
    await pickAction(w, "spotify-device");
    expect(w.findComponent(SelectField).props("options").map((o) => o.value)).toEqual(["Desk", "Phone"]);
  });
});

describe("EditTileModal extension inputs", () => {
  const knownInputs = [
    {
      source: "extension",
      extension: "Lights",
      value: "lights-set",
      label: "",
      icon: "lightbulb",
      color: "#ffaa00",
      command: '{"preset":"warm"}',
      fields: [
        { key: "preset", label: "Preset", kind: "input:select", items: [{ value: "warm", label: "Warm" }, { value: "cold", label: "Cold" }] },
        { key: "hotkey", label: "Hotkey", kind: "input:key" },
        { key: "file", label: "Scene file", kind: "input:file" },
        { key: "dir", label: "Scene folder", kind: "input:folder" },
        { key: "note", label: "Note", kind: "input:text" },
      ],
    },
    { source: "extension", extension: "Lights", value: "lights-off", mode: "toggle" },
    { source: "extension", value: "key" }, // shadowed by the catalog
    { source: "native", value: "native-thing" },
  ];

  it("groups extension inputs by package with prettified labels", async () => {
    const w = mountCreate({ knownInputs });
    const groups = w.findComponent(ActionPicker).props("groups");
    const lights = groups.find((g) => g.header === "Lights");
    expect(lights.items.map((i) => [i.value, i.label])).toEqual([
      ["lights-set", "Lights set"],
      ["lights-off", "Lights off"],
    ]);
    expect(groups.some((g) => g.items.some((i) => i.value === "native-thing"))).toBe(false);
    expect(groups.flatMap((g) => g.items).filter((i) => i.value === "key")).toHaveLength(1);
  });

  it("maps declared fields and seeds the command template", async () => {
    const w = mountCreate({ knownInputs });
    await pickAction(w, "lights-set");
    expect(w.findComponent(SelectField).props("options")).toEqual([
      { value: "warm", label: "Warm" },
      { value: "cold", label: "Cold" },
    ]);
    expect(fieldInput(w, "Hotkey").attributes("placeholder")).toBe("e.g. CTRL + K");
    expect(fieldInput(w, "Scene file").attributes("placeholder")).toBe("file path");
    expect(fieldInput(w, "Scene folder").attributes("placeholder")).toBe("folder path");
    await fieldInput(w, "Note").setValue("hi");
    const out = await save(w);
    expect(JSON.parse(out.command)).toEqual({ preset: "warm", note: "hi" });
  });

  it("applies the extension's mode and says when there are no options", async () => {
    const w = mountCreate({ knownInputs });
    await pickAction(w, "lights-off");
    expect(w.find(".no-opts").exists()).toBe(true);
    const out = await save(w);
    expect(out.mode).toBe("toggle");
  });
});

describe("EditTileModal properties and images", () => {
  it("edits shape, text position and colors from the property table", async () => {
    const w = mountCreate();
    const row = (label) => w.findAll(".prop-row").find((r) => r.text().startsWith(label));
    await row("Shape").trigger("click");
    await button(w, "Round").trigger("click");
    await row("Text Position").trigger("click");
    await w.findAll(".opt").find((o) => o.text() === "Top").trigger("click");
    await row("Border Color").trigger("click");
    await w.find(".hex").setValue("#00ff00");
    await row("Icon").trigger("click");
    await w.find('.icon-cell[title="rocket"]').trigger("click");
    const out = await save(w);
    expect(out).toMatchObject({ shape: 1, title_position: 2, border_color: "#00ff00", icon: "rocket" });
  });

  it("clears a color with None", async () => {
    const w = mountEdit(existing({ border_color: "#ff0000" }));
    await w.findAll(".prop-row").find((r) => r.text().startsWith("Border Color")).trigger("click");
    await button(w, "None").trigger("click");
    const out = await save(w);
    expect(out.border_color).toBe("");
  });

  it("previews with the action's fallback icon and color", async () => {
    const w = mountCreate();
    await pickAction(w, "url");
    // the form starts with its own red, the icon falls back to the catalog
    expect(w.find(".prev-tile i").classes()).toContain("fa-link");
  });

  it("loads a picked image and marks the dialog dirty", async () => {
    dialog.open.mockResolvedValueOnce("C:/pic.png");
    handle("read_image_data", () => "data:image/png;base64,QQ==");
    const w = mountEdit(existing());
    await w.findAll(".prop-row").find((r) => r.text().startsWith("Image")).trigger("click");
    await flushPromises();
    expect(calls).toEqual([{ cmd: "read_image_data", args: { path: "C:/pic.png" } }]);
    expect(w.find(".prev-img").attributes("src")).toBe("data:image/png;base64,QQ==");
    await w.find(".overlay").trigger("click");
    expect(w.emitted("close")).toBeUndefined();
  });

  it("does nothing when the file dialog is cancelled", async () => {
    const w = mountEdit(existing());
    await w.findAll(".prop-row").find((r) => r.text().startsWith("Image")).trigger("click");
    await flushPromises();
    expect(calls).toHaveLength(0);
  });

  it("shows an image read error under the second-state section", async () => {
    dialog.open.mockResolvedValueOnce("C:/huge.png");
    handle("read_image_data", () => {
      throw "image too large";
    });
    const w = mountEdit(existing({ mode: "toggle" }));
    await button(w, "Pick image 2").trigger("click");
    await flushPromises();
    expect(w.find(".dual-error").text()).toBe("image too large");
  });
});
