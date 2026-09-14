//! Text clipboard access (Windows). The original app types text by pasting
//! through the clipboard, so exec needs read/write around `Ctrl+V`.

#[cfg(windows)]
pub fn get_text() -> Option<String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_err() {
            return None;
        }
        if OpenClipboard(None).is_err() {
            return None;
        }
        let result = (|| {
            let raw = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?.0;
            let handle = windows::Win32::Foundation::HGLOBAL(raw);
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(handle);
            Some(text)
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(windows)]
pub fn set_text(text: &str) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("cannot open clipboard".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            wide.push(0);
            let handle = GlobalAlloc(GLOBAL_ALLOC_FLAGS, wide.len() * 2)
                .map_err(|e| format!("GlobalAlloc failed: {e}"))?;
            let ptr = GlobalLock(handle) as *mut u16;
            if ptr.is_null() {
                return Err("GlobalLock failed".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(handle);
            // ownership transfers to the clipboard on success
            SetClipboardData(
                CF_UNICODETEXT.0 as u32,
                Some(windows::Win32::Foundation::HANDLE(handle.0)),
            )
            .map_err(|e| format!("SetClipboardData failed: {e}"))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

#[cfg(windows)]
const GLOBAL_ALLOC_FLAGS: windows::Win32::System::Memory::GLOBAL_ALLOC_FLAGS =
    windows::Win32::System::Memory::GMEM_MOVEABLE;

#[cfg(not(windows))]
pub fn get_text() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn set_text(_text: &str) -> Result<(), String> {
    Err("clipboard backend is Windows-only".into())
}

#[cfg(all(test, windows))]
mod tests {
    // touches the real clipboard; restores whatever was there before
    struct TextGuard(Option<String>);
    impl Drop for TextGuard {
        fn drop(&mut self) {
            if let Some(text) = &self.0 {
                let _ = super::set_text(text);
            }
        }
    }

    #[test]
    fn set_then_get_roundtrips() {
        let prev = super::get_text();
        let _guard = TextGuard(prev.clone());
        super::set_text("deckboard-paste-test ✓").expect("set clipboard");
        assert_eq!(super::get_text().as_deref(), Some("deckboard-paste-test ✓"));
    }
}
