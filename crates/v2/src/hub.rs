//! Connected v2 clients. One `V2Session` per WebSocket; outbound frames go
//! through a bounded channel drained by the connection's pump task, so
//! `broadcast_frame` is a cheap synchronous fan-out the editor's write
//! path can call from anywhere. A peer whose queue overflows (it stopped
//! reading) is closed by the hub, and the hosts' reaper drops sessions
//! silent past the ping timeout - an unbounded queue on a stalled peer
//! would otherwise grow for the process lifetime.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pulpit_db::ButtonRow;
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
    pub fn create(&self, out: mpsc::Sender<super::session::WsOut>) -> Arc<V2Session> {
        let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
        Arc::new(V2Session {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            out,
            device: Mutex::new(None),
            last_seen: AtomicU64::new(crate::unix_millis()),
            holds: Mutex::new(HashMap::new()),
            pump: Mutex::new(None),
            cancel: cancel_tx,
            held_keys: Mutex::new(HashMap::new()),
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
            session.shutdown();
        }
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().expect("v2 hub poisoned").len()
    }

    /// Serializes once and pushes to every live session. A full or closed
    /// queue means the peer stopped reading: that session is torn down
    /// via the dead list (removal must not happen under the map lock).
    pub fn broadcast_frame(&self, frame: &Frame) {
        let Ok(text) = serde_json::to_string(frame) else {
            return;
        };
        let mut dead = Vec::new();
        {
            let sessions = self.sessions.lock().expect("v2 hub poisoned");
            for session in sessions.values() {
                if session
                    .try_send(super::session::WsOut::Text(text.clone()))
                    .is_err()
                {
                    dead.push(session.id);
                }
            }
        }
        for id in dead {
            tracing::info!(session = id, "v2 session closed: outbound queue full");
            self.remove(id);
        }
    }

    /// Drops sessions whose last inbound frame (text or pong) is older
    /// than `max_silent_ms`. The pump's pings keep a healthy client's
    /// pong arriving every `ping_interval`, so silence past the grace
    /// window means the peer is gone without a TCP close.
    pub fn reap_silent(&self, max_silent_ms: u64) {
        let mut dead = Vec::new();
        {
            let sessions = self.sessions.lock().expect("v2 hub poisoned");
            for session in sessions.values() {
                if session.silent_for_ms() > max_silent_ms {
                    dead.push(session.id);
                }
            }
        }
        for id in dead {
            tracing::info!(session = id, "v2 session reaped: silent past ping timeout");
            self.remove(id);
        }
    }

    /// The exit path: one `server.shutdown` goodbye to every attached
    /// session, then a WS close. Each pump drains its queue in order, so
    /// the frame is on the wire before the close - clients that understand
    /// it know the exit is deliberate, not a network drop. A wedged peer
    /// (full queue) misses the goodbye and just gets torn down at exit.
    /// Every session is also cancelled so its task ends and releases any
    /// keys it holds - the process dying would otherwise leave them down.
    pub fn shutdown(&self) {
        self.broadcast_frame(&Frame::bare(pulpit_proto::TYPE_SERVER_SHUTDOWN));
        let mut dead = Vec::new();
        {
            let sessions = self.sessions.lock().expect("v2 hub poisoned");
            for session in sessions.values() {
                if session.try_send(super::session::WsOut::Close).is_err() {
                    dead.push(session.id);
                }
                session.cancel_session();
            }
        }
        for id in dead {
            self.remove(id);
        }
    }
}

pub struct V2Session {
    pub id: u64,
    out: mpsc::Sender<super::session::WsOut>,
    device: Mutex<Option<DeviceEntry>>,
    last_seen: AtomicU64,
    /// Tile id -> running hold-repeat loop; aborted by `press-end`,
    /// disconnect or the cap (loop-internal).
    holds: Mutex<HashMap<i64, tokio::task::JoinHandle<()>>>,
    /// The connection's outbound pump; owned here so hub teardown can
    /// abort a pump stuck on a send that will never complete.
    pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Hub-side teardown signal (queue overflow, silence watchdog,
    /// server exit): the session task selects on it so every teardown
    /// path ends the task and runs its cleanup - a cancelled read loop
    /// would otherwise linger as a zombie.
    cancel: tokio::sync::watch::Sender<bool>,
    /// Tiles with an un-ended key press-start. Only `key` tiles hold
    /// anything down (tap-start is a no-op for every other kind), and
    /// the release-phase exec must run on every teardown path
    /// (docs/protocol-v2.md §6).
    held_keys: Mutex<HashMap<i64, ButtonRow>>,
}

impl V2Session {
    pub fn send_frame(&self, frame: &Frame) -> bool {
        match serde_json::to_string(frame) {
            Ok(text) => self.try_send(super::session::WsOut::Text(text)).is_ok(),
            Err(_) => false,
        }
    }

    fn try_send(
        &self,
        msg: super::session::WsOut,
    ) -> Result<(), mpsc::error::TrySendError<super::session::WsOut>> {
        self.out.try_send(msg)
    }

    pub fn set_pump(&self, handle: tokio::task::JoinHandle<()>) {
        *self.pump.lock().expect("session poisoned") = Some(handle);
    }

    fn take_pump(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.pump.lock().expect("session poisoned").take()
    }

    /// Waits for the pump to flush queued frames and finish within `grace`.
    pub async fn finish_pump(&self, grace: Duration) {
        if let Some(handle) = self.take_pump() {
            let _ = tokio::time::timeout(grace, handle).await;
        }
    }

    pub fn abort_pump(&self) {
        if let Some(handle) = self.take_pump() {
            handle.abort();
        }
    }

    /// Teardown from the hub side (queue overflow, silence watchdog):
    /// stop hold loops, ask the pump to close (a full queue means the
    /// socket is wedged anyway), abort the pump and cancel the session
    /// task so its tail cleanup always runs.
    pub fn shutdown(&self) {
        self.abort_holds();
        let _ = self.try_send(super::session::WsOut::Close);
        self.abort_pump();
        self.cancel_session();
    }

    /// Fires the session task's cancellation signal. Safe to call from
    /// anywhere, any number of times.
    pub fn cancel_session(&self) {
        self.cancel.send_replace(true);
    }

    /// A receiver that resolves once the session is cancelled. Fresh
    /// subscriptions see a past cancellation immediately.
    pub fn cancelled(&self) -> tokio::sync::watch::Receiver<bool> {
        self.cancel.subscribe()
    }

    /// Records a key press-start: the tile's release phase must run on
    /// every teardown path if no `press-end` arrives.
    pub fn key_pressed(&self, tile: i64, button: ButtonRow) {
        self.held_keys
            .lock()
            .expect("session poisoned")
            .insert(tile, button);
    }

    /// The matching `press-end` arrived: nothing left to release.
    pub fn key_released(&self, tile: i64) {
        self.held_keys
            .lock()
            .expect("session poisoned")
            .remove(&tile);
    }

    /// Takes the tiles with un-ended key press-starts for release.
    pub fn take_held_keys(&self) -> Vec<ButtonRow> {
        self.held_keys
            .lock()
            .expect("session poisoned")
            .drain()
            .map(|(_, button)| button)
            .collect()
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

    /// Test hook: age the liveness stamp as if the peer went quiet.
    #[cfg(test)]
    pub fn age_last_seen_by(&self, ms: u64) {
        self.last_seen
            .store(crate::unix_millis().saturating_sub(ms), Ordering::Relaxed);
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
    use tokio::sync::mpsc;

    fn frame() -> Frame {
        Frame::push(
            pulpit_proto::TYPE_BOARD_OPEN,
            serde_json::json!({"board": 0}),
        )
    }

    #[test]
    fn broadcast_reaches_sessions_and_remove_cleans_up() {
        let hub = V2Hub::new();
        let (tx_a, mut rx_a) = mpsc::channel(4);
        let (tx_b, _rx_b) = mpsc::channel(4);
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

    #[test]
    fn overflowing_queue_closes_the_session() {
        let hub = V2Hub::new();
        // receiver never drained: a peer that stopped reading
        let (tx, _rx) = mpsc::channel(4);
        let session = hub.create(tx);
        hub.attach(&session);
        assert_eq!(hub.count(), 1);
        // broadcast well past any plausible queue bound; the hub must
        // remove the wedged session instead of queueing forever
        for _ in 0..1024 {
            hub.broadcast_frame(&frame());
            if hub.count() == 0 {
                break;
            }
        }
        assert_eq!(hub.count(), 0, "wedged session must be closed");
    }

    #[test]
    fn reap_silent_drops_stale_and_keeps_fresh_sessions() {
        let hub = V2Hub::new();
        let (tx_stale, _rx_stale) = mpsc::channel(4);
        let (tx_fresh, _rx_fresh) = mpsc::channel(4);
        let stale = hub.create(tx_stale);
        let fresh = hub.create(tx_fresh);
        hub.attach(&stale);
        hub.attach(&fresh);
        stale.age_last_seen_by(10 * 60_000);
        hub.reap_silent(180_000);
        assert_eq!(hub.count(), 1, "fresh session survives the reap");
        hub.broadcast_frame(&frame());
        assert_eq!(hub.count(), 1);
    }

    #[test]
    fn shutdown_sends_frame_then_close() {
        let hub = V2Hub::new();
        let (tx, mut rx) = mpsc::channel(8);
        let a = hub.create(tx);
        hub.attach(&a);

        hub.shutdown();

        let WsOut::Text(text) = rx.blocking_recv().unwrap() else {
            panic!("goodbye frame first")
        };
        assert!(text.contains("server.shutdown"));
        assert!(matches!(rx.blocking_recv().unwrap(), WsOut::Close));
    }

    #[test]
    fn teardown_paths_cancel_the_session_task() {
        let hub = V2Hub::new();
        // overflow path
        let (tx_full, _rx_full) = mpsc::channel(4);
        let full = hub.create(tx_full);
        hub.attach(&full);
        let cancelled = full.cancelled();
        assert!(!*cancelled.borrow(), "fresh session is not cancelled");
        hub.remove(full.id);
        assert!(
            *cancelled.borrow(),
            "hub-side removal must cancel the session task"
        );

        // reaper path
        let (tx_stale, _rx_stale) = mpsc::channel(4);
        let stale = hub.create(tx_stale);
        hub.attach(&stale);
        let cancelled = stale.cancelled();
        stale.age_last_seen_by(10 * 60_000);
        hub.reap_silent(180_000);
        assert!(
            *cancelled.borrow(),
            "the reaper must cancel the session task"
        );

        // server exit path
        let (tx_exit, _rx_exit) = mpsc::channel(8);
        let exit = hub.create(tx_exit);
        hub.attach(&exit);
        let cancelled = exit.cancelled();
        hub.shutdown();
        assert!(
            *cancelled.borrow(),
            "server exit must cancel the session task"
        );
    }
}
