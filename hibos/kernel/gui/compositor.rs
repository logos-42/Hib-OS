//! 合成器 —— 把壁纸、窗口、光标合成为一帧 32bpp 画面。
//!
//! 与「字符桌面」的根本区别：这里的输出是**真像素**，可以直接送进
//! `softbuffer` 窗口或导出成图片。合成顺序就是 z-order：
//! 壁纸 → 窗口（从底到顶）→ 光标。焦点窗口最后画，保证它在最上层。
//!
//! 还实现了真实的窗口管理语义：
//! * `raise` 把窗口提到最上层（点击即置顶）
//! * `focus` 转移键盘焦点（只有焦点窗口接收字符）
//! * `close` 销毁窗口并重新计算焦点
//! * `hit_test` 自上而下命中测试
//! * `drag` 标题栏拖动 / 右下角缩放的状态机

use super::surface::{argb, palette, Rect, Surface};
use super::window::{Hit, Window, WindowId};
use crate::monitor::get_time_ns;

/// 壁纸风格。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wallpaper {
    /// 深空渐变 + 星点。
    Nebula,
    /// 纯色。
    Solid(u32),
    /// 网格。
    Grid,
}

/// 拖动 / 缩放状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    None,
    Move { id: WindowId, dx: i32, dy: i32 },
    Resize { id: WindowId, ox: i32, oy: i32 },
}

/// 合成器 = 显示服务器 + 窗口管理器。
pub struct Compositor {
    width: u32,
    height: u32,
    wallpaper: Wallpaper,
    /// true = 用全局像素壁纸库（shell / 通道 / 合成器共用）；
    /// false = 用旧式枚举背景（兼容直接调 `set_wallpaper` 的调用方与测试）。
    use_wallpaper_bank: bool,
    /// 从底到顶。
    windows: Vec<Window>,
    next_id: WindowId,
    focused: Option<WindowId>,
    cursor: (i32, i32),
    drag: DragMode,
    /// 合成输出（复用缓冲，避免每帧分配）。
    frame: Surface,
    /// 最近一次合成的窗口数量 / 脏标记（可观测性）。
    pub frames: u64,
    pub last_compose_ns: u64,
}

impl Compositor {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            wallpaper: Wallpaper::Nebula,
            use_wallpaper_bank: true,
            windows: Vec::new(),
            next_id: 1,
            focused: None,
            cursor: (width as i32 / 2, height as i32 / 2),
            drag: DragMode::None,
            frame: Surface::new(width, height),
            frames: 0,
            last_compose_ns: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn set_wallpaper(&mut self, w: Wallpaper) {
        self.wallpaper = w;
        self.use_wallpaper_bank = false;
    }

    pub fn wallpaper(&self) -> Wallpaper {
        self.wallpaper
    }

    /// 切回全局像素壁纸库并指定风格。
    pub fn use_named_wallpaper(&mut self, name: &str) -> Result<String, String> {
        self.use_wallpaper_bank = true;
        super::wallpaper::bank().lock().unwrap().set_style(name)
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn windows_mut(&mut self) -> &mut Vec<Window> {
        &mut self.windows
    }

    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    pub fn set_cursor(&mut self, x: i32, y: i32) {
        self.cursor = (x.clamp(0, self.width as i32 - 1), y.clamp(0, self.height as i32 - 1));
    }

    pub fn drag(&self) -> DragMode {
        self.drag
    }

    pub fn set_drag(&mut self, mode: DragMode) {
        self.drag = mode;
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    pub fn focused_window_mut(&mut self) -> Option<&mut Window> {
        let id = self.focused?;
        self.window_mut(id)
    }

    /// 打开一个新窗口（纯像素），自动分配 id、居中偏移、置顶并聚焦。
    pub fn open(
        &mut self,
        title: &str,
        w: u32,
        h: u32,
        accent: u32,
    ) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.windows.len() as i32;
        let x = 40 + (n % 6) * 36;
        let y = 40 + (n % 6) * 30;
        let mut win = Window::new(id, title, x, y, w, h, accent);
        win.repaint();
        self.windows.push(win);
        self.focused = Some(id);
        id
    }

    /// 打开一个终端窗口。
    pub fn open_terminal(&mut self, title: &str, w: u32, h: u32, prompt: &str) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.windows.len() as i32;
        let x = 48 + (n % 6) * 40;
        let y = 48 + (n % 6) * 34;
        let win = Window::terminal(id, title, x, y, w, h, prompt);
        self.windows.push(win);
        self.focused = Some(id);
        id
    }

    /// 关闭窗口，焦点交给最上层剩余窗口。
    pub fn close(&mut self, id: WindowId) -> bool {
        let before = self.windows.len();
        self.windows.retain(|w| w.id != id);
        if self.windows.len() == before {
            return false;
        }
        match self.drag {
            DragMode::Move { id: did, .. } | DragMode::Resize { id: did, .. } if did == id => {
                self.drag = DragMode::None;
            }
            _ => {}
        }
        if self.focused == Some(id) {
            self.focused = self.windows.last().map(|w| w.id);
        }
        true
    }

    /// 置顶。
    pub fn raise(&mut self, id: WindowId) {
        if let Some(pos) = self.windows.iter().position(|w| w.id == id) {
            let win = self.windows.remove(pos);
            self.windows.push(win);
        }
    }

    /// 设置焦点（会同时置顶）。
    pub fn focus(&mut self, id: WindowId) {
        if self.windows.iter().any(|w| w.id == id) {
            self.focused = Some(id);
            self.raise(id);
        }
    }

    /// 自上而下命中测试。
    pub fn hit_test(&self, x: i32, y: i32) -> Option<(WindowId, Hit)> {
        for w in self.windows.iter().rev() {
            let hit = w.hit(x, y);
            if hit != Hit::None {
                return Some((w.id, hit));
            }
        }
        None
    }

    /// 合成一帧，返回像素缓冲。
    pub fn compose(&mut self) -> &Surface {
        let began = get_time_ns();
        self.paint_wallpaper();

        // 焦点窗口最后画，确保最上。
        let focused = self.focused;
        let mut order: Vec<usize> = (0..self.windows.len()).collect();
        if let Some(fid) = focused {
            if let Some(pos) = self.windows.iter().position(|w| w.id == fid) {
                order.retain(|i| *i != pos);
                order.push(pos);
            }
        }

        // 先画所有非焦点窗口，再画焦点窗口。
        let mut frame = std::mem::replace(&mut self.frame, Surface::new(1, 1));
        for idx in order {
            let is_focused = focused == Some(self.windows[idx].id);
            self.windows[idx].render(&mut frame, is_focused);
        }
        draw_cursor(&mut frame, self.cursor.0, self.cursor.1);
        self.frame = frame;
        self.frames += 1;
        self.last_compose_ns = get_time_ns().saturating_sub(began);
        &self.frame
    }

    /// 只合成某个窗口区域（damage 重绘的简化版：整帧重绘但只拷一块）。
    pub fn compose_region(&mut self, rect: Rect) -> Surface {
        self.compose();
        let mut out = Surface::new(rect.w, rect.h);
        out.copy_from(&self.frame, rect.x, rect.y, 0, 0, rect.w, rect.h);
        out
    }

    /// 返回当前光标位置的像素（测试用）。
    pub fn cursor_pixel(&self) -> Option<u32> {
        self.frame.get(self.cursor.0, self.cursor.1)
    }

    fn paint_wallpaper(&mut self) {
        let mut frame = std::mem::replace(&mut self.frame, Surface::new(1, 1));
        if self.use_wallpaper_bank {
            // 真正可见的像素壁纸（含图片壁纸），由全局壁纸库统一提供。
            super::wallpaper::render_into(&mut frame);
            self.frame = frame;
            return;
        }
        frame.fill(palette::BASE);
        match self.wallpaper {
            Wallpaper::Solid(c) => frame.fill(c),
            Wallpaper::Nebula => {
                frame.vertical_gradient(argb(255, 0x0b, 0x0b, 0x18), argb(255, 0x1b, 0x14, 0x2e));
                // 静态星点：用确定性哈希，避免每帧闪烁。
                let count = (self.width as u64 * self.height as u64) / 2600;
                let mut seed = 0x9e37_79b9_7f4a_7c15u64;
                for _ in 0..count {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let x = (seed >> 33) as u32 % self.width.max(1);
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let y = (seed >> 33) as u32 % self.height.max(1);
                    let bright = 90 + (seed % 90) as u8;
                    frame.put(x as i32, y as i32, argb(255, bright, bright, bright.min(230)));
                }
                // 地平线光晕
                let glow_y = self.height as i32 * 3 / 4;
                frame.fill_rect(0, glow_y, self.width, 2, argb(30, 0x89, 0xb4, 0xfa));
            }
            Wallpaper::Grid => {
                frame.fill(argb(255, 0x0d, 0x11, 0x18));
                let step = 32;
                for x in (0..self.width).step_by(step) {
                    frame.fill_rect(x as i32, 0, 1, self.height, argb(255, 0x18, 0x24, 0x33));
                }
                for y in (0..self.height).step_by(step) {
                    frame.fill_rect(0, y as i32, self.width, 1, argb(255, 0x18, 0x24, 0x33));
                }
            }
        }
        self.frame = frame;
    }

    /// 可观测摘要。
    pub fn status(&self) -> String {
        let mut s = format!(
            "display={}x{} windows={} focused={:?} frames={} compose={}us drag={:?} wallpaper={:?}",
            self.width,
            self.height,
            self.windows.len(),
            self.focused,
            self.frames,
            self.last_compose_ns / 1000,
            self.drag,
            self.wallpaper
        );
        for w in &self.windows {
            s.push_str(&format!(
                "\n  win#{} {:<20} at=({},{}) {}x{} visible={} min={} max={} epoch={}",
                w.id,
                w.title,
                w.x,
                w.y,
                w.w,
                w.h,
                w.visible,
                w.minimized,
                w.maximized,
                w.content_epoch
            ));
        }
        s
    }

    /// 窗口列表（供 shell）。
    pub fn list(&self) -> String {
        if self.windows.is_empty() {
            return "（无窗口）".to_string();
        }
        let mut out = String::new();
        for w in &self.windows {
            let focus = if Some(w.id) == self.focused { "*" } else { " " };
            out.push_str(&format!(
                "{} win#{:<3} {:<20} ({},{}) {}x{} {}\n",
                focus,
                w.id,
                w.title,
                w.x,
                w.y,
                w.w,
                w.h,
                if w.minimized { "minimized" } else { "shown" }
            ));
        }
        out.trim_end().to_string()
    }
}

/// 画箭头光标（白描边 + 黑填充）。
fn draw_cursor(dst: &mut Surface, x: i32, y: i32) {
    let shape = [
        "X",
        "XX",
        "X.X",
        "X..X",
        "X...X",
        "X....X",
        "X.....X",
        "X......X",
        "X.......X",
        "X........X",
        "X.........X",
        "X......XXXXX",
        "X...X..X",
        "X..XX..X",
        "X.X  X..X",
        "XX   X..X",
        "X     X..X",
        "      X..X",
    ];
    for (dy, row) in shape.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch == 'X' || ch == '.' {
                let color = if ch == 'X' { palette::TEXT } else { palette::BASE };
                dst.put(x + dx as i32, y + dy as i32, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp() -> Compositor {
        Compositor::new(640, 400)
    }

    #[test]
    fn open_close_focus_and_raise() {
        let mut c = comp();
        let a = c.open("A", 200, 150, palette::BLUE);
        let b = c.open("B", 200, 150, palette::GREEN);
        assert_eq!(c.focused(), Some(b));
        c.focus(a);
        assert_eq!(c.focused(), Some(a));
        // a 被提到最上层
        assert_eq!(c.windows().last().unwrap().id, a);
        assert!(c.close(a));
        assert_eq!(c.focused(), Some(b));
        assert!(!c.close(a));
    }

    #[test]
    fn hit_test_prefers_topmost() {
        let mut c = comp();
        let a = c.open("A", 200, 150, palette::BLUE);
        c.window_mut(a).unwrap().move_to(50, 50);
        let b = c.open("B", 200, 150, palette::GREEN);
        c.window_mut(b).unwrap().move_to(60, 60);
        // 重叠区域命中 b
        assert_eq!(c.hit_test(120, 120).map(|(id, _)| id), Some(b));
        // a 独占区域命中 a
        assert_eq!(c.hit_test(55, 55).map(|(id, _)| id), Some(a));
        // 空白区域无命中
        assert!(c.hit_test(600, 380).is_none());
    }

    #[test]
    fn compose_has_wallpaper_and_windows() {
        let mut c = comp();
        let id = c.open_terminal("TERM", 300, 200, "> ");
        c.window_mut(id).unwrap().move_to(30, 30);
        let frame = c.compose();
        assert_eq!(frame.width(), 640);
        assert_eq!(frame.height(), 400);
        assert!(frame.lit_pixels() > 2000);
        // 焦点窗口的强调条一定存在
        assert_eq!(frame.get(31, 31), Some(palette::BLUE));
        assert_eq!(c.frames, 1);
    }

    #[test]
    fn focused_window_is_drawn_last() {
        let mut c = comp();
        let a = c.open("A", 100, 100, palette::RED);
        let b = c.open("B", 100, 100, palette::GREEN);
        c.window_mut(a).unwrap().move_to(100, 100);
        c.window_mut(b).unwrap().move_to(100, 100);
        c.focus(a);
        let frame = c.compose();
        // 最上层是 a（红色强调条）
        assert_eq!(frame.get(101, 101), Some(palette::RED));
    }

    #[test]
    fn cursor_pixels_are_drawn() {
        let mut c = comp();
        c.set_cursor(10, 10);
        c.compose();
        assert!(c.cursor_pixel().is_some());
    }

    #[test]
    fn solid_wallpaper_fills() {
        let mut c = comp();
        c.set_wallpaper(Wallpaper::Solid(palette::CRUST));
        let f = c.compose();
        assert_eq!(f.get(0, 0), Some(palette::CRUST));
    }

    #[test]
    fn window_list_and_status() {
        let mut c = comp();
        c.open("X", 120, 100, palette::MAUVE);
        assert!(c.list().contains("win#1"));
        assert!(c.status().contains("windows=1"));
    }

    #[test]
    fn drag_mode_cleared_when_window_closes() {
        let mut c = comp();
        let id = c.open("X", 120, 100, palette::MAUVE);
        c.set_drag(DragMode::Move { id, dx: 1, dy: 1 });
        c.close(id);
        assert_eq!(c.drag(), DragMode::None);
    }
}
