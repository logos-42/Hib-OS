//! 文件系统元数据通道 —— AI 原生的「权限 / 挂载 / 链接」。
//!
//! 不复制 Linux 的 inode，而是把 mode / uid / gid / 挂载 / 链接
//! 建模成可感知 / 可设置的通道 `fs://meta`。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub struct FsMeta {
    /// path -> (mode, uid, gid)
    pub modes: BTreeMap<String, (u16, u32, u32)>,
    /// (src, dst)
    pub mounts: Vec<(String, String)>,
    /// (target, linkpath)
    pub links: Vec<(String, String)>,
}

impl FsMeta {
    fn new() -> Self {
        Self { modes: BTreeMap::new(), mounts: Vec::new(), links: Vec::new() }
    }

    pub fn chmod(&mut self, mode: u16, path: &str) -> String {
        let e = self.modes.entry(path.to_string()).or_insert((0o644, 0, 0));
        e.0 = mode;
        format!("chmod {} -> {:o}", path, mode)
    }

    pub fn chown(&mut self, uid: u32, gid: u32, path: &str) -> String {
        let e = self.modes.entry(path.to_string()).or_insert((0o644, 0, 0));
        e.1 = uid;
        e.2 = gid;
        format!("chown {} -> uid={} gid={}", path, uid, gid)
    }

    pub fn ln(&mut self, target: &str, link: &str) -> String {
        self.links.push((target.to_string(), link.to_string()));
        format!("已创建链接 {} -> {}", link, target)
    }

    pub fn mount(&mut self, src: &str, dst: &str) -> String {
        self.mounts.push((src.to_string(), dst.to_string()));
        format!("已挂载 {} 到 {}", src, dst)
    }

    pub fn list(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("权限项 {} 个:\n", self.modes.len()));
        for (p, (m, uid, gid)) in &self.modes {
            out.push_str(&format!("  {:o} uid={} gid={} {}\n", m, uid, gid, p));
        }
        out.push_str(&format!("挂载 {} 个:\n", self.mounts.len()));
        for (s, d) in &self.mounts {
            out.push_str(&format!("  {} -> {}\n", s, d));
        }
        out.push_str(&format!("链接 {} 个:\n", self.links.len()));
        for (t, l) in &self.links {
            out.push_str(&format!("  {} -> {}\n", l, t));
        }
        out.trim_end().to_string()
    }
}

static FSMETA: OnceLock<Mutex<FsMeta>> = OnceLock::new();

/// 全局文件系统元数据。
pub fn fsmeta() -> &'static Mutex<FsMeta> {
    FSMETA.get_or_init(|| Mutex::new(FsMeta::new()))
}
