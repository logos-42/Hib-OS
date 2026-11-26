//! 干净文本渲染底座 —— 统一字距、行距与字号换算。
//!
//! 第一版复用现有 5×7 位图字形，以小字号（scale=1）排版实现简洁视觉；
//! 平滑字体（如 fontdue）可作为后续增强接入，调用方接口保持不变。

use super::surface::Surface;

pub const GLYPH_W: u32 = 5;
pub const GLYPH_H: u32 = 7;
pub const ADVANCE: u32 = 6;
pub const LINE_H: u32 = 8;

/// 把「像素字号」换算成位图字形的放大倍数。
pub fn scale_for(size: u32) -> u32 {
    (size / GLYPH_H).max(1)
}

/// 估算一行文本宽度（像素）。
pub fn measure_width(text: &str, size: u32) -> u32 {
    text.chars().count() as u32 * ADVANCE * scale_for(size)
}

/// 估算多行文本高度（像素）。
pub fn measure_height(lines: usize, size: u32) -> u32 {
    lines as u32 * LINE_H * scale_for(size)
}

/// 把一行文本渲染成独立表面（用于图标标签与顶栏信息）。
pub fn render_text(text: &str, size: u32, color: u32) -> Surface {
    let scale = scale_for(size);
    let w = measure_width(text, size).max(1);
    let h = GLYPH_H * scale;
    let mut s = Surface::new(w, h);
    s.draw_text(0, 0, text, color, scale);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_text_width_is_monotonic() {
        assert!(measure_width("AA", 14) > measure_width("A", 14));
    }

    #[test]
    fn render_text_creates_surface() {
        let s = render_text("HI", 14, 0xFFFFFFFF);
        assert!(s.width() >= 2);
        assert!(s.height() >= 1);
    }

    #[test]
    fn scale_for_small_size_is_one() {
        assert_eq!(scale_for(7), 1);
        assert_eq!(scale_for(1), 1);
    }

    #[test]
    fn measure_height_tracks_lines() {
        assert!(measure_height(3, 14) > measure_height(1, 14));
    }
}
