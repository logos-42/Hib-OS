//! 真实任务栏 —— 绘制、命中测试、启动器菜单、窗口按钮。
//!
//! 不是画在壁纸上的装饰：任务栏有独立的像素、独立的命中区域，
//! 点击会真正改变窗口状态（聚焦 / 最小化 / 还原）或启动一个新应用窗口。

use super::apps::{AppKind, ALL_APPS};
use super::surface::{argb, palette, Rect, Surface};
use super::window::{Window, WindowId};

/// 任务栏高度（像素，位于屏幕底部）。
pub const TASKBAR_H: u32 = 32;
const BTN_W: i32 = 116;
const BTN_GAP: i32 = 4;
const BTN_X0: i32 = 108;
const MENU_W: u32 = 178;
const MENU_ITEM_H: i32 = 24;

/// 任务栏上的点击目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskbarHit {
    Launcher,
    Menu(usize),
    Window(WindowId),
}

pub struct Taskbar {
    pub menu_open: bool,
}

impl Taskbar {
    pub fn new() -> Self {
        Self { menu_open: false }
    }

    pub fn toggle_menu(&mut self) -> bool {
        self.menu_open = !self.menu_open;
        self.menu_open
    }

    fn menu_rect(&self, _w: u32, h: u32) -> Rect {
        let mh = (ALL_APPS.len() as u32 * MENU_ITEM_H as u32 + 10).max(40);
        Rect::new(6, h as i32 - TASKBAR_H as i32 - mh as i32, MENU_W, mh)
    }

    /// 把任务栏画到合成帧（在窗口与光标之后，保证最上层）。
    pub fn draw(
        &self,
        dst: &mut Surface,
        windows: &[Window],
        focused: Option<WindowId>,
        status: &str,
    ) {
        let (w, h) = (dst.width(), dst.height());
        let y0 = h as i32 - TASKBAR_H as i32;

        dst.fill_rect(0, y0, w, TASKBAR_H, argb(255, 0x11, 0x11, 0x1b));
        dst.fill_rect(0, y0, w, 1, palette::MAUVE);

        // 启动器按钮
        let launcher_bg = if self.menu_open {
            palette::MAUVE
        } else {
            palette::BLUE
        };
        dst.fill_round_rect(Rect::new(6, y0 + 4, 92, 24), 6, launcher_bg);
        dst.draw_text(22, y0 + 9, "APPS", palette::CRUST, 1);

        // 每个窗口一个按钮
        for (i, win) in windows.iter().enumerate() {
            let x = BTN_X0 + i as i32 * (BTN_W + BTN_GAP);
            if x + BTN_W > w as i32 - 210 {
                break;
            }
            let is_focus = Some(win.id) == focused;
            let bg = if is_focus {
                win.accent
            } else if win.minimized {
                palette::CRUST
            } else {
                palette::MANTLE
            };
            dst.fill_round_rect(Rect::new(x, y0 + 4, BTN_W as u32, 24), 5, bg);
            let color = if is_focus { palette::CRUST } else { palette::TEXT };
            let label = if win.minimized {
                format!("[{}]", win.title)
            } else {
                win.title.clone()
            };
            dst.draw_text_clipped(x + 6, y0 + 9, &label, color, 1, x + BTN_W - 6);
        }

        // 右侧状态区
        dst.draw_text_clipped(
            (w as i32 - 200).max(BTN_X0),
            y0 + 9,
            status,
            palette::SUBTEXT0,
            1,
            w as i32 - 6,
        );

        // 启动器菜单
        if self.menu_open {
            let r = self.menu_rect(w, h);
            dst.fill_round_rect(r, 6, palette::MANTLE);
            dst.fill_rect(r.x, r.y, r.w, 1, palette::MAUVE);
            dst.draw_text(r.x + 8, r.y + 3, "LAUNCH APP", palette::MAUVE, 1);
            for (i, app) in ALL_APPS.iter().enumerate() {
                let iy = r.y + 20 + i as i32 * MENU_ITEM_H;
                dst.draw_text(
                    r.x + 8,
                    iy,
                    &format!("{} {}", i + 1, app.name()),
                    palette::TEXT,
                    1,
                );
            }
        }
    }

    /// 命中测试：优先菜单，其次任务栏按钮。
    pub fn hit(
        &self,
        w: u32,
        h: u32,
        windows: &[Window],
        x: i32,
        y: i32,
    ) -> Option<TaskbarHit> {
        if self.menu_open {
            let r = self.menu_rect(w, h);
            if r.contains(x, y) {
                let i = (y - r.y - 20) / MENU_ITEM_H;
                if i >= 0 && (i as usize) < ALL_APPS.len() {
                    return Some(TaskbarHit::Menu(i as usize));
                }
                return Some(TaskbarHit::Launcher);
            }
        }
        let y0 = h as i32 - TASKBAR_H as i32;
        if y < y0 || x < 0 {
            return None;
        }
        if x < 100 {
            return Some(TaskbarHit::Launcher);
        }
        let rel = x - BTN_X0;
        if rel < 0 || rel % (BTN_W + BTN_GAP) >= BTN_W {
            return None;
        }
        let idx = (rel / (BTN_W + BTN_GAP)) as usize;
        windows.get(idx).map(|win| TaskbarHit::Window(win.id))
    }

    /// 把菜单索引映射到应用种类。
    pub fn app_at(i: usize) -> Option<AppKind> {
        ALL_APPS.get(i).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_and_window_buttons_hit() {
        let t = Taskbar::new();
        let wins = vec![Window::new(7, "A", 10, 10, 200, 150, palette::BLUE)];
        assert_eq!(t.hit(960, 600, &wins, 20, 590), Some(TaskbarHit::Launcher));
        assert_eq!(t.hit(960, 600, &wins, 120, 590), Some(TaskbarHit::Window(7)));
        assert!(t.hit(960, 600, &wins, 400, 300).is_none());
    }

    #[test]
    fn menu_items_hit() {
        let mut t = Taskbar::new();
        assert!(t.toggle_menu());
        let wins: Vec<Window> = Vec::new();
        let r = t.menu_rect(960, 600);
        assert!(t.hit(960, 600, &wins, 20, r.y + 21).is_some());
        assert_eq!(Taskbar::app_at(0), Some(AppKind::Terminal));
        assert!(Taskbar::app_at(99).is_none());
    }

    #[test]
    fn draw_paints_taskbar_pixels() {
        let t = Taskbar::new();
        let mut s = Surface::filled(300, 100, palette::CRUST);
        t.draw(&mut s, &[], None, "windows=0");
        // 底部区域应被任务栏底色覆盖
        let px = s.get(150, 90).unwrap();
        assert_eq!(px, argb(255, 0x11, 0x11, 0x1b));
    }
}
