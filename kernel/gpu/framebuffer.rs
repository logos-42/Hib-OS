//! 1bpp 像素帧缓冲 + 5x7 位图字体光栅化。
//!
//! 这是图形栈的最底层：一块 `width × height` 的单色位图，按 64 位字打包存储。
//! 文字通过内置位图字体逐点写入，从而把「文本屏幕」真正变成「像素画面」。
//! 终端无法直接显示像素，因此提供 `preview()` 用半块字符做缩略图输出。

/// 字模宽度。
pub const FONT_WIDTH: usize = 5;
/// 字模高度。
pub const FONT_HEIGHT: usize = 7;
/// 字符步进 (含 1px 间距)。
pub const ADVANCE: usize = FONT_WIDTH + 1;
/// 行高 (含 1px 间距)。
pub const LINE_HEIGHT: usize = FONT_HEIGHT + 1;

/// 取字符的 5x7 位图；每行低 5 位有效 (bit4 = 最左)。
pub fn glyph(c: char) -> [u8; FONT_HEIGHT] {
    match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ',' => [0, 0, 0, 0, 0b01100, 0b01100, 0b01000],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        ';' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0b01000],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '*' => [0, 0b10101, 0b01110, 0b11111, 0b01110, 0b10101, 0],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '\\' => [0b10000, 0b01000, 0b01000, 0b00100, 0b00010, 0b00010, 0b00001],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        '>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '#' => [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010],
        '%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '\'' => [0b00100, 0b00100, 0, 0, 0, 0, 0],
        '"' => [0b01010, 0b01010, 0, 0, 0, 0, 0],
        ' ' => [0; FONT_HEIGHT],
        // 未知字符画成实心方块，便于发现缺字。
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}

/// 1bpp 帧缓冲。
pub struct Framebuffer {
    width: usize,
    height: usize,
    stride: usize,
    bits: Vec<u64>,
}

impl Framebuffer {
    pub fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(64).max(1);
        Self {
            width,
            height,
            stride,
            bits: vec![0; stride * height],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// 显存占用 (字节)。
    pub fn byte_len(&self) -> usize {
        self.bits.len() * 8
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let word = self.bits[y * self.stride + x / 64];
        (word >> (x % 64)) & 1 == 1
    }

    pub fn set(&mut self, x: usize, y: usize, on: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = y * self.stride + x / 64;
        let mask = 1u64 << (x % 64);
        if on {
            self.bits[idx] |= mask;
        } else {
            self.bits[idx] &= !mask;
        }
    }

    pub fn clear(&mut self) {
        self.bits.iter_mut().for_each(|w| *w = 0);
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        for yy in y..y.saturating_add(h).min(self.height) {
            for xx in x..x.saturating_add(w).min(self.width) {
                self.set(xx, yy, true);
            }
        }
    }

    pub fn frame_rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        if w == 0 || h == 0 {
            return;
        }
        for xx in x..x + w {
            self.set(xx, y, true);
            self.set(xx, y + h - 1, true);
        }
        for yy in y..y + h {
            self.set(x, yy, true);
            self.set(x + w - 1, yy, true);
        }
    }

    pub fn hline(&mut self, x0: usize, x1: usize, y: usize) {
        for x in x0..x1 {
            self.set(x, y, true);
        }
    }

    pub fn vline(&mut self, x: usize, y0: usize, y1: usize) {
        for y in y0..y1 {
            self.set(x, y, true);
        }
    }

    /// 画一个字符，返回绘制后下一个字符的起点 x。
    pub fn draw_char(&mut self, x: usize, y: usize, ch: char) -> usize {
        let rows = glyph(ch);
        for (dy, bits) in rows.iter().enumerate() {
            for dx in 0..FONT_WIDTH {
                if bits & (1 << (FONT_WIDTH - 1 - dx)) != 0 {
                    self.set(x + dx, y + dy, true);
                }
            }
        }
        x + ADVANCE
    }

    /// 画一串字符，返回结束 x。
    pub fn draw_text(&mut self, x: usize, y: usize, text: &str) -> usize {
        let mut cx = x;
        for ch in text.chars() {
            cx = self.draw_char(cx, y, ch);
        }
        cx
    }

    /// 亮起像素数量。
    pub fn lit_pixels(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// 缩略图：按最近邻采样，用半块字符输出 (每 2 像素高 = 1 行字符)。
    pub fn preview(&self, cols: usize, rows: usize) -> String {
        if cols == 0 || rows == 0 || self.width == 0 || self.height == 0 {
            return String::new();
        }
        let px_rows = rows * 2;
        let mut out = String::new();
        for cy in 0..rows {
            for cx in 0..cols {
                let top = self.sample(cx, cy * 2, cols, px_rows);
                let bottom = self.sample(cx, cy * 2 + 1, cols, px_rows);
                out.push(match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
            out.push('\n');
        }
        out
    }

    /// 带 ANSI 24 位真彩的半块字符缩略图。
    ///
    /// 每两行像素合并为一行字符：`█` 双亮，`▀` 上亮下灭，`▄` 上灭下亮，
    /// 空格全灭。前景/背景分别用上/下像素的颜色，适合直接输出到
    /// 支持真彩的终端作为「显示输出」预览。
    pub fn preview_ansi(&self, cols: usize, rows: usize) -> String {
        if cols == 0 || rows == 0 || self.width == 0 || self.height == 0 {
            return String::new();
        }
        // 猫咪拿铁 Mocha 配色：深色背景 + 蓝绿色磷光
        let on = (0x89, 0xb4, 0xfa); // 天蓝，点亮像素
        let off = (0x11, 0x11, 0x1b); // 极深底色，未点亮像素
        let surface = (0x31, 0x32, 0x44); // 表面色，空格区域
        let px_rows = rows * 2;
        let mut out = String::new();
        for cy in 0..rows {
            for cx in 0..cols {
                let top = self.sample(cx, cy * 2, cols, px_rows);
                let bottom = self.sample(cx, cy * 2 + 1, cols, px_rows);
                let (ch, fg, bg) = match (top, bottom) {
                    (true, true) => ('█', on, on),
                    (true, false) => ('▀', on, off),
                    (false, true) => ('▄', on, off),
                    (false, false) => (' ', surface, off),
                };
                if ch == ' ' {
                    out.push_str(&format!(
                        "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m \x1b[0m",
                        fg.0, fg.1, fg.2, bg.0, bg.1, bg.2
                    ));
                } else {
                    out.push_str(&format!(
                        "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m{}\x1b[0m",
                        fg.0, fg.1, fg.2, bg.0, bg.1, bg.2, ch
                    ));
                }
            }
            out.push('\n');
        }
        out
    }

    fn sample(&self, cx: usize, cy: usize, cols: usize, px_rows: usize) -> bool {
        let x = cx * self.width / cols;
        let y = cy * self.height / px_rows.max(1);
        self.get(x.min(self.width.saturating_sub(1)), y.min(self.height.saturating_sub(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_set_and_get() {
        let mut fb = Framebuffer::new(640, 200);
        assert_eq!(fb.width(), 640);
        assert_eq!(fb.height(), 200);
        assert!(!fb.get(10, 10));
        fb.set(10, 10, true);
        assert!(fb.get(10, 10));
        fb.set(10, 10, false);
        assert!(!fb.get(10, 10));
        // 越界安全
        fb.set(10_000, 10_000, true);
        assert!(!fb.get(10_000, 10_000));
        assert_eq!(fb.lit_pixels(), 0);
    }

    #[test]
    fn rect_and_lines() {
        let mut fb = Framebuffer::new(32, 32);
        fb.fill_rect(2, 2, 4, 3);
        assert_eq!(fb.lit_pixels(), 12);

        fb.clear();
        fb.frame_rect(0, 0, 10, 10);
        // 周长 4*10 - 4 = 36
        assert_eq!(fb.lit_pixels(), 36);

        fb.clear();
        fb.hline(0, 10, 5);
        fb.vline(5, 0, 10);
        assert_eq!(fb.lit_pixels(), 10 + 10 - 1);
    }

    #[test]
    fn glyphs_exist_for_ascii() {
        for c in 'A'..='Z' {
            let g = glyph(c);
            assert!(g.iter().any(|r| *r != 0), "字符 {} 字模为空", c);
        }
        for c in '0'..='9' {
            assert!(glyph(c).iter().any(|r| *r != 0));
        }
        assert!(glyph(' ').iter().all(|r| *r == 0));
        // 未知字符回落到方块
        assert!(glyph('中').iter().all(|r| *r == 0b11111 || *r == 0b10001));
    }

    #[test]
    fn text_rasterizes_into_pixels() {
        let mut fb = Framebuffer::new(100, 20);
        let end = fb.draw_text(0, 0, "HI");
        assert_eq!(end, 2 * ADVANCE);
        assert!(fb.lit_pixels() > 0);
        // H 的第一列与最后一列为实，中间列为空
        assert!(fb.get(0, 0));
        assert!(!fb.get(1, 0));
        assert!(fb.get(4, 0));
    }

    #[test]
    fn empty_preview_is_possible() {
        let fb = Framebuffer::new(10, 10);
        let p = fb.preview(5, 5);
        assert_eq!(p.lines().count(), 5);
        assert!(p.chars().all(|c| c == ' ' || c == '\n'));
    }

    #[test]
    fn preview_ansi_has_pixels_and_codes() {
        let mut fb = Framebuffer::new(10, 4);
        fb.set(0, 0, true);
        fb.set(0, 1, true);
        let p = fb.preview_ansi(5, 2);
        assert!(p.contains("\x1b[38;2;"));
        assert!(p.contains('█'));
        // 两个点亮像素 => '█' 前景/背景都是天蓝
        assert!(p.contains("\x1b[38;2;137;180;250m\x1b[48;2;137;180;250m█"));
    }

    #[test]
    fn draws_are_clipped() {
        let mut fb = Framebuffer::new(8, 8);
        fb.draw_text(4, 6, "XYZ");
        fb.fill_rect(6, 6, 100, 100);
        assert!(fb.lit_pixels() > 0);
    }
}
