//! deckboard-server: headless legacy server binary. Serves the legacy
//! socket.io v2 protocol from the existing `~/deckboard/database.db`
//! (read-only - the desktop editor app owns writes).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use deckboard_backend::SqlBackend;
use deckboard_legacy::{AppState, Hub};
use tracing_subscriber::EnvFilter;

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
        .unwrap_or_else(deckboard_db::default_db_path);
    let db = deckboard_db::Db::open_read_only(Some(&db_path))
        .with_context(|| format!("opening {}", db_path.display()))?;

    // Original Deckboard extensions: same directory and settings.json the
    // original app uses. The manager owns one JS runtime per package.
    // DECKBOARD_EXT_DIR overrides the location (profiling / hermetic runs).
    let home = dirs::home_dir().context("home directory")?;
    let settings = std::fs::read_to_string(home.join("deckboard/settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let ext_dir = std::env::var_os("DECKBOARD_EXT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("deckboard/extensions"));
    // Native system-info and callurl replace their JS packages (the JS
    // runtimes were the heaviest part of the extension fleet); the manager
    // must not load them.
    let native_replaced = ["deckboard-system-info", "deckboard-callurl"]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let (ext_manager, mut ext_events) =
        deckboard_ext::ExtManager::load(&ext_dir, &settings, &native_replaced);
    for (package, name, error) in ext_manager.summary() {
        match error {
            Some(e) => tracing::warn!(package, name, error = e, "extension disabled"),
            None => tracing::info!(package, name, "extension ready"),
        }
    }
    // Extension inputs back button styles the same way the original's
    // getExtensionButton does (e.g. si-cpu tiles take #8E44AD from the
    // system-info input), so the mapper can resolve them any time later.
    for input in ext_manager.inputs() {
        deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
            value: input.value.clone(),
            icon: input.icon.clone(),
            color: input.color.clone(),
            font_icon: input.font_icon.clone(),
            mode: input.mode.clone(),
            command: input.command.clone(),
        });
    }
    // The native Voicemeeter bridge replaces the ffi-napi extension, so its
    // input declarations register here too (vm tiles otherwise stay gray).
    for (value, icon, font_icon, color) in deckboard_vm::input_declarations() {
        deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
            value: value.to_string(),
            icon: icon.map(str::to_string),
            color: Some(color.to_string()),
            font_icon: Some(font_icon.to_string()),
            mode: None,
            command: None,
        });
    }
    // Same for the native Discord RPC (colors/icons/modes from the
    // discord-deckboard package; the custom-value mode is what makes the
    // mute/deaf tiles watch their pushed ON/OFF label).
    for (value, icon, color, mode) in deckboard_discord::input_declarations() {
        deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
            value: value.to_string(),
            icon: Some(icon.to_string()),
            color: Some(color.to_string()),
            font_icon: Some("fas".to_string()),
            mode: mode.map(str::to_string),
            command: None,
        });
    }
    // Native system-info declarations (values copied from the JS package's
    // inputs, including its odd `headphones` icon); the graph mode is what
    // makes the CPU/RAM tiles render as graphs.
    for (value, icon, font_icon, color, mode) in deckboard_sysinfo::input_declarations() {
        deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
            value: value.to_string(),
            icon: Some(icon.to_string()),
            color: Some(color.to_string()),
            font_icon: Some(font_icon.to_string()),
            mode: Some(mode.to_string()),
            command: None,
        });
    }
    // Native callurl declaration (from the JS package's single input).
    deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
        value: "url-to-call".into(),
        icon: Some("link".into()),
        color: Some("#ff29df".into()),
        font_icon: Some("fas".to_string()),
        mode: None,
        command: None,
    });
    // Native AI dev-work display tiles (plan limits, agent progress).
    for (value, icon, color, mode) in deckboard_aidev::input_declarations() {
        deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
            value: value.to_string(),
            icon: Some(icon.to_string()),
            color: Some(color.to_string()),
            font_icon: Some("fas".to_string()),
            mode: Some(mode.to_string()),
            command: None,
        });
    }
    let backend = Arc::new(
        SqlBackend::new(db)
            .with_extensions(ext_manager.clone())
            .with_discord(
                deckboard_discord::DiscordConfig::from_settings(&settings),
                home.join("deckboard/settings.json"),
            ),
    );

    let state = Arc::new(AppState {
        hub: Arc::new(Hub::new()),
        backend: backend as Arc<dyn deckboard_legacy::Backend>,
    });

    // Port 8500 is what the stock Android client hardcodes (and the original
    // app's default); `DECKBOARD_PORT` overrides it for side-by-side runs.
    let port: u16 = std::env::var("DECKBOARD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8500);

    // Protocol v2 (docs/protocol-v2.md): same port, /v2/ws + /assets +
    // /v2/pair. Devices/assets live next to the DB; DECKBOARD_DEVICES and
    // DECKBOARD_ASSETS override them (hermetic runs).
    let v2 = Arc::new(deckboard_v2::V2State {
        hub: Arc::new(deckboard_v2::V2Hub::new()),
        backend: state.backend.clone(),
        devices: Arc::new(deckboard_v2::DeviceStore::load(
            std::env::var_os("DECKBOARD_DEVICES")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join("deckboard/devices.json")),
        )?),
        pairing: Arc::new(deckboard_v2::Pairing::new()),
        assets: Arc::new(deckboard_v2::AssetStore::open(
            std::env::var_os("DECKBOARD_ASSETS")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join("deckboard/assets")),
        )?),
        engine: Arc::new(deckboard_v2::StateEngine::new(deckboard_proto::SERIES_CAP)),
        generation: deckboard_v2::Generation::starting_at(1),
        boards_cache: Default::default(),
        config: deckboard_v2::V2Config {
            public_port: port,
            ..Default::default()
        },
    });

    // Extension pushes feed both protocols: the legacy app_status_update
    // broadcast (stock client) and one v2 channel per data key.
    let feed_v2 = {
        let engine = v2.engine.clone();
        move |data: serde_json::Value| {
            if let Some(map) = data.as_object() {
                for (key, value) in map {
                    engine.set(&format!("ext.{key}"), value.clone());
                }
            }
        }
    };
    let hub_legacy = state.hub.clone();
    let feed = feed_v2.clone();
    tokio::spawn(async move {
        while let Some(deckboard_ext::ExtEvent::SetValue(data)) = ext_events.recv().await {
            feed(data.clone());
            let data = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
            hub_legacy
                .broadcast("app_status_update", Some(&payload))
                .await;
        }
    });

    // Native system-info pushes its four si-* values on the same channel
    // and cadence the JS extension used.
    let mut sysinfo_values = deckboard_sysinfo::spawn_push();
    let feed = feed_v2.clone();
    let hub_legacy = state.hub.clone();
    tokio::spawn(async move {
        while let Some(data) = sysinfo_values.recv().await {
            feed(data.clone());
            let data = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
            hub_legacy
                .broadcast("app_status_update", Some(&payload))
                .await;
        }
    });

    // Native AI dev-work source: plan limits + agent progress, same channel.
    // DECKBOARD_AIDEV_CONFIG overrides the config location (hermetic runs).
    let aidev_config = std::env::var_os("DECKBOARD_AIDEV_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("deckboard/aidev.json"));
    let aidev_paths = deckboard_aidev::Paths {
        config: aidev_config,
        zcode_cli: home.join(".zcode/cli"),
        claude_projects: home.join(".claude/projects"),
        codex_sessions: home.join(".codex/sessions"),
        opencode_db: home.join(".local/share/opencode/opencode.db"),
        antigravity_conversations: home.join(".gemini/antigravity/conversations"),
    };
    let mut aidev_values = deckboard_aidev::spawn_push(aidev_paths);
    let feed = feed_v2.clone();
    let hub_legacy = state.hub.clone();
    tokio::spawn(async move {
        while let Some(data) = aidev_values.recv().await {
            feed(data.clone());
            let data = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
            hub_legacy
                .broadcast("app_status_update", Some(&payload))
                .await;
        }
    });

    // M2 speaker watcher: master volume + mute every 5s, default device
    // id every 6th cycle (30s) - the original speaker service cadence.
    {
        let backend = state.backend.clone();
        let feed = feed_v2.clone();
        let hub = state.hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut cycle = 0u32;
            loop {
                tick.tick().await;
                cycle = (cycle + 1) % 6;
                let backend = backend.clone();
                // the original fetched the device id on the first fetch
                // and every 6th cycle after that
                let snapshot = tokio::task::spawn_blocking(move || {
                    backend.speaker_snapshot(cycle == 1)
                })
                .await
                .ok();
                if let Some((volume, muted, device)) = snapshot {
                    if let (Some(volume), Some(muted)) = (volume, muted) {
                        // one decimal-free fraction like the original n/100
                        let level = (volume / 100.0 * 1000.0).round() / 1000.0;
                        feed(serde_json::json!({
                            "speaker-volume": level,
                            "speaker-muted": muted,
                        }));
                        let payload = format!(
                            r#"{{"app":"APP_CUSTOM_VALUE","data":{{"speaker-volume":{level},"speaker-muted":{muted}}}}}"#
                        );
                        hub.broadcast("app_status_update", Some(&payload)).await;
                    }
                    if let Some(device) = device {
                        feed(serde_json::json!({ "speaker-device": device }));
                        let payload = format!(
                            r#"{{"app":"THIRD_PARTY_APP","data":{{"speaker-device":"{device}"}}}}"#
                        );
                        hub.broadcast("app_status_update", Some(&payload)).await;
                    }
                }
            }
        });
    }

    // v2 background task: coalesced state patches.
    tokio::spawn(deckboard_v2::run_flusher(
        v2.engine.clone(),
        v2.hub.clone(),
        v2.config.patch_interval,
    ));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("deckboard server listening on {addr} (legacy /socket.io/ + v2 /v2/ws)");

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
    // peer is gone without a TCP close (see deckboard_v2::run_reaper).
    tokio::spawn(deckboard_v2::run_reaper(
        v2.hub.clone(),
        v2.config.ping_interval,
    ));

    // Extension timers stretch to IDLE_TICK_FLOOR while no client is
    // watching (see ExtManager::set_activity); keep the count current.
    {
        let ext = ext_manager.clone();
        let hub = state.hub.clone();
        let v2_hub = v2.hub.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let clients = hub.len().await + v2_hub.count();
                ext.set_activity(clients);
            }
        });
    }

    // ConnectInfo is needed by the loopback guard on POST /v2/pair.
    let app = deckboard_legacy::router(state).merge(deckboard_v2::router(v2));
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
