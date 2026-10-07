//! Primary-screen capture saved as PNG, mirroring the original
//! `takeScreenshot`: `Pulpit_<UTC timestamp>.png` inside the
//! command's directory (default: the user's Pictures folder).

use std::path::PathBuf;

use crate::{OsError, Result};

/// Original naming: `new Date().toISOString()` with `-`, `:`, `T` and the
/// fractional part stripped -> `Pulpit_20260914142233.png`.
pub(crate) fn screenshot_filename(stamp: chrono::DateTime<chrono::Utc>) -> String {
    format!("Pulpit_{}.png", stamp.format("%Y%m%d%H%M%S"))
}

/// The stamp has second resolution, so two captures in one second would
/// silently overwrite; the second file gets a `_2`, `_3`, ... suffix.
fn unique_screenshot_path(dir: &std::path::Path, stamp: chrono::DateTime<chrono::Utc>) -> PathBuf {
    let filename = screenshot_filename(stamp);
    let path = dir.join(&filename);
    if !path.exists() {
        return path;
    }
    let stem = filename.trim_end_matches(".png");
    for n in 2u32.. {
        let candidate = dir.join(format!("{stem}_{n}.png"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 suffix space exhausted")
}

fn default_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::UI::Shell::{
            FOLDERID_Pictures, SHGetKnownFolderPath, KNOWN_FOLDER_FLAG,
        };
        // returns a CoTask-allocated wide string
        let path = unsafe {
            SHGetKnownFolderPath(&FOLDERID_Pictures, KNOWN_FOLDER_FLAG(0), HANDLE::default())
                .map_err(|e| OsError::Failed(format!("pictures folder: {e}")))?
        };
        let s = unsafe { crate::win::take_pwstr(path) };
        Ok(PathBuf::from(s))
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

/// Capture the primary screen and write `Pulpit_<stamp>.png` into
/// `dir` (empty means the Pictures folder). Returns the written path.
pub fn screenshot_to_dir(dir: &str) -> Result<PathBuf> {
    let target = if dir.trim().is_empty() {
        default_dir()?
    } else {
        PathBuf::from(dir)
    };
    std::fs::create_dir_all(&target)
        .map_err(|e| OsError::Failed(format!("screenshot dir {}: {e}", target.display())))?;
    let path = unique_screenshot_path(&target, chrono::Utc::now());
    let rgba = grab_screen_rgba()?;
    write_png_atomic(&path, &rgba)?;
    Ok(path)
}

/// Encode the PNG to a `.partial` sibling, then rename it into place.
/// A crash mid-encode leaves a `.partial` file behind instead of a
/// truncated screenshot under the final name - and the unique-path
/// helper would treat that truncated file as a real one on the next
/// capture in the same second.
fn write_png_atomic(path: &std::path::Path, rgba: &image::RgbaImage) -> Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".partial");
    let tmp = PathBuf::from(tmp_name);
    let written = image::save_buffer_with_format(
        &tmp,
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    );
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(OsError::Failed(format!("png write {}: {e}", tmp.display())));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(OsError::Failed(format!(
            "png rename {} -> {}: {e}",
            tmp.display(),
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn grab_screen_rgba() -> Result<image::RgbaImage> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
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
        let blit = BitBlt(mem, 0, 0, width, height, screen, 0, 0, SRCCOPY | CAPTUREBLT);
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
        for px in buf.as_chunks_mut::<4>().0 {
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
    fn same_second_screenshots_do_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = chrono::Utc::now();
        let first = unique_screenshot_path(dir.path(), stamp);
        assert_eq!(first, dir.path().join(screenshot_filename(stamp)));
        std::fs::write(&first, b"png").unwrap();
        // Same second again: the helper must dodge the existing file
        // instead of returning the same path (which would overwrite it).
        let second = unique_screenshot_path(dir.path(), stamp);
        assert_ne!(first, second);
        assert!(second
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("_2.png"));
        std::fs::write(&second, b"png").unwrap();
        let third = unique_screenshot_path(dir.path(), stamp);
        assert!(third != second && third != first);
    }

    #[test]
    fn png_writes_land_via_partial_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Pulpit_20260922000000.png");
        let rgba = image::RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 255]));
        write_png_atomic(&path, &rgba).expect("atomic png write");
        // the final name holds a real PNG...
        let bytes = std::fs::read(&path).expect("final file exists");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        // ...and no partial sibling survived the rename
        let mut partial = path.clone().into_os_string();
        partial.push(".partial");
        assert!(!std::path::Path::new(&partial).exists());

        // a failing write (parent directory vanishes) leaves nothing
        // behind either
        let gone = dir.path().join("removed").join("Pulpit_x.png");
        assert!(write_png_atomic(&gone, &rgba).is_err());
        assert!(!gone.exists());
    }

    #[test]
    #[ignore = "live: captures the real screen"]
    fn live_screenshot_writes_png() {
        let dir = std::env::temp_dir().join("pulpit-os-shot-test");
        let path = screenshot_to_dir(dir.to_str().unwrap()).expect("screenshot");
        let bytes = std::fs::read(&path).expect("read back");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a png");
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        assert!(width > 800, "suspiciously narrow screen: {width}");
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
