//! 桌面图标网格：固定 64×64 格点，拖拽自动吸附，hover / 选中 / 拖拽预览三态。

use super::super::surface::Surface;
use super::{GRID, TOPBAR_H};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    Normal,
    Hover,
    Selected,
    Dragging,
}

#[derive(Clone, Debug)]
pub struct DesktopIcon {
    pub label: String,
    pub gx: i32,
    pub gy: i32,
    pub state: IconState,
}

impl DesktopIcon {
    pub fn new(label: &str, gx: i32, gy: i32) -> Self {
        Self { label: label.to_string(), gx, gy, state: IconState::Normal }
    }

    pub fn pixel_pos(&self) -> (i32, i32) {
        (self.gx * GRID + 16, TOPBAR_H + self.gy * GRID + 16)
    }
}

/// 把任意像素坐标吸附到最近格点（返回格点索引）。
pub fn snap_to_grid(x: i32, y: i32) -> (i32, i32) {
    let gx = if x >= 0 { x / GRID } else { (x - GRID + 1) / GRID };
    let gy = if y >= 0 { y / GRID } else { (y - GRID + 1) / GRID };
    (gx, gy)
}

pub fn draw(frame: &mut Surface) {
    let icons = [
        DesktopIcon::new("CHAT", 0, 0),
        DesktopIcon::new("TASK", 1, 0),
        DesktopIcon::new("FILE", 2, 0),
    ];
    for icon in &icons {
        let (x, y) = icon.pixel_pos();
        let bg = match icon.state {
            IconState::Hover => 0xFF2E3238,
            IconState::Selected => 0xFF37424A,
            IconState::Dragging => 0xFF3A4450,
            IconState::Normal => 0xFF23262B,
        };
        frame.fill_rect(x, y, 32, 32, bg);
        frame.draw_text(x + 7, y + 12, &icon.label, 0xFFC8CCD4, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_snaps_to_64px() {
        assert_eq!(snap_to_grid(70, 70), (1, 1));
        assert_eq!(snap_to_grid(64, 64), (1, 1));
        assert_eq!(snap_to_grid(10, 10), (0, 0));
    }

    #[test]
    fn icon_pixel_pos_aligned() {
        let icon = DesktopIcon::new("X", 2, 1);
        let (x, y) = icon.pixel_pos();
        assert_eq!(x % GRID, 16);
        assert!((y - TOPBAR_H) % GRID == 16);
    }
}
