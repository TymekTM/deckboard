//! One authenticated WebSocket session: hello handshake, welcome + full
//! syncs, interaction dispatch (incl. server-side hold-to-repeat loops)
//! and the ping/watchdog keepalive (docs/protocol-v2.md §1, §3, §6).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use pulpit_actions::EventSink;
use pulpit_db::ButtonRow;
use pulpit_proto::*;

use crate::devices::PairError;
use crate::hub::V2Session;
use crate::service::{Auth, V2State};
use crate::state::{ext_channel, StateEngine};

/// Messages queued for the connection's outbound pump.
pub enum WsOut {
    Text(String),
    Pong(Vec<u8>),
    /// Flush the queue, send a WS close frame, end the pump.
    Close,
}

pub(super) async fn run(state: Arc<V2State>, socket: WebSocket, auth: Auth) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<WsOut>();
    let session = state.hub.create(out_tx.clone());
    tracing::info!(session = session.id, "v2 session open");

    // Outbound pump: frames from anywhere (session handlers, broadcasts)
    // plus the protocol-level pings. The first interval tick fires
    // immediately, so consume it.
    let pump = {
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

    if run_session(&state, &session, &mut stream, &out_tx, auth)
        .await
        .is_fatal()
    {
        // Fatal: let the pump flush the queued error frames, then close
        // politely - an abort would drop them.
        let _ = out_tx.send(WsOut::Close);
        let _ = tokio::time::timeout(Duration::from_millis(500), pump).await;
    } else {
        pump.abort();
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
    // Reaching the end of `run_session` without either just means the
    // stream ended; it maps to Closed.
}

impl End {
    fn is_fatal(&self) -> bool {
        *self == End::Fatal
    }
}

async fn run_session(
    state: &Arc<V2State>,
    session: &Arc<V2Session>,
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    out_tx: &mpsc::UnboundedSender<WsOut>,
    auth: Auth,
) -> End {
    // 1) hello within the timeout, or the connection dies.
    let frame = match tokio::time::timeout(state.config.hello_timeout, next_text(stream)).await {
        Ok(Some(Ok(frame))) => frame,
        Ok(Some(Err(_))) | Ok(None) => return End::Closed,
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
            match hello.name.as_deref().filter(|n| !n.is_empty()) {
                // hello may rename a paired device; persisted so the next
                // welcome and the desktop device list agree. The welcome
                // carries the updated entry, not the pre-auth snapshot.
                Some(name) if name != device.name => {
                    state.devices.rename(&device.id, name).unwrap_or(device)
                }
                _ => state.devices.touch(&device.id).unwrap_or(device),
            }
        }
        Auth::Pair(code) => match state.pairing.consume(&code) {
            Ok(()) => {
                let name = hello.name.clone().unwrap_or_else(|| "Device".into());
                tracing::warn!(session = session.id, name = %name, "pairing auto-accepted (no UI yet)");
                let device = state.devices.create(&name);
                issued_token = Some(device.token.clone());
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
                let _ = out_tx.send(WsOut::Pong(payload));
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
    let backend = state.backend.clone();
    let (tile, board) = (payload.tile, payload.board);
    let button = match tokio::task::spawn_blocking(move || {
        backend.get_button(tile).filter(|b| b.board_id == board)
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
            let params: Value = serde_json::from_str(button.options.as_deref().unwrap_or("")).unwrap_or(Value::Null);
            if let Some((delay_ms, interval_ms)) = crate::boards::hold_repeat_config(&params) {
                start_hold(state, session.clone(), button.clone(), delay_ms, interval_ms);
            }
        }
        Interaction::PressEnd => {
            session.stop_hold(payload.tile);
            ack_ok();
            exec_once(state, button, false);
        }
        Interaction::Slide => {
            ack_ok();
            let value = payload.args.value.unwrap_or(0.0);
            let backend = state.backend.clone();
            tokio::task::spawn_blocking(move || backend.slider(button, value));
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

fn start_hold(state: &Arc<V2State>, session: Arc<V2Session>, button: ButtonRow, delay_ms: u64, interval_ms: u64) {
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
                // tap-start would no-op everything except held keys
                exec_blocking(&backend, &engine, &hub, button.clone(), false);
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
        })
        .await;
        tracing::debug!(tile, "hold repeat ended");
    });
    session.insert_hold(tile, handle);
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
        assert_eq!(parse(r#"{"hold":{"repeat":{"delay_ms":0,"interval_ms":5}}}"#), None);
        assert_eq!(parse("{}"), None);
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
