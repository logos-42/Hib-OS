//! 顶栏（Status Rail）—— 一屏之内回答四个问题：
//!
//! 1. 我在哪？（当前焦点窗口）
//! 2. 系统健康吗？（窗口数 / 帧号）
//! 3. 现在是什么模式？（NORMAL / COMMAND）
//! 4. 刚才那步做了什么？（`last_result`，命令板的执行回执）
//!
//! 顶栏本身也是入口：点击任意位置打开命令板（与 ⌘K 等价）。

use super::surface::{palette as pal, Rect, Surface};
use super::window::Window;

/// 顶栏高度（像素）。统一指向新外壳常量，避免与 palette / desktop_shell 打架。
pub const HEIGHT: u32 = super::desktop_shell::TOPBAR_H as u32;

/// 画顶栏。调用时机：窗口与任务栏之后、命令板之前。
pub fn draw(
    dst: &mut Surface,
    windows: &[Window],
    focused: Option<u64>,
    frames: u64,
    palette_open: bool,
    last_result: &str,
) {
    let w = dst.width() as i32;
    dst.fill_rect(0, 0, dst.width(), HEIGHT, pal::MANTLE);
    dst.fill_rect(0, HEIGHT as i32 - 1, dst.width(), 1, pal::SURFACE0);

    // 品牌徽标
    dst.fill_round_rect(Rect::new(6, 4, 30, 14), 4, pal::MAUVE);
    dst.draw_text(11, 8, "HIB", pal::CRUST, 1);
    dst.draw_text(42, 8, "OS 0.2.1", pal::SUBTEXT0, 1);

    // 模式胶囊：命令模式时高亮，避免"我到底在哪个模式"的困惑。
    let (mode, mode_bg, mode_fg) = if palette_open {
        ("COMMAND", pal::MAUVE, pal::CRUST)
    } else {
        ("NORMAL", pal::SURFACE0, pal::OVERLAY0)
    };
    let mw = Surface::text_width(mode, 1) + 12;
    dst.fill_round_rect(Rect::new(96, 4, mw, 14), 7, mode_bg);
    dst.draw_text(102, 8, mode, mode_fg, 1);

    // 焦点窗口
    let focus = focused
        .and_then(|id| windows.iter().find(|w| w.id == id))
        .map(|w| format!("#{} {}", w.id, w.title))
        .unwrap_or_else(|| "desktop".to_string());
    dst.draw_text_clipped(
        96 + mw as i32 + 10,
        8,
        &format!("focus {}", focus),
        pal::TEXT,
        1,
        520,
    );

    // 右侧：回执 / 计数 / 时钟 / ⌘K
    let clock = clock_hms();
    let cw = Surface::text_width(&clock, 1) as i32;
    dst.draw_text(w - cw - 8, 8, &clock, pal::TEXT, 1);

    let hint = "⌘K";
    let hw = Surface::text_width(hint, 1) as i32;
    let hx = w - cw - hw - 22;
    dst.fill_round_rect(Rect::new(hx, 4, (hw + 12) as u32, 14), 4, pal::SURFACE0);
    dst.draw_text(hx + 6, 8, hint, pal::SUBTEXT0, 1);

    let stats = format!("win {}  frame {}", windows.len(), frames);
    let sw = Surface::text_width(&stats, 1) as i32;
    dst.draw_text(hx - sw - 14, 8, &stats, pal::OVERLAY0, 1);

    if !last_result.is_empty() {
        let max_x = hx - sw - 24;
        dst.draw_text_clipped(
            530,
            8,
            &format!("· {}", last_result),
            pal::TEAL,
            1,
            max_x,
        );
    }
}

/// 命中测试：顶栏是一条连续的"打开命令板"热区。
pub fn hit(y: i32) -> bool {
    (0..HEIGHT as i32).contains(&y)
}

pub fn clock_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

/// 顶栏底色（供测试断言）。
pub const BG: u32 = pal::MANTLE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_paints_brand_mode_and_clock() {
        let mut s = Surface::filled(960, 600, pal::BASE);
        let wins = vec![Window::new(1, "TERMINAL", 0, 40, 300, 200, pal::BLUE)];
        draw(&mut s, &wins, Some(1), 42, true, "opened files -> win#2");
        // 品牌徽标是 MAUVE
        assert_eq!(s.get(10, 10), Some(pal::MAUVE));
        // 模式胶囊在 COMMAND 时也是 MAUVE
        assert_eq!(s.get(100, 10), Some(pal::MAUVE));
        // 顶栏底边是分隔线
        assert_eq!(s.get(500, HEIGHT as i32 - 1), Some(pal::SURFACE0));
        // 顶栏以下不受影响
        assert_eq!(s.get(500, 100), Some(pal::BASE));
    }

    #[test]
    fn topbar_is_a_hit_target() {
        assert!(hit(0));
        assert!(hit(HEIGHT as i32 - 1));
        assert!(!hit(HEIGHT as i32));
    }

    #[test]
    fn clock_has_expected_shape() {
        let t = clock_hms();
        let parts: Vec<&str> = t.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit())));
    }

    #[test]
    fn bg_matches_palette() {
        assert_eq!(BG, pal::MANTLE);
    }
}
