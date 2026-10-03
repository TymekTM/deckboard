//! Protocol v2 transport (docs/protocol-v2.md): `/v2/ws` sessions, board
//! sync/delta, the state engine, pairing and the hashed-asset store.
//! Types live in `pulpit-proto`; the legacy protocol stays frozen in
//! `pulpit-legacy` - this crate only adds.

mod assets;
pub mod boards;
mod devices;
pub mod discovery;
mod hub;
mod service;
mod session;
mod state;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pulpit_proto::{Frame, StatePatch, TYPE_STATE_PATCH};

pub use assets::AssetStore;
pub use boards::build_tile;
pub use devices::{DeviceEntry, DeviceStore, PairError, Pairing};
pub use hub::V2Hub;
pub use service::{router, Auth, V2Config, V2State};
pub use state::{ext_channel, StateEngine};

/// Milliseconds since the Unix epoch; the shared clock for session
/// watchdogs and device timestamps.
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Board generation counter: one bump per committed write batch, starts
/// at 1 so generation 0 always means "no sync seen yet" client-side.
#[derive(Default)]
pub struct Generation(AtomicU64);

impl Generation {
    pub fn starting_at(value: u64) -> Generation {
        Generation(AtomicU64::new(value.max(1)))
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Flushes coalesced state changes as `state.patch` frames. Spawn once
/// with the patch interval from `V2Config`.
pub async fn run_flusher(engine: Arc<StateEngine>, hub: Arc<V2Hub>, interval: Duration) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // interval ticks immediately the first time
    loop {
        tick.tick().await;
        let changes = engine.drain_dirty();
        if !changes.is_empty() {
            let frame = Frame::push_typed(
                TYPE_STATE_PATCH,
                &StatePatch {
                    changes: changes.clone(),
                },
            );
            // A session mid-handshake must not see a patch before its
            // welcome; withhold it and re-arm the channels - the full
            // state.sync queued at welcome covers changes drained in
            // that window, later ones arrive with the next flush.
            if hub.broadcast_patch_to_welcomed(&frame) {
                engine.mark_dirty(changes.iter().map(|c| c.channel.as_str()));
            }
        }
    }
}

/// Drops v2 sessions silent for longer than `3 * ping_interval`. The
/// session pump answers protocol pings, so a healthy client's pongs keep
/// `last_seen` fresh; three missed intervals mean the peer is gone
/// without a TCP close (the queue-bounded hub also tears down peers that
/// stop reading). Spawn once per hub with the ping interval from
/// `V2Config`; sweeps every 30 s.
pub async fn run_reaper(hub: Arc<V2Hub>, ping_interval: Duration) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // interval ticks immediately the first time
    let max_silent = 3 * ping_interval.as_millis() as u64;
    loop {
        tick.tick().await;
        hub.reap_silent(max_silent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_starts_and_grows() {
        let g = Generation::starting_at(0);
        assert_eq!(g.get(), 1); // never 0
        assert_eq!(g.bump(), 2);
        assert_eq!(g.get(), 2);
    }
}
