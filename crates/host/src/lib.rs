//! Host runtime shared by every Pulpit binary (CORE-06/NET-08b): the
//! button-style input registration sweep, the live-value producer pumps
//! and the speaker watcher. The desktop editor (Tauri) and the headless
//! server both wire a [`ClientFeed`] for their sinks and call the
//! helpers here, so the two hosts cannot drift again - four drifts
//! (CORE-01's bare `speaker-volume` channels, CORE-05's ungated server
//! broadcasts, two cadence nits) came from the duplicated copies.
//!
//! # The producer seam
//!
//! [`forward_producer`] is THE place a new producer plugs into: any
//! source that hands over a `tokio` unbounded receiver of snapshot
//! objects (`{"watch-key": value, ...}`) reaches every client lane with
//! one spawned call. The native system-info and AI dev-work sources, the
//! extension fleet's `SetValue` events and the upcoming Spotify producer
//! all ride it; nothing host-side changes per producer.

use std::sync::Arc;

use serde_json::Value;

pub mod media;
pub mod spotify;

/// The client lanes a live-value producer fans out to. Implemented once
/// per host: the desktop adds its WebView as an extra sink, the headless
/// server has none beyond the two protocols.
pub trait ClientFeed: Send + Sync + 'static {
    /// v2 lane: one pushed key -> one `ext.<key>` channel
    /// ([`pulpit_v2::ext_channel`]).
    fn engine_set(&self, key: &str, value: Value);
    /// Legacy lane: one `app_status_update` hub broadcast carrying the
    /// pre-serialized payload.
    fn broadcast_status<'a>(
        &'a self,
        payload: &'a str,
    ) -> impl std::future::Future<Output = ()> + Send + 'a;
    /// The host's own status sink. Returns whether the payload was
    /// delivered: the desktop only emits while its window is visible,
    /// and a `false` must stop the caller's change gate from advancing
    /// (a shown window would otherwise keep a stale value - the pump
    /// contract from plan 004).
    fn emit_status(&self, payload: &Value) -> bool;
}

/// The legacy `app_status_update` app names producers push under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusApp {
    /// Per-key custom values (the `si-*`, `ai-*`, `speaker-*` lanes).
    CustomValue,
    /// Third-party device facts (the `speaker-device` lane).
    ThirdParty,
}

impl StatusApp {
    /// The exact string the stock client switches on.
    pub fn wire_name(self) -> &'static str {
        match self {
            StatusApp::CustomValue => "APP_CUSTOM_VALUE",
            StatusApp::ThirdParty => "THIRD_PARTY_APP",
        }
    }
}

/// The legacy `app_status_update` payload, always built through `json!`
/// so producer values are serialized rather than spliced into JSON text
/// by hand.
fn status_packet(app: StatusApp, data: &Value) -> Value {
    serde_json::json!({ "app": app.wire_name(), "data": data })
}

/// A graph-style value object (`{"title": .., "value": .., "suffix": ..}`):
/// every push appends a sparkline sample client-side, identical reading
/// or not, so these are never change-gated on the broadcast/emit lanes.
/// The v2 engine treats them the same way - series channels keep every
/// point (`StateEngine::set`).
fn is_sample(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.contains_key("value"))
}

/// Per-lane change gate for one producer: the legacy hub lane advances
/// after every broadcast, the host sink lane only after the host reports
/// delivery. A hidden desktop window therefore retries its emit on the
/// next tick without tablets hearing the same value twice.
#[derive(Default)]
pub struct ProducerGate {
    hub: std::collections::BTreeMap<String, Value>,
    host: std::collections::BTreeMap<String, Value>,
}

impl ProducerGate {
    /// The keys of `snapshot` the lane has not delivered yet (`None`
    /// when there is nothing to send).
    fn pending(
        lane: &mut std::collections::BTreeMap<String, Value>,
        snapshot: &Value,
    ) -> Option<Value> {
        let mut out = serde_json::Map::new();
        if let Some(map) = snapshot.as_object() {
            for (key, value) in map {
                if is_sample(value) || lane.get(key) != Some(value) {
                    out.insert(key.clone(), value.clone());
                }
            }
        }
        (!out.is_empty()).then_some(Value::Object(out))
    }

    /// Mark the keys of a delivered delta as seen by the lane.
    fn delivered(lane: &mut std::collections::BTreeMap<String, Value>, delta: &Value) {
        if let Some(map) = delta.as_object() {
            for (key, value) in map {
                lane.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Forward one producer snapshot to every client lane:
///
/// - v2: every key is pushed onto its `ext.<key>` channel. Deliberately
///   not change-gated - the engine dedupes identical scalars itself and
///   series channels must keep appending points.
/// - legacy hub: one `app_status_update` broadcast carrying only the
///   keys that changed since the last broadcast (graph samples always
///   included). This gate is what stops the idle 5 s re-broadcasts
///   (CORE-05).
/// - host sink: the same change gate, but it advances only when the
///   host reports delivery, so a hidden desktop window retries on the
///   next tick (plan 004).
///
/// Both broadcast/emit gates are per-key, so a snapshot with one changed
/// key sends just that key.
pub async fn forward_snapshot(
    feed: &impl ClientFeed,
    app: StatusApp,
    gate: &mut ProducerGate,
    snapshot: &Value,
) {
    if let Some(map) = snapshot.as_object() {
        for (key, value) in map {
            feed.engine_set(key, value.clone());
        }
    }
    if let Some(delta) = ProducerGate::pending(&mut gate.hub, snapshot) {
        let payload = status_packet(app, &delta);
        feed.broadcast_status(&payload.to_string()).await;
        ProducerGate::delivered(&mut gate.hub, &delta);
    }
    if let Some(delta) = ProducerGate::pending(&mut gate.host, snapshot) {
        let payload = status_packet(app, &delta);
        if feed.emit_status(&payload) {
            ProducerGate::delivered(&mut gate.host, &delta);
        }
    }
}

/// One-shot value fan-out for exec-driven pushes (desktop touch mode,
/// editor "Run now" - CORE-04): each push is a fresh user action, not a
/// periodic snapshot, so there is no change gate; the value reaches
/// every lane. Same packet shape and `ext.`-prefixed channels as the
/// periodic producers.
pub async fn push_values(feed: &impl ClientFeed, app: StatusApp, data: &Value) {
    if let Some(map) = data.as_object() {
        for (key, value) in map {
            feed.engine_set(key, value.clone());
        }
    }
    let payload = status_packet(app, data);
    feed.broadcast_status(&payload.to_string()).await;
    feed.emit_status(&payload);
}

/// THE seam for new producers (CORE-06): forward one producer's snapshot
/// stream to every client lane. Any source with an unbounded receiver of
/// snapshot objects plugs in with one call - the native system-info and
/// AI dev-work sources do, and the upcoming Spotify producer is expected
/// to do exactly the same:
///
/// ```ignore
/// tokio::spawn(pulpit_host::forward_producer(feed.clone(), rx));
/// ```
///
/// The `feed` is shared per host; every producer task owns its own
/// [`ProducerGate`], so lanes gate independently per source. See
/// [`forward_snapshot`] for the per-lane delivery rules.
pub async fn forward_producer<F: ClientFeed>(
    feed: Arc<F>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Value>,
) {
    let mut gate = ProducerGate::default();
    while let Some(snapshot) = rx.recv().await {
        forward_snapshot(&*feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
    }
}

/// The extension fleet's event stream: `SetValue(data)` snapshots ride
/// the same lanes as the native producers (plus the debug log the
/// original hosts emitted).
pub async fn forward_ext_events<F: ClientFeed>(
    feed: Arc<F>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<pulpit_ext::ExtEvent>,
) {
    let mut gate = ProducerGate::default();
    while let Some(pulpit_ext::ExtEvent::SetValue(data)) = events.recv().await {
        tracing::debug!(
            keys = ?data.as_object().map(|o| o.keys().collect::<Vec<_>>()),
            "extension value push"
        );
        forward_snapshot(&*feed, StatusApp::CustomValue, &mut gate, &data).await;
    }
}

/// One speaker-watcher tick, split out of [`speaker_watch`] so tests can
/// drive single ticks without the 5 s cadence. Speaker COM calls block,
/// so the snapshot read runs on the blocking pool.
async fn speaker_tick<F: ClientFeed>(
    feed: &F,
    backend: &Arc<dyn pulpit_legacy::Backend>,
    gate: &mut ProducerGate,
    want_device: bool,
) {
    let backend = backend.clone();
    let Ok((volume, muted, device)) =
        tokio::task::spawn_blocking(move || backend.speaker_snapshot(want_device)).await
    else {
        return;
    };
    if let (Some(volume), Some(muted)) = (volume, muted) {
        // percent 0..=100 -> one decimal-free fraction like the
        // original's n/100
        let level = (volume / 100.0 * 1000.0).round() / 1000.0;
        forward_snapshot(
            feed,
            StatusApp::CustomValue,
            gate,
            &serde_json::json!({ "speaker-volume": level, "speaker-muted": muted }),
        )
        .await;
    }
    if let Some(device) = device {
        forward_snapshot(
            feed,
            StatusApp::ThirdParty,
            gate,
            &serde_json::json!({ "speaker-device": device }),
        )
        .await;
    }
}

/// M2 speaker watcher, the one shared implementation (fixes CORE-01's
/// bare `speaker-volume` channel names and CORE-05's ungated 5 s
/// broadcasts): master volume + mute every 5 s under
/// [`StatusApp::CustomValue`], the default output device on the first
/// cycle and every 6th after that (~30 s, the original speaker service
/// cadence) under [`StatusApp::ThirdParty`]. Both lanes change-gated;
/// v2 tiles watch the prefixed `ext.speaker-*` channels.
pub async fn speaker_watch<F: ClientFeed>(feed: Arc<F>, backend: Arc<dyn pulpit_legacy::Backend>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut gate = ProducerGate::default();
    let mut cycle = 0u32;
    loop {
        interval.tick().await;
        cycle = (cycle + 1) % 6;
        // the original fetched the device id on the first fetch and
        // every 6th cycle after that
        speaker_tick(&*feed, &backend, &mut gate, cycle == 1).await;
    }
}

/// Extension timers stretch to IDLE_TICK_FLOOR while no client is
/// watching (see `ExtManager::set_activity`); keep the count current.
/// `v2_hub` is `None` when the v2 stack failed to start (desktop only).
pub async fn activity_loop(
    ext: Arc<pulpit_ext::ExtManager>,
    legacy: Arc<pulpit_legacy::Hub>,
    v2_hub: Option<Arc<pulpit_v2::V2Hub>>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let clients = legacy.len().await + v2_hub.as_ref().map_or(0, |hub| hub.count());
        ext.set_activity(clients);
    }
}

/// The "consumers" signal `pulpit_spotify::spawn_push` reads: how many
/// live state consumers exist right now - connected legacy + v2 clients
/// plus whatever host-local consumer `extra` counts (the desktop adds
/// its visible editor window). Computed on demand from the poller's own
/// thread, which asks at most every few seconds; no sampler task, so an
/// idle host gains no periodic wakeup. A contended legacy hub counts as
/// one consumer (poll rather than go stale). `v2_hub` is `None` when the
/// v2 stack failed to start.
pub fn consumer_reader(
    legacy: Arc<pulpit_legacy::Hub>,
    v2_hub: Option<Arc<pulpit_v2::V2Hub>>,
    extra: Option<Arc<dyn Fn() -> usize + Send + Sync + 'static>>,
) -> Arc<dyn Fn() -> usize + Send + Sync + 'static> {
    Arc::new(move || {
        let legacy = legacy.try_len().unwrap_or(1);
        let v2 = v2_hub.as_ref().map_or(0, |hub| hub.count());
        // the host-local check is the expensive one (the desktop asks
        // the UI thread); skip it when a client already counts
        if legacy + v2 > 0 {
            return legacy + v2;
        }
        extra.as_deref().map_or(0, |f| f())
    })
}

/// The extension packages native code replaces (their JS runtimes were
/// the heaviest part of the fleet); `ExtManager::load` must skip them.
/// One list, both hosts.
pub fn native_replaced() -> Vec<String> {
    ["deckboard-system-info", "deckboard-callurl"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Register one button-style source so the legacy mapper can style its
/// tiles (see [`register_inputs`]). `font_icon` defaults to the "fas"
/// family like the original's inputs.
fn register_input(
    value: &str,
    icon: Option<&str>,
    color: Option<&str>,
    font_icon: &str,
    mode: Option<&str>,
    command: Option<&str>,
) {
    pulpit_legacy::props::register_extension_input(pulpit_legacy::props::ExtInput {
        value: value.to_string(),
        icon: icon.map(str::to_string),
        color: color.map(str::to_string),
        font_icon: Some(font_icon.to_string()),
        mode: mode.map(str::to_string),
        command: command.map(str::to_string),
    });
}

/// Register every button-style source with the legacy mapper so its
/// tiles resolve icon/color/mode: extension inputs plus the native
/// Voicemeeter, Discord, system-info, callurl and AI dev-work
/// declarations. One sweep for both hosts; call once after
/// `ExtManager::load`, before any board is mapped.
pub fn register_inputs(ext: &pulpit_ext::ExtManager) {
    // extension inputs style tiles the same way the original's
    // getExtensionButton did
    for input in ext.inputs() {
        register_input(
            &input.value,
            input.icon.as_deref(),
            input.color.as_deref(),
            input.font_icon.as_deref().unwrap_or("fas"),
            input.mode.as_deref(),
            input.command.as_deref(),
        );
    }
    // the native Voicemeeter bridge replaces the ffi-napi extension
    for (value, icon, font_icon, color) in pulpit_vm::input_declarations() {
        register_input(value, icon, Some(color), font_icon, None, None);
    }
    // native Discord RPC (colors/icons/modes from the discord-deckboard
    // package; the custom-value mode is what makes the mute/deaf tiles
    // watch their pushed ON/OFF label)
    for (value, icon, color, mode) in pulpit_discord::input_declarations() {
        register_input(value, Some(icon), Some(color), "fas", mode, None);
    }
    // native system-info (values copied from the JS package's inputs,
    // including its odd `headphones` icon); the graph mode is what makes
    // the CPU/RAM tiles render as graphs
    for (value, icon, font_icon, color, mode) in pulpit_sysinfo::input_declarations() {
        register_input(value, Some(icon), Some(color), font_icon, Some(mode), None);
    }
    // native callurl (from the JS package's single input)
    register_input(
        "url-to-call",
        Some("link"),
        Some("#ff29df"),
        "fas",
        None,
        None,
    );
    // native AI dev-work display tiles (plan limits, agent progress)
    for (value, icon, color, mode) in pulpit_aidev::input_declarations() {
        register_input(value, Some(icon), Some(color), "fas", Some(mode), None);
    }
    // native Spotify (control buttons + the custom-value toggles and
    // slider/status shapes the pushed state keys drive)
    for (value, icon, color, mode) in pulpit_spotify::input_declarations() {
        register_input(value, Some(icon), Some(color), "fas", mode, None);
    }
    // native utility tools (clock, timer, stopwatch, counter)
    for (value, icon, color, mode) in pulpit_tools::input_declarations() {
        register_input(value, Some(icon), Some(color), "fas", mode, None);
    }
    // native system media (SMTC): the now-playing display tile, transport
    // buttons and the seek slider, for any player the system reports
    for (value, icon, color, mode) in pulpit_os::media::input_declarations() {
        register_input(value, Some(icon), Some(color), "fas", mode, None);
    }
}

/// The AI dev-work producer's source locations: `config` (the
/// `PULPIT_AIDEV_CONFIG` override or `pulpitApp/aidev.json`, resolved by
/// the host) plus the transcript roots in the real user home (they live
/// outside the pulpitApp data dir). One place, both hosts.
pub fn aidev_paths(config: std::path::PathBuf) -> pulpit_aidev::Paths {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    pulpit_aidev::Paths {
        config,
        zcode_cli: home.join(".zcode").join("cli"),
        claude_projects: home.join(".claude").join("projects"),
        codex_sessions: home.join(".codex").join("sessions"),
        opencode_db: home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db"),
        antigravity_conversations: home
            .join(".gemini")
            .join("antigravity")
            .join("conversations"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use pulpit_v2::StateEngine;

    /// Records every lane; the host lane can be told to "not deliver"
    /// (hidden desktop window).
    struct FakeFeed {
        engine: StateEngine,
        broadcasts: std::sync::Mutex<Vec<String>>,
        emit_attempts: AtomicUsize,
        emits: std::sync::Mutex<Vec<Value>>,
        emit_delivers: AtomicBool,
    }

    impl FakeFeed {
        fn delivering() -> FakeFeed {
            FakeFeed {
                engine: StateEngine::new(64),
                broadcasts: std::sync::Mutex::new(Vec::new()),
                emit_attempts: AtomicUsize::new(0),
                emits: std::sync::Mutex::new(Vec::new()),
                emit_delivers: AtomicBool::new(true),
            }
        }

        fn broadcasts(&self) -> Vec<String> {
            self.broadcasts.lock().unwrap().clone()
        }

        fn emits(&self) -> Vec<Value> {
            self.emits.lock().unwrap().clone()
        }
    }

    impl ClientFeed for FakeFeed {
        fn engine_set(&self, key: &str, value: Value) {
            self.engine.set(&pulpit_v2::ext_channel(key), value);
        }
        async fn broadcast_status(&self, payload: &str) {
            self.broadcasts.lock().unwrap().push(payload.to_string());
        }
        fn emit_status(&self, payload: &Value) -> bool {
            self.emit_attempts.fetch_add(1, Ordering::Relaxed);
            if self.emit_delivers.load(Ordering::Relaxed) {
                self.emits.lock().unwrap().push(payload.clone());
                true
            } else {
                false
            }
        }
    }

    /// Speaker stub: fixed snapshot, no SQLite behind it.
    struct FakeSpeaker {
        volume: f32,
        muted: bool,
        device: &'static str,
    }

    impl pulpit_legacy::Backend for FakeSpeaker {
        fn get_boards(&self) -> Vec<pulpit_db::BoardRow> {
            Vec::new()
        }
        fn get_board(&self, _board_id: i64) -> Option<pulpit_db::BoardRow> {
            None
        }
        fn get_buttons_by_board(&self, _board_id: i64) -> Vec<pulpit_db::ButtonRow> {
            Vec::new()
        }
        fn get_button(&self, _id: i64) -> Option<pulpit_db::ButtonRow> {
            None
        }
        fn exec(
            &self,
            _button: pulpit_db::ButtonRow,
            _is_tap_start: bool,
            _sink: &mut dyn pulpit_actions::EventSink,
        ) {
        }
        fn slider(&self, _button: pulpit_db::ButtonRow, _value: f64) {}
        fn speaker_snapshot(
            &self,
            want_device: bool,
        ) -> (Option<f32>, Option<bool>, Option<String>) {
            (
                Some(self.volume),
                Some(self.muted),
                want_device.then(|| self.device.to_string()),
            )
        }
    }

    #[tokio::test]
    async fn forward_prefixes_v2_channels_and_shapes_the_legacy_packet() {
        let feed = FakeFeed::delivering();
        let mut gate = ProducerGate::default();
        forward_snapshot(
            &feed,
            StatusApp::CustomValue,
            &mut gate,
            &serde_json::json!({ "speaker-volume": 0.5, "speaker-muted": false }),
        )
        .await;

        // v2 lane: ext.-prefixed channels (CORE-01)
        let sync = feed.engine.snapshot();
        assert_eq!(sync.values["ext.speaker-volume"], serde_json::json!(0.5));
        assert_eq!(sync.values["ext.speaker-muted"], serde_json::json!(false));

        // legacy lane: the exact APP_CUSTOM_VALUE packet shape, built by
        // serde (keys sorted, values serialized)
        assert_eq!(
            feed.broadcasts(),
            [r#"{"app":"APP_CUSTOM_VALUE","data":{"speaker-muted":false,"speaker-volume":0.5}}"#]
        );
        // host lane: the same payload
        assert_eq!(feed.emits().len(), 1);
        assert_eq!(
            feed.emits()[0]["data"]["speaker-volume"],
            serde_json::json!(0.5)
        );
    }

    #[tokio::test]
    async fn identical_snapshots_broadcast_once_until_a_key_changes() {
        // CORE-05: an unchanged value must not re-broadcast every tick
        let feed = FakeFeed::delivering();
        let mut gate = ProducerGate::default();
        let snapshot = serde_json::json!({ "speaker-volume": 0.42, "speaker-muted": false });
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
        assert_eq!(feed.broadcasts().len(), 1);

        // one changed key sends exactly that key
        let changed = serde_json::json!({ "speaker-volume": 0.42, "speaker-muted": true });
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &changed).await;
        assert_eq!(feed.broadcasts().len(), 2);
        assert_eq!(
            feed.broadcasts()[1],
            r#"{"app":"APP_CUSTOM_VALUE","data":{"speaker-muted":true}}"#
        );
    }

    #[tokio::test]
    async fn undelivered_host_emits_retry_without_re_broadcasting() {
        // plan 004's pump contract: a hidden window must not advance the
        // gate - but tablets must not hear the value twice either
        let feed = FakeFeed::delivering();
        feed.emit_delivers.store(false, Ordering::Relaxed);
        let mut gate = ProducerGate::default();
        let snapshot = serde_json::json!({ "speaker-volume": 0.42 });
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
        // hub lane advanced after its one broadcast; host lane retried
        assert_eq!(feed.broadcasts().len(), 1);
        assert_eq!(feed.emit_attempts.load(Ordering::Relaxed), 2);

        // the window shows: the pending value finally lands, still with
        // no extra hub broadcast
        feed.emit_delivers.store(true, Ordering::Relaxed);
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &snapshot).await;
        assert_eq!(feed.broadcasts().len(), 1);
        assert_eq!(feed.emits().len(), 1);
    }

    #[tokio::test]
    async fn graph_sample_keys_keep_flowing_on_identical_values() {
        // sparklines append a sample per push (App.vue mergeCustomValues
        // and the stock client do the same), so a repeated identical
        // reading must still go out
        let feed = FakeFeed::delivering();
        let mut gate = ProducerGate::default();
        let sample = serde_json::json!({
            "si-load-cpu": { "title": "CPU Load", "value": "45.2", "suffix": "%" }
        });
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &sample).await;
        forward_snapshot(&feed, StatusApp::CustomValue, &mut gate, &sample).await;
        assert_eq!(feed.broadcasts().len(), 2);
        assert_eq!(feed.emits().len(), 2);
        // the engine's series lane gets every push too
        assert_eq!(feed.engine.drain_dirty().len(), 1);
    }

    #[tokio::test]
    async fn push_values_fans_out_without_a_gate() {
        // CORE-04's exec lane: every user-driven push reaches every lane
        let feed = FakeFeed::delivering();
        push_values(
            &feed,
            StatusApp::CustomValue,
            &serde_json::json!({ "toggle-microphone": "ON" }),
        )
        .await;
        push_values(
            &feed,
            StatusApp::CustomValue,
            &serde_json::json!({ "toggle-microphone": "ON" }),
        )
        .await;
        assert_eq!(feed.broadcasts().len(), 2);
        assert_eq!(feed.emits().len(), 2);
        let sync = feed.engine.snapshot();
        assert_eq!(
            sync.values["ext.toggle-microphone"],
            serde_json::json!("ON")
        );
    }

    #[tokio::test]
    async fn speaker_tick_uses_prefixed_channels_and_gates_repeats() {
        let feed = FakeFeed::delivering();
        let backend: Arc<dyn pulpit_legacy::Backend> = Arc::new(FakeSpeaker {
            volume: 42.0,
            muted: false,
            device: "speakers",
        });
        let mut gate = ProducerGate::default();
        speaker_tick(&feed, &backend, &mut gate, true).await;

        // CORE-01: the engine channels are the prefixed ones the v2
        // tiles declare, and the level is the n/100 fraction (f32
        // artifact included - the wire value matches the old pumps)
        let level = (42.0f32 / 100.0 * 1000.0).round() / 1000.0;
        let sync = feed.engine.snapshot();
        assert_eq!(sync.values["ext.speaker-volume"], serde_json::json!(level));
        assert_eq!(sync.values["ext.speaker-muted"], serde_json::json!(false));
        assert_eq!(
            sync.values["ext.speaker-device"],
            serde_json::json!("speakers")
        );

        // volume/mute under APP_CUSTOM_VALUE, device under
        // THIRD_PARTY_APP, both in the legacy packet shapes
        let volume_packet = serde_json::json!({
            "app": "APP_CUSTOM_VALUE",
            "data": { "speaker-muted": false, "speaker-volume": level },
        })
        .to_string();
        assert_eq!(
            feed.broadcasts(),
            [
                volume_packet,
                r#"{"app":"THIRD_PARTY_APP","data":{"speaker-device":"speakers"}}"#.to_string(),
            ]
        );

        // CORE-05: unchanged ticks stay silent; a changed volume sends
        // just the changed keys
        speaker_tick(&feed, &backend, &mut gate, false).await;
        assert_eq!(feed.broadcasts().len(), 2);
        let backend: Arc<dyn pulpit_legacy::Backend> = Arc::new(FakeSpeaker {
            volume: 43.0,
            muted: false,
            device: "speakers",
        });
        speaker_tick(&feed, &backend, &mut gate, false).await;
        assert_eq!(feed.broadcasts().len(), 3);
        let next = (43.0f32 / 100.0 * 1000.0).round() / 1000.0;
        assert_eq!(
            feed.broadcasts()[2],
            serde_json::json!({
                "app": "APP_CUSTOM_VALUE",
                "data": { "speaker-volume": next },
            })
            .to_string()
        );
    }

    #[test]
    fn native_replaced_lists_the_js_packages_native_code_replaces() {
        assert_eq!(
            native_replaced(),
            [
                "deckboard-system-info".to_string(),
                "deckboard-callurl".to_string()
            ]
        );
    }
}
