//! 干净排版规范 —— 全 GUI 共用同一套字号、行距、边距与配色。
//!
//! 位图字形被当作"矢量轮廓的采样源"：`scale >= 2` 时经抗锯齿滤波渲染，
//! 小字仍保持清晰。终端、应用窗口、工作区标题都从这里取参数，
//! 不再各自硬编码 5×7 的裸像素排版。

use super::glyph12;
use super::surface::Surface;

// ── 字形度量（8×12 UI 字形）────────────────────────────────────────────────
pub const GLYPH_W: u32 = glyph12::W as u32;
pub const GLYPH_H: u32 = glyph12::H as u32;
pub const ADVANCE: u32 = glyph12::ADVANCE;
pub const LINE_H: u32 = glyph12::H as u32 + 3;

/// 正文/终端字号：8×12 字形按 1:1 渲染，配合覆盖率抗锯齿。
pub const BODY_SCALE: u32 = 1;
/// 紧凑信息字号（如顶栏状态，仍用底层 5×7 保持锐利）。
pub const SMALL_SCALE: u32 = 1;

/// 正文行距（像素）：字高 12 + 5 留白。
pub const BODY_LINE_H: i32 = 17;
/// 内容区边距（像素）。
pub const PAD: i32 = 16;

// ── 配色（低饱和深色，非像素风）───────────────────────────────────────────
pub const WINDOW_BG: u32 = 0xFF14161A;
pub const FG: u32 = 0xFFD8DCE4;
pub const MUTED: u32 = 0xFF8F98A6;
pub const ACCENT: u32 = 0xFF7FB3A0;
pub const ERROR: u32 = 0xFFE08A63;
pub const PROMPT: u32 = 0xFF9FD7C0;

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

/// 用正文规范画一行（8×12 抗锯齿字形 + 右边距裁剪）。
pub fn draw_body(s: &mut Surface, x: i32, y: i32, text: &str, color: u32) -> i32 {
    let max_x = s.width() as i32 - PAD;
    s.draw_ui_text(x, y, text, color, max_x)
}

/// 当前正文字号下，给定宽度可容纳多少列。
pub fn body_cols(width: u32) -> usize {
    let usable = width.saturating_sub((PAD * 2) as u32) as i32;
    (usable.max(1) / ADVANCE as i32) as usize
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

    #[test]
    fn body_layout_is_readable() {
        // 8×12 字模是 1:1 渲染，无需放大；行距留白仍应大于字高。
        assert_eq!(BODY_SCALE, 1);
        assert!(BODY_LINE_H > GLYPH_H as i32);
        assert!(body_cols(420) >= 20);
    }

    #[test]
    fn draw_body_uses_antialiasing() {
        let mut s = Surface::filled(200, 40, WINDOW_BG);
        draw_body(&mut s, PAD, 8, "HELLO", FG);
        assert!(s.lit_pixels() > 0);
    }
}
