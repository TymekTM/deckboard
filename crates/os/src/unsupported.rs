//! Non-Windows stubs: every OS call reports `Unsupported`, so the crate
//! compiles where the WASAPI/clipboard/capture backends do not exist yet.
//! A real Linux port replaces these (see the crate docs for the seams).

use super::{AudioDevice, OsError, Result, Speaker};

pub(crate) struct StubSpeaker;

impl Speaker for StubSpeaker {
    fn volume(&mut self) -> Result<f32> {
        Err(OsError::Unsupported("speaker"))
    }
    fn muted(&mut self) -> Result<bool> {
        Err(OsError::Unsupported("speaker"))
    }
    fn set_volume(&mut self, _percent: f32) -> Result<()> {
        Err(OsError::Unsupported("speaker"))
    }
    fn devices(&mut self) -> Result<Vec<AudioDevice>> {
        Err(OsError::Unsupported("speaker"))
    }
    fn active_device(&mut self) -> Result<String> {
        Err(OsError::Unsupported("speaker"))
    }
    fn set_active_device(&mut self, _id: &str) -> Result<()> {
        Err(OsError::Unsupported("speaker"))
    }
}
