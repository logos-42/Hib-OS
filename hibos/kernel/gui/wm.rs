//! 窗口管理器 —— 窗口生命周期、装饰绘制、焦点管理与 Z-order 排序。
//!
//! `WindowManager` 管理窗口的创建、显示/隐藏、关闭、聚焦、最小化/最大化/还原、
//! 移动、缩放、Z-order 调整与自动布局。`Window` 包含标题栏、客户区、边框、
//! 位置、大小、状态、层级与所属应用。
//!
//! 窗口装饰：标题栏绘制、关闭/最小化/最大化按钮、标题栏拖动移动窗口、
//! 边框调整大小。焦点管理：点击聚焦、Alt-Tab 循环、激活窗口置顶。
//! 与 `kernel/gui/compositor.rs` 的 `Compositor` 互补：WindowManager 负责窗口
//! 逻辑状态，Compositor 负责像素合成输出。

use super::surface::{argb, Rect, Surface};
use super::surface::palette;
use super::window::{Hit, WindowId, TITLEBAR_H, BORDER, CLOSE_BTN, TRAFFIC_RIGHT_GAP, TRAFFIC_GAP};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 窗口标志
// ============================================================================

/// 窗口创建标志（位掩码）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowFlags(pub u32);

impl WindowFlags {
    pub const RESIZABLE: u32 = 1 << 0;
    pub const MINIMIZABLE: u32 = 1 << 1;
    pub const MAXIMIZABLE: u32 = 1 << 2;
    pub const CLOSABLE: u32 = 1 << 3;
    pub const FLOATING: u32 = 1 << 4;
    pub const MODAL: u32 = 1 << 5;
    pub const NO_DECORATION: u32 = 1 << 6;

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn all() -> Self {
        Self(
            Self::RESIZABLE
                | Self::MINIMIZABLE
                | Self::MAXIMIZABLE
                | Self::CLOSABLE
                | Self::FLOATING,
        )
    }

    pub fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    pub fn insert(&mut self, bit: u32) {
        self.0 |= bit;
    }

    pub fn remove(&mut self, bit: u32) {
        self.0 &= !bit;
    }
}

// ============================================================================
// 窗口状态
// ============================================================================

/// 窗口显示状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WinState {
    /// 正常显示。
    #[default]
    Normal,
    /// 最小化（仅任务栏图标）。
    Minimized,
    /// 最大化（占满屏幕）。
    Maximized,
    /// 隐藏（不可见，不参与布局）。
    Hidden,
}

impl WinState {
    pub fn label(self) -> &'static str {
        match self {
            WinState::Normal => "normal",
            WinState::Minimized => "minimized",
            WinState::Maximized => "maximized",
            WinState::Hidden => "hidden",
        }
    }
}

// ============================================================================
// 窗口对象（wm 独立于 compositor 的 Window，侧重管理状态）
// ============================================================================

/// 一个管理层窗口。
#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub title: String,
    /// 屏幕坐标左上角（含装饰）。
    pub x: i32,
    pub y: i32,
    /// 总尺寸（含装饰）。
    pub w: u32,
    pub h: u32,
    /// 客户区左上角（屏幕坐标）。
    pub client_x: i32,
    /// 客户区左上角 y。
    pub client_y: i32,
    /// 客户区尺寸。
    pub client_w: u32,
    pub client_h: u32,
    pub state: WinState,
    pub flags: WindowFlags,
    /// Z-order 层级（数值越大越靠前）。
    pub z_level: i32,
    /// 所属应用名。
    pub app: Option<String>,
    /// 最小化/最大化前的矩形（用于还原）。
    pub restore_rect: Option<Rect>,
    /// 创建时间戳（ns）。
    pub created_at: u64,
}

impl Window {
    /// 创建一个新窗口。
    pub fn new(
        id: WindowId,
        title: &str,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        flags: WindowFlags,
    ) -> Self {
        let (client_x, client_y, client_w, client_h) = if flags.contains(WindowFlags::NO_DECORATION) {
            (x, y, w, h)
        } else {
            (
                x + BORDER as i32,
                y + TITLEBAR_H as i32,
                w.saturating_sub(BORDER * 2).max(1),
                h.saturating_sub(TITLEBAR_H + BORDER).max(1),
            )
        };
        Self {
            id,
            title: title.to_string(),
            x,
            y,
            w,
            h,
            client_x,
            client_y,
            client_w,
            client_h,
            state: WinState::Normal,
            flags,
            z_level: 0,
            app: None,
            restore_rect: None,
            created_at: crate::monitor::get_time_ns(),
        }
    }

    /// 整个窗口矩形（含装饰）。
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    /// 客户区矩形（屏幕坐标）。
    pub fn client_rect(&self) -> Rect {
        Rect::new(self.client_x, self.client_y, self.client_w, self.client_h)
    }

    /// 标题栏矩形。
    pub fn titlebar_rect(&self) -> Rect {
        if self.flags.contains(WindowFlags::NO_DECORATION) {
            Rect::new(0, 0, 0, 0)
        } else {
            Rect::new(self.x, self.y, self.w, TITLEBAR_H)
        }
    }

    /// 关闭按钮矩形（标题栏右上角）。
    pub fn close_rect(&self) -> Rect {
        Rect::new(
            self.x + self.w as i32 - CLOSE_BTN as i32 - TRAFFIC_RIGHT_GAP,
            self.y + (TITLEBAR_H as i32 - CLOSE_BTN as i32) / 2,
            CLOSE_BTN,
            CLOSE_BTN,
        )
    }

    /// 最小化按钮矩形。
    pub fn minimize_rect(&self) -> Rect {
        let mut r = self.close_rect();
        r.x -= CLOSE_BTN as i32 + TRAFFIC_GAP;
        r
    }

    /// 最大化按钮矩形。
    pub fn maximize_rect(&self) -> Rect {
        let mut r = self.close_rect();
        r.x -= (CLOSE_BTN as i32 + TRAFFIC_GAP) * 2;
        r
    }

    /// 调整大小手柄（右下角）。
    pub fn resize_corner_rect(&self) -> Rect {
        let size = 12;
        Rect::new(
            self.x + self.w as i32 - size as i32,
            self.y + self.h as i32 - size as i32,
            size,
            size,
        )
    }

    /// 命中测试：返回窗口部件。
    pub fn hit(&self, px: i32, py: i32) -> Hit {
        if self.state == WinState::Hidden || self.state == WinState::Minimized {
            return Hit::None;
        }
        if !self.rect().contains(px, py) {
            return Hit::None;
        }
        if !self.flags.contains(WindowFlags::NO_DECORATION) && self.titlebar_rect().contains(px, py) {
            if self.close_rect().contains(px, py) && self.flags.contains(WindowFlags::CLOSABLE) {
                return Hit::CloseButton;
            }
            if self.minimize_rect().contains(px, py) && self.flags.contains(WindowFlags::MINIMIZABLE) {
                return Hit::MinimizeButton;
            }
            if self.maximize_rect().contains(px, py) && self.flags.contains(WindowFlags::MAXIMIZABLE) {
                return Hit::MaximizeButton;
            }
            return Hit::TitleBar;
        }
        if self.flags.contains(WindowFlags::RESIZABLE) && self.resize_corner_rect().contains(px, py) {
            return Hit::ResizeCorner;
        }
        if self.client_rect().contains(px, py) {
            return Hit::Client;
        }
        Hit::None
    }

    /// 保存当前矩形为可还原状态。
    pub fn save_restore(&mut self) {
        self.restore_rect = Some(self.rect());
    }

    /// 还原到保存的矩形。
    pub fn restore_geometry(&mut self) -> bool {
        if let Some(r) = self.restore_rect.take() {
            self.x = r.x;
            self.y = r.y;
            self.w = r.w;
            self.h = r.h;
            self.client_x = self.x + BORDER as i32;
            self.client_y = self.y + TITLEBAR_H as i32;
            self.client_w = self.w.saturating_sub(BORDER * 2).max(1);
            self.client_h = self.h.saturating_sub(TITLEBAR_H + BORDER).max(1);
            true
        } else {
            false
        }
    }
}

// ============================================================================
// 窗口管理器
// ============================================================================

/// 窗口管理器：持有全部窗口、执行焦点/Z-order 管理、布局与装饰绘制。
pub struct WindowManager {
    /// 窗口注册表，按 id 排序。
    windows: BTreeMap<WindowId, Window>,
    /// 下一个窗口 id。
    next_id: WindowId,
    /// 当前焦点窗口 id。
    focused: Option<WindowId>,
    /// 屏幕尺寸。
    screen_w: u32,
    screen_h: u32,
    /// 是否启用叠窗模式（点击置顶）。
    pub click_to_raise: bool,
    /// Alt-Tab 循环索引。
    alt_tab_index: usize,
    /// 全局装饰颜色。
    pub title_bg: u32,
    pub title_fg: u32,
    pub border_color: u32,
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new(960, 600)
    }
}

impl WindowManager {
    /// 创建窗口管理器。
    pub fn new(screen_w: u32, screen_h: u32) -> Self {
        Self {
            windows: BTreeMap::new(),
            next_id: 1,
            focused: None,
            screen_w,
            screen_h,
            click_to_raise: true,
            alt_tab_index: 0,
            title_bg: palette::SURFACE0,
            title_fg: palette::TEXT,
            border_color: palette::OVERLAY0,
        }
    }

    /// 设置屏幕尺寸。
    pub fn set_screen_size(&mut self, w: u32, h: u32) {
        self.screen_w = w;
        self.screen_h = h;
    }

    /// 创建窗口，返回窗口 id。
    pub fn create_window(
        &mut self,
        rect: Rect,
        title: &str,
        flags: WindowFlags,
    ) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        // 防止窗口超出屏幕。
        let x = rect.x.clamp(0, self.screen_w as i32 - rect.w as i32);
        let y = rect.y.clamp(0, self.screen_h as i32 - rect.h as i32);
        let mut win = Window::new(id, title, x, y, rect.w, rect.h, flags);
        // 自动分配 z_level，新窗口置顶。
        let max_z = self.windows.values().map(|w| w.z_level).max().unwrap_or(0);
        win.z_level = max_z + 1;
        self.windows.insert(id, win);
        self.focused = Some(id);
        id
    }

    /// 创建默认标志的窗口（可调整/可最小化/可最大化/可关闭）。
    pub fn create_window_default(&mut self, rect: Rect, title: &str) -> WindowId {
        self.create_window(rect, title, WindowFlags::all())
    }

    /// 显示窗口。
    pub fn show(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            if w.state == WinState::Hidden {
                w.state = WinState::Normal;
            }
            w.state = WinState::Normal;
            self.focused = Some(id);
            self.raise_to_top(id);
            true
        } else {
            false
        }
    }

    /// 隐藏窗口。
    pub fn hide(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            w.state = WinState::Hidden;
            if self.focused == Some(id) {
                self.focused = self.topmost_normal().map(|w| w.id);
            }
            true
        } else {
            false
        }
    }

    /// 关闭窗口并移除。
    pub fn close(&mut self, id: WindowId) -> bool {
        let removed = self.windows.remove(&id).is_some();
        if removed {
            if self.focused == Some(id) {
                self.focused = self.topmost_normal().map(|w| w.id);
            }
        }
        removed
    }

    /// 聚焦窗口并置顶。
    pub fn focus(&mut self, id: WindowId) -> bool {
        if self.windows.contains_key(&id) {
            self.focused = Some(id);
            self.raise_to_top(id);
            true
        } else {
            false
        }
    }

    /// 取消焦点。
    pub fn blur(&mut self, id: WindowId) -> bool {
        if self.focused == Some(id) {
            self.focused = self.topmost_normal().map(|w| w.id);
            true
        } else {
            false
        }
    }

    /// 返回当前焦点窗口 id。
    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// 最小化窗口。
    pub fn minimize(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            if w.state != WinState::Minimized && w.flags.contains(WindowFlags::MINIMIZABLE) {
                w.save_restore();
                w.state = WinState::Minimized;
                if self.focused == Some(id) {
                    self.focused = self.topmost_normal().map(|w| w.id);
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// 最大化窗口。
    pub fn maximize(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            if w.state != WinState::Maximized && w.flags.contains(WindowFlags::MAXIMIZABLE) {
                w.save_restore();
                w.state = WinState::Maximized;
                w.x = 0;
                w.y = 0;
                w.w = self.screen_w;
                w.h = self.screen_h;
                w.client_x = BORDER as i32;
                w.client_y = TITLEBAR_H as i32;
                w.client_w = self.screen_w.saturating_sub(BORDER * 2).max(1);
                w.client_h = self.screen_h.saturating_sub(TITLEBAR_H + BORDER).max(1);
                self.focused = Some(id);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// 还原窗口。
    pub fn restore(&mut self, id: WindowId) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            if w.state == WinState::Minimized || w.state == WinState::Maximized {
                w.state = WinState::Normal;
                w.restore_geometry();
                self.focused = Some(id);
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// 移动窗口。
    pub fn move_window(&mut self, id: WindowId, x: i32, y: i32) -> bool {
        if let Some(w) = self.windows.get_mut(&id) {
            let nx = x.clamp(0, self.screen_w as i32 - w.w as i32);
            let ny = y.clamp(0, self.screen_h as i32 - w.h as i32);
            w.x = nx;
            w.y = ny;
            if !w.flags.contains(WindowFlags::NO_DECORATION) {
                w.client_x = nx + BORDER as i32;
                w.client_y = ny + TITLEBAR_H as i32;
            } else {
                w.client_x = nx;
                w.client_y = ny;
            }
            true
        } else {
            false
        }
    }

    /// 缩放窗口。
    pub fn resize(&mut self, id: WindowId, w: u32, h: u32) -> bool {
        if let Some(win) = self.windows.get_mut(&id) {
            if !win.flags.contains(WindowFlags::RESIZABLE) {
                return false;
            }
            let nw = w.clamp(200, self.screen_w);
            let nh = h.clamp(150, self.screen_h);
            win.w = nw;
            win.h = nh;
            if !win.flags.contains(WindowFlags::NO_DECORATION) {
                win.client_w = nw.saturating_sub(BORDER * 2).max(1);
                win.client_h = nh.saturating_sub(TITLEBAR_H + BORDER).max(1);
            } else {
                win.client_w = nw;
                win.client_h = nh;
            }
            true
        } else {
            false
        }
    }

    /// 设置 Z-order 层级。
    pub fn z_order(&mut self, id: WindowId, level: i32) -> bool {
        if self.windows.contains_key(&id) {
            if let Some(w) = self.windows.get_mut(&id) {
                w.z_level = level;
            }
            true
        } else {
            false
        }
    }

    /// 把窗口提到最上层（z_level 设为当前最大+1）。
    pub fn raise_to_top(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get(&id) {
            let current = w.z_level;
            let max_z = self
                .windows
                .values()
                .map(|w| w.z_level)
                .max()
                .unwrap_or(0);
            if current <= max_z {
                if let Some(w) = self.windows.get_mut(&id) {
                    w.z_level = max_z + 1;
                }
            }
        }
    }

    /// 把窗口送到最底层。
    pub fn send_to_bottom(&mut self, id: WindowId) {
        if let Some(w) = self.windows.get(&id) {
            let min_z = self.windows.values().map(|w| w.z_level).min().unwrap_or(0);
            if let Some(w) = self.windows.get_mut(&id) {
                w.z_level = min_z.saturating_sub(1);
            }
        }
    }

    /// 返回按 z-order 从底到顶排序的窗口列表。
    pub fn windows_by_zorder(&self) -> Vec<&Window> {
        let mut list: Vec<&Window> = self.windows.values().filter(|w| w.state != WinState::Hidden).collect();
        list.sort_by_key(|w| w.z_level);
        list
    }

    /// 返回可见（非隐藏、非最小化）窗口列表。
    pub fn visible_windows(&self) -> Vec<&Window> {
        self.windows
            .values()
            .filter(|w| w.state == WinState::Normal || w.state == WinState::Maximized)
            .collect()
    }

    /// 获取窗口引用。
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// 获取可变窗口引用。
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    /// 返回焦点窗口引用。
    pub fn focused_window(&self) -> Option<&Window> {
        self.focused.and_then(|id| self.windows.get(&id))
    }

    /// Alt-Tab 循环：切换到下一个正常窗口。
    pub fn alt_tab_next(&mut self) -> Option<WindowId> {
        let visible: Vec<WindowId> = self
            .visible_windows()
            .iter()
            .map(|w| w.id)
            .collect();
        if visible.is_empty() {
            return None;
        }
        let idx = self
            .focused
            .and_then(|id| visible.iter().position(|&x| x == id))
            .unwrap_or(0);
        let next = (idx + 1) % visible.len();
        let id = visible[next];
        self.focused = Some(id);
        self.alt_tab_index = next;
        self.raise_to_top(id);
        Some(id)
    }

    /// Alt-Shift-Tab：切换到上一个正常窗口。
    pub fn alt_tab_prev(&mut self) -> Option<WindowId> {
        let visible: Vec<WindowId> = self
            .visible_windows()
            .iter()
            .map(|w| w.id)
            .collect();
        if visible.is_empty() {
            return None;
        }
        let idx = self
            .focused
            .and_then(|id| visible.iter().position(|&x| x == id))
            .unwrap_or(0);
        let prev = if idx == 0 { visible.len() - 1 } else { idx - 1 };
        let id = visible[prev];
        self.focused = Some(id);
        self.alt_tab_index = prev;
        self.raise_to_top(id);
        Some(id)
    }

    /// 自动布局：层叠排列全部可见窗口（级联）。
    pub fn layout_windows(&mut self) {
        let visible: Vec<WindowId> = self.visible_windows().iter().map(|w| w.id).collect();
        if visible.is_empty() {
            return;
        }
        let offset = 40i32;
        let step = 30i32;
        let mut cx = offset;
        let mut cy = offset;
        for id in visible {
            if let Some(w) = self.windows.get_mut(&id) {
                if w.state == WinState::Maximized {
                    continue;
                }
                let nx = cx.clamp(0, self.screen_w as i32 - w.w as i32);
                let ny = cy.clamp(0, self.screen_h as i32 - w.h as i32);
                w.x = nx;
                w.y = ny;
                if !w.flags.contains(WindowFlags::NO_DECORATION) {
                    w.client_x = nx + BORDER as i32;
                    w.client_y = ny + TITLEBAR_H as i32;
                } else {
                    w.client_x = nx;
                    w.client_y = ny;
                }
                cx += step;
                cy += step;
                if cx + w.w as i32 > self.screen_w as i32 {
                    cx = offset;
                    cy += step;
                }
            }
        }
    }

    /// 网格布局：把全部可见窗口平铺到屏幕。
    pub fn tile_windows(&mut self) {
        let visible: Vec<WindowId> = self.visible_windows().iter().map(|w| w.id).collect();
        let n = visible.len();
        if n == 0 {
            return;
        }
        let cols = (n as f64).sqrt().ceil() as u32;
        let rows = (n as u32 + cols - 1) / cols;
        let cell_w = self.screen_w / cols;
        let cell_h = self.screen_h / rows;
        for (i, id) in visible.iter().enumerate() {
            if let Some(w) = self.windows.get_mut(*id) {
                if w.state == WinState::Maximized {
                    continue;
                }
                let col = (i as u32) % cols;
                let row = (i as u32) / cols;
                let x = (col as u32 * cell_w) as i32;
                let y = (row as u32 * cell_h) as i32;
                w.w = cell_w.saturating_sub(4);
                w.h = cell_h.saturating_sub(4);
                w.x = x + 2;
                w.y = y + 2;
                if !w.flags.contains(WindowFlags::NO_DECORATION) {
                    w.client_x = w.x + BORDER as i32;
                    w.client_y = w.y + TITLEBAR_H as i32;
                    w.client_w = w.w.saturating_sub(BORDER * 2).max(1);
                    w.client_h = w.h.saturating_sub(TITLEBAR_H + BORDER).max(1);
                } else {
                    w.client_x = w.x;
                    w.client_y = w.y;
                    w.client_w = w.w;
                    w.client_h = w.h;
                }
            }
        }
    }

    /// 鼠标命中测试：自上而下查找第一个命中的窗口及其部件。
    pub fn hit_test(&self, x: i32, y: i32) -> Option<(WindowId, Hit)> {
        for w in self.windows_by_zorder().iter().rev() {
            let hit = w.hit(x, y);
            if hit != Hit::None {
                return Some((w.id, hit));
            }
        }
        None
    }

    /// 返回最上层普通窗口（用于焦点回退）。
    fn topmost_normal(&self) -> Option<&Window> {
        self.windows_by_zorder()
            .iter()
            .rev()
            .find(|w| w.state == WinState::Normal || w.state == WinState::Maximized)
            .copied()
    }

    /// 返回窗口总数。
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// 迭代全部窗口（id 升序）。
    pub fn iter(&self) -> impl Iterator<Item = (WindowId, &Window)> {
        self.windows.iter().map(|(id, w)| (*id, w))
    }

    /// 返回焦点窗口标题。
    pub fn focused_title(&self) -> Option<String> {
        self.focused_window().map(|w| w.title.clone())
    }

    /// 设置窗口所属应用。
    pub fn set_app(&mut self, id: WindowId, app: &str) -> bool {
        self.windows.get_mut(&id).map_or(false, |w| {
            w.app = Some(app.to_string());
            true
        })
    }
}

// ============================================================================
// 窗口装饰渲染（把窗口标题栏、边框、按钮画到目标表面）
// ============================================================================

impl WindowManager {
    /// 绘制单个窗口的装饰（标题栏 + 边框 + 按钮）到表面。
    /// 客户区内容由窗口所属应用自行绘制，这里只画装饰。
    pub fn render_decoration(&self, win: &Window, surface: &mut Surface) {
        if win.state == WinState::Hidden || win.state == WinState::Minimized {
            return;
        }
        let focused = self.focused == Some(win.id);
        let bg = if focused { self.title_bg } else { palette::SURFACE1 };
        let fg = if focused { self.title_fg } else { palette::SUBTEXT0 };

        // 标题栏背景。
        let tb = win.titlebar_rect();
        if tb.w > 0 && tb.h > 0 {
            surface.fill_rect(tb.x, tb.y, tb.w, tb.h, bg);
            // 标题栏文字。
            surface.draw_text(tb.x + 8, tb.y + 4, &win.title, fg, 2);
            // 标题栏底边线。
            surface.draw_line(
                tb.x,
                tb.y + tb.h as i32 - 1,
                tb.x + tb.w as i32,
                tb.y + tb.h as i32 - 1,
                self.border_color,
            );
        }

        // 窗口边框。
        surface.draw_rect(win.x, win.y, win.w, win.h, self.border_color, 1);

        if !win.flags.contains(WindowFlags::NO_DECORATION) {
            // 关闭按钮（红）。
            let cr = win.close_rect();
            surface.fill_rect(cr.x, cr.y, cr.w, cr.h, palette::RED);
            surface.draw_text(cr.x + 2, cr.y + 2, "✕", palette::BASE, 2);
            // 最小化按钮（黄）。
            if win.flags.contains(WindowFlags::MINIMIZABLE) {
                let mr = win.minimize_rect();
                surface.fill_rect(mr.x, mr.y, mr.w, mr.h, palette::YELLOW);
                surface.draw_line(mr.x + 2, mr.y + 6, mr.x + mr.w as i32 - 2, mr.y + 6, palette::BASE);
            }
            // 最大化按钮（绿）。
            if win.flags.contains(WindowFlags::MAXIMIZABLE) {
                let mxr = win.maximize_rect();
                surface.fill_rect(mxr.x, mxr.y, mxr.w, mxr.h, palette::GREEN);
                surface.draw_rect(mxr.x + 2, mxr.y + 2, mxr.w.saturating_sub(4), mxr.h.saturating_sub(4), palette::BASE, 1);
            }
            // 调整大小角。
            if win.flags.contains(WindowFlags::RESIZABLE) {
                let rr = win.resize_corner_rect();
                surface.draw_rect(rr.x, rr.y, rr.w, rr.h, self.border_color, 1);
            }
        }
    }

    /// 绘制全部窗口装饰（按 z-order 从底到顶）。
    pub fn render_all_decorations(&self, surface: &mut Surface) {
        for w in self.windows_by_zorder() {
            self.render_decoration(w, surface);
        }
    }
}

// ============================================================================
// 全局窗口管理器单例（供 kernel/sense.rs 调用）
// ============================================================================

static WINDOW_MANAGER: OnceLock<Mutex<WindowManager>> = OnceLock::new();

/// 获取全局窗口管理器。
pub fn window_manager() -> &'static Mutex<WindowManager> {
    WINDOW_MANAGER.get_or_init(|| Mutex::new(WindowManager::new(960, 600)))
}

/// `sense gui://wm`：返回窗口管理器状态摘要。
pub fn wm_sense() -> String {
    let mgr = window_manager().lock().unwrap();
    let mut lines = vec![
        format!(
            "windows={} focused={:?} screen={}x{}",
            mgr.len(),
            mgr.focused,
            mgr.screen_w,
            mgr.screen_h
        ),
        format!("click_to_raise={}", mgr.click_to_raise),
    ];
    for (id, w) in mgr.iter() {
        lines.push(format!(
            "  #{}: title={:?} state={} z={} pos=({},{} ) size={}x{} app={:?}",
            id,
            w.title,
            w.state.label(),
            w.z_level,
            w.x,
            w.y,
            w.w,
            w.h,
            w.app
        ));
    }
    lines.join("\n")
}

/// `act gui://wm <op> ...`：对窗口执行行动。
pub fn wm_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mgr = window_manager();
    let mut m = mgr.lock().unwrap();
    match it.next().unwrap_or("") {
        "create" => {
            let title = it.next().unwrap_or("window");
            let x: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(100);
            let y: i32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(100);
            let w: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(400);
            let h: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(300);
            let id = m.create_window(Rect::new(x, y, w, h), title, WindowFlags::all());
            Ok(format!("已创建窗口 #{}", id))
        }
        "show" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.show(id) {
                Ok(format!("窗口 #{} 已显示", id))
            } else {
                Err(format!("窗口 #{} 不存在", id))
            }
        }
        "hide" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.hide(id) {
                Ok(format!("窗口 #{} 已隐藏", id))
            } else {
                Err(format!("窗口 #{} 不存在", id))
            }
        }
        "close" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.close(id) {
                Ok(format!("窗口 #{} 已关闭", id))
            } else {
                Err(format!("窗口 #{} 不存在", id))
            }
        }
        "focus" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.focus(id) {
                Ok(format!("窗口 #{} 已聚焦", id))
            } else {
                Err(format!("无法聚焦窗口 #{}", id))
            }
        }
        "blur" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.blur(id) {
                Ok(format!("窗口 #{} 已失焦点", id))
            } else {
                Err(format!("窗口 #{} 没有焦点", id))
            }
        }
        "minimize" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.minimize(id) {
                Ok(format!("窗口 #{} 已最小化", id))
            } else {
                Err(format!("无法最小化窗口 #{}", id))
            }
        }
        "maximize" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.maximize(id) {
                Ok(format!("窗口 #{} 已最大化", id))
            } else {
                Err(format!("无法最大化窗口 #{}", id))
            }
        }
        "restore" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            if m.restore(id) {
                Ok(format!("窗口 #{} 已还原", id))
            } else {
                Err(format!("无法还原窗口 #{}", id))
            }
        }
        "move" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            let x: i32 = it.next().ok_or("缺少 x")?.parse().map_err(|e| e.to_string())?;
            let y: i32 = it.next().ok_or("缺少 y")?.parse().map_err(|e| e.to_string())?;
            if m.move_window(id, x, y) {
                Ok(format!("窗口 #{} 已移动到 ({},{})", id, x, y))
            } else {
                Err(format!("无法移动窗口 #{}", id))
            }
        }
        "resize" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            let w: u32 = it.next().ok_or("缺少 w")?.parse().map_err(|e| e.to_string())?;
            let h: u32 = it.next().ok_or("缺少 h")?.parse().map_err(|e| e.to_string())?;
            if m.resize(id, w, h) {
                Ok(format!("窗口 #{} 已缩放到 {}x{}", id, w, h))
            } else {
                Err(format!("无法缩放窗口 #{}", id))
            }
        }
        "z_order" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            let level: i32 = it.next().ok_or("缺少 level")?.parse().map_err(|e| e.to_string())?;
            if m.z_order(id, level) {
                Ok(format!("窗口 #{} z_order={}", id, level))
            } else {
                Err(format!("窗口 #{} 不存在", id))
            }
        }
        "raise" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e| e.to_string())?;
            m.raise_to_top(id);
            Ok(format!("窗口 #{} 已置顶", id))
        }
        "alt_tab" => {
            if let Some(id) = m.alt_tab_next() {
                Ok(format!("Alt-Tab 切换到窗口 #{}", id))
            } else {
                Ok("无可见窗口".to_string())
            }
        }
        "layout" => {
            m.layout_windows();
            Ok("窗口已级联布局".to_string())
        }
        "tile" => {
            m.tile_windows();
            Ok("窗口已平铺布局".to_string())
        }
        "list" => Ok(wm_sense()),
        _ => Err(
            "用法: wm create|show|hide|close|focus|blur|minimize|maximize|restore|move|resize|z_order|raise|alt_tab|layout|tile|list".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wm_create_window() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(10, 10, 400, 300), "test", WindowFlags::all());
        assert!(id > 0);
        let w = mgr.window(id).unwrap();
        assert_eq!(w.title, "test");
        assert_eq!(w.x, 10);
        assert_eq!(w.y, 10);
        assert_eq!(w.state, WinState::Normal);
        assert_eq!(mgr.focused(), Some(id));
    }

    #[test]
    fn wm_window_clamped_to_screen() {
        let mut mgr = WindowManager::new(200, 200);
        let id = mgr.create_window(Rect::new(150, 150, 100, 100), "big", WindowFlags::all());
        let w = mgr.window(id).unwrap();
        assert!(w.x + w.w as i32 <= 200);
        assert!(w.y + w.h as i32 <= 200);
    }

    #[test]
    fn wm_show_hide() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        assert!(mgr.hide(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Hidden);
        assert!(mgr.show(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Normal);
    }

    #[test]
    fn wm_close() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        assert!(mgr.close(id));
        assert!(mgr.window(id).is_none());
    }

    #[test]
    fn wm_focus_raise() {
        let mut mgr = WindowManager::new(960, 600);
        let a = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        let b = mgr.create_window(Rect::new(10, 10, 100, 100), "b", WindowFlags::all());
        // b 创建后自动聚焦且置顶。
        assert_eq!(mgr.focused(), Some(b));
        assert!(mgr.window(b).unwrap().z_level > mgr.window(a).unwrap().z_level);
        // 聚焦 a，a 应置顶。
        assert!(mgr.focus(a));
        assert_eq!(mgr.focused(), Some(a));
    }

    #[test]
    fn wm_minimize_maximize_restore() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(50, 50, 300, 200), "win", WindowFlags::all());
        let saved_x = mgr.window(id).unwrap().x;
        assert!(mgr.minimize(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Minimized);
        assert!(mgr.restore(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Normal);
        assert_eq!(mgr.window(id).unwrap().x, saved_x);
        assert!(mgr.maximize(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Maximized);
        assert!(mgr.restore(id));
        assert_eq!(mgr.window(id).unwrap().state, WinState::Normal);
    }

    #[test]
    fn wm_move_resize() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(0, 0, 100, 100), "win", WindowFlags::all());
        assert!(mgr.move_window(id, 200, 150));
        let w = mgr.window(id).unwrap();
        assert_eq!(w.x, 200);
        assert_eq!(w.y, 150);
        assert!(mgr.resize(id, 500, 400));
        let w = mgr.window(id).unwrap();
        assert_eq!(w.w, 500);
        assert_eq!(w.h, 400);
    }

    #[test]
    fn wm_z_order() {
        let mut mgr = WindowManager::new(960, 600);
        let a = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        let b = mgr.create_window(Rect::new(10, 10, 100, 100), "b", WindowFlags::all());
        assert!(mgr.z_order(a, 100));
        assert_eq!(mgr.window(a).unwrap().z_level, 100);
        assert!(!mgr.z_order(99999, 1));
    }

    #[test]
    fn wm_alt_tab_cycle() {
        let mut mgr = WindowManager::new(960, 600);
        let a = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        let b = mgr.create_window(Rect::new(10, 10, 100, 100), "b", WindowFlags::all());
        let c = mgr.create_window(Rect::new(20, 20, 100, 100), "c", WindowFlags::all());
        // 当前焦点在 c。
        assert_eq!(mgr.alt_tab_next(), Some(a));
        assert_eq!(mgr.alt_tab_next(), Some(b));
        assert_eq!(mgr.alt_tab_next(), Some(c));
        assert_eq!(mgr.alt_tab_prev(), Some(b));
    }

    #[test]
    fn wm_layout_and_tile() {
        let mut mgr = WindowManager::new(800, 600);
        let _ = mgr.create_window(Rect::new(0, 0, 200, 150), "a", WindowFlags::all());
        let _ = mgr.create_window(Rect::new(0, 0, 200, 150), "b", WindowFlags::all());
        let _ = mgr.create_window(Rect::new(0, 0, 200, 150), "c", WindowFlags::all());
        mgr.layout_windows();
        // 级联后窗口位置应不同。
        let ws: Vec<&Window> = mgr.windows_by_zorder();
        assert!(ws[1].x > ws[0].x || ws[1].y > ws[0].y);
        mgr.tile_windows();
        // 平铺后窗口不应重叠太多。
        assert!(ws.len() == 3);
    }

    #[test]
    fn wm_hit_test() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(10, 10, 200, 150), "win", WindowFlags::all());
        let (hit_id, hit) = mgr.hit_test(50, 30).unwrap();
        assert_eq!(hit_id, id);
        assert_eq!(hit, Hit::TitleBar);
        let (_, hit) = mgr.hit_test(50, 100).unwrap();
        assert_eq!(hit, Hit::Client);
        // 关闭按钮区域。
        let cr = mgr.window(id).unwrap().close_rect();
        let (_, hit) = mgr.hit_test(cr.x + 2, cr.y + 2).unwrap();
        assert_eq!(hit, Hit::CloseButton);
    }

    #[test]
    fn wm_windows_by_zorder() {
        let mut mgr = WindowManager::new(960, 600);
        let a = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        let b = mgr.create_window(Rect::new(0, 0, 100, 100), "b", WindowFlags::all());
        let order = mgr.windows_by_zorder();
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].id, a);
        assert_eq!(order[1].id, b);
    }

    #[test]
    fn wm_hidden_excluded_from_layout() {
        let mut mgr = WindowManager::new(960, 600);
        let a = mgr.create_window(Rect::new(0, 0, 100, 100), "a", WindowFlags::all());
        let b = mgr.create_window(Rect::new(0, 0, 100, 100), "b", WindowFlags::all());
        mgr.hide(b);
        let visible = mgr.visible_windows();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, a);
    }

    #[test]
    fn wm_sense_and_act() {
        let mut mgr = window_manager().lock().unwrap();
        let id = mgr.create_window(Rect::new(0, 0, 100, 100), "wm-test", WindowFlags::all());
        drop(mgr);
        let sense = wm_sense();
        assert!(sense.contains(&format!("#{}", id)));
        let res = wm_act(&format!("focus {}", id)).unwrap();
        assert!(res.contains("聚焦"));
        let res = wm_act("list").unwrap();
        assert!(sense.contains("windows="));
        let err = wm_act("focus 99999").unwrap_err();
        assert!(err.contains("不存在"));
    }

    #[test]
    fn wm_decoration_render_no_panic() {
        let mut mgr = WindowManager::new(960, 600);
        let id = mgr.create_window(Rect::new(10, 10, 300, 200), "decoration", WindowFlags::all());
        let win = mgr.window(id).unwrap().clone();
        let mut surface = Surface::filled(960, 600, palette::BASE);
        mgr.render_decoration(&win, &mut surface);
        mgr.render_all_decorations(&mut surface);
    }

    #[test]
    fn wm_state_labels() {
        assert_eq!(WinState::Normal.label(), "normal");
        assert_eq!(WinState::Minimized.label(), "minimized");
        assert_eq!(WinState::Maximized.label(), "maximized");
        assert_eq!(WinState::Hidden.label(), "hidden");
    }

    #[test]
    fn wm_window_flags() {
        let mut f = WindowFlags::empty();
        assert!(!f.contains(WindowFlags::CLOSABLE));
        f.insert(WindowFlags::CLOSABLE);
        assert!(f.contains(WindowFlags::CLOSABLE));
        f.remove(WindowFlags::CLOSABLE);
        assert!(!f.contains(WindowFlags::CLOSABLE));
    }
}
