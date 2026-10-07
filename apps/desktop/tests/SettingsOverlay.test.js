import { describe, it, expect, afterEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import SettingsOverlay from "../src/components/SettingsOverlay.vue";
import { calls, handle, dialog } from "./tauri.js";

// The overlay's script-setup bindings are reachable through wrapper.vm
// (test-utils proxies setupState), which keeps these tests on behavior -
// the api calls and the state the template renders - instead of on the
// tile wall's markup.

const status = { dbOk: true, port: 8500, clients: 0, version: "1.4.0" };

function defaults() {
  handle("get_settings", () => ({ hotkey: "Ctrl+Alt+D", port: 8500, port_locked: false }));
  handle("get_autostart", () => false);
  handle("list_lan_addresses", () => [{ ipv4: "192.168.1.10", name: "Wi-Fi", qr: "data:qr" }]);
  handle("list_devices", () => []);
  handle("aidev_status_config", () => ({ detected: [], show: [], summary: true, row_style: "name" }));
  handle("spotify_status", () => ({ configured: true, clientId: "", loggedIn: false }));
}

async function mountSettings(props = {}) {
  const w = mount(SettingsOverlay, { props: { status, ...props }, attachTo: document.body });
  await flushPromises();
  return w;
}

const cmds = (name) => calls.filter((c) => c.cmd === name);

afterEach(() => {
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("SettingsOverlay startup", () => {
  it("loads settings, autostart, addresses, devices, AI usage and Spotify", async () => {
    defaults();
    const w = await mountSettings();
    for (const cmd of [
      "get_settings",
      "get_autostart",
      "list_lan_addresses",
      "list_devices",
      "aidev_status_config",
      "spotify_status",
    ]) {
      expect(cmds(cmd), cmd).toHaveLength(1);
    }
    expect(w.vm.hotkey).toBe("Ctrl+Alt+D");
    expect(w.vm.portDraft).toBe("8500");
    expect(w.vm.portPending).toBeNull();
    expect(w.text()).toContain("192.168.1.10:8500");
    expect(w.find("img.qr").attributes("src")).toBe("data:qr");
  });

  it("resets the QR selection when the address list shrinks", async () => {
    defaults();
    handle("list_lan_addresses", () => [
      { ipv4: "10.0.0.1", name: "a", qr: "q1" },
      { ipv4: "10.0.0.2", name: "b", qr: "q2" },
    ]);
    const w = await mountSettings();
    await w.findAll("button.addr")[1].trigger("click");
    expect(w.find("img.qr").attributes("src")).toBe("q2");
    handle("list_lan_addresses", () => [{ ipv4: "10.0.0.1", name: "a", qr: "q1" }]);
    await w.vm.refreshLan();
    await flushPromises();
    expect(w.vm.activeAddress).toBe(0);
    expect(w.find("img.qr").attributes("src")).toBe("q1");
  });

  it("shows the no-network note when listing fails", async () => {
    defaults();
    handle("list_lan_addresses", () => {
      throw "no adapters";
    });
    const w = await mountSettings();
    expect(w.vm.lanAddresses).toEqual([]);
    expect(w.text()).toContain("nie widzi żadnej sieci");
  });

  it("flags a stored port that differs from the running one", async () => {
    defaults();
    handle("get_settings", () => ({ hotkey: "X", port: 9000 }));
    const w = await mountSettings();
    expect(w.vm.portPending).toBe(9000);
    expect(w.vm.portDraft).toBe("9000");
  });

  it("keeps defaults when the backend is unreachable", async () => {
    // An unreachable backend rejects every invoke instead of answering
    // null, so each loader must hit its catch path, not a null payload.
    const down = () => {
      throw "down";
    };
    for (const cmd of [
      "get_settings",
      "get_autostart",
      "list_lan_addresses",
      "list_devices",
      "aidev_status_config",
      "spotify_status",
    ]) {
      handle(cmd, down);
    }
    const w = await mountSettings();
    expect(w.vm.hotkey).toBe("Ctrl+Alt+D");
    expect(w.vm.autostart).toBe(false);
    expect(w.vm.lanAddresses).toEqual([]);
    expect(w.vm.devices).toEqual([]);
  });

  it("closes on Escape and the close button", async () => {
    defaults();
    const w = await mountSettings();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await w.find("button.close").trigger("click");
    expect(w.emitted("close")).toHaveLength(2);
    w.unmount();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
  });
});

describe("SettingsOverlay hotkey and autostart", () => {
  it("saves a trimmed hotkey", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.hotkeyDraft = "  Ctrl+Shift+T ";
    await w.vm.saveHotkey();
    expect(cmds("set_touch_mode_hotkey")[0].args).toEqual({ combo: "Ctrl+Shift+T" });
    expect(w.vm.hotkey).toBe("Ctrl+Shift+T");
    expect(w.vm.editingHotkey).toBe(false);
  });

  it("shows why a hotkey was refused", async () => {
    defaults();
    handle("set_touch_mode_hotkey", () => {
      throw "taken by another app";
    });
    const w = await mountSettings();
    w.vm.editingHotkey = true;
    w.vm.hotkeyDraft = "Ctrl+C";
    await w.vm.saveHotkey();
    await flushPromises();
    expect(w.vm.hotkey).toBe("Ctrl+Alt+D");
    expect(w.find(".err").text()).toBe("taken by another app");
  });

  it("flips autostart and reverts on failure", async () => {
    defaults();
    const w = await mountSettings();
    await w.vm.toggleAutostart();
    expect(cmds("set_autostart")[0].args).toEqual({ enable: true });
    expect(w.vm.autostart).toBe(true);
    handle("set_autostart", () => {
      throw "registry locked";
    });
    await w.vm.toggleAutostart();
    expect(w.vm.autostart).toBe(true);
    expect(w.vm.autostartError).toBe("registry locked");
  });
});

describe("SettingsOverlay port", () => {
  it.each(["80", "70000", "abc", "1024.5", ""])("rejects %j", async (draft) => {
    defaults();
    const w = await mountSettings();
    w.vm.portDraft = draft;
    await w.vm.savePort();
    expect(cmds("set_server_port")).toHaveLength(0);
    expect(w.vm.portError).toContain("1024");
  });

  it("stores a valid port as pending until restart", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.portDraft = " 9100 ";
    await w.vm.savePort();
    expect(cmds("set_server_port")[0].args).toEqual({ port: 9100 });
    expect(w.vm.portPending).toBe(9100);
    w.vm.portDraft = "8500";
    await w.vm.savePort();
    expect(w.vm.portPending).toBeNull();
  });

  it("shows the backend reason for a refused port", async () => {
    defaults();
    handle("set_server_port", () => {
      throw "port in use";
    });
    const w = await mountSettings();
    w.vm.portDraft = "9100";
    await w.vm.savePort();
    expect(w.vm.portError).toBe("port in use");
  });
});

describe("SettingsOverlay pairing", () => {
  it("mints a code and counts it down to expiry", async () => {
    vi.useFakeTimers();
    defaults();
    handle("create_pairing_code", () => ({ code: "ABC123", expires_in_secs: 3, addresses: [] }));
    const w = await mountSettings();
    await w.vm.generatePairing();
    expect(w.vm.pairingClock).toBe("0:03");
    await vi.advanceTimersByTimeAsync(1000);
    expect(w.vm.pairingClock).toBe("0:02");
    await vi.advanceTimersByTimeAsync(2000);
    expect(w.vm.pairingOffer).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("formats minutes in the countdown", async () => {
    defaults();
    handle("create_pairing_code", () => ({ code: "X", expires_in_secs: 300 }));
    const w = await mountSettings();
    await w.vm.generatePairing();
    expect(w.vm.pairingClock).toBe("5:00");
    w.unmount();
  });

  it("restarts the clock for a new code instead of stacking timers", async () => {
    vi.useFakeTimers();
    defaults();
    handle("create_pairing_code", () => ({ code: "X", expires_in_secs: 60 }));
    const w = await mountSettings();
    await w.vm.generatePairing();
    await w.vm.generatePairing();
    expect(vi.getTimerCount()).toBe(1);
    w.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("reports minting failures", async () => {
    defaults();
    handle("create_pairing_code", () => {
      throw "v2 stack down";
    });
    const w = await mountSettings();
    await w.vm.generatePairing();
    expect(w.vm.pairingError).toBe("v2 stack down");
    expect(w.vm.pairingBusy).toBe(false);
  });
});

describe("SettingsOverlay devices", () => {
  const device = { id: "d1", name: "Tablet", last_seen: 0 };

  it("revokes only after the in-app confirmation", async () => {
    defaults();
    handle("list_devices", () => [device]);
    const w = await mountSettings();
    w.vm.revokeDevice(device);
    await flushPromises();
    expect(cmds("revoke_device")).toHaveLength(0);
    expect(w.vm.confirmRevoke).toEqual(device);
    await w.vm.doRevoke();
    expect(cmds("revoke_device")[0].args).toEqual({ id: "d1" });
    expect(w.vm.confirmRevoke).toBeNull();
    // the list refetches after the revoke
    await flushPromises();
    expect(cmds("list_devices").length).toBeGreaterThanOrEqual(2);
  });

  it("keeps the confirmation open with the reason on failure", async () => {
    defaults();
    handle("revoke_device", () => {
      throw "unknown device";
    });
    const w = await mountSettings();
    w.vm.revokeDevice(device);
    await w.vm.doRevoke();
    expect(w.vm.confirmRevoke).toEqual(device);
    expect(w.vm.revokeError).toBe("unknown device");
  });

  it("closes the confirmation on Escape without closing settings", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.revokeDevice(device);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(w.vm.confirmRevoke).toBeNull();
    expect(w.emitted("close")).toBeUndefined();
  });

  it("shows no stale rows when listing fails", async () => {
    defaults();
    handle("list_devices", () => {
      throw "down";
    });
    const w = await mountSettings();
    expect(w.vm.devices).toEqual([]);
  });

  it("refetches devices when the devices tile is focused", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.focused = "devices";
    await flushPromises();
    expect(cmds("list_devices")).toHaveLength(2);
  });

  it.each([
    [0, "nigdy"],
    [-30, "teraz"],
    [-600, "10 min temu"],
    [-3 * 3600, "3 godz. temu"],
  ])("labels last-seen offset %d as %s", async (offset, label) => {
    defaults();
    const w = await mountSettings();
    const at = offset === 0 ? 0 : Date.now() / 1000 + offset;
    expect(w.vm.lastSeenLabel(at)).toBe(label);
  });

  it("falls back to a date for old devices", async () => {
    defaults();
    const w = await mountSettings();
    const at = Date.now() / 1000 - 30 * 24 * 3600;
    expect(w.vm.lastSeenLabel(at)).toBe(new Date(at * 1000).toLocaleDateString());
  });
});

describe("SettingsOverlay adb and updates", () => {
  it("installs a picked APK and refreshes the device list", async () => {
    defaults();
    dialog.open.mockResolvedValueOnce("C:/pulpit.apk");
    handle("adb_install_apk", () => "Success");
    handle("adb_devices", () => ["emulator-5554"]);
    const w = await mountSettings();
    await w.vm.installApk();
    expect(cmds("adb_install_apk")[0].args).toEqual({ path: "C:/pulpit.apk" });
    expect(w.vm.adbNote).toBe("Success");
    expect(w.vm.adbList).toEqual(["emulator-5554"]);
  });

  it("skips the install when the picker is cancelled", async () => {
    defaults();
    const w = await mountSettings();
    await w.vm.installApk();
    expect(cmds("adb_install_apk")).toHaveLength(0);
  });

  it("reports adb failures", async () => {
    defaults();
    handle("adb_devices", () => {
      throw "adb not found";
    });
    const w = await mountSettings();
    await w.vm.refreshAdb();
    expect(w.vm.adbList).toEqual([]);
    expect(w.vm.adbNote).toBe("adb not found");
    expect(w.vm.adbNoteBad).toBe(true);
  });

  it("announces an available update and installs it", async () => {
    defaults();
    handle("check_for_updates", () => ({ update_available: true, latest: "1.5.0", current: "1.4.0" }));
    handle("install_update", () => "Restarting...");
    const w = await mountSettings();
    await w.vm.checkUpdates();
    expect(w.vm.updateReady).toBe("1.5.0");
    expect(w.vm.updateNote).toBe("Dostępna wersja 1.5.0 (masz 1.4.0).");
    await w.vm.installUpdate();
    expect(w.vm.updateNote).toBe("Restarting...");
    // stays busy while the app restarts itself
    expect(w.vm.updateBusy).toBe(true);
  });

  it("says when the app is up to date", async () => {
    defaults();
    handle("check_for_updates", () => ({ update_available: false, latest: "1.4.0", current: "1.4.0" }));
    const w = await mountSettings();
    await w.vm.checkUpdates();
    expect(w.vm.updateReady).toBe("");
    expect(w.vm.updateNote).toBe("System jest aktualny (v1.4.0).");
  });

  it("re-enables the install button after a failed update", async () => {
    defaults();
    handle("install_update", () => {
      throw "sha256 mismatch";
    });
    const w = await mountSettings();
    await w.vm.installUpdate();
    expect(w.vm.updateNote).toBe("sha256 mismatch");
    expect(w.vm.updateNoteBad).toBe(true);
    expect(w.vm.updateBusy).toBe(false);
  });
});

describe("SettingsOverlay AI usage", () => {
  const detected = [
    { id: "glm:5h", label: "GLM 5h" },
    { id: "glm:week", label: "GLM week" },
    { id: "claude:5h", label: "Claude 5h" },
    { id: "mystery:x", label: "Mystery" },
  ];

  it("groups detected rows by provider and checks everything for an empty selection", async () => {
    defaults();
    handle("aidev_status_config", () => ({ detected, show: [], summary: false, row_style: "logo" }));
    const w = await mountSettings();
    expect(w.vm.aidevGroups.map((g) => [g.name, g.rows.length])).toEqual([
      ["GLM", 2],
      ["Claude", 1],
      ["mystery", 1],
    ]);
    expect(w.vm.aidevShow).toEqual(detected.map((r) => r.id));
    expect(w.vm.aidevSummary).toBe(false);
    expect(w.vm.aidevRowStyle).toBe("logo");
  });

  it("keeps only still-detected rows of a saved selection", async () => {
    defaults();
    handle("aidev_status_config", () => ({ detected, show: ["glm:5h", "gone:row"] }));
    const w = await mountSettings();
    expect(w.vm.aidevShow).toEqual(["glm:5h"]);
    expect(w.vm.aidevSummary).toBe(true);
    expect(w.vm.aidevRowStyle).toBe("name");
  });

  it("saves a partial selection explicitly and a full one as []", async () => {
    defaults();
    handle("aidev_status_config", () => ({ detected, show: [] }));
    const w = await mountSettings();
    w.vm.toggleAidevRow("glm:week");
    await flushPromises();
    expect(cmds("set_aidev_status_config").at(-1).args).toEqual({
      show: ["glm:5h", "claude:5h", "mystery:x"],
      summary: true,
      rowStyle: "name",
    });
    w.vm.toggleAidevRow("glm:week");
    await flushPromises();
    expect(cmds("set_aidev_status_config").at(-1).args.show).toEqual([]);
  });

  it("refuses to uncheck the last row", async () => {
    defaults();
    handle("aidev_status_config", () => ({ detected: [detected[0]], show: [] }));
    const w = await mountSettings();
    w.vm.toggleAidevRow("glm:5h");
    await flushPromises();
    expect(cmds("set_aidev_status_config")).toHaveLength(0);
    expect(w.vm.aidevShow).toEqual(["glm:5h"]);
    expect(w.vm.aidevNote).toContain("Przynajmniej jeden");
  });

  it("saves summary and row-style toggles, skipping no-op style changes", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.toggleAidevSummary();
    await flushPromises();
    w.vm.setAidevRowStyle("name");
    w.vm.setAidevRowStyle("logo");
    await flushPromises();
    const saves = cmds("set_aidev_status_config").map((c) => c.args);
    expect(saves).toHaveLength(2);
    expect(saves[0]).toMatchObject({ summary: false, rowStyle: "name" });
    expect(saves[1]).toMatchObject({ summary: false, rowStyle: "logo" });
  });

  it("reports save failures", async () => {
    defaults();
    handle("set_aidev_status_config", () => {
      throw "disk full";
    });
    const w = await mountSettings();
    await w.vm.saveAidev();
    expect(w.vm.aidevError).toBe("disk full");
    expect(w.vm.aidevBusy).toBe(false);
  });
});

describe("SettingsOverlay Spotify", () => {
  it("reads as logged out without a login", async () => {
    defaults();
    const w = await mountSettings();
    expect(w.vm.spotifyLoggedIn).toBe(false);
    expect(w.vm.spotifyChip).toEqual({ text: "wymaga logowania", ok: false });
  });

  it("describes a premium login with the live device", async () => {
    defaults();
    handle("spotify_status", () => ({ configured: true, clientId: "cid", loggedIn: true, user: "tymek", product: "premium" }));
    const w = await mountSettings({ customValues: { "spotify-device": "Desk" } });
    expect(w.vm.spotifyClientIdDraft).toBe("cid");
    expect(w.vm.spotifyStatusLine).toBe("Zalogowano jako tymek (Premium) — urządzenie: Desk");
    expect(w.vm.spotifyChip).toEqual({ text: "tymek", ok: true });
  });

  it("warns free accounts that playback control needs Premium", async () => {
    defaults();
    handle("spotify_status", () => ({ configured: true, loggedIn: true, product: "free" }));
    const w = await mountSettings();
    expect(w.vm.spotifyStatusLine).toBe("Zalogowano  (Free — sterowanie odtwarzaniem wymaga Premium)");
    expect(w.vm.spotifyChip.text).toBe("zalogowano");
  });

  it("lets live auth pushes override the fetched status", async () => {
    defaults();
    handle("spotify_status", () => ({ configured: true, loggedIn: true, user: "u" }));
    const w = await mountSettings({ customValues: { "spotify-auth": "needs-login" } });
    expect(w.vm.spotifyLoggedIn).toBe(false);
    expect(w.vm.spotifyStatusLine).toContain("Wymaga logowania");
    await w.setProps({ customValues: { "spotify-auth": "off" } });
    expect(w.vm.spotifyStatusLine).toContain("nieaktywne");
    expect(w.vm.spotifyChip).toEqual({ text: "wył.", ok: false });
    await w.setProps({ customValues: { "spotify-auth": "ok" } });
    expect(w.vm.spotifyLoggedIn).toBe(true);
  });

  it("reports a broken spotify.json", async () => {
    defaults();
    handle("spotify_status", () => ({ configured: false }));
    const w = await mountSettings({ customValues: { "spotify-auth": "ok" } });
    expect(w.vm.spotifyLoggedIn).toBe(false);
    expect(w.vm.spotifyStatusLine).toContain("uszkodzony");
    expect(w.vm.spotifyChip.text).toBe("wył.");
  });

  it("saves a trimmed client id and refreshes", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.spotifyClientIdDraft = "  abc  ";
    await w.vm.saveSpotifyClientId();
    expect(cmds("spotify_set_client_id")[0].args).toEqual({ clientId: "abc" });
    expect(cmds("spotify_status")).toHaveLength(2);
    expect(w.vm.spotifyNote).toBe("Zapisano Client ID.");
  });

  it("logs in and out", async () => {
    defaults();
    handle("spotify_login", () => ({ user: "tymek" }));
    const w = await mountSettings();
    await w.vm.doSpotifyLogin();
    expect(w.vm.spotifyNote).toBe("Zalogowano jako tymek.");
    await w.vm.doSpotifyLogout();
    expect(cmds("spotify_logout")).toHaveLength(1);
    expect(w.vm.spotifyNote).toContain("Wylogowano");
    expect(w.vm.spotifyBusy).toBe(false);
  });

  it("shows login failures", async () => {
    defaults();
    handle("spotify_login", () => {
      throw "login timed out";
    });
    const w = await mountSettings();
    await w.vm.doSpotifyLogin();
    expect(w.vm.spotifyError).toBe("login timed out");
  });

  it("copies the redirect URI", async () => {
    defaults();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const w = await mountSettings();
    await w.vm.copyRedirectUri();
    expect(writeText).toHaveBeenCalledWith("http://127.0.0.1:8502/spotify/callback");
    expect(w.vm.spotifyNote).toBe("Skopiowano redirect URI.");
  });

  it("refetches Spotify status when its tile is focused", async () => {
    defaults();
    const w = await mountSettings();
    w.vm.focused = "spotify";
    await flushPromises();
    expect(cmds("spotify_status")).toHaveLength(2);
  });
});
