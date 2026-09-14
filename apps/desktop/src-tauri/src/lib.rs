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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let state = setup_core(app.handle().clone());
            app.manage(state);

            build_tray(app.handle())?;
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
            exec_slider,
            get_settings,
            set_touch_mode_hotkey,
            get_autostart,
            set_autostart,
            read_image_data,
            list_known_inputs,
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
    let port: u16 = std::env::var("DECKBOARD_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8501);

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
    let (ext_manager, mut ext_events) = ExtManager::load(&ext_dir, &settings);
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

    // extensions push custom values -> app_status_update, like the original
    {
        let hub = hub.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(deckboard_ext::ExtEvent::SetValue(data)) = ext_events.recv().await {
                let payload = serde_json::json!({"app": "APP_CUSTOM_VALUE", "data": data});
                hub.broadcast("app_status_update", Some(&payload.to_string()))
                    .await;
                let _ = app.emit("app-status-update", &payload);
            }
        });
    }

    // master audio status watcher: the original polls every 5 s and pushes
    // speaker-volume/speaker-muted; that is what flips mute tiles live
    {
        let hub = hub.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                // COM calls block; keep them off the runtime workers
                let status =
                    tauri::async_runtime::spawn_blocking(deckboard_actions::audio::master_status)
                        .await
                        .ok()
                        .flatten();
                let Some((volume, muted)) = status else {
                    continue;
                };
                let payload = serde_json::json!({
                    "app": "APP_CUSTOM_VALUE",
                    "data": {"speaker-volume": volume, "speaker-muted": muted},
                });
                hub.broadcast("app_status_update", Some(&payload.to_string()))
                    .await;
                let _ = app.emit("app-status-update", &payload);
            }
        });
    }

    let state = Arc::new(AppState {
        hub: hub.clone(),
        backend: backend.clone() as Arc<dyn Backend>,
    });
    tauri::async_runtime::spawn(async move {
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("cannot bind {addr}: {e}");
                return;
            }
        };
        tracing::info!("legacy server listening on {addr}");
        // Engine.IO: reap sessions silent longer than pingInterval+pingTimeout
        let hub_reaper = state.hub.clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                hub_reaper.reap(75).await;
            }
        });
        if let Err(e) = axum::serve(listener, deckboard_legacy::router(state)).await {
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
        hotkey: std::sync::Mutex::new(hotkey),
        settings_path: Some(settings_path),
    }
}

// ---- tray + hotkey ---------------------------------------------------------

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
