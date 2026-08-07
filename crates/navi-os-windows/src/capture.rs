//! Screen capture via GDI `BitBlt` + `GetDIBits`.
//!
//! Saves a 32-bit BMP file (no external image crate needed). The BMP format
//! with 32-bit BGRA has naturally 4-byte-aligned rows, so no padding is
//! required. `view_image` supports `image/bmp`.

use super::WinScreenshot;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

/// Captures the primary monitor to a BMP file under `out_dir`.
///
/// Returns metadata about the saved screenshot. The file path is suitable for
/// passing to the `view_image` tool so the model can see the screen.
pub fn capture_screen(out_dir: &str) -> Result<WinScreenshot> {
    let out_dir = Path::new(out_dir);
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create screenshot dir: {}", out_dir.display()))?;

    unsafe {
        // 1. Get the screen DC and dimensions.
        let screen_dc = GetDC(std::ptr::null_mut());
        if screen_dc.is_null() {
            bail!("GetDC(NULL) returned NULL");
        }

        let width = GetSystemMetrics(SM_CXSCREEN);
        let height = GetSystemMetrics(SM_CYSCREEN);
        if width <= 0 || height <= 0 {
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            bail!("GetSystemMetrics returned invalid screen dimensions: {width}x{height}");
        }
        let width = width as u32;
        let height = height as u32;

        // 2. Create a compatible memory DC + bitmap.
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            bail!("CreateCompatibleDC failed");
        }

        let bmp = CreateCompatibleBitmap(screen_dc, width as i32, height as i32);
        if bmp.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            bail!("CreateCompatibleBitmap failed");
        }

        let old_obj = SelectObject(mem_dc, bmp);

        // 3. BitBlt the screen into the memory bitmap.
        let ok = BitBlt(
            mem_dc,
            0,
            0,
            width as i32,
            height as i32,
            screen_dc,
            0,
            0,
            SRCCOPY,
        );
        if ok == 0 {
            SelectObject(mem_dc, old_obj);
            DeleteObject(bmp);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            bail!("BitBlt failed");
        }

        // 4. Extract pixel data via GetDIBits (32-bit BGRA, top-down).
        let pixel_count = (width as usize) * (height as usize);
        let mut pixels = vec![0u8; pixel_count * 4];

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // negative = top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };

        let lines = GetDIBits(
            mem_dc,
            bmp,
            0,
            height,
            pixels.as_mut_ptr() as *mut _,
            &mut bi,
            DIB_RGB_COLORS,
        );
        if lines == 0 {
            SelectObject(mem_dc, old_obj);
            DeleteObject(bmp);
            DeleteDC(mem_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
            bail!("GetDIBits returned 0 lines");
        }

        // 5. Cleanup GDI handles.
        SelectObject(mem_dc, old_obj);
        DeleteObject(bmp);
        DeleteDC(mem_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);

        // 6. Encode BMP file.
        let bmp_bytes = encode_bmp(width, height, &pixels);

        // 7. Write to file with a timestamped name.
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let filename = format!("screenshot_{timestamp}.bmp");
        let filepath = out_dir.join(filename);
        fs::write(&filepath, &bmp_bytes)
            .with_context(|| format!("failed to write BMP: {}", filepath.display()))?;

        Ok(WinScreenshot {
            path: filepath.to_string_lossy().replace('\\', "/"),
            width,
            height,
            size_bytes: bmp_bytes.len() as u64,
        })
    }
}

/// Encodes 32-bit BGRA top-down pixels into a BMP file (BITMAPFILEHEADER +
/// BITMAPINFOHEADER + pixel data). No row padding needed at 32 bpp.
fn encode_bmp(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let pixel_data_size = pixels.len();
    let header_size = 14 + 40; // BITMAPFILEHEADER + BITMAPINFOHEADER
    let file_size = header_size + pixel_data_size;

    let mut out = Vec::with_capacity(file_size);
    // BITMAPFILEHEADER (14 bytes)
    out.extend_from_slice(b"BM"); // signature
    out.extend_from_slice(&(file_size as u32).to_le_bytes()); // file size
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved1
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved2
    out.extend_from_slice(&(header_size as u32).to_le_bytes()); // pixel data offset

    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes()); // header size
    out.extend_from_slice(&(width as i32).to_le_bytes()); // width
    out.extend_from_slice(&(height as i32).to_le_bytes()); // height (positive = bottom-up in file)
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // compression (BI_RGB)
    out.extend_from_slice(&(pixel_data_size as u32).to_le_bytes()); // image size
    out.extend_from_slice(&0u32.to_le_bytes()); // x ppm
    out.extend_from_slice(&0u32.to_le_bytes()); // y ppm
    out.extend_from_slice(&0u32.to_le_bytes()); // colors used
    out.extend_from_slice(&0u32.to_le_bytes()); // important colors

    // Pixel data: our pixels are top-down BGRA. BMP stores bottom-up, so
    // reverse the row order.
    let row_size = (width as usize) * 4;
    for row in (0..height as usize).rev() {
        let start = row * row_size;
        out.extend_from_slice(&pixels[start..start + row_size]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── encode_bmp unit tests ─────────────────────────────────────────────

    #[test]
    fn encode_bmp_produces_valid_header() {
        let width = 2u32;
        let height = 2u32;
        // 4 pixels, 4 bytes each (BGRA)
        let pixels = vec![0u8; 16];
        let bmp = encode_bmp(width, height, &pixels);
        assert_eq!(&bmp[0..2], b"BM");
        // File size = 14 + 40 + 16 = 70
        let file_size = u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]);
        assert_eq!(file_size, 70);
        // Pixel data offset = 54
        let offset = u32::from_le_bytes([bmp[10], bmp[11], bmp[12], bmp[13]]);
        assert_eq!(offset, 54);
        // Header size at offset 14 = 40
        let hsize = u32::from_le_bytes([bmp[14], bmp[15], bmp[16], bmp[17]]);
        assert_eq!(hsize, 40);
        // Width at offset 18
        let w = i32::from_le_bytes([bmp[18], bmp[19], bmp[20], bmp[21]]);
        assert_eq!(w, 2);
        // Height at offset 22
        let h = i32::from_le_bytes([bmp[22], bmp[23], bmp[24], bmp[25]]);
        assert_eq!(h, 2);
        // bpp at offset 28
        let bpp = u16::from_le_bytes([bmp[28], bmp[29]]);
        assert_eq!(bpp, 32);
    }

    #[test]
    fn encode_bmp_1x1_pixel() {
        let pixels = vec![0xFF, 0x00, 0x00, 0xFF]; // 1 pixel, BGRA
        let bmp = encode_bmp(1, 1, &pixels);
        assert_eq!(&bmp[0..2], b"BM");
        // File size = 14 + 40 + 4 = 58
        let file_size = u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]);
        assert_eq!(file_size, 58);
        // Width = 1
        let w = i32::from_le_bytes([bmp[18], bmp[19], bmp[20], bmp[21]]);
        assert_eq!(w, 1);
        // Height = 1
        let h = i32::from_le_bytes([bmp[22], bmp[23], bmp[24], bmp[25]]);
        assert_eq!(h, 1);
        // Pixel data starts at offset 54, should be the 4 bytes we passed.
        assert_eq!(&bmp[54..58], &[0xFF, 0x00, 0x00, 0xFF]);
    }

    #[test]
    fn encode_bmp_pixel_data_is_bottom_up() {
        // 2 rows × 2 cols × 4 bytes = 16 bytes.
        // Row 0 (top): all red (BGRA: 0,0,255,0)
        // Row 1 (bottom): all blue (BGRA: 255,0,0,0)
        let mut pixels = vec![0u8; 16];
        // Top row (row 0): red
        pixels[0..4].copy_from_slice(&[0, 0, 255, 0]);
        pixels[4..8].copy_from_slice(&[0, 0, 255, 0]);
        // Bottom row (row 1): blue
        pixels[8..12].copy_from_slice(&[255, 0, 0, 0]);
        pixels[12..16].copy_from_slice(&[255, 0, 0, 0]);

        let bmp = encode_bmp(2, 2, &pixels);
        // BMP stores bottom-up, so the first pixel row in the file
        // should be the bottom row (blue).
        let pixel_start = 54;
        assert_eq!(
            &bmp[pixel_start..pixel_start + 4],
            &[255, 0, 0, 0],
            "first pixel row in BMP should be bottom (blue)"
        );
        // Second row in file should be top (red).
        assert_eq!(
            &bmp[pixel_start + 8..pixel_start + 12],
            &[0, 0, 255, 0],
            "second pixel row in BMP should be top (red)"
        );
    }

    #[test]
    fn encode_bmp_large_dimensions() {
        // 4K screenshot: 3840×2160
        let width = 3840u32;
        let height = 2160u32;
        let pixel_count = (width as usize) * (height as usize);
        let pixels = vec![0u8; pixel_count * 4];
        let bmp = encode_bmp(width, height, &pixels);
        assert_eq!(&bmp[0..2], b"BM");
        let file_size = u32::from_le_bytes([bmp[2], bmp[3], bmp[4], bmp[5]]);
        // 14 + 40 + (3840 * 2160 * 4)
        let expected = 14 + 40 + (3840 * 2160 * 4);
        assert_eq!(file_size as usize, expected);
        let w = i32::from_le_bytes([bmp[18], bmp[19], bmp[20], bmp[21]]);
        assert_eq!(w, 3840);
        let h = i32::from_le_bytes([bmp[22], bmp[23], bmp[24], bmp[25]]);
        assert_eq!(h, 2160);
    }

    #[test]
    fn encode_bmp_compression_is_bi_rgb() {
        let pixels = vec![0u8; 4];
        let bmp = encode_bmp(1, 1, &pixels);
        // Compression at offset 30 (after header start at 14 + 16 bytes)
        let compression = u32::from_le_bytes([bmp[30], bmp[31], bmp[32], bmp[33]]);
        assert_eq!(compression, 0, "BI_RGB = 0");
    }

    #[test]
    fn encode_bmp_planes_is_one() {
        let pixels = vec![0u8; 4];
        let bmp = encode_bmp(1, 1, &pixels);
        // Planes at offset 26
        let planes = u16::from_le_bytes([bmp[26], bmp[27]]);
        assert_eq!(planes, 1);
    }

    // ── capture_screen error path tests ───────────────────────────────────
    //
    // These test error paths that fail before reaching GDI calls.
    // They should work without a desktop.

    #[test]
    #[cfg(windows)]
    fn capture_screen_invalid_dir_errors() {
        // A path with a null byte is invalid on Windows.
        let result = capture_screen("invalid\0path");
        assert!(result.is_err(), "invalid path should error");
    }

    #[test]
    #[cfg(windows)]
    fn capture_screen_writable_temp_dir_succeeds_or_skips() {
        let tmp = std::env::temp_dir().join("navi_capture_test");
        let result = capture_screen(tmp.to_str().unwrap());
        match result {
            Ok(screenshot) => {
                assert!(screenshot.width > 0, "width should be positive");
                assert!(screenshot.height > 0, "height should be positive");
                assert!(screenshot.size_bytes > 0, "size should be positive");
                assert!(
                    screenshot.path.contains("screenshot_"),
                    "path should contain screenshot_, got: {}",
                    screenshot.path
                );
                // Clean up.
                let _ = std::fs::remove_file(&screenshot.path);
            }
            Err(e) => {
                eprintln!("skipping capture_screen_writable: {e}");
            }
        }
        // Clean up temp dir.
        let _ = std::fs::remove_dir(&tmp);
    }
}
