//! Protocol v2 transport (docs/protocol-v2.md): `/v2/ws` sessions, board
//! sync/delta, the state engine, pairing and the hashed-asset store.
//! Types live in `deckboard-proto`; the legacy protocol stays frozen in
//! `deckboard-legacy` - this crate only adds.

mod assets;
mod boards;
mod devices;
mod hub;
mod service;
mod session;
mod state;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deckboard_proto::{Frame, StatePatch, TYPE_STATE_PATCH};

pub use assets::AssetStore;
pub use devices::{DeviceEntry, DeviceStore, PairError, Pairing};
pub use hub::V2Hub;
pub use service::{router, Auth, V2Config, V2State};
pub use state::StateEngine;

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
            let frame = Frame::push(
                TYPE_STATE_PATCH,
                serde_json::to_value(&StatePatch { changes }).unwrap_or(serde_json::Value::Null),
            );
            hub.broadcast_frame(&frame);
        }
    }
}

/// Kills connections silent longer than the watchdog timeout. Spawn once.
pub async fn run_watchdog(hub: Arc<V2Hub>, timeout: Duration) {
    let mut tick = tokio::time::interval(Duration::from_secs(15));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tick.tick().await;
        let silent: Vec<u64> = hub
            .session_ids()
            .into_iter()
            .filter(|id| {
                hub.silent_ms(*id)
                    .map(|ms| Duration::from_millis(ms) >= timeout)
                    .unwrap_or(false)
            })
            .collect();
        for id in silent {
            tracing::info!(session = id, "v2 watchdog closing silent connection");
            hub.remove(id);
        }
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
