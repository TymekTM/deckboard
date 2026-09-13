//! Legacy compatibility layer: Engine.IO v3 + socket.io v2 server that the
//! stock Deckboard Android client (Free and Pro) speaks. Wire behavior is
//! ported from the original Electron main process (v3.2.0).

pub mod hub;
pub mod mapping;
pub mod props;
pub mod service;

pub use hub::{Hub, Session, ACCESS_KEY_PRO};
pub use mapping::Mapper;
pub use props::StyleResolver;
pub use service::{router, AppState, Backend};

pub const LEGACY_VERSION_REPLY: &str = "1.6.0";
