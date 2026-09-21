//! Clipboard text access for the paste-based unicode typing the original
//! used (`typeString`: save clipboard, write text, Ctrl+V, restore).

use crate::{OsError, Result};

/// Current clipboard sequence number; the OS bumps it on every write,
/// whoever made it. Lets the paste-restore path notice that the user (or
/// another app) copied something in the meantime.
pub fn sequence_number() -> u32 {
    #[cfg(windows)]
    {
        use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
        unsafe { GetClipboardSequenceNumber() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Current clipboard text, empty when the clipboard holds non-text data.
pub fn get_text() -> Result<String> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HGLOBAL, HWND};
        use windows::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        unsafe {
            OpenClipboard(HWND::default())
                .map_err(|e| OsError::Failed(format!("open clipboard: {e}")))?;
            let result = (|| {
                let handle = GetClipboardData(13 /* CF_UNICODETEXT */)
                    .map_err(|e| OsError::Failed(format!("get clipboard: {e}")))?;
                if handle.0.is_null() {
                    return Ok(String::new());
                }
                let ptr = GlobalLock(HGLOBAL(handle.0)).cast::<u16>();
                if ptr.is_null() {
                    return Ok(String::new());
                }
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(HGLOBAL(handle.0));
                Ok(text)
            })();
            let _ = CloseClipboard();
            result
        }
    }
    #[cfg(not(windows))]
    {
        let _ = OsError::Unsupported("clipboard read");
        Ok(String::new())
    }
}

/// Replace the clipboard with `text` (UTF-16, NUL-terminated).
pub fn set_text(text: &str) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
        unsafe {
            OpenClipboard(HWND::default())
                .map_err(|e| OsError::Failed(format!("open clipboard: {e}")))?;
            let result = (|| {
                EmptyClipboard().map_err(|e| OsError::Failed(format!("empty clipboard: {e}")))?;
                let mut wide: Vec<u16> = text.encode_utf16().collect();
                wide.push(0);
                let handle =
                    GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| {
                        OsError::Failed(format!("clipboard alloc: {e}"))
                    })?;
                let ptr = GlobalLock(handle);
                if ptr.is_null() {
                    let _ = GlobalFree(handle);
                    return Err(OsError::Failed("clipboard lock failed".into()));
                }
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr.cast::<u16>(), wide.len());
                let _ = GlobalUnlock(handle);
                // 13 = CF_UNICODETEXT; on success the system owns the
                // allocation - only a failed handover leaks it back to us.
                if let Err(e) = SetClipboardData(13, HANDLE(handle.0)) {
                    let _ = GlobalFree(handle);
                    return Err(OsError::Failed(format!("set clipboard: {e}")));
                }
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }
    }
    #[cfg(not(windows))]
    Err(OsError::Unsupported("clipboard write"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "live: replaces the real clipboard, restoring it after"]
    fn live_clipboard_round_trip() {
        let original = get_text().expect("read");
        set_text("deckboard-m2-四十二").expect("write");
        assert_eq!(get_text().expect("read back"), "deckboard-m2-四十二");
        if original.is_empty() {
            set_text(" ").ok(); // keep unicode text, cannot restore empty
        } else {
            set_text(&original).expect("restore");
        }
    }
}
