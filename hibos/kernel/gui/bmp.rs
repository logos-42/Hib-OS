//! BMP 导出 —— 把 32bpp 表面写成标准 24 位 BMP 文件。
//!
//! 选择 BMP 的原因：零依赖、Windows 资源管理器直接可看、格式足够简单到
//! 可以手写并测试每一个字节。这是「GUI 真的画出来了」的最直接证据。

use std::fs;
use std::io;
use std::path::Path;

use super::surface::{blue_of, green_of, red_of, Surface};

/// 编码为 24 位 BMP 字节流（BITMAPINFOHEADER, BI_RGB, 行自底向上）。
pub fn encode(surface: &Surface) -> Vec<u8> {
    let w = surface.width();
    let h = surface.height();
    let row_bytes = (w as usize * 3).div_ceil(4) * 4;
    let pixel_bytes = row_bytes * h as usize;
    let file_size = 14 + 40 + pixel_bytes;

    let mut out = Vec::with_capacity(file_size);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved1
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved2
    out.extend_from_slice(&(54u32).to_le_bytes()); // pixel data offset

    // BITMAPINFOHEADER
    out.extend_from_slice(&(40u32).to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // compression = BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // x ppm (~72dpi)
    out.extend_from_slice(&2835i32.to_le_bytes()); // y ppm
    out.extend_from_slice(&0u32.to_le_bytes()); // palette colors
    out.extend_from_slice(&0u32.to_le_bytes()); // important colors

    // 像素：BGR，自底向上，每行 4 字节对齐
    let padding = row_bytes - w as usize * 3;
    for y in (0..h).rev() {
        for x in 0..w {
            let p = surface.get(x as i32, y as i32).unwrap_or(0);
            out.push(blue_of(p));
            out.push(green_of(p));
            out.push(red_of(p));
        }
        for _ in 0..padding {
            out.push(0);
        }
    }
    out
}

/// 写入文件，返回字节数。
pub fn save(surface: &Surface, path: impl AsRef<Path>) -> io::Result<usize> {
    let bytes = encode(surface);
    if let Some(parent) = path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, &bytes)?;
    Ok(bytes.len())
}

/// 从 BMP 字节流里读回宽高（仅用于验证导出正确）。
pub fn decode_header(bytes: &[u8]) -> Option<(u32, u32, u16, usize)> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return None;
    }
    let w = i32::from_le_bytes(bytes[18..22].try_into().ok()?);
    let h = i32::from_le_bytes(bytes[22..26].try_into().ok()?);
    let bpp = u16::from_le_bytes(bytes[28..30].try_into().ok()?);
    let size = u32::from_le_bytes(bytes[2..6].try_into().ok()?) as usize;
    Some((w as u32, h as u32, bpp, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::{palette, rgb};

    #[test]
    fn header_is_valid() {
        let s = Surface::filled(3, 2, rgb(255, 0, 0));
        let bytes = encode(&s);
        let (w, h, bpp, size) = decode_header(&bytes).unwrap();
        assert_eq!((w, h, bpp), (3, 2, 24));
        assert_eq!(size, bytes.len());
        // 每行 9 字节 → 补到 12，共 24 字节像素数据
        assert_eq!(bytes.len(), 54 + 24);
    }

    #[test]
    fn pixel_order_is_bgr_bottom_up() {
        let mut s = Surface::new(2, 2);
        s.put(0, 0, rgb(1, 2, 3)); // 左上
        s.put(1, 1, rgb(4, 5, 6)); // 右下
        let bytes = encode(&s);
        // 第一行像素数据对应图像最后一行（y=1），x=0 是黑色
        let row0 = 54;
        assert_eq!(&bytes[row0..row0 + 3], &[0, 0, 0]);
        assert_eq!(&bytes[row0 + 3..row0 + 6], &[6, 5, 4]);
        let row1 = row0 + 8; // 2*3=6 → 补 2 字节 = 8
        assert_eq!(&bytes[row1..row1 + 3], &[3, 2, 1]);
    }

    #[test]
    fn save_writes_file() {
        let dir = std::env::temp_dir().join("hibos-gui-test");
        let path = dir.join("surface.bmp");
        let s = Surface::filled(16, 16, palette::BLUE);
        let n = save(&s, &path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len() as usize, n);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(decode_header(&bytes).unwrap().0, 16);
    }

    #[test]
    fn rejects_non_bmp() {
        assert!(decode_header(b"not a bmp at all").is_none());
    }
}
