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
        assert_eq!(
            next_deadline(Some(&ending)),
            Duration::from_millis(1000) + TRACK_END_MARGIN
        );

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

    // -- the loop itself, over a scripted transport ------------------------

    use crate::config::SpotifyConfig;
    use crate::http::{FakeTransport, HttpRequest, HttpResponse, Transport, TransportError};

    #[derive(Clone)]
    struct SharedFake(Arc<FakeTransport>);

    impl Transport for SharedFake {
        fn send(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, TransportError> {
            self.0.send(req)
        }
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn config(logged_in: bool) -> SpotifyConfig {
        SpotifyConfig {
            client_id: "cid".into(),
            access_token: if logged_in {
                "ACCESS".into()
            } else {
                String::new()
            },
            refresh_token: logged_in.then(|| "REFRESH".into()),
            expires_at: logged_in.then(|| now_unix() + 3600),
            user: None,
            product: None,
        }
    }

    fn handle(logged_in: bool) -> (Spotify, Arc<FakeTransport>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeTransport::new());
        let spotify = Spotify::with_transport(
            config(logged_in),
            dir.path().join("spotify.json"),
            Box::new(SharedFake(fake.clone())),
        );
        (spotify, fake, dir)
    }

    fn json_response(status: u16, body: Value) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn empty(status: u16, headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn playing_track(uri: &str, volume: u64) -> HttpResponse {
        json_response(
            200,
            serde_json::json!({
                "is_playing": true,
                "shuffle_state": false,
                "repeat_state": "off",
                "progress_ms": 1_000,
                "device": { "id": "d", "name": "Desk", "volume_percent": volume, "is_active": true },
                "item": { "uri": uri, "name": "Song", "duration_ms": 200_000, "artists": [] }
            }),
        )
    }

    fn consumers(n: usize) -> Arc<dyn Fn() -> usize + Send + Sync> {
        Arc::new(move || n)
    }

    fn recv_within(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
        limit: Duration,
    ) -> Option<Value> {
        let deadline = Instant::now() + limit;
        loop {
            if let Ok(v) = rx.try_recv() {
                return Some(v);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn urls(fake: &FakeTransport) -> Vec<String> {
        fake.requests().into_iter().map(|r| r.url).collect()
    }

    #[test]
    fn idle_without_consumers_makes_no_requests() {
        let (spotify, fake, _dir) = handle(true);
        let mut rx = spawn_push(spotify, consumers(0));
        assert!(recv_within(&mut rx, Duration::from_millis(150)).is_none());
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn needs_login_publishes_the_login_snapshot_without_requests() {
        let (spotify, fake, _dir) = handle(false);
        let mut rx = spawn_push(spotify, consumers(1));
        let snap = recv_within(&mut rx, Duration::from_secs(5)).expect("a snapshot");
        assert_eq!(snap["spotify-auth"], "needs-login");
        assert_eq!(snap["spotify-playing"], "OFF");
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn a_new_track_triggers_one_contains_check() {
        let (spotify, fake, _dir) = handle(true);
        fake.push(playing_track("spotify:track:abc", 55))
            .push_json(200, serde_json::json!([true]));
        let mut rx = spawn_push(spotify, consumers(2));
        let snap = recv_within(&mut rx, Duration::from_secs(5)).expect("a snapshot");
        assert_eq!(snap["spotify-auth"], "ok");
        assert_eq!(snap["spotify-playing"], "ON");
        assert_eq!(snap["spotify-liked"], "ON");
        assert_eq!(snap["spotify-volume"], "55");
        assert_eq!(snap["spotify-device"], "Desk");
        let urls = urls(&fake);
        assert_eq!(urls.len(), 2, "{urls:?}");
        assert!(urls[0].ends_with("/me/player"));
        assert!(urls[1].contains("contains"), "{}", urls[1]);
    }

    #[test]
    fn nothing_playing_skips_the_contains_check() {
        let (spotify, fake, _dir) = handle(true);
        fake.push(empty(204, &[]));
        let mut rx = spawn_push(spotify, consumers(1));
        let snap = recv_within(&mut rx, Duration::from_secs(5)).expect("a snapshot");
        assert_eq!(snap["spotify-playing"], "OFF");
        assert_eq!(snap["spotify-liked"], "OFF");
        assert_eq!(snap["spotify-volume"], "100");
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn a_control_call_wakes_a_fast_repoll() {
        let (spotify, fake, _dir) = handle(true);
        // nothing playing: the regular cadence would be 20 s
        fake.push(empty(204, &[])).push(empty(204, &[]));
        let wake = spotify.wake();
        let mut rx = spawn_push(spotify, consumers(1));
        recv_within(&mut rx, Duration::from_secs(5)).expect("first snapshot");
        let started = Instant::now();
        wake.signal();
        recv_within(&mut rx, Duration::from_secs(5)).expect("fast re-poll");
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(250), "{waited:?}");
        assert!(waited < PAUSED_POLL, "{waited:?}");
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn rate_limit_withholds_snapshots_until_the_pause_ends() {
        let (spotify, fake, _dir) = handle(true);
        fake.push(empty(429, &[("Retry-After", "0")]))
            .push(empty(204, &[]));
        let mut rx = spawn_push(spotify, consumers(1));
        // the 429 answer is never published as a snapshot
        assert!(recv_within(&mut rx, Duration::from_millis(500)).is_none());
        assert_eq!(fake.requests().len(), 1);
        // Retry-After 0 (+0.5 s pause, +1 s loop wait): the next poll lands
        let snap = recv_within(&mut rx, Duration::from_secs(5)).expect("post-pause snapshot");
        assert_eq!(snap["spotify-playing"], "OFF");
        assert_eq!(fake.requests().len(), 2);
    }

    #[test]
    fn transport_failures_keep_the_loop_alive() {
        let (spotify, fake, _dir) = handle(true);
        // nothing scripted: the first poll fails at the transport level
        let mut rx = spawn_push(spotify, consumers(1));
        let snap = recv_within(&mut rx, Duration::from_secs(5)).expect("a snapshot");
        // an offline machine must not look like it needs a login
        assert_eq!(snap["spotify-auth"], "ok");
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn dropping_the_receiver_stops_the_thread_at_the_next_push() {
        let (spotify, fake, _dir) = handle(true);
        fake.push(empty(204, &[])).push(empty(204, &[]));
        let wake = spotify.wake();
        let mut rx = spawn_push(spotify, consumers(1));
        recv_within(&mut rx, Duration::from_secs(5)).expect("first snapshot");
        drop(rx);
        wake.signal();
        // the loop polls once more, fails to push and exits; later
        // signals cause no further requests
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(fake.requests().len(), 2);
        wake.signal();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(fake.requests().len(), 2);
    }
}
