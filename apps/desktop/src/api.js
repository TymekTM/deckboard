import { invoke } from "@tauri-apps/api/core";
import { ref } from "vue";

export const vmDevicesState = ref({ strips: [], buses: [] });

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
  // tool tiles (timer/stopwatch/counter): alternative triggers of the
  // tile's action - double-tap / long-press / reset gestures
  execButtonGesture: (id, gesture) =>
    invoke("exec_button_gesture", { id, gesture }),
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
  resolveOperatorAsk: (id, approved) => invoke("resolve_operator_ask", { id, approved }),
  takePendingTouchToggle: () => invoke("take_pending_touch_toggle"),
  adbDevices: () => invoke("adb_devices"),
  adbInstallApk: (path) => invoke("adb_install_apk", { path }),
  checkForUpdates: () => invoke("check_for_updates"),
  installUpdate: () => invoke("install_update"),
  listAudioDevices: () => invoke("list_audio_devices"),
  exportBoards: (ids, path) => invoke("export_boards", { ids, path }),
  importBoards: (path) => invoke("import_boards", { path }),
  aidevStatusConfig: () => invoke("aidev_status_config"),
  setAidevStatusConfig: (show, summary, rowStyle) =>
    invoke("set_aidev_status_config", { show, summary, rowStyle }),
  spotifyStatus: () => invoke("spotify_status"),
  spotifySetClientId: (clientId) => invoke("spotify_set_client_id", { clientId }),
  spotifyLogin: () => invoke("spotify_login"),
  spotifyLogout: () => invoke("spotify_logout"),
  spotifyPlaylists: () => invoke("spotify_playlists"),
  spotifyDevices: () => invoke("spotify_devices"),
  mediaSessions: () => invoke("media_sessions"),
  obsStatus: () => invoke("obs_status"),
  obsApplyConfig: (enabled, host, port, password) =>
    invoke("obs_apply_config", { enabled, host, port, password }),
  obsTestConnection: (host, port, password) =>
    invoke("obs_test_connection", { host, port, password }),
  obsChoices: () => invoke("obs_choices"),
  assetDataUrl: (hash) => invoke("asset_data_url", { hash }),
  discordStatus: () => invoke("discord_status"),
  discordSaveConfig: (clientId, clientSecret) =>
    invoke("discord_save_config", { clientId, clientSecret }),
  discordAuthorize: () => invoke("discord_authorize"),
  discordDisconnect: () => invoke("discord_disconnect"),
  vmStatus: () => invoke("vm_status"),
  vmSetDllOverride: (path) => invoke("vm_set_dll_override", { path }),
  vmReconnect: () => invoke("vm_reconnect"),
  vmRun: (vmType) => invoke("vm_run", { vmType }),
  vmDevices: () => invoke("vm_devices"),
  execButtonGesture: (id, gesture) => invoke("exec_button_gesture", { id, gesture }),
};

export async function refreshVmDevices() {
  try {
    const res = await api.vmDevices();
    vmDevicesState.value = res || { strips: [], buses: [] };
  } catch {
    vmDevicesState.value = { strips: [], buses: [] };
  }
}
