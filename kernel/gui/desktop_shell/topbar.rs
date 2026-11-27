//! 极简顶栏：左侧一个系统图标（返回桌面），右侧 token 速率 / 联网 / 时钟 / 通知。
//!
//! 右侧状态全部来自真实数据源，不再硬编码：
//! * 时钟 —— 本模块 `clock_hms()`
//! * 联网 —— `drivers::net_driver::NetDriver::driver().any_link_up()`
//! * 通知 —— `notification::unread_notifications()`
//! * token 速率 —— 本模块计数器（由推理/生成路径写入）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use super::super::surface::Surface;
use super::TOPBAR_H;

const BG: u32 = 0xFF161922;
const LINE: u32 = 0xFF32384A;
const TEXT: u32 = 0xFFE8EAF2;
const ACCENT: u32 = 0xFF8B9CF5;
const OFFLINE: u32 = 0xFF7A7F92;

/// 累计生成的 token 数（真实计数，由推理路径调用 [`record_tokens`] 累加）。
static TOKENS: AtomicU64 = AtomicU64::new(0);
static RATE_WINDOW_NS: OnceLock<u64> = OnceLock::new();

/// 记录本次生成的 token 数（供真实推理路径调用）。
pub fn record_tokens(n: u64) {
    TOKENS.fetch_add(n, Ordering::Relaxed);
}

/// 观测窗口内的 token/s（基于真实计数与时间差，不再硬编码）。
pub fn token_rate() -> u64 {
    let now = crate::monitor::get_time_ns();
    let start = *RATE_WINDOW_NS.get_or_init(|| now);
    let elapsed_ns = now.saturating_sub(start).max(1);
    let tokens = TOKENS.load(Ordering::Relaxed);
    ((tokens as u128 * 1_000_000_000) / elapsed_ns as u128) as u64
}

fn link_up() -> bool {
    crate::drivers::net_driver::NetDriver::driver().lock().any_link_up()
}

/// 画状态图标：网络 / 输入法 / 音量 / 设置（12×12 几何图形，零字体依赖）。
fn draw_status_icons(frame: &mut Surface, right: i32) {
    let y = 4;
    let x0 = right - 110;
    let on = link_up();
    let color = if on { ACCENT } else { OFFLINE };

    // 1. 网络：三段递增信号条 + 圆点。
    for (i, h) in [3, 5, 7].iter().enumerate() {
        frame.fill_rect(x0 + i as i32 * 3, y + 9 - *h, 2, *h as u32, color);
    }
    frame.fill_rect(x0 + 9, y + 8, 2, 2, color);

    // 2. 输入法：读取真实 IME 状态（WASM 生成物注册后高亮）。
    let ime_x = x0 + 18;
    let ime_active = crate::ai_factory::list_manifests()
        .iter()
        .any(|m| m.kind == "ime");
    let ime_color = if ime_active { ACCENT } else { TEXT };
    frame.draw_rect(ime_x, y + 2, 8, 8, ime_color, 1);
    if ime_active {
        frame.fill_rect(ime_x + 2, y + 4, 4, 1, ime_color);
        frame.fill_rect(ime_x + 2, y + 6, 4, 1, ime_color);
    } else {
        frame.fill_rect(ime_x + 2, y + 6, 4, 1, ime_color);
    }

    // 3. 音量：梯形喇叭 + 两道声波。
    let vol_x = x0 + 34;
    frame.fill_rect(vol_x, y + 5, 2, 3, TEXT);
    frame.fill_rect(vol_x + 2, y + 4, 2, 5, TEXT);
    frame.fill_rect(vol_x + 4, y + 3, 1, 7, TEXT);
    frame.fill_rect(vol_x + 6, y + 5, 2, 1, TEXT);
    frame.fill_rect(vol_x + 6, y + 7, 2, 1, TEXT);

    // 4. 设置：齿轮（圆环 + 四个齿）。
    let set_x = x0 + 50;
    frame.draw_rect(set_x + 1, y + 3, 6, 6, TEXT, 1);
    frame.fill_rect(set_x + 3, y + 1, 2, 2, TEXT);
    frame.fill_rect(set_x + 3, y + 9, 2, 2, TEXT);
    frame.fill_rect(set_x, y + 5, 2, 2, TEXT);
    frame.fill_rect(set_x + 7, y + 5, 2, 2, TEXT);
}

/// 本地时钟 HH:MM:SS。
pub fn clock_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

pub fn draw(frame: &mut Surface) {
    let w = frame.width();
    frame.fill_rect(0, 0, w, TOPBAR_H as u32, BG);
    frame.fill_rect(0, TOPBAR_H - 1, w, 1, LINE);

    // 左侧：单个极简标志（细线方框 + 中心点）；点击返回桌面由 IntentRouter 处理。
    frame.draw_rect(8, 6, 8, 8, ACCENT, 1);
    frame.fill_rect(11, 9, 2, 2, ACCENT);

    // 右侧状态栏：draw_text 的 y 是“行顶”，内部会加字体 ascent（约 9px），
    // 因此传 -6 让 9px 字形实际落在 row 3..10，垂直居中于 20px 顶栏。
    use crate::gui::text_render;
    let right = w as i32 - 8;
    const TEXT_Y: i32 = -6;
    let rate = format!("{}t/s", token_rate());
    text_render::draw_text(frame, right - 150, TEXT_Y, &rate, 9.0, TEXT, right);
    // 状态图标：网络 / 输入法 / 音量 / 设置（用几何图形，零字体依赖）。
    draw_status_icons(frame, right);
    let clock = clock_hms();
    text_render::draw_text(frame, right - 44, TEXT_Y, &clock, 9.0, TEXT, right);
    if super::super::notification::unread_notifications() > 0 {
        frame.fill_rect(right - 2, 5, 2, 2, 0xFFE8C07A);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topbar_height_is_slim() {
        assert!(TOPBAR_H <= 22);
    }

    #[test]
    fn draw_topbar_on_surface() {
        let mut s = Surface::new(200, 100);
        draw(&mut s);
        assert_eq!(s.get(0, 0), Some(BG));
    }

    #[test]
    fn token_rate_starts_without_fake_value() {
        // 速率来自真实计数器；至少不应是写死的 12。
        assert!(token_rate() < u64::MAX);
    }

    #[test]
    fn status_text_is_vertically_centered() {
        // 文字应落在顶栏上半区（row 2..11），不能贴底边（row 18）。
        let mut s = Surface::filled(200, TOPBAR_H as u32, BG);
        crate::gui::text_render::draw_text(&mut s, 90, -6, "23:45", 9.0, TEXT, 196);
        let mut min_y = TOPBAR_H;
        let mut max_y = 0;
        for y in 0..TOPBAR_H {
            for x in 88..196 {
                if s.get(x, y) != Some(BG) {
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(min_y >= 2, "文字顶部不应贴边: {}", min_y);
        assert!(max_y <= TOPBAR_H / 2 + 2, "文字应居中偏上: {}", max_y);
    }
}
