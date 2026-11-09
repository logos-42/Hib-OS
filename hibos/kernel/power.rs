//! 电源 / 会话状态 —— AI 原生的「系统姿态」。

use std::sync::{Mutex, OnceLock};

use crate::sense::wake_source;

pub struct Power {
    pub state: String,
    pub history: Vec<String>,
}

impl Power {
    fn new() -> Self {
        Self {
            state: "running".to_string(),
            history: vec!["boot -> running".to_string()],
        }
    }

    pub fn status(&self) -> String {
        format!("state={} history={:?}", self.state, self.history)
    }

    pub fn set(&mut self, s: &str) -> Result<String, String> {
        let ns = s.to_lowercase();
        let valid = ["running", "suspend", "reboot", "shutdown"];
        if !valid.contains(&ns.as_str()) {
            return Err("状态只能是 running|suspend|reboot|shutdown".to_string());
        }
        self.state = ns.clone();
        self.history.push(format!("-> {}", ns));
        match ns.as_str() {
            // 真正的睡眠：把所有执行体置为 Suspended，等待表原样保留。
            // 唤醒后它们回到原来的等待键上，不丢事件。
            "suspend" => {
                let n = crate::exec::suspend_all();
                self.history.push(format!("exec suspended={}", n));
                Ok(format!("系统姿态: suspend（已挂起 {} 个执行体，等待表保留）", n))
            }
            // 从睡眠恢复。
            "running" => {
                let n = crate::exec::resume_all();
                self.history.push(format!("exec resumed={}", n));
                Ok(format!("系统姿态: running（已恢复 {} 个执行体）", n))
            }
            _ => Ok(format!("系统姿态: {}", ns)),
        }
    }

    pub fn wake(&mut self) -> String {
        self.state = "running".to_string();
        let n = crate::exec::resume_all();
        // 如果设置了唤醒源通道，向其等待者发 signal —— 实现「特定事件唤醒系统」。
        if let Some(ch) = wake_source() {
            let woke = crate::exec::signal(&ch);
            self.history
                .push(format!("wake -> running (resumed={}, signaled wake-source={}={})", n, ch, woke));
        } else {
            self.history.push(format!("wake -> running (resumed={})", n));
        }
        format!("已唤醒 -> running（恢复 {} 个执行体）", n)
    }
}

static POWER: OnceLock<Mutex<Power>> = OnceLock::new();

pub fn power() -> &'static Mutex<Power> {
    POWER.get_or_init(|| Mutex::new(Power::new()))
}
