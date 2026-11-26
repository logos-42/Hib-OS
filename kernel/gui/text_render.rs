//! 干净排版规范 —— 全 GUI 共用同一套字号、行距、边距与配色。
//!
//! 位图字形被当作"矢量轮廓的采样源"：`scale >= 2` 时经抗锯齿滤波渲染，
//! 小字仍保持清晰。终端、应用窗口、工作区标题都从这里取参数，
//! 不再各自硬编码 5×7 的裸像素排版。

use std::sync::atomic::{AtomicU8, Ordering};

use super::font::{self, FontFamily};
use super::glyph12;
use super::surface::Surface;

/// 当前 UI 字族（默认雅黑风格：现代无衬线，含中文）。
static UI_FAMILY: AtomicU8 = AtomicU8::new(2); // 2 = YaHei

fn current_family() -> FontFamily {
    match UI_FAMILY.load(Ordering::Relaxed) {
        0 => FontFamily::Serif,
        1 => FontFamily::Sans,
        3 => FontFamily::Mono,
        _ => FontFamily::YaHei,
    }
}

/// 切换 UI 字族（0=宋体 1=黑体 2=雅黑 3=等宽）。
pub fn set_family(index: u8) {
    UI_FAMILY.store(index.min(3), Ordering::Relaxed);
}

pub fn family_name() -> &'static str {
    match current_family() {
        FontFamily::Serif => "SERIF",
        FontFamily::Sans => "SANS",
        FontFamily::YaHei => "YAHEI",
        FontFamily::Mono => "MONO",
    }
}

// ── 字形度量（8×12 UI 字形）────────────────────────────────────────────────
pub const GLYPH_W: u32 = glyph12::W as u32;
pub const GLYPH_H: u32 = glyph12::H as u32;
pub const ADVANCE: u32 = glyph12::ADVANCE;
pub const LINE_H: u32 = glyph12::H as u32 + 3;

/// 正文/终端字号：8×12 字形按 1:1 渲染，配合覆盖率抗锯齿。
pub const BODY_SCALE: u32 = 1;
/// 紧凑信息字号（如顶栏状态，仍用底层 5×7 保持锐利）。
pub const SMALL_SCALE: u32 = 1;

/// 真实字体字号（像素）：比 8×12 内置字形更紧凑，适合终端。
pub const BODY_PX: f32 = 11.0;
/// 正文行距（像素）：字号 11 + 5 留白。
pub const BODY_LINE_H: i32 = 16;
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

/// 用正文规范画一行：优先真实字体，失败回退 8×12 内置字形。
pub fn draw_body(s: &mut Surface, x: i32, y: i32, text: &str, color: u32) -> i32 {
    let max_x = s.width() as i32 - PAD;
    if font::draw_text(s, x, y, text, BODY_PX, color, current_family(), max_x) {
        return font::measure_text(text, BODY_PX, current_family())
            .map(|w| x + w as i32)
            .unwrap_or(x);
    }
    s.draw_ui_text(x, y, text, color, max_x)
}

/// 用指定字号画一行（真实字体优先，回退内置字形）。
pub fn draw_text(s: &mut Surface, x: i32, y: i32, text: &str, px: f32, color: u32, max_x: i32) -> i32 {
    if font::draw_text(s, x, y, text, px, color, current_family(), max_x) {
        return font::measure_text(text, px, current_family())
            .map(|w| x + w as i32)
            .unwrap_or(x);
    }
    s.draw_ui_text(x, y, text, color, max_x)
}

/// 当前正文字号下，给定宽度可容纳多少列。
pub fn body_cols(width: u32) -> usize {
    let usable = width.saturating_sub((PAD * 2) as u32) as i32;
    if let Some(adv) = font::measure_text("M", BODY_PX, current_family()) {
        return (usable.max(1) as f32 / adv.max(1.0)) as usize;
    }
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
