//! The poller thread (aidev's `spawn_push` shape, adapted to the design
//! §1 cadence rules):
//!
//! - ~3 s while playing, ~20 s paused/idle,
//! - fully idle (no requests) while `consumers() == 0`,
//! - a fast re-poll ~300 ms after any control call (exec signals the
//!   shared [`Wake`]),
//! - a poll at the expected track end (duration - progress + margin),
//! - progress is NOT extrapolated here - clients interpolate between
//!   polls from the `progress` object in the now-playing payload.
//!
//! Each poll publishes one snapshot with exactly the design §4 keys
//! (see [`crate::snapshot`]). The extra `spotify-art-url` key is
//! host-consumed (album-art import is the host lane's job) and must be
//! stripped before the snapshot is forwarded to clients.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::Spotify;
use crate::error::SpotifyError;
use crate::snapshot::{build_snapshot, AuthState};

/// Playing cadence (design: ~3 s).
const PLAYING_POLL: Duration = Duration::from_secs(3);
/// Paused/idle cadence (design: ~20 s).
const PAUSED_POLL: Duration = Duration::from_secs(20);
/// While nobody consumes state: only re-check `consumers` this often,
/// no Spotify requests at all.
const IDLE_POLL: Duration = Duration::from_secs(5);
/// Fast re-poll delay after a control call (design: ~300 ms).
const FAST_REPOLL: Duration = Duration::from_millis(300);
/// Poll slightly after the computed track end so the API has flipped to
/// the next item.
const TRACK_END_MARGIN: Duration = Duration::from_millis(250);
/// NeedsLogin keeps the loop alive (a re-login swaps the handle state)
/// but polls slowly and without any requests.
const NEEDS_LOGIN_POLL: Duration = Duration::from_secs(30);
/// Upper bound on how long a rate-limited loop iteration sleeps before
/// re-checking the pause.
const PAUSE_POLL_CAP: Duration = Duration::from_secs(30);

/// Spawn the poll loop. `consumers` reports how many state consumers
/// exist (connected legacy + v2 clients plus a visible desktop window,
/// passed by the host as a closure or `Arc<AtomicUsize>` reader); zero
/// means stop polling entirely.
pub fn spawn_push(
    spotify: Spotify,
    consumers: Arc<dyn Fn() -> usize + Send + Sync + 'static>,
) -> tokio::sync::mpsc::UnboundedReceiver<Value> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("spotify-push".into())
        .spawn(move || poll_loop(&tx, &spotify, consumers))
        .expect("spawn spotify poller thread");
    rx
}

fn poll_loop(
    tx: &tokio::sync::mpsc::UnboundedSender<Value>,
    spotify: &Spotify,
    consumers: Arc<dyn Fn() -> usize + Send + Sync>,
) {
    let wake = spotify.wake();
    // cross-poll memories for values the API reports as null/absent
    let mut liked: Option<bool> = None;
    let mut last_volume: Option<u32> = None;
    let mut last_track: Option<String> = None;
    loop {
        if consumers() == 0 {
            // nobody renders state: no requests, just re-check (and stay
            // wakeable so a control call during idle still works)
            wake.sleep_until(Instant::now() + IDLE_POLL);
            continue;
        }
        let mut auth = AuthState::Ok;
        let mut player: Option<Value> = None;
        match spotify.player() {
            Ok(found) => player = found,
            Err(SpotifyError::NeedsLogin) => auth = AuthState::NeedsLogin,
            Err(SpotifyError::RateLimited { retry_after_secs }) => {
                // every call fails fast while the pause lasts; wait it
                // out without pushing a stale snapshot
                let wait = Duration::from_secs(retry_after_secs as u64 + 1).min(PAUSE_POLL_CAP);
                wake.sleep_until(Instant::now() + wait);
                continue;
            }
            Err(e) => {
                // transient (offline, 5xx): keep the clients' last values
                tracing::debug!(error = %e, "spotify poll failed");
            }
        }
        if auth == AuthState::Ok {
            // contains-check only on track change / like action (§4)
            let track = player
                .as_ref()
                .and_then(|p| p.pointer("/item/uri"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if track != last_track {
                spotify.mark_liked_dirty();
                last_track = track;
            }
            if spotify.take_liked_dirty() {
                if let Some(uri) = player
                    .as_ref()
                    .and_then(|p| p.pointer("/item/uri"))
                    .and_then(Value::as_str)
                {
                    match spotify.library_contains(uri) {
                        Ok(value) => liked = Some(value),
                        Err(e) => tracing::debug!(error = %e, "spotify contains-check failed"),
                    }
                } else {
                    liked = None;
                }
            }
        }
        let snapshot = build_snapshot(player.as_ref(), liked, auth, last_volume);
        if let Some(volume) = player
            .as_ref()
            .and_then(|p| p.pointer("/device/volume_percent"))
            .and_then(Value::as_u64)
        {
            last_volume = Some(volume.min(100) as u32);
        }
        if tx.send(snapshot).is_err() {
            tracing::debug!("spotify push channel closed, stopping");
            return;
        }
        let next = if auth == AuthState::NeedsLogin {
            NEEDS_LOGIN_POLL
        } else {
            next_deadline(player.as_ref())
        };
        let signaled = wake.sleep_until(Instant::now() + next);
        if signaled {
            // a control call just landed: re-poll shortly after it
            std::thread::sleep(FAST_REPOLL);
        }
    }
}

/// When to poll next: 3 s while playing, sooner near the track end,
/// 20 s otherwise.
fn next_deadline(player: Option<&Value>) -> Duration {
    let Some(player) = player else {
        return PAUSED_POLL;
    };
    let is_playing = player
        .get("is_playing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_playing {
        return PAUSED_POLL;
    }
    if let (Some(progress), Some(duration)) = (
        player.get("progress_ms").and_then(Value::as_u64),
        player.pointer("/item/duration_ms").and_then(Value::as_u64),
    ) {
        let remaining = duration.saturating_sub(progress);
        let until_end = Duration::from_millis(remaining) + TRACK_END_MARGIN;
        if until_end < PLAYING_POLL {
            return until_end;
        }
    }
    PLAYING_POLL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_follows_playback() {
        let playing = serde_json::json!({
            "is_playing": true,
            "progress_ms": 10_000,
            "item": { "duration_ms": 200_000 }
        });
        assert_eq!(next_deadline(Some(&playing)), PLAYING_POLL);

        // near the end: remaining + margin
        let ending = serde_json::json!({
            "is_playing": true,
            "progress_ms": 199_000,
            "item": { "duration_ms": 200_000 }
        });
        assert_eq!(next_deadline(Some(&ending)), Duration::from_millis(1000) + TRACK_END_MARGIN);

        let paused = serde_json::json!({ "is_playing": false, "progress_ms": 5 });
        assert_eq!(next_deadline(Some(&paused)), PAUSED_POLL);
        assert_eq!(next_deadline(None), PAUSED_POLL);
    }

    #[test]
    fn wake_sleep_returns_early_on_signal() {
        let wake = crate::client::Wake::default();
        let waker = wake.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            waker.signal();
        });
        let start = Instant::now();
        let signaled = wake.sleep_until(Instant::now() + Duration::from_secs(10));
        assert!(signaled);
        assert!(start.elapsed() < Duration::from_secs(5));
        // the flag was consumed: a later full sleep runs to its deadline
        // (bounded here so the test cannot hang)
        assert!(!wake.sleep_until(Instant::now() + Duration::from_millis(10)));
    }
}
