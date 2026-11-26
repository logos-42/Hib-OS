//! Dock：常驻 / 条件显示 / 隐藏三态，支持自动隐藏。

use super::super::intent::PageKind;
use super::super::surface::Surface;
use super::super::window::Window;
use super::DOCK_H;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Conversation,
    Tasks,
    Files,
    Permission,
    Notification,
    Advanced,
}

impl Icon {
    fn label(self) -> &'static str {
        match self {
            Icon::Conversation => "CHAT",
            Icon::Tasks => "TASK",
            Icon::Files => "FILE",
            Icon::Permission => "PERM",
            Icon::Notification => "NOTE",
            Icon::Advanced => "ADV",
        }
    }

    /// 图标对应的全屏工作区页面（Dock 点击 → 打开语义页面）。
    pub fn page(self) -> PageKind {
        match self {
            Icon::Conversation => PageKind::Conversation,
            Icon::Tasks => PageKind::Tasks,
            Icon::Files => PageKind::Files,
            Icon::Permission => PageKind::Permission,
            Icon::Notification => PageKind::Observation,
            Icon::Advanced => PageKind::Advanced,
        }
    }
}

/// 命中 Dock 图标，返回应打开的页面；未命中返回 None。
pub fn hit_page(x: i32, y: i32, frame_w: u32, frame_h: u32) -> Option<PageKind> {
    let dock = super::dock_state();
    let d = dock.lock().unwrap();
    if !d.is_shown(y, frame_h as i32) {
        return None;
    }
    let top = frame_h as i32 - DOCK_H + 12;
    if y < top || y > top + 32 {
        return None;
    }
    let order = [
        Icon::Conversation,
        Icon::Tasks,
        Icon::Files,
        Icon::Permission,
        Icon::Notification,
        Icon::Advanced,
    ];
    let mut cx = 24;
    for icon in order {
        if d.icon_visible(icon) {
            if x >= cx && x < cx + 32 {
                return Some(icon.page());
            }
            cx += 48;
        }
    }
    let _ = frame_w;
    None
}

#[derive(Debug)]
pub struct Dock {
    pub auto_hide: bool,
    pub revealed: bool,
    pub pending_permission: bool,
    pub unread: usize,
}

impl Dock {
    pub fn new() -> Self {
        // 默认常驻显示；「高级工具」需显式展开；自动隐藏作为可选项。
        Self { auto_hide: false, revealed: false, pending_permission: false, unread: 0 }
    }

    /// 三态可见性：常驻始终显示；条件图标按状态；隐藏图标需显式展开。
    pub fn icon_visible(&self, icon: Icon) -> bool {
        match icon {
            Icon::Conversation | Icon::Tasks | Icon::Files => true,
            Icon::Permission => self.pending_permission,
            Icon::Notification => self.unread > 0,
            // 高级工具需要显式展开（点击「更多」或快捷键）。
            Icon::Advanced => self.revealed,
        }
    }

    pub fn is_shown(&self, cursor_y: i32, frame_h: i32) -> bool {
        if !self.auto_hide {
            return true;
        }
        self.revealed || cursor_y >= frame_h - 8
    }
}

impl Default for Dock {
    fn default() -> Self {
        Self::new()
    }
}

pub fn draw(frame: &mut Surface, _windows: &[Window], _focused: Option<u64>) {
    let h = frame.height() as i32;
    let dock = super::dock_state();
    let d = dock.lock().unwrap();
    if !d.is_shown(0, h) {
        return;
    }
    let y = h - DOCK_H;
    frame.fill_rect(0, y, frame.width(), DOCK_H as u32, 0xFF1B1C1F);
    frame.fill_rect(0, y, frame.width(), 1, 0xFF303238);

    let mut x = 24;
    for icon in [
        Icon::Conversation,
        Icon::Tasks,
        Icon::Files,
        Icon::Permission,
        Icon::Notification,
        Icon::Advanced,
    ] {
        if !d.icon_visible(icon) {
            continue;
        }
        frame.fill_rect(x, y + 12, 32, 32, 0xFF25282D);
        frame.draw_text(x + 7, y + 24, icon.label(), 0xFFC8CCD4, 1);
        x += 48;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_hides_conditional_icons() {
        let dock = Dock::new();
        assert!(!dock.icon_visible(Icon::Permission));
        assert!(!dock.icon_visible(Icon::Notification));
        assert!(!dock.icon_visible(Icon::Advanced));
        assert!(dock.icon_visible(Icon::Conversation));
    }

    #[test]
    fn dock_reveals_conditional_icons() {
        let mut dock = Dock::new();
        dock.pending_permission = true;
        dock.unread = 2;
        dock.revealed = true;
        assert!(dock.icon_visible(Icon::Permission));
        assert!(dock.icon_visible(Icon::Notification));
        assert!(dock.icon_visible(Icon::Advanced));
    }

    #[test]
    fn auto_hide_respects_cursor() {
        let mut dock = Dock::new();
        // 默认常驻；开启自动隐藏后才按光标位置判定。
        assert!(dock.is_shown(100, 600));
        dock.auto_hide = true;
        assert!(!dock.is_shown(100, 600));
        assert!(dock.is_shown(596, 600));
    }
}
