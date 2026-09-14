//! deckboard-server: M0 spike binary. Serves the legacy socket.io v2
//! protocol on port 8500 from the existing `~/deckboard/database.db`.

mod backend;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use deckboard_legacy::{router, AppState, Hub};
use tracing_subscriber::EnvFilter;

use crate::backend::SqlBackend;

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
        font_icon: Some("fas".into()),
        mode: None,
        command: None,
    });
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

    // Extensions push custom values (graph/button state); the original
    // forwards them to every client as app_status_update APP_CUSTOM_VALUE.
    let hub = state.hub.clone();
    tokio::spawn(async move {
        while let Some(deckboard_ext::ExtEvent::SetValue(data)) = ext_events.recv().await {
            let data = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
            hub.broadcast("app_status_update", Some(&payload)).await;
        }
    });

    // Native system-info pushes its four si-* values on the same channel
    // and cadence the JS extension used.
    let mut sysinfo_values = deckboard_sysinfo::spawn_push();
    let hub = state.hub.clone();
    tokio::spawn(async move {
        while let Some(data) = sysinfo_values.recv().await {
            let data = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
            let payload = format!(r#"{{"app":"APP_CUSTOM_VALUE","data":{data}}}"#);
            hub.broadcast("app_status_update", Some(&payload)).await;
        }
    });

    // TEMPORARY default 8501: the original desktop app still owns 8500 and
    // the DB. Note the stock Android client hardcodes port 8500 - testing
    // with the real tablet requires closing the old app so we can bind 8500
    // (set DECKBOARD_PORT=8500), or waiting for protocol v2 (our client).
    // Flip the default back to 8500 when the original app is retired.
    let port: u16 = std::env::var("DECKBOARD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8501);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("deckboard legacy server listening on {addr}");

    // Engine.IO: drop sessions silent for longer than pingInterval+pingTimeout
    let hub = state.hub.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            hub.reap(75).await;
        }
    });

    axum::serve(listener, router(state)).await?;
    Ok(())
}
