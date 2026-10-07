//! Native OBS Studio integration over obs-websocket v5: protocol
//! framing, SHA-256 auth, one connection worker with reconnect/backoff,
//! a live-state reducer whose snapshots ride the shared host producer
//! pump, and the exec surface the backend's native dispatch chain uses.

pub mod auth;
pub mod client;
pub mod config;
pub mod protocol;
pub mod state;

pub use auth::compute_auth_response;
pub use client::{is_obs_action, parse_action, Obs, ObsAction, ObsChoices, ObsStatusInfo};
pub use config::ObsConfig;
pub use protocol::Message;
pub use state::ObsState;
