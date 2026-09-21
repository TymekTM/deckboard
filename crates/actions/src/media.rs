//! Local audio playback for "Play Audio" tiles (the original played the
//! file through an HTML5 Audio element in its renderer). MCI covers the
//! formats a soundboard realistically uses (mp3/wav/wma) with zero extra
//! dependencies; a fresh press restarts the clip, matching the original's
//! default `audioPress` restart behavior.

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
        unsafe { mciSendStringW(PCWSTR(wide(s).as_ptr()), std::ptr::null_mut(), 0, std::ptr::null_mut()) }
    }

    if path.trim().is_empty() {
        return Ok(());
    }
    // single alias: a new press restarts instead of layering clips
    let _ = mci("close deckboard_play");
    // mpegvideo covers mp3/wma; plain open handles wav if that fails
    let quoted = path.replace('"', "'");
    let ok = mci(&format!("open \"{quoted}\" type mpegvideo alias deckboard_play")) == 0
        || mci(&format!("open \"{quoted}\" alias deckboard_play")) == 0;
    if !ok {
        let _ = mci("close deckboard_play");
        return Err(format!("cannot open audio file: {path}"));
    }
    if mci("play deckboard_play") != 0 {
        let _ = mci("close deckboard_play");
        return Err(format!("cannot play audio file: {path}"));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn play_audio(_path: &str) -> Result<(), String> {
    Err("audio playback is Windows-only".into())
}

#[cfg(all(test, windows))]
mod tests {
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
