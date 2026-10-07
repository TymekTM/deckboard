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

    /// Session count without awaiting, for sync callers on plain threads
    /// (the Spotify poller's consumers check); `None` while the session
    /// map is locked.
    pub fn try_len(&self) -> Option<usize> {
        self.sessions.try_lock().ok().map(|sessions| sessions.len())
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

    /// Emit a socket.io EVENT packet to every connected session. Session
    /// Arcs are snapshotted under the lock and sent after releasing it -
    /// one slow drain must not stall every other hub operation. (The
    /// original's pro/basic room split rides inside the payloads; every
    /// frame goes to every client.)
    pub async fn broadcast(&self, event: &str, payload: Option<&str>) {
        let packet = event_packet(event, payload);
        let targets: Vec<_> = {
            let sessions = self.sessions.lock().await;
            sessions.values().cloned().collect()
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

    /// Backend that records key releases from teardown paths.
    #[derive(Default)]
    struct Releases(std::sync::Mutex<Vec<(i64, bool)>>);
    impl Backend for Releases {
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
            button: pulpit_db::ButtonRow,
            is_tap_start: bool,
            _sink: &mut dyn pulpit_actions::EventSink,
        ) {
            self.0.lock().unwrap().push((button.id, is_tap_start));
        }
        fn slider(&self, _button: pulpit_db::ButtonRow, _value: f64) {}
    }

    fn key(id: i64) -> pulpit_db::ButtonRow {
        pulpit_db::ButtonRow {
            id,
            kind: "key".into(),
            ..pulpit_db::ButtonRow::default()
        }
    }

    #[test]
    fn event_packet_shapes() {
        assert_eq!(event_packet("x", None), r#"42["x"]"#);
        assert_eq!(event_packet("x", Some("{}")), r#"42["x",{}]"#);
        assert_eq!(event_packet("x", Some("[1,2]")), r#"42["x",[1,2]]"#);
    }

    #[test]
    fn sids_are_twenty_hex_chars_and_unique() {
        let a = new_sid();
        let b = new_sid();
        assert_eq!(a.len(), 20);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn get_len_and_remove_track_sessions() {
        let hub = Hub::new();
        assert!(hub.is_empty().await);
        assert_eq!(hub.try_len(), Some(0));
        let a = hub.create(backend(), false).await;
        let b = hub.create(backend(), true).await;
        assert_eq!(hub.len().await, 2);
        assert_eq!(hub.try_len(), Some(2));
        assert!(Arc::ptr_eq(&hub.get(&a.sid).await.unwrap(), &a));
        assert!(hub.get("missing").await.is_none());
        hub.remove(&a.sid).await;
        hub.remove(&a.sid).await; // second remove is a no-op
        hub.remove("missing").await;
        assert_eq!(hub.len().await, 1);
        assert!(hub.get(&b.sid).await.is_some());
    }

    #[tokio::test]
    async fn broadcast_reaches_polling_and_ws_sessions() {
        let hub = Hub::new();
        let polling = hub.create(backend(), false).await;
        let ws = hub.create(backend(), true).await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        ws.upgrade_to_ws(tx).await;
        hub.broadcast("change_board", Some(r#"{"boardId":3}"#)).await;
        let expected = r#"42["change_board",{"boardId":3}]"#;
        assert_eq!(polling.poll(10).await, expected);
        match rx.recv().await {
            Some(WsOut::Packet(p)) => assert_eq!(p, expected),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_joins_queued_packets_with_the_record_separator() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        s.send("40".into()).await;
        s.send("3".into()).await;
        s.send(r#"42["a"]"#.into()).await;
        assert_eq!(s.poll(10).await, "40\u{1e}3\u{1e}42[\"a\"]");
        // drained: the next poll is empty
        assert_eq!(s.poll(1).await, "");
    }

    #[tokio::test]
    async fn poll_wakes_as_soon_as_a_packet_arrives() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        let sender = s.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            sender.send("3".into()).await;
        });
        let started = std::time::Instant::now();
        assert_eq!(s.poll(5_000).await, "3");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[tokio::test]
    async fn poll_on_a_ws_session_returns_immediately() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        s.upgrade_to_ws(tx).await;
        let started = std::time::Instant::now();
        assert_eq!(s.poll(5_000).await, "");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn upgrade_flushes_the_queue_in_order() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        s.send("a".into()).await;
        s.send("b".into()).await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        s.upgrade_to_ws(tx).await;
        for want in ["a", "b"] {
            match rx.recv().await {
                Some(WsOut::Packet(p)) => assert_eq!(p, want),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn send_to_a_closed_ws_channel_does_not_panic() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        let (tx, rx) = mpsc::unbounded_channel();
        s.upgrade_to_ws(tx).await;
        drop(rx);
        s.send("x".into()).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_releases_every_held_key_once() {
        let rec = Arc::new(Releases::default());
        let hub = Hub::new();
        let s = hub.create(rec.clone(), false).await;
        s.hold_key(key(1)).await;
        s.hold_key(key(2)).await;
        s.hold_key(key(1)).await; // re-press replaces, never doubles
        s.key_released(2).await;
        hub.remove(&s.sid).await;
        assert_eq!(*rec.0.lock().unwrap(), vec![(1, false)]);
        // releasing again (already drained) runs nothing
        s.release_held_keys().await;
        assert_eq!(rec.0.lock().unwrap().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reap_drops_only_idle_sessions_and_releases_their_keys() {
        let rec = Arc::new(Releases::default());
        let hub = Hub::new();
        let stale = hub.create(rec.clone(), false).await;
        let fresh = hub.create(rec.clone(), false).await;
        stale.hold_key(key(5)).await;
        *stale.last_seen.lock().await =
            std::time::Instant::now() - std::time::Duration::from_secs(120);
        assert_eq!(hub.reap(85).await, 1);
        assert!(hub.get(&stale.sid).await.is_none());
        assert!(hub.get(&fresh.sid).await.is_some());
        assert_eq!(*rec.0.lock().unwrap(), vec![(5, false)]);
        // nothing else is idle
        assert_eq!(hub.reap(85).await, 0);
    }

    #[tokio::test]
    async fn touch_resets_the_idle_clock() {
        let hub = Hub::new();
        let s = hub.create(backend(), false).await;
        *s.last_seen.lock().await = std::time::Instant::now() - std::time::Duration::from_secs(50);
        assert!(s.idle_secs().await >= 50);
        s.touch().await;
        assert_eq!(s.idle_secs().await, 0);
    }

    #[tokio::test]
    async fn try_len_reports_none_while_the_map_is_locked() {
        let hub = Hub::new();
        let guard = hub.sessions.lock().await;
        assert_eq!(hub.try_len(), None);
        drop(guard);
        assert_eq!(hub.try_len(), Some(0));
    }
}
