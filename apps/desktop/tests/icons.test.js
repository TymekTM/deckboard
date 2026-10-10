import { describe, it, expect } from "vitest";
import { ALL_ICONS, ICON_GROUPS, searchIcons } from "../src/icons.js";

describe("icon picker data", () => {
  it("curated groups only use icons from the shared set, without repeats", () => {
    const all = new Set(ALL_ICONS);
    const seen = new Set();
    for (const group of ICON_GROUPS) {
      for (const name of group.icons) {
        expect(all.has(name), name).toBe(true);
        expect(seen.has(name), `${name} listed twice`).toBe(false);
        seen.add(name);
      }
    }
    expect(seen.size).toBeGreaterThan(300);
  });

  it("keeps every icon the old picker offered", () => {
    const seen = new Set(ICON_GROUPS.flatMap((g) => g.icons));
    for (const name of ["keyboard", "power-off", "sliders-h", "puzzle-piece", "map-marker", "hand-pointer"]) {
      expect(seen.has(name), name).toBe(true);
    }
  });

  it("ALL_ICONS is sorted and unique", () => {
    expect(new Set(ALL_ICONS).size).toBe(ALL_ICONS.length);
    expect([...ALL_ICONS]).toEqual([...ALL_ICONS].sort());
  });

  it("search ranks prefix matches first and ignores case and the fa- prefix", () => {
    const hits = searchIcons("MIC");
    expect(hits[0].startsWith("mic")).toBe(true);
    expect(hits).toContain("microphone");
    expect(searchIcons("fa-bell")).toContain("bell-slash");
    expect(searchIcons("")).toEqual([]);
    expect(searchIcons("zzzzzz")).toEqual([]);
  });

  it("search respects the limit", () => {
    expect(searchIcons("a", 10)).toHaveLength(10);
  });
});
