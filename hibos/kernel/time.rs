//! 内核时间戳 —— no_std 环境下的简单时钟。
//! 当前基于原子计数器模拟（每次调用推进 1 秒），后续可接入 TSC / PIT / HPET。

use core::sync::atomic::{AtomicU64, Ordering};

static TIME_NS: AtomicU64 = AtomicU64::new(0);

/// 获取当前纳秒时间戳（单调递增，模拟）。
pub fn get_time_ns() -> u64 {
    TIME_NS.fetch_add(1_000_000_000, Ordering::Relaxed)
}

/// 推进时间戳（供测试或定时器调用）。
pub fn advance_ns(ns: u64) {
    TIME_NS.fetch_add(ns, Ordering::Relaxed);
}
