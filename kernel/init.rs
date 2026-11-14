//! 服务 / init 管理 —— AI 原生的「自启动意图」。
//!
//! 把系统服务建模为可启停 / 启用的 `init/services` 通道。

use std::sync::{Mutex, OnceLock};

pub struct Service {
    pub name: String,
    pub state: String,
    pub enabled: bool,
}

pub struct Init {
    pub services: Vec<Service>,
}

impl Init {
    fn new() -> Self {
        Self {
            services: ["gpu", "sched", "syscall", "verify", "vfs"]
                .iter()
                .map(|n| Service {
                    name: n.to_string(),
                    state: "running".to_string(),
                    enabled: true,
                })
                .collect(),
        }
    }

    pub fn list(&self) -> String {
        self.services
            .iter()
            .map(|s| {
                format!(
                    "{:<10} state={:<8} enabled={}",
                    s.name,
                    s.state,
                    if s.enabled { "yes" } else { "no" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find(&mut self, name: &str) -> Result<&mut Service, String> {
        self.services
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("服务 {} 不存在", name))
    }

    pub fn start(&mut self, name: &str) -> Result<String, String> {
        self.find(name)?.state = "running".to_string();
        Ok(format!("服务 {} 已启动", name))
    }

    pub fn stop(&mut self, name: &str) -> Result<String, String> {
        self.find(name)?.state = "stopped".to_string();
        Ok(format!("服务 {} 已停止", name))
    }

    pub fn enable(&mut self, name: &str) -> Result<String, String> {
        self.find(name)?.enabled = true;
        Ok(format!("服务 {} 已启用", name))
    }

    pub fn disable(&mut self, name: &str) -> Result<String, String> {
        self.find(name)?.enabled = false;
        Ok(format!("服务 {} 已禁用", name))
    }
}

static INIT: OnceLock<Mutex<Init>> = OnceLock::new();

pub fn init_mgr() -> &'static Mutex<Init> {
    INIT.get_or_init(|| Mutex::new(Init::new()))
}
