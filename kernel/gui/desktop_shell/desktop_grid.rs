//! 桌面图标网格：固定 64×64 单元格，拖拽自动吸附，hover / 选中 / 拖拽预览三态。

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

/// 把任意像素坐标吸附到最近网格单元（返回行、列索引）。
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
    let count = icons.len() as i32;
    // 整行图标居中：总宽 = 单元格宽 × 数量，水平居中；垂直放在工作区上四分之一处。
    let total_w = count * GRID;
    let origin_x = (frame.width() as i32 - total_w) / 2;
    let content_h = frame.height() as i32 - TOPBAR_H;
    let origin_y = TOPBAR_H + content_h / 4;
    for icon in &icons {
        let x = origin_x + icon.gx * GRID + (GRID - 32) / 2;
        let y = origin_y + icon.gy * GRID;
        let bg = match icon.state {
            IconState::Hover => 0xFF222736,
            IconState::Selected => 0xFF2B3244,
            IconState::Dragging => 0xFF323A50,
            IconState::Normal => 0xFF1E222E,
        };
        frame.fill_rect(x, y, 32, 32, bg);
        let label: String = icon.label.chars().take(3).collect();
        frame.draw_ui_text(x + 4, y + 10, &label, 0xFFE8EAF2, x + 30);
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
