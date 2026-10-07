import { describe, it, expect } from "vitest";
import { api } from "../src/api.js";
import { calls, handle } from "./tauri.js";

// The argument names are the contract with the #[tauri::command]
// signatures in src-tauri (camelCase on this side, snake_case in Rust):
// a renamed key silently turns into a missing-argument error at runtime.
describe("api", () => {
  it.each([
    ["serverStatus", [], "server_status", undefined],
    ["listBoards", [], "list_boards", undefined],
    ["createBoard", ["B", "#000", 4, 3], "create_board", { name: "B", background: "#000", width: 4, height: 3 }],
    ["updateBoard", [{ id: 1 }], "update_board", { board: { id: 1 } }],
    ["deleteBoard", [7], "delete_board", { boardId: 7 }],
    ["reorderBoards", [[3, 1, 2]], "reorder_boards", { orderedIds: [3, 1, 2] }],
    ["createButton", [1, "key", "button", 2, 3], "create_button", { boardId: 1, kind: "key", mode: "button", x: 2, y: 3 }],
    ["updateButton", [{ id: 9 }], "update_button", { button: { id: 9 } }],
    ["moveButton", [9, 1, 2, 3, 4, 5], "move_button", { id: 9, boardId: 1, x: 2, y: 3, w: 4, h: 5 }],
    ["deleteButton", [9, 1], "delete_button", { id: 9, boardId: 1 }],
    ["clearBoard", [1], "clear_board", { boardId: 1 }],
    ["execButton", [9], "exec_button", { id: 9 }],
    ["execSlider", [9, 0.25], "exec_slider", { id: 9, value: 0.25 }],
    ["getSettings", [], "get_settings", undefined],
    ["setHotkey", ["Ctrl+T"], "set_touch_mode_hotkey", { combo: "Ctrl+T" }],
    ["setServerPort", [8611], "set_server_port", { port: 8611 }],
    ["getAutostart", [], "get_autostart", undefined],
    ["setAutostart", [true], "set_autostart", { enable: true }],
    ["readImageData", ["C:/a.png"], "read_image_data", { path: "C:/a.png" }],
    ["listKnownInputs", [], "list_known_inputs", undefined],
    ["listLanAddresses", [], "list_lan_addresses", undefined],
    ["createPairingCode", [], "create_pairing_code", undefined],
    ["listDevices", [], "list_devices", undefined],
    ["revokeDevice", ["d1"], "revoke_device", { id: "d1" }],
    ["resolveOperatorAsk", [3, false], "resolve_operator_ask", { id: 3, approved: false }],
    ["takePendingTouchToggle", [], "take_pending_touch_toggle", undefined],
    ["adbDevices", [], "adb_devices", undefined],
    ["adbInstallApk", ["x.apk"], "adb_install_apk", { path: "x.apk" }],
    ["checkForUpdates", [], "check_for_updates", undefined],
    ["installUpdate", [], "install_update", undefined],
    ["listAudioDevices", [], "list_audio_devices", undefined],
    ["exportBoards", [[1, 2], "out.boardjson"], "export_boards", { ids: [1, 2], path: "out.boardjson" }],
    ["importBoards", ["in.boardjson"], "import_boards", { path: "in.boardjson" }],
    ["aidevStatusConfig", [], "aidev_status_config", undefined],
    ["setAidevStatusConfig", [true, false, "logo"], "set_aidev_status_config", { show: true, summary: false, rowStyle: "logo" }],
    ["spotifyStatus", [], "spotify_status", undefined],
    ["spotifySetClientId", ["abc"], "spotify_set_client_id", { clientId: "abc" }],
    ["spotifyLogin", [], "spotify_login", undefined],
    ["spotifyLogout", [], "spotify_logout", undefined],
    ["spotifyPlaylists", [], "spotify_playlists", undefined],
    ["spotifyDevices", [], "spotify_devices", undefined],
    ["assetDataUrl", ["h"], "asset_data_url", { hash: "h" }],
  ])("%s invokes %s", async (method, args, cmd, payload) => {
    await api[method](...args);
    expect(calls).toEqual([{ cmd, args: payload }]);
  });

  it("covers every method in the table above", () => {
    expect(Object.keys(api)).toHaveLength(42);
  });

  it("passes the command's result through", async () => {
    handle("list_boards", () => [{ id: 1, name: "Main" }]);
    await expect(api.listBoards()).resolves.toEqual([{ id: 1, name: "Main" }]);
  });

  it("propagates command errors", async () => {
    handle("delete_board", () => {
      throw new Error("last board");
    });
    await expect(api.deleteBoard(1)).rejects.toThrow("last board");
  });
});
