//! 安全能力 (capabilities) —— AI 原生的「权限意图」（no_std 内核版本）。
//!
//! 用能力位而非 uid 硬编码来约束行动，可授予 / 撤销 / 校验。

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::ksync::{SpinLock, OnceLock};

pub const ALL_CAPS: &[&str] = &[
    "CAP_CHOWN",
    "CAP_DAC_OVERRIDE",
    "CAP_KILL",
    "CAP_NET_BIND",
    "CAP_NET_RAW",
    "CAP_NET_ADMIN",
    "CAP_SYS_ADMIN",
    "CAP_SYS_PTRACE",
    "CAP_SYS_TIME",
    "CAP_SYS_MODULE",
    "CAP_SYS_RAWIO",
];

pub struct Caps {
    pub set: BTreeSet<String>,
}

impl Caps {
    fn new() -> Self {
        // 默认授予全部能力；AI 可主动 revoke 来演示最小权限。
        Self {
            set: ALL_CAPS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn has(&self, cap: &str) -> bool {
        self.set.contains(cap)
    }

    pub fn list(&self) -> String {
        let mut out = String::new();
        for c in ALL_CAPS {
            out.push_str(&format!(
                "{} {}\n",
                if self.set.contains(*c) { "[x]" } else { "[ ]" },
                c
            ));
        }
        out.trim_end().to_string()
    }

    pub fn grant(&mut self, cap: &str) -> Result<String, String> {
        if !ALL_CAPS.contains(&cap) {
            return Err(format!("未知能力: {}", cap));
        }
        self.set.insert(cap.to_string());
        Ok(format!("已授予 {}", cap))
    }

    pub fn revoke(&mut self, cap: &str) -> Result<String, String> {
        self.set.remove(cap);
        Ok(format!("已撤销 {}", cap))
    }

    pub fn check(&self, cap: &str) -> String {
        format!("{} : {}", cap, if self.set.contains(cap) { "允许" } else { "拒绝" })
    }
}

static CAPS: OnceLock<SpinLock<Caps>> = OnceLock::new();

pub fn caps() -> &'static SpinLock<Caps> {
    CAPS.get_or_init(|| SpinLock::new(Caps::new()))
}
