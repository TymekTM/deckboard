//! Extension host: runs original Deckboard extensions (Node.js CommonJS
//! modules shipped as `.asar` packages or plain directories) inside a
//! pure-Rust JS engine (Boa) with faithful shims of the modules they use.
//!
//! Trust model: extensions are trusted user-installed code with full
//! user powers - no sandbox, no permission model (see docs/decisions.md
//! ADR-012). One context per package. The shims cover the modules the
//! published ecosystem actually uses; modules outside that surface fail
//! with an explicit "not available" error instead of silently
//! misbehaving.

pub mod asar;
pub mod host;
pub mod manager;
pub mod source;

pub use host::{ExtRuntime, HostError, HostEvent};
pub use manager::{ExtEvent, ExtManager, ManagerError};
pub use source::PackageSource;
