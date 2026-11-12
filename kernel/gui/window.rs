//! 窗口对象 —— 有身份、有像素、有客户端区域的真实窗口。
//!
//! 与文本桌面的「字符窗口」不同：这里的窗口拥有自己的 32bpp 客户端表面
//! (`client`)，内容是真的像素；`render()` 负责把标题栏、边框和客户端表面
//! 画到合成目标上。窗口还带最小化/最大化/关闭等真实状态，
//! 命中测试（标题栏 / 关闭按钮 / 客户端）由 `hit()` 给出。
//!
//! 客户端内容模型有两种：
//! * `TerminalModel` —— 内置终端（可读写的行缓冲，验证键盘输入闭环）。
//! * 任意 `Surface` —— 应用可直接往 `client` 上画像素。

use super::apps::AppState;
use super::surface::{argb, palette, Rect, Surface};
use crate::gpu::framebuffer::{ADVANCE, LINE_HEIGHT};

/// 窗口标识。
pub type WindowId = u64;

/// 标题栏高度（像素）。
pub const TITLEBAR_H: u32 = 28;
/// 边框宽度。
pub const BORDER: u32 = 1;
/// 红绿灯直径。
pub const CLOSE_BTN: u32 = 14;
/// 红绿灯距窗口右边距 / 按钮间距（保持在标题栏右上角）。
pub const TRAFFIC_RIGHT_GAP: i32 = 10;
pub const TRAFFIC_GAP: i32 = 8;

/// macOS 标准三色。
pub const MAC_RED: (u8, u8, u8) = (0xff, 0x5f, 0x57);
pub const MAC_YELLOW: (u8, u8, u8) = (0xfe, 0xbc, 0x2e);
pub const MAC_GREEN: (u8, u8, u8) = (0x28, 0xc8, 0x40);

/// 鼠标命中窗口的哪个部位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Client,
    TitleBar,
    CloseButton,
    MinimizeButton,
    MaximizeButton,
    ResizeCorner,
    None,
}

/// 内置终端内容模型（验证「键盘 → 窗口 → 像素」闭环）。
#[derive(Clone, Debug, Default)]
pub struct TerminalModel {
    pub lines: Vec<String>,
    pub input: String,
    pub prompt: String,
    pub scroll: usize,
}

impl TerminalModel {
    pub fn new(prompt: &str) -> Self {
        Self {
            lines: Vec::new(),
            input: String::new(),
            prompt: prompt.to_string(),
            scroll: 0,
        }
    }

    pub fn push_line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
        while self.lines.len() > 500 {
            self.lines.remove(0);
        }
        self.scroll = self.lines.len();
    }

    /// 处理一次回车，返回 true 表示调用了 `clear`（内容已清空）。
    pub fn enter(&mut self) -> bool {
        let line = std::mem::take(&mut self.input);
        let echoed = format!("{}{}", self.prompt, line);
        self.push_line(echoed);
        if line.trim() == "clear" {
            self.lines.clear();
        }
        line.trim() == "clear"
    }

    pub fn type_char(&mut self, ch: char) {
        if self.input.chars().count() < 200 {
            self.input.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }
}

/// 一个窗口。
pub struct Window {
    pub id: WindowId,
    pub title: String,
    /// 左上角（屏幕坐标，含装饰）。
    pub x: i32,
    pub y: i32,
    /// 总尺寸（含装饰）。
    pub w: u32,
    pub h: u32,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub resizable: bool,
    /// 客户端表面（内容区，尺寸 = w-2BORDER × h-TITLEBAR-BORDER）。
    pub client: Surface,
    /// 内置终端模型；为 None 时表示纯像素窗口。
    pub terminal: Option<TerminalModel>,
    /// 窗口内运行的应用；存在时优先接管键盘与客户端绘制。
    pub app: Option<AppState>,
    /// 文本缩放。
    pub scale: u32,
    /// 标题栏颜色（不同应用不同色带）。
    pub accent: u32,
    /// 客户端内容版本号：变化后合成器需要重绘。
    pub content_epoch: u64,
}

impl Window {
    /// 创建一个纯像素窗口。
    pub fn new(id: WindowId, title: &str, x: i32, y: i32, w: u32, h: u32, accent: u32) -> Self {
        let cw = w.saturating_sub(BORDER * 2).max(1);
        let chh = h.saturating_sub(TITLEBAR_H + BORDER).max(1);
        Self {
            id,
            title: title.to_string(),
            x,
            y,
            w,
            h,
            visible: true,
            minimized: false,
            maximized: false,
            resizable: true,
            client: Surface::filled(cw, chh, palette::BASE),
            terminal: None,
            app: None,
            scale: 2,
            accent,
            content_epoch: 1,
        }
    }

    /// 创建一个内置终端窗口。
    pub fn terminal(id: WindowId, title: &str, x: i32, y: i32, w: u32, h: u32, prompt: &str) -> Self {
        let mut win = Self::new(id, title, x, y, w, h, palette::BLUE);
        win.terminal = Some(TerminalModel::new(prompt));
        win.repaint();
        win
    }

    /// 整个窗口（含装饰）的矩形。
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    /// 客户端区域（屏幕坐标）。
    pub fn client_rect(&self) -> Rect {
        Rect::new(
            self.x + BORDER as i32,
            self.y + TITLEBAR_H as i32,
            self.client.width(),
            self.client.height(),
        )
    }

    pub fn titlebar_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, TITLEBAR_H)
    }

    /// Mac 风格红绿灯：保持在标题栏右上角，关闭(红)在最右。
    pub fn close_rect(&self) -> Rect {
        Rect::new(
            self.x + self.w as i32 - CLOSE_BTN as i32 - TRAFFIC_RIGHT_GAP,
            self.y + (TITLEBAR_H as i32 - CLOSE_BTN as i32) / 2,
            CLOSE_BTN,
            CLOSE_BTN,
        )
    }

    pub fn minimize_rect(&self) -> Rect {
        let mut r = self.close_rect();
        r.x -= CLOSE_BTN as i32 + TRAFFIC_GAP;
        r
    }

    pub fn maximize_rect(&self) -> Rect {
        let mut r = self.close_rect();
        r.x -= (CLOSE_BTN as i32 + TRAFFIC_GAP) * 2;
        r
    }

    /// 命中测试。
    pub fn hit(&self, px: i32, py: i32) -> Hit {
        if !self.visible || self.minimized {
            return Hit::None;
        }
        if self.close_rect().contains(px, py) {
            return Hit::CloseButton;
        }
        if self.minimize_rect().contains(px, py) {
            return Hit::MinimizeButton;
        }
        if self.maximize_rect().contains(px, py) {
            return Hit::MaximizeButton;
        }
        // 右下角缩放区必须早于 client 判断，否则永远命中 Client。
        if self.resizable
            && px >= self.x + self.w as i32 - 12
            && py >= self.y + self.h as i32 - 12
            && self.rect().contains(px, py)
        {
            return Hit::ResizeCorner;
        }
        if self.titlebar_rect().contains(px, py) {
            return Hit::TitleBar;
        }
        if self.client_rect().contains(px, py) {
            return Hit::Client;
        }
        Hit::None
    }

    /// 移动窗口。
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }

    /// 调整窗口尺寸（客户端表面随之重建）。
    pub fn resize(&mut self, w: u32, h: u32) {
        let w = w.max(120);
        let h = h.max(TITLEBAR_H + BORDER + 20);
        if w == self.w && h == self.h {
            return;
        }
        let cw = w - BORDER * 2;
        let chh = h - TITLEBAR_H - BORDER;
        let old = std::mem::replace(&mut self.client, Surface::filled(cw, chh, palette::BASE));
        // 保留原有内容（左上对齐）。
        self.client.draw_surface(&old, 0, 0);
        self.w = w;
        self.h = h;
        self.repaint();
    }

    /// 键盘输入：字符 / 退格 / 回车。
    pub fn type_char(&mut self, ch: char) {
        if let Some(app) = self.app.as_mut() {
            app.on_char(ch);
            self.content_epoch += 1;
        } else if let Some(t) = self.terminal.as_mut() {
            t.type_char(ch);
            self.repaint();
        }
    }

    pub fn backspace(&mut self) {
        if let Some(app) = self.app.as_mut() {
            app.on_backspace();
            self.content_epoch += 1;
        } else if let Some(t) = self.terminal.as_mut() {
            t.backspace();
            self.repaint();
        }
    }

    pub fn enter(&mut self) -> Option<String> {
        if let Some(app) = self.app.as_mut() {
            let cmd = app.on_enter();
            self.content_epoch += 1;
            cmd
        } else if let Some(t) = self.terminal.as_mut() {
            let cmd = t.input.trim().to_string();
            t.enter();
            self.repaint();
            Some(cmd)
        } else {
            None
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        if let Some(app) = self.app.as_mut() {
            app.scroll_by(delta);
            self.content_epoch += 1;
        } else if let Some(t) = self.terminal.as_mut() {
            let n = t.lines.len();
            let cur = t.scroll.min(n);
            t.scroll = (cur as i32 + delta).clamp(0, n as i32) as usize;
            self.repaint();
        }
    }

    /// 把终端行缓冲重绘到客户端表面。
    ///
    /// 折行在这里**显式**完成：每个逻辑行先按可用列数切成若干视觉行，
    /// 再逐视觉行推进 y。这样长命令行不会盖住下一行。
    pub fn repaint(&mut self) {
        self.content_epoch += 1;
        let Some(t) = self.terminal.clone() else {
            return;
        };
        let bg = palette::BASE;
        self.client.fill(bg);
        let chh = self.client.height() as i32;
        let line_h = LINE_HEIGHT as i32 * self.scale.max(1) as i32;
        let pad = 8i32;
        let cols = Surface::cols_for_width(
            self.client.width().saturating_sub(pad as u32 * 2),
            self.scale,
        )
        .max(1);
        let max_lines = ((chh - pad * 2) / line_h).max(1) as usize;

        // 头部提示
        self.client
            .draw_text(pad, pad, "HIB OS / REAL PIXEL WINDOW", palette::OVERLAY1, 1);

        // 逻辑行 → 视觉行
        let mut all: Vec<String> = Vec::new();
        for line in &t.lines {
            all.extend(Surface::wrap_text(line, cols, self.scale));
        }
        all.extend(Surface::wrap_text(
            &format!("{}{}_", t.prompt, t.input),
            cols,
            self.scale,
        ));
        if all.is_empty() {
            all.push(String::new());
        }

        let end = all.len();
        let start = t.scroll.min(end).saturating_sub(max_lines);
        let visible = &all[start..end.min(start + max_lines)];

        let mut y = pad + LINE_HEIGHT as i32 * 2;
        for line in visible {
            if y + line_h > chh {
                break;
            }
            let color = if line.starts_with(&t.prompt) {
                palette::TEXT
            } else if line.contains("ERR") || line.contains("error") {
                palette::RED
            } else {
                palette::SUBTEXT0
            };
            self.client
                .draw_text_clipped(pad, y, line, color, self.scale, self.client.width() as i32 - pad);
            y += line_h;
        }
    }

    /// 窗口渲染：阴影 + 边框 + 标题栏 + 客户端。
    pub fn render(&self, dst: &mut Surface, focused: bool) {
        if !self.visible || self.minimized {
            return;
        }
        let r = self.rect();

        // 1. 投影（半透明黑，右下偏移 6px）
        let shadow = Rect::new(r.x + 6, r.y + 8, r.w, r.h);
        dst.fill_round_rect(shadow, 6, argb(70, 0, 0, 0));

        // 2. 窗口背景 + 边框
        dst.fill_round_rect(r, 6, palette::CRUST);
        let border = if focused { self.accent } else { palette::SURFACE0 };
        dst.stroke_rect(r, border);

        // 3. 标题栏
        let tb = Rect::new(r.x + 1, r.y + 1, r.w.saturating_sub(2), TITLEBAR_H - 1);
        dst.fill_rect(tb.x, tb.y, tb.w, tb.h, palette::MANTLE);
        // 左侧强调条
        dst.fill_rect(tb.x, tb.y, 3, tb.h, self.accent);
        // 标题左侧对齐，右侧让出红绿灯区域
        let title_max = tb.right() - (CLOSE_BTN as i32 + TRAFFIC_GAP) * 3 - TRAFFIC_RIGHT_GAP;
        dst.draw_text_clipped(tb.x + 12, tb.y + 6, &self.title, palette::TEXT, 2, title_max);

        // 4. Mac 红绿灯：焦点窗口才显示操作符号
        draw_traffic_light(dst, self.close_rect(), MAC_RED, LightGlyph::Close, focused);
        draw_traffic_light(dst, self.minimize_rect(), MAC_YELLOW, LightGlyph::Minimize, focused);
        draw_traffic_light(dst, self.maximize_rect(), MAC_GREEN, LightGlyph::Maximize, focused);

        // 5. 客户端
        let cr = self.client_rect();
        dst.draw_surface(&self.client, cr.x, cr.y);
        dst.stroke_rect(cr, if focused { self.accent } else { palette::SURFACE0 });

        // 6. 焦点指示：客户端底部细线
        if focused {
            dst.fill_rect(cr.x, cr.bottom() - 2, cr.w, 2, self.accent);
        }
    }
}

#[derive(Clone, Copy)]
enum LightGlyph {
    Close,
    Minimize,
    Maximize,
}

fn mix(a: u8, b: u8, k: f32) -> u8 {
    ((a as f32) * (1.0 - k) + (b as f32) * k).clamp(0.0, 255.0) as u8
}

/// 画一颗 Mac 风格红绿灯：抗锯齿圆 + 球面渐变 + 顶部高光 + 外圈暗描边。
///
/// 不用 `fill_round_rect` 的硬边，而是逐像素按到圆心的距离算覆盖率，
/// 因此边缘是平滑的，看起来像真实的玻璃按钮而不是像素方块。
fn draw_traffic_light(
    dst: &mut Surface,
    rect: Rect,
    base: (u8, u8, u8),
    glyph: LightGlyph,
    show_glyph: bool,
) {
    let r = rect.w as f32 / 2.0;
    let cx = rect.x as f32 + r;
    let cy = rect.y as f32 + rect.h as f32 / 2.0;
    let (br, bg, bb) = base;

    for yy in 0..rect.h as i32 {
        for xx in 0..rect.w as i32 {
            let dx = (rect.x + xx) as f32 + 0.5 - cx;
            let dy = (rect.y + yy) as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > r + 0.5 {
                continue;
            }
            // 上亮下暗：模拟球面受光
            let light = 1.0 + (0.5 - yy as f32 / rect.h as f32) * 0.20;
            let mut cr = (br as f32 * light).min(255.0) as u8;
            let mut cg = (bg as f32 * light).min(255.0) as u8;
            let mut cb = (bb as f32 * light).min(255.0) as u8;
            // 顶部弧形高光斑
            let hy = dy + r * 0.36;
            let hd = (dx * dx + hy * hy).sqrt();
            if hd < r * 0.66 {
                let k = (1.0 - hd / (r * 0.66)) * 0.34;
                cr = mix(cr, 255, k);
                cg = mix(cg, 255, k);
                cb = mix(cb, 255, k);
            }
            // 外圈 1px 暗描边，把球从标题栏里"抠"出来
            if d > r - 1.15 {
                cr = mix(cr, 0x28, 0.40);
                cg = mix(cg, 0x20, 0.40);
                cb = mix(cb, 0x20, 0.40);
            }
            // 距离场产生的抗锯齿 alpha
            let alpha = ((r + 0.5 - d) * 255.0).clamp(0.0, 255.0) as u8;
            dst.blend(rect.x + xx, rect.y + yy, argb(alpha, cr, cg, cb));
        }
    }
    if show_glyph {
        draw_light_glyph(dst, rect, glyph);
    }
}

/// 悬停（这里用窗口焦点近似）时显示的细线符号，取代原来的位图字符 x/-/o。
fn draw_light_glyph(dst: &mut Surface, rect: Rect, glyph: LightGlyph) {
    let ink = argb(255, 0x3a, 0x2c, 0x28);
    let cx = rect.x + rect.w as i32 / 2;
    let cy = rect.y + rect.h as i32 / 2;
    match glyph {
        LightGlyph::Close => {
            for i in -3..=3 {
                dst.put(cx + i, cy + i, ink);
                dst.put(cx + i, cy - i, ink);
            }
        }
        LightGlyph::Minimize => {
            for i in -3..=3 {
                dst.put(cx + i, cy, ink);
            }
        }
        LightGlyph::Maximize => {
            for i in -3..=3 {
                dst.put(cx + i, cy, ink);
                dst.put(cx, cy + i, ink);
            }
        }
    }
}

/// 标题栏里的文字基线（供测试断言）。
pub fn title_baseline(win: &Window) -> i32 {
    win.y + 6 + 7
}

/// 客户端每行字符数（估算）。
pub fn cols_of(win: &Window) -> usize {
    ((win.client.width() as i32 - 16).max(1) / (ADVANCE as i32 * win.scale as i32)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::{blue_of, green_of, red_of};

    #[test]
    fn hit_test_regions() {
        let win = Window::new(1, "TEST", 10, 20, 200, 150, palette::BLUE);
        assert_eq!(win.hit(10, 20), Hit::TitleBar);
        assert_eq!(win.hit(50, 20), Hit::TitleBar);
        assert_eq!(win.hit(100, 60), Hit::Client);
        assert_eq!(win.hit(5, 5), Hit::None);
        let c = win.close_rect();
        assert_eq!(win.hit(c.x + 2, c.y + 2), Hit::CloseButton);
    }

    #[test]
    fn move_and_resize_keep_client_in_sync() {
        let mut win = Window::new(2, "T", 0, 0, 160, 120, palette::GREEN);
        win.move_to(30, 40);
        assert_eq!(win.x, 30);
        win.resize(300, 200);
        assert_eq!(win.w, 300);
        assert_eq!(win.client.width(), 298);
        assert_eq!(win.client.height(), 200 - TITLEBAR_H - 1);
        // 最小尺寸被钳制
        win.resize(10, 10);
        assert!(win.w >= 120);
    }

    #[test]
    fn terminal_input_roundtrip() {
        let mut win = Window::terminal(3, "TERM", 0, 0, 320, 240, "> ");
        win.type_char('h');
        win.type_char('i');
        assert_eq!(win.terminal.as_ref().unwrap().input, "hi");
        win.backspace();
        assert_eq!(win.terminal.as_ref().unwrap().input, "h");
        win.type_char('i');
        let cmd = win.enter().unwrap();
        assert_eq!(cmd, "hi");
        assert!(win.terminal.as_ref().unwrap().lines.iter().any(|l| l == "> hi"));
        assert_eq!(win.terminal.as_ref().unwrap().input, "");
    }

    #[test]
    fn clear_command_empties_scrollback() {
        let mut win = Window::terminal(4, "TERM", 0, 0, 320, 240, "$ ");
        for c in "echo".chars() {
            win.type_char(c);
        }
        win.enter();
        assert!(!win.terminal.as_ref().unwrap().lines.is_empty());
        for c in "clear".chars() {
            win.type_char(c);
        }
        win.enter();
        assert!(win.terminal.as_ref().unwrap().lines.is_empty());
    }

    #[test]
    fn render_draws_pixels_and_title() {
        let win = Window::terminal(5, "HELLO", 10, 10, 200, 150, "> ");
        let mut screen = Surface::filled(400, 300, palette::BASE);
        win.render(&mut screen, false);
        assert!(screen.lit_pixels() > 1000);
        // 标题栏强调色
        assert_eq!(screen.get(11, 11), Some(palette::BLUE));
        // 红球（未聚焦时不画深色符号）
        let c = win.close_rect();
        let p = screen.get(c.x + c.w as i32 / 2, c.y + 2).unwrap();
        assert!(red_of(p) > 180, "红球中心应为红色系: {:08x}", p);
    }

    #[test]
    fn mac_traffic_lights_are_ordered_rounded_and_shaded() {
        let win = Window::new(9, "MAC", 20, 20, 300, 180, palette::BLUE);
        let c = win.close_rect();
        let m = win.minimize_rect();
        let x = win.maximize_rect();
        assert!(x.x < m.x && m.x < c.x, "右上角依次为 绿→黄→红(关闭在最右)");

        let mut s = Surface::filled(400, 250, palette::BASE);
        win.render(&mut s, false);
        let centre = |r: Rect| {
            s.get(r.x + r.w as i32 / 2, r.y + r.h as i32 / 2)
                .unwrap()
        };
        let p_red = centre(c);
        let p_yellow = centre(m);
        let p_green = centre(x);
        assert!(red_of(p_red) > 180 && green_of(p_red) < 190, "红球 {:08x}", p_red);
        assert!(red_of(p_yellow) > 180 && green_of(p_yellow) > 120, "黄球 {:08x}", p_yellow);
        assert!(green_of(p_green) > 130 && red_of(p_green) < 150, "绿球 {:08x}", p_green);
        // 圆外四角应保持标题栏底色 -> 说明是圆而不是方块
        assert_eq!(s.get(c.x, c.y), Some(palette::MANTLE));
        // 球面受光：上半球比下半球亮
        let top = s.get(c.x + c.w as i32 / 2, c.y + 2).unwrap();
        let bottom = s.get(c.x + c.w as i32 / 2, c.y + c.h as i32 - 3).unwrap();
        assert!(red_of(top) >= red_of(bottom), "上半球应有高光");
    }

    #[test]
    fn focused_window_reveals_traffic_glyphs() {
        let win = Window::new(10, "F", 0, 0, 200, 120, palette::BLUE);
        let mut idle = Surface::filled(250, 150, palette::BASE);
        win.render(&mut idle, false);
        let mut hover = Surface::filled(250, 150, palette::BASE);
        win.render(&mut hover, true);
        let c = win.close_rect();
        let (cx, cy) = (c.x + c.w as i32 / 2, c.y + c.h as i32 / 2);
        assert_ne!(idle.get(cx, cy), hover.get(cx, cy), "聚焦时应显示 × 符号");
    }

    #[test]
    fn minimized_windows_are_not_hit_or_rendered() {
        let mut win = Window::new(6, "HIDDEN", 0, 0, 100, 100, palette::BLUE);
        win.minimized = true;
        assert_eq!(win.hit(10, 10), Hit::None);
        // 用纯黑背景，渲染后应保持全黑。
        let mut s = Surface::filled(200, 200, 0);
        win.render(&mut s, false);
        assert_eq!(s.lit_pixels(), 0);
    }
}
