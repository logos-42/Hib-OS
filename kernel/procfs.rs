//! 虚拟文件系统快照 (procfs) —— AI 原生的「系统自检报告」。
//!
//! 类比 Linux 的 `/proc/stat`、`/proc/meminfo`、`/proc/uptime`，
//! 但这里不生成真实目录项，而是把全部关键指标聚合成一条只读通道
//! `proc://sysinfo`，让 AI 用一次 `sense` 就拿到整机体检结果。
//!
//! * `sense proc://sysinfo`        —— 读取快照（不递增采样计数）
//! * `act proc://sysinfo refresh`  —— 递增采样计数后返回快照

use std::sync::{Mutex, OnceLock};

use crate::gpu::vram;
use crate::mem::mem;
use crate::monitor::get_stats;
use crate::proc::proctable;

/// procfs 采样器。
pub struct ProcFs {
    /// 被 `refresh` 触发的采样次数。
    pub refreshes: u64,
}

impl ProcFs {
    fn new() -> Self {
        Self { refreshes: 0 }
    }

    /// 采样一次并返回快照。
    pub fn refresh(&mut self) -> String {
        self.refreshes += 1;
        self.snapshot()
    }

    /// 读取当前快照（纯只读，不改状态）。
    pub fn snapshot(&self) -> String {
        let procs = proctable().lock().unwrap().count();
        let stats = get_stats();
        let (mem_total, mem_used) = mem().lock().unwrap().usage();
        let (vram_total, vram_free) = {
            let v = vram().lock().unwrap();
            (v.total_vram(), v.available())
        };

        // 负载用「十分位」整数表示，避免浮点：每个进程约贡献 0.5 的 1 分钟负载。
        let procs_u64 = procs as u64;
        let load_1 = procs_u64.saturating_mul(5);
        let load_5 = procs_u64.saturating_mul(4);
        let load_15 = procs_u64.saturating_mul(3);

        let uptime_ns = stats.system_uptime_ns;

        format!(
            concat!(
                "version: Hib OS ",
                crate::version::hibos_version!(),
                " (GPU-Centric macro-kernel)\n\
                 uptime: {}ms ({}ns)\n\
                 loadavg: {} {} {} (procs={}, running={})\n\
                 meminfo: total={}MiB used={}MiB free={}MiB\n\
                 vram: total={}MiB used={}MiB free={}MiB\n\
                 tokens: total={} active={} completed={} failed={} avg={}ns\n\
                 procfs: refreshes={}"
            ),
            uptime_ns / 1_000_000,
            uptime_ns,
            fmt_tenths(load_1),
            fmt_tenths(load_5),
            fmt_tenths(load_15),
            procs,
            procs,
            mem_total / (1024 * 1024),
            mem_used / (1024 * 1024),
            (mem_total - mem_used) / (1024 * 1024),
            vram_total / (1024 * 1024),
            (vram_total - vram_free) / (1024 * 1024),
            vram_free / (1024 * 1024),
            stats.total_tokens,
            stats.active_tokens,
            stats.total_tasks_completed,
            stats.total_tasks_failed,
            stats.avg_task_time_ns,
            self.refreshes
        )
    }
}

/// 把十分位整数渲染成 `1.5` 形式。
fn fmt_tenths(v: u64) -> String {
    format!("{}.{}", v / 10, v % 10)
}

static PROCFS: OnceLock<Mutex<ProcFs>> = OnceLock::new();

/// 全局 procfs 采样器（`proc://sysinfo` 的实体）。
pub fn procfs() -> &'static Mutex<ProcFs> {
    PROCFS.get_or_init(|| Mutex::new(ProcFs::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_all_sections() {
        let p = ProcFs::new();
        let s = p.snapshot();
        for key in ["version:", "uptime:", "loadavg:", "meminfo:", "vram:", "tokens:"] {
            assert!(s.contains(key), "缺少 {}: {}", key, s);
        }
        assert!(s.contains("refreshes=0"), "{}", s);
    }

    #[test]
    fn refresh_increments_counter() {
        let mut p = ProcFs::new();
        let first = p.refresh();
        assert!(first.contains("refreshes=1"), "{}", first);
        let second = p.refresh();
        assert!(second.contains("refreshes=2"), "{}", second);
        assert_eq!(p.refreshes, 2);
    }

    #[test]
    fn loadavg_is_tenths_formatted() {
        assert_eq!(fmt_tenths(0), "0.0");
        assert_eq!(fmt_tenths(5), "0.5");
        assert_eq!(fmt_tenths(15), "1.5");
    }
}
