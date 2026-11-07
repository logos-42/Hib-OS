//! 电源 / 会话状态 —— AI 原生的「系统姿态」。

use std::sync::{Mutex, OnceLock};

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
        Ok(format!("系统姿态: {}", ns))
    }

    pub fn wake(&mut self) -> String {
        self.state = "running".to_string();
        self.history.push("wake -> running".to_string());
        "已唤醒 -> running".to_string()
    }
}

static POWER: OnceLock<Mutex<Power>> = OnceLock::new();

pub fn power() -> &'static Mutex<Power> {
    POWER.get_or_init(|| Mutex::new(Power::new()))
}
