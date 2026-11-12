//! 多调度域 —— `domain://table`。
//!
//! Linux 按 CPU 核切 runqueue；Hib 按**通道族**切认知域：
//! cpu / gpu / net / io / ui 各自一套配额与亲和性，一个域拥塞不拖垮另一个。
//! 这里把此前"定义了却零使用"的 `CpuAffinity` 真正落地：
//! 每个域绑定一个核，创建域时自动分配下一个核。
//!
//! * `sense domain://table`
//! * `act domain://table create <name>`
//! * `act domain://table assign|move <cell> <domain>`
//! * `act domain://table tick <domain>` / `balance` / `quota <domain> <n>`

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::scheduler::CpuAffinity;

/// 一个认知调度域。
pub struct Domain {
    pub name: String,
    pub affinity: CpuAffinity,
    /// 该域最多容纳多少个意识主体。
    pub quota: u32,
    pub cells: Vec<String>,
    pub ticks: u64,
    pub migrations: u64,
}

impl Domain {
    fn core(&self) -> String {
        match &self.affinity {
            CpuAffinity::Core(c) => format!("core{}", c),
            CpuAffinity::Group(g) => format!("group{:?}", g),
            CpuAffinity::None => "any".to_string(),
        }
    }
}

pub struct DomainTable {
    pub domains: BTreeMap<String, Domain>,
    next_core: u8,
}

impl DomainTable {
    fn new() -> Self {
        let mut t = Self {
            domains: BTreeMap::new(),
            next_core: 0,
        };
        for name in ["cpu", "gpu", "net", "io", "ui"] {
            t.add(name);
        }
        t
    }

    fn add(&mut self, name: &str) {
        let core = self.next_core;
        self.next_core += 1;
        self.domains.insert(
            name.to_string(),
            Domain {
                name: name.to_string(),
                affinity: CpuAffinity::Core(core),
                quota: 4,
                cells: Vec::new(),
                ticks: 0,
                migrations: 0,
            },
        );
    }

    pub fn status(&self) -> String {
        let mut out = String::from("domains (按通道族切分, 每域绑定一个核):");
        for d in self.domains.values() {
            out.push_str(&format!(
                "\n  {:<6} {} quota={} cells={}/{} ticks={} migrations={} [{}]",
                d.name,
                d.core(),
                d.quota,
                d.cells.len(),
                d.quota,
                d.ticks,
                d.migrations,
                d.cells.join(",")
            ));
        }
        out
    }

    pub fn create(&mut self, name: &str) -> Result<String, String> {
        if name.is_empty() {
            return Err("域名不能为空".to_string());
        }
        if self.domains.contains_key(name) {
            return Err(format!("域 {} 已存在", name));
        }
        self.add(name);
        Ok(format!(
            "已创建域 {} -> {}",
            name,
            self.domains.get(name).unwrap().core()
        ))
    }

    /// 把主体放到某个域；若原来在别的域，先摘除。
    pub fn assign(&mut self, cell: &str, target: &str) -> Result<String, String> {
        if !self.domains.contains_key(target) {
            return Err(format!("域 {} 不存在", target));
        }
        // 先摘除（不论原先在哪）
        for d in self.domains.values_mut() {
            d.cells.retain(|c| c != cell);
        }
        let d = self.domains.get_mut(target).unwrap();
        if d.cells.len() >= d.quota as usize {
            return Err(format!("域 {} 配额已满 ({})", target, d.quota));
        }
        d.cells.push(cell.to_string());
        Ok(format!("{} 已分配 -> {} ({})", cell, target, d.core()))
    }

    /// 迁移（记录迁移次数，供调度观测）。
    pub fn migrate(&mut self, cell: &str, target: &str) -> Result<String, String> {
        let from = self
            .domains
            .values()
            .find(|d| d.cells.iter().any(|c| c == cell))
            .map(|d| d.name.clone());
        let r = self.assign(cell, target)?;
        if let Some(f) = from {
            if f != target {
                self.domains.get_mut(target).unwrap().migrations += 1;
                return Ok(format!("{} -> {} : {}", f, target, r));
            }
        }
        Ok(r)
    }

    pub fn tick(&mut self, target: &str) -> Result<String, String> {
        let d = self
            .domains
            .get_mut(target)
            .ok_or_else(|| format!("域 {} 不存在", target))?;
        d.ticks += 1;
        Ok(format!("{} tick={} ({} 上的 {} 个主体)", target, d.ticks, d.core(), d.cells.len()))
    }

    pub fn set_quota(&mut self, target: &str, n: u32) -> Result<String, String> {
        if n == 0 {
            return Err("配额必须 > 0".to_string());
        }
        let d = self
            .domains
            .get_mut(target)
            .ok_or_else(|| format!("域 {} 不存在", target))?;
        d.quota = n;
        Ok(format!("域 {} 配额 = {}", target, n))
    }

    /// 负载均衡：把最忙域里的一个主体迁到最闲且有余额的域。
    pub fn balance(&mut self) -> Result<String, String> {
        let (busy, cell) = self
            .domains
            .values()
            .max_by_key(|d| d.cells.len())
            .and_then(|d| d.cells.first().map(|c| (d.name.clone(), c.clone())))
            .ok_or_else(|| "没有可迁移的主体".to_string())?;
        let idle = self
            .domains
            .values()
            .filter(|d| d.name != busy && d.cells.len() < d.quota as usize)
            .min_by_key(|d| d.cells.len())
            .map(|d| d.name.clone())
            .ok_or_else(|| "无空闲域可接收".to_string())?;
        if self.domains[&busy].cells.len() <= self.domains[&idle].cells.len() {
            return Ok(format!("已均衡 ({}={} , {}={})", busy, self.domains[&busy].cells.len(), idle, self.domains[&idle].cells.len()));
        }
        self.migrate(&cell, &idle)
    }
}

static TABLE: OnceLock<Mutex<DomainTable>> = OnceLock::new();

pub fn table() -> &'static Mutex<DomainTable> {
    TABLE.get_or_init(|| Mutex::new(DomainTable::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_one_core_each() {
        let t = DomainTable::new();
        assert_eq!(t.domains.len(), 5);
        let s = t.status();
        assert!(s.contains("core0"), "{}", s);
        assert!(s.contains("core4"), "{}", s);
    }

    #[test]
    fn create_assign_and_quota() {
        let mut t = DomainTable::new();
        assert!(t.create("ai").is_ok());
        assert!(t.create("ai").is_err());
        assert!(t.domains["ai"].affinity == CpuAffinity::Core(5));
        t.assign("c1", "cpu").unwrap();
        t.assign("c1", "gpu").unwrap();
        assert!(!t.domains["cpu"].cells.contains(&"c1".to_string()), "旧域应摘除");
        assert_eq!(t.domains["gpu"].cells.len(), 1);
        t.set_quota("gpu", 1).unwrap();
        assert!(t.assign("c2", "gpu").is_err());
    }

    #[test]
    fn migrate_tick_and_balance() {
        let mut t = DomainTable::new();
        for i in 0..4 {
            t.assign(&format!("c{}", i), "cpu").unwrap();
        }
        assert!(t.tick("cpu").unwrap().contains("tick=1"));
        let r = t.balance().unwrap();
        assert!(r.contains("->"), "{}", r);
        assert_eq!(t.domains["cpu"].cells.len(), 3);
        assert!(t.migrate("c0", "net").unwrap().contains("net"));
    }

    #[test]
    fn reject_unknown_domain() {
        let mut t = DomainTable::new();
        assert!(t.assign("x", "nope").is_err());
        assert!(t.set_quota("nope", 2).is_err());
        assert!(t.set_quota("cpu", 0).is_err());
    }
}
