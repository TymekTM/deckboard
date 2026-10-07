import { vi, beforeEach } from "vitest";
import { resetTauri } from "./tauri.js";

vi.mock("@tauri-apps/api/core", async () => {
  const t = await import("./tauri.js");
  return { invoke: t.invoke };
});
vi.mock("@tauri-apps/api/event", async () => {
  const t = await import("./tauri.js");
  return { listen: t.listen };
});
vi.mock("@tauri-apps/plugin-dialog", async () => {
  const t = await import("./tauri.js");
  return {
    ask: (...a) => t.dialog.ask(...a),
    open: (...a) => t.dialog.open(...a),
    save: (...a) => t.dialog.save(...a),
  };
});

beforeEach(() => {
  resetTauri();
});
