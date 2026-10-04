//! Native Spotify Web API integration (design round 4 §1-§4): PKCE
//! login with a BYO client id, playback control tiles, a live-state
//! poller and typed errors with user-facing messages.
//!
//! Shape (following the discord/aidev patterns):
//! - [`SpotifyConfig`] persists `spotify.json` (atomic writes,
//!   redacted `Debug`);
//! - [`login`] runs the browser PKCE flow against the fixed loopback
//!   redirect `http://127.0.0.1:8502/spotify/callback` - blocking, the
//!   host calls it from `spawn_blocking`;
//! - [`Spotify`] is the cheap-to-clone handle all requests serialize
//!   through (lazy token refresh with rotation, `invalid_grant` ->
//!   NeedsLogin, 429 global pause);
//! - [`input_declarations`] / [`is_spotify_action`] feed the style
//!   resolver and the backend's native dispatch chain;
//! - [`spawn_push`] polls playback into design-§4 snapshots at an
//!   adaptive cadence, idle while nobody consumes state.
//!
//! The tile kinds and their commands are design §3; the snapshot keys
//! are design §4. This crate never talks to the hosts' UI: the host
//! lane forwards snapshots, imports album art and owns the settings
//! panel.

pub mod http;

mod auth;
mod client;
mod config;
mod error;
mod pkce;
mod plan;
mod poller;
mod snapshot;

pub use auth::login;
pub use client::{Device, Playlist, Spotify};
pub use config::{logout, SpotifyConfig};
pub use error::{user_message, Result, SpotifyError};
pub use pkce::REDIRECT_URI;
pub use plan::{input_declarations, is_spotify_action, Plan};
pub use poller::spawn_push;
pub use snapshot::{build_snapshot, AuthState};
