//! 控制组 (cgroups) —— AI 原生的「资源分组」。
//!
//! 把进程按组约束 CPU / 内存配额，统一为 `cgroup/groups` 通道。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub struct Group {
    pub name: String,
    pub cpu_quota: u32,
    pub mem_limit_mb: u64,
    pub pids: Vec<u32>,
}

pub struct Cgroups {
    pub groups: BTreeMap<String, Group>,
}

impl Cgroups {
    fn new() -> Self {
        let mut groups = BTreeMap::new();
        groups.insert(
            "root".to_string(),
            Group { name: "root".into(), cpu_quota: 100, mem_limit_mb: 0, pids: vec![1] },
        );
        Self { groups }
    }

    pub fn list(&self) -> String {
        self.groups
            .values()
            .map(|g| {
                format!(
                    "{:<12} cpu={}% mem={}MiB pids={:?}",
                    g.name, g.cpu_quota, g.mem_limit_mb, g.pids
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn create(&mut self, name: &str) -> Result<String, String> {
        if self.groups.contains_key(name) {
            return Err(format!("cgroup {} 已存在", name));
        }
        self.groups.insert(
            name.to_string(),
            Group { name: name.to_string(), cpu_quota: 100, mem_limit_mb: 0, pids: Vec::new() },
        );
        Ok(format!("已创建 cgroup {}", name))
    }

    pub fn delete(&mut self, name: &str) -> Result<String, String> {
        if name == "root" {
            return Err("不能删除 root cgroup".to_string());
        }
        match self.groups.remove(name) {
            Some(_) => Ok(format!("已删除 cgroup {}", name)),
            None => Err(format!("cgroup {} 不存在", name)),
        }
    }

    pub fn set_limit(&mut self, name: &str, cpu: u32, mem: u64) -> Result<String, String> {
        match self.groups.get_mut(name) {
            Some(g) => {
                g.cpu_quota = cpu;
                g.mem_limit_mb = mem;
                Ok(format!("cgroup {} cpu={}% mem={}MiB", name, cpu, mem))
            }
            None => Err(format!("cgroup {} 不存在", name)),
        }
    }

    pub fn attach(&mut self, name: &str, pid: u32) -> Result<String, String> {
        match self.groups.get_mut(name) {
            Some(g) => {
                if !g.pids.contains(&pid) {
                    g.pids.push(pid);
                }
                Ok(format!("pid {} 已加入 cgroup {}", pid, name))
            }
            None => Err(format!("cgroup {} 不存在", name)),
        }
    }
}

static CGROUPS: OnceLock<Mutex<Cgroups>> = OnceLock::new();

pub fn cgroups() -> &'static Mutex<Cgroups> {
    CGROUPS.get_or_init(|| Mutex::new(Cgroups::new()))
}
