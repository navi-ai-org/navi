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
}
