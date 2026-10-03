//! Local audio playback for "Play Audio" tiles (the original played the
//! file through an HTML5 Audio element in its renderer). MCI covers the
//! formats a soundboard realistically uses (mp3/wav/wma) with zero extra
//! dependencies; a fresh press restarts the clip, matching the original's
//! default `audioPress` restart behavior.

#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
/// Serializes the whole close/open/play sequence: two concurrent presses
/// (editor + tablet) must not interleave their MCI commands.
static PLAY_LOCK: Mutex<()> = Mutex::new(());

#[cfg(windows)]
/// The alias of the clip still open from the previous press, closed when
/// the next one starts (restart semantics without a shared alias).
static LAST_ALIAS: Mutex<Option<String>> = Mutex::new(None);

#[cfg(windows)]
fn next_alias() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("pulpit_play_{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Start `path` playing; a press while a clip is running restarts it.
/// An empty path is a no-op (tiles without a configured file).
#[cfg(windows)]
pub fn play_audio(path: &str) -> Result<(), String> {
    use windows::core::PCWSTR;

    #[link(name = "winmm")]
    extern "system" {
        // returns MCIERROR; 0 == success
        fn mciSendStringW(
            command: PCWSTR,
            ret: *mut u16,
            ret_len: u32,
            callback: *mut core::ffi::c_void,
        ) -> u32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn mci(s: &str) -> u32 {
        unsafe {
            mciSendStringW(
                PCWSTR(wide(s).as_ptr()),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            )
        }
    }

    if path.trim().is_empty() {
        return Ok(());
    }
    // One lock around close/open/play: MCI aliases live in a per-process
    // namespace, so a shared alias name across concurrent playbacks let
    // two racing sequences close each other's device mid-open. Every
    // playback gets a fresh alias; the previous one is closed first so a
    // new press still restarts the clip instead of layering sounds.
    let guard = PLAY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut previous = LAST_ALIAS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(last) = previous.as_ref() {
        let _ = mci(&format!("close {last}"));
    }
    *previous = None;
    drop(previous);

    // mpegvideo covers mp3/wma; plain open handles wav if that fails
    let alias = next_alias();
    let quoted = path.replace('"', "'");
    let ok = mci(&format!("open \"{quoted}\" type mpegvideo alias {alias}")) == 0
        || mci(&format!("open \"{quoted}\" alias {alias}")) == 0;
    if !ok {
        let _ = mci(&format!("close {alias}"));
        return Err(format!("cannot open audio file: {path}"));
    }
    if mci(&format!("play {alias}")) != 0 {
        let _ = mci(&format!("close {alias}"));
        return Err(format!("cannot play audio file: {path}"));
    }
    *LAST_ALIAS.lock().unwrap_or_else(|p| p.into_inner()) = Some(alias);
    drop(guard);
    Ok(())
}

#[cfg(not(windows))]
pub fn play_audio(_path: &str) -> Result<(), String> {
    Err("audio playback is Windows-only".into())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn every_playback_gets_a_fresh_alias() {
        // aliases must never repeat within a process: a repeated name
        // would collide with the still-open device of a previous clip
        let first = super::next_alias();
        let second = super::next_alias();
        assert_ne!(first, second);
        assert!(first.starts_with("pulpit_play_"));
    }

    #[test]
    fn empty_path_is_a_no_op() {
        assert!(super::play_audio("").is_ok());
        assert!(super::play_audio("   ").is_ok());
    }

    #[test]
    fn missing_file_reports_error() {
        assert!(super::play_audio("Z:/definitely/not/a/file.mp3").is_err());
    }
}
