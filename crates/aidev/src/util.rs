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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("", 0), "");
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_cuts_with_an_ellipsis_within_the_budget() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abcdef", 4).chars().count(), 4);
        assert_eq!(truncate("abcdef", 1), "…");
        // a zero budget still marks the cut instead of underflowing
        assert_eq!(truncate("abc", 0), "…");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate("żółw", 4), "żółw");
        assert_eq!(truncate("żółwik", 4), "żół…");
    }

    #[test]
    fn mtime_reads_existing_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        assert_eq!(mtime(&path), None);
        std::fs::write(&path, b"{}").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let got = mtime(&path).unwrap();
        assert!((now - got).abs() < 120, "mtime {got} vs now {now}");
    }

    #[test]
    fn done_grace_is_a_minute() {
        assert_eq!(DONE_GRACE_SECS, 60);
    }
}
