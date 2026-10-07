import { describe, it, expect, vi, beforeEach } from "vitest";

// statusMedia keeps module-level memo state, so each test loads a fresh
// copy of it - and of the Tauri stand-in, which the reset reloads too.
let media;
let calls;
let handle;
beforeEach(async () => {
  vi.resetModules();
  ({ calls, handle } = await import("./tauri.js"));
  media = await import("../src/statusMedia.js");
});

const resolve = (hash) => new Promise((r) => media.resolveStatusArt(hash, r));
const artCalls = () => calls.filter((c) => c.cmd === "asset_data_url");

describe("resolveStatusArt", () => {
  it("answers null for an empty hash without a lookup", async () => {
    await expect(resolve("")).resolves.toBeNull();
    await expect(resolve(undefined)).resolves.toBeNull();
    expect(artCalls()).toHaveLength(0);
  });

  it("looks a hash up once and memoizes the data URL", async () => {
    handle("asset_data_url", ({ hash }) => `data:image/png;base64,${hash}`);
    await expect(resolve("abc")).resolves.toBe("data:image/png;base64,abc");
    await expect(resolve("abc")).resolves.toBe("data:image/png;base64,abc");
    expect(artCalls()).toEqual([{ cmd: "asset_data_url", args: { hash: "abc" } }]);
  });

  it("answers a memo hit synchronously", async () => {
    handle("asset_data_url", () => "data:x");
    await resolve("h");
    let got;
    media.resolveStatusArt("h", (url) => (got = url));
    expect(got).toBe("data:x");
  });

  it("shares one in-flight lookup between concurrent callers", async () => {
    let release;
    handle("asset_data_url", () => new Promise((r) => (release = r)));
    const a = resolve("same");
    const b = resolve("same");
    await Promise.resolve();
    release("data:shared");
    await expect(Promise.all([a, b])).resolves.toEqual(["data:shared", "data:shared"]);
    expect(artCalls()).toHaveLength(1);
  });

  it("remembers a failed lookup as null and never retries", async () => {
    handle("asset_data_url", () => {
      throw new Error("gone");
    });
    await expect(resolve("dead")).resolves.toBeNull();
    await expect(resolve("dead")).resolves.toBeNull();
    expect(artCalls()).toHaveLength(1);
  });

  it("caps the memo at 32 covers, dropping the oldest insertion", async () => {
    handle("asset_data_url", ({ hash }) => `url-${hash}`);
    for (let i = 0; i < 33; i++) await resolve(`h${i}`);
    expect(artCalls()).toHaveLength(33);
    // h1..h32 are still memoized
    await resolve("h32");
    await resolve("h1");
    expect(artCalls()).toHaveLength(33);
    // h0 fell out and costs a new lookup; a memo hit does not refresh
    // recency, only a (re)insert does
    await resolve("h0");
    expect(artCalls()).toHaveLength(34);
  });
});

describe("payloadReceivedAt", () => {
  it("stamps a payload once and keeps the stamp per identity", () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(1000);
      const payload = { rows: [] };
      expect(media.payloadReceivedAt(payload)).toBe(1000);
      vi.setSystemTime(5000);
      expect(media.payloadReceivedAt(payload)).toBe(1000);
      // an equal but new object is a new push with a fresh stamp
      expect(media.payloadReceivedAt({ rows: [] })).toBe(5000);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("windowVisible", () => {
  it("mirrors document visibility", async () => {
    expect(media.windowVisible.value).toBe(!document.hidden);
    const spy = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(media.windowVisible.value).toBe(false);
    spy.mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(media.windowVisible.value).toBe(true);
  });
});
