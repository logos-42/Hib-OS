//! AI 智能体调度器 —— 把「CPU 调度」映射为「模型推理配额调度」。
//!
//! 对应《现代操作系统》第 2.4 节「调度」：
//! * 就绪队列按优先级分层（High / Medium / Low）；
//! * 每层内部使用虚拟运行时间 (vruntime) 实现近似公平；
//! * 支持模型实例亲和 (affinity) 与预算 (token budget)；
//! * 提供 pick_next / enqueue / dequeue / tick / status。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::scheduler::CpuPriority;

/// 调度条目。
#[derive(Clone, Debug)]
pub struct SchedEntry {
    pub agent_id: u64,
    pub priority: CpuPriority,
    /// 虚拟运行时间（越小越优先）。
    pub vruntime: u64,
    /// 本轮 token 预算。
    pub budget: u64,
    /// 已消耗。
    pub consumed: u64,
    /// 模型实例亲和。
    pub affinity: Option<u64>,
}

impl SchedEntry {
    pub fn new(agent_id: u64, priority: CpuPriority, budget: u64) -> Self {
        Self {
            agent_id,
            priority,
            vruntime: 0,
            budget,
            consumed: 0,
            affinity: None,
        }
    }

    pub fn weight(&self) -> u64 {
        match self.priority {
            CpuPriority::Critical => 4,
            CpuPriority::High => 3,
            CpuPriority::Medium => 2,
            CpuPriority::Low => 1,
        }
    }

    pub fn exhausted(&self) -> bool {
        self.budget > 0 && self.consumed >= self.budget
    }
}

/// 单层就绪队列。
#[derive(Default)]
pub struct RunQueue {
    entries: Vec<SchedEntry>,
}

impl RunQueue {
    pub fn enqueue(&mut self, e: SchedEntry) {
        self.entries.push(e);
    }

    /// 选 vruntime 最小的条目。
    pub fn pick(&self) -> Option<&SchedEntry> {
        self.entries.iter().min_by_key(|e| e.vruntime)
    }

    pub fn take_next(&mut self) -> Option<SchedEntry> {
        let idx = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.vruntime)
            .map(|(i, _)| i)?;
        Some(self.entries.remove(idx))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 多级调度器。
pub struct AiScheduler {
    critical: RunQueue,
    high: RunQueue,
    medium: RunQueue,
    low: RunQueue,
    /// 已完成调度的 agent 累计配额账本。
    ledger: BTreeMap<u64, u64>,
    pub switches: u64,
}

impl AiScheduler {
    pub fn new() -> Self {
        Self {
            critical: RunQueue::default(),
            high: RunQueue::default(),
            medium: RunQueue::default(),
            low: RunQueue::default(),
            ledger: BTreeMap::new(),
            switches: 0,
        }
    }

    fn queue_mut(&mut self, p: CpuPriority) -> &mut RunQueue {
        match p {
            CpuPriority::Critical | CpuPriority::High => &mut self.high,
            CpuPriority::Medium => &mut self.medium,
            CpuPriority::Low => &mut self.low,
        }
    }

    pub fn enqueue(&mut self, e: SchedEntry) {
        let p = e.priority;
        self.queue_mut(p).enqueue(e);
    }

    /// 高优先级先选，同级按 vruntime。
    pub fn pick_next(&self) -> Option<&SchedEntry> {
        self.critical
            .pick()
            .or_else(|| self.high.pick())
            .or_else(|| self.medium.pick())
            .or_else(|| self.low.pick())
    }

    /// 取下一个并记一次上下文切换。
    pub fn dequeue_next(&mut self) -> Option<SchedEntry> {
        let e = if !self.critical.is_empty() {
            self.critical.take_next()
        } else if !self.high.is_empty() {
            self.high.take_next()
        } else if !self.medium.is_empty() {
            self.medium.take_next()
        } else {
            self.low.take_next()
        };
        if e.is_some() {
            self.switches += 1;
        }
        e
    }

    /// 记账：消耗 token 后按权重推进 vruntime。
    pub fn charge(&mut self, agent_id: u64, tokens: u64) {
        let delta = tokens.saturating_mul(1024) / 3;
        *self.ledger.entry(agent_id).or_insert(0) += tokens;
        for q in [
            &mut self.critical,
            &mut self.high,
            &mut self.medium,
            &mut self.low,
        ] {
            for e in q.entries.iter_mut().filter(|e| e.agent_id == agent_id) {
                e.consumed += tokens;
                e.vruntime += delta / e.weight();
            }
        }
    }

    pub fn total_ready(&self) -> usize {
        self.critical.len() + self.high.len() + self.medium.len() + self.low.len()
    }

    pub fn ledger(&self) -> &BTreeMap<u64, u64> {
        &self.ledger
    }
}

impl Default for AiScheduler {
    fn default() -> Self {
        Self::new()
    }
}

static SCHED: OnceLock<Mutex<AiScheduler>> = OnceLock::new();

pub fn scheduler() -> &'static Mutex<AiScheduler> {
    SCHED.get_or_init(|| Mutex::new(AiScheduler::new()))
}

pub fn init() {
    // 预留：启动时可按模型实例亲和预热队列。
}

pub fn status() -> String {
    let s = scheduler().lock().unwrap();
    format!(
        "ready={} switches={} ledger={}",
        s.total_ready(),
        s.switches,
        s.ledger().len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ordering() {
        let mut s = AiScheduler::new();
        s.enqueue(SchedEntry::new(1, CpuPriority::Low, 100));
        s.enqueue(SchedEntry::new(2, CpuPriority::High, 100));
        assert_eq!(s.pick_next().unwrap().agent_id, 2);
        assert_eq!(s.dequeue_next().unwrap().agent_id, 2);
        assert_eq!(s.dequeue_next().unwrap().agent_id, 1);
    }

    #[test]
    fn vruntime_fairness() {
        let mut s = AiScheduler::new();
        s.enqueue(SchedEntry::new(1, CpuPriority::Medium, 0));
        s.enqueue(SchedEntry::new(2, CpuPriority::Medium, 0));
        s.charge(1, 300);
        assert_eq!(s.pick_next().unwrap().agent_id, 2);
    }

    #[test]
    fn charge_records_ledger() {
        let mut s = AiScheduler::new();
        s.enqueue(SchedEntry::new(7, CpuPriority::Low, 10));
        s.charge(7, 5);
        assert_eq!(s.ledger()[&7], 5);
        assert_eq!(s.total_ready(), 1);
    }

    #[test]
    fn exhausted_budget() {
        let mut s = AiScheduler::new();
        s.enqueue(SchedEntry::new(3, CpuPriority::Low, 2));
        s.charge(3, 5);
        assert!(s.total_ready() == 1);
        let e = s.pick_next().unwrap();
        assert!(e.exhausted());
    }
}
