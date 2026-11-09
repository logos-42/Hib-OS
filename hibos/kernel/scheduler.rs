//! GPU / 执行体调度器 —— MLFQ + 语义权重 + 防饿死老化。
//!
//! 这一版修掉了原型里两个真实缺陷：
//!
//! 1. **时间片到期会丢任务**：旧实现 `pick_next` 把节点从队列 `remove(0)` 之后，
//!    `on_time_slice_expiry` 再去队列里找它，永远找不到 —— 任务被静默丢弃。
//!    现在运行中的节点被单独保存在 `running_node`，到期后按 MLFQ 规则**降级重入队**。
//! 2. **低优先级永久饿死**：旧实现是纯排序队列，只要持续有高优先级任务入队，
//!    低优先级永远排不到。现在引入**老化**（等待越久有效层级越高），
//!    并给出可观测的 `aged` 计数。
//!
//! 与 Linux 的差异（AI 原生）：
//! * 排序键里有 `semantic_weight`（概念图算出来的「这件事有多重要」）。
//! * 等待不是靠轮询，而是 `mark_waiting(token, channel)` + `wake_key(channel)`，
//!    与 `exec` 的事件等待表联动 —— 就绪/等待是同一套事件机制的两个视图。

use std::collections::BTreeMap;

use crate::ai_token::{AIConsciousness, ConceptId, SchedulePolicy, TokenId};
use crate::monitor::get_time_ns;

/// 时间片长度: 10 ms。
pub const TIME_SLICE_NS: u64 = 10_000_000;

/// 老化单位：等待超过这么久，有效层级 +1。
pub const AGING_UNIT_NS: u64 = 50_000_000;

/// 老化上限：最多提升 3 级（保证实时任务仍能压过长期等待的批处理）。
pub const AGING_MAX: u8 = 3;

/// CPU / GPU 调度优先级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CpuPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl CpuPriority {
    pub fn rank(self) -> u8 {
        match self {
            CpuPriority::Low => 0,
            CpuPriority::Medium => 1,
            CpuPriority::High => 2,
            CpuPriority::Critical => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CpuPriority::Low => "Low",
            CpuPriority::Medium => "Medium",
            CpuPriority::High => "High",
            CpuPriority::Critical => "Critical",
        }
    }

    /// 优先级提升（用于优先级继承）。
    pub fn raise_to(&mut self, to: CpuPriority) {
        if to > *self {
            *self = to;
        }
    }
}

impl Default for CpuPriority {
    fn default() -> Self {
        CpuPriority::Medium
    }
}

/// MLFQ 队列层级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SchedulerPriorityLevel {
    RealTime = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

impl SchedulerPriorityLevel {
    /// 数值越大越优先。
    fn rank(self) -> u8 {
        match self {
            SchedulerPriorityLevel::RealTime => 3,
            SchedulerPriorityLevel::High => 2,
            SchedulerPriorityLevel::Medium => 1,
            SchedulerPriorityLevel::Low => 0,
        }
    }

    /// MLFQ 降级：用完时间片就往下掉一层，最低停在 Low。
    fn demote(self) -> Self {
        match self {
            SchedulerPriorityLevel::RealTime => SchedulerPriorityLevel::High,
            SchedulerPriorityLevel::High => SchedulerPriorityLevel::Medium,
            SchedulerPriorityLevel::Medium => SchedulerPriorityLevel::Low,
            SchedulerPriorityLevel::Low => SchedulerPriorityLevel::Low,
        }
    }

    fn name(self) -> &'static str {
        match self {
            SchedulerPriorityLevel::RealTime => "RT",
            SchedulerPriorityLevel::High => "HIGH",
            SchedulerPriorityLevel::Medium => "MED",
            SchedulerPriorityLevel::Low => "LOW",
        }
    }
}

/// CPU / GPU 亲和性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuAffinity {
    None,
    Core(u8),
    Group(Vec<u8>),
}

/// 就绪队列节点。
#[derive(Clone, Debug)]
struct PriorityNode {
    token_id: TokenId,
    priority: CpuPriority,
    queue_level: SchedulerPriorityLevel,
    concept_id: Option<ConceptId>,
    semantic_weight: f32,
    last_activity: u64,
    /// 进入队列的时间，用于老化。
    entered_ns: u64,
    /// 已用掉的时间片数。
    slices_used: u32,
}

impl PriorityNode {
    /// 有效层级 = 基础层级 + 老化加成。
    fn effective_rank(&self, now_ns: u64) -> u8 {
        let waited = now_ns.saturating_sub(self.entered_ns);
        let bonus = (waited / AGING_UNIT_NS).min(AGING_MAX as u64) as u8;
        self.queue_level.rank().saturating_add(bonus)
    }
}

/// 调度器状态快照。
#[derive(Clone, Debug, Default)]
pub struct SchedulerState {
    pub running_token: Option<TokenId>,
    pub running_level: Option<SchedulerPriorityLevel>,
    pub enqueued_count: u64,
    pub requeues: u64,
    pub preemptions: u64,
    pub waiting_count: usize,
    pub time_slice_expired: bool,
}

/// 调度错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerError {
    InvalidProcessState(String),
    NoReadyProcess,
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchedulerError::InvalidProcessState(s) => write!(f, "非法进程状态: {}", s),
            SchedulerError::NoReadyProcess => write!(f, "没有就绪进程"),
        }
    }
}

/// 调度器。
pub struct GpuScheduler {
    ready_queue: Vec<PriorityNode>,
    /// 正在运行的节点。旧实现没有这个字段，导致时间片到期后任务丢失。
    running_node: Option<PriorityNode>,
    running: Option<TokenId>,
    /// 事件等待表：通道键 → 等待中的 token。与 `exec` 是同一套语义的调度视图。
    waiting: BTreeMap<String, Vec<TokenId>>,
    pub state: SchedulerState,
    pub time_slice_counter: u64,
    pub ai_consciousness: AIConsciousness,
}

impl GpuScheduler {
    pub fn new(ai_consciousness: AIConsciousness) -> Self {
        Self {
            ready_queue: Vec::new(),
            running_node: None,
            running: None,
            waiting: BTreeMap::new(),
            state: SchedulerState::default(),
            time_slice_counter: 0,
            ai_consciousness,
        }
    }

    fn level_for(priority: CpuPriority) -> SchedulerPriorityLevel {
        match priority {
            CpuPriority::Critical => SchedulerPriorityLevel::RealTime,
            CpuPriority::High => SchedulerPriorityLevel::High,
            CpuPriority::Medium => SchedulerPriorityLevel::Medium,
            CpuPriority::Low => SchedulerPriorityLevel::Low,
        }
    }

    /// 入队 (带语义概念)。
    pub fn enqueue(
        &mut self,
        token_id: TokenId,
        priority: CpuPriority,
        concept_id: Option<ConceptId>,
    ) {
        let semantic_weight = concept_id
            .map(|c| self.ai_consciousness.get_priority(c))
            .unwrap_or(0.5);
        let now = get_time_ns();
        self.ready_queue.push(PriorityNode {
            token_id,
            priority,
            queue_level: Self::level_for(priority),
            concept_id,
            semantic_weight,
            last_activity: now,
            entered_ns: now,
            slices_used: 0,
        });
        self.state.enqueued_count += 1;
    }

    /// 选择下一个要运行的 Token。
    ///
    /// 注意：与旧行为一致，这里**不**回收上一个运行节点 —— 调用方（内核主循环）
    /// 必须先用 `on_time_slice_expiry` / `yield_running` 把它放回队列。
    /// 到期回收由 `on_time_slice_expiry` 负责，任务不会再被静默丢弃。
    pub fn pick_next(&mut self) -> Option<TokenId> {
        self.pick_next_at(get_time_ns())
    }

    /// 以指定时间戳调度（便于确定性测试老化）。
    pub fn pick_next_at(&mut self, now: u64) -> Option<TokenId> {
        if self.ready_queue.is_empty() {
            self.running = None;
            self.state.running_token = None;
            self.state.running_level = None;
            return None;
        }

        // 排序键: 有效层级(含老化, 降序) → 优先级(降序) → 语义权重(降序) → 入队时间(升序)
        self.ready_queue.sort_by(|a, b| {
            b.effective_rank(now)
                .cmp(&a.effective_rank(now))
                .then(b.priority.rank().cmp(&a.priority.rank()))
                .then(
                    b.semantic_weight
                        .partial_cmp(&a.semantic_weight)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.entered_ns.cmp(&b.entered_ns))
        });

        let node = self.ready_queue.remove(0);
        self.running = Some(node.token_id);
        self.running_node = Some(node.clone());
        self.state.running_token = Some(node.token_id);
        self.state.running_level = Some(node.queue_level);
        self.state.time_slice_expired = false;
        self.time_slice_counter = 0;
        Some(node.token_id)
    }

    /// 时间片到期 —— 运行中的任务**降级后重入队**（旧实现会把它丢掉）。
    pub fn on_time_slice_expiry(&mut self) {
        if let Some(mut node) = self.running_node.take() {
            node.queue_level = node.queue_level.demote();
            node.slices_used += 1;
            node.last_activity = get_time_ns();
            node.entered_ns = get_time_ns();
            self.ready_queue.push(node);
            self.state.requeues += 1;
        }
        self.running = None;
        self.state.running_token = None;
        self.state.running_level = None;
        self.time_slice_counter = 0;
        self.state.time_slice_expired = true;
    }

    /// 主动让出（不降级，保持层级），用于 I/O 密集任务。
    pub fn yield_running(&mut self) {
        if let Some(node) = self.running_node.take() {
            self.requeue(node, true);
        }
        self.running = None;
        self.state.running_token = None;
        self.state.running_level = None;
    }

    fn requeue(&mut self, mut node: PriorityNode, keep_level: bool) {
        if !keep_level {
            node.queue_level = node.queue_level.demote();
            node.slices_used += 1;
        }
        node.entered_ns = get_time_ns();
        self.ready_queue.push(node);
        self.state.requeues += 1;
    }

    /// 把 token 标记为「等待某条通道的事件」，移出就绪队列。
    pub fn mark_waiting(&mut self, token_id: TokenId, channel_key: &str) {
        if let Some(pos) = self.ready_queue.iter().position(|n| n.token_id == token_id) {
            self.ready_queue.remove(pos);
            self.waiting
                .entry(channel_key.to_string())
                .or_default()
                .push(token_id);
            self.state.waiting_count = self.waiting.values().map(|v| v.len()).sum();
        }
    }

    /// 通道事件到达：把等待在该键上的 token 放回就绪队列。
    pub fn wake_key(&mut self, channel_key: &str) -> usize {
        let ids = self.waiting.remove(channel_key).unwrap_or_default();
        let now = get_time_ns();
        for token_id in &ids {
            self.ready_queue.push(PriorityNode {
                token_id: *token_id,
                priority: CpuPriority::Medium,
                queue_level: SchedulerPriorityLevel::Medium,
                concept_id: None,
                semantic_weight: 0.5,
                last_activity: now,
                // 从等待被唤醒时重新计老化，避免"睡久了就直接插队"。
                entered_ns: now,
                slices_used: 0,
            });
        }
        self.state.waiting_count = self.waiting.values().map(|v| v.len()).sum();
        ids.len()
    }

    /// 优先级继承入口：抬升某个 token 的优先级（供 lease / sync 调用）。
    pub fn boost(&mut self, token_id: TokenId, to: CpuPriority) -> bool {
        for n in self.ready_queue.iter_mut() {
            if n.token_id == token_id {
                n.priority.raise_to(to);
                n.queue_level = Self::level_for(n.priority);
                return true;
            }
        }
        if let Some(n) = self.running_node.as_mut() {
            if n.token_id == token_id {
                n.priority.raise_to(to);
                return true;
            }
        }
        false
    }

    /// 统计多少任务在等待（含事件等待表）。
    pub fn waiting_count(&self) -> usize {
        self.waiting.values().map(|v| v.len()).sum()
    }

    /// 可观测摘要。
    pub fn summary(&self) -> String {
        let now = get_time_ns();
        let aged = self
            .ready_queue
            .iter()
            .filter(|n| n.effective_rank(now) > n.queue_level.rank())
            .count();
        let running = match (&self.running, self.state.running_level) {
            (Some(t), Some(l)) => format!("{}@{}", t.0, l.name()),
            (Some(t), None) => t.0.to_string(),
            _ => "-".to_string(),
        };
        format!(
            "ready={} waiting={} running={} enqueued={} requeues={} aged={} slice=10ms policy=MLFQ+semantic+aging",
            self.ready_queue.len(),
            self.waiting_count(),
            running,
            self.state.enqueued_count,
            self.state.requeues,
            aged
        )
    }

    pub fn get_state(&self) -> SchedulerState {
        let mut s = self.state.clone();
        s.waiting_count = self.waiting_count();
        s
    }

    pub fn ready_count(&self) -> usize {
        self.ready_queue.len()
    }

    /// 查询某 Token 的策略 (若在队列中)。
    pub fn get_schedule_policy(&self, token_id: TokenId) -> Option<SchedulePolicy> {
        let node = self
            .ready_queue
            .iter()
            .find(|n| n.token_id == token_id)
            .or_else(|| self.running_node.as_ref().filter(|n| n.token_id == token_id))?;
        let concept = node.concept_id?;
        let n = self.ai_consciousness.get(concept)?;
        Some(
            self.ai_consciousness
                .find_schedule_policy(&n.compute_pattern)
                .unwrap_or(SchedulePolicy::Batch),
        )
    }
}

/// 初始化调度器子系统 (极简启动，不在终端打印)。
pub fn init() {
    // 调度器已就绪；启动阶段保持静默。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consciousness() -> AIConsciousness {
        let mut c = AIConsciousness::new();
        c.add_common_concepts();
        c
    }

    #[test]
    fn priority_ordering() {
        let mut s = GpuScheduler::new(consciousness());
        let low = TokenId::new();
        let high = TokenId::new();
        let crit = TokenId::new();
        s.enqueue(low, CpuPriority::Low, None);
        s.enqueue(high, CpuPriority::High, None);
        s.enqueue(crit, CpuPriority::Critical, None);

        assert_eq!(s.ready_count(), 3);
        assert_eq!(s.pick_next(), Some(crit));
        assert_eq!(s.pick_next(), Some(high));
        assert_eq!(s.pick_next(), Some(low));
        assert_eq!(s.pick_next(), None);
    }

    #[test]
    fn semantic_weight_breaks_ties() {
        let mut c = consciousness();
        c.add_named(
            "Tiny_Job",
            crate::ai_token::ComputePattern::Custom("tiny".into()),
            SchedulePolicy::Batch,
            1024,
            0.10,
        );
        let heavy = c.search_by_name("Transformer_Attention_Base").unwrap().id;
        let tiny = c.search_by_name("Tiny_Job").unwrap().id;
        let mut s = GpuScheduler::new(c);
        let t_heavy = TokenId::new();
        let t_tiny = TokenId::new();
        s.enqueue(t_tiny, CpuPriority::Medium, Some(tiny));
        s.enqueue(t_heavy, CpuPriority::Medium, Some(heavy));
        assert_eq!(s.pick_next(), Some(t_heavy));
    }

    #[test]
    fn time_slice_expiry_flag() {
        let mut s = GpuScheduler::new(consciousness());
        let t = TokenId::new();
        s.enqueue(t, CpuPriority::Medium, None);
        assert_eq!(s.pick_next(), Some(t));
        s.on_time_slice_expiry();
        assert!(s.state.time_slice_expired);
        assert_eq!(s.state.running_token, None);
    }

    #[test]
    fn enqueued_count_tracks() {
        let mut s = GpuScheduler::new(consciousness());
        s.enqueue(TokenId::new(), CpuPriority::Low, None);
        s.enqueue(TokenId::new(), CpuPriority::Low, None);
        assert_eq!(s.get_state().enqueued_count, 2);
    }

    #[test]
    fn expiry_does_not_lose_the_task() {
        let mut s = GpuScheduler::new(consciousness());
        let t = TokenId::new();
        s.enqueue(t, CpuPriority::High, None);
        assert_eq!(s.pick_next(), Some(t));
        s.on_time_slice_expiry();
        // 旧实现这里会把任务丢掉，ready_count 为 0 且再也跑不到。
        assert_eq!(s.ready_count(), 1, "到期任务必须重新入队");
        assert_eq!(s.pick_next(), Some(t), "降级后仍应被调度到");
        // 层级应从 High 降到 Medium
        assert_eq!(s.state.running_level, Some(SchedulerPriorityLevel::Medium));
    }

    #[test]
    fn mlfq_demotes_on_each_slice() {
        let mut s = GpuScheduler::new(consciousness());
        let t = TokenId::new();
        s.enqueue(t, CpuPriority::Critical, None);
        assert_eq!(s.pick_next(), Some(t));
        s.on_time_slice_expiry();
        assert_eq!(s.pick_next(), Some(t));
        assert_eq!(s.state.running_level, Some(SchedulerPriorityLevel::High));
        s.on_time_slice_expiry();
        assert_eq!(s.pick_next(), Some(t));
        assert_eq!(s.state.running_level, Some(SchedulerPriorityLevel::Medium));
        s.on_time_slice_expiry();
        assert_eq!(s.pick_next(), Some(t));
        assert_eq!(s.state.running_level, Some(SchedulerPriorityLevel::Low));
        s.on_time_slice_expiry();
        assert_eq!(s.pick_next(), Some(t));
        // 最低就停在 Low，不会跌出队列
        assert_eq!(s.state.running_level, Some(SchedulerPriorityLevel::Low));
    }

    #[test]
    fn aging_prevents_starvation() {
        let mut s = GpuScheduler::new(consciousness());
        let old_low = TokenId::new();
        s.enqueue(old_low, CpuPriority::Low, None);
        // 手工把入队时间归零，等价于"已经等了很久"（确定性测试，不依赖挂钟）。
        s.ready_queue.last_mut().unwrap().entered_ns = 0;
        // 持续灌入新的 High 任务
        let fresh_high = TokenId::new();
        s.enqueue(fresh_high, CpuPriority::High, None);
        let now = AGING_UNIT_NS * 10;
        // 新任务的时间戳对齐到 now，避免它也被"老化"（真实调度里它才刚入队）。
        s.ready_queue.last_mut().unwrap().entered_ns = now;
        // 老任务老化到顶(3) + Low(0) = 3 > High(2)，应优先
        assert_eq!(s.pick_next_at(now), Some(old_low), "老化必须让等待久的任务插队");
    }

    #[test]
    fn effective_rank_is_capped() {
        let mut s = GpuScheduler::new(consciousness());
        s.enqueue(TokenId::new(), CpuPriority::Low, None);
        s.ready_queue.last_mut().unwrap().entered_ns = 0;
        let n = &s.ready_queue[0];
        assert_eq!(n.effective_rank(0), 0);
        assert_eq!(n.effective_rank(AGING_UNIT_NS), 1);
        assert_eq!(n.effective_rank(AGING_UNIT_NS * 100), AGING_MAX);
    }

    #[test]
    fn waiting_tokens_leave_and_rejoin_ready_queue() {
        let mut s = GpuScheduler::new(consciousness());
        let t = TokenId::new();
        s.enqueue(t, CpuPriority::Medium, None);
        assert_eq!(s.ready_count(), 1);
        s.mark_waiting(t, "gpu://vram");
        assert_eq!(s.ready_count(), 0);
        assert_eq!(s.waiting_count(), 1);
        assert_eq!(s.wake_key("gpu://vram"), 1);
        assert_eq!(s.ready_count(), 1);
        assert_eq!(s.waiting_count(), 0);
        assert_eq!(s.pick_next(), Some(t));
    }

    #[test]
    fn boost_promotes_queued_token() {
        let mut s = GpuScheduler::new(consciousness());
        let low = TokenId::new();
        let high = TokenId::new();
        s.enqueue(low, CpuPriority::Low, None);
        s.enqueue(high, CpuPriority::High, None);
        assert!(s.boost(low, CpuPriority::Critical));
        assert_eq!(s.pick_next(), Some(low));
    }

    #[test]
    fn summary_is_observable() {
        let mut s = GpuScheduler::new(consciousness());
        s.enqueue(TokenId::new(), CpuPriority::Medium, None);
        let text = s.summary();
        assert!(text.contains("ready=1"));
        assert!(text.contains("MLFQ"));
    }
}
