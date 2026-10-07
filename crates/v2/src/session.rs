//! One authenticated WebSocket session: hello handshake, welcome + full
//! syncs, interaction dispatch (incl. server-side hold-to-repeat loops)
//! and the ping/queue keepalive (docs/protocol-v2.md §1, §3, §6): the
//! pump owns pings, the outbound queue is bounded (a peer that stops
//! reading closes the session instead of growing the queue forever) and
//! the hosts' reaper drops sessions silent past the ping timeout.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use pulpit_actions::EventSink;
use pulpit_db::ButtonRow;
use pulpit_proto::*;

use crate::devices::{PairError, Pairing};
use crate::hub::V2Session;
use crate::service::{Auth, V2State};
use crate::state::{ext_channel, StateEngine};

/// Outbound queue bound. Generous against current push rates (a healthy
/// client drains it well under a second); overflow means the peer stopped
/// reading, which closes the session.
const QUEUE_CAP: usize = 256;

/// Messages queued for the connection's outbound pump.
pub enum WsOut {
    Text(String),
    Pong(Vec<u8>),
    /// Flush the queue, send a WS close frame, end the pump.
    Close,
}

pub(super) async fn run(state: Arc<V2State>, socket: WebSocket, auth: Auth) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsOut>(QUEUE_CAP);
    let session = state.hub.create(out_tx.clone());
    tracing::info!(session = session.id, "v2 session open");

    // Outbound pump: frames from anywhere (session handlers, broadcasts)
    // plus the protocol-level pings. The first interval tick fires
    // immediately, so consume it.
    let pump: tokio::task::JoinHandle<()> = {
        let ping_interval = state.config.ping_interval;
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(ping_interval);
            ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ping.tick().await;
            loop {
                tokio::select! {
                    msg = out_rx.recv() => match msg {
                        Some(WsOut::Text(text)) => {
                            if sink.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        Some(WsOut::Pong(payload)) => {
                            if sink.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Some(WsOut::Close) => {
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        }
                        None => break,
                    },
                    _ = ping.tick() => {
                        if sink.send(Message::Ping(Vec::new())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        })
    };
    // The hub's teardown path (overflow, silence watchdog) aborts the pump
    // through the session; the handle lives there, not in a local.
    session.set_pump(pump);

    // The session body races hub-side teardown (queue overflow, silence
    // watchdog, server exit): those paths cancel the session so the task
    // ends promptly instead of lingering on a dead socket, and the tail
    // below always runs its cleanup.
    let mut cancelled = session.cancelled();
    let outcome = tokio::select! {
        outcome = run_session(&state, &session, &mut stream, &out_tx, auth) => outcome,
        _ = cancelled.wait_for(|v| *v) => End::Cancelled,
    };
    // Unconditional teardown on every path: stop repeat loops, then run
    // the release phase for keys whose press-start never got a
    // press-end (§6: a repeat loop "is released when the connection
    // dies").
    session.abort_holds();
    release_held_keys(&state, &session).await;
    if outcome.flushes_pump() {
        // Fatal or cancelled by the hub: let the pump flush the queued
        // frames (error bursts, the shutdown goodbye), then close
        // politely - an abort would drop them.
        let _ = out_tx.try_send(WsOut::Close);
        session.finish_pump(Duration::from_millis(500)).await;
    } else {
        session.abort_pump();
    }
    state.hub.remove(session.id);
    tracing::info!(session = session.id, "v2 session closed");
}

/// Outcome of the session body: whether to close the TCP socket.
#[derive(PartialEq)]
enum End {
    /// Client went away or hung up cleanly.
    Closed,
    /// Server decided the connection must die (auth/size violations).
    Fatal,
    /// Hub-side teardown (queue overflow, silence watchdog, exit).
    Cancelled,
    // Reaching the end of `run_session` without either just means the
    // stream ended; it maps to Closed.
}

impl End {
    /// Whether the pump should flush queued frames before the close.
    /// Cancelled sessions may still owe the client frames (the shutdown
    // goodbye), so they flush like fatal ones.
    fn flushes_pump(&self) -> bool {
        !matches!(self, End::Closed)
    }
}

async fn run_session(
    state: &Arc<V2State>,
    session: &Arc<V2Session>,
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    out_tx: &mpsc::Sender<WsOut>,
    auth: Auth,
) -> End {
    // 1) hello within the timeout, or the connection dies.
    let frame = match tokio::time::timeout(state.config.hello_timeout, next_text(stream)).await {
        Ok(Some(Ok(frame))) => frame,
        // Unparseable text is a protocol violation, not a silent drop:
        // answer `bad-frame` like the post-handshake loop does (NET-05),
        // then close - there is no session to continue with.
        Ok(Some(Err(_))) => {
            session.send_frame(&Frame::push(
                TYPE_ERROR,
                json!({"code": error_code::BAD_FRAME, "message": "hello payload"}),
            ));
            return End::Fatal;
        }
        Ok(None) => return End::Closed,
        Err(_) => {
            session.send_frame(&Frame::push(
                TYPE_ERROR,
                json!({"code": error_code::BAD_FRAME, "message": "hello timeout"}),
            ));
            return End::Fatal;
        }
    };
    session.touch();
    let hello: Hello = match serde_json::from_value(frame.payload.clone().unwrap_or(Value::Null)) {
        Ok(h) => h,
        Err(_) => {
            session.send_frame(&error_ack(&frame, error_code::BAD_FRAME, "hello payload"));
            return End::Fatal;
        }
    };
    if frame.kind != TYPE_HELLO || frame.ack.is_some() {
        session.send_frame(&error_ack(&frame, error_code::BAD_FRAME, "expected hello"));
        return End::Fatal;
    }

    if version_lt(&hello.version, &state.config.min_client) {
        session.send_frame(&error_ack(
            &frame,
            error_code::OUTDATED_CLIENT,
            &format!("client {} < min {}", hello.version, state.config.min_client),
        ));
        tracing::info!(session = session.id, client = %hello.client, version = %hello.version, "outdated client");
        return End::Fatal;
    }

    // 2) Resolve the device: known token, or consume the pairing code.
    // A fresh pairing issues the token to the device via `welcome.token`.
    let mut issued_token: Option<String> = None;
    let device = match auth {
        Auth::Device(device) => {
            // hello.name may rename a paired device; persisted so the next
            // welcome and the desktop device list agree. The welcome
            // carries the updated entry, not the pre-auth snapshot. The
            // name is untrusted client input: sanitized before it can
            // reach the registry, the logs or the desktop device list.
            let rename_to = sanitize_device_name(hello.name.as_deref());
            let touched = match rename_to {
                Some(name) if name != device.name => state.devices.rename(&device.id, &name),
                _ => state.devices.touch(&device.id),
            };
            match touched {
                Some(device) => device,
                // The entry vanished between the upgrade's verify and
                // here (revoked mid-handshake): treat as revoked, not as
                // the stale pre-auth snapshot (audit B2 step 3).
                None => {
                    session.send_frame(&error_ack(
                        &frame,
                        error_code::UNAUTHORIZED,
                        "device no longer trusted",
                    ));
                    return End::Fatal;
                }
            }
        }
        Auth::Pair(code) => match state.pairing.consume(&code) {
            Ok(()) => {
                let name = sanitize_device_name(hello.name.as_deref())
                    .unwrap_or_else(|| "Device".to_string());
                // Trust gate (audit B2 step 6): a fresh pairing needs the
                // desktop operator's approval before a device is minted.
                // The consult runs on the blocking pool (the prompt may
                // wait for the operator; the socket loop must not stall)
                // and is bounded by the pairing-code TTL: an unanswered
                // prompt denies once the code would have expired anyway.
                // A denial rejects the hello; the one-time code is
                // already burned, so a retry needs a fresh code.
                if !approve_pairing(&state.pairing, &name, &code).await {
                    tracing::warn!(session = session.id, name = %name, "pairing denied on the desktop");
                    session.send_frame(&error_ack(
                        &frame,
                        error_code::PAIR_EXPIRED,
                        "pairing was not approved on the desktop in time",
                    ));
                    return End::Fatal;
                }
                tracing::info!(session = session.id, name = %name, "pairing approved on the desktop");
                let (device, token) = state.devices.create(&name);
                issued_token = Some(token);
                device
            }
            Err(PairError::Expired) => {
                session.send_frame(&error_ack(
                    &frame,
                    error_code::PAIR_EXPIRED,
                    "pairing code expired",
                ));
                return End::Fatal;
            }
            Err(PairError::Invalid) => {
                session.send_frame(&error_ack(
                    &frame,
                    error_code::PAIR_INVALID,
                    "unknown pairing code",
                ));
                return End::Fatal;
            }
        },
    };
    session.set_device(device.clone());
    tracing::info!(session = session.id, device = %device.name, client = %hello.client, version = %hello.version, "v2 client authenticated");
    // M5 capability negotiation: the declared set is logged so a missing
    // feature on some client is diagnosable from the server log. Gating
    // pushes on it is a separate, later decision.
    if !hello.capabilities.is_empty() {
        tracing::info!(session = session.id, capabilities = ?hello.capabilities, "client capabilities");
    }

    // Broadcast fan-out starts only now that the session is authenticated:
    // a pre-auth socket must never see pushes. Attaching before the boards
    // build means no delta published during the build is lost, and the
    // build loop in `boards_snapshot` keeps the snapshot at least as new
    // as any delta already queued for this socket.
    state.hub.attach(session);

    // 3) Welcome + full syncs. Boards build first: it registers tile
    // channels so the catalog in `welcome` is already complete. Both
    // frames carry the same stable generation.
    let (boards_sync, generation) = state.boards_snapshot().await;
    let welcome = Welcome {
        protocol: PROTOCOL_VERSION,
        desktop_version: state.config.desktop_version.clone(),
        min_client: state.config.min_client.clone(),
        generation,
        device: Device {
            id: device.id,
            name: device.name,
        },
        channels: state.engine.catalog(),
        token: issued_token,
        capabilities: SERVER_CAPABILITIES.iter().map(|s| s.to_string()).collect(),
    };
    session.send_frame(&Frame {
        v: PROTOCOL_VERSION,
        id: None,
        ack: frame.id.clone(),
        kind: TYPE_WELCOME.into(),
        payload: Some(serde_json::to_value(&welcome).unwrap_or(Value::Null)),
    });
    session.send_frame(&boards_sync);
    session.send_frame(&Frame::push_typed(
        TYPE_STATE_SYNC,
        &state.engine.snapshot(),
    ));
    // Handshake complete: the flusher may start patching this socket.
    // Everything drained before this point is covered by the state sync
    // above (or re-marked dirty and arriving with the next flush).
    session.set_welcomed();

    // 4) Live frames until the client goes away.
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                session.touch();
                if text.len() > MAX_FRAME_BYTES {
                    session.send_frame(&Frame::push(
                        TYPE_ERROR,
                        json!({"code": error_code::TOO_LARGE, "message": "frame over 1 MiB"}),
                    ));
                    return End::Fatal;
                }
                let parsed: Result<Frame, _> = serde_json::from_str(&text);
                match parsed {
                    Ok(frame) => handle_frame(state, session, frame).await,
                    Err(e) => {
                        tracing::debug!(session = session.id, error = %e, "bad v2 frame");
                        session.send_frame(&Frame::push(
                            TYPE_ERROR,
                            json!({"code": error_code::BAD_FRAME, "message": e.to_string()}),
                        ));
                    }
                }
            }
            Ok(Message::Ping(payload)) => {
                session.touch();
                let _ = out_tx.try_send(WsOut::Pong(payload));
            }
            Ok(Message::Pong(_)) => session.touch(),
            Ok(Message::Close(_)) => return End::Closed,
            Ok(_) => {}
            Err(_) => return End::Closed,
        }
    }
    End::Closed
}

/// Reads until the next text frame, letting pings flow (the pump answers
/// pongs; tungstenite on the client side does the same for us).
async fn next_text(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Option<Result<Frame, serde_json::Error>> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => return Some(serde_json::from_str(&text)),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Asks the pairing trust gate about a fresh device, off the async
/// workers (the gate may block on the operator). The wait is bounded by
/// the pairing's code TTL (the same shrinkable timer the codes use); a
/// timeout, a denied answer or a failed blocking dispatch all count as
/// a denial.
async fn approve_pairing(pairing: &Arc<Pairing>, name: &str, code: &str) -> bool {
    // M8 Bluetooth-style requests arrive pre-approved: the operator just
    // confirmed the dialog carrying this very code. Single-use, so a
    // replayed code cannot mint a second device.
    if pairing.take_pre_approved(code) {
        return true;
    }
    let wait = pairing.ttl();
    let name = name.to_string();
    let pairing = pairing.clone();
    let ask = tokio::task::spawn_blocking(move || pairing.ask_trust(&name));
    match tokio::time::timeout(wait, ask).await {
        Ok(Ok(approved)) => approved,
        Ok(Err(_)) | Err(_) => false,
    }
}

fn error_ack(request: &Frame, code: &str, message: &str) -> Frame {
    Frame {
        v: PROTOCOL_VERSION,
        id: None,
        ack: request.id.clone(),
        kind: TYPE_ERROR.into(),
        payload: Some(json!({"code": code, "message": message})),
    }
}

async fn handle_frame(state: &Arc<V2State>, session: &Arc<V2Session>, frame: Frame) {
    if frame.ack.is_some() {
        // Clients never answer server pushes.
        tracing::debug!(session = session.id, type = %frame.kind, "unexpected ack from client");
        return;
    }
    match (frame.kind.as_str(), frame.id.is_some()) {
        (TYPE_INTERACTION, true) => handle_interaction(state, session, frame).await,
        (_, true) => {
            session.send_frame(&error_ack(
                &frame,
                error_code::UNKNOWN_TYPE,
                "unsupported request type",
            ));
        }
        // Server pushes from the future or junk: ignore, log.
        _ => tracing::debug!(session = session.id, type = %frame.kind, "ignored v2 frame"),
    }
}

async fn handle_interaction(state: &Arc<V2State>, session: &Arc<V2Session>, frame: Frame) {
    let payload: InteractionPayload =
        match serde_json::from_value(frame.payload.clone().unwrap_or(Value::Null)) {
            Ok(p) => p,
            Err(e) => {
                session.send_frame(&error_ack(&frame, error_code::BAD_FRAME, &e.to_string()));
                return;
            }
        };
    // The lookup is a SQLite read; it must not run on the async workers.
    // The image-less read: taps and slides hit this per event and the
    // image columns only bloat the row (the exec path never reads them).
    let backend = state.backend.clone();
    let (tile, board) = (payload.tile, payload.board);
    let button = match tokio::task::spawn_blocking(move || {
        backend
            .get_button_meta(tile)
            .filter(|b| b.board_id == board)
    })
    .await
    {
        Ok(button) => button,
        Err(join) => {
            tracing::error!(session = session.id, tile, error = %join, "tile lookup panicked");
            session.send_frame(&error_ack(
                &frame,
                error_code::UNKNOWN_TILE,
                "no such tile on that board",
            ));
            return;
        }
    };
    let Some(button) = button else {
        session.send_frame(&error_ack(
            &frame,
            error_code::UNKNOWN_TILE,
            "no such tile on that board",
        ));
        return;
    };
    // Only gestures the tile declares are served; everything else (incl.
    // wheel/drag, which no M1 tile declares) is a typed error.
    if !crate::boards::allowed_interactions(&button).contains(&payload.interaction) {
        session.send_frame(&error_ack(
            &frame,
            error_code::UNSUPPORTED_INTERACTION,
            "gesture not declared for this tile",
        ));
        return;
    }

    // An ack echoes the request's `id` and its type; the client matches
    // on `ack` alone.
    let id = frame.id.clone().unwrap_or_default();
    let kind = frame.kind.clone();
    let ack_ok = || {
        session.send_frame(&Frame {
            v: PROTOCOL_VERSION,
            id: None,
            ack: Some(id.clone()),
            kind: kind.clone(),
            payload: Some(serde_json::to_value(Ack {}).unwrap_or(Value::Null)),
        })
    };

    match payload.interaction {
        Interaction::Tap => {
            ack_ok();
            // a tap is a full click: release-phase semantics, so every
            // kind acts exactly once (tap-start only drives held keys)
            exec_once(state, button, false);
        }
        Interaction::PressStart => {
            ack_ok();
            exec_once(state, button.clone(), true);
            // tap-start only holds something down for key tiles; their
            // release phase must run even if this socket never sends
            // press-end (dropped tablet, overflow teardown, exit).
            if button.kind == "key" {
                session.key_pressed(payload.tile, button.clone());
            }
            let params: Value = serde_json::from_str(button.options.as_deref().unwrap_or(""))
                .unwrap_or(Value::Null);
            if let Some((delay_ms, interval_ms)) = crate::boards::hold_repeat_config(&params) {
                start_hold(
                    state,
                    session.clone(),
                    button.clone(),
                    delay_ms,
                    interval_ms,
                );
            }
        }
        Interaction::PressEnd => {
            session.stop_hold(payload.tile);
            session.key_released(payload.tile);
            ack_ok();
            exec_once(state, button, false);
        }
        Interaction::Slide => {
            ack_ok();
            let value = payload.args.value.unwrap_or(0.0);
            let backend = state.backend.clone();
            tokio::task::spawn_blocking(move || backend.slider(button, value));
        }
        Interaction::LongPress | Interaction::DoubleTap | Interaction::SwipeLeft | Interaction::SwipeRight => {
            ack_ok();
            let mut target_button = button;
            // an editor-configured `gesture_actions` override replaces the
            // kind/command per field; without one the tile's own action fires
            // (tool tiles execute their own gesture semantics instead)
            let gesture = match payload.interaction {
                Interaction::LongPress => "long-press",
                Interaction::DoubleTap => "double-tap",
                Interaction::SwipeLeft => "swipe-left",
                _ => "swipe-right",
            };
            let params: Value = serde_json::from_str(
                target_button.options.as_deref().unwrap_or(""),
            )
            .unwrap_or(Value::Null);
            if let Some((kind, command)) =
                crate::boards::gesture_action_override(&params, gesture)
            {
                target_button.kind = kind;
                if let Some(command) = command {
                    target_button.command = Some(command);
                }
            }
            exec_gesture(state, target_button, gesture);
        }
        // Unreachable while the allowed-interactions check stands (no tile
        // declares wheel/drag); kept as a defensive typed error.
        Interaction::Wheel | Interaction::Drag | Interaction::Other => {
            session.send_frame(&error_ack(
                &frame,
                error_code::UNSUPPORTED_INTERACTION,
                "gesture not declared for this tile",
            ));
        }
    }
}

fn start_hold(
    state: &Arc<V2State>,
    session: Arc<V2Session>,
    button: ButtonRow,
    delay_ms: u64,
    interval_ms: u64,
) {
    let engine = state.engine.clone();
    let hub = state.hub.clone();
    let backend = state.backend.clone();
    let cap = state.config.hold_cap;
    let tile = button.id;
    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        let _ = tokio::time::timeout(cap, async {
            loop {
                // each repeat tick executes the action (release phase) -
                // tap-start would no-op everything except held keys. The
                // exec is blocking work (actions sleep, fetch URLs,
                // re-auth): it must run on the blocking pool, and the
                // await serializes ticks so a slow one is never
                // overlapped by the next. The server runtime is
                // single-threaded - calling it inline would stall every
                // socket for the duration of the action.
                let tick_button = button.clone();
                let tick_backend = backend.clone();
                let tick_engine = engine.clone();
                let tick_hub = hub.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    exec_blocking(&tick_backend, &tick_engine, &tick_hub, tick_button, false)
                })
                .await;
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        })
        .await;
        tracing::debug!(tile, "hold repeat ended");
    });
    session.insert_hold(tile, handle);
}

fn exec_gesture(state: &Arc<V2State>, button: ButtonRow, gesture: &str) {
    let backend = state.backend.clone();
    let engine = state.engine.clone();
    let hub = state.hub.clone();
    let gesture = gesture.to_string();
    tokio::task::spawn_blocking(move || {
        exec_gesture_blocking(&backend, &engine, &hub, button, &gesture)
    });
}

fn exec_gesture_blocking(
    backend: &Arc<dyn pulpit_legacy::Backend>,
    engine: &Arc<StateEngine>,
    hub: &Arc<crate::hub::V2Hub>,
    button: ButtonRow,
    gesture: &str,
) {
    let (board_tx, mut board_rx) = mpsc::unbounded_channel::<i64>();
    let (value_tx, mut value_rx) = mpsc::unbounded_channel::<(String, String)>();
    struct Sink(
        tokio::sync::mpsc::UnboundedSender<i64>,
        tokio::sync::mpsc::UnboundedSender<(String, String)>,
    );
    impl EventSink for Sink {
        fn change_board(&mut self, board_id: i64) {
            let _ = self.0.send(board_id);
        }
        fn app_value(&mut self, key: &str, value: &str) {
            let _ = self.1.send((key.to_string(), value.to_string()));
        }
    }
    let mut sink = Sink(board_tx, value_tx);
    backend.exec_gesture(button, gesture, &mut sink);
    while let Ok(board) = board_rx.try_recv() {
        hub.broadcast_frame(&Frame::push_typed(TYPE_BOARD_OPEN, &BoardOpen { board }));
    }
    while let Ok((key, value)) = value_rx.try_recv() {
        let channel = ext_channel(&key);
        engine.set(
            &channel,
            serde_json::from_str(&value).unwrap_or(Value::String(value)),
        );
    }
}

/// Executes a tile command off the async workers; its effects (board
/// switch, custom values) surface as broadcasts/state, like legacy.
fn exec_once(state: &Arc<V2State>, button: ButtonRow, is_tap_start: bool) {
    let backend = state.backend.clone();
    let engine = state.engine.clone();
    let hub = state.hub.clone();
    tokio::task::spawn_blocking(move || {
        exec_blocking(&backend, &engine, &hub, button, is_tap_start)
    });
}

/// Runs the release phase for every key the session left pressed. The
/// exec is blocking work (enigo key-ups), so it runs on the blocking
/// pool; the session task has nothing else to do afterwards, so the
/// await is harmless even for slow backends.
async fn release_held_keys(state: &Arc<V2State>, session: &Arc<V2Session>) {
    let held = session.take_held_keys();
    if held.is_empty() {
        return;
    }
    tracing::info!(
        session = session.id,
        tiles = held.len(),
        "releasing keys held by the closing session"
    );
    let backend = state.backend.clone();
    let engine = state.engine.clone();
    let hub = state.hub.clone();
    let _ = tokio::task::spawn_blocking(move || {
        for button in held {
            exec_blocking(&backend, &engine, &hub, button, false);
        }
    })
    .await;
}

fn exec_blocking(
    backend: &Arc<dyn pulpit_legacy::Backend>,
    engine: &Arc<StateEngine>,
    hub: &Arc<crate::hub::V2Hub>,
    button: ButtonRow,
    is_tap_start: bool,
) {
    let (board_tx, mut board_rx) = mpsc::unbounded_channel::<i64>();
    let (value_tx, mut value_rx) = mpsc::unbounded_channel::<(String, String)>();
    struct Sink(
        tokio::sync::mpsc::UnboundedSender<i64>,
        tokio::sync::mpsc::UnboundedSender<(String, String)>,
    );
    impl EventSink for Sink {
        fn change_board(&mut self, board_id: i64) {
            let _ = self.0.send(board_id);
        }
        fn app_value(&mut self, key: &str, value: &str) {
            let _ = self.1.send((key.to_string(), value.to_string()));
        }
    }
    let mut sink = Sink(board_tx, value_tx);
    backend.exec(button, is_tap_start, &mut sink);
    while let Ok(board) = board_rx.try_recv() {
        hub.broadcast_frame(&Frame::push_typed(TYPE_BOARD_OPEN, &BoardOpen { board }));
    }
    while let Ok((key, value)) = value_rx.try_recv() {
        engine.set(&ext_channel(&key), Value::String(value));
    }
}

/// Device names from `hello.name`: untrusted client input that ends up
/// in logs, the registry and the desktop device list - trimmed,
/// control-character-free, capped, `None` when nothing usable remains.
pub(crate) fn sanitize_device_name(raw: Option<&str>) -> Option<String> {
    let name: String = raw
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(DEVICE_NAME_MAX_CHARS)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Cap for `hello.name` (audit B2): enough for any honest device label,
/// small enough that junk cannot bloat logs or `devices.json`.
const DEVICE_NAME_MAX_CHARS: usize = 64;

/// Naive semver-ish compare: numeric dot parts, missing parts are 0.
fn version_lt(client: &str, min: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect()
    }
    let (c, m) = (parts(client), parts(min));
    for i in 0..3 {
        let cv = c.get(i).copied().unwrap_or(0);
        let mv = m.get(i).copied().unwrap_or(0);
        if cv != mv {
            return cv < mv;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pairing_wait_denies_after_the_ttl() {
        // The gate answers "yes" - but only after the pairing's code TTL
        // is spent: the desktop was unreachable, so the wait must deny
        // (the TTL-expiry fallback, audit B2 step 6).
        let pairing = crate::devices::Pairing::with_ttl(Duration::from_millis(50));
        pairing.set_trust_gate(|_name| {
            std::thread::sleep(Duration::from_millis(300));
            true
        });
        assert!(
            !approve_pairing(&std::sync::Arc::new(pairing), "Tablet salon", "CODE1234").await,
            "a gate that answers past the TTL is a denial"
        );
    }

    #[test]
    fn version_compare() {
        assert!(version_lt("0.1.0", "0.2.0"));
        assert!(version_lt("0.1", "0.1.1"));
        assert!(!version_lt("0.2.0", "0.2.0"));
        assert!(!version_lt("1.0.0", "0.9.9"));
        assert!(version_lt("0.2.0", "9.9.9"));
        assert!(!version_lt("garbage", "0.0.0"));
    }

    #[test]
    fn device_names_are_sanitized() {
        // trim + control stripping, alone and combined
        assert_eq!(
            sanitize_device_name(Some("  Tablet salon \n")),
            Some("Tablet salon".into())
        );
        assert_eq!(
            sanitize_device_name(Some("a\u{0}b\u{7}c")),
            Some("abc".into())
        );
        assert_eq!(sanitize_device_name(Some(" \u{1}\t ")), None);
        assert_eq!(sanitize_device_name(Some("")), None);
        assert_eq!(sanitize_device_name(None), None);
        // cap at 64 chars, by characters not bytes
        let long = "ą".repeat(100);
        assert_eq!(
            sanitize_device_name(Some(&long)).unwrap().chars().count(),
            DEVICE_NAME_MAX_CHARS
        );
    }

    #[test]
    fn hold_config_parsing() {
        let parse = |options: &str| {
            let params: Value = serde_json::from_str(options).unwrap();
            crate::boards::hold_repeat_config(&params)
        };
        assert_eq!(
            parse(r#"{"hold":{"repeat":{"delay_ms":400,"interval_ms":120}}}"#),
            Some((400, 120))
        );
        assert_eq!(parse(r#"{"hold":{}}"#), None);
        assert_eq!(
            parse(r#"{"hold":{"repeat":{"delay_ms":0,"interval_ms":5}}}"#),
            None
        );
        assert_eq!(parse("{}"), None);
        // NET-06: an imported options JSON is free-form - junk rates are
        // clamped into 50..=60000 ms instead of trusted
        assert_eq!(
            parse(r#"{"hold":{"repeat":{"delay_ms":1,"interval_ms":1}}}"#),
            Some((50, 50))
        );
        assert_eq!(
            parse(r#"{"hold":{"repeat":{"delay_ms":999999,"interval_ms":120}}}"#),
            Some((60000, 120))
        );
    }

    #[test]
    fn interaction_declarations_follow_command_style() {
        use pulpit_proto::Interaction;
        fn allowed(row: &ButtonRow) -> Vec<Interaction> {
            crate::boards::allowed_interactions(row)
        }
        let mk = |kind: &str, options: Option<String>| ButtonRow {
            id: 1,
            board_id: 1,
            kind: kind.into(),
            command: Some("x".into()),
            title: None,
            title_position: 0,
            title_color: None,
            title_box_color: None,
            color: None,
            icon_color: None,
            icon_color2: None,
            border_color: None,
            shape: 0,
            icon: None,
            img: None,
            img2: None,
            icon2: None,
            color2: None,
            shape2: 0,
            border_color2: None,
            title_position2: 0,
            title_box_color2: None,
            title_color2: None,
            position: None,
            position2: 0,
            mode: "button".into(),
            x: Some(0),
            y: Some(0),
            w: 1,
            h: 1,
            options,
        };
        let plain = allowed(&mk("url", None));
        assert_eq!(plain, vec![Interaction::Tap]);
        let key = allowed(&mk("key", None));
        assert!(key.contains(&Interaction::PressStart) && key.contains(&Interaction::PressEnd));
        let hold = allowed(&mk(
            "vol",
            Some(r#"{"hold":{"repeat":{"delay_ms":400,"interval_ms":120}}}"#.into()),
        ));
        assert!(hold.contains(&Interaction::PressStart));
    }
}
