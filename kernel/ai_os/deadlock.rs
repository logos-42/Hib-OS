//! 死锁检测与恢复 —— 智能体之间的资源等待图。
//!
//! 对应《现代操作系统》第 6 章「死锁」：
//! * 资源类型：模型实例、工具租约、上下文窗口；
//! * 等待图 (wait-for graph)：边 `A -> B` 表示 A 等待 B 释放资源；
//! * 环路检测 = 死锁；恢复策略：终止牺牲者或回滚；
//! * 银行家算法式的「安全状态」检查（简化）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

/// 资源标识。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Resource {
    pub kind: String,
    pub id: String,
}

impl Resource {
    pub fn new(kind: &str, id: &str) -> Self {
        Self {
            kind: kind.to_string(),
            id: id.to_string(),
        }
    }
}

/// 等待图。
pub struct WaitForGraph {
    /// agent -> 它等待的 agent 集合。
    edges: BTreeMap<u64, BTreeSet<u64>>,
    /// 资源 -> 当前持有者。
    owners: BTreeMap<Resource, u64>,
    /// agent -> 它等待的资源。
    waiting: BTreeMap<u64, BTreeSet<Resource>>,
    pub deadlocks_detected: u64,
    pub recoveries: u64,
}

impl WaitForGraph {
    pub fn new() -> Self {
        Self {
            edges: BTreeMap::new(),
            owners: BTreeMap::new(),
            waiting: BTreeMap::new(),
            deadlocks_detected: 0,
            recoveries: 0,
        }
    }

    /// 授予资源给 agent。
    pub fn acquire(&mut self, agent: u64, res: Resource) -> Result<(), String> {
        if let Some(holder) = self.owners.get(&res) {
            if *holder != agent {
                return Err(format!("资源 {:?} 已被 {} 持有", res, holder));
            }
        }
        self.owners.insert(res.clone(), agent);
        if let Some(w) = self.waiting.get_mut(&agent) {
            w.remove(&res);
        }
        self.rebuild_edges(agent);
        Ok(())
    }

    /// agent 请求资源；若被他人持有则登记等待。
    pub fn request(&mut self, agent: u64, res: Resource) -> Result<bool, String> {
        match self.owners.get(&res).copied() {
            None => {
                self.owners.insert(res, agent);
                Ok(true)
            }
            Some(holder) if holder == agent => Ok(true),
            Some(holder) => {
                self.waiting.entry(agent).or_default().insert(res.clone());
                self.edges.entry(agent).or_default().insert(holder);
                Ok(false)
            }
        }
    }

    /// 释放资源。
    pub fn release(&mut self, agent: u64, res: &Resource) -> bool {
        if self.owners.get(res) == Some(&agent) {
            self.owners.remove(res);
            // 所有等待该资源的人，等待边可能变化。
            let waiters: Vec<u64> = self
                .waiting
                .iter()
                .filter(|(_, set)| set.contains(res))
                .map(|(a, _)| *a)
                .collect();
            for w in waiters {
                self.rebuild_edges(w);
            }
            true
        } else {
            false
        }
    }

    fn rebuild_edges(&mut self, agent: u64) {
        let holders: BTreeSet<u64> = self
            .waiting
            .get(&agent)
            .map(|rs| {
                rs.iter()
                    .filter_map(|r| self.owners.get(r).copied())
                    .filter(|h| *h != agent)
                    .collect()
            })
            .unwrap_or_default();
        if holders.is_empty() {
            self.edges.remove(&agent);
        } else {
            self.edges.insert(agent, holders);
        }
    }

    /// 检测一个环，返回环上的 agent 序列。
    pub fn find_cycle(&self) -> Option<Vec<u64>> {
        let mut visited = BTreeSet::new();
        for &start in self.edges.keys() {
            let mut stack = Vec::new();
            let mut on_stack = BTreeSet::new();
            if let Some(cycle) = self.dfs(start, &mut visited, &mut stack, &mut on_stack) {
                return Some(cycle);
            }
        }
        None
    }

    fn dfs(
        &self,
        node: u64,
        visited: &mut BTreeSet<u64>,
        stack: &mut Vec<u64>,
        on_stack: &mut BTreeSet<u64>,
    ) -> Option<Vec<u64>> {
        if on_stack.contains(&node) {
            let pos = stack.iter().position(|n| *n == node).unwrap_or(0);
            return Some(stack[pos..].to_vec());
        }
        if !visited.insert(node) {
            return None;
        }
        on_stack.insert(node);
        stack.push(node);
        if let Some(nexts) = self.edges.get(&node) {
            for &n in nexts {
                if let Some(c) = self.dfs(n, visited, stack, on_stack) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        on_stack.remove(&node);
        None
    }

    /// 检测并尝试恢复：终止环上 id 最大的 agent（牺牲者），释放其资源。
    pub fn detect_and_recover(&mut self) -> Option<Vec<u64>> {
        let cycle = self.find_cycle()?;
        self.deadlocks_detected += 1;
        let victim = *cycle.iter().max().unwrap_or(&0);
        self.kill(victim);
        self.recoveries += 1;
        Some(cycle)
    }

    /// 模拟终止：移除该 agent 的边与持有的资源。
    pub fn kill(&mut self, agent: u64) {
        self.edges.remove(&agent);
        self.waiting.remove(&agent);
        let owned: Vec<Resource> = self
            .owners
            .iter()
            .filter(|(_, a)| **a == agent)
            .map(|(r, _)| r.clone())
            .collect();
        for r in owned {
            self.owners.remove(&r);
        }
        for set in self.edges.values_mut() {
            set.remove(&agent);
        }
    }

    /// 简化安全状态检查：无环即安全。
    pub fn is_safe(&self) -> bool {
        self.find_cycle().is_none()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|s| s.len()).sum()
    }
}

impl Default for WaitForGraph {
    fn default() -> Self {
        Self::new()
    }
}

static WFG: OnceLock<Mutex<WaitForGraph>> = OnceLock::new();

pub fn wait_for_graph() -> &'static Mutex<WaitForGraph> {
    WFG.get_or_init(|| Mutex::new(WaitForGraph::new()))
}

pub fn init() {
    let _ = wait_for_graph();
}

pub fn status() -> String {
    let g = wait_for_graph().lock().unwrap();
    format!(
        "edges={} safe={} deadlocks={} recoveries={}",
        g.edge_count(),
        g.is_safe(),
        g.deadlocks_detected,
        g.recoveries
    )
}

/// `act deadlock/*` 的后端动作：
/// * `graph` 打印等待图
/// * `request <agent> <kind> <id>` 申请资源（被占用则登记等待）
/// * `acquire <agent> <kind> <id>` 强制授予
/// * `release <agent> <kind> <id>` 释放
/// * `detect` 检测环
/// * `recover` 检测并恢复（终止牺牲者）
/// * `kill <agent>` 模拟终止
/// * `status`
pub fn deadlock_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let action = it.next().unwrap_or("graph").to_string();
    let parse_triple = |it: &mut std::str::SplitWhitespace<'_>| -> Result<(u64, Resource), String> {
        let agent = it
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("用法: <agent> <kind> <id>")?;
        let kind = it.next().ok_or("用法: <agent> <kind> <id>")?;
        let id = it.next().ok_or("用法: <agent> <kind> <id>")?;
        Ok((agent, Resource::new(kind, id)))
    };
    let mut g = wait_for_graph().lock().unwrap();
    match action.as_str() {
        "graph" | "status" => {
            let mut out = String::from("等待图:\n");
            match g.find_cycle() {
                Some(cycle) => out.push_str(&format!("  检测到环: {:?}\n", cycle)),
                None => out.push_str("  无环（安全）\n"),
            }
            out.push_str(&format!(
                "  edges={} deadlocks={} recoveries={}\n",
                g.edge_count(),
                g.deadlocks_detected,
                g.recoveries
            ));
            Ok(out)
        }
        "request" => {
            let (agent, res) = parse_triple(&mut it)?;
            let granted = g.request(agent, res.clone())?;
            Ok(if granted {
                format!("已授予 agent={} 资源 {:?}", agent, res)
            } else {
                format!("agent={} 等待资源 {:?}", agent, res)
            })
        }
        "acquire" => {
            let (agent, res) = parse_triple(&mut it)?;
            g.acquire(agent, res.clone())?;
            Ok(format!("已强制授予 agent={} 资源 {:?}", agent, res))
        }
        "release" => {
            let (agent, res) = parse_triple(&mut it)?;
            if g.release(agent, &res) {
                Ok(format!("已释放 {:?}", res))
            } else {
                Err(format!("agent={} 不持有资源 {:?}", agent, res))
            }
        }
        "detect" => match g.find_cycle() {
            Some(cycle) => Ok(format!("检测到死锁环: {:?}", cycle)),
            None => Ok("未检测到死锁".to_string()),
        },
        "recover" => match g.detect_and_recover() {
            Some(cycle) => Ok(format!("已恢复：终止牺牲者，环={:?}", cycle)),
            None => Ok("无死锁可恢复".to_string()),
        },
        "kill" => {
            let agent = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: kill <agent>")?;
            g.kill(agent);
            Ok(format!("已终止 agent={}", agent))
        }
        other => Err(format!("未知 deadlock 动作: {}", other)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutual_wait_creates_cycle() {
        let mut g = WaitForGraph::new();
        let r1 = Resource::new("model", "inst-1");
        let r2 = Resource::new("model", "inst-2");
        g.acquire(1, r1.clone()).unwrap();
        g.acquire(2, r2.clone()).unwrap();
        assert!(!g.request(1, r2).unwrap());
        assert!(!g.request(2, r1).unwrap());
        assert!(!g.is_safe());
        assert_eq!(g.find_cycle().unwrap().len(), 2);
    }

    #[test]
    fn recovery_kills_victim() {
        let mut g = WaitForGraph::new();
        let r1 = Resource::new("tool", "shell");
        let r2 = Resource::new("tool", "gpu");
        g.acquire(1, r1.clone()).unwrap();
        g.acquire(2, r2.clone()).unwrap();
        g.request(1, r2).unwrap();
        g.request(2, r1).unwrap();
        let cycle = g.detect_and_recover().unwrap();
        assert!(cycle.contains(&2));
        assert!(g.is_safe());
        assert_eq!(g.recoveries, 1);
    }

    #[test]
    fn no_deadlock_without_cycle() {
        let mut g = WaitForGraph::new();
        let r = Resource::new("ctx", "w1");
        g.acquire(1, r.clone()).unwrap();
        assert!(!g.request(2, r).unwrap());
        assert!(g.is_safe());
        assert!(g.find_cycle().is_none());
    }

    #[test]
    fn release_unblocks_waiter() {
        let mut g = WaitForGraph::new();
        let r = Resource::new("model", "m");
        g.acquire(1, r.clone()).unwrap();
        g.request(2, r.clone()).unwrap();
        assert_eq!(g.edge_count(), 1);
        g.release(1, &r);
        assert_eq!(g.edge_count(), 0);
    }
}
