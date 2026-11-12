//! 系统监控与性能统计。

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// 单调递增的系统运行时间 (纳秒)。
pub fn get_time_ns() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// 系统统计快照。
#[derive(Clone, Debug, Default)]
pub struct SystemStats {
    pub total_tokens: u64,
    pub active_tokens: u64,
    pub total_vram_used: u64,
    pub total_vram_capacity: u64,
    pub total_tasks_completed: u64,
    pub total_tasks_failed: u64,
    pub avg_task_time_ns: u64,
    pub system_uptime_ns: u64,
}

/// 监控管理器。
pub struct Monitor {
    stats: SystemStats,
    recent_task_times: Vec<u64>,
    max_recent_records: usize,
}

impl Monitor {
    pub fn new(total_vram_capacity: u64) -> Self {
        Self {
            stats: SystemStats {
                total_vram_capacity,
                ..Default::default()
            },
            recent_task_times: Vec::new(),
            max_recent_records: 100,
        }
    }

    pub fn record_token_created(&mut self) {
        self.stats.total_tokens += 1;
        self.stats.active_tokens += 1;
    }

    pub fn record_token_destroyed(&mut self) {
        self.stats.active_tokens = self.stats.active_tokens.saturating_sub(1);
    }

    pub fn record_task_completed(&mut self, exec_time_ns: u64) {
        self.stats.total_tasks_completed += 1;
        if self.recent_task_times.len() >= self.max_recent_records {
            self.recent_task_times.remove(0);
        }
        self.recent_task_times.push(exec_time_ns);
        let sum: u64 = self.recent_task_times.iter().sum();
        self.stats.avg_task_time_ns = sum / self.recent_task_times.len() as u64;
    }

    pub fn record_task_failed(&mut self) {
        self.stats.total_tasks_failed += 1;
    }

    pub fn set_vram_used(&mut self, used: u64) {
        self.stats.total_vram_used = used;
    }

    pub fn get_stats(&self) -> SystemStats {
        let mut s = self.stats.clone();
        s.system_uptime_ns = get_time_ns();
        s
    }

    pub fn reset(&mut self) {
        self.stats = SystemStats {
            total_vram_capacity: self.stats.total_vram_capacity,
            ..Default::default()
        };
        self.recent_task_times.clear();
    }
}

static MONITOR: OnceLock<Mutex<Monitor>> = OnceLock::new();

/// 全局监控管理器。
pub fn monitor() -> &'static Mutex<Monitor> {
    MONITOR.get_or_init(|| Mutex::new(Monitor::new(crate::gpu::vram::VramManager::DEFAULT_TOTAL_VRAM)))
}

/// 初始化监控子系统。
pub fn init() {
    let _ = monitor();
}

pub fn record_token_created() {
    monitor().lock().unwrap().record_token_created();
}

pub fn record_token_destroyed() {
    monitor().lock().unwrap().record_token_destroyed();
}

pub fn record_task_completed(exec_time_ns: u64) {
    monitor().lock().unwrap().record_task_completed(exec_time_ns);
}

pub fn record_task_failed() {
    monitor().lock().unwrap().record_task_failed();
}

pub fn get_stats() -> SystemStats {
    monitor().lock().unwrap().get_stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_average() {
        let mut m = Monitor::new(1024);
        m.record_token_created();
        m.record_token_created();
        m.record_token_destroyed();
        m.record_task_completed(100);
        m.record_task_completed(300);
        m.record_task_failed();
        m.set_vram_used(512);

        let s = m.get_stats();
        assert_eq!(s.total_tokens, 2);
        assert_eq!(s.active_tokens, 1);
        assert_eq!(s.total_tasks_completed, 2);
        assert_eq!(s.total_tasks_failed, 1);
        assert_eq!(s.avg_task_time_ns, 200);
        assert_eq!(s.total_vram_capacity, 1024);
        assert_eq!(s.total_vram_used, 512);
    }

    #[test]
    fn reset_clears_counters() {
        let mut m = Monitor::new(2048);
        m.record_token_created();
        m.record_task_completed(50);
        m.reset();
        let s = m.get_stats();
        assert_eq!(s.total_tokens, 0);
        assert_eq!(s.total_tasks_completed, 0);
        assert_eq!(s.total_vram_capacity, 2048);
    }

    #[test]
    fn rolling_window_is_bounded() {
        let mut m = Monitor::new(0);
        for i in 0..150 {
            m.record_task_completed(i);
        }
        assert_eq!(m.get_stats().total_tasks_completed, 150);
        assert_eq!(m.recent_task_times.len(), 100);
    }

    #[test]
    fn time_is_monotonic() {
        let a = get_time_ns();
        let b = get_time_ns();
        assert!(b >= a);
    }
}

