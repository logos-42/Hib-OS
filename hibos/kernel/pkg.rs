//! 软件包管理 —— AI 原生的「能力装配」。
//!
//! 包 = 一组可安装的能力单元，统一为 `pkg://store` 通道。

use std::sync::{Mutex, OnceLock};

pub struct Package {
    pub name: String,
    pub version: String,
    pub size_kb: u64,
    pub installed: bool,
}

pub struct PkgStore {
    pub packages: Vec<Package>,
}

impl PkgStore {
    fn new() -> Self {
        let seed = [
            ("libgpu", "1.2.0", 4096u64),
            ("ai-runtime", "0.9.1", 15360),
            ("wl-compositor", "2.0.3", 8192),
            ("netd", "1.4.0", 2048),
            ("hibos-tools", "0.1.0", 512),
        ];
        Self {
            packages: seed
                .iter()
                .map(|(n, v, s)| Package {
                    name: n.to_string(),
                    version: v.to_string(),
                    size_kb: *s,
                    installed: false,
                })
                .collect(),
        }
    }

    pub fn list(&self) -> String {
        self.packages
            .iter()
            .map(|p| {
                format!(
                    "{:<14} {:<8} {}KB {}",
                    p.name,
                    p.version,
                    p.size_kb,
                    if p.installed { "installed" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn search(&self, kw: &str) -> String {
        let hits: Vec<String> = self
            .packages
            .iter()
            .filter(|p| p.name.contains(kw))
            .map(|p| format!("{} {}", p.name, p.version))
            .collect();
        if hits.is_empty() {
            format!("未找到匹配 {} 的包", kw)
        } else {
            hits.join("\n")
        }
    }

    pub fn install(&mut self, name: &str) -> Result<String, String> {
        match self.packages.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                p.installed = true;
                Ok(format!("已安装 {} {}", p.name, p.version))
            }
            None => Err(format!("包 {} 不存在", name)),
        }
    }

    pub fn remove(&mut self, name: &str) -> Result<String, String> {
        match self.packages.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                p.installed = false;
                Ok(format!("已卸载 {}", p.name))
            }
            None => Err(format!("包 {} 不存在", name)),
        }
    }
}

static PKG: OnceLock<Mutex<PkgStore>> = OnceLock::new();

pub fn pkgstore() -> &'static Mutex<PkgStore> {
    PKG.get_or_init(|| Mutex::new(PkgStore::new()))
}
