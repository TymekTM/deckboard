//! Typed errors. `Display` of the three design-mapped variants is the
//! user-facing message (design §3): toasts on the desktop, the flash path
//! on tablets read exactly these strings.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpotifyError {
    /// `invalid_grant`, or no tokens at all: the saved login is gone or
    /// expired (refresh tokens reportedly live ~6 months). Never retried
    /// by the crate - the user must log in again from the settings.
    #[error("Log in to Spotify in Pulpit settings")]
    NeedsLogin,
    /// 403 with `reason: PREMIUM_REQUIRED`: control endpoints need
    /// Premium; reads work on Free.
    #[error("Spotify Premium required")]
    PremiumRequired,
    /// 404 with `reason: NO_ACTIVE_DEVICE`: nothing is playing anywhere.
    #[error("Open Spotify on a device first")]
    NoActiveDevice,
    /// 429 with `Retry-After`: the whole handle pauses for
    /// `Retry-After + 0.5 s`; every later call fails fast with the
    /// remaining pause.
    #[error("Spotify rate limit - retry in {retry_after_secs}s")]
    RateLimited { retry_after_secs: u32 },
    /// A real API answer the caller must see as a failure (non-2xx
    /// without a mapped reason). Carries Spotify's own message.
    #[error("Spotify error: {0}")]
    Api(String),
    /// Transport-level failure: the request never reached Spotify.
    /// Distinguished from a rejection so offline machines do not look
    /// like they need a login (same discipline as `DiscordError::Network`).
    #[error("network unreachable for Spotify ({0})")]
    Network(&'static str),
    /// The tile's command JSON could not be parsed into an action.
    #[error("bad tile payload for {0}: {1}")]
    BadPayload(&'static str, String),
    /// The login flow failed before tokens existed (port busy, callback
    /// denied/timed out, code exchange rejected).
    #[error("Spotify login failed: {0}")]
    Login(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SpotifyError>;

/// User-facing text for toasts/flashes: the design §3 messages are the
/// `Display` strings of the mapped variants, everything else keeps its
/// technical text (shown in logs).
pub fn user_message(err: &SpotifyError) -> String {
    err.to_string()
}
