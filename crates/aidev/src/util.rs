//! Tiny helpers shared by the provider modules.

use std::path::Path;

/// Extra seconds beyond `agent_done_secs` before a session counts as
/// done: filesystem mtimes lag the last write, and a session finishing
/// right at the boundary should not flap between states.
pub(crate) const DONE_GRACE_SECS: i64 = 60;

/// File modification time in epoch seconds.
pub(crate) fn mtime(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Cut to `max` characters, ending with an ellipsis when truncated.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}
