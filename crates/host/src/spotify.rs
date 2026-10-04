//! The Spotify producer's host lane: the poller (`pulpit_spotify::
//! spawn_push`) publishes design-§4 snapshots, this side strips the
//! host-consumed internal key and forwards the rest through the same
//! three client lanes as every other producer.
//!
//! The internal `spotify-art-url` key (the raw cover URL) must NEVER
//! reach a client: it is stripped from every snapshot before anything
//! is forwarded. Album art import into the v2 [`pulpit_v2::AssetStore`]
//! rides the same pump (see [`forward_spotify`]).

use std::sync::Arc;

use serde_json::Value;

use crate::{ClientFeed, ProducerGate, StatusApp, forward_snapshot};

/// The poller's host-consumed key carrying the raw cover URL
/// (`crates/spotify/src/snapshot.rs`). Internal: stripped before any
/// forward, never rendered.
pub const ART_URL_KEY: &str = "spotify-art-url";

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


/// The snapshot pushed once when Spotify is disabled (no/empty
/// `spotify.json`): design §4 defines `"off"` as "no configuration at
/// all, so no poller exists" and has the host - not the poller - say it.
pub fn disabled_snapshot() -> Value {
    serde_json::json!({ "spotify-auth": "off" })
}

/// The Spotify producer pump: one poller receiver in, the usual three
/// client lanes out ([`crate::forward_snapshot`] under
/// [`StatusApp::CustomValue`], like every custom-value producer).
/// `assets` is `None` when the v2 stack failed to start - snapshots are
/// forwarded text-only then.
pub async fn forward_spotify<F: ClientFeed>(
    feed: Arc<F>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
    _assets: Option<Arc<pulpit_v2::AssetStore>>,
) {
    let mut gate = ProducerGate::default();
    while let Some(mut snapshot) = rx.recv().await {
        let _art_url = take_art_url(&mut snapshot);
        forward_snapshot_stripped(&*feed, &mut gate, &snapshot).await;
    }
}

/// One disabled-state push: the `spotify-auth: "off"` marker through the
/// same lanes, so a `spotify-auth` tile shows a defined value instead of
/// nothing. No poller exists to send anything else.
pub async fn forward_spotify_disabled<F: ClientFeed>(feed: Arc<F>) {
    let mut gate = ProducerGate::default();
    let mut snapshot = disabled_snapshot();
    let _art_url = take_art_url(&mut snapshot);
    forward_snapshot_stripped(&*feed, &mut gate, &snapshot).await;
}

async fn forward_snapshot_stripped<F: ClientFeed>(
    feed: &F,
    gate: &mut ProducerGate,
    snapshot: &Value,
) {
    forward_snapshot(feed, StatusApp::CustomValue, gate, snapshot).await;
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

    #[tokio::test]
    async fn forwarded_snapshots_never_carry_the_art_url() {
        let feed = feed();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(snapshot_with_art()).unwrap();
        tx.send(disabled_snapshot()).unwrap();
        drop(tx);
        forward_spotify(feed.clone(), rx, None).await;

        // v2 lane: only the design keys, no spotify-art-url channel
        let sync = feed.engine.snapshot();
        assert!(sync.values.contains_key("ext.spotify-playing"));
        assert!(sync.values.contains_key("ext.spotify-auth"));
        assert!(sync.values.contains_key("ext.spotify-now-playing"));
        assert!(!sync.values.contains_key("ext.spotify-art-url"));

        // legacy hub lane: the packet data has no internal key either
        let broadcasts = feed.broadcasts.lock().unwrap();
        assert_eq!(broadcasts.len(), 2);
        assert!(!broadcasts[0].contains("art-url"));
        assert_eq!(
            broadcasts[1],
            r#"{"app":"APP_CUSTOM_VALUE","data":{"spotify-auth":"off"}}"#
        );
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
