//! GPU 调度器 —— MLFQ (多级反馈队列) + 语义感知。
//!
//! 调度器持有 AI 意识本体，可根据概念权重与调度策略做出选择。

use crate::ai_token::{AIConsciousness, ConceptId, SchedulePolicy, TokenId};
use crate::monitor::get_time_ns;

/// 时间片长度: 10 ms。
pub const TIME_SLICE_NS: u64 = 10_000_000;

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
}

/// CPU / GPU 亲和性。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuAffinity {
    None,
    Core(u8),
    Group(Vec<u8>),
}

/// 就绪队列节点。
struct PriorityNode {
    token_id: TokenId,
    priority: CpuPriority,
    queue_level: SchedulerPriorityLevel,
    concept_id: Option<ConceptId>,
    semantic_weight: f32,
    last_activity: u64,
}

/// 调度器状态快照。
#[derive(Clone, Debug, Default)]
pub struct SchedulerState {
    pub running_token: Option<TokenId>,
    pub enqueued_count: u64,
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

/// GPU 调度器。
pub struct GpuScheduler {
    ready_queue: Vec<PriorityNode>,
    running: Option<TokenId>,
    pub state: SchedulerState,
    pub time_slice_counter: u64,
    pub ai_consciousness: AIConsciousness,
}

impl GpuScheduler {
    pub fn new(ai_consciousness: AIConsciousness) -> Self {
        Self {
            ready_queue: Vec::new(),
            running: None,
            state: SchedulerState::default(),
            time_slice_counter: 0,
            ai_consciousness,
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
        let queue_level = if priority == CpuPriority::Critical {
            SchedulerPriorityLevel::RealTime
        } else if priority >= CpuPriority::High {
            SchedulerPriorityLevel::High
        } else if priority == CpuPriority::Medium {
            SchedulerPriorityLevel::Medium
        } else {
            SchedulerPriorityLevel::Low
        };
        self.ready_queue.push(PriorityNode {
            token_id,
            priority,
            queue_level,
            concept_id,
            semantic_weight,
            last_activity: get_time_ns(),
        });
        self.state.enqueued_count += 1;
    }

    /// 选择下一个要运行的 Token。
    pub fn pick_next(&mut self) -> Option<TokenId> {
        if self.ready_queue.is_empty() {
            self.running = None;
            self.state.running_token = None;
            return None;
        }

        // 排序键: 层级(降序) → 优先级(降序) → 语义权重(降序) → 活动时间(升序)
        self.ready_queue.sort_by(|a, b| {
            b.queue_level
                .rank()
                .cmp(&a.queue_level.rank())
                .then(b.priority.rank().cmp(&a.priority.rank()))
                .then(
                    b.semantic_weight
                        .partial_cmp(&a.semantic_weight)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.last_activity.cmp(&b.last_activity))
        });

        let node = self.ready_queue.remove(0);
        self.running = Some(node.token_id);
        self.state.running_token = Some(node.token_id);
        self.state.time_slice_expired = false;
        Some(node.token_id)
    }

    /// 时间片到期 —— 将当前任务放回队尾。
    pub fn on_time_slice_expiry(&mut self) {
        if let Some(running) = self.running {
            if let Some(idx) = self.ready_queue.iter().position(|n| n.token_id == running) {
                let mut node = self.ready_queue.remove(idx);
                node.last_activity = get_time_ns();
                self.ready_queue.push(node);
            }
        }
        self.running = None;
        self.state.running_token = None;
        self.time_slice_counter = 0;
        self.state.time_slice_expired = true;
    }

    pub fn get_state(&self) -> SchedulerState {
        self.state.clone()
    }

    pub fn ready_count(&self) -> usize {
        self.ready_queue.len()
    }

    /// 查询某 Token 的策略 (若在队列中)。
    pub fn get_schedule_policy(&self, token_id: TokenId) -> Option<SchedulePolicy> {
        let concept = self
            .ready_queue
            .iter()
            .find(|n| n.token_id == token_id)
            .and_then(|n| n.concept_id)?;
        // 用概念自身的模式做一次策略查找
        let node = self.ai_consciousness.get(concept)?;
        Some(
            self.ai_consciousness
                .find_schedule_policy(&node.compute_pattern)
                .unwrap_or(SchedulePolicy::Batch),
        )
    }
}

/// 初始化调度器子系统 (极简启动，不在终端打印)。
pub fn init() {
    // GPU 调度器已就绪；启动阶段保持静默。
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
        // 同为 Medium 优先级时，语义权重更高者先出队
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
}

