//! Desktop OS integration for M2: playback-audio control, screen capture
//! and clipboard access.
//!
//! Everything platform-specific hides behind small seams so a Linux port
//! is an implementation swap, not a rewrite:
//! - [`Speaker`]: default-playback volume/mute/device. Windows ships now
//!   (WASAPI + IPolicyConfig); Linux would bind ALSA/PipeWire here.
//! - [`clipboard`]: text get/set + the paste round-trip the original app
//!   used for unicode text. Linux would use the X11/Wayland clipboard.
//! - [`capture`]: primary-screen PNG grab. Linux would use XShm/pipewire.
//!
//! Non-Windows builds compile with stubs returning `Unsupported`.

pub mod capture;
pub mod clipboard;
pub mod play;
pub use play::play_audio;

#[derive(Debug, thiserror::Error)]
pub enum OsError {
    #[error("unsupported on this platform: {0}")]
    Unsupported(&'static str),
    #[error("os call failed: {0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, OsError>;

/// One playback endpoint as shown in the original device picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// Endpoint id (`{0.0.0.00000000}.{guid}` on Windows) - the value the
    /// original app stored in `speaker-device` button commands.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Default playback control: master volume, mute and device switching.
/// All methods block; call from a blocking thread.
pub trait Speaker: Send {
    /// Master volume in percent 0..=100.
    fn volume(&mut self) -> Result<f32>;
    fn muted(&mut self) -> Result<bool>;
    /// Set master volume, percent clamped to 0..=100.
    fn set_volume(&mut self, percent: f32) -> Result<()>;
    /// Active (enabled) playback devices; `is_default` marks the current
    /// default one.
    fn devices(&mut self) -> Result<Vec<AudioDevice>>;
    /// Endpoint id of the current default playback device.
    fn active_device(&mut self) -> Result<String>;
    /// Make `id` the default playback device (console, multimedia and
    /// communications roles, like SoundSwitch does).
    fn set_active_device(&mut self, id: &str) -> Result<()>;
}

/// Construct the platform Speaker implementation.
pub fn platform_speaker() -> impl Speaker {
    #[cfg(windows)]
    return win::WinSpeaker::new();
    #[cfg(not(windows))]
    return unsupported::StubSpeaker;
}

pub fn speaker_is_supported() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
pub(crate) mod win;

#[cfg(not(windows))]
pub(crate) mod unsupported;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_filename_matches_original_shape() {
        // original: UTC ISO stamp with separators stripped, e.g.
        // Deckboard_20260914142233.png
        let name = capture::screenshot_filename(
            chrono::DateTime::parse_from_rfc3339("2026-09-14T14:22:33Z")
                .unwrap()
                .into(),
        );
        assert_eq!(name, "Deckboard_20260914142233.png");
    }
}
