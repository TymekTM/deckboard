//! The Spotify producer's host lane: the poller (`pulpit_spotify::
//! spawn_push`) publishes design-§4 snapshots, this side strips the
//! host-consumed internal key, imports album art into the v2
//! [`pulpit_v2::AssetStore`] and forwards the rest through the same
//! three client lanes as every other producer.
//!
//! The internal `spotify-art-url` key (the raw cover URL) must NEVER
//! reach a client: it is stripped from every snapshot before anything
//! is forwarded. The URL is resolved to a content-addressed asset hash
//! once per album (LRU of the last 32, evicted entries are deleted from
//! the store again - the Spotify ToS allows temporary caching only),
//! and the hash lands in the now-playing payload's `image` field.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::{forward_snapshot, ClientFeed, ProducerGate, StatusApp};

/// The poller's host-consumed key carrying the raw cover URL
/// (`crates/spotify/src/snapshot.rs`). Internal: stripped before any
/// forward, never rendered.
pub const ART_URL_KEY: &str = "spotify-art-url";

/// LRU cap (design §4: keep the last 32 covers, delete evicted ones).
const ART_LRU_CAP: usize = 32;

/// One art fetch: URL -> `(bytes, file ext)`. The real one downloads
/// over the shared agent; tests script it.
type ArtFetch = Arc<dyn Fn(&str) -> Option<(Vec<u8>, &'static str)> + Send + Sync>;

/// Download cap. A 300 px cover is tens of KB; 2 MiB bounds a hostile
/// or misbehaving answer without ever pinning a real one.
const MAX_ART_BYTES: u64 = 2 * 1024 * 1024;

/// Remove the internal art key from a snapshot, returning its value
/// (`None` when absent, `Some("")` when there is no art). The key must
/// never reach clients, so this runs on every snapshot whatever the
/// AssetStore situation is.
pub fn take_art_url(snapshot: &mut Value) -> Option<String> {
    snapshot
        .as_object_mut()
        .and_then(|map| map.remove(ART_URL_KEY))
        .and_then(|v| v.as_str().map(str::to_string))
}

/// Put the art hash into the now-playing payload's `image` field
/// (design §4). `None` removes a stale field, so "nothing playing"
/// never keeps an old cover around.
fn attach_image(snapshot: &mut Value, hash: Option<&str>) {
    let Some(payload) = snapshot.get_mut("spotify-now-playing") else {
        return;
    };
    match hash {
        Some(hash) => payload["image"] = Value::String(hash.to_string()),
        None => {
            if let Some(map) = payload.as_object_mut() {
                map.remove("image");
            }
        }
    }
}

/// url -> hash, oldest first. A `VecDeque` because the cap is 32: a
/// proper LRU structure would outlive its justification here.
/// Pure so the eviction contract is unit-testable.
fn lru_lookup(entries: &mut VecDeque<(String, String)>, url: &str) -> Option<String> {
    let index = entries.iter().position(|(u, _)| u == url)?;
    let (_, hash) = entries.remove(index).expect("position just found");
    entries.push_back((url.to_string(), hash.clone())); // promote to newest
    Some(hash)
}

/// Insert a freshly imported entry and evict past the cap, handing every
/// evicted hash not still referenced by another URL to `remove` (the
/// caller deletes it from the AssetStore, which refuses any hash a board
/// image resolved to - covers are content-addressed and can be
/// byte-identical to a board image).
fn lru_remember(
    entries: &mut VecDeque<(String, String)>,
    url: String,
    hash: String,
    remove: &mut dyn FnMut(&str),
) {
    if let Some(index) = entries.iter().position(|(u, _)| *u == url) {
        entries.remove(index); // re-import of a known URL: refresh position
    }
    entries.push_back((url, hash));
    while entries.len() > ART_LRU_CAP {
        let (_, evicted) = entries.pop_front().expect("len above cap");
        let still_referenced = entries.iter().any(|(_, h)| h == &evicted);
        if !still_referenced {
            remove(&evicted);
        }
    }
}

/// One album-art resolve: LRU hit, else fetch + import + remember.
/// `fetch` returns `(bytes, file ext)`; the real one downloads over the
/// shared agent, tests script it. Every failure leaves the payload
/// text-only and simply retries on a later poll.
async fn resolve_art(
    lru: &Mutex<VecDeque<(String, String)>>,
    url: Option<String>,
    assets: Option<&Arc<pulpit_v2::AssetStore>>,
    fetch: ArtFetch,
) -> Option<String> {
    let url = url.filter(|u| !u.is_empty())?;
    let assets = assets?;
    if let Some(hash) = lru_lookup(&mut lru.lock().unwrap(), &url) {
        return Some(hash);
    }
    // download + import off the async runtime (ureq blocks)
    let (bytes, ext) = tokio::task::spawn_blocking({
        let url = url.clone();
        move || fetch(&url)
    })
    .await
    .ok()??;
    let hash = assets.import_bytes(&bytes, ext).ok()?;
    let store = assets.clone();
    lru_remember(
        &mut lru.lock().unwrap(),
        url,
        hash.clone(),
        &mut |evicted| {
            if store.remove(evicted) {
                tracing::debug!(
                    hash = evicted,
                    "evicted spotify album art from the asset store"
                );
            }
        },
    );
    Some(hash)
}

/// The real fetch: GET over the shared agent, body capped at 2 MiB, the
/// file extension sniffed from magic bytes (Spotify's CDN serves jpeg).
fn download_art(agent: &ureq::Agent, url: &str) -> Option<(Vec<u8>, &'static str)> {
    let response = match agent.get(url).call() {
        Ok(resp) => resp,
        Err(e) => {
            tracing::debug!(url, error = %e, "spotify art download failed");
            return None;
        }
    };
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut bytes = Vec::new();
    use std::io::Read as _;
    if let Err(e) = response
        .into_body()
        .into_reader()
        .take(MAX_ART_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        tracing::debug!(url, error = %e, "spotify art body read failed");
        return None;
    }
    if bytes.len() as u64 > MAX_ART_BYTES {
        tracing::warn!(url, "spotify art larger than the 2 MiB cap - skipped");
        return None;
    }
    let ext = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && bytes[8..].starts_with(b"WEBP") {
        "webp"
    } else if content_type.starts_with("image/png") {
        "png"
    } else if content_type.starts_with("image/webp") {
        "webp"
    } else {
        "jpg"
    };
    Some((bytes, ext))
}

/// The download agent for art fetches (one per pump, so connections
/// are reused across album changes), from the workspace's shared
/// constructor (timeouts, user agent).
fn art_agent() -> ureq::Agent {
    pulpit_db::http_agent(Duration::from_secs(15), false)
}

/// The Spotify producer pump: one poller receiver in, the usual three
/// client lanes out ([`forward_snapshot`] under [`StatusApp::CustomValue`],
/// like every custom-value producer). `assets` is `None` when the v2
/// stack failed to start - snapshots are forwarded without `image` then.
pub async fn forward_spotify<F: ClientFeed>(
    feed: Arc<F>,
    rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
    assets: Option<Arc<pulpit_v2::AssetStore>>,
) {
    let agent = art_agent();
    let fetch: ArtFetch = Arc::new(move |url: &str| download_art(&agent, url));
    forward_spotify_with(feed, rx, assets, fetch).await;
}

/// The pump body with the fetch seam exposed (tests script it, no
/// network). Per snapshot: strip the internal art key, resolve it to an
/// asset hash (LRU-cached download + import), stamp the hash into the
/// now-playing payload, then forward.
async fn forward_spotify_with<F: ClientFeed>(
    feed: Arc<F>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
    assets: Option<Arc<pulpit_v2::AssetStore>>,
    fetch: ArtFetch,
) {
    let lru = Mutex::new(VecDeque::new());
    let mut gate = ProducerGate::default();
    while let Some(mut snapshot) = rx.recv().await {
        let art_url = take_art_url(&mut snapshot);
        let hash = resolve_art(&lru, art_url, assets.as_ref(), fetch.clone()).await;
        attach_image(&mut snapshot, hash.as_deref());
        forward_snapshot(&*feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
    }
}

/// The snapshot pushed once when Spotify is disabled (no/empty
/// `spotify.json`): design §4 defines `"off"` as "no configuration at
/// all, so no poller exists" and has the host - not the poller - say it.
pub fn disabled_snapshot() -> Value {
    serde_json::json!({ "spotify-auth": "off" })
}

/// One disabled-state push: the `spotify-auth: "off"` marker through the
/// same lanes, so a `spotify-auth` tile shows a defined value instead of
/// nothing. No poller exists to send anything else.
pub async fn forward_spotify_disabled<F: ClientFeed>(feed: Arc<F>) {
    let mut gate = ProducerGate::default();
    let mut snapshot = disabled_snapshot();
    let _art_url = take_art_url(&mut snapshot);
    forward_snapshot(&*feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use pulpit_v2::StateEngine;

    /// Records every lane (the lib.rs FakeFeed shape, local so this
    /// module's tests stay self-contained).
    struct FakeFeed {
        engine: StateEngine,
        broadcasts: Mutex<Vec<String>>,
        emits: Mutex<Vec<Value>>,
    }

    impl ClientFeed for FakeFeed {
        fn engine_set(&self, key: &str, value: Value) {
            self.engine.set(&pulpit_v2::ext_channel(key), value);
        }
        async fn broadcast_status(&self, payload: &str) {
            self.broadcasts.lock().unwrap().push(payload.to_string());
        }
        fn emit_status(&self, payload: &Value) -> bool {
            self.emits.lock().unwrap().push(payload.clone());
            true
        }
    }

    fn feed() -> Arc<FakeFeed> {
        Arc::new(FakeFeed {
            engine: StateEngine::new(64),
            broadcasts: Mutex::new(Vec::new()),
            emits: Mutex::new(Vec::new()),
        })
    }

    fn snapshot_with_art() -> Value {
        serde_json::json!({
            "spotify-playing": "ON",
            "spotify-auth": "ok",
            "spotify-art-url": "https://i.scdn.co/image/300",
            "spotify-now-playing": { "title": "Spotify", "rows": [] },
        })
    }

    #[test]
    fn take_art_url_removes_the_internal_key_and_returns_it() {
        let mut snapshot = snapshot_with_art();
        assert_eq!(
            take_art_url(&mut snapshot).as_deref(),
            Some("https://i.scdn.co/image/300")
        );
        // stripped on this and later calls, whatever the value
        assert_eq!(take_art_url(&mut snapshot), None);
        assert!(snapshot.get(ART_URL_KEY).is_none());
        // absent key: None, snapshot untouched
        let mut bare = serde_json::json!({ "spotify-auth": "off" });
        assert_eq!(take_art_url(&mut bare), None);
        assert_eq!(bare["spotify-auth"], "off");
    }

    #[test]
    fn attach_image_sets_and_clears_the_payload_field() {
        let mut snapshot = snapshot_with_art();
        take_art_url(&mut snapshot);
        attach_image(&mut snapshot, Some("ab12"));
        assert_eq!(snapshot["spotify-now-playing"]["image"], "ab12");
        // no art: the field must go, not stay stale
        attach_image(&mut snapshot, None);
        assert!(snapshot["spotify-now-playing"].get("image").is_none());
        // a snapshot without a now-playing payload is left alone
        let mut bare = serde_json::json!({ "spotify-auth": "off" });
        attach_image(&mut bare, Some("ab12"));
        assert!(bare.get("spotify-now-playing").is_none());
    }

    #[test]
    fn lru_lookup_promotes_and_misses_cleanly() {
        let mut entries: VecDeque<(String, String)> = [
            ("u1".to_string(), "h1".to_string()),
            ("u2".to_string(), "h2".to_string()),
        ]
        .into();
        assert_eq!(lru_lookup(&mut entries, "u2").as_deref(), Some("h2"));
        // u2 was promoted: u1 is now the oldest
        assert_eq!(entries.front().unwrap().0, "u1");
        assert_eq!(lru_lookup(&mut entries, "nope"), None);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn lru_remember_evicts_oldest_and_deletes_unreferenced_hashes() {
        let deleted: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let mut entries: VecDeque<(String, String)> = VecDeque::new();
        let mut remove = |hash: &str| deleted.borrow_mut().push(hash.to_string());

        for i in 0..ART_LRU_CAP {
            lru_remember(&mut entries, format!("u{i}"), format!("h{i}"), &mut remove);
        }
        assert!(deleted.borrow().is_empty());
        // one more entry evicts the oldest (h0) - deleted through remove
        lru_remember(
            &mut entries,
            "new".to_string(),
            "hnew".to_string(),
            &mut remove,
        );
        assert_eq!(*deleted.borrow(), ["h0"]);
        assert_eq!(entries.len(), ART_LRU_CAP);
        assert!(lru_lookup(&mut entries, "u0").is_none());
        assert_eq!(lru_lookup(&mut entries, "new").as_deref(), Some("hnew"));

        // a hash still referenced by a second URL must survive eviction
        // while any URL entry remains, and go once the last one leaves
        let deleted: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let mut entries: VecDeque<(String, String)> = VecDeque::new();
        let mut remove = |hash: &str| deleted.borrow_mut().push(hash.to_string());
        lru_remember(
            &mut entries,
            "a".to_string(),
            "shared".to_string(),
            &mut remove,
        );
        lru_remember(
            &mut entries,
            "b".to_string(),
            "shared".to_string(),
            &mut remove,
        );
        for i in 0..ART_LRU_CAP {
            lru_remember(&mut entries, format!("x{i}"), format!("hx{i}"), &mut remove);
        }
        // 34 entries -> the loop evicted a and b; "shared" only left with
        // the SECOND one (the last reference), deleted exactly once
        assert_eq!(*deleted.borrow(), ["shared"]);
        // the next eviction deletes the now-oldest unreferenced hash
        lru_remember(&mut entries, "y".to_string(), "hy".to_string(), &mut remove);
        assert_eq!(*deleted.borrow(), ["shared", "hx0"]);
    }

    #[tokio::test]
    async fn forwarded_snapshots_carry_the_image_hash_never_the_art_url() {
        let dir = tempfile::tempdir().unwrap();
        let assets = Arc::new(pulpit_v2::AssetStore::open(dir.path().to_path_buf()).unwrap());
        let feed = feed();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetch_calls = calls.clone();
        let fetch: ArtFetch = Arc::new(move |url: &str| {
            fetch_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some((
                format!("bytes-of-{url}").into_bytes(),
                "jpg" as &'static str,
            ))
        });
        let pump_feed = feed.clone();
        let assets_for_pump = Some(assets.clone());
        let pump = tokio::spawn(async move {
            forward_spotify_with(pump_feed, rx, assets_for_pump, fetch).await;
        });

        tx.send(snapshot_with_art()).unwrap();
        // second track, SAME album url: the LRU must answer, no second
        // download (the now-playing payload differs, so it forwards)
        tx.send(serde_json::json!({
            "spotify-playing": "OFF",
            "spotify-auth": "ok",
            "spotify-art-url": "https://i.scdn.co/image/300",
            "spotify-now-playing": { "title": "Spotify", "rows": [], "compact": "Track 2" },
        }))
        .unwrap();
        // no art: image field cleared, nothing crashes
        tx.send(serde_json::json!({
            "spotify-auth": "ok",
            "spotify-now-playing": { "title": "Spotify", "rows": [] },
        }))
        .unwrap();
        drop(tx);
        pump.await.unwrap();

        // downloaded exactly once for the album
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);

        // v2 lane: the internal channel never exists
        let sync = feed.engine.snapshot();
        assert!(!sync.values.contains_key("ext.spotify-art-url"));
        assert!(sync.values.contains_key("ext.spotify-playing"));

        // the forwarded now-playing payloads carry the same asset hash
        // for both tracks (content-addressed, one download), and the
        // art-less snapshot has no image field
        let emits = feed.emits.lock().unwrap();
        fn data(emit: &Value) -> &Value {
            &emit["data"]
        }
        let first = data(&emits[0])["spotify-now-playing"]["image"]
            .as_str()
            .expect("image hash attached")
            .to_string();
        assert_eq!(first.len(), 64, "sha256 hex from import_bytes");
        assert_eq!(
            data(&emits[1])["spotify-now-playing"]["image"].as_str(),
            Some(first.as_str()),
            "same album resolves to the same hash"
        );
        assert!(data(&emits[2])["spotify-now-playing"]
            .get("image")
            .is_none());

        // the art landed in the store exactly once
        assert_eq!(
            assets.get(&first).unwrap(),
            b"bytes-of-https://i.scdn.co/image/300".to_vec()
        );
    }

    #[tokio::test]
    async fn failed_or_unstoreable_art_leaves_the_payload_text_only() {
        // no AssetStore (v2 stack down): no download, no image
        let feed = feed();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let fetch: ArtFetch = Arc::new(|_url: &str| panic!("must not download without a store"));
        let pump_feed = feed.clone();
        let pump = tokio::spawn(async move {
            forward_spotify_with(pump_feed, rx, None, fetch).await;
        });
        tx.send(snapshot_with_art()).unwrap();
        drop(tx);
        pump.await.unwrap();
        let emits = feed.emits.lock().unwrap();
        assert!(emits[0]["data"]["spotify-now-playing"]
            .get("image")
            .is_none());
        // the art URL is still stripped even with no store
        assert!(emits[0]["data"].get("spotify-art-url").is_none());
    }

    #[tokio::test]
    async fn eviction_removes_only_the_hashes_this_pump_created() {
        let dir = tempfile::tempdir().unwrap();
        let assets = Arc::new(pulpit_v2::AssetStore::open(dir.path().to_path_buf()).unwrap());
        // a board image the pump never touched must survive the LRU churn
        let board_hash = assets.import_bytes(b"board-image", "png").unwrap();

        let feed = feed();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let fetch: ArtFetch =
            Arc::new(|url: &str| Some((format!("art-{url}").into_bytes(), "jpg" as &'static str)));
        let assets_for_pump = Some(assets.clone());
        let pump_feed = feed.clone();
        let pump = tokio::spawn(async move {
            forward_spotify_with(pump_feed, rx, assets_for_pump, fetch).await;
        });
        let count = ART_LRU_CAP + 4;
        for i in 0..count {
            tx.send(serde_json::json!({
                "spotify-art-url": format!("https://i.scdn.co/image/{i}"),
                "spotify-now-playing": { "title": "Spotify", "rows": [] },
            }))
            .unwrap();
        }
        drop(tx);
        pump.await.unwrap();

        // one payload per URL, in order: read each snapshot's hash back
        let emits = feed.emits.lock().unwrap();
        assert_eq!(emits.len(), count);
        let hash_of = |i: usize| {
            emits[i]["data"]["spotify-now-playing"]["image"]
                .as_str()
                .expect("art hash attached")
                .to_string()
        };
        // the LRU holds the last 32: the first 4 hashes were evicted and
        // deleted from the store, everything newer is still served
        assert!(assets.get(&hash_of(0)).is_none(), "evicted art is deleted");
        assert!(assets.get(&hash_of(3)).is_none());
        assert!(assets.get(&hash_of(4)).is_some());
        assert!(assets.get(&hash_of(count - 1)).is_some());
        // the board image the pump never created was never touched
        assert!(assets.get(&board_hash).is_some());
    }

    #[tokio::test]
    async fn disabled_push_marks_auth_off_only() {
        let feed = feed();
        forward_spotify_disabled(feed.clone()).await;
        let sync = feed.engine.snapshot();
        assert_eq!(sync.values["ext.spotify-auth"], serde_json::json!("off"));
        // exactly one key: a disabled integration pushes no playback state
        assert_eq!(sync.values.len(), 1);
    }
}
