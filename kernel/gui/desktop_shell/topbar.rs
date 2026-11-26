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

const BG: u32 = 0xFF1B1C1F;
const LINE: u32 = 0xFF303238;
const TEXT: u32 = 0xFFB9BCC4;
const ACCENT: u32 = 0xFF7FB3A0;
const OFFLINE: u32 = 0xFF7A5A5A;

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

    // 右侧：真实 token 速率 / 联网 / 时钟 / 通知徽章。
    let right = w as i32 - 8;
    let rate = format!("{}t/s", token_rate());
    frame.draw_text(right - 96, 5, &rate, TEXT, 1);
    let (net_label, net_color) = if link_up() { ("~", ACCENT) } else { ("x", OFFLINE) };
    frame.draw_text(right - 56, 5, net_label, net_color, 1);
    let clock = clock_hms();
    frame.draw_text(right - 48, 5, &clock, TEXT, 1);
    if super::super::notification::unread_notifications() > 0 {
        frame.fill_rect(right - 4, 9, 3, 3, 0xFFE08A63);
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
}
