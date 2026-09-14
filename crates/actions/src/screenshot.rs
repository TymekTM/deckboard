//! Screen capture (Windows GDI) - the original's `screenshot` action saves
//! `Deckboard_<utc-timestamp>.png` into the folder given by the tile
//! command (empty command falls back to `~/Pictures`).

use std::path::PathBuf;

pub fn take_screenshot(dir: &str) -> Result<PathBuf, String> {
    let target = if dir.trim().is_empty() {
        home_pictures()
    } else {
        PathBuf::from(dir)
    };
    std::fs::create_dir_all(&target).map_err(|e| format!("cannot create {target:?}: {e}"))?;

    let (width, height, rgba) = capture_primary_screen()?;
    let path = target.join(format!("Deckboard_{}.png", timestamp_utc_compact()));
    encode_png(&path, width, height, &rgba)?;
    Ok(path)
}

fn home_pictures() -> PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Pictures")
}

// the original builds the name from `new Date().toISOString()` with
// T/-/:/. stripped: `Deckboard_20260914193000.png`
fn timestamp_utc_compact() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let secs = now % 86_400;
    format!(
        "{year:04}{month:02}{day:02}{h:02}{m:02}{s:02}",
        h = secs / 3600,
        m = (secs % 3600) / 60,
        s = secs % 60,
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// BGRA (top-down DIB) -> RGBA
#[cfg(windows)]
fn capture_primary_screen() -> Result<(u32, u32, Vec<u8>), String> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
        SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    unsafe {
        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            return Err("invalid screen metrics".into());
        }
        let (width, height) = (width as u32, height as u32);

        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // negative height = top-down rows, like the original capture
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(mem_dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("CreateDIBSection failed: {e}"))?;
        let old = SelectObject(mem_dc, dib.into());

        let blit = BitBlt(
            mem_dc,
            0,
            0,
            width as i32,
            height as i32,
            Some(screen_dc),
            0,
            0,
            SRCCOPY | CAPTUREBLT,
        )
        .map_err(|e| format!("BitBlt failed: {e}"));

        let pixels = blit.and_then(|_| {
            let count = (width as usize) * (height as usize) * 4;
            let mut bgra = vec![0u8; count];
            let ok = GetDIBits(
                mem_dc,
                dib,
                0,
                height,
                Some(bgra.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            if ok == 0 {
                return Err("GetDIBits failed".into());
            }
            let mut rgba = bgra;
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
                px[3] = 255;
            }
            Ok(rgba)
        });

        SelectObject(mem_dc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem_dc);
        ReleaseDC(None, screen_dc);
        pixels.map(|rgba| (width, height, rgba))
    }
}

#[cfg(not(windows))]
fn capture_primary_screen() -> Result<(u32, u32, Vec<u8>), String> {
    Err("screenshot backend is Windows-only".into())
}

fn encode_png(path: &PathBuf, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("png data: {e}"))
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn captures_real_screen_to_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = super::take_screenshot(dir.path().to_str().unwrap()).expect("screenshot");
        let bytes = std::fs::read(&path).expect("read png");
        assert!(bytes.len() > 1000);
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("Deckboard_") && name.ends_with(".png"),
            "{name}"
        );
    }

    #[test]
    fn empty_dir_falls_back_to_pictures() {
        // no file assertions - just make sure the fallback resolves and a
        // capture attempt does not panic; clean up the written file
        if let Ok(path) = super::take_screenshot("") {
            let _ = std::fs::remove_file(path);
        }
    }
}
