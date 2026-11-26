//! 桌面外壳 —— 极简顶栏、网格桌面、三态 Dock。
//!
//! 外壳是一等语义层：顶栏只有左侧一个图标和右侧状态；桌面只是入口；
//! Dock 图标分常驻 / 条件显示 / 隐藏三类。风格为极简现代，不是像素放大风。

pub mod desktop_grid;
pub mod dock;
pub mod topbar;

use std::sync::{Mutex, OnceLock};

use super::surface::Surface;
use super::window::Window;

pub const TOPBAR_H: i32 = 20;
pub const DOCK_H: i32 = 56;
pub const GRID: i32 = 64;

static DOCK: OnceLock<Mutex<dock::Dock>> = OnceLock::new();

pub fn dock_state() -> &'static Mutex<dock::Dock> {
    DOCK.get_or_init(|| Mutex::new(dock::Dock::new()))
}

/// 合成外壳到帧缓冲的最上层。
pub fn draw_shell(frame: &mut Surface, windows: &[Window], focused: Option<u64>) {
    topbar::draw(frame);
    desktop_grid::draw(frame);
    dock::draw(frame, windows, focused);
}
