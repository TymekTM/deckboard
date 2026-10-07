// In-memory stand-in for the Tauri bridge. Tests register command
// handlers (`handle("list_boards", () => [...])`), fire backend events
// (`emit("operator-ask", payload)`) and script dialog answers; every
// invoke is recorded in `calls` so a test can assert the exact command
// and argument names the Rust side expects.
import { vi } from "vitest";

// State lives on globalThis: vi.resetModules() reloads this file but
// keeps the registered vi.mock factories bound to the first copy, so a
// per-module state would split between the two.
const state = (globalThis.__pulpitTauriMock ??= {
  calls: [],
  handlers: new Map(),
  listeners: new Map(),
  dialog: {
    ask: vi.fn(async () => true),
    open: vi.fn(async () => null),
    save: vi.fn(async () => null),
  },
});
export const calls = state.calls;
export const dialog = state.dialog;
const { handlers, listeners } = state;

export function handle(cmd, fn) {
  handlers.set(cmd, fn);
}

export async function invoke(cmd, args) {
  calls.push({ cmd, args });
  const fn = handlers.get(cmd);
  if (!fn) return null;
  return fn(args);
}

export async function listen(event, cb) {
  if (!listeners.has(event)) listeners.set(event, new Set());
  const set = listeners.get(event);
  const entry = (payload) => cb({ event, payload });
  set.add(entry);
  return () => set.delete(entry);
}

export function emit(event, payload) {
  for (const cb of listeners.get(event) || []) cb(payload);
}

export function listenerCount(event) {
  return listeners.get(event)?.size || 0;
}

export function resetTauri() {
  calls.length = 0;
  handlers.clear();
  listeners.clear();
  dialog.ask.mockReset().mockImplementation(async () => true);
  dialog.open.mockReset().mockImplementation(async () => null);
  dialog.save.mockReset().mockImplementation(async () => null);
}
