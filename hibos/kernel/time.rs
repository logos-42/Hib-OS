//! 时间 / 时钟 / 定时任务 —— AI 原生的「时间感」。
//!
//! 真实系统时钟 + 单调时钟 + cron 式定时任务，统一为通道。

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct CronJob {
    pub id: u64,
    pub interval_secs: u64,
    pub next_epoch: u64,
    pub cmd: String,
}

pub struct TimeKeeper {
    pub next_id: u64,
    pub jobs: Vec<CronJob>,
}

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl TimeKeeper {
    fn new() -> Self {
        Self { next_id: 1, jobs: Vec::new() }
    }

    pub fn clock(&self) -> String {
        let epoch = now_epoch();
        let uptime = crate::monitor::get_time_ns() / 1_000_000_000;
        format!("epoch={} ({}s) uptime={}s tz=UTC", epoch, epoch, uptime)
    }

    pub fn cron_list(&self) -> String {
        if self.jobs.is_empty() {
            return "（无定时任务）".to_string();
        }
        self.jobs
            .iter()
            .map(|j| format!("job {} every={}s next={} cmd={}", j.id, j.interval_secs, j.next_epoch, j.cmd))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn cron_add(&mut self, interval_secs: u64, cmd: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.jobs.push(CronJob {
            id,
            interval_secs,
            next_epoch: now_epoch() + interval_secs,
            cmd: cmd.to_string(),
        });
        format!("已添加定时任务 {} 每 {}s 执行: {}", id, interval_secs, cmd)
    }

    pub fn cron_del(&mut self, id: u64) -> Result<String, String> {
        let pos = self.jobs.iter().position(|j| j.id == id).ok_or_else(|| format!("任务 {} 不存在", id))?;
        self.jobs.remove(pos);
        Ok(format!("已删除定时任务 {}", id))
    }

    pub fn cron_run_due(&mut self) -> String {
        let now = now_epoch();
        let mut ran = Vec::new();
        for j in self.jobs.iter_mut() {
            if j.next_epoch <= now {
                let _ = crate::proc::proctable().lock().unwrap().spawn(&j.cmd);
                j.next_epoch = now + j.interval_secs;
                ran.push(j.id);
            }
        }
        if ran.is_empty() {
            "（当前没有到期的定时任务）".to_string()
        } else {
            format!("已触发 {} 个定时任务: {:?}", ran.len(), ran)
        }
    }
}

static TIME: OnceLock<Mutex<TimeKeeper>> = OnceLock::new();

pub fn timekeeper() -> &'static Mutex<TimeKeeper> {
    TIME.get_or_init(|| Mutex::new(TimeKeeper::new()))
}
