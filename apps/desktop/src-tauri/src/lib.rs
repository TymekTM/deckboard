//! Deckboard desktop editor: Tauri 2 shell that embeds the legacy socket.io
//! v2 server (so the stock Android client keeps working) and adds the board
//! editor write path on top of the shared [`SqlBackend`].

use std::sync::Arc;

use deckboard_backend::SqlBackend;
use deckboard_db::{BoardRow, ButtonRow};
use deckboard_ext::ExtManager;
use deckboard_legacy::{AppState, Backend, EditorBroadcaster, Hub};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Everything the UI commands need, built once in [`setup_core`].
struct DesktopState {
    backend: Option<Arc<SqlBackend>>,
    broadcaster: Option<EditorBroadcaster>,
    hub: Option<Arc<Hub>>,
    port: u16,
    /// Loaded extensions, for the editor's action catalog and tile styling.
    ext: Option<Arc<ExtManager>>,
    /// Protocol v2 pairing codes; `None` when the v2 stack failed to start
    /// (bad devices.json or asset store) - the UI then hides pairing.
    pairing: Option<Arc<deckboard_v2::Pairing>>,
    /// Current touch-mode hotkey combo ("Ctrl+Alt+D" style).
    hotkey: std::sync::Mutex<String>,
    /// `deckboard/editor.json` - editor-local settings (hotkey), kept
    /// separate from the original app's settings.json.
    settings_path: Option<std::path::PathBuf>,
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
    // a daily-rotated file next to the rest of the deckboard data (only
    // `RUST_LOG` needs stderr for development). A missing home directory
    // keeps the stdout fallback rather than blocking startup.
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    if let Some(home) = dirs::home_dir() {
        let log_dir = home.join("deckboard/logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let (writer, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
            log_dir,
            "deckboard-desktop.log",
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

    tauri::Builder::default()
        // Must be the first plugin: a second launch would fight this instance
        // for port 8500 and the SQLite file, so it only surfaces the window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            // The embedded server keeps the tablets connected; closing the
            // window only hides it. The tray (Show / Hide, Quit Deckboard)
            // stays in charge of the real exit.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let state = setup_core(app.handle().clone());
            app.manage(state);

            build_tray(app.handle())?;
            create_main_window(app.handle())?;
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
            get_autostart,
            set_autostart,
            read_image_data,
            list_known_inputs,
            list_lan_addresses,
            create_pairing_code,
            export_boards,
            import_boards,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Register one button-style source (extension input, Voicemeeter or
/// Discord declaration) so the legacy mapper can style its tiles.
fn register_ext_input(
    value: &str,
    icon: Option<&str>,
    color: Option<&str>,
    font_icon: &str,
    mode: Option<&str>,
    command: Option<&str>,
) {
    deckboard_legacy::props::register_extension_input(deckboard_legacy::props::ExtInput {
        value: value.to_string(),
        icon: icon.map(str::to_string),
        color: color.map(str::to_string),
        font_icon: Some(font_icon.to_string()),
        mode: mode.map(str::to_string),
        command: command.map(str::to_string),
    });
}

/// Open the database (read-write: the editor is now the single writer,
/// ADR-001), load extensions and start the embedded legacy server. A failure
/// keeps the UI alive with `backend: None` so the window can explain why.
fn setup_core(app: tauri::AppHandle) -> DesktopState {
    // Port 8500 is what the stock Android client hardcodes (and the original
    // app's default); `DECKBOARD_PORT` overrides it for side-by-side runs.
    let port: u16 = std::env::var("DECKBOARD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8500);

    // DECKBOARD_DB overrides the database location (profiling / hermetic runs)
    let db_path = std::env::var_os("DECKBOARD_DB").map(std::path::PathBuf::from);
    let db = deckboard_db::Db::open_read_write(db_path.as_deref());
    if let Err(e) = &db {
        tracing::error!("cannot open database read-write: {e}");
        return DesktopState {
            backend: None,
            broadcaster: None,
            hub: None,
            ext: None,
            port,
            pairing: None,
            hotkey: std::sync::Mutex::new("Ctrl+Alt+D".to_string()),
            settings_path: None,
        };
    }
    let db = db.unwrap();

    let home = dirs::home_dir().expect("home directory");
    let settings_path = home.join("deckboard/editor.json");
    let settings: serde_json::Value = std::fs::read_to_string(home.join("deckboard/settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let ext_dir = std::env::var_os("DECKBOARD_EXT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join("deckboard/extensions"));
    // Native system-info and callurl replace their JS packages (the JS
    // runtimes were the heaviest part of the extension fleet); the manager
    // must not load them. Mirrors the headless server.
    let native_replaced = ["deckboard-system-info", "deckboard-callurl"]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let (ext_manager, mut ext_events) = ExtManager::load(&ext_dir, &settings, &native_replaced);
    for (package, name, error) in ext_manager.summary() {
        match error {
            Some(e) => tracing::warn!(package, name, error = e, "extension disabled"),
            None => tracing::info!(package, name, "extension ready"),
        }
    }
    for input in ext_manager.inputs() {
        register_ext_input(
            &input.value,
            input.icon.as_deref(),
            input.color.as_deref(),
            input.font_icon.as_deref().unwrap_or("fas"),
            input.mode.as_deref(),
            input.command.as_deref(),
        );
    }
    for (value, icon, font_icon, color) in deckboard_vm::input_declarations() {
        register_ext_input(value, icon, Some(color), font_icon, None, None);
    }
    for (value, icon, color, mode) in deckboard_discord::input_declarations() {
        register_ext_input(value, Some(icon), Some(color), "fas", mode, None);
    }

    let backend = Arc::new(
        SqlBackend::new(db)
            .with_extensions(ext_manager.clone())
            .with_discord(
                deckboard_discord::DiscordConfig::from_settings(&settings),
                home.join("deckboard/settings.json"),
            ),
    );

    let hub = Arc::new(Hub::new());
    let broadcaster = EditorBroadcaster::new(hub.clone(), backend.clone());

    // Protocol v2 (docs/protocol-v2.md): same port, /v2/ws + /assets +
    // /v2/pair. Shares the backend with the legacy layer; a broken devices
    // list or asset store only disables v2, never the whole editor.
    let feed_v2 = Arc::new(deckboard_v2::StateEngine::new(deckboard_proto::SERIES_CAP));
    let v2 = {
        let devices = deckboard_v2::DeviceStore::load(home.join("deckboard/devices.json"));
        let assets = deckboard_v2::AssetStore::open(home.join("deckboard/assets"));
        match (devices, assets) {
            (Ok(devices), Ok(assets)) => {
                tracing::info!("protocol v2 ready on the shared port");
                Some(Arc::new(deckboard_v2::V2State {
                    hub: Arc::new(deckboard_v2::V2Hub::new()),
                    backend: backend.clone() as Arc<dyn Backend>,
                    devices: Arc::new(devices),
                    pairing: Arc::new(deckboard_v2::Pairing::new()),
                    assets: Arc::new(assets),
                    engine: feed_v2.clone(),
                    generation: deckboard_v2::Generation::starting_at(1),
                    boards_cache: Default::default(),
                    config: deckboard_v2::V2Config {
                        public_port: port,
                        ..Default::default()
                    },
                }))
            }
            (Err(e), _) | (_, Err(e)) => {
                tracing::error!("protocol v2 disabled: {e}");
                None
            }
        }
    };
    // v2 background task: coalesced state patches.
    if let Some(v2) = &v2 {
        tauri::async_runtime::spawn(deckboard_v2::run_flusher(
            v2.engine.clone(),
            v2.hub.clone(),
            v2.config.patch_interval,
        ));
    }
    // Extension pushes feed both protocols: the legacy app_status_update
    // broadcast (stock client) and one v2 channel per data key.
    fn feed_ext(engine: &deckboard_v2::StateEngine, data: &serde_json::Value) {
        if let Some(map) = data.as_object() {
            for (key, value) in map {
                engine.set(&format!("ext.{key}"), value.clone());
            }
        }
    }

    // extensions push custom values -> app_status_update, like the original
    {
        let hub = hub.clone();
        let app = app.clone();
        let feed_v2 = feed_v2.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(deckboard_ext::ExtEvent::SetValue(data)) = ext_events.recv().await {
                tracing::debug!(keys = ?data.as_object().map(|o| o.keys().collect::<Vec<_>>()), "extension value push");
                feed_ext(&feed_v2, &data);
                let payload = serde_json::json!({"app": "APP_CUSTOM_VALUE", "data": data});
                hub.broadcast("app_status_update", Some(&payload.to_string()))
                    .await;
                let _ = app.emit("app-status-update", &payload);
            }
        });
    }

    // native system-info: declarations style si-* tiles like the JS package
    // did, and its push loop feeds CPU/RAM and friends to both protocols on
    // the original cadence (the JS runtime itself was dropped in M2).
    for (value, icon, font_icon, color, mode) in deckboard_sysinfo::input_declarations() {
        register_ext_input(value, Some(icon), Some(color), font_icon, Some(mode), None);
    }
    {
        let mut sysinfo_values = deckboard_sysinfo::spawn_push();
        let hub = hub.clone();
        let app = app.clone();
        let feed_v2 = feed_v2.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(data) = sysinfo_values.recv().await {
                feed_ext(&feed_v2, &data);
                let payload = serde_json::json!({"app": "APP_CUSTOM_VALUE", "data": data});
                hub.broadcast("app_status_update", Some(&payload.to_string()))
                    .await;
                let _ = app.emit("app-status-update", &payload);
            }
        });
    }

    // native AI dev-work source: declarations style the ai-* display tiles
    // (plan limits, agent progress), and its poll loop feeds both protocols
    // from local transcripts, agent sessions and configured plan APIs.
    for (value, icon, color, mode) in deckboard_aidev::input_declarations() {
        register_ext_input(value, Some(icon), Some(color), "fas", Some(mode), None);
    }
    {
        let paths = deckboard_aidev::Paths {
            config: home.join("deckboard").join("aidev.json"),
            zcode_cli: home.join(".zcode").join("cli"),
            claude_projects: home.join(".claude").join("projects"),
            codex_sessions: home.join(".codex").join("sessions"),
            opencode_db: home.join(".local").join("share").join("opencode").join("opencode.db"),
            antigravity_conversations: home.join(".gemini").join("antigravity").join("conversations"),
        };
        let mut aidev_values = deckboard_aidev::spawn_push(paths);
        let hub = hub.clone();
        let app = app.clone();
        let feed_v2 = feed_v2.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(data) = aidev_values.recv().await {
                feed_ext(&feed_v2, &data);
                let payload = serde_json::json!({"app": "APP_CUSTOM_VALUE", "data": data});
                hub.broadcast("app_status_update", Some(&payload.to_string()))
                    .await;
                let _ = app.emit("app-status-update", &payload);
            }
        });
    }

    // master audio status watcher: the original polls every 5 s and pushes
    // speaker-volume/speaker-muted; that is what flips mute tiles live.
    // The active output device rides along (THIRD_PARTY_APP, like the
    // original) but only when it changed, so tablets are not spammed.
    {
        let hub = hub.clone();
        let app = app.clone();
        let backend = backend.clone();
        let feed_v2 = feed_v2.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_device: Option<String> = None;
            loop {
                interval.tick().await;
                // Speaker COM calls block; keep them off the runtime
                // workers. The shared SqlBackend owns the lazy speaker
                // instance, so exec switches and watcher reads agree.
                let snapshot_backend = backend.clone();
                let snapshot = tauri::async_runtime::spawn_blocking(move || {
                    let (volume, muted) = snapshot_backend.speaker_status();
                    let device = snapshot_backend.speaker_device_id();
                    (volume, muted, device)
                })
                .await
                .ok();
                if let Some((Some(volume), Some(muted), _)) = snapshot {
                    // percent 0..=100 -> fraction like the original n/100
                    let level = (volume / 100.0 * 1000.0).round() / 1000.0;
                    feed_v2.set("speaker-volume", serde_json::json!(level));
                    feed_v2.set("speaker-muted", serde_json::json!(muted));
                    let payload = serde_json::json!({
                        "app": "APP_CUSTOM_VALUE",
                        "data": {"speaker-volume": level, "speaker-muted": muted},
                    });
                    hub.broadcast("app_status_update", Some(&payload.to_string()))
                        .await;
                    let _ = app.emit("app-status-update", &payload);
                }
                if let Some((_, _, Some(id))) = snapshot {
                    if last_device.as_deref() != Some(id.as_str()) {
                        last_device = Some(id.clone());
                        feed_v2.set("speaker-device", serde_json::json!(id));
                        let payload = serde_json::json!({
                            "app": "THIRD_PARTY_APP",
                            "data": {"speaker-device": id},
                        });
                        hub.broadcast("app_status_update", Some(&payload.to_string()))
                            .await;
                        let _ = app.emit("app-status-update", &payload);
                    }
                }
            }
        });
    }

    let state = Arc::new(AppState {
        hub: hub.clone(),
        backend: backend.clone() as Arc<dyn Backend>,
    });
    // one router for both protocols; ConnectInfo is needed by the loopback
    // guard on POST /v2/pair
    let app_router = match &v2 {
        Some(v2) => deckboard_legacy::router(state.clone()).merge(deckboard_v2::router(v2.clone())),
        None => deckboard_legacy::router(state.clone()),
    };
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
        if let Err(e) = axum::serve(
            listener,
            app_router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        {
            tracing::error!("legacy server stopped: {e}");
        }
    });

    let hotkey = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("hotkey").and_then(|h| h.as_str()).map(str::to_string))
        .unwrap_or_else(|| "Ctrl+Alt+D".to_string());
    register_touch_mode_hotkey(&app, &hotkey);

    DesktopState {
        backend: Some(backend),
        broadcaster: Some(broadcaster),
        hub: Some(hub),
        ext: Some(ext_manager),
        port,
        pairing: v2.map(|v| v.pairing.clone()),
        hotkey: std::sync::Mutex::new(hotkey),
        settings_path: Some(settings_path),
    }
}

// ---- tray + hotkey ---------------------------------------------------------

/// The main window is built here instead of `tauri.conf.json` because the
/// WebView2 argument list is only reachable through the builder API, and a
/// custom list replaces the default one - so the stock feature disables are
/// repeated alongside the memory-oriented flags (no background networking,
/// no component updates, capped HTTP disk cache).
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("Deckboard")
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
    let quit = MenuItem::with_id(app, "quit", "Quit Deckboard", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_hide, &touch, &launch, &quit])?;

    tauri::tray::TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("app icon").clone())
        .tooltip("Deckboard")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show-hide" => toggle_main_window(app),
            "touch-mode" => {
                let _ = app.emit("toggle-touch-mode", ());
            }
            "autostart" => {
                use tauri_plugin_autostart::ManagerExt;
                let launch = app.autolaunch();
                let enabled = launch.is_enabled().unwrap_or(false);
                let _ = if enabled {
                    launch.disable()
                } else {
                    launch.enable()
                };
            }
            "quit" => app.exit(0),
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
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// Register the touch-mode hotkey. The combo is user-configurable
/// (`deckboard/editor.json`, default Ctrl+Alt+D - the original's
/// `toggleTouchMode` concept); an unusable stored combo falls back to the
/// default with a warning.
fn register_touch_mode_hotkey(app: &AppHandle, combo: &str) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let shortcut = match combo.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!("invalid hotkey \"{combo}\" - touch-mode hotkey not registered");
            return;
        }
    };
    let result = app
        .global_shortcut()
        .on_shortcut(shortcut, |app, _s, event| {
            if event.state() == ShortcutState::Pressed {
                let _ = app.emit("toggle-touch-mode", ());
            }
        });
    if let Err(e) = result {
        tracing::warn!("could not register hotkey \"{combo}\": {e}");
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
    for (value, icon, _font_icon, color) in deckboard_vm::input_declarations() {
        out.push(serde_json::json!({
            "value": value,
            "icon": icon,
            "color": color,
            "mode": serde_json::Value::Null,
            "command": serde_json::Value::Null,
            "source": "device",
        }));
    }
    for (value, icon, color, mode) in deckboard_discord::input_declarations() {
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

/// Read an image file and return it as a data URL for tile backgrounds and
/// icons. Done in Rust so no filesystem plugin/scope is needed.
#[tauri::command]
fn read_image_data(path: String) -> Result<String, String> {
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
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
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
/// client (`deckboard://<ip>:<port>?pair=<code>`, docs/protocol-v2.md).
#[derive(Serialize)]
struct PairingOffer {
    code: String,
    expires_in_secs: u64,
    addresses: Vec<LanAddress>,
}

#[tauri::command]
fn create_pairing_code(state: State<'_, DesktopState>) -> Result<PairingOffer, String> {
    let pairing = state.pairing.as_ref().ok_or_else(|| {
        "protocol v2 unavailable (devices.json or asset store failed to load)".to_string()
    })?;
    let code = pairing.new_code();
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
    tracing::info!(%code, "pairing code minted from the editor - expires in 5 minutes");
    Ok(PairingOffer {
        code,
        expires_in_secs: 300,
        addresses,
    })
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
        assert_eq!(url, "deckboard://192.168.0.97:8500?pair=ABCD2345");
    }
}

/// The QR payload for pairing, kept separate so the command body stays
/// thin and the exact `deckboard://` shape is pinned by a test.
fn pairing_shape(ip: &str, port: u16, code: &str) -> String {
    format!("deckboard://{ip}:{port}?pair={code}")
}

#[tauri::command]
async fn list_boards(state: State<'_, DesktopState>) -> Result<Vec<BoardWithButtons>, String> {
    let backend = state.backend()?;
    tauri::async_runtime::spawn_blocking(move || {
        let boards = backend.get_boards();
        Ok(boards
            .into_iter()
            .map(|board| {
                let buttons = backend.get_buttons_by_board(board.id);
                BoardWithButtons { board, buttons }
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
    let id = backend
        .create_board(&name, &background, width, height)
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    Ok(id)
}

#[tauri::command]
async fn update_board(state: State<'_, DesktopState>, board: BoardRow) -> Result<(), String> {
    let backend = state.backend()?;
    backend.update_board(&board).map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    Ok(())
}

#[tauri::command]
async fn delete_board(state: State<'_, DesktopState>, board_id: i64) -> Result<(), String> {
    let backend = state.backend()?;
    backend.delete_board(board_id).map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
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
    let id = backend
        .create_button(board_id, &kind, &mode, x, y)
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    Ok(id)
}

#[tauri::command]
async fn update_button(state: State<'_, DesktopState>, button: ButtonRow) -> Result<(), String> {
    let backend = state.backend()?;
    let board_id = button.board_id;
    backend.update_button(&button).map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
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
    backend
        .move_button(id, x, y, w, h)
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    Ok(())
}

#[tauri::command]
async fn delete_button(
    state: State<'_, DesktopState>,
    id: i64,
    board_id: i64,
) -> Result<(), String> {
    let backend = state.backend()?;
    backend.delete_button(id).map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
    Ok(())
}

#[tauri::command]
async fn clear_board(state: State<'_, DesktopState>, board_id: i64) -> Result<(), String> {
    let backend = state.backend()?;
    backend.clear_board(board_id).map_err(|e| e.to_string())?;
    state.broadcaster()?.refresh_board(board_id).await;
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
    use deckboard_actions::EventSink;

    let backend = state.backend()?;
    let Some(button) = backend.get_button(id) else {
        return Ok(());
    };
    struct UiSink(AppHandle);
    impl EventSink for UiSink {
        fn change_board(&mut self, board_id: i64) {
            let _ = self.0.emit("change-board", board_id);
        }
        fn app_value(&mut self, _key: &str, _value: &str) {}
    }
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let mut sink = UiSink(app.clone());
        backend.exec(button, false, &mut sink);
    })
    .await;
    Ok(())
}

/// Write the selected boards to `path` in the original's `.boardjson`
/// format. The file IO lives here so the webview needs no fs permissions.
/// Touch mode slider: forward the 0..1 value to the tile's backend.
#[tauri::command]
async fn exec_slider(state: State<'_, DesktopState>, id: i64, value: f64) -> Result<(), String> {
    let backend = state.backend()?;
    let Some(button) = backend.get_button(id) else {
        return Ok(());
    };
    let _ = tauri::async_runtime::spawn_blocking(move || backend.slider(button, value)).await;
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

/// Editor-local settings (currently just the touch-mode hotkey).
#[tauri::command]
async fn get_settings(state: State<'_, DesktopState>) -> Result<serde_json::Value, String> {
    let hotkey = state.hotkey.lock().unwrap().clone();
    Ok(serde_json::json!({ "hotkey": hotkey }))
}

/// Validate, register and persist a new touch-mode hotkey combo.
#[tauri::command]
async fn set_touch_mode_hotkey(
    app: AppHandle,
    state: State<'_, DesktopState>,
    combo: String,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    // validation only: registration re-parses the combo
    let _validated: Shortcut = combo
        .parse()
        .map_err(|_| format!("invalid shortcut \"{combo}\" - use e.g. Ctrl+Alt+D"))?;

    let old = state.hotkey.lock().unwrap().clone();
    if let Ok(old_shortcut) = old.parse::<Shortcut>() {
        let _ = app.global_shortcut().unregister(old_shortcut);
    }
    register_touch_mode_hotkey(&app, &combo);
    *state.hotkey.lock().unwrap() = combo.clone();

    if let Some(path) = &state.settings_path {
        let json = serde_json::json!({ "hotkey": combo }).to_string();
        if let Err(e) = std::fs::write(path, json) {
            tracing::warn!(error = %e, "could not persist hotkey");
        }
    }
    Ok(())
}

#[tauri::command]
async fn export_boards(
    state: State<'_, DesktopState>,
    ids: Vec<i64>,
    path: String,
) -> Result<(), String> {
    let backend = state.backend()?;
    let data = tauri::async_runtime::spawn_blocking(move || backend.export_boards(&ids))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Read and import a `.boardjson` file written by this editor or the
/// original app.
#[tauri::command]
async fn import_boards(state: State<'_, DesktopState>, path: String) -> Result<Vec<i64>, String> {
    let backend = state.backend()?;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let boards: Vec<serde_json::Value> =
        serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let ids = tauri::async_runtime::spawn_blocking(move || backend.import_boards(&boards))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    state.broadcaster()?.sync_boards().await;
    Ok(ids)
}
