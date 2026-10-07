//! OBS WebSocket v5 client: one connection worker (handshake, events,
//! request/response demux, reconnect with backoff), one action runner
//! (the sync `exec` path) and the handles the hosts use.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify, RwLock};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use crate::auth::compute_auth_response;
use crate::config::ObsConfig;
use crate::protocol::*;
use crate::state::{ObsState, QueryCtx};

/// Connection status for the settings panel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObsStatusInfo {
    pub enabled: bool,
    pub connected: bool,
    pub auth_failed: bool,
    pub version: Option<String>,
    pub host: String,
    pub port: u16,
    pub has_password: bool,
}

/// Names the editor's tile dialog offers as picker choices while OBS is
/// connected (catalog `devices: "obs-*"` fields). Empty lists mean the
/// fields fall back to free text.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ObsChoices {
    pub scenes: Vec<String>,
    pub sources: Vec<String>,
    pub inputs: Vec<String>,
    pub filters: Vec<String>,
}

/// One executable OBS tile action, parsed from the stored command JSON
/// (`parse_action`). The action runner consumes these.
#[derive(Clone, Debug, PartialEq)]
pub enum ObsAction {
    SetScene(String),
    ToggleSceneItem {
        scene: Option<String>,
        source: String,
    },
    ToggleMute(String),
    ToggleFilter {
        source: Option<String>,
        filter: String,
    },
    ToggleStudioMode,
    ToggleRecord,
    ToggleStream,
    SaveReplay,
    SetVolume {
        input: String,
        mul: f64,
    },
}

impl ObsAction {
    fn label(&self) -> &'static str {
        match self {
            ObsAction::SetScene(_) => "obs-scene",
            ObsAction::ToggleSceneItem { .. } => "obs-source",
            ObsAction::ToggleMute(_) => "obs-device-audio",
            ObsAction::ToggleFilter { .. } => "obs-filter",
            ObsAction::ToggleStudioMode => "obs-studio-mode",
            ObsAction::ToggleRecord => "obs-record",
            ObsAction::ToggleStream => "obs-stream",
            ObsAction::SaveReplay => "obs-replay-save",
            ObsAction::SetVolume { .. } => "obs-audio-slider",
        }
    }
}

/// The obs kinds this integration implements (the backend's native arm
/// dispatches on this). `obs-transition` and friends stay unclaimed.
pub fn is_obs_action(kind: &str) -> bool {
    matches!(
        kind,
        "obs-scene"
            | "obs-source"
            | "obs-device-audio"
            | "obs-filter"
            | "obs-studio-mode"
            | "obs-record"
            | "obs-stream"
            | "obs-replay-save"
            | "obs-audio-slider"
    )
}

/// Stored command JSON -> action. `Ok(None)` means "not an obs kind".
/// The argument shapes are the ones the catalog fields write
/// (`scene` / `source` / `device` / `filter`); unknown or missing
/// arguments are a claimed-but-warned error, never a crash.
pub fn parse_action(
    kind: &str,
    command: &str,
    slider_value: Option<f64>,
) -> Result<Option<ObsAction>, String> {
    if !is_obs_action(kind) {
        return Ok(None);
    }
    let args: Value = serde_json::from_str(command).unwrap_or(Value::Null);
    let field = |name: &str| -> &str {
        args.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let opt_field = |name: &str| -> Option<String> {
        let v = field(name);
        (!v.is_empty()).then(|| v.to_string())
    };
    let action = match kind {
        "obs-scene" => ObsAction::SetScene(required(kind, "scene", field("scene"))?.into()),
        "obs-source" => ObsAction::ToggleSceneItem {
            scene: opt_field("scene"),
            source: required(kind, "source", field("source"))?.into(),
        },
        // stored payloads use `device` (catalog), older hand edits may
        // say `source` - both name an input
        "obs-device-audio" => {
            let input = opt_field("device").or_else(|| opt_field("source"));
            let input = required(kind, "device", input.as_deref().unwrap_or_default())?;
            ObsAction::ToggleMute(input.into())
        }
        "obs-filter" => ObsAction::ToggleFilter {
            source: opt_field("source"),
            filter: required(kind, "filter", field("filter"))?.into(),
        },
        "obs-studio-mode" => ObsAction::ToggleStudioMode,
        "obs-record" => ObsAction::ToggleRecord,
        "obs-stream" => ObsAction::ToggleStream,
        "obs-replay-save" => ObsAction::SaveReplay,
        "obs-audio-slider" => {
            let input = opt_field("device").or_else(|| opt_field("source"));
            let input = required(kind, "device", input.as_deref().unwrap_or_default())?.into();
            let mul = slider_value
                .ok_or_else(|| format!("{kind}: slider value missing"))?
                .clamp(0.0, 1.0);
            ObsAction::SetVolume { input, mul }
        }
        _ => return Ok(None),
    };
    Ok(Some(action))
}

fn required<'a>(kind: &str, field: &str, value: &'a str) -> Result<&'a str, String> {
    if value.is_empty() {
        return Err(format!("{kind}: argument `{field}` is missing or empty"));
    }
    Ok(value)
}

enum ClientCmd {
    Request {
        request_type: String,
        request_data: Option<Value>,
        reply: oneshot::Sender<Result<Value, String>>,
    },
}

/// The OBS integration handle: cheap to clone, shared between the
/// backend exec chain, the host producer pump and the settings UI.
#[derive(Clone)]
pub struct Obs {
    config: Arc<RwLock<ObsConfig>>,
    config_path: Option<PathBuf>,
    state: Arc<RwLock<ObsState>>,
    cmd_tx: mpsc::UnboundedSender<ClientCmd>,
    action_tx: mpsc::UnboundedSender<ObsAction>,
    reconnect_notify: Arc<Notify>,
}

impl Obs {
    /// Build the handle and start the connection worker plus the action
    /// runner. A disabled config parks the worker on a notify - zero
    /// traffic until `apply_config` enables it.
    pub fn new(
        config: ObsConfig,
        config_path: Option<PathBuf>,
        push_tx: mpsc::UnboundedSender<Value>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        let reconnect_notify = Arc::new(Notify::new());
        let config_arc = Arc::new(RwLock::new(config));
        let state_arc = Arc::new(RwLock::new(ObsState::default()));

        let client = Self {
            config: config_arc.clone(),
            config_path,
            state: state_arc.clone(),
            cmd_tx: cmd_tx.clone(),
            action_tx,
            reconnect_notify: reconnect_notify.clone(),
        };

        tokio::spawn(run_worker(
            config_arc,
            state_arc.clone(),
            cmd_rx,
            push_tx.clone(),
            reconnect_notify.clone(),
        ));
        tokio::spawn(run_actions(client.clone(), action_rx));

        client
    }

    /// Snapshot of the stored config (password masked out by the type's
    /// Debug; the value itself is only used to prefill the editor form).
    pub async fn config_async(&self) -> ObsConfig {
        self.config.read().await.clone()
    }

    /// Persist and apply a new config; the worker reconnects (or parks)
    /// without an app restart.
    pub async fn apply_config(&self, new_config: ObsConfig) -> Result<(), String> {
        if let Some(path) = &self.config_path {
            new_config.save(path)?;
        }
        *self.config.write().await = new_config;
        self.reconnect_notify.notify_one();
        Ok(())
    }

    pub async fn status(&self) -> ObsStatusInfo {
        let cfg = self.config.read().await;
        let st = self.state.read().await;
        ObsStatusInfo {
            enabled: cfg.enabled,
            connected: st.connected,
            auth_failed: st.auth_failed,
            version: st.obs_version.clone(),
            host: cfg.host.clone(),
            port: cfg.port,
            has_password: cfg.password.as_ref().is_some_and(|p| !p.is_empty()),
        }
    }

    pub async fn choices(&self) -> ObsChoices {
        let st = self.state.read().await;
        let mut sources = Vec::new();
        for (_, src) in st.scene_items.keys() {
            if !sources.contains(src) {
                sources.push(src.clone());
            }
        }
        let mut filters = Vec::new();
        for (_, flt) in st.source_filters.keys() {
            if !filters.contains(flt) {
                filters.push(flt.clone());
            }
        }
        let mut inputs: Vec<String> = st.input_mutes.keys().cloned().collect();
        inputs.extend(st.input_volumes.keys().cloned());
        inputs.sort();
        inputs.dedup();
        ObsChoices {
            scenes: st.scenes.clone(),
            sources,
            inputs,
            filters,
        }
    }

    /// Fire-and-forget exec for the backend's native arm: parses the
    /// stored command and queues the action. Returns true when the kind
    /// belongs to OBS (claimed even on a malformed payload - the parse
    /// error is logged once per press, not silently dropped).
    pub fn exec(&self, kind: &str, command: &str, slider_value: Option<f64>) -> bool {
        match parse_action(kind, command, slider_value) {
            Ok(Some(action)) => {
                let _ = self.action_tx.send(action);
                true
            }
            Ok(None) => false,
            Err(e) => {
                warn!(kind, error = %e, "obs tile has a malformed command payload");
                true
            }
        }
    }

    /// One request/response round trip against the connection worker.
    async fn send_request(
        &self,
        request_type: &str,
        request_data: Option<Value>,
    ) -> Result<Value, String> {
        if !self.state.read().await.connected {
            return Err("OBS nie jest połączony".to_string());
        }
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(ClientCmd::Request {
                request_type: request_type.to_string(),
                request_data,
                reply,
            })
            .map_err(|_| "OBS worker task is dead".to_string())?;

        tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .map_err(|_| "OBS request timed out".to_string())?
            .map_err(|_| "OBS request dropped".to_string())?
    }

    pub async fn set_current_program_scene(&self, scene_name: &str) -> Result<(), String> {
        self.send_request(
            "SetCurrentProgramScene",
            Some(serde_json::json!({ "sceneName": scene_name })),
        )
        .await?;
        Ok(())
    }

    /// Toggle one scene item enabled in `scene` (empty: the current
    /// program scene): GetSceneItemId -> GetSceneItemEnabled -> invert.
    pub async fn toggle_scene_item(
        &self,
        scene_name: Option<&str>,
        source_name: &str,
    ) -> Result<(), String> {
        let scene = match scene_name {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                let st = self.state.read().await;
                st.current_scene
                    .clone()
                    .ok_or_else(|| "Brak aktywnej sceny w OBS".to_string())?
            }
        };

        let id_res = self
            .send_request(
                "GetSceneItemId",
                Some(serde_json::json!({
                    "sceneName": scene,
                    "sourceName": source_name
                })),
            )
            .await?;
        let item_id = id_res
            .get("sceneItemId")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("Nie znaleziono zrodla {source_name} na scenie {scene}"))?;

        let enabled_res = self
            .send_request(
                "GetSceneItemEnabled",
                Some(serde_json::json!({
                    "sceneName": scene,
                    "sceneItemId": item_id
                })),
            )
            .await?;
        let currently_enabled = enabled_res
            .get("sceneItemEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        self.send_request(
            "SetSceneItemEnabled",
            Some(serde_json::json!({
                "sceneName": scene,
                "sceneItemId": item_id,
                "sceneItemEnabled": !currently_enabled
            })),
        )
        .await?;
        Ok(())
    }

    pub async fn toggle_input_mute(&self, input_name: &str) -> Result<(), String> {
        self.send_request(
            "ToggleInputMute",
            Some(serde_json::json!({ "inputName": input_name })),
        )
        .await?;
        Ok(())
    }

    /// Toggle one source filter; an empty `source_name` resolves the
    /// (globally unique) filter name against the cached filter list.
    pub async fn toggle_source_filter(
        &self,
        source_name: Option<&str>,
        filter_name: &str,
    ) -> Result<(), String> {
        let source = if let Some(s) = source_name.filter(|s| !s.is_empty()) {
            s.to_string()
        } else {
            let st = self.state.read().await;
            st.source_filters
                .keys()
                .find(|(_, f)| f == filter_name)
                .map(|(s, _)| s.clone())
                .ok_or_else(|| format!("Nie znaleziono filtra {filter_name}"))?
        };

        let filter_res = self
            .send_request(
                "GetSourceFilter",
                Some(serde_json::json!({
                    "sourceName": source,
                    "filterName": filter_name
                })),
            )
            .await?;
        let currently_enabled = filter_res
            .get("filterEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        self.send_request(
            "SetSourceFilterEnabled",
            Some(serde_json::json!({
                "sourceName": source,
                "filterName": filter_name,
                "filterEnabled": !currently_enabled
            })),
        )
        .await?;
        Ok(())
    }

    pub async fn toggle_studio_mode(&self) -> Result<(), String> {
        let res = self.send_request("GetStudioModeEnabled", None).await?;
        let enabled = res
            .get("studioModeEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.send_request(
            "SetStudioModeEnabled",
            Some(serde_json::json!({ "studioModeEnabled": !enabled })),
        )
        .await?;
        Ok(())
    }

    pub async fn toggle_record(&self) -> Result<(), String> {
        self.send_request("ToggleRecord", None).await?;
        Ok(())
    }

    pub async fn toggle_stream(&self) -> Result<(), String> {
        self.send_request("ToggleStream", None).await?;
        Ok(())
    }

    pub async fn save_replay_buffer(&self) -> Result<(), String> {
        self.send_request("SaveReplayBuffer", None).await?;
        Ok(())
    }

    pub async fn set_input_volume(&self, input_name: &str, volume_mul: f64) -> Result<(), String> {
        self.send_request(
            "SetInputVolume",
            Some(serde_json::json!({
                "inputName": input_name,
                "inputVolumeMul": volume_mul.clamp(0.0, 1.0)
            })),
        )
        .await?;
        Ok(())
    }

    /// One-shot connect + auth probe for the settings panel's
    /// "Testuj połączenie": independent of the worker, returns the OBS
    /// version on success. Never logs the password.
    pub async fn test_connection(config: &ObsConfig) -> Result<String, String> {
        let url = config.ws_url();
        let (ws_stream, _) = tokio::time::timeout(Duration::from_secs(3), connect_async(&url))
            .await
            .map_err(|_| "Timeout połączenia z OBS".to_string())?
            .map_err(|e| format!("Błąd połączenia: {e}"))?;

        let (mut write, mut read) = ws_stream.split();

        let hello = read_handshake_half(&mut read, "Hello").await?;
        let hello = match hello {
            Message::Hello(h) => h,
            _ => return Err("Oczekiwano Hello (op 0)".to_string()),
        };

        let auth_str = handshake_auth(&hello, config.password.as_deref())
            .map_err(|_| "OBS wymaga hasła, a hasło nie zostało podane".to_string())?;

        let identify = Message::Identify(Identify {
            rpc_version: RPC_VERSION,
            authentication: auth_str,
            event_subscriptions: Some(0),
        });
        write
            .send(WsMessage::Text(
                serde_json::to_string(&identify).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| format!("Błąd wysyłania Identify: {e}"))?;

        let identified = read_handshake_half(&mut read, "Identified").await?;
        match identified {
            Message::Identified(_) => Ok(format!("OBS v{}", hello.obs_web_socket_version)),
            _ => Err("Uwierzytelnienie odrzucone przez OBS".to_string()),
        }
    }
}

enum HandshakeAuthError {
    /// OBS asked for a password and none was configured.
    AuthRequired,
}

/// Build the Identify authentication string from the Hello challenge
/// (None when OBS asked for no auth).
fn handshake_auth(
    hello: &Hello,
    password: Option<&str>,
) -> Result<Option<String>, HandshakeAuthError> {
    match &hello.authentication {
        None => Ok(None),
        Some(auth) => {
            let pw = password.unwrap_or_default();
            if pw.is_empty() {
                return Err(HandshakeAuthError::AuthRequired);
            }
            Ok(Some(compute_auth_response(pw, &auth.salt, &auth.challenge)))
        }
    }
}

/// Read one text frame with a timeout, mapped to a protocol Message.
async fn read_handshake_half(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    what: &str,
) -> Result<Message, String> {
    let msg = tokio::time::timeout(Duration::from_secs(3), read.next())
        .await
        .map_err(|_| format!("Timeout oczekiwania na {what}"))?
        .ok_or_else(|| format!("Połączenie zamknięte przed {what}"))?
        .map_err(|e| format!("Błąd odczytu {what}: {e}"))?;
    match msg {
        WsMessage::Text(t) => {
            serde_json::from_str(&t).map_err(|e| format!("Nieprawidłowa ramka {what}: {e}"))
        }
        _ => Err(format!("Nieprawidłowy format {what}")),
    }
}

static REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

struct PendingResponse {
    reply: Option<oneshot::Sender<Result<Value, String>>>,
    ctx: QueryCtx,
}

/// Every request this connection sends (initial sync, event-driven
/// refreshes, user actions) registers here; the response arm updates
/// state from type + ctx and resolves the optional user reply.
async fn send_tracked(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    pending: &mut HashMap<String, PendingResponse>,
    request_type: &str,
    request_data: Option<Value>,
    ctx: QueryCtx,
    reply: Option<oneshot::Sender<Result<Value, String>>>,
) -> Result<(), String> {
    let request_id = format!("q{}", REQ_COUNTER.fetch_add(1, Ordering::Relaxed));
    let req = Message::Request(Request {
        request_type: request_type.to_string(),
        request_id: request_id.clone(),
        request_data,
    });
    let json = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    sink.send(WsMessage::Text(json))
        .await
        .map_err(|e| format!("Błąd wysyłania: {e}"))?;
    pending.insert(
        request_id,
        PendingResponse {
            reply,
            ctx: ctx.clone(),
        },
    );
    Ok(())
}

/// The initial sync after every (re)connect: current scene, scene list,
/// inputs (each fanning out to mute/volume/filters), studio mode and
/// the two output flags.
const INIT_QUERIES: &[&str] = &[
    "GetCurrentProgramScene",
    "GetSceneList",
    "GetInputList",
    "GetStudioModeEnabled",
    "GetRecordStatus",
    "GetStreamStatus",
];

async fn run_worker(
    config: Arc<RwLock<ObsConfig>>,
    state: Arc<RwLock<ObsState>>,
    mut cmd_rx: mpsc::UnboundedReceiver<ClientCmd>,
    push_tx: mpsc::UnboundedSender<Value>,
    reconnect_notify: Arc<Notify>,
) {
    let mut backoff = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(16);

    loop {
        let (enabled, url, password) = {
            let cfg = config.read().await;
            (cfg.enabled, cfg.ws_url(), cfg.password.clone())
        };

        if !enabled {
            // one inactive snapshot so clients drop their lit OBS tiles,
            // then park until the config changes - zero traffic
            let snapshot = state.write().await.take_reset_snapshot();
            let _ = push_tx.send(snapshot);
            reconnect_notify.notified().await;
            backoff = Duration::from_secs(1);
            continue;
        }

        debug!(url = %url, "connecting to OBS");
        match connect_async(&url).await {
            Ok((ws_stream, _)) => {
                backoff = Duration::from_secs(1);
                if let Err(e) = run_connection(
                    &state,
                    &mut cmd_rx,
                    &push_tx,
                    ws_stream,
                    password.as_deref(),
                    &reconnect_notify,
                )
                .await
                {
                    debug!(error = %e, "OBS connection ended");
                }
            }
            Err(e) => {
                debug!(error = %e, "cannot connect to OBS");
            }
        }

        {
            let mut st = state.write().await;
            st.reset_connection();
            let snapshot = st.take_reset_snapshot();
            let _ = push_tx.send(snapshot);
        }

        // config change reconnects immediately; otherwise back off
        tokio::select! {
            _ = reconnect_notify.notified() => {
                backoff = Duration::from_secs(1);
            }
            _ = tokio::time::sleep(backoff) => {
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Handshake + the event loop of one connection. Returns when the
/// socket dies, a ping fails or a config change requests a reconnect.
async fn run_connection(
    state: &Arc<RwLock<ObsState>>,
    cmd_rx: &mut mpsc::UnboundedReceiver<ClientCmd>,
    push_tx: &mpsc::UnboundedSender<Value>,
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    password: Option<&str>,
    reconnect_notify: &Arc<Notify>,
) -> Result<(), String> {
    let (mut ws_sink, mut ws_src) = ws_stream.split();

    // -- handshake --------------------------------------------------------
    let hello_msg = read_handshake_half(&mut ws_src, "Hello").await?;
    let hello = match hello_msg {
        Message::Hello(h) => h,
        _ => return Err("Expected Hello".to_string()),
    };
    let auth_str = match handshake_auth(&hello, password) {
        Ok(auth) => auth,
        Err(HandshakeAuthError::AuthRequired) => return Err("auth_required".to_string()),
    };
    let identify = Message::Identify(Identify {
        rpc_version: RPC_VERSION,
        authentication: auth_str,
        event_subscriptions: Some(ALL_SUBSCRIPTIONS),
    });
    ws_sink
        .send(WsMessage::Text(
            serde_json::to_string(&identify).map_err(|e| e.to_string())?,
        ))
        .await
        .map_err(|e| e.to_string())?;
    let identified = read_handshake_half(&mut ws_src, "Identified").await?;
    match identified {
        Message::Identified(_) => {}
        _ => return Err("auth_failed".to_string()),
    }

    info!(version = %hello.obs_web_socket_version, "OBS connected and identified");
    state.write().await.set_connected(hello.obs_web_socket_version.clone());

    // -- initial sync -----------------------------------------------------
    let mut pending: HashMap<String, PendingResponse> = HashMap::new();
    for query in INIT_QUERIES {
        send_tracked(&mut ws_sink, &mut pending, query, None, QueryCtx::None, None).await?;
    }

    // -- event loop -------------------------------------------------------
    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    ping_interval.tick().await;

    loop {
        tokio::select! {
            _ = reconnect_notify.notified() => {
                return Err("reconnect requested".to_string());
            }
            _ = ping_interval.tick() => {
                if let Err(e) = ws_sink.send(WsMessage::Ping(vec![])).await {
                    return Err(format!("ping failed: {e}"));
                }
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else {
                    return Err("command channel closed".to_string());
                };
                match cmd {
                    ClientCmd::Request { request_type, request_data, reply } => {
                        send_tracked(
                            &mut ws_sink,
                            &mut pending,
                            &request_type,
                            request_data,
                            QueryCtx::None,
                            Some(reply),
                        )
                        .await?;
                    }
                }
            }
            msg = ws_src.next() => {
                let Some(msg) = msg else {
                    return Err("connection closed by OBS".to_string());
                };
                let msg = msg.map_err(|e| format!("socket read error: {e}"))?;
                match msg {
                    WsMessage::Text(text) => {
                        match serde_json::from_str::<Message>(&text) {
                            Ok(Message::Event(event)) => {
                                handle_event_frame(state, push_tx, &mut ws_sink, &mut pending, event).await;
                            }
                            Ok(Message::RequestResponse(resp)) => {
                                handle_response_frame(state, push_tx, &mut ws_sink, &mut pending, resp).await;
                            }
                            Ok(_) => {}
                            Err(e) => debug!(error = %e, "unparseable OBS frame"),
                        }
                    }
                    WsMessage::Ping(data) => {
                        let _ = ws_sink.send(WsMessage::Pong(data)).await;
                    }
                    WsMessage::Close(_) => {
                        return Err("OBS closed connection".to_string());
                    }
                    _ => {}
                }
            }
        }
    }
}

/// One inbound event: reduce, refresh backing lists the event cannot
/// carry, push a snapshot when something changed.
async fn handle_event_frame(
    state: &Arc<RwLock<ObsState>>,
    push_tx: &mpsc::UnboundedSender<Value>,
    ws_sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    pending: &mut HashMap<String, PendingResponse>,
    event: Event,
) {
    let changed = state.write().await.handle_event(&event.event_type, &event.event_data);

    // CurrentProgramSceneChanged: the new scene's item list is needed
    // for the ::source watch keys (the event names the scene only)
    if event.event_type == "CurrentProgramSceneChanged" {
        if let Some(scene) = event.event_data.get("sceneName").and_then(Value::as_str) {
            let _ = send_tracked(
                ws_sink,
                pending,
                "GetSceneItemList",
                Some(serde_json::json!({ "sceneName": scene })),
                QueryCtx::Scene(scene.to_string()),
                None,
            )
            .await;
        }
    }
    // SceneItemEnableStateChanged carries a numeric item id only; when
    // the id is unknown the backing list refresh resolves it
    if event.event_type == "SceneItemEnableStateChanged" {
        if let (Some(scene), Some(id)) = (
            event.event_data.get("sceneName").and_then(Value::as_str),
            event.event_data.get("sceneItemId").and_then(Value::as_i64),
        ) {
            let known = state.read().await.knows_scene_item(scene, &id.to_string());
            if !known {
                let _ = send_tracked(
                    ws_sink,
                    pending,
                    "GetSceneItemList",
                    Some(serde_json::json!({ "sceneName": scene })),
                    QueryCtx::Scene(scene.to_string()),
                    None,
                )
                .await;
            }
        }
    }

    if changed {
        let snapshot = state.read().await.to_snapshot();
        let _ = push_tx.send(snapshot);
    }
}

/// One inbound response: update state from type + ctx, fan out the
/// per-input queries after the input list lands, then resolve the
/// pending user reply.
async fn handle_response_frame(
    state: &Arc<RwLock<ObsState>>,
    push_tx: &mpsc::UnboundedSender<Value>,
    ws_sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        WsMessage,
    >,
    pending: &mut HashMap<String, PendingResponse>,
    resp: RequestResponse,
) {
    let entry = pending.remove(&resp.request_id);
    let ctx = entry
        .as_ref()
        .map(|p| p.ctx.clone())
        .unwrap_or(QueryCtx::None);

    if let Some(data) = &resp.response_data {
        // GetInputList fans out into per-input queries (mute, volume,
        // filter list) - the response itself stores nothing
        if resp.request_type == "GetInputList" {
            if let Some(inputs) = data.get("inputs").and_then(Value::as_array) {
                for input in inputs {
                    if let Some(name) = input.get("inputName").and_then(Value::as_str) {
                        let ctx = QueryCtx::Input(name.to_string());
                        let _ = send_tracked(
                            ws_sink,
                            pending,
                            "GetInputMute",
                            Some(serde_json::json!({ "inputName": name })),
                            ctx.clone(),
                            None,
                        )
                        .await;
                        let _ = send_tracked(
                            ws_sink,
                            pending,
                            "GetInputVolume",
                            Some(serde_json::json!({ "inputName": name })),
                            ctx.clone(),
                            None,
                        )
                        .await;
                        let _ = send_tracked(
                            ws_sink,
                            pending,
                            "GetSourceFilterList",
                            Some(serde_json::json!({ "sourceName": name })),
                            ctx,
                            None,
                        )
                        .await;
                    }
                }
            }
        }
        // a fresh program scene needs its item list for the watch keys
        if resp.request_type == "GetCurrentProgramScene" {
            if let Some(scene) = data
                .get("currentProgramSceneName")
                .and_then(Value::as_str)
            {
                let _ = send_tracked(
                    ws_sink,
                    pending,
                    "GetSceneItemList",
                    Some(serde_json::json!({ "sceneName": scene })),
                    QueryCtx::Scene(scene.to_string()),
                    None,
                )
                .await;
            }
        }

        let changed = state
            .write()
            .await
            .handle_response(&resp.request_type, &ctx, data);
        if changed {
            let snapshot = state.read().await.to_snapshot();
            let _ = push_tx.send(snapshot);
        }
    }

    if let Some(entry) = entry {
        if let Some(reply) = entry.reply {
            if resp.request_status.result {
                let _ = reply.send(Ok(resp.response_data.unwrap_or(Value::Null)));
            } else {
                let comment = resp
                    .request_status
                    .comment
                    .unwrap_or_else(|| "Błąd zapytania OBS".to_string());
                let _ = reply.send(Err(comment));
            }
        }
    }
}

/// The action runner: consumes the sync `exec` queue and drives the
/// async request methods. Awaiting here is safe - the connection worker
/// stays free to serve the requests. Every failure is logged with the
/// tile kind; nothing is ever surfaced per press to clients.
async fn run_actions(obs: Obs, mut rx: mpsc::UnboundedReceiver<ObsAction>) {
    while let Some(action) = rx.recv().await {
        let kind = action.label();
        let result = match action {
            ObsAction::SetScene(scene) => obs.set_current_program_scene(&scene).await,
            ObsAction::ToggleSceneItem { scene, source } => {
                obs.toggle_scene_item(scene.as_deref(), &source).await
            }
            ObsAction::ToggleMute(input) => obs.toggle_input_mute(&input).await,
            ObsAction::ToggleFilter { source, filter } => {
                obs.toggle_source_filter(source.as_deref(), &filter).await
            }
            ObsAction::ToggleStudioMode => obs.toggle_studio_mode().await,
            ObsAction::ToggleRecord => obs.toggle_record().await,
            ObsAction::ToggleStream => obs.toggle_stream().await,
            ObsAction::SaveReplay => obs.save_replay_buffer().await,
            ObsAction::SetVolume { input, mul } => obs.set_input_volume(&input, mul).await,
        };
        if let Err(e) = result {
            warn!(kind, error = %e, "obs action failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_maps_the_stored_command_shapes() {
        // scene
        assert_eq!(
            parse_action("obs-scene", r#"{"scene":"Game"}"#, None).unwrap(),
            Some(ObsAction::SetScene("Game".into()))
        );
        // source without a scene: the current program scene
        assert_eq!(
            parse_action("obs-source", r#"{"source":"Webcam"}"#, None).unwrap(),
            Some(ObsAction::ToggleSceneItem {
                scene: None,
                source: "Webcam".into()
            })
        );
        // device audio: `device`, with a `source` fallback for old edits
        assert_eq!(
            parse_action("obs-device-audio", r#"{"device":"Mic"}"#, None).unwrap(),
            Some(ObsAction::ToggleMute("Mic".into()))
        );
        assert_eq!(
            parse_action("obs-device-audio", r#"{"source":"Old"}"#, None).unwrap(),
            Some(ObsAction::ToggleMute("Old".into()))
        );
        // filter with the optional source
        assert_eq!(
            parse_action("obs-filter", r#"{"filter":"Blur"}"#, None).unwrap(),
            Some(ObsAction::ToggleFilter {
                source: None,
                filter: "Blur".into()
            })
        );
        assert_eq!(
            parse_action("obs-filter", r#"{"source":"Webcam","filter":"Blur"}"#, None).unwrap(),
            Some(ObsAction::ToggleFilter {
                source: Some("Webcam".into()),
                filter: "Blur".into()
            })
        );
        // plain toggles need no arguments (junk command tolerated)
        for (kind, action) in [
            ("obs-studio-mode", ObsAction::ToggleStudioMode),
            ("obs-record", ObsAction::ToggleRecord),
            ("obs-stream", ObsAction::ToggleStream),
            ("obs-replay-save", ObsAction::SaveReplay),
        ] {
            assert_eq!(parse_action(kind, "", None).unwrap(), Some(action), "{kind}");
        }
        // slider value rides the exec_slider argument
        assert_eq!(
            parse_action("obs-audio-slider", r#"{"device":"Mic"}"#, Some(0.42)).unwrap(),
            Some(ObsAction::SetVolume {
                input: "Mic".into(),
                mul: 0.42
            })
        );
        // out-of-range values clamp
        assert_eq!(
            parse_action("obs-audio-slider", r#"{"device":"Mic"}"#, Some(2.0)).unwrap(),
            Some(ObsAction::SetVolume {
                input: "Mic".into(),
                mul: 1.0
            })
        );
    }

    #[test]
    fn parse_action_rejects_malformed_payloads_and_foreign_kinds() {
        assert!(parse_action("obs-scene", "{}", None).is_err());
        assert!(parse_action("obs-scene", "not json", None).is_err());
        assert!(parse_action("obs-source", r#"{"scene":"S"}"#, None).is_err());
        assert!(parse_action("obs-audio-slider", r#"{"device":"Mic"}"#, None).is_err());
        // unimplemented obs kinds and foreign kinds are not ours
        assert_eq!(parse_action("obs-transition", "", None).unwrap(), None);
        assert_eq!(parse_action("slobs-scene", r#"{"scene":"S"}"#, None).unwrap(), None);
        assert_eq!(parse_action("key", "CTRL+K", None).unwrap(), None);
        // ...but the implemented list matches exactly what exec claims
        for kind in [
            "obs-scene",
            "obs-source",
            "obs-device-audio",
            "obs-filter",
            "obs-studio-mode",
            "obs-record",
            "obs-stream",
            "obs-replay-save",
            "obs-audio-slider",
        ] {
            assert!(is_obs_action(kind), "{kind}");
        }
        assert!(!is_obs_action("obs-transition"));
        assert!(!is_obs_action("slobs-scene"));
    }
}
