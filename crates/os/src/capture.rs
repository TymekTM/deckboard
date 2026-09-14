//! Primary-screen capture saved as PNG, mirroring the original
//! `takeScreenshot`: `Deckboard_<UTC timestamp>.png` inside the
//! command's directory (default: the user's Pictures folder).

use std::path::PathBuf;

use crate::{OsError, Result};

/// Original naming: `new Date().toISOString()` with `-`, `:`, `T` and the
/// fractional part stripped -> `Deckboard_20260914142233.png`.
pub(crate) fn screenshot_filename(stamp: chrono::DateTime<chrono::Utc>) -> String {
    format!("Deckboard_{}.png", stamp.format("%Y%m%d%H%M%S"))
}

fn default_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::UI::Shell::{
            SHGetKnownFolderPath, FOLDERID_Pictures, KNOWN_FOLDER_FLAG,
        };
        // returns a CoTask-allocated wide string
        let path = unsafe {
            SHGetKnownFolderPath(&FOLDERID_Pictures, KNOWN_FOLDER_FLAG(0), HANDLE::default())
                .map_err(|e| OsError::Failed(format!("pictures folder: {e}")))?
        };
        let s = unsafe { crate::win::take_pwstr(path) };
        return Ok(PathBuf::from(s));
    }
    #[cfg(not(windows))]
    {
        if let Some(dirs) = std::env::var_os("XDG_PICTURES_DIR") {
            return Ok(PathBuf::from(dirs));
        }
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Pictures"))
            .ok_or(OsError::Unsupported("screenshot: no home directory"))
    }
}

/// Capture the primary screen and write `Deckboard_<stamp>.png` into
/// `dir` (empty means the Pictures folder). Returns the written path.
pub fn screenshot_to_dir(dir: &str) -> Result<PathBuf> {
    let target = if dir.trim().is_empty() {
        default_dir()?
    } else {
        PathBuf::from(dir)
    };
    std::fs::create_dir_all(&target)
        .map_err(|e| OsError::Failed(format!("screenshot dir {}: {e}", target.display())))?;
    let path = target.join(screenshot_filename(chrono::Utc::now()));
    let rgba = grab_screen_rgba()?;
    image::save_buffer_with_format(
        &path,
        &rgba,
        rgba.width(),
        rgba.height(),
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
        .map_err(|e| OsError::Failed(format!("png write {}: {e}", path.display())))?;
    Ok(path)
}

#[cfg(windows)]
fn grab_screen_rgba() -> Result<image::RgbaImage> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
        SRCCOPY, CAPTUREBLT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    unsafe {
        let screen = GetDC(windows::Win32::Foundation::HWND::default());
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(mem, bmp);
        // CAPTUREBLT includes layered windows like the original capture did
        let blit = BitBlt(
            mem, 0, 0, width, height, screen, 0, 0, SRCCOPY | CAPTUREBLT,
        );
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        // negative height: rows top-down, matching the PNG layout
        bmi.bmiHeader.biHeight = -height;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0; // BI_RGB
        let mut buf = vec![0u8; (width.max(0) as usize) * (height.max(0) as usize) * 4];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            height as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(windows::Win32::Foundation::HWND::default(), screen);
        if blit.is_err() || lines == 0 {
            return Err(OsError::Failed("screen bit copy failed".into()));
        }
        // GDI gives BGRA; the PNG wants RGBA with opaque alpha
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        image::RgbaImage::from_raw(width as u32, height as u32, buf)
            .ok_or_else(|| OsError::Failed("screen buffer size mismatch".into()))
    }
}

#[cfg(not(windows))]
fn grab_screen_rgba() -> Result<image::RgbaImage> {
    Err(OsError::Unsupported("screen capture"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "live: captures the real screen"]
    fn live_screenshot_writes_png() {
        let dir = std::env::temp_dir().join("deckboard-os-shot-test");
        let path = screenshot_to_dir(dir.to_str().unwrap()).expect("screenshot");
        let bytes = std::fs::read(&path).expect("read back");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a png");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        assert!(width > 800, "suspiciously narrow screen: {width}");
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
