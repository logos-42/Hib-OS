//! 内核参数 / 系统设置 (sysctl) —— AI 原生的「系统旋钮」。
//!
//! 把 `kernel.*` 可调参数与主机身份配置建模为可感知 / 可设置的通道。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub struct Sysctl {
    pub kernel: BTreeMap<String, String>,
    pub identity: BTreeMap<String, String>,
}

impl Sysctl {
    fn new() -> Self {
        let mut kernel = BTreeMap::new();
        for (k, v) in [
            ("kernel.hostname", "hibos"),
            ("kernel.domainname", "localdomain"),
            ("kernel.panic", "0"),
            ("kernel.ostype", "HibOS"),
            ("kernel.osrelease", concat!(crate::version::hibos_version!(), "-hibos")),
            ("vm.overcommit_memory", "0"),
            ("vm.swappiness", "60"),
            ("net.ipv4.ip_forward", "0"),
            ("net.ipv4.tcp_syncookies", "1"),
            ("fs.protected_hardlinks", "1"),
            ("log.level", "info"),
        ] {
            kernel.insert(k.to_string(), v.to_string());
        }
        let mut identity = BTreeMap::new();
        for (k, v) in [
            ("hostname", "hibos"),
            ("timezone", "UTC"),
            ("locale", "zh_CN.UTF-8"),
            ("shell", "/bin/hibsh"),
            (
                "kernel.version",
                concat!("Hib OS ", crate::version::hibos_version!(), " (AI-native)"),
            ),
        ] {
            identity.insert(k.to_string(), v.to_string());
        }
        Self { kernel, identity }
    }

    pub fn kernel_list(&self) -> String {
        self.kernel
            .iter()
            .map(|(k, v)| format!("{} = {}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn identity_list(&self) -> String {
        self.identity
            .iter()
            .map(|(k, v)| format!("{} = {}", k, v))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set_kernel(&mut self, key: &str, val: &str) -> Result<String, String> {
        if !self.kernel.contains_key(key) {
            return Err(format!("未知内核参数: {}", key));
        }
        self.kernel.insert(key.to_string(), val.to_string());
        Ok(format!("{} = {}", key, val))
    }

    pub fn set_identity(&mut self, key: &str, val: &str) -> Result<String, String> {
        if !self.identity.contains_key(key) {
            return Err(format!("未知系统配置: {}", key));
        }
        self.identity.insert(key.to_string(), val.to_string());
        Ok(format!("{} = {}", key, val))
    }
}

static SYSCTL: OnceLock<Mutex<Sysctl>> = OnceLock::new();

pub fn sysctl() -> &'static Mutex<Sysctl> {
    SYSCTL.get_or_init(|| Mutex::new(Sysctl::new()))
}
