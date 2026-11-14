//! 32bpp ARGB 表面 —— 图形栈真正的像素层。
//!
//! 与 `gpu::framebuffer` 的 1bpp 单色位图不同，这里是每个像素 32 位的真彩表面，
//! 支持 alpha 混合、矩形填充、文本光栅化与 BMP 导出。窗口、合成器、壁纸
//! 都建立在这一层之上。
//!
//! 像素格式为 `0xAARRGGBB`（与 Windows GDI / softbuffer 的 0x00RRGGBB 兼容，
//! 高位 alpha 仅在本模块内部用于混合）。

use crate::gpu::framebuffer::{glyph, ADVANCE, FONT_WIDTH, LINE_HEIGHT};

/// 打包一个 ARGB 像素。
#[inline]
pub const fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// 不透明颜色。
#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    argb(0xff, r, g, b)
}

#[inline]
pub fn alpha_of(c: u32) -> u8 {
    (c >> 24) as u8
}
#[inline]
pub fn red_of(c: u32) -> u8 {
    (c >> 16) as u8
}
#[inline]
pub fn green_of(c: u32) -> u8 {
    (c >> 8) as u8
}
#[inline]
pub fn blue_of(c: u32) -> u8 {
    c as u8
}

/// Catppuccin Mocha 调色板（与文本桌面保持一致）。
pub mod palette {
    use super::rgb;

    pub const BASE: u32 = rgb(0x11, 0x11, 0x1b);
    pub const MANTLE: u32 = rgb(0x18, 0x18, 0x25);
    pub const CRUST: u32 = rgb(0x0b, 0x0b, 0x12);
    pub const SURFACE0: u32 = rgb(0x31, 0x32, 0x44);
    pub const SURFACE1: u32 = rgb(0x45, 0x47, 0x5a);
    pub const OVERLAY0: u32 = rgb(0x6c, 0x70, 0x86);
    pub const OVERLAY1: u32 = rgb(0x7f, 0x84, 0x9c);
    pub const TEXT: u32 = rgb(0xcd, 0xd6, 0xf4);
    pub const SUBTEXT0: u32 = rgb(0xa6, 0xad, 0xc8);
    pub const RED: u32 = rgb(0xf3, 0x8b, 0xa8);
    pub const GREEN: u32 = rgb(0xa6, 0xe3, 0xa1);
    pub const YELLOW: u32 = rgb(0xf9, 0xe2, 0xaf);
    pub const BLUE: u32 = rgb(0x89, 0xb4, 0xfa);
    pub const MAUVE: u32 = rgb(0xcb, 0xa6, 0xf7);
    pub const PINK: u32 = rgb(0xf5, 0xc2, 0xe7);
    pub const TEAL: u32 = rgb(0x94, 0xe2, 0xd5);
    pub const PEACH: u32 = rgb(0xfa, 0xb3, 0x87);
}

/// 一个矩形区域（屏幕坐标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && py >= self.y
            && px < self.x + self.w as i32
            && py < self.y + self.h as i32
    }

    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    /// 与另一个矩形求交。
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        Rect {
            x,
            y,
            w: (r - x).max(0) as u32,
            h: (b - y).max(0) as u32,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// 每像素 32 位的绘图表面。
#[derive(Clone)]
pub struct Surface {
    width: u32,
    height: u32,
    pixels: Vec<u32>,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize)],
        }
    }

    pub fn filled(width: u32, height: u32, color: u32) -> Self {
        let mut s = Self::new(width, height);
        s.fill(color);
        s
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn bytes(&self) -> usize {
        self.pixels.len() * 4
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut [u32] {
        &mut self.pixels
    }

    /// 转成 softbuffer / Win32 需要的 `0x00RRGGBB`。
    pub fn to_xrgb(&self) -> Vec<u32> {
        self.pixels
            .iter()
            .map(|p| p & 0x00ff_ffff)
            .collect()
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(self.pixels[(y as usize) * (self.width as usize) + (x as usize)])
    }

    /// 直接写入（不做混合）。越界忽略。
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.pixels[(y as usize) * (self.width as usize) + (x as usize)] = color;
    }

    /// alpha 混合写入（源覆盖目标）。
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, src: u32) {
        let a = alpha_of(src);
        if a == 0 {
            return;
        }
        if a == 255 {
            self.put(x, y, src);
            return;
        }
        let Some(dst) = self.get(x, y) else { return };
        let sa = a as u32;
        let da = alpha_of(dst) as u32;
        // 非预乘 alpha 的 source-over
        let blend = |s: u32, d: u32| -> u8 {
            let num = s * sa + d * da * (255 - sa) / 255;
            let den = sa + da * (255 - sa) / 255;
            if den == 0 {
                0
            } else {
                (num / den).min(255) as u8
            }
        };
        let r = blend(red_of(src) as u32, red_of(dst) as u32);
        let g = blend(green_of(src) as u32, green_of(dst) as u32);
        let b = blend(blue_of(src) as u32, blue_of(dst) as u32);
        let out_a = (sa + da * (255 - sa) / 255).min(255) as u8;
        self.put(x, y, argb(out_a, r, g, b));
    }

    pub fn fill(&mut self, color: u32) {
        self.pixels.iter_mut().for_each(|p| *p = color);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w as i32).min(self.width as i32);
        let y1 = (y + h as i32).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let a = alpha_of(color);
        for yy in y0..y1 {
            let row = (yy as usize) * (self.width as usize);
            for xx in x0..x1 {
                let idx = row + xx as usize;
                if a == 255 {
                    self.pixels[idx] = color;
                } else if a != 0 {
                    let dst = self.pixels[idx];
                    self.pixels[idx] = blend_pixel(color, dst);
                }
            }
        }
    }

    /// `put` 的别名：供合成器等上层模块按「写像素」语义调用。
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, color: u32) {
        self.put(x, y, color);
    }

    /// 画矩形边框，`thickness` 为线宽（0 按 1 处理）。
    pub fn draw_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32, thickness: u32) {
        let t = thickness.max(1);
        if w == 0 || h == 0 {
            return;
        }
        // 上、下两条边。
        self.fill_rect(x, y, w, t, color);
        if h > t {
            self.fill_rect(x, y + h as i32 - t as i32, w, t, color);
        }
        // 左、右两条边。
        self.fill_rect(x, y, t, h, color);
        if w > t {
            self.fill_rect(x + w as i32 - t as i32, y, t, h, color);
        }
    }

    /// 画一条线段（Bresenham，整数坐标）。
    pub fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
        let dx = (x2 - x1).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let dy = -(y2 - y1).abs();
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut err = dx + dy;
        let (mut x, mut y) = (x1, y1);
        loop {
            self.blend(x, y, color);
            if x == x2 && y == y2 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// 画圆环（中点画圆，`thickness` 为线宽）。
    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, color: u32, thickness: u32) {
        let t = thickness.max(1) as i32;
        if r <= 0 {
            return;
        }
        if t == 1 {
            // 单像素圆环：八分对称绘制。
            let mut x = r;
            let mut y = 0;
            let mut err = 1 - r;
            while x >= y {
                for &(px, py) in &[
                    (x, y),
                    (y, x),
                    (-y, x),
                    (-x, y),
                    (-x, -y),
                    (-y, -x),
                    (y, -x),
                    (x, -y),
                ] {
                    self.put(cx + px, cy + py, color);
                }
                y += 1;
                if err < 0 {
                    err += 2 * y + 1;
                } else {
                    x -= 1;
                    err += 2 * (y - x) + 1;
                }
            }
            return;
        }
        // 粗线：在内、外半径之间填充像素。
        let outer = r + t / 2;
        let inner = (r - t / 2).max(0);
        for y in -outer..=outer {
            for x in -outer..=outer {
                let d2 = x * x + y * y;
                if d2 <= outer * outer && d2 >= inner * inner {
                    self.blend(cx + x, cy + y, color);
                }
            }
        }
    }

    /// 填充实心圆。
    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        if r <= 0 {
            return;
        }
        for y in -r..=r {
            let dx = (((r * r - y * y) as f64).sqrt()) as i32;
            self.fill_rect(cx - dx, cy + y, (dx * 2 + 1) as u32, 1, color);
        }
    }

    /// 画 1px 边框。
    pub fn stroke_rect(&mut self, rect: Rect, color: u32) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.fill_rect(rect.x, rect.y, rect.w, 1, color);
        self.fill_rect(rect.x, rect.bottom() - 1, rect.w, 1, color);
        self.fill_rect(rect.x, rect.y, 1, rect.h, color);
        self.fill_rect(rect.right() - 1, rect.y, 1, rect.h, color);
    }

    /// 圆角矩形（逐行内缩，无依赖）。
    pub fn fill_round_rect(&mut self, rect: Rect, radius: u32, color: u32) {
        let r = radius.min(rect.w / 2).min(rect.h / 2).max(1);
        for row in 0..rect.h {
            // 距离上/下圆角中心的垂直偏移；中间行偏移为 0（满宽）。
            let dy = if row < r {
                (r - row) as f32
            } else if row >= rect.h - r {
                (row - (rect.h - r)) as f32
            } else {
                0.0
            };
            let inset = if dy == 0.0 {
                0
            } else {
                let rr = (r * r) as f32;
                let d = (rr - dy * dy).max(0.0).sqrt();
                (r as f32 - d).round().max(0.0) as u32
            };
            self.fill_rect(
                rect.x + inset as i32,
                rect.y + row as i32,
                rect.w.saturating_sub(inset * 2),
                1,
                color,
            );
        }
    }

    /// 把另一个表面混合到 `(dx, dy)`。
    pub fn draw_surface(&mut self, src: &Surface, dx: i32, dy: i32) {
        for sy in 0..src.height as i32 {
            for sx in 0..src.width as i32 {
                let px = dx + sx;
                let py = dy + sy;
                if px < 0 || py < 0 || px >= self.width as i32 || py >= self.height as i32 {
                    continue;
                }
                let s = src.pixels[(sy as usize) * (src.width as usize) + (sx as usize)];
                self.blend(px, py, s);
            }
        }
    }

    /// 不透明拷贝一块区域（合成器内部用）。
    pub fn copy_from(&mut self, src: &Surface, sx: i32, sy: i32, dx: i32, dy: i32, w: u32, h: u32) {
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                if let Some(px) = src.get(sx + col, sy + row) {
                    self.put(dx + col, dy + row, px);
                }
            }
        }
    }

    /// 画一个字符，返回下一个字符的 x。
    pub fn draw_char(&mut self, x: i32, y: i32, ch: char, color: u32, scale: u32) -> i32 {
        let rows = glyph(ch);
        let scale = scale.max(1) as i32;
        for (dy, bits) in rows.iter().enumerate() {
            for dx in 0..FONT_WIDTH {
                if bits & (1 << (FONT_WIDTH - 1 - dx)) != 0 {
                    self.fill_rect(
                        x + dx as i32 * scale,
                        y + dy as i32 * scale,
                        scale as u32,
                        scale as u32,
                        color,
                    );
                }
            }
        }
        x + ADVANCE as i32 * scale
    }

    /// 画一行字符（**不换行**，超出表面宽度自动裁剪），返回结束 x。
    ///
    /// 换行由调用方通过 [`wrap_text`] 显式完成 —— 之前这里是隐式换行，
    /// 结果调用方按「一行一个 y」推进时，折行文字会画到下一行文字上，
    /// 造成真实可见的重叠。把换行权交给调用方，布局才可预测。
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32, scale: u32) -> i32 {
        let mut cx = x;
        for ch in text.chars() {
            if ch == '\n' {
                break;
            }
            if cx + FONT_WIDTH as i32 * scale.max(1) as i32 > self.width as i32 {
                break;
            }
            cx = self.draw_char(cx, y, ch, color, scale);
        }
        cx
    }

    /// 画一行字符，限制右边界 `max_x`（窗口客户端内绘制用）。
    pub fn draw_text_clipped(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: u32,
        scale: u32,
        max_x: i32,
    ) -> i32 {
        let mut cx = x;
        let adv = ADVANCE as i32 * scale.max(1) as i32;
        for ch in text.chars() {
            if ch == '\n' || cx + adv > max_x {
                break;
            }
            cx = self.draw_char(cx, y, ch, color, scale);
        }
        cx
    }

    /// 按可见列数把一行文本折成多行。
    ///
    /// 优先在空格处折行（保留单词完整），超长单词才硬切。
    pub fn wrap_text(text: &str, cols: usize, scale: u32) -> Vec<String> {
        let cols = cols.max(1);
        let _ = scale;
        if text.is_empty() {
            return vec![String::new()];
        }
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for word in text.split_inclusive(' ') {
            let wlen = word.chars().count();
            if cur_len + wlen <= cols {
                cur.push_str(word);
                cur_len += wlen;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            if wlen <= cols {
                cur.push_str(word);
                cur_len = wlen;
            } else {
                for ch in word.chars() {
                    if cur_len == cols {
                        lines.push(std::mem::take(&mut cur));
                        cur_len = 0;
                    }
                    cur.push(ch);
                    cur_len += 1;
                }
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    }

    /// 一行能放多少个字符。
    pub fn cols_for_width(width: u32, scale: u32) -> usize {
        ((width as i32).max(1) / (ADVANCE as i32 * scale.max(1) as i32)) as usize
    }

    /// 文本宽度（像素）。
    pub fn text_width(text: &str, scale: u32) -> u32 {
        text.chars().count() as u32 * ADVANCE as u32 * scale.max(1)
    }

    /// 文本高度（像素）。
    pub fn text_height(lines: usize, scale: u32) -> u32 {
        lines as u32 * LINE_HEIGHT as u32 * scale.max(1)
    }

    /// 亮像素计数（用于验证真的有内容被画出来）。
    pub fn lit_pixels(&self) -> usize {
        self.pixels
            .iter()
            .filter(|p| **p != 0 && **p & 0x00ff_ffff != 0)
            .count()
    }

    /// 竖直渐变（壁纸用）。
    pub fn vertical_gradient(&mut self, top: u32, bottom: u32) {
        if self.height == 0 {
            return;
        }
        for y in 0..self.height {
            let t = y as f32 / (self.height.max(2) - 1) as f32;
            let lerp = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
            let color = rgb(
                lerp(red_of(top), red_of(bottom)),
                lerp(green_of(top), green_of(bottom)),
                lerp(blue_of(top), blue_of(bottom)),
            );
            self.fill_rect(0, y as i32, self.width, 1, color);
        }
    }
}

#[inline]
fn blend_pixel(src: u32, dst: u32) -> u32 {
    let sa = alpha_of(src) as u32;
    let da = alpha_of(dst) as u32;
    let blend = |s: u32, d: u32| -> u8 {
        let num = s * sa + d * da * (255 - sa) / 255;
        let den = sa + da * (255 - sa) / 255;
        if den == 0 {
            0
        } else {
            (num / den).min(255) as u8
        }
    };
    let r = blend(red_of(src) as u32, red_of(dst) as u32);
    let g = blend(green_of(src) as u32, green_of(dst) as u32);
    let b = blend(blue_of(src) as u32, blue_of(dst) as u32);
    let a = (sa + da * (255 - sa) / 255).min(255) as u8;
    argb(a, r, g, b)
}

impl Default for Surface {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_get() {
        let mut s = Surface::new(8, 4);
        s.fill(rgb(1, 2, 3));
        assert_eq!(s.get(0, 0), Some(rgb(1, 2, 3)));
        assert_eq!(s.get(8, 0), None);
        assert_eq!(s.get(-1, 0), None);
        assert_eq!(s.width(), 8);
        assert_eq!(s.height(), 4);
    }

    #[test]
    fn alpha_blend_halfway() {
        let mut s = Surface::filled(2, 2, rgb(0, 0, 0));
        s.blend(0, 0, argb(128, 255, 255, 255));
        let p = s.get(0, 0).unwrap();
        assert!(red_of(p) > 100 && red_of(p) < 160, "实际 {}", red_of(p));
    }

    #[test]
    fn opaque_blend_replaces() {
        let mut s = Surface::filled(2, 2, rgb(10, 20, 30));
        s.blend(1, 1, rgb(200, 100, 50));
        assert_eq!(s.get(1, 1), Some(rgb(200, 100, 50)));
    }

    #[test]
    fn rects_are_clipped() {
        let mut s = Surface::new(10, 10);
        s.fill_rect(-5, -5, 8, 8, rgb(9, 9, 9));
        assert_eq!(s.get(0, 0), Some(rgb(9, 9, 9)));
        assert_ne!(s.get(3, 3), Some(rgb(9, 9, 9)));
        s.stroke_rect(Rect::new(0, 0, 4, 4), rgb(1, 1, 1));
        assert_eq!(s.get(3, 3), Some(rgb(1, 1, 1)));
    }

    #[test]
    fn text_rasterizes() {
        let mut s = Surface::new(64, 16);
        let end = s.draw_text(0, 0, "HI", palette::TEXT, 1);
        assert_eq!(end, 2 * ADVANCE as i32);
        assert!(s.lit_pixels() > 0);
        assert_eq!(Surface::text_width("ABC", 2), 3 * ADVANCE as u32 * 2);
    }

    #[test]
    fn wrap_prefers_word_boundaries() {
        let lines = Surface::wrap_text("WAIT TABLE IS THE TRUTH", 12, 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 12));
        // 不在单词中间断开
        assert!(lines.iter().any(|l| l == "WAIT TABLE "));
        assert!(lines.iter().any(|l| l.trim() == "IS THE TRUTH"));
        assert_eq!(lines.join(""), "WAIT TABLE IS THE TRUTH");
        // 超长单词硬切，且不丢字符
        let long = Surface::wrap_text("ABCDEFGHIJKL", 5, 1);
        assert_eq!(long.join(""), "ABCDEFGHIJKL");
        assert!(long.iter().all(|l| l.chars().count() <= 5));
    }

    #[test]
    fn draw_text_clipped_stops_at_edge() {
        let mut s = Surface::new(40, 10);
        let end = s.draw_text_clipped(0, 0, "ABCDEFGH", palette::TEXT, 2, 24);
        assert_eq!(end, 24);
    }

    #[test]
    fn gradient_is_monotonic() {
        let mut s = Surface::new(4, 8);
        s.vertical_gradient(rgb(0, 0, 0), rgb(255, 255, 255));
        let top = red_of(s.get(0, 0).unwrap());
        let bottom = red_of(s.get(0, 7).unwrap());
        assert!(bottom > top);
    }

    #[test]
    fn draw_surface_transfers() {
        let src = Surface::filled(2, 2, rgb(5, 6, 7));
        let mut dst = Surface::new(4, 4);
        dst.draw_surface(&src, 1, 1);
        assert_eq!(dst.get(1, 1), Some(rgb(5, 6, 7)));
        assert_eq!(dst.get(2, 2), Some(rgb(5, 6, 7)));
        assert_eq!(dst.get(3, 3), Some(0));
    }
}
