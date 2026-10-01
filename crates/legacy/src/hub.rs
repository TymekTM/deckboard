//! Session hub: tracks connected devices, their rooms (PRO/BASIC) and the
//! active Engine.IO transport (polling queue or websocket channel).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, Notify};

use crate::service::Backend;

pub const ACCESS_KEY_PRO: &str = "DCKBRD_PRO_1_3_0";

/// Anything a session can receive while its transport is a websocket.
#[derive(Debug)]
pub enum WsOut {
    /// Plain Engine.IO packet delivered as one websocket text message.
    Packet(String),
    /// The websocket client disconnected; drop the session.
    Closed,
}

struct PollState {
    queue: Mutex<VecDeque<String>>,
    notify: Notify,
}

impl PollState {
    async fn push(&self, packet: String) {
        self.queue.lock().await.push_back(packet);
        self.notify.notify_one();
    }

    async fn drain(&self) -> Option<Vec<String>> {
        let mut q = self.queue.lock().await;
        if q.is_empty() {
            None
        } else {
            Some(q.drain(..).collect())
        }
    }
}

enum Transport {
    Polling(Arc<PollState>),
    Ws(mpsc::UnboundedSender<WsOut>),
}

pub struct Session {
    pub sid: String,
    pub is_pro: bool,
    transport: Mutex<Transport>,
    /// Last client activity (poll, packet); drives the ping-timeout reaper.
    last_seen: Mutex<std::time::Instant>,
    /// Execution seam for releasing keys the session left pressed (the
    /// hub owns teardown, so it owns the undo).
    backend: Arc<dyn Backend>,
    /// Tiles with an un-ended key press-start (`isTapStart: true`): only
    /// key tiles hold anything down, and their release phase must run on
    /// every teardown path - websocket close, poll silence, anything.
    held_keys: Mutex<HashMap<i64, pulpit_db::ButtonRow>>,
}

impl Session {
    pub fn room(&self) -> &'static str {
        if self.is_pro {
            "PRO_ROOM"
        } else {
            "BASIC_ROOM"
        }
    }

    pub async fn send(&self, packet: String) {
        let t = self.transport.lock().await;
        match &*t {
            Transport::Polling(st) => st.push(packet).await,
            Transport::Ws(tx) => {
                let _ = tx.send(WsOut::Packet(packet));
            }
        }
    }

    /// Swap the transport to an attached websocket, delivering queued
    /// packets (e.g. the connect frame) over the new channel.
    pub async fn upgrade_to_ws(&self, tx: mpsc::UnboundedSender<WsOut>) {
        let mut t = self.transport.lock().await;
        if let Transport::Polling(st) = &*t {
            if let Some(packets) = st.drain().await {
                for p in packets {
                    let _ = tx.send(WsOut::Packet(p));
                }
            }
        }
        *t = Transport::Ws(tx);
    }

    /// Long-poll: wait until packets are queued or the timeout elapses.
    /// Returns the packets joined by the Engine.IO v3 record separator.
    pub async fn poll(&self, timeout_ms: u64) -> String {
        const SEP: char = '\u{1e}';
        self.touch().await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            // Grab the polling state (or bail out if a websocket took over).
            let st = {
                let t = self.transport.lock().await;
                match &*t {
                    Transport::Polling(st) => st.clone(),
                    Transport::Ws(_) => return String::new(),
                }
            };
            if let Some(packets) = st.drain().await {
                return packets.join(&SEP.to_string());
            }
            let _ = tokio::time::timeout_at(deadline, st.notify.notified()).await;
            // loop once more to drain whatever arrived (or return empty on timeout)
            if tokio::time::Instant::now() >= deadline {
                if let Some(packets) = st.drain().await {
                    return packets.join(&SEP.to_string());
                }
                return String::new();
            }
        }
    }

    pub async fn touch(&self) {
        *self.last_seen.lock().await = std::time::Instant::now();
    }

    pub async fn idle_secs(&self) -> u64 {
        self.last_seen.lock().await.elapsed().as_secs()
    }

    /// Records a key press-start (`isTapStart: true` on a key tile).
    pub async fn hold_key(&self, button: pulpit_db::ButtonRow) {
        self.held_keys.lock().await.insert(button.id, button);
    }

    /// The matching tap end arrived: nothing left to release.
    pub async fn key_released(&self, id: i64) {
        self.held_keys.lock().await.remove(&id);
    }

    /// Runs the release phase (`isTapStart: false`) for every key the
    /// session left pressed. Key-ups are blocking input work, so they
    /// run on the blocking pool.
    pub async fn release_held_keys(&self) {
        let held: Vec<pulpit_db::ButtonRow> = self
            .held_keys
            .lock()
            .await
            .drain()
            .map(|(_, b)| b)
            .collect();
        for button in held {
            let backend = self.backend.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mut sink = pulpit_actions::NullSink;
                backend.exec(button, false, &mut sink);
            })
            .await;
        }
    }
}

/// All connected devices. Cheap clones behind the scenes: sessions are Arc'd.
#[derive(Default)]
pub struct Hub {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl Hub {
    pub fn new() -> Hub {
        Hub::default()
    }

    pub async fn create(&self, backend: Arc<dyn Backend>, is_pro: bool) -> Arc<Session> {
        let sid = new_sid();
        let session = Arc::new(Session {
            sid: sid.clone(),
            is_pro,
            last_seen: Mutex::new(std::time::Instant::now()),
            backend,
            held_keys: Mutex::new(HashMap::new()),
            transport: Mutex::new(Transport::Polling(Arc::new(PollState {
                queue: Mutex::new(VecDeque::new()),
                notify: Notify::new(),
            }))),
        });
        self.sessions.lock().await.insert(sid, session.clone());
        tracing::info!(sid = %session.sid, room = session.room(), "client connected");
        session
    }

    pub async fn get(&self, sid: &str) -> Option<Arc<Session>> {
        self.sessions.lock().await.get(sid).cloned()
    }

    pub async fn remove(&self, sid: &str) {
        if let Some(session) = self.sessions.lock().await.remove(sid) {
            tracing::info!(sid, "client disconnected");
            // Whatever killed the connection (clean close, error), keys
            // the client left pressed must not stay down.
            session.release_held_keys().await;
        }
    }

    pub async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.sessions.lock().await.is_empty()
    }

    /// Drop sessions silent for longer than Engine.IO allows
    /// (pingInterval + pingTimeout). Returns the number removed.
    pub async fn reap(&self, max_idle_secs: u64) -> usize {
        let mut stale = Vec::new();
        {
            let sessions = self.sessions.lock().await;
            for (sid, s) in sessions.iter() {
                if s.idle_secs().await > max_idle_secs {
                    stale.push(sid.clone());
                }
            }
        }
        let mut sessions = self.sessions.lock().await;
        let mut removed = Vec::new();
        for sid in stale {
            if let Some(session) = sessions.remove(&sid) {
                removed.push(session);
                tracing::info!(sid, "session reaped (ping timeout)");
            }
        }
        drop(sessions);
        // releases run their own blocking execs - never under the map lock
        for session in &removed {
            session.release_held_keys().await;
        }
        removed.len()
    }

    /// Emit a socket.io EVENT packet to every connected session.
    pub async fn broadcast(&self, event: &str, payload: Option<&str>) {
        self.send_to_room("both", event, payload).await;
    }

    /// Emit to a room: "PRO_ROOM", "BASIC_ROOM" or "both". Session Arcs
    /// are snapshotted under the lock and sent after releasing it - one
    /// slow drain must not stall every other hub operation.
    pub async fn send_to_room(&self, room: &str, event: &str, payload: Option<&str>) {
        let packet = event_packet(event, payload);
        let targets: Vec<_> = {
            let sessions = self.sessions.lock().await;
            sessions
                .values()
                .filter(|s| room == "both" || room == s.room())
                .cloned()
                .collect()
        };
        for s in targets {
            s.send(packet.clone()).await;
        }
    }
}

/// `42["event",payload]` socket.io v2 EVENT packet.
pub fn event_packet(event: &str, payload: Option<&str>) -> String {
    match payload {
        Some(p) => format!("42[\"{event}\",{p}]"),
        None => format!("42[\"{event}\"]"),
    }
}

fn new_sid() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..20)
        .map(|_| format!("{:x}", rng.gen::<u8>() & 0x0f))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Backend;

    /// Backend stub: the hub only needs it for held-key release, which
    /// these tests never trigger.
    struct NoBackend;
    impl Backend for NoBackend {
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
    }

    fn backend() -> Arc<NoBackend> {
        Arc::new(NoBackend)
    }

    #[tokio::test]
    async fn polling_session_queues_and_upgrades() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        s.send(event_packet("get_version", Some(r#"{"version":"1.6.0"}"#)))
            .await;

        // queued packets arrive via poll
        let got = tokio::time::timeout(std::time::Duration::from_secs(2), s.poll(100)).await;
        assert!(got.unwrap().starts_with("42[\"get_version\""));

        // upgrade delivers drained packets to the ws channel
        let (tx, mut rx) = mpsc::unbounded_channel();
        s.upgrade_to_ws(tx).await;
        s.send("42[\"ping\"]".into()).await;
        match rx.recv().await {
            Some(WsOut::Packet(p)) => assert_eq!(p, "42[\"ping\"]"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn rooms_are_assigned() {
        let hub = Hub::new();
        let pro = hub.create(backend(), true).await;
        let basic = hub.create(backend(), false).await;
        assert_eq!(pro.room(), "PRO_ROOM");
        assert_eq!(basic.room(), "BASIC_ROOM");
    }

    #[tokio::test]
    async fn poll_times_out_empty() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        let started = std::time::Instant::now();
        let got = s.poll(50).await;
        assert!(got.is_empty());
        assert!(started.elapsed() >= std::time::Duration::from_millis(40));
    }
}
