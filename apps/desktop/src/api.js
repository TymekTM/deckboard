import { invoke } from "@tauri-apps/api/core";

export const api = {
  serverStatus: () => invoke("server_status"),
  listBoards: () => invoke("list_boards"),
  createBoard: (name, background, width, height) =>
    invoke("create_board", { name, background, width, height }),
  updateBoard: (board) => invoke("update_board", { board }),
  deleteBoard: (boardId) => invoke("delete_board", { boardId }),
  createButton: (boardId, kind, mode, x, y) =>
    invoke("create_button", { boardId, kind, mode, x, y }),
  updateButton: (button) => invoke("update_button", { button }),
  moveButton: (id, boardId, x, y, w, h) =>
    invoke("move_button", { id, boardId, x, y, w, h }),
  deleteButton: (id, boardId) => invoke("delete_button", { id, boardId }),
  clearBoard: (boardId) => invoke("clear_board", { boardId }),
  execButton: (id) => invoke("exec_button", { id }),
  execSlider: (id, value) => invoke("exec_slider", { id, value }),
  getSettings: () => invoke("get_settings"),
  setHotkey: (combo) => invoke("set_touch_mode_hotkey", { combo }),
  setServerPort: (port) => invoke("set_server_port", { port }),
  getAutostart: () => invoke("get_autostart"),
  setAutostart: (enable) => invoke("set_autostart", { enable }),
  readImageData: (path) => invoke("read_image_data", { path }),
  listKnownInputs: () => invoke("list_known_inputs"),
  listLanAddresses: () => invoke("list_lan_addresses"),
  createPairingCode: () => invoke("create_pairing_code"),
  listDevices: () => invoke("list_devices"),
  revokeDevice: (id) => invoke("revoke_device", { id }),
  listAudioDevices: () => invoke("list_audio_devices"),
  exportBoards: (ids, path) => invoke("export_boards", { ids, path }),
  importBoards: (path) => invoke("import_boards", { path }),
};
