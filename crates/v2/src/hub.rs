//! Connected v2 clients. One `V2Session` per WebSocket; outbound frames go
//! through an unbounded channel drained by the connection's pump task, so
//! `broadcast_frame` is a cheap synchronous fan-out the editor's write
//! path can call from anywhere.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pulpit_proto::Frame;
use tokio::sync::mpsc;

use crate::devices::DeviceEntry;

#[derive(Default)]
pub struct V2Hub {
    sessions: Mutex<HashMap<u64, Arc<V2Session>>>,
    next_id: AtomicU64,
}

impl V2Hub {
    pub fn new() -> V2Hub {
        V2Hub::default()
    }

    /// Builds the session without hub membership; `attach` adds it once the
    /// hello/auth handshake succeeded. Until then broadcasts must skip the
    /// socket entirely - an unauthenticated peer must not receive pushes.
    pub fn create(&self, out: mpsc::UnboundedSender<super::session::WsOut>) -> Arc<V2Session> {
        Arc::new(V2Session {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            out,
            device: Mutex::new(None),
            last_seen: AtomicU64::new(crate::unix_millis()),
            holds: Mutex::new(HashMap::new()),
        })
    }

    /// Adds an authenticated session to the broadcast fan-out.
    pub fn attach(&self, session: &Arc<V2Session>) {
        self.sessions
            .lock()
            .expect("v2 hub poisoned")
            .insert(session.id, session.clone());
    }

    pub fn remove(&self, id: u64) {
        if let Some(session) = self.sessions.lock().expect("v2 hub poisoned").remove(&id) {
            session.abort_holds();
        }
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().expect("v2 hub poisoned").len()
    }

    /// Serializes once and pushes to every live session. Send errors mean a
    /// dying connection; its own loop notices and cleans up.
    pub fn broadcast_frame(&self, frame: &Frame) {
        let Ok(text) = serde_json::to_string(frame) else {
            return;
        };
        for session in self.sessions.lock().expect("v2 hub poisoned").values() {
            let _ = session.out.send(super::session::WsOut::Text(text.clone()));
        }
    }
}

pub struct V2Session {
    pub id: u64,
    out: mpsc::UnboundedSender<super::session::WsOut>,
    device: Mutex<Option<DeviceEntry>>,
    last_seen: AtomicU64,
    /// Tile id -> running hold-repeat loop; aborted by `press-end`,
    /// disconnect or the cap (loop-internal).
    holds: Mutex<HashMap<i64, tokio::task::JoinHandle<()>>>,
}

impl V2Session {
    pub fn send_frame(&self, frame: &Frame) -> bool {
        match serde_json::to_string(frame) {
            Ok(text) => self.out.send(super::session::WsOut::Text(text)).is_ok(),
            Err(_) => false,
        }
    }

    pub fn set_device(&self, device: DeviceEntry) {
        *self.device.lock().expect("session poisoned") = Some(device);
    }

    pub fn device_name(&self) -> Option<String> {
        self.device
            .lock()
            .expect("session poisoned")
            .as_ref()
            .map(|d| d.name.clone())
    }

    pub fn touch(&self) {
        self.last_seen
            .store(crate::unix_millis(), Ordering::Relaxed);
    }

    /// Milliseconds since the last inbound frame.
    pub fn silent_for_ms(&self) -> u64 {
        crate::unix_millis().saturating_sub(self.last_seen.load(Ordering::Relaxed))
    }

    pub fn insert_hold(&self, tile: i64, handle: tokio::task::JoinHandle<()>) {
        // A second press-start for the same tile replaces the first loop;
        // without the abort it would keep firing (double rate) until the
        // 120 s cap, invisible to press-end which only sees the newest.
        if let Some(previous) = self
            .holds
            .lock()
            .expect("session poisoned")
            .insert(tile, handle)
        {
            previous.abort();
        }
    }

    /// Stops the hold loop for a tile, if one is running.
    pub fn stop_hold(&self, tile: i64) {
        if let Some(handle) = self.holds.lock().expect("session poisoned").remove(&tile) {
            handle.abort();
        }
    }

    pub fn abort_holds(&self) {
        let mut holds = self.holds.lock().expect("session poisoned");
        for (_, handle) in holds.drain() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::WsOut;

    #[test]
    fn broadcast_reaches_sessions_and_remove_cleans_up() {
        let hub = V2Hub::new();
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (tx_b, _rx_b) = mpsc::unbounded_channel();
        let a = hub.create(tx_a);
        // Unauthenticated sessions (created but not attached) are outside
        // the fan-out; only attach brings a socket into the broadcast set.
        hub.broadcast_frame(&Frame::push(
            pulpit_proto::TYPE_BOARD_OPEN,
            serde_json::json!({"board": 0}),
        ));
        assert!(
            rx_a.try_recv().is_err(),
            "pre-auth session must not receive broadcasts"
        );
        hub.attach(&a);
        hub.attach(&hub.create(tx_b));
        assert_eq!(hub.count(), 2);

        hub.broadcast_frame(&Frame::push(
            pulpit_proto::TYPE_BOARD_OPEN,
            serde_json::json!({"board": 3}),
        ));
        let WsOut::Text(text) = rx_a.blocking_recv().unwrap() else {
            panic!("text")
        };
        assert!(text.contains("board.open"));

        hub.remove(a.id);
        assert_eq!(hub.count(), 1);
    }
}
