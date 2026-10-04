//! pulpit-server: headless legacy server binary. Serves the legacy
//! socket.io v2 protocol from `~/pulpitApp/database.db` (read-only - the
//! desktop editor app owns writes; a legacy `~/deckboard` dir is migrated
//! on first use).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use pulpit_backend::SqlBackend;
use pulpit_legacy::{AppState, Hub};
use tracing_subscriber::EnvFilter;

/// The headless host's [`pulpit_host::ClientFeed`] impl: the v2 state
/// engine and the legacy hub, nothing extra (no UI to feed).
struct HeadlessFeed {
    engine: Arc<pulpit_v2::StateEngine>,
    hub: Arc<Hub>,
}

impl pulpit_host::ClientFeed for HeadlessFeed {
    fn engine_set(&self, key: &str, value: serde_json::Value) {
        self.engine.set(&pulpit_v2::ext_channel(key), value);
    }
    async fn broadcast_status(&self, payload: &str) {
        self.hub.broadcast("app_status_update", Some(payload)).await;
    }
    fn emit_status(&self, _payload: &serde_json::Value) -> bool {
        true
    }
}

/// How long the shutdown goodbye waits for the session pumps to put the
/// `server.shutdown` frame and the WS close on the wire before `main`
/// returns and the process exits (mirrors the desktop's flush grace).
const SHUTDOWN_FLUSH_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

// current_thread: the workload is a couple of tablets doing tiny async IO;
// everything blocking (exec, sliders, extension JS, Discord, Voicemeeter)
// already runs on spawn_blocking or dedicated extension threads, so the
// default worker-per-core fleet only cost threads and memory.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Single-writer rule: while this server runs, the original Deckboard
    // desktop app must be closed (docs/decisions.md). We open read-only so
    // we can never fight over the file.
    let db_path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(pulpit_db::default_db_path);
    let db = pulpit_db::Db::open_read_only(Some(&db_path))
        .with_context(|| format!("opening {}", db_path.display()))?;

    // Original Deckboard extensions: same directory and settings.json the
    // original app used. The manager owns one JS runtime per package.
    // PULPIT_EXT_DIR overrides the location (profiling / hermetic runs).
    let data_dir = pulpit_db::data_dir();
    let settings = std::fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let ext_dir = std::env::var_os("PULPIT_EXT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("extensions"));
    // Extension inputs and every native declaration (Voicemeeter,
    // Discord, system-info, callurl, AI dev-work) register through the
    // shared host module, like the desktop does.
    let (ext_manager, ext_events) =
        pulpit_ext::ExtManager::load(&ext_dir, &settings, &pulpit_host::native_replaced());
    for (package, name, error) in ext_manager.summary() {
        match error {
            Some(e) => tracing::warn!(package, name, error = e, "extension disabled"),
            None => tracing::info!(package, name, "extension ready"),
        }
    }
    pulpit_host::register_inputs(&ext_manager);
    // Spotify: the same `spotify.json` the desktop writes. A missing
    // file (or one without a login) disables Spotify here - the headless
    // server never logs in interactively. `PULPIT_SPOTIFY_CONFIG`
    // overrides the location (hermetic runs).
    let spotify_path = std::env::var_os("PULPIT_SPOTIFY_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("spotify.json"));
    let spotify = match pulpit_spotify::SpotifyConfig::load(&spotify_path) {
        Ok(config) if config.has_login() => {
            Some(pulpit_spotify::Spotify::new(config, spotify_path))
        }
        Ok(_) => {
            tracing::info!("no Spotify login in spotify.json - Spotify disabled");
            None
        }
        Err(e) => {
            tracing::error!("spotify.json exists but cannot be read: {e} - Spotify disabled");
            None
        }
    };
    let backend = Arc::new(
        SqlBackend::new(db)
            .with_extensions(ext_manager.clone())
            .with_discord(
                pulpit_discord::DiscordConfig::from_settings(&settings),
                data_dir.join("settings.json"),
            )
            .with_spotify(spotify.clone()),
    );

    let state = Arc::new(AppState {
        hub: Arc::new(Hub::new()),
        backend: backend as Arc<dyn pulpit_legacy::Backend>,
    });

    // Port 8500 is what the stock Android client hardcodes (and the original
    // app's default); `PULPIT_PORT` overrides it for side-by-side runs.
    let port: u16 = std::env::var("PULPIT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8500);

    // Protocol v2 (docs/protocol-v2.md): same port, /v2/ws + /assets +
    // /v2/pair. Devices/assets live next to the DB; PULPIT_DEVICES and
    // PULPIT_ASSETS override them (hermetic runs).
    let v2 = Arc::new(pulpit_v2::V2State {
        hub: Arc::new(pulpit_v2::V2Hub::new()),
        backend: state.backend.clone(),
        devices: Arc::new(pulpit_v2::DeviceStore::load(
            std::env::var_os("PULPIT_DEVICES")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| data_dir.join("devices.json")),
        )?),
        pairing: Arc::new(pulpit_v2::Pairing::new()),
        assets: Arc::new(pulpit_v2::AssetStore::open(
            std::env::var_os("PULPIT_ASSETS")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| data_dir.join("assets")),
        )?),
        engine: Arc::new(pulpit_v2::StateEngine::new(pulpit_proto::SERIES_CAP)),
        generation: pulpit_v2::Generation::starting_at(1),
        boards_cache: Default::default(),
        pair_requests: Default::default(),
        config: Default::default(),
    });

    // M8 discovery: advertise on mDNS so tablets can find this server
    // (headless = no gate, so pair-requests auto-accept with a warning).
    match pulpit_v2::discovery::advertise(port, env!("CARGO_PKG_VERSION")) {
        Ok(d) => {
            std::mem::forget(d);
        }
        Err(e) => tracing::warn!("mDNS advertisement failed: {e}"),
    }

    // Producer pumps (extension fleet, native system-info, AI dev-work,
    // speaker watcher) and the extension activity loop all live in the
    // shared host module now - one implementation, same lanes and
    // change-gating as the desktop (CORE-06).
    let feed = Arc::new(HeadlessFeed {
        engine: v2.engine.clone(),
        hub: state.hub.clone(),
    });
    tokio::spawn(pulpit_host::forward_ext_events(
        feed.clone(),
        ext_events,
    ));
    tokio::spawn(pulpit_host::forward_producer(
        feed.clone(),
        pulpit_sysinfo::spawn_push(),
    ));
    // PULPIT_AIDEV_CONFIG overrides the config location (hermetic runs).
    let aidev_config = std::env::var_os("PULPIT_AIDEV_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("aidev.json"));
    tokio::spawn(pulpit_host::forward_producer(
        feed.clone(),
        pulpit_aidev::spawn_push(pulpit_host::aidev_paths(aidev_config)),
    ));
    tokio::spawn(pulpit_host::speaker_watch(feed.clone(), state.backend.clone()));
    // Spotify poller: consumers = connected legacy + v2 clients (no
    // host-local UI on the headless server). Snapshots ride the shared
    // spotify pump (internal art key stripped, album art imported).
    match &spotify {
        Some(spotify) => {
            let consumers =
                pulpit_host::consumer_reader(state.hub.clone(), Some(v2.hub.clone()), None);
            tokio::spawn(pulpit_host::spotify::forward_spotify(
                feed.clone(),
                pulpit_spotify::spawn_push(spotify.clone(), consumers),
                Some(v2.assets.clone()),
            ));
        }
        None => {
            // no poller: one `spotify-auth: "off"` marker so clients see
            // a defined state
            tokio::spawn(pulpit_host::spotify::forward_spotify_disabled(
                feed.clone(),
            ));
        }
    }
    tokio::spawn(pulpit_host::activity_loop(
        ext_manager.clone(),
        state.hub.clone(),
        Some(v2.hub.clone()),
    ));

    // v2 background task: coalesced state patches.
    tokio::spawn(pulpit_v2::run_flusher(
        v2.engine.clone(),
        v2.hub.clone(),
        v2.config.patch_interval,
    ));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("pulpit server listening on {addr} (legacy /socket.io/ + v2 /v2/ws)");

    // Engine.IO: drop sessions silent for longer than pingInterval+pingTimeout
    let hub = state.hub.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            hub.reap(75).await;
        }
    });
    // v2: same sweep for the typed protocol - three missed pongs mean the
    // peer is gone without a TCP close (see pulpit_v2::run_reaper).
    tokio::spawn(pulpit_v2::run_reaper(
        v2.hub.clone(),
        v2.config.ping_interval,
    ));

    // ConnectInfo is needed by the loopback guard on POST /v2/pair.
    let app = pulpit_legacy::router(state).merge(pulpit_v2::router(v2.clone()));

    // A deliberate exit (Ctrl+C, service stop) must tell the v2 tablets:
    // per protocol-v2.md §9 one server.shutdown frame beats a bare WS
    // drop (NET-04) - a conforming client treats a drop as transient
    // loss and would keep retrying into the void for minutes.
    let v2_hub = v2.hub.clone();
    let goodbye = async move {
        wait_for_terminate().await;
        tracing::info!("shutdown requested - saying goodbye to v2 tablets");
        v2_hub.shutdown();
        // the session pumps write the frame + close asynchronously; give
        // them a beat before `main` returns and the process exits
        tokio::time::sleep(SHUTDOWN_FLUSH_GRACE).await;
    };
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(goodbye)
    .await?;
    Ok(())
}

/// Resolve on the process's termination signals: Ctrl+C everywhere,
/// plus SIGTERM where it exists (service managers).
async fn wait_for_terminate() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
