// Shared module state for status payloads that carry album art and/or
// playback progress (spotify-now-playing, but generic: any producer's
// status push with these optional fields renders them).

import { ref } from "vue";
import { api } from "./api";

// One window-visibility mirror for every TileCell: the progress tick may
// only run while the window is actually on screen. The app already
// buffers status pushes behind a hidden window (App.vue), this mirrors
// the same fact for the per-tile ticker.
export const windowVisible = ref(
  typeof document === "undefined" || !document.hidden,
);
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    windowVisible.value = !document.hidden;
  });
}

// hash -> data URL (null = lookup failed and stays failed; the backend
// validates the hash, so a failure means the asset is gone). Module
// level: every TileCell in the WebView shares the memo, so re-renders
// and sibling tiles on the same album never refetch. Capped like the
// host's art LRU: a long listening session would otherwise keep every
// cover's data URL alive in the WebView.
const ART_CACHE_CAP = 32;
const artUrlCache = new Map();

function rememberArt(hash, url) {
  artUrlCache.delete(hash);
  artUrlCache.set(hash, url);
  while (artUrlCache.size > ART_CACHE_CAP) {
    artUrlCache.delete(artUrlCache.keys().next().value);
  }
}
const artPending = new Map();

/** Resolves a status payload's asset hash through the asset_data_url
 *  Tauri command, memoized per hash. Calls `done` with the data URL or
 *  null - dropped lookups resolve as null and the tile simply renders
 *  without art. */
export function resolveStatusArt(hash, done) {
  if (!hash) {
    done(null);
    return;
  }
  const cached = artUrlCache.get(hash);
  if (cached !== undefined) {
    done(cached);
    return;
  }
  let pending = artPending.get(hash);
  if (!pending) {
    pending = api
      .assetDataUrl(hash)
      .then((url) => {
        rememberArt(hash, url);
        return url;
      })
      .catch(() => {
        rememberArt(hash, null);
        return null;
      })
      .finally(() => artPending.delete(hash));
    artPending.set(hash, pending);
  }
  pending.then(done);
}

// Local receive-time stamp per status payload (design §4): no server
// timestamp rides the wire because clocks differ. Identity-stable
// payloads keep their stamp - mergeCustomValues keeps object identity
// for unchanged pushes, so a re-render never restarts the clock - and a
// changed payload is a new object with a fresh stamp.
const receivedAt = new WeakMap();

/** The local arrival time of [payload], stamped on first sight. */
export function payloadReceivedAt(payload) {
  let at = receivedAt.get(payload);
  if (at === undefined) {
    at = Date.now();
    receivedAt.set(payload, at);
  }
  return at;
}
