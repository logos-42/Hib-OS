//! 主机内存子系统 —— AI 原生的「工作记忆」。
//!
//! 把主机 RAM 与 mmap 区域建模为可感知 / 可分配 / 可映射的通道：
//! `mem://ram` 负责 RAM 配额，`mem://mmap` 负责虚拟映射区域。

use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct Region {
    pub addr: u64,
    pub len: u64,
    pub perms: String,
    pub uid: u32,
}

pub struct MemManager {
    total: u64,
    used: u64,
    regions: Vec<Region>,
    next_addr: u64,
}

impl MemManager {
    fn new() -> Self {
        Self {
            total: 16 * 1024 * 1024 * 1024,
            used: 0,
            regions: Vec::new(),
            next_addr: 0x6000_0000,
        }
    }

    pub fn stats(&self) -> String {
        format!(
            "host_ram total={}MiB used={}MiB free={}MiB regions={}",
            self.total / (1024 * 1024),
            self.used / (1024 * 1024),
            (self.total - self.used) / (1024 * 1024),
            self.regions.len()
        )
    }

    pub fn alloc(&mut self, bytes: u64) -> Result<String, String> {
        if self.used.saturating_add(bytes) > self.total {
            return Err("主机 RAM 不足".to_string());
        }
        self.used += bytes;
        Ok(format!(
            "已分配 {}MiB, 已用 {}MiB/{}MiB",
            bytes / (1024 * 1024),
            self.used / (1024 * 1024),
            self.total / (1024 * 1024)
        ))
    }

    pub fn free(&mut self, bytes: u64) -> Result<String, String> {
        if bytes > self.used {
            return Err("释放量超过已分配".to_string());
        }
        self.used -= bytes;
        Ok(format!(
            "已释放 {}MiB, 已用 {}MiB",
            bytes / (1024 * 1024),
            self.used / (1024 * 1024)
        ))
    }

    pub fn mmap(&mut self, len: u64, perms: &str, uid: u32) -> Result<String, String> {
        let addr = self.next_addr;
        self.next_addr += (len + 0xfff) & !0xfff;
        self.regions.push(Region { addr, len, perms: perms.to_string(), uid });
        Ok(format!("mmap 0x{:x} len={}B perms={} uid={}", addr, len, perms, uid))
    }

    pub fn munmap(&mut self, addr: u64) -> Result<String, String> {
        match self.regions.iter().position(|r| r.addr == addr) {
            Some(i) => {
                let r = self.regions.remove(i);
                Ok(format!("munmap 0x{:x} ({}B)", r.addr, r.len))
            }
            None => Err(format!("无此映射 0x{:x}", addr)),
        }
    }

    pub fn regions(&self) -> String {
        if self.regions.is_empty() {
            return "（无 mmap 区域）".to_string();
        }
        self.regions
            .iter()
            .map(|r| format!("0x{:x} len={}B perms={} uid={}", r.addr, r.len, r.perms, r.uid))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

static MEM: OnceLock<Mutex<MemManager>> = OnceLock::new();

/// 全局主机内存管理器。
pub fn mem() -> &'static Mutex<MemManager> {
    MEM.get_or_init(|| Mutex::new(MemManager::new()))
}
