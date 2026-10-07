//! Pulpit desktop editor: Tauri 2 shell that embeds the legacy socket.io
//! v2 server (so the stock Android client keeps working) and adds the board
//! editor write path on top of the shared [`SqlBackend`].

use std::sync::Arc;

use pulpit_backend::SqlBackend;use pulpit_db::{BoardRow, ButtonRow};
use pulpit_ext::ExtManager;
use pulpit_legacy::{AppState, Backend, EditorBroadcaster, Hub};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use std::collections::HashMap;
use std::sync::OnceLock;

/// Everything the UI commands need, built once in [`setup_core`].
struct DesktopState {
    backend: Option<Arc<SqlBackend>>,
    broadcaster: Option<EditorBroadcaster>,
    hub: Option<Arc<Hub>>,
    port: u16,
    /// Loaded extensions, for the editor's action catalog and tile styling.
    ext: Option<Arc<ExtManager>>,
    /// Protocol v2 state; `None` when the stack failed to start. Carries
    /// the session fan-out for the shutdown goodbye, the pairing pool
    /// (codes + trust gate), the device registry and the delta
    /// publisher the editor's write path notifies after each commit.
    v2: Option<Arc<pulpit_v2::V2State>>,
    /// Current touch-mode hotkey combo ("Ctrl+Alt+D" style).
    hotkey: std::sync::Mutex<String>,
    /// `pulpitApp/editor.json` - editor-local settings (hotkey), kept
    /// separate from the original app's settings.json.
    settings_path: Option<std::path::PathBuf>,
    /// `pulpitApp/aidev.json` - the AI dev-work producer's config (plan
    /// API keys, and the AI-usage tile selection under `status`).
    aidev_config: Option<std::path::PathBuf>,
    /// The native Spotify handle (cheap clone over the shared state the
    /// backend exec chain uses). `None` only when `spotify.json` exists
    /// but is unreadable - the Settings panel then shows Spotify as
    /// unavailable instead of silently wiping the file.
    spotify: Option<pulpit_spotify::Spotify>,
    /// `pulpitApp/spotify.json` - the Spotify config (client id + tokens).
    spotify_path: Option<std::path::PathBuf>,
    /// Editor picker cache: the user's playlists (the `spotify_playlists`
    /// command caches them for 60 s so opening the tile dialog does not
    /// hammer the API).
    spotify_playlists:
        std::sync::Mutex<Option<(std::time::Instant, Vec<pulpit_spotify::Playlist>)>>,
    #[allow(dead_code)]
    tools: Option<Arc<pulpit_tools::ToolManager>>,
}

impl DesktopState {
    fn backend(&self) -> Result<Arc<SqlBackend>, String> {
        self.backend.clone().ok_or_else(|| {
            "database unavailable - is the original Deckboard app still running?".into()
        })
    }

    fn broadcaster(&self) -> Result<&EditorBroadcaster, String> {
        self.broadcaster
            .as_ref()
            .ok_or_else(|| "database unavailable".into())
    }

    /// Broadcasts one committed editor write to the v2 tablets as a
    /// `boards.delta`. No-op when the v2 stack is unavailable. The ops are
    /// built from the post-commit DB state, so tablets never see a stale
    /// tile.
    fn publish_v2(&self, ops: Vec<pulpit_proto::BoardOp>) {
        if let Some(v2) = self.v2.as_ref() {
            v2.publish_delta(ops);
        }
    }

    /// Delta after a tile add/update/move: the full post-commit tile.
    fn publish_tile_set(&self, board_id: i64, tile_id: i64) {
        if let Some(v2) = self.v2.as_ref() {
            if let Some(op) = v2.tile_set_op(board_id, tile_id) {
                self.publish_v2(vec![op]);
            }
        }
    }

    /// Delta after a board create/update: the full post-commit board.
    fn publish_board_set(&self, board_id: i64) {
        if let Some(v2) = self.v2.as_ref() {
            if let Some(op) = v2.board_set_op(board_id) {
                self.publish_v2(vec![op]);
            }
        }
    }
}

/// One board together with its tiles - the editor's full state payload.
#[derive(Serialize)]
struct BoardWithButtons {
    #[serde(flatten)]
    board: BoardRow,
    buttons: Vec<ButtonRow>,
}

pub fn run() {
    // The release build is a windowed binary with no console, so logs go to
    // a daily-rotated file next to the rest of the pulpitApp data (only
    // `RUST_LOG` needs stderr for development). A missing home directory
    // keeps the stdout fallback rather than blocking startup.
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    if dirs::home_dir().is_some() {
        let log_dir = pulpit_db::data_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        prune_old_logs(&log_dir, "pulpit-desktop.log", 14);
        let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
            log_dir,
            "pulpit-desktop.log",
        ));
        // The guard owns the flush-worker thread; dropping it would lose the
        // tail of the log, and the writer must outlive `run()` anyway.
        std::mem::forget(guard);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(writer)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    // Cap the async worker pool: the tokio default is one worker per logical
    // core (28 threads on a big desktop), each costing stack plus runtime
    // bookkeeping. Four keep the editor and the embedded legacy server
    // responsive; blocking work (SQLite, extensions, COM) runs on the
    // separate spawn_blocking pool either way.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("build tokio runtime");
    tauri::async_runtime::set(runtime.handle().clone());
    // The global handle needs a live runtime behind it for the whole process
    // lifetime, so the owner is intentionally never dropped.
    std::mem::forget(runtime);

    // Must be the first plugin: a second launch would fight this instance
    // for port 8500 and the SQLite file, so it only surfaces the window.
    // `PULPIT_NO_SINGLE_INSTANCE=1` opts out (profiling side-by-side
    // builds against a copied database).
    let builder = tauri::Builder::default();
    let builder = if std::env::var_os("PULPIT_NO_SINGLE_INSTANCE").is_none() {
        builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
    } else {
        builder
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            // The embedded server keeps the tablets connected; closing the
            // window only hides it. The tray (Show / Hide, Quit Pulpit)
            // stays in charge of the real exit.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                HIDDEN_SINCE.store(
                    pulpit_v2::unix_millis(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let state = setup_core(app.handle().clone());
            app.manage(state);

            build_tray(app.handle())?;
            create_main_window(app.handle())?;
            spawn_webview_teardown(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            server_status,
            list_boards,
            create_board,
            update_board,
            delete_board,
            create_button,
            update_button,
            move_button,
            delete_button,
            clear_board,
            exec_button,
            list_audio_devices,
            exec_slider,
            get_settings,
            set_touch_mode_hotkey,
            set_server_port,
            get_autostart,
            set_autostart,
            read_image_data,
            list_known_inputs,
            list_lan_addresses,
            create_pairing_code,
            list_devices,
            revoke_device,
            resolve_operator_ask,
            take_pending_touch_toggle,
            adb_devices,
            adb_install_apk,
            check_for_updates,
            export_boards,
            import_boards,
            aidev_status_config,
            set_aidev_status_config,
            install_update,
            spotify_status,
            spotify_set_client_id,
            spotify_login,
            spotify_logout,
            spotify_playlists,
            spotify_devices,
            asset_data_url,
            exec_button_gesture,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // The idle sweep destroys the hidden WebView, which leaves the
            // process window-less for a while; the default reaction to that
            // (exit) would kill the server and the tray. Only an explicit
            // exit carries a code - the tray's Quit.
            match event {
                tauri::RunEvent::ExitRequested {
                    code: None, api, ..
                } => {
                    api.prevent_exit();
                }
                // A real exit (tray quit, OS shutdown when Tauri surfaces
                // it): tell the v2 tablets the exit is deliberate. The
                // once-guard keeps the tray quit from paying the flush
                // wait twice.
                tauri::RunEvent::ExitRequested { .. } => goodbye_v2(app),
                _ => {}
            }
        });
}

/// Delete rotated log files older than `keep_days` (by modification
/// time). Daily rotation with no cap grows forever; the app is a 24/7
/// tray resident, so this runs once per launch. Best-effort: a failed
/// delete only skips the file.
fn prune_old_logs(log_dir: &std::path::Path, prefix: &str, keep_days: u64) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(keep_days * 86_400));
    let Some(cutoff) = cutoff else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) || name == prefix {
            continue; // only rotated files (prefix.<date>), not today's
        }
        let ok = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|modified| modified < cutoff)
            .unwrap_or(false);
        if ok {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::debug!(file = %name, error = %e, "could not prune old log");
            }
        }
    }
}

/// Emit to the editor WebView only when its window can be seen: a
/// tray-hidden window cannot render pushes, and every emit is an IPC
/// round-trip with a second serialization of the payload. Tablets ride
/// the hub broadcasts and are unaffected. Periodic lanes re-deliver on
/// their next tick; change-gated callers must NOT advance their gate
/// when this returns false, or the shown window keeps a stale value.
fn emit_if_visible(app: &AppHandle, event: &str, payload: &serde_json::Value) -> bool {
    let visible = app
        .get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        let _ = app.emit(event, payload);
    }
    visible
}

/// The desktop host's [`pulpit_host::ClientFeed`] impl: the v2 state
/// engine, the legacy hub, and the editor WebView as the extra sink
/// (gated on window visibility by [`emit_if_visible`]). `engine` is
/// `None` when the v2 stack failed to start - pushes still reach the
/// WebView and the stock clients.
struct DesktopFeed {
    app: AppHandle,
    engine: Option<Arc<pulpit_v2::StateEngine>>,
    hub: Arc<Hub>,
}

impl pulpit_host::ClientFeed for DesktopFeed {
    fn engine_set(&self, key: &str, value: serde_json::Value) {
        if let Some(engine) = &self.engine {
            engine.set(&pulpit_v2::ext_channel(key), value);
        }
    }
    async fn broadcast_status(&self, payload: &str) {
        self.hub.broadcast("app_status_update", Some(payload)).await;
    }
    fn emit_status(&self, payload: &serde_json::Value) -> bool {
        emit_if_visible(&self.app, "app-status-update", payload)
    }
}

/// Open the database (read-write: the editor is now the single writer,
/// ADR-001), load extensions and start the embedded legacy server. A failure
/// keeps the UI alive with `backend: None` so the window can explain why.
fn setup_core(app: tauri::AppHandle) -> DesktopState {
    // leftover from a self-update: the running exe was renamed aside and
    // replaced; now nothing holds it and it can finally be deleted
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
    // Port 8500 is what the stock Android client hardcodes (and the
    // original app's default). `PULPIT_PORT` overrides it for
    // side-by-side runs; otherwise the port persisted in editor.json
    // (Ustawienia -> Serwer) applies.
    let data_dir = pulpit_db::data_dir();
    let settings_path = data_dir.join("editor.json");
    // PULPIT_AIDEV_CONFIG overrides the aidev config location (profiling
    // / hermetic runs), like PULPIT_DB for the database
    let aidev_config = std::env::var_os("PULPIT_AIDEV_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("aidev.json"));
    let stored_editor: serde_json::Value = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let port = effective_port(
        std::env::var("PULPIT_PORT").ok().as_deref(),
        stored_editor.get("port"),
    );

    // PULPIT_DB overrides the database location (profiling / hermetic runs)
    let db_path = std::env::var_os("PULPIT_DB").map(std::path::PathBuf::from);
    let db = pulpit_db::Db::open_read_write(db_path.as_deref());
    if let Err(e) = &db {
        tracing::error!("cannot open database read-write: {e}");
        return DesktopState {
            backend: None,
            broadcaster: None,
            hub: None,
            ext: None,
            port,
            v2: None,
            hotkey: std::sync::Mutex::new(DEFAULT_HOTKEY.to_string()),
            settings_path: None,
            aidev_config: Some(aidev_config),
            spotify: None,
            spotify_path: None,
            spotify_playlists: std::sync::Mutex::new(None),
            tools: None,
        };
    }
    let db = db.unwrap();

    // PULPIT_SPOTIFY_CONFIG overrides the config location (profiling /
    // hermetic runs), like PULPIT_AIDEV_CONFIG for aidev.
    let spotify_path = std::env::var_os("PULPIT_SPOTIFY_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("spotify.json"));
    // The desktop always builds the handle (a missing file is a default,
    // logged-out config) so Settings can paste a client id and log in.
    // Only an unreadable file (corrupt JSON) leaves Spotify disabled -
    // failing closed instead of overwriting the user's config.
    let spotify = match pulpit_spotify::SpotifyConfig::load(&spotify_path) {
        Ok(config) => Some(pulpit_spotify::Spotify::new(config, spotify_path.clone())),
        Err(e) => {
            tracing::error!("spotify.json exists but cannot be read: {e} - Spotify disabled");
            None
        }
    };

    let settings: serde_json::Value = std::fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let ext_dir = std::env::var_os("PULPIT_EXT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| data_dir.join("extensions"));
    // Extension inputs and every native declaration (Voicemeeter,
    // Discord, system-info, callurl, AI dev-work) register through the
    // shared host module, mirroring the headless server exactly.
    let (ext_manager, ext_events) =
        ExtManager::load(&ext_dir, &settings, &pulpit_host::native_replaced());
    for (package, name, error) in ext_manager.summary() {
        match error {
            Some(e) => tracing::warn!(package, name, error = e, "extension disabled"),
            None => tracing::info!(package, name, "extension ready"),
        }
    }
    pulpit_host::register_inputs(&ext_manager);

    let hub = Arc::new(Hub::new());
    let tools_path = data_dir.join("tools.json");
    let (tools, tools_rx) = pulpit_tools::spawn_tools(tools_path, hub.clone());
    // A timer's on-finish "switch board" acts on this host's own touch
    // mode: the same DOM event a board-switch tile's exec emits.
    let (tools_board_tx, mut tools_board_rx) = tokio::sync::mpsc::unbounded_channel::<i64>();
    tools.set_board_sink(tools_board_tx);
    let app_for_tools = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(board) = tools_board_rx.recv().await {
            let _ = app_for_tools.emit("change-board", board);
        }
    });

    let backend = Arc::new(
        SqlBackend::new(db)
            .with_extensions(ext_manager.clone())
            .with_discord(
                pulpit_discord::DiscordConfig::from_settings(&settings),
                data_dir.join("settings.json"),
            )
            .with_spotify(spotify.clone())
            .with_tools(Some(tools.clone())),
    );
    let broadcaster = EditorBroadcaster::new(hub.clone(), backend.clone());

    // Protocol v2 (docs/protocol-v2.md): same port, /v2/ws + /assets +
    // /v2/pair. Shares the backend with the legacy layer; a broken devices
    // list or asset store only disables v2, never the whole editor.
    let v2 = {
        let devices = pulpit_v2::DeviceStore::load(data_dir.join("devices.json"));
        let assets = pulpit_v2::AssetStore::open(data_dir.join("assets"));
        match (devices, assets) {
            (Ok(devices), Ok(assets)) => {
                tracing::info!("protocol v2 ready on the shared port");
                Some(Arc::new(pulpit_v2::V2State {
                    hub: Arc::new(pulpit_v2::V2Hub::new()),
                    backend: backend.clone() as Arc<dyn Backend>,
                    devices: Arc::new(devices),
                    pairing: Arc::new(pulpit_v2::Pairing::new()),
                    assets: Arc::new(assets),
                    engine: Arc::new(pulpit_v2::StateEngine::new(pulpit_proto::SERIES_CAP)),
                    generation: pulpit_v2::Generation::starting_at(1),
                    boards_cache: Default::default(),
                    pair_requests: Default::default(),
                    config: Default::default(),
                }))
            }
            (Err(e), _) | (_, Err(e)) => {
                tracing::error!("protocol v2 disabled: {e}");
                None
            }
        }
    };
    // Fresh pairings need the operator's approval (audit B2 step 6): the
    // gate asks in a native dialog. The session bounds the wait by the
    // pairing-code TTL, so an unanswered dialog (nobody at the desk)
    // denies the pairing once the code would have expired anyway; a late
    // answer is dropped - the one-time code is burned, a retry needs a
    // fresh one.
    if let Some(v2) = &v2 {
        let app = app.clone();
        v2.pairing
            .set_trust_gate(move |name| ask_operator(&app, "trust", name, None));
    }
    // M8 Bluetooth-style pair-requests (plan 014): a tablet that found us
    // over mDNS asks to pair; the editor popup shows the verification code
    // the tablet is displaying too - pairing completes only when the
    // numbers match and the operator confirms.
    if let Some(v2) = &v2 {
        let app = app.clone();
        v2.pairing.set_pair_request_gate(move |name, code| {
            ask_operator(&app, "pair-request", name, Some(code.to_string()))
        });
    }
    // M8 discovery: announce the server on mDNS so tablets can find it
    // without typing an address. Failure is non-fatal (manual pairing
    // keeps working); the handle stays alive for the process lifetime.
    // `PULPIT_NO_DISCOVERY=1` skips it: a side-by-side instance would
    // announce the same "Pulpit on <host>" name on another port.
    if std::env::var_os("PULPIT_NO_DISCOVERY").is_none() {
        match pulpit_v2::discovery::advertise(port, env!("CARGO_PKG_VERSION")) {
            Ok(discovery) => std::mem::forget(discovery),
            Err(e) => tracing::warn!("mDNS advertisement failed: {e}"),
        }
    }
    // v2 background task: coalesced state patches.
    if let Some(v2) = &v2 {
        tauri::async_runtime::spawn(pulpit_v2::run_flusher(
            v2.engine.clone(),
            v2.hub.clone(),
            v2.config.patch_interval,
        ));
    }
    // Extension timers stretch to IDLE_TICK_FLOOR while no client is
    // watching (see ExtManager::set_activity); the shared activity loop
    // keeps the count current.
    tauri::async_runtime::spawn(pulpit_host::activity_loop(
        ext_manager.clone(),
        hub.clone(),
        v2.as_ref().map(|v2| v2.hub.clone()),
    ));

    // Producer pumps (extension fleet, native system-info, AI dev-work,
    // speaker watcher) live in the shared host module too - one
    // implementation, same lanes and change-gating as the headless
    // server (CORE-06), with the WebView as this host's extra sink.
    let feed = Arc::new(DesktopFeed {
        app: app.clone(),
        engine: v2.as_ref().map(|v2| v2.engine.clone()),
        hub: hub.clone(),
    });
    tauri::async_runtime::spawn(pulpit_host::forward_ext_events(
        feed.clone(),
        ext_events,
    ));
    tauri::async_runtime::spawn(pulpit_host::forward_producer(
        feed.clone(),
        pulpit_sysinfo::spawn_push(),
    ));
    tauri::async_runtime::spawn(pulpit_host::forward_producer(
        feed.clone(),
        pulpit_aidev::spawn_push(pulpit_host::aidev_paths(aidev_config.clone())),
    ));
    tauri::async_runtime::spawn(pulpit_host::speaker_watch(
        feed.clone(),
        backend.clone() as Arc<dyn Backend>,
    ));
    tauri::async_runtime::spawn(pulpit_host::forward_producer(
        feed.clone(),
        tools_rx,
    ));

    // Spotify (when its config is readable): the poller's consumers
    // signal counts connected legacy + v2 clients plus this host's extra
    // consumer - the visible editor window (a tray-hidden window renders
    // nothing). Snapshots ride the shared spotify pump, which strips the
    // internal art key and imports album art into the v2 asset store.
    {
        match &spotify {
            Some(spotify) => {
                let app_for_consumers = app.clone();
                let extra: Arc<dyn Fn() -> usize + Send + Sync> = Arc::new(move || {
                    app_for_consumers
                        .get_webview_window("main")
                        .map(|w| w.is_visible().unwrap_or(false))
                        .unwrap_or(false) as usize
                });
                let consumers = pulpit_host::consumer_reader(
                    hub.clone(),
                    v2.as_ref().map(|v2| v2.hub.clone()),
                    Some(extra),
                );
                tauri::async_runtime::spawn(pulpit_host::spotify::forward_spotify(
                    feed.clone(),
                    pulpit_spotify::spawn_push(spotify.clone(), consumers),
                    v2.as_ref().map(|v2| v2.assets.clone()),
                ));
            }
            None => {
                // Spotify disabled: say so once (`spotify-auth: "off"`),
                // no poller exists.
                tauri::async_runtime::spawn(pulpit_host::spotify::forward_spotify_disabled(
                    feed.clone(),
                ));
            }
        }
    }

    let state = Arc::new(AppState {
        hub: hub.clone(),
        backend: backend.clone() as Arc<dyn Backend>,
    });
    // one router for both protocols; ConnectInfo is needed by the loopback
    // guard on POST /v2/pair
    let app_router = match &v2 {
        Some(v2) => pulpit_legacy::router(state.clone()).merge(pulpit_v2::router(v2.clone())),
        None => pulpit_legacy::router(state.clone()),
    };
    let v2_reaper = v2
        .as_ref()
        .map(|v2| (v2.hub.clone(), v2.config.ping_interval));
    tauri::async_runtime::spawn(async move {
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        // A busy port must not cost the whole session: a leftover instance
        // releasing 8500 (or the old app finishing its shutdown) self-heals.
        let listener = loop {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => break l,
                Err(e) => {
                    tracing::error!("cannot bind {addr}: {e} - retrying in 5 s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        };
        tracing::info!("legacy server listening on {addr} (legacy /socket.io/ + v2 /v2/ws)");
        // Engine.IO: reap sessions silent longer than pingInterval+pingTimeout
        let hub_reaper = state.hub.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                hub_reaper.reap(75).await;
            }
        });
        if let Some((v2_hub, ping_interval)) = v2_reaper {
            // v2: three missed pongs mean the peer is gone without a TCP
            // close (see pulpit_v2::run_reaper)
            tauri::async_runtime::spawn(pulpit_v2::run_reaper(v2_hub, ping_interval));
        }
        if let Err(e) = axum::serve(
            listener,
            app_router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            tracing::error!("legacy server stopped: {e}");
        }
    });

    let mut hotkey = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("hotkey").and_then(|h| h.as_str()).map(str::to_string))
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
    if let Err(e) = register_touch_mode_hotkey(&app, &hotkey) {
        // fallback like the original: a stored combo another app now owns
        // must not leave the user without any hotkey
        tracing::warn!("stored hotkey unusable ({e}) - falling back to {DEFAULT_HOTKEY}");
        if register_touch_mode_hotkey(&app, DEFAULT_HOTKEY).is_ok() {
            hotkey = DEFAULT_HOTKEY.to_string();
        } else {
            // nothing is registered: record that as an empty combo so a
            // later save of the same string re-registers instead of
            // no-op-ing (hotkey_plan treats an unparseable old as Register)
            tracing::error!("default hotkey also unusable - no hotkey registered");
            hotkey = String::new();
        }
    }

    DesktopState {
        backend: Some(backend),
        broadcaster: Some(broadcaster),
        hub: Some(hub),
        ext: Some(ext_manager),
        port,
        v2: v2.clone(),
        hotkey: std::sync::Mutex::new(hotkey),
        settings_path: Some(settings_path),
        aidev_config: Some(aidev_config),
        spotify,
        spotify_path: Some(spotify_path),
        spotify_playlists: std::sync::Mutex::new(None),
        tools: Some(tools),
    }
}

// ---- tray + hotkey ---------------------------------------------------------

/// Unix millis of the moment the main window was hidden; 0 means visible
/// or not created. The teardown sweep below reads it.
static HIDDEN_SINCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long the goodbye waits for the session pumps to put the frame and
/// the WS close on the wire before the process exits.
const SHUTDOWN_FLUSH_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// How long the window may stay tray-hidden before its WebView is torn
/// down. The WebView2 process tree costs ~60-150 MB resident around the
/// clock; the embedded server, tray and tablets are unaffected. Showing
/// the window again rebuilds the UI from scratch (the Vue app refetches
/// everything on mount). `PULPIT_WEBVIEW_TEARDOWN_SECS` overrides the
/// threshold (profiling / tests).
fn webview_teardown_after() -> std::time::Duration {
    std::env::var("PULPIT_WEBVIEW_TEARDOWN_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(10 * 60))
}

/// Sweep: destroy the WebView once the window has been hidden longer
/// than `WEBVIEW_TEARDOWN_AFTER`. Runs on the 30 s loop; the window is
/// recreated by whatever shows it next (tray, second launch).
fn spawn_webview_teardown(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let hidden_at = HIDDEN_SINCE.load(std::sync::atomic::Ordering::Relaxed);
            if hidden_at == 0 {
                continue;
            }
            let hidden_for = pulpit_v2::unix_millis().saturating_sub(hidden_at);
            if hidden_for < webview_teardown_after().as_millis() as u64 {
                continue;
            }
            if let Some(window) = app.get_webview_window("main") {
                match window.is_visible() {
                    Ok(false) => {
                        tracing::info!(
                            hidden_for_secs = hidden_for / 1000,
                            "window hidden too long - tearing the WebView down"
                        );
                        HIDDEN_SINCE.store(0, std::sync::atomic::Ordering::Relaxed);
                        let _ = window.destroy();
                    }
                    Ok(true) => {
                        // raced a manual show; the flag is stale
                        HIDDEN_SINCE.store(0, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(_) => {}
                }
            } else {
                HIDDEN_SINCE.store(0, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });
}

/// Show (or rebuild) the main window and clear the hidden flag.
fn show_main_window(app: &AppHandle) {
    HIDDEN_SINCE.store(0, std::sync::atomic::Ordering::Relaxed);
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        // torn down by the idle sweep: rebuild; the fresh Vue app
        // refetches boards, settings and live state on mount
        None => {
            tracing::info!("rebuilding the main window WebView");
            if let Err(e) = create_main_window(app) {
                tracing::error!("could not rebuild the main window: {e}");
            }
        }
    }
}

/// Tells connected v2 tablets this exit is deliberate (docs/protocol-v2.md
/// §9): one `server.shutdown` frame per session, then a WS close. Without
/// it a quit looks like a network drop and tablets retry into the void.
/// Best effort by design - the process exits either way.
fn goodbye_v2(app: &AppHandle) {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Some(hub) = app
        .try_state::<DesktopState>()
        .and_then(|state| state.v2.as_ref().map(|v2| v2.hub.clone()))
    else {
        return;
    };
    hub.shutdown();
    // The session pumps write the frame + close asynchronously; give them
    // a beat before `exit` tears the process down.
    std::thread::sleep(SHUTDOWN_FLUSH_GRACE);
}

/// The main window is built here instead of `tauri.conf.json` because the
/// WebView2 argument list is only reachable through the builder API, and a
/// custom list replaces the default one - so the stock feature disables are
/// repeated alongside the memory-oriented flags (no background networking,
/// no component updates, capped HTTP disk cache).
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("Pulpit")
        .inner_size(1280.0, 800.0)
        .min_inner_size(900.0, 600.0)
        .theme(Some(tauri::Theme::Dark))
        .additional_browser_args(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --disable-background-networking --disable-component-update \
             --disk-cache-size=33554432",
        )
        .build()?;
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{CheckMenuItem, Menu, MenuItem};
    use tauri_plugin_autostart::ManagerExt;

    let show_hide = MenuItem::with_id(app, "show-hide", "Show / Hide", true, None::<&str>)?;
    let touch = MenuItem::with_id(app, "touch-mode", "Toggle Touch Mode", true, None::<&str>)?;
    let launch = CheckMenuItem::with_id(
        app,
        "autostart",
        "Launch at startup",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Pulpit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_hide, &touch, &launch, &quit])?;

    // The check handle must outlive the builder: muda check items do
    // not self-toggle, so the handler flips it after a successful
    // enable/disable (DESK-08) - without this the checkmark kept its
    // build-time state until the next app restart.
    let launch_item = launch.clone();
    tauri::tray::TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("app icon").clone())
        .tooltip("Pulpit")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show-hide" => toggle_main_window(app),
            "touch-mode" => toggle_touch_mode(app),
            "autostart" => {
                use tauri_plugin_autostart::ManagerExt;
                let launch = app.autolaunch();
                let enabled = launch.is_enabled().unwrap_or(false);
                let result = if enabled {
                    launch.disable()
                } else {
                    launch.enable()
                };
                match result {
                    Ok(()) => {
                        let _ = launch_item.set_checked(!enabled);
                    }
                    Err(e) => tracing::warn!(error = %e, "could not toggle autostart"),
                }
            }
            "quit" => {
                goodbye_v2(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false) {
            HIDDEN_SINCE.store(
                pulpit_v2::unix_millis(),
                std::sync::atomic::Ordering::Relaxed,
            );
            let _ = window.hide();
        } else {
            show_main_window(app);
        }
    } else {
        show_main_window(app);
    }
}

/// A touch-mode toggle that arrived while the WebView was torn down
/// (DESK-07): the fresh app takes it once its listeners and boards are
/// up, via the [`take_pending_touch_toggle`] command.
static PENDING_TOUCH_TOGGLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Toggle touch mode from the tray item or the global hotkey (DESK-07).
/// The idle sweep may have torn the WebView down - emitting
/// `toggle-touch-mode` then reaches nobody and, worse, no window
/// appears. With a live window the emit is delivered (and the window
/// surfaced so the flip is visible); without one the window is rebuilt
/// and the intended toggle parked in [`PENDING_TOUCH_TOGGLE`] for the
/// fresh app to consume on mount - an emit racing the page load would
/// be lost.
fn toggle_touch_mode(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(window) => {
            let _ = window.show();
            let _ = app.emit("toggle-touch-mode", ());
        }
        None => {
            tracing::info!("rebuilding the main window WebView for the touch toggle");
            if let Err(e) = create_main_window(app) {
                tracing::error!("could not rebuild the main window: {e}");
                return;
            }
            PENDING_TOUCH_TOGGLE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Register the touch-mode hotkey. The combo is user-configurable
/// (`pulpitApp/editor.json`, default Ctrl+Alt+D - the original's
/// `toggleTouchMode` concept); an unusable stored combo falls back to the
/// default with a warning.
fn register_touch_mode_hotkey(app: &AppHandle, combo: &str) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let shortcut = match combo.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        Ok(s) => s,
        Err(_) => {
            let msg = format!("invalid hotkey \"{combo}\" - use e.g. Ctrl+Alt+D");
            tracing::warn!("{msg}");
            return Err(msg);
        }
    };
    let result = app
        .global_shortcut()
        .on_shortcut(shortcut, |app, _s, event| {
            if event.state() == ShortcutState::Pressed {
                toggle_touch_mode(app);
            }
        });
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = format!("could not register hotkey \"{combo}\": {e}");
            tracing::warn!("{msg}");
            Err(msg)
        }
    }
}

/// The default touch-mode hotkey, used when a stored combo is unusable.
const DEFAULT_HOTKEY: &str = "Ctrl+Alt+D";

/// What a hotkey change (012 A4) has to do with (old, new): re-registering
/// the combo that is already active would fail with "already registered",
/// so an identical pair is a no-op; anything else registers the new combo
/// first so a failure leaves the old one working. A stored combo that no
/// longer parses must not block a change - there is nothing to lose.
enum HotkeyPlan {
    Noop,
    Register,
}

fn hotkey_plan(old: &str, new: &str) -> Result<HotkeyPlan, String> {
    let new_shortcut: tauri_plugin_global_shortcut::Shortcut = new
        .parse()
        .map_err(|_| format!("invalid shortcut \"{new}\" - use e.g. Ctrl+Alt+D"))?;
    match old.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        Ok(old_shortcut) if old_shortcut == new_shortcut => Ok(HotkeyPlan::Noop),
        _ => Ok(HotkeyPlan::Register),
    }
}

// ---- tauri commands --------------------------------------------------------

#[tauri::command]
async fn server_status(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<serde_json::Value, String> {
    let clients = match &state.hub {
        Some(h) => h.len().await,
        None => 0,
    };
    Ok(serde_json::json!({
        "dbOk": state.backend.is_some(),
        "port": state.port,
        "clients": clients,
        "version": app.package_info().version.to_string(),
    }))
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// Action/style metadata for everything the editor did not statically ship:
/// extension inputs and device declarations (Voicemeeter, Discord). The
/// editor merges this into its action dropdown and uses it to style tiles
/// that carry no icon/color of their own.
#[tauri::command]
fn list_known_inputs(state: State<'_, DesktopState>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Some(ext) = &state.ext {
        for input in ext.inputs() {
            out.push(serde_json::json!({
                "value": input.value,
                "label": input.label,
                "icon": input.icon,
                "color": input.color,
                "mode": input.mode,
                "command": input.command,
                "source": "extension",
                "extension": input.extension,
                "fields": input.fields.iter().map(|f| serde_json::json!({
                    "kind": f.kind,
                    "label": f.label,
                    "key": f.key,
                    "items": f.items.iter().map(|(v, l)| serde_json::json!({
                        "value": v,
                        "label": l,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }));
        }
    }
    for (value, icon, _font_icon, color) in pulpit_vm::input_declarations() {
        out.push(serde_json::json!({
            "value": value,
            "icon": icon,
            "color": color,
            "mode": serde_json::Value::Null,
            "command": serde_json::Value::Null,
            "source": "device",
        }));
    }
    for (value, icon, color, mode) in pulpit_discord::input_declarations() {
        out.push(serde_json::json!({
            "value": value,
            "icon": icon,
            "color": color,
            "mode": mode,
            "command": serde_json::Value::Null,
            "source": "device",
        }));
    }
    for (value, icon, color, mode) in pulpit_spotify::input_declarations() {
        out.push(serde_json::json!({
            "value": value,
            "icon": icon,
            "color": color,
            "mode": mode,
            "command": serde_json::Value::Null,
            "source": "device",
        }));
    }
    out
}

#[tauri::command]
fn set_autostart(app: AppHandle, enable: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let launch = app.autolaunch();
    if enable {
        launch.enable().map_err(|e| e.to_string())
    } else {
        launch.disable().map_err(|e| e.to_string())
    }
}

/// Hard cap on images read into the WebView as data URLs (012 B3): the
/// path comes from the frontend, so a giant file must not be base64'd
/// into memory. 10 MiB is far above any sensible tile icon.
const IMAGE_READ_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Read an image file and return it as a data URL for tile backgrounds and
/// icons. Done in Rust so no filesystem plugin/scope is needed.
#[tauri::command]
async fn read_image_data(path: String) -> Result<String, String> {
    // up to a 10 MiB read plus a base64 pass: run it on the blocking
    // pool, not the main thread (sync commands) or an async worker
    tauri::async_runtime::spawn_blocking(move || read_image_data_blocking(&path))
        .await
        .map_err(|e| e.to_string())?
}

/// The blocking half of [`read_image_data`], split out so the size-cap
/// behavior stays testable without a runtime.
fn read_image_data_blocking(path: &str) -> Result<String, String> {
    use base64::Engine;
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => return Err(format!("unsupported image type \".{ext}\"")),
    };
    // take() bounds the read itself: a file that grows between the length
    // check and the read still cannot pull more than cap+1 bytes in
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    use std::io::Read as _;
    let mut bytes = Vec::new();
    file.take(IMAGE_READ_CAP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > IMAGE_READ_CAP_BYTES {
        return Err(format!(
            "image \".{ext}\" is larger than 10 MiB - pick a smaller file"
        ));
    }
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// One reachable LAN endpoint shown in the "Connect a tablet" popover.
#[derive(Serialize)]
struct LanAddress {
    /// Interface name ("Ethernet", "Wi-Fi"), like the original's list.
    name: String,
    ipv4: String,
    /// Data URL with the QR the stock client scans - it encodes the bare
    /// IPv4 (the client appends port 8500 itself), matching the original.
    qr: String,
}

/// The QR payload for one address, as an `image/svg+xml` data URL. Pure so
/// the QR contract is testable without any interface present.
fn qr_data_url(text: &str) -> Option<String> {
    use base64::Engine;
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .dark_color(qrcode::render::svg::Color("#242424"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Some(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg.as_bytes())
    ))
}

/// Reachable LAN endpoints as (interface name, IPv4) pairs: IPv4, up,
/// non-loopback, deduplicated by address. Empty when the machine is offline.
fn lan_ipv4s() -> Vec<(String, String)> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("cannot enumerate network interfaces: {e}");
            return Vec::new();
        }
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for iface in &interfaces {
        if iface.is_loopback() || !iface.is_oper_up() {
            continue;
        }
        let ip = match iface.addr {
            if_addrs::IfAddr::V4(ref v4) => v4.ip,
            if_addrs::IfAddr::V6(_) => continue,
        };
        let ipv4 = ip.to_string();
        if out.iter().any(|(_, seen)| seen == &ipv4) {
            continue;
        }
        out.push((iface.name.clone(), ipv4));
    }
    out
}

/// LAN endpoints a tablet can reach, each with the QR the stock client
/// scans (it encodes the bare IP; the client appends port 8500 itself).
#[tauri::command]
fn list_lan_addresses() -> Vec<LanAddress> {
    lan_ipv4s()
        .into_iter()
        .filter_map(|(name, ipv4)| {
            Some(LanAddress {
                name,
                ipv4: ipv4.clone(),
                qr: qr_data_url(&ipv4)?,
            })
        })
        .collect()
}

/// A minted one-time pairing code plus the per-address QR for the v2
/// client (`pulpit://<ip>:<port>?pair=<code>`, docs/protocol-v2.md).
#[derive(Serialize)]
struct PairingOffer {
    code: String,
    expires_in_secs: u64,
    addresses: Vec<LanAddress>,
}

#[tauri::command]
fn create_pairing_code(state: State<'_, DesktopState>) -> Result<PairingOffer, String> {
    let v2 = v2_or_err(&state)?;
    let code = v2.pairing.new_code();
    let addresses = lan_ipv4s()
        .into_iter()
        .map(|(name, ipv4)| {
            let url = pairing_shape(&ipv4, state.port, &code);
            LanAddress {
                name,
                ipv4,
                qr: qr_data_url(&url).unwrap_or_default(),
            }
        })
        .collect();
    // Code-free mint log line, the exact text the v2 routes use (audit
    // B2 step 1 / CORE-03: logs outlive the 5-minute TTL, and pairing
    // auto-accepts, so a logged code is a standing invite).
    tracing::info!("{}", pulpit_v2::pair_minted_message());
    Ok(PairingOffer {
        code,
        expires_in_secs: pulpit_v2::PAIR_CODE_TTL.as_secs(),
        addresses,
    })
}

/// The operator gates (B2 trust prompt + M8 pair-request) ask through
/// the editor's own pairing popup - a styled in-app modal - instead of a
/// native dialog. `ask_operator` emits `operator-ask` to the webview and
/// parks the calling worker thread on a channel until
/// [`resolve_operator_ask`] answers (or the timeout denies). Runs on the
/// pairing session's blocking thread / the pair-request worker, so the
/// wait costs no socket-loop time. Deny-safe by construction: a closed
/// webview, an emit failure, a timeout or a dropped channel all deny -
/// nothing here can accidentally approve.
fn pending_asks() -> &'static std::sync::Mutex<HashMap<String, std::sync::mpsc::Sender<bool>>> {
    static PENDING: OnceLock<std::sync::Mutex<HashMap<String, std::sync::mpsc::Sender<bool>>>> =
        OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// How long the popup may wait for the operator: the pairing-code TTL
/// (5 min) plus a grace gap, so the popup outlives the code it gates.
const OPERATOR_ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(360);

fn next_ask_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!(
        "ask-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn ask_operator(app: &AppHandle, kind: &str, name: &str, code: Option<String>) -> bool {
    let id = next_ask_id();
    let (tx, rx) = std::sync::mpsc::channel();
    pending_asks()
        .lock()
        .expect("asks poisoned")
        .insert(id.clone(), tx);
    let payload = serde_json::json!({ "id": id, "kind": kind, "name": name, "code": code });
    if let Err(e) = app.emit("operator-ask", payload) {
        tracing::warn!(error = %e, "operator ask: webview emit failed - denying");
        pending_asks().lock().expect("asks poisoned").remove(&id);
        return false;
    }
    tracing::info!(kind, name, "operator ask shown in the editor");
    let answer = match rx.recv_timeout(OPERATOR_ASK_TIMEOUT) {
        Ok(approved) => approved,
        Err(_) => {
            tracing::warn!(kind, name, "operator ask unanswered - denying");
            false
        }
    };
    pending_asks().lock().expect("asks poisoned").remove(&id);
    answer
}

/// The editor's answer to a shown ask. Unknown ids (already resolved or
/// expired) are errors so a double-click cannot resurrect a decision.
#[tauri::command]
fn resolve_operator_ask(id: String, approved: bool) -> Result<(), String> {
    let sender = pending_asks().lock().expect("asks poisoned").remove(&id);
    match sender {
        Some(tx) => tx
            .send(approved)
            .map_err(|_| "ask already gone".to_string()),
        None => Err("no such ask (already resolved or expired)".to_string()),
    }
}

/// A paired device for the settings UI: registry metadata only - no
/// token material (plaintext or digest) ever leaves the backend.
#[derive(Serialize)]
struct DeviceInfo {
    id: String,
    name: String,
    created: u64,
    last_seen: u64,
}

// ---- M8: APK sideload + update check ----------------------------------

/// One adb device for the sideload UI (`serial (state)`).
#[tauri::command]
async fn adb_devices() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let out = std::process::Command::new("adb")
            .args(["devices"])
            .output()
            .map_err(|e| format!("adb nie znaleziony w PATH: {e}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let devices = text
            .lines()
            .filter(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                parts.len() == 2 && parts[1] != "devices" && !line.starts_with('*')
            })
            .map(|line| {
                let mut parts = line.split_whitespace();
                format!(
                    "{} ({})",
                    parts.next().unwrap_or("?"),
                    parts.next().unwrap_or("?")
                )
            })
            .collect();
        Ok(devices)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Installs an APK on a connected device (`adb install -r`, keep-data
/// reinstall). Runs on the blocking pool - an install over USB can take
/// a while - and returns adb's own output so the UI can show exactly
/// what happened.
#[tauri::command]
async fn adb_install_apk(path: String) -> Result<String, String> {
    if !path.to_lowercase().ends_with(".apk") {
        return Err("Wybierz plik .apk.".to_string());
    }
    if !std::path::Path::new(&path).exists() {
        return Err(format!("Plik nie istnieje: {path}"));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let out = std::process::Command::new("adb")
            .args(["install", "-r", &path])
            .output()
            .map_err(|e| format!("adb nie znaleziony w PATH: {e}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .trim()
        .to_string();
        if out.status.success() && text.contains("Success") {
            Ok(text)
        } else {
            Err(if text.is_empty() {
                "adb install nie powiodł się (brak wyjścia)".to_string()
            } else {
                text
            })
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Default update feed: a `latest.json` in the public repo
/// (`{"version": "...", "url": "...", "sha256": "..."}`). editor.json's
/// `update_url` overrides it; the literal "off" disables checking.
const DEFAULT_UPDATE_URL: &str =
    "https://raw.githubusercontent.com/TymekTM/deckboard/main/latest.json";

/// The effective update feed: editor.json's `update_url` wins ("off"
/// disables), the public repo manifest is the default.
fn resolve_update_url(stored: Option<&str>) -> Option<String> {
    match stored.map(str::trim) {
        Some("off") => None,
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => Some(DEFAULT_UPDATE_URL.to_string()),
    }
}

/// A parsed update manifest: where to fetch and how to verify.
/// `sha256` is REQUIRED (DESK-02): the digest is the only integrity
/// mechanism in the flow, so a manifest without it is refused instead
/// of silently installing an unverified binary.
struct UpdateManifest {
    version: String,
    url: String,
    sha256: String,
}

fn parse_update_manifest(text: &str) -> Result<UpdateManifest, String> {
    let manifest: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Manifest nie jest JSON-em: {e}"))?;
    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("Manifest nie ma pola \"version\".")?
        .to_string();
    let url = manifest
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if url.is_empty() {
        return Err("Manifest nie ma pola \"url\".".to_string());
    }
    if !url.starts_with("https://") {
        return Err("Adres pliku aktualizacji musi być HTTPS.".to_string());
    }
    let sha256 = manifest
        .get("sha256")
        .and_then(|v| v.as_str())
        .ok_or("Manifest nie ma pola \"sha256\" - bez sumy kontrolnej odmowa instalacji.")?
        .to_string();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Pole \"sha256\" manifestu nie jest sumą kontrolną (64 znaki hex).".to_string());
    }
    Ok(UpdateManifest {
        version,
        url,
        sha256,
    })
}

fn fetch_manifest(url: &str) -> Result<UpdateManifest, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("Nie udało się pobrać manifestu: {e}"))?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Nie udało się odczytać manifestu: {e}"))?;
    parse_update_manifest(&text)
}

/// Numeric dot-version compare ("0.1.2" vs "v0.2.0"): true when `latest`
/// is strictly newer than `current`. Non-numeric parts read as 0.
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect()
    };
    let (l, c) = (parse(latest), parse(current));
    for i in 0..l.len().max(c.len()) {
        let a = l.get(i).copied().unwrap_or(0);
        let b = c.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// Checks the update feed for a newer version. No download, no install:
/// the UI shows the result and the release URL.
#[tauri::command]
async fn check_for_updates(state: State<'_, DesktopState>) -> Result<serde_json::Value, String> {
    let url = {
        let path = state
            .settings_path
            .as_ref()
            .ok_or("Brak ścieżki ustawień (uruchomienie awaryjne).")?;
        let stored: serde_json::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);
        resolve_update_url(stored.get("update_url").and_then(|v| v.as_str()))
            .ok_or("Sprawdzanie aktualizacji jest wyłączone (update_url: \"off\").")?
    };
    let current = env!("CARGO_PKG_VERSION").to_string();
    let manifest =
        tauri::async_runtime::spawn_blocking(move || fetch_manifest(url.trim()))
            .await
            .map_err(|e| e.to_string())??;
    let update_available = version_newer(&manifest.version, &current);
    Ok(serde_json::json!({
        "current": current,
        "latest": manifest.version,
        "update_available": update_available,
        "url": manifest.url,
    }))
}

/// Download size cap: the desktop exe is ~21 MiB today; anything past
/// this is a wrong or hostile asset and the update aborts.
const UPDATE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Download the asset into `dest`, enforcing the size cap.
fn download_update(url: &str, dest: &std::path::Path) -> Result<(), String> {
    use std::io::Read;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .build()
        .new_agent();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("Nie udało się pobrać aktualizacji: {e}"))?;
    let mut body = resp
        .into_body()
        .into_reader()
        .take(UPDATE_MAX_BYTES + 1);
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let written =
        std::io::copy(&mut body, &mut file).map_err(|e| format!("Pobieranie przerwane: {e}"))?;
    if written > UPDATE_MAX_BYTES {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "Plik aktualizacji ma {written} B - powyżej limitu {UPDATE_MAX_BYTES} B."
        ));
    }
    Ok(())
}

/// Verify a lowercase hex sha256 digest against the file's bytes.
fn sha256_matches(path: &std::path::Path, expected: &str) -> Result<bool, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let digest = hex::encode(Sha256::digest(&bytes));
    Ok(digest.eq_ignore_ascii_case(expected.trim()))
}

/// Swap the freshly downloaded exe into place: the running exe renames
/// fine on NTFS, the download then takes its name. On any failure the
/// rename is rolled back so the current install keeps working.
fn apply_update(exe: &std::path::Path, downloaded: &std::path::Path) -> Result<(), String> {
    let old = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| format!("Nie udało się odłożyć starego exe: {e}"))?;
    if let Err(e) = std::fs::rename(downloaded, exe) {
        // put the running version back; the app must keep working
        let _ = std::fs::rename(&old, exe);
        return Err(format!("Nie udało się podmienić pliku: {e}"));
    }
    Ok(())
}

/// The cmd.exe script the update guard runs (DESK-02): wait out this
/// process's exit, then either start the swapped exe (the normal path),
/// or - when the process died between the swap's two renames, leaving
/// no exe in place - finish the swap first: the staged download (or, as
/// the last resort, the rolled-back `.old`) takes the exe's name and
/// starts, so a crash mid-swap cannot leave the machine with no exe.
/// Every path rides in quotes: spaces, `&` and parentheses in an
/// install path must stay literal.
fn relaunch_script(
    exe: &std::path::Path,
    staged: &std::path::Path,
    old: &std::path::Path,
) -> String {
    let (exe, staged, old) = (
        exe.to_string_lossy(),
        staged.to_string_lossy(),
        old.to_string_lossy(),
    );
    format!(
        "/C timeout /t 2 /nobreak >nul & \
         if exist \"{exe}\" (start \"\" \"{exe}\") \
         else if exist \"{staged}\" (move /y \"{staged}\" \"{exe}\" >nul & start \"\" \"{exe}\") \
         else if exist \"{old}\" (move /y \"{old}\" \"{exe}\" >nul & start \"\" \"{exe}\")"
    )
}

/// Spawn the detached recovery guard BEFORE the swap starts, so the
/// crash window between the two renames is covered. Returned as a
/// [`std::process::Child`] so a failed swap can kill it before its
/// timeout fires; on success the caller must [`std::mem::forget`] the
/// child - the guard has to outlive this process to relaunch it.
fn spawn_update_guard(
    exe: &std::path::Path,
    staged: &std::path::Path,
) -> Result<std::process::Child, String> {
    use std::os::windows::process::CommandExt;
    const DETACHED: u32 = 0x0000_0008;
    const NO_WINDOW: u32 = 0x0800_0000;
    // raw_arg passes the script verbatim: Rust's default arg escaping
    // would wrap it in quotes and backslash-escape the inner ones,
    // which cmd.exe does not understand - the paths would arrive
    // mangled. Paths cannot contain a literal `"` on Windows; a
    // %-sequence in a path would still expand, like in every cmd line.
    let mut cmd = std::process::Command::new("cmd");
    cmd.raw_arg(relaunch_script(
        exe,
        staged,
        &exe.with_extension("exe.old"),
    ))
    .creation_flags(DETACHED | NO_WINDOW)
    .spawn()
    .map_err(|e| format!("Nie udało się zaplanować restartu: {e}"))
}

/// Download (manifest URL again, so the check cannot go stale), verify
/// and stage the new exe, then swap and restart. The `.old` copy is
/// removed by the next launch's cleanup sweep.
#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> Result<String, String> {
    let (url, staging_dir) = {
        let path = state
            .settings_path
            .as_ref()
            .ok_or("Brak ścieżki ustawień (uruchomienie awaryjne).")?;
        let stored: serde_json::Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);
        let feed = resolve_update_url(stored.get("update_url").and_then(|v| v.as_str()))
            .ok_or("Sprawdzanie aktualizacji jest wyłączone (update_url: \"off\").")?;
        let dir = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("updates");
        (feed, dir)
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let manifest = fetch_manifest(&url)?;
        let current = env!("CARGO_PKG_VERSION");
        if !version_newer(&manifest.version, current) {
            return Err(format!(
                "Zainstalowana wersja {current} jest aktualna (feed: {}).",
                manifest.version
            ));
        }
        std::fs::create_dir_all(&staging_dir).map_err(|e| e.to_string())?;
        let staged = staging_dir.join("pulpit-desktop.new");
        let _ = std::fs::remove_file(&staged);
        download_update(&manifest.url, &staged)?;
        // unconditional: sha256 is required by the parser, so an
        // unverified binary can never be swapped in (DESK-02)
        if !sha256_matches(&staged, &manifest.sha256)? {
            let _ = std::fs::remove_file(&staged);
            return Err(
                "Suma kontrolna pobranego pliku się nie zgadza - instalacja przerwana.".to_string(),
            );
        }
        // The guard is alive BEFORE the first rename: a death between
        // the swap's two renames (no exe in place) gets healed by its
        // script instead of bricking the install. A failed swap kills
        // it before its timeout fires; a successful one forgets it so
        // it survives this process as the relauncher.
        let mut guard = spawn_update_guard(&exe, &staged)?;
        match apply_update(&exe, &staged) {
            Ok(()) => std::mem::forget(guard),
            Err(e) => {
                let _ = guard.kill();
                return Err(e);
            }
        }
        Ok(format!(
            "Zainstalowano v{}. Aplikacja uruchomi się ponownie.",
            manifest.version
        ))
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        // the intermediary `cmd` starts the new build ~2 s after the
        // swap; this process must be gone by then for the
        // single-instance guard to let it through
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            app.exit(0);
        });
    }
    result
}

fn device_infos(devices: &pulpit_v2::DeviceStore) -> Vec<DeviceInfo> {
    devices
        .list()
        .into_iter()
        .map(|d| DeviceInfo {
            id: d.id,
            name: d.name,
            created: d.created,
            last_seen: d.last_seen,
        })
        .collect()
}

/// Revokes the device and closes its live sessions (audit B2 step 6):
/// a revoked tablet is cut off mid-flight, not at its next reconnect.
/// `false` when there is no such entry (unknown id, already revoked).
fn revoke_and_teardown(devices: &pulpit_v2::DeviceStore, hub: &pulpit_v2::V2Hub, id: &str) -> bool {
    let removed = devices.revoke(id);
    if removed {
        let closed = hub.close_device_sessions(id);
        tracing::info!(device = id, closed, "device revoked: live sessions closed");
    }
    removed
}

/// The v2 stack for the pairing/device commands; `None` means it failed
/// to start (bad devices.json or asset store) and the UI hides the
/// feature - the same message every such command reports.
fn v2_or_err(state: &DesktopState) -> Result<&Arc<pulpit_v2::V2State>, String> {
    state.v2.as_ref().ok_or_else(|| {
        "protocol v2 unavailable (devices.json or asset store failed to load)".to_string()
    })
}

#[tauri::command]
async fn list_devices(state: State<'_, DesktopState>) -> Result<Vec<DeviceInfo>, String> {
    let v2 = v2_or_err(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move || device_infos(&v2.devices))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn revoke_device(id: String, state: State<'_, DesktopState>) -> Result<bool, String> {
    let v2 = v2_or_err(&state)?.clone();
    // revoke persists devices.json (atomic write): off the worker
    tauri::async_runtime::spawn_blocking(move || revoke_and_teardown(&v2.devices, &v2.hub, &id))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_encodes_the_bare_address_like_the_original() {
        let url = qr_data_url("192.168.0.97").expect("qr");
        assert!(url.starts_with("data:image/svg+xml;base64,"));
        // decode round trip: the payload must be an intact SVG document
        use base64::Engine;
        let svg = base64::engine::general_purpose::STANDARD
            .decode(&url["data:image/svg+xml;base64,".len()..])
            .expect("base64");
        let svg = String::from_utf8(svg).expect("utf8");
        assert!(svg.contains("<svg") && svg.contains("viewBox"));
    }

    #[test]
    fn qr_rejects_unusable_payload() {
        // an oversized payload cannot fit a QR code
        let huge = "x".repeat(4000);
        assert!(qr_data_url(&huge).is_none());
    }

    #[test]
    fn pairing_url_matches_the_protocol_doc() {
        let url = pairing_shape("192.168.0.97", 8500, "ABCD2345");
        assert_eq!(url, "pulpit://192.168.0.97:8500?pair=ABCD2345");
    }

    #[test]
    fn pairing_mint_log_and_ttl_stay_code_free() {
        // CORE-03/DESK-05: the mint log line is the shared static string
        // (no site interpolates the code into it), and the offer's TTL
        // is the same constant the v2 routes derive it from.
        let message = pulpit_v2::pair_minted_message();
        assert!(message.contains("code suppressed in logs"));
        assert!(!message.contains("{code}"), "got: {message}");
        assert_eq!(pulpit_v2::PAIR_CODE_TTL.as_secs(), 300);
    }

    #[test]
    fn hotkey_plan_rejects_an_unparseable_new_combo() {
        assert!(hotkey_plan("Ctrl+Alt+D", "Not+A+Combo").is_err());
    }

    #[test]
    fn hotkey_plan_treats_re_saving_the_same_combo_as_a_noop() {
        // re-registering the shortcut that is already registered would
        // fail with "already registered" - the plan must say Noop
        assert!(matches!(
            hotkey_plan("Ctrl+Alt+D", "Ctrl+Alt+D"),
            Ok(HotkeyPlan::Noop)
        ));
    }

    #[test]
    fn hotkey_plan_swaps_to_a_different_combo() {
        assert!(matches!(
            hotkey_plan("Ctrl+Alt+D", "Ctrl+Alt+P"),
            Ok(HotkeyPlan::Register)
        ));
        // a stored combo that no longer parses must not block a change
        assert!(matches!(
            hotkey_plan("hand-edited junk", "Ctrl+Alt+P"),
            Ok(HotkeyPlan::Register)
        ));
        // startup's both-combos-taken state records "" (nothing
        // registered); saving any combo - even the same string - must
        // register rather than no-op
        assert!(matches!(
            hotkey_plan("", "Ctrl+Alt+D"),
            Ok(HotkeyPlan::Register)
        ));
    }

    #[test]
    fn effective_port_prefers_env_then_stored_then_default() {
        let stored = serde_json::json!(8555);
        assert_eq!(effective_port(Some("9000"), Some(&stored)), 9000);
        assert_eq!(effective_port(None, Some(&stored)), 8555);
        assert_eq!(effective_port(None, None), 8500);
    }

    #[test]
    fn effective_port_falls_back_on_unusable_values() {
        let big = serde_json::json!(70000);
        let low = serde_json::json!(80);
        let junk = serde_json::json!("not-a-port");
        // an unparseable env var must not poison the stored value's chance
        assert_eq!(effective_port(Some("nope"), Some(&big)), 8500);
        assert_eq!(effective_port(None, Some(&big)), 8500);
        assert_eq!(effective_port(None, Some(&low)), 8500);
        assert_eq!(effective_port(None, Some(&junk)), 8500);
        // a hand-edited file may keep the port as a string
        assert_eq!(effective_port(None, Some(&serde_json::json!("8555"))), 8555);
    }

    #[test]
    fn version_compare_for_updates() {
        assert!(version_newer("0.2.0", "0.1.2"));
        assert!(version_newer("v0.2.0", "0.1.9"));
        assert!(version_newer("0.10.0", "0.9.9"));
        assert!(!version_newer("0.1.2", "0.1.2"));
        assert!(!version_newer("0.1.1", "0.1.2"));
        assert!(!version_newer("garbage", "0.1.2"));
    }

    #[test]
    fn persist_editor_setting_preserves_other_keys() {
        let path =
            std::env::temp_dir().join(format!("pulpit-editor-rmw-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        persist_editor_setting(&path, "port", serde_json::json!(8555)).expect("write port");
        persist_editor_setting(&path, "hotkey", serde_json::json!("Ctrl+Alt+X"))
            .expect("write hotkey");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(saved["port"], 8555);
        assert_eq!(saved["hotkey"], "Ctrl+Alt+X");
        // over a non-object file (first run / torn write) it starts fresh
        std::fs::write(&path, b"null").expect("seed null");
        persist_editor_setting(&path, "port", serde_json::json!(8600)).expect("rewrite");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(saved["port"], 8600);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn update_manifest_and_feed_resolution() {
        // default feed when editor.json has nothing; "off" wins when set
        assert_eq!(
            resolve_update_url(None).as_deref(),
            Some(DEFAULT_UPDATE_URL)
        );
        assert_eq!(
            resolve_update_url(Some("  ")).as_deref(),
            Some(DEFAULT_UPDATE_URL)
        );
        assert_eq!(resolve_update_url(Some("off")), None);
        assert_eq!(
            resolve_update_url(Some(" https://example.com/feed.json ")).as_deref(),
            Some("https://example.com/feed.json")
        );
        // manifest: version+url required, url must be https, sha256 is
        // REQUIRED and must be a 64-hex digest (DESK-02) - a feed
        // without a digest must never yield an installable manifest
        let digest = "a".repeat(64);
        let m = parse_update_manifest(&format!(
            r#"{{"version":"1.3.0","url":"https://x/y.exe","sha256":"{digest}"}}"#
        ))
        .expect("parses");
        assert_eq!(m.version, "1.3.0");
        assert_eq!(m.url, "https://x/y.exe");
        assert_eq!(m.sha256, digest);
        assert!(parse_update_manifest(r#"{"version":"1.3.0"}"#).is_err());
        assert!(parse_update_manifest(r#"{"version":"1.3.0","url":"http://x/y.exe"}"#).is_err());
        assert!(parse_update_manifest(r#"{"version":"1.3.0","url":"https://x/y.exe"}"#).is_err());
        assert!(parse_update_manifest(
            r#"{"version":"1.3.0","url":"https://x/y.exe","sha256":"abc"}"#
        )
        .is_err());
        assert!(parse_update_manifest(&format!(
            r#"{{"version":"1.3.0","url":"https://x/y.exe","sha256":"{}"}}"#,
            "z".repeat(64)
        ))
        .is_err());
        // version compare unchanged
        assert!(version_newer("1.3.0", "1.2.9"));
        assert!(!version_newer("1.3.0", "1.3.0"));
    }

    #[test]
    fn relaunch_script_quotes_paths_and_heals_a_lost_exe() {
        // DESK-02: the guard script waits, then either starts the exe,
        // or finishes a swap that crashed between its two renames
        // (staged first, .old as the last resort)
        let exe = std::path::Path::new(r"C:\Program Files\Pulpit\pulpit-desktop.exe");
        let staged =
            std::path::Path::new(r"C:\Users\T\AppData\pulpitApp\updates\pulpit-desktop.new");
        let old = std::path::Path::new(r"C:\Program Files\Pulpit\pulpit-desktop.exe.old");
        let script = relaunch_script(exe, staged, old);
        assert!(
            script.starts_with("/C timeout /t 2 /nobreak >nul & "),
            "got: {script}"
        );
        // spaces and every path mention ride inside quotes
        assert!(
            script.contains(
                r#"if exist "C:\Program Files\Pulpit\pulpit-desktop.exe" (start "" "C:\Program Files\Pulpit\pulpit-desktop.exe")"#
            ),
            "got: {script}"
        );
        assert!(
            script.contains(&format!(
                r#"else if exist "{}" (move /y "{}" "{}" >nul & start "" "{}")"#,
                staged.display(),
                staged.display(),
                exe.display(),
                exe.display()
            )),
            "got: {script}"
        );
        assert!(
            script.contains(&format!(
                r#"else if exist "{}" (move /y "{}" "{}" >nul"#,
                old.display(),
                old.display(),
                exe.display()
            )),
            "got: {script}"
        );
    }

    #[test]
    fn update_download_verifies_and_swaps() {
        let dir = std::env::temp_dir().join(format!("pulpit-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let fake_exe = dir.join("pulpit-desktop.exe");
        let staged = dir.join("updates").join("pulpit-desktop.new");
        std::fs::write(&fake_exe, b"old-build-bytes").expect("seed exe");
        std::fs::create_dir_all(staged.parent().unwrap()).expect("staging");
        std::fs::write(&staged, b"new-build-bytes").expect("seed download");

        // digest check matches the real sha256 of the payload
        let digest = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"new-build-bytes"))
        };
        assert!(sha256_matches(&staged, &digest).expect("hash"));
        assert!(!sha256_matches(&staged, "deadbeef").expect("hash"));

        // swap: running exe moves to .old, download takes its place
        apply_update(&fake_exe, &staged).expect("swap");
        assert_eq!(
            std::fs::read(dir.join("pulpit-desktop.exe")).expect("new in place"),
            b"new-build-bytes"
        );
        assert_eq!(
            std::fs::read(dir.join("pulpit-desktop.exe.old")).expect("old aside"),
            b"old-build-bytes"
        );
        // a failed second swap rolls back: no staged file -> rename fails,
        // and the good exe must survive
        let _ = std::fs::remove_file(&staged);
        assert!(apply_update(&fake_exe, &staged).is_err());
        assert_eq!(
            std::fs::read(dir.join("pulpit-desktop.exe")).expect("exe intact"),
            b"new-build-bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aidev_status_selection_defaults_and_parses() {
        // absent section: everything detected, summary line on, name rows
        let sel = aidev_status_selection(&serde_json::Value::Null);
        assert!(sel.show.is_empty());
        assert!(sel.summary);
        assert_eq!(sel.row_style, "name");
        let raw: serde_json::Value = serde_json::from_str(
            r#"{"poll_secs":30,"status":{"show":["glm:5h","codex:week"],"summary":false,"row_style":"logo"}}"#,
        )
        .expect("seed json");
        let sel = aidev_status_selection(&raw);
        assert_eq!(sel.show, ["glm:5h", "codex:week"]);
        assert!(!sel.summary);
        assert_eq!(sel.row_style, "logo");
    }

    #[test]
    fn persist_aidev_status_keeps_provider_keys() {
        let path =
            std::env::temp_dir().join(format!("pulpit-aidev-rmw-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        persist_editor_setting(&path, "providers", serde_json::json!({"codex": {}}))
            .expect("seed providers");
        persist_editor_setting(
            &path,
            "status",
            serde_json::json!({"show": ["glm:5h"], "summary": false}),
        )
        .expect("write status");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        // the producer's API keys survive the selection write
        assert_eq!(saved["providers"]["codex"], serde_json::json!({}));
        assert_eq!(saved["status"]["show"], serde_json::json!(["glm:5h"]));
        assert_eq!(saved["status"]["summary"], false);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_image_data_rejects_files_over_the_cap() {
        let path =
            std::env::temp_dir().join(format!("pulpit-image-cap-{}.png", std::process::id()));
        let file = std::fs::File::create(&path).expect("create temp file");
        // set_len extends without writing, so the test stays cheap
        file.set_len(IMAGE_READ_CAP_BYTES + 1).expect("extend");
        drop(file);
        let err = read_image_data_blocking(&path.to_string_lossy()).expect_err("must refuse");
        assert!(err.contains("10 MiB"), "unexpected error: {err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_image_data_encodes_a_small_image_as_a_data_url() {
        let path =
            std::env::temp_dir().join(format!("pulpit-image-small-{}.png", std::process::id()));
        std::fs::write(&path, b"not-really-png-bytes").expect("write temp file");
        let url = read_image_data_blocking(&path.to_string_lossy()).expect("must read");
        assert!(url.starts_with("data:image/png;base64,"), "got: {url}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_rejects_files_over_the_cap_before_reading() {
        // CORE-09: an oversized pick refuses at the stat - the file is
        // never read into memory (set_len extends without writing, so
        // the test stays cheap)
        let dir = scratch_dir("import-cap");
        let db = pulpit_db::Db::open_or_create(&dir.join("t.db")).expect("db");
        let backend = Arc::new(SqlBackend::new(db));
        let big = dir.join("big.boardjson");
        let file = std::fs::File::create(&big).expect("create");
        file.set_len(IMPORT_READ_CAP_BYTES + 1).expect("extend");
        drop(file);
        let err = import_boards_blocking(backend, &big.to_string_lossy()).expect_err("must refuse");
        assert!(err.contains("limit to 64 MiB"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An isolated scratch directory for a test's devices.json.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pulpit-desktop-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn device_listing_omits_token_material() {
        let dir = scratch_dir("list-devices");
        let devices = pulpit_v2::DeviceStore::load(dir.join("devices.json")).expect("store");
        let (device, plaintext) = devices.create("Tablet salon");

        let infos = device_infos(&devices);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, device.id);
        assert_eq!(infos[0].name, "Tablet salon");
        assert!(infos[0].created > 0 && infos[0].last_seen > 0);

        // no token material - neither plaintext nor digest - leaves the
        // backend for the UI
        let json = serde_json::to_string(&infos).expect("json");
        assert!(!json.contains("token"));
        assert!(!json.contains(&plaintext));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revoke_teardown_removes_entry_and_closes_its_session() {
        let dir = scratch_dir("revoke-teardown");
        let devices = pulpit_v2::DeviceStore::load(dir.join("devices.json")).expect("store");
        let (device, _token) = devices.create("Tablet salon");
        let hub = pulpit_v2::V2Hub::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let session = hub.create(tx);
        hub.attach(&session);
        session.set_device(device.clone());

        assert!(revoke_and_teardown(&devices, &hub, &device.id));
        assert!(devices.list().is_empty(), "the entry is gone");
        assert!(
            *session.cancelled().borrow(),
            "the live session is torn down"
        );
        // revoking an unknown id (or the same one twice) is an honest no-op
        assert!(!revoke_and_teardown(&devices, &hub, &device.id));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Take (and clear) a touch-mode toggle that was parked while the
/// WebView was torn down (DESK-07). Called by the fresh app after its
/// listeners and boards are up.
#[tauri::command]
fn take_pending_touch_toggle() -> bool {
    PENDING_TOUCH_TOGGLE.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// The QR payload for pairing, kept separate so the command body stays
/// thin and the exact `pulpit://` shape is pinned by a test.
fn pairing_shape(ip: &str, port: u16, code: &str) -> String {
    format!("pulpit://{ip}:{port}?pair={code}")
}

#[tauri::command]
async fn list_boards(state: State<'_, DesktopState>) -> Result<Vec<BoardWithButtons>, String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || {
        // one grouped query for every board's shortcuts (was: one SELECT
        // per board); boards with no shortcuts get an empty vec
        let buttons = backend.all_buttons_by_board();
        Ok(backend
            .get_boards()
            .into_iter()
            .map(|board| BoardWithButtons {
                buttons: buttons.get(&board.id).cloned().unwrap_or_default(),
                board,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn create_board(
    state: State<'_, DesktopState>,
    name: String,
    background: String,
    width: i64,
    height: i64,
) -> Result<i64, String> {
    let backend = state.backend()?;
    // SQLite writes (and the multi-MB image payloads update_button can
    // carry) run on the blocking pool, not the tauri async workers
    let id = tauri::async_runtime::spawn_blocking(move || {
        backend.create_board(&name, &background, width, height)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    state.publish_board_set(id);
    Ok(id)
}

#[tauri::command]
async fn update_board(state: State<'_, DesktopState>, board: BoardRow) -> Result<(), String> {
    let backend = state.backend()?;
    let board_id = board.id;
    tauri::async_runtime::spawn_blocking(move || backend.update_board(&board))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    state.publish_board_set(board_id);
    Ok(())
}

#[tauri::command]
async fn delete_board(state: State<'_, DesktopState>, board_id: i64) -> Result<(), String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || backend.delete_board(board_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    state.publish_v2(vec![pulpit_proto::BoardOp::BoardRemove { board: board_id }]);
    Ok(())
}

#[tauri::command]
async fn create_button(
    state: State<'_, DesktopState>,
    board_id: i64,
    kind: String,
    mode: String,
    x: i64,
    y: i64,
) -> Result<i64, String> {
    let backend = state.backend()?;
    let id = tauri::async_runtime::spawn_blocking(move || {
        backend.create_button(board_id, &kind, &mode, x, y)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    state.publish_tile_set(board_id, id);
    Ok(id)
}

#[tauri::command]
async fn update_button(state: State<'_, DesktopState>, button: ButtonRow) -> Result<(), String> {
    let backend = state.backend()?;
    let board_id = button.board_id;
    let tile_id = button.id;
    tauri::async_runtime::spawn_blocking(move || backend.update_button(&button))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    state.publish_tile_set(board_id, tile_id);
    Ok(())
}

#[tauri::command]
async fn move_button(
    state: State<'_, DesktopState>,
    id: i64,
    board_id: i64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
) -> Result<(), String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || backend.move_button(id, x, y, w, h))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    state.publish_tile_set(board_id, id);
    Ok(())
}

#[tauri::command]
async fn delete_button(
    state: State<'_, DesktopState>,
    id: i64,
    board_id: i64,
) -> Result<(), String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || backend.delete_button(id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    state.publish_v2(vec![pulpit_proto::BoardOp::TileRemove {
        board: board_id,
        tile: id,
    }]);
    Ok(())
}

#[tauri::command]
async fn clear_board(state: State<'_, DesktopState>, board_id: i64) -> Result<(), String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || backend.clear_board(board_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    state.publish_v2(vec![pulpit_proto::BoardOp::TileClear { board: board_id }]);
    Ok(())
}

/// Touch mode: run the tile locally like the original editor does. A
/// multiaction `board` step switches the editor's own view via a DOM event.
#[tauri::command]
async fn exec_button(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: i64,
) -> Result<(), String> {
    use pulpit_actions::EventSink;

    let backend = state.backend()?;
    let backend_for_error = backend.clone();
    // Value pushes from a desktop-originated exec fan out to every
    // client lane exactly like a tablet-originated exec (CORE-04): the
    // v2 engine, the legacy APP_CUSTOM_VALUE / THIRD_PARTY_APP
    // broadcasts and the WebView.
    let feed = DesktopFeed {
        app: app.clone(),
        engine: state.v2.as_ref().map(|v2| v2.engine.clone()),
        hub: state
            .hub
            .clone()
            .ok_or_else(|| "database unavailable".to_string())?,
    };
    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<(pulpit_host::StatusApp, String, String)>();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        struct UiSink(
            AppHandle,
            tokio::sync::mpsc::UnboundedSender<(pulpit_host::StatusApp, String, String)>,
        );
        impl EventSink for UiSink {
            fn change_board(&mut self, board_id: i64) {
                let _ = self.0.emit("change-board", board_id);
            }
            fn app_value(&mut self, key: &str, value: &str) {
                let _ = self.1.send((
                    pulpit_host::StatusApp::CustomValue,
                    key.to_string(),
                    value.to_string(),
                ));
            }
            fn third_party_value(&mut self, key: &str, value: &str) {
                let _ = self.1.send((
                    pulpit_host::StatusApp::ThirdParty,
                    key.to_string(),
                    value.to_string(),
                ));
            }
        }
        // per-tap lookup, inside the blocking closure and image-less
        // (CORE-02): exec never reads img/img2, and a tap must not
        // materialize multi-MB base64 columns on an async worker
        let Some(button) = backend.get_button_meta(id) else {
            return;
        };
        let mut sink = UiSink(app, tx);
        // full tap sequence (press-start + release): a lone release-phase
        // exec never presses `key` tiles (A1)
        backend.exec_tap(button, &mut sink);
    })
    .await;
    while let Ok((app_kind, key, value)) = rx.try_recv() {
        pulpit_host::push_values(&feed, app_kind, &serde_json::json!({ key: value })).await;
    }
    // Spotify failures (design §3: Premium required / no active device /
    // needs login) ride the command's own error -> the editor's existing
    // flash path shows them; other kinds never record anything.
    if let Some(message) = backend_for_error.take_last_spotify_error() {
        return Err(message);
    }
    Ok(())
}

/// Touch mode slider: forward the 0..1 value to the tile's backend.
#[tauri::command]
async fn exec_slider(state: State<'_, DesktopState>, id: i64, value: f64) -> Result<(), String> {
    let backend = state.backend()?;
    let backend_for_error = backend.clone();
    // same per-event meta read as exec_button (CORE-02); the lookup
    // rides the blocking closure because slides fire per pointer event
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let Some(button) = backend.get_button_meta(id) else {
            return;
        };
        backend.slider(button, value);
    })
    .await;
    // same failure surface as exec_button (spotify slider kinds)
    if let Some(message) = backend_for_error.take_last_spotify_error() {
        return Err(message);
    }
    Ok(())
}

/// Active audio output endpoints for the Set Audio Device dialog.
#[tauri::command]
async fn list_audio_devices(
    state: State<'_, DesktopState>,
) -> Result<Vec<serde_json::Value>, String> {
    let backend = state.backend()?;
    let devices = tauri::async_runtime::spawn_blocking(move || backend.speaker_devices())
        .await
        .map_err(|e| e.to_string())?;
    Ok(devices
        .into_iter()
        .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
        .collect())
}

/// Editor-local settings: the touch-mode hotkey plus the server port
/// (`port_locked` when the `PULPIT_PORT` environment variable wins over
/// the stored value).
#[tauri::command]
async fn get_settings(state: State<'_, DesktopState>) -> Result<serde_json::Value, String> {
    let hotkey = state.hotkey.lock().unwrap().clone();
    Ok(serde_json::json!({
        "hotkey": hotkey,
        "port": state.port,
        "port_locked": std::env::var_os("PULPIT_PORT").is_some(),
    }))
}

/// Validate, register and persist a new touch-mode hotkey combo. The new
/// combo is registered BEFORE the old one is unregistered (012 A4): a
/// registration failure returns `Err` and leaves the previous hotkey
/// working, persisted and shown, instead of leaving no hotkey at all.
#[tauri::command]
async fn set_touch_mode_hotkey(
    app: AppHandle,
    state: State<'_, DesktopState>,
    combo: String,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let old = state.hotkey.lock().unwrap().clone();
    match hotkey_plan(&old, &combo)? {
        HotkeyPlan::Noop => return Ok(()),
        HotkeyPlan::Register => {}
    }

    register_touch_mode_hotkey(&app, &combo)?;
    if let Ok(old_shortcut) = old.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        let _ = app.global_shortcut().unregister(old_shortcut);
    }
    *state.hotkey.lock().unwrap() = combo.clone();

    if let Some(path) = &state.settings_path {
        if let Err(e) = persist_editor_setting(path, "hotkey", serde_json::json!(combo)) {
            tracing::warn!(error = %e, "could not persist hotkey");
        }
    }
    Ok(())
}

/// Read-modify-write a single key of editor.json, preserving every other
/// key (the file carries both the touch-mode hotkey and the server
/// port). Atomic like every other JSON config rewrite (012 A5): a crash
/// mid-write must not tear the file.
fn persist_editor_setting(
    path: &std::path::Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let mut editor: serde_json::Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    if !editor.is_object() {
        // missing file or a torn/hand-mangled write: start a fresh object
        editor = serde_json::json!({});
    }
    editor[key] = value;
    pulpit_db::write_atomic(path, editor.to_string().as_bytes()).map_err(|e| e.to_string())
}

/// The effective server port: the `PULPIT_PORT` environment variable
/// wins (side-by-side profiling runs), then the value persisted in
/// editor.json (Ustawienia -> Serwer), then the stock-client default
/// 8500. An unparseable or out-of-range value falls through to the next
/// source instead of failing the launch.
fn effective_port(env: Option<&str>, stored: Option<&serde_json::Value>) -> u16 {
    if let Some(p) = env.and_then(|p| p.trim().parse::<u16>().ok()) {
        if (1024..=65535).contains(&p) {
            return p;
        }
    }
    let stored = stored
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        })
        .filter(|p| (1024..=65535).contains(p));
    stored.map_or(8500, |p| p as u16)
}

/// Persist a new server port (Ustawienia -> Serwer). The sockets bind
/// once at startup and tablets aim at `host:port` explicitly, so a live
/// rebind would strand them: the value applies on the next launch.
#[tauri::command]
async fn set_server_port(state: State<'_, DesktopState>, port: u16) -> Result<(), String> {
    if !(1024..=65535).contains(&port) {
        return Err(format!(
            "Port {port} jest poza dozwolonym zakresem 1024-65535."
        ));
    }
    if std::env::var_os("PULPIT_PORT").is_some() {
        return Err(
            "Port nadpisuje zmienna środowiskowa PULPIT_PORT — ma ona pierwszeństwo.".to_string(),
        );
    }
    let path = state
        .settings_path
        .as_ref()
        .ok_or_else(|| "Brak ścieżki ustawień (uruchomienie awaryjne).".to_string())?;
    persist_editor_setting(path, "port", serde_json::json!(port))
}

/// The `status` section of aidev.json, with the defaults an absent
/// section implies: no selection (everything detected is shown), the
/// summary line on, rows identified by name.
struct AidevSelection {
    show: Vec<String>,
    summary: bool,
    row_style: String,
}

fn aidev_status_selection(raw: &serde_json::Value) -> AidevSelection {
    let show = raw
        .pointer("/status/show")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let summary = raw
        .pointer("/status/summary")
        .and_then(|b| b.as_bool())
        .unwrap_or(true);
    let row_style = match raw.pointer("/status/row_style").and_then(|s| s.as_str()) {
        Some("logo") => "logo".to_string(),
        _ => "name".to_string(),
    };
    AidevSelection {
        show,
        summary,
        row_style,
    }
}

/// Detected plan-limits rows plus the current AI-usage selection
/// (Ustawienia -> AI usage). `detected` fills up as the producer cycles,
/// so the checkbox list auto-follows whatever the machine reports.
#[derive(serde::Serialize)]
struct AidevStatusConfig {
    detected: Vec<pulpit_aidev::DetectedRow>,
    show: Vec<String>,
    summary: bool,
    row_style: String,
}

#[tauri::command]
async fn aidev_status_config(state: State<'_, DesktopState>) -> Result<AidevStatusConfig, String> {
    let path = state
        .aidev_config
        .clone()
        .ok_or_else(|| "Brak ścieżki konfiguracji aidev.".to_string())?;
    // the config read (and any provider detection) is file IO: run it
    // off the main thread
    tauri::async_runtime::spawn_blocking(move || {
        let raw: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);
        let selection = aidev_status_selection(&raw);
        Ok::<AidevStatusConfig, String>(AidevStatusConfig {
            detected: pulpit_aidev::detected_rows(),
            show: selection.show,
            summary: selection.summary,
            row_style: selection.row_style,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Persist the AI-usage tile settings to aidev.json (read-modify-write,
/// atomic). The producer re-reads the file every cycle, so a change
/// lands within one poll interval without an app restart.
#[tauri::command]
async fn set_aidev_status_config(
    state: State<'_, DesktopState>,
    show: Vec<String>,
    summary: bool,
    row_style: String,
) -> Result<(), String> {
    let path = state
        .aidev_config
        .clone()
        .ok_or_else(|| "Brak ścieżki konfiguracji aidev.".to_string())?;
    persist_editor_setting(
        &path,
        "status",
        serde_json::json!({ "show": show, "summary": summary, "row_style": row_style }),
    )
}

#[tauri::command]
async fn export_boards(
    state: State<'_, DesktopState>,
    ids: Vec<i64>,
    path: String,
) -> Result<(), String> {
    let backend = state.backend()?;
    // serialize + write can each walk multi-MB image payloads: keep
    // both off the async workers
    tauri::async_runtime::spawn_blocking(move || {
        let data = backend.export_boards(&ids).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(&data).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Hard cap on a `.boardjson` import (CORE-09): the path comes from the
/// frontend, so a giant file must be rejected before it is read and
/// parsed into memory - the board/tile count bounds only fire after the
/// parse. 64 MiB is far above what the 100-board/1024-tile caps allow
/// an honest export to weigh.
const IMPORT_READ_CAP_BYTES: u64 = 64 * 1024 * 1024;

/// Read, parse and import a `.boardjson` file written by this editor or
/// the original app. Split out of the command so the size-cap behavior
/// is testable without a Tauri app; runs on the blocking pool.
fn import_boards_blocking(
    backend: Arc<SqlBackend>,
    path: &str,
) -> Result<Vec<i64>, String> {
    // stat first: an oversized file rejects before a single byte is read
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > IMPORT_READ_CAP_BYTES {
        return Err(format!(
            "Plik importu ma {} MiB - limit to {} MiB.",
            meta.len() / (1024 * 1024),
            IMPORT_READ_CAP_BYTES / (1024 * 1024)
        ));
    }
    // take() bounds the read itself: a file that grows between the stat
    // and the read still cannot pull more than cap+1 bytes in
    use std::io::Read as _;
    let mut content = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(IMPORT_READ_CAP_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|e| e.to_string())?;
    if content.len() as u64 > IMPORT_READ_CAP_BYTES {
        return Err("Plik importu przekracza limit 64 MiB.".to_string());
    }
    let boards: Vec<serde_json::Value> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;
    backend.import_boards(&boards).map_err(|e| e.to_string())
}

/// Read and import a `.boardjson` file written by this editor or the
/// original app.
#[tauri::command]
async fn import_boards(state: State<'_, DesktopState>, path: String) -> Result<Vec<i64>, String> {
    let backend = state.backend()?;
    let ids = tauri::async_runtime::spawn_blocking(move || import_boards_blocking(backend, &path))
        .await
        .map_err(|e| e.to_string())??;
    state.broadcaster()?.sync_boards().await;
    for id in ids.iter().copied() {
        state.publish_board_set(id);
    }
    Ok(ids)
}

// ---- Spotify (Ustawienia; design round 4 §2 + §5) ---------------------------

/// How long the editor's playlist picker list may be reused.
const SPOTIFY_PLAYLISTS_CACHE: std::time::Duration = std::time::Duration::from_secs(60);

/// Spotify state for the settings panel: what is configured, who is
/// logged in and the redirect URI the user's own Spotify app must
/// register (fixed port, design §2).
#[tauri::command]
fn spotify_status(state: State<'_, DesktopState>) -> serde_json::Value {
    let Some(spotify) = &state.spotify else {
        return serde_json::json!({
            "configured": false,
            "redirectUri": pulpit_spotify::REDIRECT_URI,
        });
    };
    let config = spotify.config();
    serde_json::json!({
        "configured": true,
        "clientId": config.client_id,
        "loggedIn": config.has_login() && !spotify.needs_login(),
        "user": config.user,
        "product": config.product,
        "redirectUri": pulpit_spotify::REDIRECT_URI,
    })
}

/// Store the user's own Spotify app client id (BYO, design §2). The live
/// handle re-reads it, so a login right after pasting uses the new id.
#[tauri::command]
async fn spotify_set_client_id(
    state: State<'_, DesktopState>,
    client_id: String,
) -> Result<(), String> {
    let path = state
        .spotify_path
        .clone()
        .ok_or_else(|| "Brak ścieżki spotify.json.".to_string())?;
    let client_id = client_id.trim().to_string();
    // file IO off the main thread
    tauri::async_runtime::spawn_blocking(move || {
        let mut config = pulpit_spotify::SpotifyConfig::load(&path).map_err(|e| e.to_string())?;
        config.client_id = client_id;
        config.save(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(spotify) = &state.spotify {
        spotify.reload().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Full PKCE login (design §2). Blocking (browser + loopback callback
/// wait, up to 5 min) on the blocking pool; resolves when the callback
/// lands or the crate gives up. A busy 127.0.0.1:8502 fails here with
/// the crate's clear error - the redirect URI is registered exactly, so
/// there is no port fallback.
#[tauri::command]
async fn spotify_login(state: State<'_, DesktopState>) -> Result<serde_json::Value, String> {
    let spotify = state
        .spotify
        .clone()
        .ok_or_else(|| "Spotify wyłączony - spotify.json jest uszkodzony.".to_string())?;
    let path = state
        .spotify_path
        .clone()
        .ok_or_else(|| "Brak ścieżki spotify.json.".to_string())?;
    let client_id = spotify.config().client_id;
    if client_id.trim().is_empty() {
        return Err("Wklej najpierw Client ID swojej aplikacji Spotify.".into());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        pulpit_spotify::login(&client_id, |url| {
            if let Err(e) = open::that(url) {
                tracing::warn!(error = %e, "could not open the browser for the Spotify login");
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    let config = result.map_err(|e| e.to_string())?;
    config.save(&path).map_err(|e| e.to_string())?;
    spotify.reload().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "loggedIn": true,
        "user": config.user,
        "product": config.product,
    }))
}

/// Logout: delete the tokens, keep the client id (design §2).
#[tauri::command]
async fn spotify_logout(state: State<'_, DesktopState>) -> Result<(), String> {
    let path = state
        .spotify_path
        .clone()
        .ok_or_else(|| "Brak ścieżki spotify.json.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        pulpit_spotify::logout(&path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(spotify) = &state.spotify {
        spotify.reload().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The user's playlists for the editor picker, paged by the crate and
/// cached here for 60 s (design §5). Errors are NOT cached - a transient
/// API failure retries on the next dialog open.
#[tauri::command]
async fn spotify_playlists(
    state: State<'_, DesktopState>,
) -> Result<Vec<pulpit_spotify::Playlist>, String> {
    let Some(spotify) = state.spotify.clone() else {
        return Err("Spotify wyłączony.".into());
    };
    {
        let cache = state.spotify_playlists.lock().unwrap();
        if let Some((at, list)) = cache.as_ref() {
            if at.elapsed() < SPOTIFY_PLAYLISTS_CACHE {
                return Ok(list.clone());
            }
        }
    }
    let list = tauri::async_runtime::spawn_blocking(move || spotify.playlists().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??;
    *state.spotify_playlists.lock().unwrap() = Some((std::time::Instant::now(), list.clone()));
    Ok(list)
}

/// Active Spotify Connect devices for the editor picker.
#[tauri::command]
async fn spotify_devices(state: State<'_, DesktopState>) -> Result<Vec<pulpit_spotify::Device>, String> {
    let spotify = state
        .spotify
        .clone()
        .ok_or_else(|| "Spotify wyłączony.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || spotify.devices().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// One stored asset as a data URL for the WebView (the desktop TileCell
/// resolves `spotify-now-playing` art this way; the spotify-art lane
/// memoizes the result per hash). Reads like [`read_image_data`]: file
/// IO + base64 on the blocking pool.
#[tauri::command]
async fn asset_data_url(state: State<'_, DesktopState>, hash: String) -> Result<String, String> {
    if !pulpit_v2::is_valid_hash(&hash) {
        return Err(format!("invalid asset hash {hash:?}"));
    }
    let assets = state
        .v2
        .as_ref()
        .map(|v2| v2.assets.clone())
        .ok_or_else(|| "protocol v2 unavailable".to_string())?;
    use base64::Engine as _;
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = assets.get(&hash).ok_or_else(|| "unknown asset".to_string())?;
        let mime = assets
            .content_type(&hash)
            .unwrap_or("application/octet-stream");
        Ok(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}


// ---- utility tools commands ---------------------------------------------

/// Touch mode / tool gesture: execute a named gesture on a tool tile
/// (tap, double-tap, long-press, reset).
#[tauri::command]
async fn exec_button_gesture(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: i64,
    gesture: String,
) -> Result<(), String> {
    use pulpit_actions::EventSink;

    let backend = state.backend()?;
    let backend_for_error = backend.clone();
    let feed = DesktopFeed {
        app: app.clone(),
        engine: state.v2.as_ref().map(|v2| v2.engine.clone()),
        hub: state
            .hub
            .clone()
            .ok_or_else(|| "database unavailable".to_string())?,
    };
    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<(pulpit_host::StatusApp, String, String)>();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        struct UiSink(
            AppHandle,
            tokio::sync::mpsc::UnboundedSender<(pulpit_host::StatusApp, String, String)>,
        );
        impl EventSink for UiSink {
            fn change_board(&mut self, board_id: i64) {
                let _ = self.0.emit("change-board", board_id);
            }
            fn app_value(&mut self, key: &str, value: &str) {
                let _ = self.1.send((
                    pulpit_host::StatusApp::CustomValue,
                    key.to_string(),
                    value.to_string(),
                ));
            }
            fn third_party_value(&mut self, key: &str, value: &str) {
                let _ = self.1.send((
                    pulpit_host::StatusApp::ThirdParty,
                    key.to_string(),
                    value.to_string(),
                ));
            }
        }
        let Some(button) = backend.get_button_meta(id) else {
            return;
        };
        let mut sink = UiSink(app, tx);
        backend.exec_gesture(button, &gesture, &mut sink);
    })
    .await;
    while let Ok((app_kind, key, value)) = rx.try_recv() {
        pulpit_host::push_values(&feed, app_kind, &serde_json::json!({ key: value })).await;
    }
    if let Some(message) = backend_for_error.take_last_spotify_error() {
        return Err(message);
    }
    Ok(())
}

