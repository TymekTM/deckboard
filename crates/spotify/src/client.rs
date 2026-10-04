//! The Spotify handle: one cheap-to-clone `Arc` owning the config file,
//! the HTTP transport and the live token state. All requests are
//! serialized through the handle; a global 429 pause gates every call;
//! token refresh is lazy and rotation is persisted.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::auth::{self, now_unix};
use crate::config::SpotifyConfig;
use crate::error::{Result, SpotifyError};
use crate::http::{HttpRequest, HttpResponse, Method, Transport};
use crate::plan::Plan;

const API: &str = "https://api.spotify.com/v1";

/// Refresh the token when the access token expires within this window.
const EXPIRY_MARGIN: i64 = 60;

/// Cap a 429 pause so a hostile/broken `Retry-After` cannot brick the
/// handle for hours.
const MAX_PAUSE: Duration = Duration::from_secs(300);

/// Extra quiet time after `Retry-After` (design §1: +0.5 s).
const PAUSE_SLACK: Duration = Duration::from_millis(500);

/// Live token state, mirrored into `spotify.json` on every change.
#[derive(Clone)]
struct Auth {
    client_id: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    needs_login: bool,
    user: Option<String>,
    product: Option<String>,
}

impl From<&SpotifyConfig> for Auth {
    fn from(config: &SpotifyConfig) -> Auth {
        Auth {
            needs_login: !config.has_login(),
            client_id: config.client_id.clone(),
            access_token: config.access_token.clone(),
            refresh_token: config.refresh_token.clone(),
            expires_at: config.expires_at,
            user: config.user.clone(),
            product: config.product.clone(),
        }
    }
}

impl Auth {
    fn to_config(&self) -> SpotifyConfig {
        SpotifyConfig {
            client_id: self.client_id.clone(),
            access_token: if self.needs_login {
                String::new()
            } else {
                self.access_token.clone()
            },
            refresh_token: if self.needs_login {
                None
            } else {
                self.refresh_token.clone()
            },
            expires_at: self.expires_at,
            user: self.user.clone(),
            product: self.product.clone(),
        }
    }
}

/// Which `/v1/me/library...` variant worked last (Feb 2026: the
/// `/v1/me/tracks` endpoints were replaced by `/v1/me/library`; on
/// 400/404 we fall back and remember).
#[derive(Debug, Clone, Copy, PartialEq)]
enum LibraryEndpoint {
    New,
    Legacy,
}

/// Fast re-poll signal between exec (control calls) and the poller
/// thread: waking makes the poller re-read state ~300 ms later.
#[derive(Clone, Default)]
pub(crate) struct Wake {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl Wake {
    pub(crate) fn signal(&self) {
        let (flag, cv) = &*self.inner;
        *flag.lock().unwrap() = true;
        cv.notify_all();
    }

    /// Sleep until `deadline` or a signal. Returns true when signaled.
    pub(crate) fn sleep_until(&self, deadline: Instant) -> bool {
        let (flag, cv) = &*self.inner;
        let mut signaled = flag.lock().unwrap();
        loop {
            if *signaled {
                *signaled = false;
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, _timed_out) = cv
                .wait_timeout(signaled, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            signaled = guard;
        }
    }
}

struct Inner {
    path: PathBuf,
    transport: Box<dyn Transport>,
    auth: Mutex<Auth>,
    paused_until: Mutex<Instant>,
    library: Mutex<Option<LibraryEndpoint>>,
    /// Volume before the last mute, for mute-restore.
    last_volume: Mutex<Option<u32>>,
    /// Re-check the liked state on the next poll (track changed / Like
    /// pressed); keeps contains-calls off the hot path (design §4).
    liked_dirty: AtomicBool,
    wake: Wake,
}

/// The integration handle. Clone is cheap (`Arc`); requests serialize
/// through the shared state, so clones (backend + poller + host) never
/// race tokens or the rate-limit pause.
#[derive(Clone)]
pub struct Spotify {
    inner: Arc<Inner>,
}

impl Spotify {
    /// Production handle over the real transport.
    pub fn new(config: SpotifyConfig, path: PathBuf) -> Spotify {
        Spotify::with_transport(config, path, Box::new(crate::http::UreqTransport::new()))
    }

    /// Test handle over a scripted transport (no network).
    pub fn with_transport(
        config: SpotifyConfig,
        path: PathBuf,
        transport: Box<dyn Transport>,
    ) -> Spotify {
        Spotify {
            inner: Arc::new(Inner {
                path,
                transport,
                auth: Mutex::new(Auth::from(&config)),
                paused_until: Mutex::new(Instant::now()),
                library: Mutex::new(None),
                last_volume: Mutex::new(None),
                liked_dirty: AtomicBool::new(false),
                wake: Wake::default(),
            }),
        }
    }

    /// Wake clone for the poller (shares the fast re-poll signal).
    pub(crate) fn wake(&self) -> Wake {
        self.inner.wake.clone()
    }

    /// The persisted config as it stands now (settings UI).
    pub fn config(&self) -> SpotifyConfig {
        self.inner.auth.lock().unwrap().to_config()
    }

    /// True when the saved login is gone/expired: every call now fails
    /// fast with [`SpotifyError::NeedsLogin`] until the host re-logs in.
    pub fn needs_login(&self) -> bool {
        self.inner.auth.lock().unwrap().needs_login
    }

    /// Account facts from `/v1/me` (cached from login/refresh time).
    pub fn user(&self) -> Option<String> {
        self.inner.auth.lock().unwrap().user.clone()
    }

    pub fn product(&self) -> Option<String> {
        self.inner.auth.lock().unwrap().product.clone()
    }

    // ------------------------------------------------------------- requests

    /// Send one authorized request: pause gate, lazy token refresh, one
    /// 401-refresh-retry, 429 global pause. Statuses are the caller's
    /// business (204 is data here, not an error).
    fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse> {
        {
            let paused = self.inner.paused_until.lock().unwrap();
            let remaining = paused.saturating_duration_since(Instant::now());
            if remaining > Duration::ZERO {
                return Err(SpotifyError::RateLimited {
                    retry_after_secs: remaining.as_secs() as u32,
                });
            }
        }
        let token = self.usable_token()?;
        let resp = self.send(method, url, body, &token)?;
        if resp.status == 429 {
            return Err(self.apply_pause(&resp));
        }
        if resp.status == 401 {
            // expired between calls: silent refresh, then exactly one retry
            self.force_refresh()?;
            let token = self.usable_token()?;
            let resp = self.send(method, url, body, &token)?;
            if resp.status == 429 {
                return Err(self.apply_pause(&resp));
            }
            return Ok(resp);
        }
        Ok(resp)
    }

    fn send(&self, method: Method, url: &str, body: Option<&Value>, token: &str) -> Result<HttpResponse> {
        let req = HttpRequest {
            method,
            url: url.to_string(),
            headers: vec![
                ("authorization".into(), format!("Bearer {token}")),
                (
                    "content-type".into(),
                    "application/json".into(),
                ),
            ],
            body: match body {
                Some(v) => Some(serde_json::to_vec(v).map_err(|e| {
                    SpotifyError::Io(std::io::Error::other(format!("request body: {e}")))
                })?),
                None => None,
            },
        };
        self.inner
            .transport
            .send(&req)
            .map_err(|_| SpotifyError::Network("api request"))
    }

    /// Read a fresh-enough access token, refreshing when inside the
    /// 60 s expiry margin (design §1).
    fn usable_token(&self) -> Result<String> {
        let mut auth = self.inner.auth.lock().unwrap();
        if auth.needs_login {
            return Err(SpotifyError::NeedsLogin);
        }
        let expires_soon = auth
            .expires_at
            .is_none_or(|at| now_unix() >= at - EXPIRY_MARGIN);
        if expires_soon {
            self.refresh_locked(&mut auth)?;
        }
        if auth.access_token.is_empty() {
            return Err(SpotifyError::NeedsLogin);
        }
        Ok(auth.access_token.clone())
    }

    /// Unconditional refresh (used by the 401 retry path).
    fn force_refresh(&self) -> Result<()> {
        let mut auth = self.inner.auth.lock().unwrap();
        if auth.needs_login {
            return Err(SpotifyError::NeedsLogin);
        }
        self.refresh_locked(&mut auth)
    }

    /// Refresh under the auth lock. A rotating answer (new refresh
    /// token) is stored and persisted immediately - Spotify reportedly
    /// invalidates the old token. `invalid_grant` flips the handle into
    /// NeedsLogin and clears the tokens on disk; it is never retried.
    fn refresh_locked(&self, auth: &mut Auth) -> Result<()> {
        let Some(refresh_token) = auth.refresh_token.clone() else {
            self.enter_needs_login(auth);
            return Err(SpotifyError::NeedsLogin);
        };
        match auth::refresh_tokens(self.inner.transport.as_ref(), &auth.client_id, &refresh_token) {
            Ok(tokens) => {
                auth.access_token = tokens.access_token;
                auth.expires_at = Some(now_unix() + tokens.expires_in as i64);
                if tokens.refresh_token.is_some() {
                    auth.refresh_token = tokens.refresh_token;
                }
                self.persist(auth);
                Ok(())
            }
            Err(SpotifyError::NeedsLogin) => {
                self.enter_needs_login(auth);
                Err(SpotifyError::NeedsLogin)
            }
            Err(e) => Err(e),
        }
    }

    fn enter_needs_login(&self, auth: &mut Auth) {
        auth.needs_login = true;
        auth.access_token.clear();
        auth.refresh_token = None;
        auth.expires_at = None;
        self.persist(auth);
        tracing::warn!("spotify login expired (invalid_grant) - needs re-login from settings");
    }

    fn persist(&self, auth: &Auth) {
        if let Err(e) = auth.to_config().save(&self.inner.path) {
            tracing::warn!(error = %e, "could not persist spotify.json");
        }
    }

    /// Record a 429: pause everything for `Retry-After + 0.5 s`.
    fn apply_pause(&self, resp: &HttpResponse) -> SpotifyError {
        let retry_after = resp
            .header("retry-after")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1)
            .min(MAX_PAUSE.as_secs());
        let until = Instant::now() + Duration::from_secs(retry_after) + PAUSE_SLACK;
        *self.inner.paused_until.lock().unwrap() = until;
        tracing::warn!(retry_after_secs = retry_after, "spotify rate limit - pausing all calls");
        SpotifyError::RateLimited {
            retry_after_secs: retry_after as u32,
        }
    }

    /// Map a non-2xx answer to the typed errors (design §3).
    fn api_error(resp: &HttpResponse) -> SpotifyError {
        let parsed = resp.json().unwrap_or_default();
        let reason = parsed.pointer("/error/reason").and_then(Value::as_str).unwrap_or("");
        let message = parsed.pointer("/error/message").and_then(Value::as_str).unwrap_or("");
        match (resp.status, reason) {
            (403, "PREMIUM_REQUIRED") => SpotifyError::PremiumRequired,
            (404, "NO_ACTIVE_DEVICE") => SpotifyError::NoActiveDevice,
            _ => SpotifyError::Api(if message.is_empty() {
                format!("HTTP {}", resp.status)
            } else {
                format!("HTTP {}: {message}", resp.status)
            }),
        }
    }

    /// Fire-and-forget control call: success is any 2xx (204 typical).
    fn control(&self, method: Method, url: &str, body: Option<&Value>) -> Result<()> {
        let resp = self.request(method, url, body)?;
        if (200..300).contains(&resp.status) {
            Ok(())
        } else {
            Err(Self::api_error(&resp))
        }
    }

    // ------------------------------------------------------------- reads

    /// Current playback (`GET /v1/me/player`): `None` on 204 (nothing
    /// playing anywhere). This is the poller's read.
    pub fn player(&self) -> Result<Option<Value>> {
        let resp = self.request(Method::Get, &format!("{API}/me/player"), None)?;
        match resp.status {
            204 => Ok(None),
            200 => Ok(resp.json()),
            _ => Err(Self::api_error(&resp)),
        }
    }

    /// One device entry for the editor picker / transfer matching.
    fn devices_internal(&self) -> Result<Vec<Device>> {
        let resp = self.request(Method::Get, &format!("{API}/me/player/devices"), None)?;
        if resp.status == 401 {
            return Err(SpotifyError::NeedsLogin);
        }
        if !(200..300).contains(&resp.status) {
            return Err(Self::api_error(&resp));
        }
        let parsed = resp.json().unwrap_or_default();
        let Some(list) = parsed.pointer("/devices").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        Ok(list
            .iter()
            .map(|d| Device {
                id: d.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                name: d.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                is_active: d.get("is_active").and_then(Value::as_bool).unwrap_or(false),
            })
            .filter(|d| !d.id.is_empty())
            .collect())
    }

    /// Active Spotify Connect devices (editor picker).
    pub fn devices(&self) -> Result<Vec<Device>> {
        self.devices_internal()
    }

    /// The user's playlists, paged (editor picker). The 60 s caching the
    /// design asks for lives in the host command layer.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        // hard cap: 10 pages of 50 - enough for real accounts, bounded
        // against a misbehaving API
        for _ in 0..10 {
            let resp = self.request(
                Method::Get,
                &format!("{API}/me/playlists?limit=50&offset={offset}"),
                None,
            )?;
            if resp.status == 401 {
                return Err(SpotifyError::NeedsLogin);
            }
            if !(200..300).contains(&resp.status) {
                return Err(Self::api_error(&resp));
            }
            let parsed = resp.json().unwrap_or_default();
            let Some(items) = parsed.pointer("/items").and_then(Value::as_array) else {
                break;
            };
            if items.is_empty() {
                break;
            }
            for item in items {
                let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                if id.is_empty() {
                    continue;
                }
                out.push(Playlist {
                    id: id.to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    uri: item
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            let more = parsed.pointer("/next").and_then(Value::as_str).is_some();
            if !more {
                break;
            }
            offset += 50;
        }
        Ok(out)
    }

    // ------------------------------------------------------------ library

    /// `spotify:track:ID` -> `ID` (the legacy `/me/tracks` endpoints take
    /// bare ids).
    fn track_id(uri: &str) -> &str {
        uri.rsplit(':').next().unwrap_or(uri)
    }

    /// Save (`save = true`) or unsave the track. Tries the memoized
    /// endpoint variant first, then the other one on 400/404 (Feb 2026
    /// endpoint migration), remembering which worked.
    fn library_mutate(&self, save: bool, uri: &str) -> Result<()> {
        let memo = *self.inner.library.lock().unwrap();
        let order = match memo {
            Some(LibraryEndpoint::Legacy) => [LibraryEndpoint::Legacy, LibraryEndpoint::New],
            _ => [LibraryEndpoint::New, LibraryEndpoint::Legacy],
        };
        let method = if save { Method::Put } else { Method::Delete };
        let mut last: Option<SpotifyError> = None;
        for variant in order {
            let resp = match variant {
                LibraryEndpoint::New => self.request(
                    method,
                    &format!("{API}/me/library"),
                    Some(&json!({ "uris": [uri] })),
                )?,
                LibraryEndpoint::Legacy => self.request(
                    method,
                    &format!("{API}/me/tracks?ids={}", Self::track_id(uri)),
                    None,
                )?,
            };
            if (200..300).contains(&resp.status) {
                *self.inner.library.lock().unwrap() = Some(variant);
                return Ok(());
            }
            // 404/400: the variant does not exist on this account/API
            // build - fall back to the other one
            if resp.status == 400 || resp.status == 404 {
                last = Some(Self::api_error(&resp));
                continue;
            }
            return Err(Self::api_error(&resp));
        }
        Err(last.unwrap_or(SpotifyError::Api("library endpoints exhausted".into())))
    }

    /// Is the track in the user's library? Same new-then-legacy fallback.
    pub(crate) fn library_contains(&self, uri: &str) -> Result<bool> {
        let memo = *self.inner.library.lock().unwrap();
        let order = match memo {
            Some(LibraryEndpoint::Legacy) => [LibraryEndpoint::Legacy, LibraryEndpoint::New],
            _ => [LibraryEndpoint::New, LibraryEndpoint::Legacy],
        };
        let mut last: Option<SpotifyError> = None;
        for variant in order {
            let url = match variant {
                LibraryEndpoint::New => {
                    format!("{API}/me/library/contains?uris={}", crate::pkce::urlencode(uri))
                }
                LibraryEndpoint::Legacy => {
                    format!("{API}/me/tracks/contains?ids={}", Self::track_id(uri))
                }
            };
            let resp = self.request(Method::Get, &url, None)?;
            if resp.status == 200 {
                *self.inner.library.lock().unwrap() = Some(variant);
                if let Some(list) = resp.json().and_then(|v| v.as_array().cloned()) {
                    return Ok(list.first().and_then(Value::as_bool).unwrap_or(false));
                }
            }
            if resp.status == 400 || resp.status == 404 {
                last = Some(Self::api_error(&resp));
                continue;
            }
            return Err(Self::api_error(&resp));
        }
        Err(last.unwrap_or(SpotifyError::Api("library contains endpoints exhausted".into())))
    }

    pub(crate) fn mark_liked_dirty(&self) {
        self.inner.liked_dirty.store(true, Ordering::Relaxed);
    }

    pub(crate) fn take_liked_dirty(&self) -> bool {
        self.inner.liked_dirty.swap(false, Ordering::Relaxed)
    }

    // -------------------------------------------------------------- exec

    /// Execute one tile action: parse (pure) then apply over HTTP. After
    /// any control call the poller is woken for a fast re-poll.
    pub fn exec(&self, kind: &str, command: &str, slider_value: Option<f64>) -> Result<()> {
        let what = crate::plan::plan(kind, command, slider_value)?;
        self.apply(&what)
    }

    fn apply(&self, what: &Plan) -> Result<()> {
        match what {
            Plan::Noop => {
                tracing::debug!("spotify display tile pressed (no-op)");
                Ok(())
            }
            Plan::PlayPause => {
                let playing = self
                    .player()?
                    .as_ref()
                    .and_then(|p| p.get("is_playing"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let outcome = if playing {
                    self.control(Method::Put, &format!("{API}/me/player/pause"), None)
                } else {
                    self.control(Method::Put, &format!("{API}/me/player/play"), Some(&json!({})))
                };
                self.after_control(outcome)
            }
            Plan::Next => self.after_control(self.control(Method::Post, &format!("{API}/me/player/next"), None)),
            Plan::Previous => {
                self.after_control(self.control(Method::Post, &format!("{API}/me/player/previous"), None))
            }
            Plan::VolumeUp => self.apply_volume_step(10),
            Plan::VolumeDown => self.apply_volume_step(-10),
            Plan::VolumeMute => self.apply_volume_mute(),
            Plan::Shuffle => {
                let on = self
                    .player()?
                    .as_ref()
                    .and_then(|p| p.get("shuffle_state"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let outcome = self.control(
                    Method::Put,
                    &format!("{API}/me/player/shuffle?state={}", !on),
                    None,
                );
                self.after_control(outcome)
            }
            Plan::Repeat => {
                let player = self.player()?;
                let current = player
                    .as_ref()
                    .and_then(|p| p.get("repeat_state"))
                    .and_then(Value::as_str)
                    .unwrap_or("off")
                    .to_string();
                let next = match current.as_str() {
                    "off" => "context",
                    "context" => "track",
                    _ => "off",
                };
                let outcome = self.control(
                    Method::Put,
                    &format!("{API}/me/player/repeat?state={next}"),
                    None,
                );
                self.after_control(outcome)
            }
            Plan::Like => {
                let uri = self.current_track_uri()?;
                let liked = self.library_contains(&uri).unwrap_or(false);
                let outcome = self.library_mutate(!liked, &uri);
                // the poller re-checks on the fast re-poll so the heart
                // flips even if the optimistic guess was wrong
                self.mark_liked_dirty();
                self.after_control(outcome)
            }
            Plan::AddToPlaylist(target) => {
                let uri = self.current_track_uri()?;
                let id = target
                    .rsplit(':')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(target.as_str());
                let outcome = self.control(
                    Method::Post,
                    &format!("{API}/playlists/{id}/tracks"),
                    Some(&json!({ "uris": [uri] })),
                );
                self.after_control(outcome)
            }
            Plan::PlayUri(raw) => {
                let uri = normalize_context_uri(raw);
                let body = if uri.starts_with("spotify:track:") {
                    json!({ "uris": [uri] })
                } else {
                    json!({ "context_uri": uri })
                };
                let outcome = self.control(Method::Put, &format!("{API}/me/player/play"), Some(&body));
                self.after_control(outcome)
            }
            Plan::TransferDevice(target) => {
                let devices = self.devices_internal()?;
                let matched = match_device(&devices, target);
                let Some(device) = matched else {
                    return Err(SpotifyError::Api(format!("unknown Spotify device {target:?}")));
                };
                let outcome = self.control(
                    Method::Put,
                    &format!("{API}/me/player"),
                    Some(&json!({ "device_ids": [device.id], "play": false })),
                );
                self.after_control(outcome)
            }
            Plan::SetVolume(v) => {
                let percent = (v.clamp(0.0, 1.0) * 100.0).round() as i32;
                let outcome = self.control(
                    Method::Put,
                    &format!("{API}/me/player/volume?volume_percent={percent}"),
                    None,
                );
                self.after_control(outcome)
            }
            Plan::Seek(v) => {
                let player = self.player()?.ok_or(SpotifyError::NoActiveDevice)?;
                let duration = player
                    .pointer("/item/duration_ms")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| SpotifyError::Api("no track duration to seek within".into()))?;
                let position = (v.clamp(0.0, 1.0) * duration as f64).round() as u64;
                let outcome = self.control(
                    Method::Put,
                    &format!("{API}/me/player/seek?position_ms={position}"),
                    None,
                );
                self.after_control(outcome)
            }
        }
    }

    /// Volume ±10 % on the active device's volume (design §3).
    fn apply_volume_step(&self, delta: i32) -> Result<()> {
        let current = self.current_volume()?;
        let next = (current as i32 + delta).clamp(0, 100) as u32;
        let outcome = self.control(
            Method::Put,
            &format!("{API}/me/player/volume?volume_percent={next}"),
            None,
        );
        self.after_control(outcome)
    }

    /// Mute remembering the level; restore when already muted.
    fn apply_volume_mute(&self) -> Result<()> {
        let current = self.current_volume()?;
        let next = if current > 0 {
            *self.inner.last_volume.lock().unwrap() = Some(current);
            0
        } else {
            self.inner.last_volume.lock().unwrap().unwrap_or(50)
        };
        let outcome = self.control(
            Method::Put,
            &format!("{API}/me/player/volume?volume_percent={next}"),
            None,
        );
        self.after_control(outcome)
    }

    fn current_volume(&self) -> Result<u32> {
        self.player()?
            .as_ref()
            .and_then(|p| p.pointer("/device/volume_percent"))
            .and_then(Value::as_u64)
            .map(|v| v.min(100) as u32)
            .ok_or(SpotifyError::NoActiveDevice)
    }

    fn current_track_uri(&self) -> Result<String> {
        self.player()?
            .as_ref()
            .and_then(|p| p.pointer("/item/uri"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(SpotifyError::NoActiveDevice)
    }

    /// Bookkeeping after a control call: signal the fast re-poll and
    /// pass the outcome through.
    fn after_control(&self, outcome: Result<()>) -> Result<()> {
        self.inner.wake.signal();
        outcome
    }
}

/// One Connect device.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub is_active: bool,
}

/// One user playlist (editor picker).
#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub uri: String,
}

/// Match a `{"device": "<name or id>"}` target: exact name
/// (case-insensitive), then substring, then id (design §3 - names are
/// stable, ids change).
fn match_device<'a>(devices: &'a [Device], target: &str) -> Option<&'a Device> {
    let lower = target.to_ascii_lowercase();
    devices
        .iter()
        .find(|d| d.name.to_ascii_lowercase() == lower)
        .or_else(|| {
            devices
                .iter()
                .find(|d| d.name.to_ascii_lowercase().contains(&lower))
        })
        .or_else(|| devices.iter().find(|d| d.id == target))
}

/// Accept `spotify:playlist:...` URIs and `open.spotify.com/<type>/<id>`
/// URLs; anything else passes through untouched (already a URI).
fn normalize_context_uri(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("https://open.spotify.com/") {
        let mut parts = rest.split('/');
        if let (Some(kind), Some(id)) = (parts.next(), parts.next()) {
            let id = id.split(['?', '#']).next().unwrap_or(id);
            if matches!(kind, "playlist" | "album" | "track" | "artist") && !id.is_empty() {
                return format!("spotify:{kind}:{id}");
            }
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::user_message;
    use crate::http::{FakeTransport, TransportError};

    /// FakeTransport handle shared with the Spotify instance, so tests
    /// can script answers and read requests after construction.
    #[derive(Clone)]
    struct SharedFake(Arc<FakeTransport>);

    impl Transport for SharedFake {
        fn send(&self, req: &HttpRequest) -> std::result::Result<HttpResponse, TransportError> {
            self.0.send(req)
        }
    }

    fn logged_in(expires_in: i64) -> SpotifyConfig {
        SpotifyConfig {
            client_id: "cid".into(),
            access_token: "ACCESS".into(),
            refresh_token: Some("REFRESH".into()),
            expires_at: Some(now_unix() + expires_in),
            user: Some("u".into()),
            product: Some("premium".into()),
        }
    }

    fn handle(config: SpotifyConfig) -> (Spotify, Arc<FakeTransport>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeTransport::new());
        let path = dir.path().join("spotify.json");
        config.save(&path).unwrap();
        let spotify = Spotify::with_transport(
            SpotifyConfig::load(&path).unwrap(),
            path.clone(),
            Box::new(SharedFake(fake.clone())),
        );
        (spotify, fake, dir)
    }

    fn body_of(req: &HttpRequest) -> Option<String> {
        req.body
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn ok(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn api_err(status: u16, reason: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: serde_json::to_vec(&json!({ "error": { "message": "m", "reason": reason } })).unwrap(),
        }
    }

    fn player_body(playing: bool) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&json!({
                "is_playing": playing,
                "shuffle_state": true,
                "repeat_state": "off",
                "progress_ms": 1000,
                "device": { "id": "d", "name": "Kitchen", "volume_percent": 55, "is_active": true },
                "item": { "uri": "spotify:track:abc", "name": "S", "duration_ms": 200_000 }
            }))
            .unwrap(),
        }
    }

    // ---- design §7: error mapping -----------------------------------

    #[test]
    fn unauthorized_refreshes_once_with_rotation_persisted() {
        let (spotify, fake, dir) = handle(logged_in(3600));
        // player 401 -> token refresh (rotated) -> player retry 200
        fake.push(ok(401));
        fake.push_json(
            200,
            json!({ "access_token": "FRESH", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "ROTATED" }),
        );
        let mut player = player_body(true);
        player.body = serde_json::to_vec(&json!({ "is_playing": false })).unwrap();
        fake.push(player);

        let found = spotify.player().unwrap();
        assert!(found.is_some());
        // the retry used the fresh token
        assert_eq!(
            fake.requests()[2].headers.iter().find(|(k, _)| k == "authorization").map(|(_, v)| v.clone()),
            Some("Bearer FRESH".into())
        );
        // rotation is persisted: the file (not just memory) has it
        let saved = SpotifyConfig::load(&dir.path().join("spotify.json")).unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("ROTATED"));
        assert_eq!(saved.access_token, "FRESH");
    }

    #[test]
    fn premium_required_maps_with_its_user_message() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // playing -> pause
        fake.push(api_err(403, "PREMIUM_REQUIRED"));
        let err = spotify.exec("spotify-playback", "play", None).unwrap_err();
        assert!(matches!(err, SpotifyError::PremiumRequired));
        assert_eq!(user_message(&err), "Spotify Premium required");
    }

    #[test]
    fn no_active_device_maps_with_its_user_message() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(api_err(404, "NO_ACTIVE_DEVICE"));
        let err = spotify.exec("spotify-playback", "next", None).unwrap_err();
        assert!(matches!(err, SpotifyError::NoActiveDevice));
        assert_eq!(user_message(&err), "Open Spotify on a device first");
    }

    #[test]
    fn too_many_requests_pauses_everything() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        let mut resp = api_err(429, "rate limit exceeded");
        resp.headers.push(("retry-after".into(), "3".into()));
        fake.push(resp);
        let err = spotify.exec("spotify-playback", "next", None).unwrap_err();
        assert!(matches!(err, SpotifyError::RateLimited { .. }));
        // the pause gates the NEXT call before any transport happens:
        // no scripted response remains, so a transport attempt would
        // surface as Network, not RateLimited
        let err = spotify.player().unwrap_err();
        assert!(matches!(err, SpotifyError::RateLimited { retry_after_secs: s } if s <= 4));
    }

    #[test]
    fn invalid_grant_flips_to_needs_login_and_is_not_retried() {
        let (spotify, fake, dir) = handle(logged_in(-10)); // expired: refresh on entry
        fake.push_json(400, json!({ "error": "invalid_grant" }));
        assert!(matches!(spotify.player(), Err(SpotifyError::NeedsLogin)));
        assert!(spotify.needs_login());
        // tokens wiped on disk; the client id survives
        let saved = SpotifyConfig::load(&dir.path().join("spotify.json")).unwrap();
        assert_eq!(saved.client_id, "cid");
        assert_eq!(saved.refresh_token, None);
        // later calls fail fast without touching the transport
        assert!(matches!(spotify.player(), Err(SpotifyError::NeedsLogin)));
        assert_eq!(fake.requests().len(), 1);
        assert!(matches!(
            spotify.exec("spotify-playback", "play", None),
            Err(SpotifyError::NeedsLogin)
        ));
        assert_eq!(fake.requests().len(), 1);
    }

    #[test]
    fn needs_login_message_is_user_facing() {
        assert_eq!(user_message(&SpotifyError::NeedsLogin), "Log in to Spotify in Pulpit settings");
    }

    // ---- design §7: library endpoint fallback ------------------------

    #[test]
    fn library_falls_back_to_legacy_endpoints_and_memorizes() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        // mutate (save): new 400 -> legacy 200; the memo remembers legacy
        fake.push(ok(400));
        fake.push(ok(200));
        spotify.library_mutate(true, "spotify:track:abc").unwrap();
        // contains: memoized legacy directly, one call
        fake.push_json(200, json!([true]));
        assert!(spotify.library_contains("spotify:track:abc").unwrap());
        let urls: Vec<String> = fake.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            vec![
                "https://api.spotify.com/v1/me/library",
                "https://api.spotify.com/v1/me/tracks?ids=abc",
                "https://api.spotify.com/v1/me/tracks/contains?ids=abc",
            ]
        );
        // the new-endpoint attempt carried the documented body
        assert_eq!(body_of(&fake.requests()[0]).as_deref(), Some(r#"{"uris":["spotify:track:abc"]}"#));

        // fresh handle: contains falls back new 404 -> legacy
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(ok(404));
        fake.push_json(200, json!([false]));
        assert!(!spotify.library_contains("spotify:track:abc").unwrap());
        assert_eq!(
            fake.requests()[1].url,
            "https://api.spotify.com/v1/me/tracks/contains?ids=abc"
        );
    }

    // ---- design §7-ish: apply URLs per plan --------------------------

    #[test]
    fn play_pauses_or_resumes_by_current_state() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // playing -> pause
        fake.push(ok(204));
        spotify.exec("spotify-playback", "play", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/pause"));

        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(false)); // paused -> play
        fake.push(ok(204));
        spotify.exec("spotify-playback", "play", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/play"));
    }

    #[test]
    fn volume_steps_mute_memory_and_slider_mapping() {
        // vol_up: 55 -> 65
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true));
        fake.push(ok(204));
        spotify.exec("spotify-playback", "vol_up", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/volume?volume_percent=65"));

        // mute remembers 55 and restores it
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true));
        fake.push(ok(204));
        spotify.exec("spotify-playback", "vol_mute", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/volume?volume_percent=0"));
        let mut muted = player_body(false);
        muted.body = serde_json::to_vec(&json!({
            "is_playing": false,
            "device": { "id": "d", "name": "Kitchen", "volume_percent": 0 }
        }))
        .unwrap();
        fake.push(muted);
        fake.push(ok(204));
        spotify.exec("spotify-playback", "vol_mute", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/volume?volume_percent=55"));

        // slider: 0..1 -> percent
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(ok(204));
        spotify.exec("spotify-volume", "", Some(0.55)).unwrap();
        assert!(fake.last_url().ends_with("/me/player/volume?volume_percent=55"));
    }

    #[test]
    fn seek_maps_the_slider_into_the_track_position() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // duration 200_000
        fake.push(ok(204));
        spotify.exec("spotify-seek", "", Some(0.5)).unwrap();
        assert!(fake.last_url().ends_with("/me/player/seek?position_ms=100000"));
    }

    #[test]
    fn shuffle_and_repeat_send_the_toggled_state() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // shuffle_state: true
        fake.push(ok(204));
        spotify.exec("spotify-shuffle", "", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/shuffle?state=false"));

        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // repeat_state: "off" -> context
        fake.push(ok(204));
        spotify.exec("spotify-repeat", "", None).unwrap();
        assert!(fake.last_url().ends_with("/me/player/repeat?state=context"));
    }

    #[test]
    fn transfer_matches_by_name_then_id() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push_json(
            200,
            json!({ "devices": [
                { "id": "id-1", "name": "Kitchen", "is_active": false },
                { "id": "id-2", "name": "PC", "is_active": true },
            ]}),
        );
        fake.push(ok(204));
        spotify.exec("spotify-device", r#"{"device":"kitchen"}"#, None).unwrap();
        assert!(fake.last_url().ends_with("/me/player"));
        assert_eq!(body_of(&fake.requests()[1]).as_deref(), Some(r#"{"device_ids":["id-1"],"play":false}"#));

        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push_json(
            200,
            json!({ "devices": [{ "id": "id-1", "name": "Kitchen" }, { "id": "id-2", "name": "PC" }] }),
        );
        fake.push(ok(204));
        spotify.exec("spotify-device", r#"{"device":"id-2"}"#, None).unwrap();
        assert_eq!(body_of(&fake.requests()[1]).as_deref(), Some(r#"{"device_ids":["id-2"],"play":false}"#));
    }

    #[test]
    fn play_uri_picks_uris_or_context_by_type() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(ok(204));
        spotify
            .exec("spotify-tracks", r#"{"uri":"spotify:track:xyz"}"#, None)
            .unwrap();
        assert_eq!(
            body_of(&fake.requests()[0]).as_deref(),
            Some(r#"{"uris":["spotify:track:xyz"]}"#)
        );

        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(ok(204));
        spotify
            .exec(
                "spotify-tracks",
                r#"{"uri":"https://open.spotify.com/album/5z7tK5aXmnS7jlU6iv4Z7e?si=abc"}"#,
                None,
            )
            .unwrap();
        assert_eq!(
            body_of(&fake.requests()[0]).as_deref(),
            Some(r#"{"context_uri":"spotify:album:5z7tK5aXmnS7jlU6iv4Z7e"}"#)
        );
    }

    #[test]
    fn add_to_playlist_extracts_the_id_from_uri() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // current track uri
        fake.push(ok(201));
        spotify
            .exec("spotify-add", r#"{"playlist":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"}"#, None)
            .unwrap();
        assert!(fake.last_url().ends_with("/playlists/37i9dQZF1DXcBWIGoYBM5M/tracks"));
        assert_eq!(
            body_of(&fake.requests()[1]).as_deref(),
            Some(r#"{"uris":["spotify:track:abc"]}"#)
        );
    }

    #[test]
    fn like_flips_the_current_library_state_and_marks_dirty() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(player_body(true)); // current track
        fake.push_json(200, json!([true])); // contains: liked -> unsave
        fake.push(ok(200));
        spotify.exec("spotify-like", "", None).unwrap();
        assert!(fake.last_url().ends_with("/me/library"));
        assert_eq!(fake.requests()[2].method, crate::http::Method::Delete);
        // the poller re-checks on its fast re-poll
        assert!(spotify.take_liked_dirty());
    }

    #[test]
    fn display_tile_press_is_a_claimed_noop() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        spotify.exec("spotify-now-playing", "", None).unwrap();
        assert!(fake.requests().is_empty());
    }

    #[test]
    fn playlists_and_devices_read_their_endpoints() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push_json(
            200,
            json!({ "items": [{ "id": "p1", "name": "Focus", "uri": "spotify:playlist:p1" }], "next": null }),
        );
        let lists = spotify.playlists().unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "Focus");

        fake.push_json(
            200,
            json!({ "devices": [{ "id": "d1", "name": "Kitchen", "is_active": true }] }),
        );
        let devices = spotify.devices().unwrap();
        assert_eq!(devices[0].id, "d1");
        assert!(devices[0].is_active);
    }

    #[test]
    fn devices_parses_and_seek_requires_a_track() {
        let (spotify, fake, _dir) = handle(logged_in(3600));
        fake.push(ok(204)); // nothing playing
        assert!(matches!(
            spotify.exec("spotify-seek", "", Some(0.5)),
            Err(SpotifyError::NoActiveDevice)
        ));
    }
}
