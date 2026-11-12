//! GPU 硬件保护域 —— `gpu://domains` / `gpu://faults`。
//!
//! Linux 用 IOMMU + MMU 把设备与进程隔离开；Hib 把"保护"暴露成可感知的通道：
//! 每个 token / 子意识一个保护域，绑定独立的 IOMMU 域与 VRAM 配额，
//! 越界访问不会读写到别人的显存，而是**产生 fault、挂起该主体、留下可审计记录**。
//!
//! * `sense gpu://domains` / `sense gpu://faults`
//! * `act gpu://domains create <id> <配额MiB>`
//! * `act gpu://domains access <id> <MiB>`（模拟一次显存访问）
//! * `act gpu://domains resume <id>` / `quota <id> <MiB>` / `list`

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::ai_token::TokenId;
use crate::gpu::iommu;

/// 一次保护故障。
#[derive(Clone, Debug)]
pub struct Fault {
    pub seq: u64,
    pub id: String,
    pub kind: String,
    pub detail: String,
}

/// 一个保护域。
pub struct ProtectionDomain {
    pub id: String,
    pub iommu_domain: u64,
    pub vram_quota_mib: u64,
    pub used_mib: u64,
    pub faults: u64,
    pub suspended: bool,
}

pub struct Protection {
    pub domains: BTreeMap<String, ProtectionDomain>,
    pub fault_log: Vec<Fault>,
    seq: u64,
}

impl Protection {
    fn new() -> Self {
        Self {
            domains: BTreeMap::new(),
            fault_log: Vec::new(),
            seq: 0,
        }
    }

    /// 创建保护域，同时向 IOMMU 申请一个隔离域。
    pub fn create(&mut self, id: &str, quota_mib: u64) -> Result<String, String> {
        if id.is_empty() {
            return Err("保护域 id 不能为空".to_string());
        }
        if self.domains.contains_key(id) {
            return Err(format!("保护域 {} 已存在", id));
        }
        // 用稳定的序号当 token id，向真实 IOMMU 模拟器申请域。
        let token = TokenId(self.seq + 1);
        let dom = iommu().lock().unwrap().allocate_domain(token);
        self.seq += 1;
        self.domains.insert(
            id.to_string(),
            ProtectionDomain {
                id: id.to_string(),
                iommu_domain: dom.0,
                vram_quota_mib: quota_mib,
                used_mib: 0,
                faults: 0,
                suspended: false,
            },
        );
        Ok(format!(
            "保护域 {} 已建立: iommu-domain={} 配额={}MiB",
            id, dom.0, quota_mib
        ))
    }

    fn fault(&mut self, id: &str, kind: &str, detail: String) {
        self.seq += 1;
        self.fault_log.push(Fault {
            seq: self.seq,
            id: id.to_string(),
            kind: kind.to_string(),
            detail,
        });
        if let Some(d) = self.domains.get_mut(id) {
            d.faults += 1;
            d.suspended = true;
        }
        if self.fault_log.len() > 64 {
            self.fault_log.remove(0);
        }
    }

    /// 模拟一次显存访问：校验配额 + 域状态，越界则挂起。
    pub fn access(&mut self, id: &str, mib: u64) -> Result<String, String> {
        let (over, suspended, used, quota, dom) = {
            let d = self
                .domains
                .get(id)
                .ok_or_else(|| format!("保护域 {} 不存在", id))?;
            (
                d.used_mib.saturating_add(mib) > d.vram_quota_mib,
                d.suspended,
                d.used_mib,
                d.vram_quota_mib,
                d.iommu_domain,
            )
        };
        if suspended {
            return Err(format!("保护域 {} 已因故障挂起，先 resume", id));
        }
        if over {
            self.fault(
                id,
                "vram_quota",
                format!("申请 {}MiB 会超出配额 {}MiB (已用 {}MiB)", mib, quota, used),
            );
            return Err(format!(
                "GPU 保护故障: {} 超出 VRAM 配额 {}MiB，主体已挂起",
                id, quota
            ));
        }
        let d = self.domains.get_mut(id).unwrap();
        d.used_mib += mib;
        Ok(format!(
            "{} 经 iommu-domain={} 访问 {}MiB (已用 {}/{}MiB)",
            id, dom, mib, d.used_mib, quota
        ))
    }

    pub fn resume(&mut self, id: &str) -> Result<String, String> {
        let d = self
            .domains
            .get_mut(id)
            .ok_or_else(|| format!("保护域 {} 不存在", id))?;
        if !d.suspended {
            return Err(format!("保护域 {} 未挂起", id));
        }
        d.suspended = false;
        Ok(format!("保护域 {} 已恢复", id))
    }

    pub fn set_quota(&mut self, id: &str, mib: u64) -> Result<String, String> {
        let d = self
            .domains
            .get_mut(id)
            .ok_or_else(|| format!("保护域 {} 不存在", id))?;
        d.vram_quota_mib = mib;
        Ok(format!("保护域 {} 配额 -> {}MiB", id, mib))
    }

    pub fn status(&self) -> String {
        if self.domains.is_empty() {
            return "（无保护域）".to_string();
        }
        self.domains
            .values()
            .map(|d| {
                format!(
                    "{:<12} iommu={} used={}/{}MiB faults={} {}",
                    d.id,
                    d.iommu_domain,
                    d.used_mib,
                    d.vram_quota_mib,
                    d.faults,
                    if d.suspended { "SUSPENDED" } else { "active" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn faults(&self) -> String {
        if self.fault_log.is_empty() {
            return "（无 GPU 保护故障）".to_string();
        }
        self.fault_log
            .iter()
            .rev()
            .take(10)
            .map(|f| format!("#{} [{}] {} : {}", f.seq, f.id, f.kind, f.detail))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

static PROTECTION: OnceLock<Mutex<Protection>> = OnceLock::new();

pub fn protection() -> &'static Mutex<Protection> {
    PROTECTION.get_or_init(|| Mutex::new(Protection::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_violation_creates_fault_and_suspends() {
        let mut p = Protection::new();
        p.create("agent-a", 100).unwrap();
        assert!(p.access("agent-a", 80).is_ok());
        let err = p.access("agent-a", 30).unwrap_err();
        assert!(err.contains("保护故障"), "{}", err);
        assert!(p.domains["agent-a"].suspended);
        assert_eq!(p.domains["agent-a"].faults, 1);
        assert!(p.faults().contains("vram_quota"));
        // 挂起后连合法访问也拒绝
        assert!(p.access("agent-a", 1).is_err());
        assert!(p.resume("agent-a").is_ok());
        assert!(p.access("agent-a", 10).is_ok());
    }

    #[test]
    fn create_rejects_duplicates() {
        let mut p = Protection::new();
        assert!(p.create("x", 10).is_ok());
        assert!(p.create("x", 10).is_err());
        assert!(p.create("", 10).is_err());
        assert!(p.access("missing", 1).is_err());
    }

    #[test]
    fn status_and_quota_change() {
        let mut p = Protection::new();
        p.create("x", 10).unwrap();
        assert!(p.status().contains("iommu="));
        p.set_quota("x", 50).unwrap();
        assert!(p.access("x", 40).is_ok());
    }
}
