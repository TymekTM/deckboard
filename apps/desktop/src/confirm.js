import { reactive } from "vue";

// In-app replacement for @tauri-apps/plugin-dialog's `ask`: same call shape
// (`ask(message, { title, kind })` -> Promise<boolean>), but the popup is
// ours (ConfirmDialog.vue, mounted once in App.vue) so it matches the editor
// instead of the OS dialog. `okLabel` / `cancelLabel` are additions.
export const confirmState = reactive({
  open: false,
  message: "",
  title: "",
  kind: "info", // "warning" paints the confirm button red
  okLabel: "OK",
  cancelLabel: "Cancel",
});

let pending = null;

export function ask(message, opts = {}) {
  // only one popup at a time: a newer ask cancels the one still open
  if (pending) pending(false);
  return new Promise((resolve) => {
    pending = resolve;
    Object.assign(confirmState, {
      open: true,
      message,
      title: opts.title || "",
      kind: opts.kind || "info",
      okLabel: opts.okLabel || "OK",
      cancelLabel: opts.cancelLabel || "Cancel",
    });
  });
}

export function answerConfirm(value) {
  if (!pending) return;
  const resolve = pending;
  pending = null;
  confirmState.open = false;
  resolve(Boolean(value));
}
