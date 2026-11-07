//! 进程环境变量 —— AI 原生的「上下文注入」。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub struct EnvStore {
    pub vars: BTreeMap<String, String>,
}

impl EnvStore {
    fn new() -> Self {
        let mut vars = BTreeMap::new();
        for (k, v) in [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/root"),
            ("LANG", "zh_CN.UTF-8"),
            ("SHELL", "/bin/hibsh"),
            ("TERM", "xterm-256color"),
            ("USER", "ai"),
            ("PWD", "/"),
        ] {
            vars.insert(k.to_string(), v.to_string());
        }
        Self { vars }
    }

    pub fn list(&self) -> String {
        self.vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn get(&self, k: &str) -> String {
        match self.vars.get(k) {
            Some(v) => format!("{}={}", k, v),
            None => format!("（未设置 {}）", k),
        }
    }

    pub fn set(&mut self, k: &str, v: &str) -> String {
        self.vars.insert(k.to_string(), v.to_string());
        format!("已设置 {}={}", k, v)
    }

    pub fn unset(&mut self, k: &str) -> String {
        if self.vars.remove(k).is_some() {
            format!("已取消 {}", k)
        } else {
            format!("（未设置 {}）", k)
        }
    }
}

static ENV: OnceLock<Mutex<EnvStore>> = OnceLock::new();

pub fn envstore() -> &'static Mutex<EnvStore> {
    ENV.get_or_init(|| Mutex::new(EnvStore::new()))
}
