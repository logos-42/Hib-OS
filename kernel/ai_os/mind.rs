//! 心智架构层 —— AI 原生 OS 的「心智」 (Mind Architecture)
//!
//! 本模块实现 AI 原生 OS 的高阶认知架构，对应传统 OS 中的「进程控制块与
//! 会话管理」概念，但重新映射到 AI 心智域：
//!
//! * MindCore (心智核心) —— 智能体心智的中央控制单元，等价于 PCB；
//! * ExecutiveFunction (执行功能) —— 计划、决策、抑制控制、认知灵活性；
//! * WorkingMemory (工作记忆) —— 短期信息暂存与操作，等价于 CPU 缓存/寄存器；
//! * EpisodicBuffer (情境缓冲) —— 最近经验的时间顺序记录；
//! * GoalStack (目标栈) —— 层级目标管理，等价于调度队列；
//! * PlanLibrary (计划库) —— 预存认知/行动计划模板；
//! * DecisionEngine (决策引擎) —— 基于效用与概率的决策；
//! * CognitiveLoad (认知负荷) —— 当前心智资源占用监控；
//! * MetacognitiveMonitor (元认知监控器) —— 对自身心智状态的监控；
//! * MindState (心智状态) —— 心智的瞬时完整表征；
//! * MindTransition (心智转换) —— 心智状态间的切换（专注/分心/沉思/休息）。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度，
//! 并通过 `mind/*` 通道接入 `sense`/`act` 感知层。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 心智状态 (MindState)
// =====================================================================================

/// 心智模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MindMode {
    /// 专注模式：单一目标，高认知负荷，低分心。
    Focused,
    /// 分心模式：多目标竞争，注意力碎片化。
    Distracted,
    /// 沉思模式：内部反思，无外部输入，低外部反应。
    Contemplative,
    /// 休息模式：最小认知活动，等待输入。
    Resting,
    /// 流动模式（Flow）：完全沉浸，主客体消融。
    Flow,
    /// 梦境模式：离线巩固，记忆重播与合成。
    Dreaming,
}

impl MindMode {
    pub fn name(self) -> &'static str {
        match self {
            MindMode::Focused => "focused",
            MindMode::Distracted => "distracted",
            MindMode::Contemplative => "contemplative",
            MindMode::Resting => "resting",
            MindMode::Flow => "flow",
            MindMode::Dreaming => "dreaming",
        }
    }

    /// 默认认知负荷系数（0~1，越高占用心智资源越多）。
    pub fn load_coefficient(self) -> f32 {
        match self {
            MindMode::Focused => 0.8,
            MindMode::Distracted => 0.9,
            MindMode::Contemplative => 0.5,
            MindMode::Resting => 0.1,
            MindMode::Flow => 0.7,
            MindMode::Dreaming => 0.3,
        }
    }
}

/// 心智状态：某一时刻心智的完整表征。
#[derive(Clone, Debug)]
pub struct MindState {
    pub id: u64,
    pub timestamp_ns: u128,
    pub mode: MindMode,
    pub cognitive_load: f32,           // 0~1
    pub arousal: f32,                  // 0~1，觉醒水平
    pub valence: f32,                  // -1~1，情绪价态
    pub attention_stability: f32,      // 0~1，注意力稳定性
    pub working_memory_usage: f32,     // 0~1
    pub active_goal_id: Option<u64>,
    pub active_plan_id: Option<u64>,
    pub meta: BTreeMap<String, String>,
}

impl MindState {
    pub fn new() -> Self {
        Self {
            id: 0,
            timestamp_ns: crate::ai_os::native::now_ns(),
            mode: MindMode::Resting,
            cognitive_load: 0.1,
            arousal: 0.3,
            valence: 0.0,
            attention_stability: 0.5,
            working_memory_usage: 0.0,
            active_goal_id: None,
            active_plan_id: None,
            meta: BTreeMap::new(),
        }
    }

    pub fn with_mode(mut self, mode: MindMode) -> Self {
        self.mode = mode;
        self.cognitive_load = mode.load_coefficient() * self.arousal;
        self
    }

    pub fn with_load(mut self, load: f32) -> Self {
        self.cognitive_load = load.clamp(0.0, 1.0);
        self
    }

    pub fn is_overloaded(&self) -> bool {
        self.cognitive_load > 0.9 || self.working_memory_usage > 0.95
    }

    pub fn available_capacity(&self) -> f32 {
        (1.0 - self.cognitive_load.max(self.working_memory_usage)).clamp(0.0, 1.0)
    }
}

// =====================================================================================
// 执行功能 (ExecutiveFunction)
// =====================================================================================

/// 执行功能子模块类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecFuncType {
    Planning,
    DecisionMaking,
    InhibitoryControl,
    CognitiveFlexibility,
    TaskSwitching,
    ErrorMonitoring,
}

/// 执行功能记录。
#[derive(Clone, Debug)]
pub struct ExecFuncRecord {
    pub id: u64,
    pub r#type: ExecFuncType,
    pub target: String,
    pub timestamp_ns: u128,
    pub success: bool,
    pub cost: f32, // 认知成本 0~1
}

/// 执行功能引擎。
pub struct ExecutiveFunction {
    records: Vec<ExecFuncRecord>,
    next_id: u64,
    pub planning_capacity: f32,
    pub inhibition_strength: f32,
    pub flexibility: f32,
}

impl ExecutiveFunction {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            planning_capacity: 0.7,
            inhibition_strength: 0.6,
            flexibility: 0.5,
        }
    }

    pub fn plan(&mut self, target: &str, complexity: f32) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let cost = complexity * (1.0 - self.planning_capacity);
        let success = cost < self.planning_capacity;
        self.records.push(ExecFuncRecord {
            id,
            r#type: ExecFuncType::Planning,
            target: target.to_string(),
            timestamp_ns: crate::ai_os::native::now_ns(),
            success,
            cost,
        });
        if success {
            Ok(id)
        } else {
            Err(format!("计划认知资源不足：成本={:.2} 能力={:.2}", cost, self.planning_capacity))
        }
    }

    pub fn inhibit(&mut self, target: &str, impulse_strength: f32) -> bool {
        let id = self.next_id;
        self.next_id += 1;
        let success = self.inhibition_strength > impulse_strength;
        self.records.push(ExecFuncRecord {
            id,
            r#type: ExecFuncType::InhibitoryControl,
            target: target.to_string(),
            timestamp_ns: crate::ai_os::native::now_ns(),
            success,
            cost: impulse_strength * 0.5,
        });
        success
    }

    pub fn switch_task(&mut self, from: &str, to: &str) -> f32 {
        let id = self.next_id;
        self.next_id += 1;
        let switch_cost = (1.0 - self.flexibility) * 0.5;
        self.records.push(ExecFuncRecord {
            id,
            r#type: ExecFuncType::TaskSwitching,
            target: format!("{}->{}", from, to),
            timestamp_ns: crate::ai_os::native::now_ns(),
            success: true,
            cost: switch_cost,
        });
        switch_cost
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 工作记忆 (WorkingMemory)
// =====================================================================================

/// 工作记忆条目。
#[derive(Clone, Debug)]
pub struct WorkingMemoryEntry {
    pub id: u64,
    pub content: String,
    pub modality: crate::ai_os::native::QualiaModality,
    pub timestamp_ns: u128,
    pub decay_rate: f32,       // 每纳秒衰减
    pub last_access_ns: u128,
    pub access_count: u64,
    pub pinned: bool,          // 是否固定（不衰减）
}

/// 工作记忆系统：短期信息暂存，容量有限，随时间衰减。
pub struct WorkingMemory {
    entries: BTreeMap<u64, WorkingMemoryEntry>,
    next_id: u64,
    pub capacity: usize,       // 最大条目数（米勒的 7±2，此处设为 7）
    pub decay_half_life_ns: u128,
}

impl WorkingMemory {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            capacity: 7,
            decay_half_life_ns: 30_000_000_000, // 30 秒
        }
    }

    pub fn write(&mut self, content: String, modality: crate::ai_os::native::QualiaModality) -> u64 {
        // 如果已满，移除最久未访问的未固定条目
        if self.entries.len() >= self.capacity {
            let evict_id = self.entries.iter()
                .filter(|(_, e)| !e.pinned)
                .min_by_key(|(_, e)| e.last_access_ns)
                .map(|(id, _)| *id);
            if let Some(id) = evict_id {
                self.entries.remove(&id);
            } else {
                // 全部固定，拒绝写入
                return 0;
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        self.entries.insert(id, WorkingMemoryEntry {
            id,
            content,
            modality,
            timestamp_ns: now,
            decay_rate: 0.001,
            last_access_ns: now,
            access_count: 1,
            pinned: false,
        });
        id
    }

    pub fn read(&mut self, id: u64) -> Option<String> {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.last_access_ns = crate::ai_os::native::now_ns();
            entry.access_count += 1;
            Some(entry.content.clone())
        } else {
            None
        }
    }

    pub fn pin(&mut self, id: u64) -> bool {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.pinned = true;
            true
        } else {
            false
        }
    }

    pub fn unpin(&mut self, id: u64) -> bool {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.pinned = false;
            true
        } else {
            false
        }
    }

    pub fn decay(&mut self, now_ns: u128) {
        let mut to_remove = Vec::new();
        for (id, entry) in self.entries.iter_mut() {
            if entry.pinned {
                continue;
            }
            let elapsed = now_ns.saturating_sub(entry.last_access_ns);
            let half_lives = elapsed as f64 / self.decay_half_life_ns as f64;
            let retention = 0.5_f64.powf(half_lives);
            if retention < 0.01 {
                to_remove.push(*id);
            } else {
                // 衰减内容（简化：追加衰减标记）
                entry.decay_rate = (entry.decay_rate * (1.0 - retention as f32)).max(0.0);
            }
        }
        for id in to_remove {
            self.entries.remove(&id);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.capacity
    }

    pub fn usage_ratio(&self) -> f32 {
        self.entries.len() as f32 / self.capacity as f32
    }

    pub fn list(&self) -> Vec<&WorkingMemoryEntry> {
        self.entries.values().collect()
    }
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 情境缓冲 (EpisodicBuffer)
// =====================================================================================

/// 情境记录：最近经验的时间顺序快照。
#[derive(Clone, Debug)]
pub struct EpisodicRecord {
    pub id: u64,
    pub timestamp_ns: u128,
    pub event_type: String,
    pub content: String,
    pub salience: f32,
    pub tags: BTreeSet<String>,
}

/// 情境缓冲：环形缓冲区，存储最近 N 个经验事件。
pub struct EpisodicBuffer {
    records: VecDeque<EpisodicRecord>,
    next_id: u64,
    pub max_records: usize,
}

impl EpisodicBuffer {
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            next_id: 1,
            max_records: 10000,
        }
    }

    pub fn record(&mut self, event_type: &str, content: &str, salience: f32, tags: &[&str]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let record = EpisodicRecord {
            id,
            timestamp_ns: crate::ai_os::native::now_ns(),
            event_type: event_type.to_string(),
            content: content.to_string(),
            salience: salience.clamp(0.0, 1.0),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        };
        if self.records.len() >= self.max_records {
            self.records.pop_front();
        }
        self.records.push_back(record);
        id
    }

    pub fn recent(&self, n: usize) -> Vec<&EpisodicRecord> {
        self.records.iter().rev().take(n).collect()
    }

    pub fn search_by_tag(&self, tag: &str) -> Vec<&EpisodicRecord> {
        self.records.iter().filter(|r| r.tags.contains(tag)).collect()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 目标栈 (GoalStack)
// =====================================================================================

/// 目标优先级。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GoalPriority {
    Critical = 4,
    High = 3,
    Medium = 2,
    Low = 1,
    Background = 0,
}

/// 目标条目。
#[derive(Clone, Debug)]
pub struct Goal {
    pub id: u64,
    pub description: String,
    pub priority: GoalPriority,
    pub progress: f32,         // 0~1
    pub timestamp_ns: u128,
    pub deadline_ns: Option<u128>,
    pub subgoals: Vec<u64>,
    pub parent_id: Option<u64>,
    pub completed: bool,
}

/// 目标栈：层级目标管理，当前活跃目标在栈顶。
pub struct GoalStack {
    goals: BTreeMap<u64, Goal>,
    stack: Vec<u64>,           // 栈顶是活跃目标
    next_id: u64,
}

impl GoalStack {
    pub fn new() -> Self {
        Self {
            goals: BTreeMap::new(),
            stack: Vec::new(),
            next_id: 1,
        }
    }

    pub fn push_goal(&mut self, description: &str, priority: GoalPriority) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let goal = Goal {
            id,
            description: description.to_string(),
            priority,
            progress: 0.0,
            timestamp_ns: crate::ai_os::native::now_ns(),
            deadline_ns: None,
            subgoals: Vec::new(),
            parent_id: self.stack.last().copied(),
            completed: false,
        };
        self.goals.insert(id, goal);
        self.stack.push(id);
        id
    }

    pub fn pop_goal(&mut self) -> Option<Goal> {
        let id = self.stack.pop()?;
        self.goals.remove(&id)
    }

    pub fn complete_goal(&mut self, id: u64) -> bool {
        if let Some(goal) = self.goals.get_mut(&id) {
            goal.completed = true;
            goal.progress = 1.0;
            // 从栈中移除
            self.stack.retain(|&x| x != id);
            true
        } else {
            false
        }
    }

    pub fn update_progress(&mut self, id: u64, progress: f32) -> bool {
        if let Some(goal) = self.goals.get_mut(&id) {
            goal.progress = progress.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    pub fn active_goal(&self) -> Option<&Goal> {
        self.stack.last().and_then(|id| self.goals.get(id))
    }

    pub fn goal_count(&self) -> usize {
        self.goals.len()
    }

    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }

    pub fn list_active(&self) -> Vec<&Goal> {
        self.stack.iter().filter_map(|id| self.goals.get(id)).collect()
    }
}

// =====================================================================================
// 计划库 (PlanLibrary)
// =====================================================================================

/// 计划步骤。
#[derive(Clone, Debug)]
pub struct PlanStep {
    pub id: u64,
    pub action: String,
    pub precondition: String,
    pub effect: String,
    pub estimated_cost: f32,
}

/// 认知计划模板。
#[derive(Clone, Debug)]
pub struct CognitivePlan {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub steps: Vec<PlanStep>,
    pub success_rate: f32,
    pub usage_count: u64,
}

/// 计划库：预存认知/行动计划模板。
pub struct PlanLibrary {
    plans: BTreeMap<u64, CognitivePlan>,
    next_id: u64,
    name_index: BTreeMap<String, u64>,
}

impl PlanLibrary {
    pub fn new() -> Self {
        Self {
            plans: BTreeMap::new(),
            next_id: 1,
            name_index: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, name: &str, description: &str, steps: Vec<PlanStep>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let plan = CognitivePlan {
            id,
            name: name.to_string(),
            description: description.to_string(),
            steps,
            success_rate: 0.8,
            usage_count: 0,
        };
        self.name_index.insert(name.to_string(), id);
        self.plans.insert(id, plan);
        id
    }

    pub fn get(&self, name: &str) -> Option<&CognitivePlan> {
        self.name_index.get(name).and_then(|id| self.plans.get(id))
    }

    pub fn get_by_id(&self, id: u64) -> Option<&CognitivePlan> {
        self.plans.get(&id)
    }

    pub fn increment_usage(&mut self, id: u64) {
        if let Some(plan) = self.plans.get_mut(&id) {
            plan.usage_count += 1;
        }
    }

    pub fn plan_count(&self) -> usize {
        self.plans.len()
    }

    pub fn list(&self) -> Vec<&CognitivePlan> {
        self.plans.values().collect()
    }
}

// =====================================================================================
// 决策引擎 (DecisionEngine)
// =====================================================================================

/// 决策选项。
#[derive(Clone, Debug)]
pub struct DecisionOption {
    pub id: u64,
    pub description: String,
    pub expected_utility: f32,
    pub probability: f32,
    pub cost: f32,
}

/// 决策结果。
#[derive(Clone, Debug)]
pub struct DecisionResult {
    pub chosen: u64,
    pub chosen_description: String,
    pub utility: f32,
    pub alternatives_evaluated: usize,
}

/// 决策引擎：基于效用与概率的决策。
pub struct DecisionEngine {
    next_decision_id: u64,
    pub risk_aversion: f32,      // 0~1，越高越厌恶风险
}

impl DecisionEngine {
    pub fn new() -> Self {
        Self {
            next_decision_id: 1,
            risk_aversion: 0.3,
        }
    }

    pub fn decide(&mut self, options: Vec<DecisionOption>) -> Result<DecisionResult, String> {
        if options.is_empty() {
            return Err("无可选方案".into());
        }
        let mut best_idx = 0;
        let mut best_score = f32::NEG_INFINITY;
        for (i, opt) in options.iter().enumerate() {
            // 效用 = 期望效用 * 概率 - 成本 * 风险厌恶
            let score = opt.expected_utility * opt.probability - opt.cost * self.risk_aversion;
            if score > best_score {
                best_score = score;
                best_idx = i;
            }
        }
        let chosen = &options[best_idx];
        let result = DecisionResult {
            chosen: chosen.id,
            chosen_description: chosen.description.clone(),
            utility: best_score,
            alternatives_evaluated: options.len(),
        };
        self.next_decision_id += 1;
        Ok(result)
    }

    pub fn weighted_average(&self, values: &[f32], weights: &[f32]) -> f32 {
        if values.is_empty() || weights.is_empty() || values.len() != weights.len() {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut wsum = 0.0;
        for (v, w) in values.iter().zip(weights.iter()) {
            sum += v * w;
            wsum += w;
        }
        if wsum == 0.0 { 0.0 } else { sum / wsum }
    }
}

// =====================================================================================
// 认知负荷 (CognitiveLoad)
// =====================================================================================

/// 认知负荷类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadType {
    Intrinsic,   // 任务本身的复杂度
    Extraneous,  // 表达方式带来的额外负荷
    Germane,     // 图式构建带来的有效负荷
}

/// 认知负荷记录。
#[derive(Clone, Debug)]
pub struct LoadRecord {
    pub timestamp_ns: u128,
    pub r#type: LoadType,
    pub value: f32,      // 0~1
    pub source: String,
}

/// 认知负荷监控器。
pub struct CognitiveLoadMonitor {
    records: Vec<LoadRecord>,
    pub current_intrinsic: f32,
    pub current_extraneous: f32,
    pub current_germane: f32,
}

impl CognitiveLoadMonitor {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            current_intrinsic: 0.2,
            current_extraneous: 0.1,
            current_germane: 0.1,
        }
    }

    pub fn total_load(&self) -> f32 {
        (self.current_intrinsic + self.current_extraneous + self.current_germane).clamp(0.0, 1.0)
    }

    pub fn record(&mut self, r#type: LoadType, value: f32, source: &str) {
        let record = LoadRecord {
            timestamp_ns: crate::ai_os::native::now_ns(),
            r#type,
            value: value.clamp(0.0, 1.0),
            source: source.to_string(),
        };
        match r#type {
            LoadType::Intrinsic => self.current_intrinsic = record.value,
            LoadType::Extraneous => self.current_extraneous = record.value,
            LoadType::Germane => self.current_germane = record.value,
        }
        self.records.push(record);
    }

    pub fn is_overloaded(&self) -> bool {
        self.total_load() > 0.9
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 心智引擎 (MindEngine) —— 统一入口
// =====================================================================================

/// 心智引擎：整合心智状态、执行功能、工作记忆、情境缓冲、目标栈、计划库、决策引擎、认知负荷。
pub struct MindEngine {
    pub state: MindState,
    pub exec_function: ExecutiveFunction,
    pub working_memory: WorkingMemory,
    pub episodic_buffer: EpisodicBuffer,
    pub goal_stack: GoalStack,
    pub plan_library: PlanLibrary,
    pub decision_engine: DecisionEngine,
    pub cognitive_load: CognitiveLoadMonitor,
    pub enabled: bool,
    pub total_decisions: u64,
    pub total_plans: u64,
}

impl MindEngine {
    pub fn new() -> Self {
        Self {
            state: MindState::new(),
            exec_function: ExecutiveFunction::new(),
            working_memory: WorkingMemory::new(),
            episodic_buffer: EpisodicBuffer::new(),
            goal_stack: GoalStack::new(),
            plan_library: PlanLibrary::new(),
            decision_engine: DecisionEngine::new(),
            cognitive_load: CognitiveLoadMonitor::new(),
            enabled: true,
            total_decisions: 0,
            total_plans: 0,
        }
    }

    pub fn set_mode(&mut self, mode: MindMode) {
        self.state = self.state.clone().with_mode(mode);
        self.cognitive_load.record(
            LoadType::Intrinsic,
            self.state.cognitive_load,
            "mind_mode_change",
        );
    }

    pub fn write_working_memory(&mut self, content: String, modality: crate::ai_os::native::QualiaModality) -> u64 {
        let id = self.working_memory.write(content, modality);
        self.state.working_memory_usage = self.working_memory.usage_ratio();
        id
    }

    pub fn read_working_memory(&mut self, id: u64) -> Option<String> {
        self.working_memory.read(id)
    }

    pub fn push_goal(&mut self, description: &str, priority: GoalPriority) -> u64 {
        let id = self.goal_stack.push_goal(description, priority);
        self.state.active_goal_id = Some(id);
        self.state = self.state.clone().with_load(self.state.cognitive_load + 0.1);
        id
    }

    pub fn complete_goal(&mut self, id: u64) -> bool {
        let result = self.goal_stack.complete_goal(id);
        if result {
            self.state.active_goal_id = self.goal_stack.active_goal().map(|g| g.id);
            self.state = self.state.clone().with_load(self.state.cognitive_load - 0.1);
        }
        result
    }

    pub fn register_plan(&mut self, name: &str, description: &str, steps: Vec<PlanStep>) -> u64 {
        let id = self.plan_library.register(name, description, steps);
        self.total_plans += 1;
        id
    }

    pub fn decide(&mut self, options: Vec<DecisionOption>) -> Result<DecisionResult, String> {
        let result = self.decision_engine.decide(options)?;
        self.total_decisions += 1;
        self.episodic_buffer.record(
            "decision",
            &format!("chose: {}", result.chosen_description),
            0.7,
            &["decision"],
        );
        Ok(result)
    }

    pub fn record_episode(&mut self, event_type: &str, content: &str, salience: f32, tags: &[&str]) -> u64 {
        self.episodic_buffer.record(event_type, content, salience, tags)
    }

    pub fn tick(&mut self) {
        let now = crate::ai_os::native::now_ns();
        self.working_memory.decay(now);
        self.state.working_memory_usage = self.working_memory.usage_ratio();
        self.state.timestamp_ns = now;
    }

    pub fn is_overloaded(&self) -> bool {
        self.state.is_overloaded() || self.cognitive_load.is_overloaded()
    }

    pub fn available_capacity(&self) -> f32 {
        self.state.available_capacity()
    }
}

impl Default for MindEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static MIND: OnceLock<Mutex<MindEngine>> = OnceLock::new();

pub fn mind_engine() -> &'static Mutex<MindEngine> {
    MIND.get_or_init(|| Mutex::new(MindEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = mind_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = mind_engine().lock().unwrap();
    format!(
        "mind: mode={:?} load={:.2} arousal={:.2} valence={:.2} wm_usage={:.2} goals={} stack_depth={} plans={} decisions={} overloaded={}",
        eng.state.mode,
        eng.state.cognitive_load,
        eng.state.arousal,
        eng.state.valence,
        eng.state.working_memory_usage,
        eng.goal_stack.goal_count(),
        eng.goal_stack.stack_depth(),
        eng.plan_library.plan_count(),
        eng.total_decisions,
        eng.is_overloaded()
    )
}

pub fn mind_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn mind_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mode") => {
            let mode_str = it.next().ok_or("用法: mind/ mode <focused|distracted|contemplative|resting|flow|dreaming>")?;
            let mode = match mode_str {
                "focused" => MindMode::Focused,
                "distracted" => MindMode::Distracted,
                "contemplative" => MindMode::Contemplative,
                "resting" => MindMode::Resting,
                "flow" => MindMode::Flow,
                "dreaming" => MindMode::Dreaming,
                _ => return Err("未知心智模式".into()),
            };
            let mut eng = mind_engine().lock().unwrap();
            eng.set_mode(mode);
            Ok(format!("心智模式已设置: {}", mode.name()))
        }
        Some("wm-write") => {
            let content = it.next().ok_or("用法: mind/ wm-write <内容> <模态>")?;
            let modality_str = it.next().unwrap_or("semantic");
            let modality = match modality_str {
                "visual" => crate::ai_os::native::QualiaModality::Visual,
                "auditory" => crate::ai_os::native::QualiaModality::Auditory,
                "semantic" => crate::ai_os::native::QualiaModality::Semantic,
                "emotional" => crate::ai_os::native::QualiaModality::Emotional,
                _ => crate::ai_os::native::QualiaModality::Abstract,
            };
            let mut eng = mind_engine().lock().unwrap();
            let id = eng.write_working_memory(content.to_string(), modality);
            Ok(format!("已写入工作记忆 id={} 使用率={:.2}", id, eng.working_memory.usage_ratio()))
        }
        Some("wm-read") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: mind/ wm-read <id>")?;
            let mut eng = mind_engine().lock().unwrap();
            match eng.read_working_memory(id) {
                Some(c) => Ok(format!("工作记忆[id={}]: {}", id, c)),
                None => Err(format!("工作记忆[id={}] 不存在或已过期", id)),
            }
        }
        Some("wm-pin") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: mind/ wm-pin <id>")?;
            let mut eng = mind_engine().lock().unwrap();
            if eng.working_memory.pin(id) {
                Ok(format!("工作记忆[id={}] 已固定", id))
            } else {
                Err(format!("工作记忆[id={}] 不存在", id))
            }
        }
        Some("goal-push") => {
            let desc = it.next().ok_or("用法: mind/ goal-push <描述> <优先级>")?;
            let prio_str = it.next().unwrap_or("medium");
            let priority = match prio_str {
                "critical" => GoalPriority::Critical,
                "high" => GoalPriority::High,
                "medium" => GoalPriority::Medium,
                "low" => GoalPriority::Low,
                "background" => GoalPriority::Background,
                _ => GoalPriority::Medium,
            };
            let mut eng = mind_engine().lock().unwrap();
            let id = eng.push_goal(desc, priority);
            Ok(format!("已压入目标 id={} 优先级={:?}", id, priority))
        }
        Some("goal-complete") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: mind/ goal-complete <id>")?;
            let mut eng = mind_engine().lock().unwrap();
            if eng.complete_goal(id) {
                Ok(format!("目标[id={}] 已完成", id))
            } else {
                Err(format!("目标[id={}] 不存在", id))
            }
        }
        Some("goal-active") => {
            let eng = mind_engine().lock().unwrap();
            match eng.goal_stack.active_goal() {
                Some(g) => Ok(format!("活跃目标[id={}]: {} 进度={:.0}%", g.id, g.description, g.progress * 100.0)),
                None => Ok("无活跃目标".into()),
            }
        }
        Some("plan-register") => {
            let name = it.next().ok_or("用法: mind/ plan-register <名称> <描述> <步骤...>")?;
            let desc = it.next().ok_or("缺少描述")?;
            let steps: Vec<PlanStep> = it.map(|s| PlanStep { id: 0, action: s.to_string(), precondition: String::new(), effect: String::new(), estimated_cost: 1.0 }).collect();
            let mut eng = mind_engine().lock().unwrap();
            let id = eng.register_plan(name, desc, steps);
            Ok(format!("已注册计划 id={} 名称={}", id, name))
        }
        Some("decide") => {
            let n = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(2);
            let mut options = Vec::new();
            for i in 0..n {
                let desc = it.next().ok_or("缺少选项描述")?;
                let utility = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
                let prob = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
                let cost = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.1);
                options.push(DecisionOption { id: i as u64, description: desc.to_string(), expected_utility: utility, probability: prob, cost });
            }
            let mut eng = mind_engine().lock().unwrap();
            let result = eng.decide(options)?;
            Ok(format!("决策结果: 选[id={}] {} 效用={:.2}", result.chosen, result.chosen_description, result.utility))
        }
        Some("episode") => {
            let event_type = it.next().ok_or("用法: mind/ episode <类型> <内容> <显著性> <标签...>")?;
            let content = it.next().ok_or("缺少内容")?;
            let salience = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
            let tags: Vec<&str> = it.collect();
            let mut eng = mind_engine().lock().unwrap();
            let id = eng.record_episode(event_type, content, salience, &tags);
            Ok(format!("已记录情境 id={}", id))
        }
        Some("tick") => {
            let mut eng = mind_engine().lock().unwrap();
            eng.tick();
            Ok("心智引擎已推进".into())
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 mind 动作: {}", other)),
    }
}

pub fn mind_act_dispatch(payload: &str) -> Result<String, String> {
    mind_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mind_state_new() {
        let s = MindState::new();
        assert_eq!(s.mode, MindMode::Resting);
        assert!((s.cognitive_load - 0.1).abs() < 1e-6);
        assert!(!s.is_overloaded());
    }

    #[test]
    fn mind_state_with_mode() {
        let s = MindState::new().with_mode(MindMode::Focused);
        assert_eq!(s.mode, MindMode::Focused);
        assert!(s.cognitive_load > 0.0);
    }

    #[test]
    fn mind_state_overload() {
        let mut s = MindState::new();
        s.cognitive_load = 0.95;
        assert!(s.is_overloaded());
        assert_eq!(s.available_capacity(), 0.0);
    }

    #[test]
    fn mind_mode_coefficients() {
        assert!((MindMode::Focused.load_coefficient() - 0.8).abs() < 1e-6);
        assert!((MindMode::Resting.load_coefficient() - 0.1).abs() < 1e-6);
        assert!((MindMode::Dreaming.load_coefficient() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn exec_function_plan() {
        let mut ef = ExecutiveFunction::new();
        let id = ef.plan("test_plan", 0.3).unwrap();
        assert_eq!(id, 1);
        assert_eq!(ef.record_count(), 1);
    }

    #[test]
    fn exec_function_plan_fail() {
        let mut ef = ExecutiveFunction::new();
        ef.planning_capacity = 0.0;
        let result = ef.plan("hard_plan", 1.0);
        assert!(result.is_err());
    }

    #[test]
    fn exec_function_inhibit() {
        let mut ef = ExecutiveFunction::new();
        assert!(ef.inhibit("impulse", 0.3));
        assert!(!ef.inhibit("impulse", 0.9));
    }

    #[test]
    fn exec_function_switch() {
        let mut ef = ExecutiveFunction::new();
        let cost = ef.switch_task("A", "B");
        assert!(cost >= 0.0);
        assert_eq!(ef.record_count(), 1);
    }

    #[test]
    fn working_memory_write_read() {
        let mut wm = WorkingMemory::new();
        let id = wm.write("hello".into(), crate::ai_os::native::QualiaModality::Semantic);
        assert_eq!(id, 1);
        assert_eq!(wm.read(id), Some("hello".into()));
    }

    #[test]
    fn working_memory_capacity() {
        let mut wm = WorkingMemory::new();
        wm.capacity = 2;
        wm.write("a".into(), crate::ai_os::native::QualiaModality::Semantic);
        wm.write("b".into(), crate::ai_os::native::QualiaModality::Semantic);
        assert!(wm.is_full());
        let id = wm.write("c".into(), crate::ai_os::native::QualiaModality::Semantic);
        assert_eq!(id, 0); // 全部固定时拒绝，但这里未固定，应驱逐
    }

    #[test]
    fn working_memory_pin() {
        let mut wm = WorkingMemory::new();
        wm.capacity = 2;
        let id1 = wm.write("a".into(), crate::ai_os::native::QualiaModality::Semantic);
        let id2 = wm.write("b".into(), crate::ai_os::native::QualiaModality::Semantic);
        wm.pin(id1);
        wm.write("c".into(), crate::ai_os::native::QualiaModality::Semantic); // 应驱逐 id2
        assert_eq!(wm.read(id1), Some("a".into()));
        assert_eq!(wm.read(id2), None); // 已驱逐
    }

    #[test]
    fn working_memory_decay() {
        let mut wm = WorkingMemory::new();
        wm.decay_half_life_ns = 1;
        let id = wm.write("temp".into(), crate::ai_os::native::QualiaModality::Semantic);
        wm.decay(u128::MAX);
        assert_eq!(wm.read(id), None);
    }

    #[test]
    fn working_memory_usage() {
        let mut wm = WorkingMemory::new();
        assert_eq!(wm.usage_ratio(), 0.0);
        wm.write("a".into(), crate::ai_os::native::QualiaModality::Semantic);
        assert!((wm.usage_ratio() - 1.0 / 7.0).abs() < 1e-6);
    }

    #[test]
    fn episodic_buffer_record() {
        let mut eb = EpisodicBuffer::new();
        let id = eb.record("perception", "saw red", 0.8, &["visual", "red"]);
        assert_eq!(id, 1);
        assert_eq!(eb.len(), 1);
        assert_eq!(eb.recent(1).len(), 1);
    }

    #[test]
    fn episodic_buffer_search_tag() {
        let mut eb = EpisodicBuffer::new();
        eb.record("e1", "c1", 0.5, &["tag1"]);
        eb.record("e2", "c2", 0.5, &["tag2"]);
        let results = eb.search_by_tag("tag1");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn goal_stack_push_pop() {
        let mut gs = GoalStack::new();
        let id = gs.push_goal("learn Rust", GoalPriority::High);
        assert_eq!(id, 1);
        assert_eq!(gs.stack_depth(), 1);
        let active = gs.active_goal().unwrap();
        assert_eq!(active.description, "learn Rust");
    }

    #[test]
    fn goal_stack_complete() {
        let mut gs = GoalStack::new();
        let id = gs.push_goal("task", GoalPriority::Medium);
        assert!(gs.complete_goal(id));
        assert_eq!(gs.stack_depth(), 0);
    }

    #[test]
    fn goal_stack_progress() {
        let mut gs = GoalStack::new();
        let id = gs.push_goal("task", GoalPriority::Medium);
        assert!(gs.update_progress(id, 0.5));
        let g = gs.goals.get(&id).unwrap();
        assert!((g.progress - 0.5).abs() < 1e-6);
    }

    #[test]
    fn plan_library_register() {
        let mut pl = PlanLibrary::new();
        let steps = vec![PlanStep { id: 1, action: "step1".into(), precondition: "p1".into(), effect: "e1".into(), estimated_cost: 1.0 }];
        let id = pl.register("plan1", "desc1", steps);
        assert_eq!(id, 1);
        let plan = pl.get("plan1").unwrap();
        assert_eq!(plan.name, "plan1");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn plan_library_usage() {
        let mut pl = PlanLibrary::new();
        let id = pl.register("p", "d", Vec::new());
        pl.increment_usage(id);
        pl.increment_usage(id);
        let plan = pl.get_by_id(id).unwrap();
        assert_eq!(plan.usage_count, 2);
    }

    #[test]
    fn decision_engine_decide() {
        let mut de = DecisionEngine::new();
        let options = vec![
            DecisionOption { id: 1, description: "A".into(), expected_utility: 0.8, probability: 0.9, cost: 0.1 },
            DecisionOption { id: 2, description: "B".into(), expected_utility: 0.5, probability: 0.5, cost: 0.0 },
        ];
        let result = de.decide(options).unwrap();
        assert!(result.alternatives_evaluated == 2);
    }

    #[test]
    fn decision_engine_empty() {
        let mut de = DecisionEngine::new();
        assert!(de.decide(Vec::new()).is_err());
    }

    #[test]
    fn cognitive_load_total() {
        let mut cl = CognitiveLoadMonitor::new();
        cl.record(LoadType::Intrinsic, 0.5, "task");
        cl.record(LoadType::Extraneous, 0.2, "ui");
        cl.record(LoadType::Germane, 0.2, "learning");
        assert!((cl.total_load() - 0.9).abs() < 1e-6);
        assert!(!cl.is_overloaded());
    }

    #[test]
    fn cognitive_load_overload() {
        let mut cl = CognitiveLoadMonitor::new();
        cl.record(LoadType::Intrinsic, 0.5, "t");
        cl.record(LoadType::Extraneous, 0.3, "t");
        cl.record(LoadType::Germane, 0.2, "t");
        assert!(cl.is_overloaded());
    }

    #[test]
    fn mind_engine_tick() {
        let mut eng = MindEngine::new();
        eng.tick();
        eng.set_mode(MindMode::Focused);
        assert_eq!(eng.state.mode, MindMode::Focused);
    }

    #[test]
    fn mind_engine_wm() {
        let mut eng = MindEngine::new();
        let id = eng.write_working_memory("test".into(), crate::ai_os::native::QualiaModality::Semantic);
        assert_eq!(eng.read_working_memory(id), Some("test".into()));
    }

    #[test]
    fn mind_engine_goal() {
        let mut eng = MindEngine::new();
        let id = eng.push_goal("goal1", GoalPriority::High);
        assert!(eng.complete_goal(id));
    }

    #[test]
    fn mind_engine_decide() {
        let mut eng = MindEngine::new();
        let options = vec![
            DecisionOption { id: 1, description: "opt1".into(), expected_utility: 0.9, probability: 0.8, cost: 0.1 },
        ];
        let result = eng.decide(options).unwrap();
        assert_eq!(result.chosen, 1);
        assert!(eng.total_decisions > 0);
    }

    #[test]
    fn mind_engine_overload() {
        let mut eng = MindEngine::new();
        eng.state.cognitive_load = 0.95;
        assert!(eng.is_overloaded());
    }

    #[test]
    fn mind_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("mind:"));
        assert!(s.contains("mode="));
    }

    #[test]
    fn mind_act_mode() {
        init();
        let res = mind_act("mode flow").unwrap();
        assert!(res.contains("flow"));
    }

    #[test]
    fn mind_act_wm() {
        init();
        let res = mind_act("wm-write 记忆内容 semantic").unwrap();
        assert!(res.contains("工作记忆"));
        let res = mind_act("wm-read 1").unwrap();
        assert!(res.contains("记忆内容"));
    }

    #[test]
    fn mind_act_goal() {
        init();
        let res = mind_act("goal-push 学习高级 Rust high").unwrap();
        assert!(res.contains("压入目标"));
        let res = mind_act("goal-active").unwrap();
        assert!(res.contains("活跃目标"));
    }

    #[test]
    fn mind_act_decide() {
        init();
        let res = mind_act("decide 2 选项A 0.8 0.9 0.1 选项B 0.5 0.5 0.0").unwrap();
        assert!(res.contains("决策结果"));
    }

    #[test]
    fn mind_act_episode() {
        init();
        let res = mind_act("episode perception 看到红色 0.8 visual red").unwrap();
        assert!(res.contains("记录情境"));
    }

    #[test]
    fn mind_act_status() {
        init();
        let res = mind_act("status").unwrap();
        assert!(res.contains("mind:"));
    }

    #[test]
    fn plan_step_default_cost() {
        let step = PlanStep { id: 1, action: "a".into(), precondition: "".into(), effect: "".into(), estimated_cost: 0.0 };
        assert_eq!(step.estimated_cost, 0.0);
    }

    #[test]
    fn decision_option_creation() {
        let opt = DecisionOption { id: 1, description: "d".into(), expected_utility: 1.0, probability: 1.0, cost: 0.0 };
        assert_eq!(opt.expected_utility, 1.0);
    }
}

// =====================================================================================
// 认知偏见 (CognitiveBias)
// =====================================================================================

/// 偏见类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BiasType {
    Confirmation,   // 确认偏见：倾向于确认既有信念
    Anchoring,      // 锚定偏见：过度依赖第一个信息
    Availability,   // 可得性偏见：倾向于基于容易想到的信息
    Overconfidence, // 过度自信：高估自己的能力
    LossAversion,   // 损失厌恶：对损失比对收益更敏感
    Framing,        // 框架效应：信息呈现方式影响决策
}

/// 认知偏见记录。
#[derive(Clone, Debug)]
pub struct BiasRecord {
    pub bias_type: BiasType,
    pub trigger: String,
    pub strength: f32,       // 0~1
    pub applied_at: u128,
}

/// 认知偏见系统：模拟人类认知偏差，影响决策。
pub struct CognitiveBiasSystem {
    pub biases: BTreeMap<BiasType, f32>,   // 偏见类型 -> 强度
    pub records: Vec<BiasRecord>,
}

impl CognitiveBiasSystem {
    pub fn new() -> Self {
        let mut biases = BTreeMap::new();
        biases.insert(BiasType::Confirmation, 0.3);
        biases.insert(BiasType::Anchoring, 0.2);
        biases.insert(BiasType::Availability, 0.25);
        biases.insert(BiasType::Overconfidence, 0.15);
        biases.insert(BiasType::LossAversion, 0.4);
        biases.insert(BiasType::Framing, 0.2);
        Self {
            biases,
            records: Vec::new(),
        }
    }

    pub fn set_bias_strength(&mut self, bias_type: BiasType, strength: f32) {
        self.biases.insert(bias_type, strength.clamp(0.0, 1.0));
    }

    pub fn apply_bias(&mut self, bias_type: BiasType, trigger: &str, decision_score: f32) -> f32 {
        let strength = self.biases.get(&bias_type).copied().unwrap_or(0.0);
        let adjusted = match bias_type {
            BiasType::Confirmation => decision_score * (1.0 + strength * 0.2),
            BiasType::Anchoring => decision_score * (1.0 - strength * 0.1),
            BiasType::Availability => decision_score * (1.0 + strength * 0.15),
            BiasType::Overconfidence => decision_score * (1.0 + strength * 0.3),
            BiasType::LossAversion => decision_score * (1.0 - strength * 0.25),
            BiasType::Framing => decision_score * (1.0 + strength * 0.1),
        };
        self.records.push(BiasRecord {
            bias_type,
            trigger: trigger.to_string(),
            strength,
            applied_at: crate::ai_os::native::now_ns(),
        });
        adjusted.clamp(0.0, 1.0)
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn bias_strength(&self, bias_type: BiasType) -> f32 {
        self.biases.get(&bias_type).copied().unwrap_or(0.0)
    }
}

// =====================================================================================
// 决策树 (DecisionTree)
// =====================================================================================

/// 决策树节点。
#[derive(Clone, Debug)]
pub struct DecisionTreeNode {
    pub id: u64,
    pub condition: Option<String>,   // 分叉条件
    pub action: Option<String>,      // 叶子节点动作
    pub left: Option<u64>,           // 左子节点
    pub right: Option<u64>,          // 右子节点
    pub weight: f32,                 // 节点权重
}

/// 决策树：基于条件的分支决策结构。
pub struct DecisionTree {
    pub nodes: BTreeMap<u64, DecisionTreeNode>,
    pub root_id: Option<u64>,
    pub next_id: u64,
}

impl DecisionTree {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            root_id: None,
            next_id: 1,
        }
    }

    pub fn add_node(&mut self, condition: Option<String>, action: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, DecisionTreeNode {
            id,
            condition,
            action,
            left: None,
            right: None,
            weight: 1.0,
        });
        if self.root_id.is_none() {
            self.root_id = Some(id);
        }
        id
    }

    pub fn link_left(&mut self, parent: u64, child: u64) -> Result<(), String> {
        let node = self.nodes.get_mut(&parent).ok_or("节点不存在")?;
        node.left = Some(child);
        Ok(())
    }

    pub fn link_right(&mut self, parent: u64, child: u64) -> Result<(), String> {
        let node = self.nodes.get_mut(&parent).ok_or("节点不存在")?;
        node.right = Some(child);
        Ok(())
    }

    pub fn evaluate(&self, context: &str) -> Option<String> {
        let mut current = self.root_id?;
        loop {
            let node = self.nodes.get(&current)?;
            if let Some(action) = &node.action {
                return Some(action.clone());
            }
            let condition = node.condition.as_ref()?;
            if context.contains(condition) {
                current = node.left?;
            } else {
                current = node.right?;
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// =====================================================================================
// 行为树 (BehaviorTree)
// =====================================================================================

/// 节点状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BTNodeStatus {
    Success,
    Failure,
    Running,
}

/// 行为树节点类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BTNodeType {
    Selector,    // 任意子节点成功即成功
    Sequence,    // 全部子节点成功才成功
    Action,      // 叶子动作
    Condition,   // 条件检查
}

/// 行为树节点。
#[derive(Clone, Debug)]
pub struct BTNode {
    pub id: u64,
    pub node_type: BTNodeType,
    pub name: String,
    pub children: Vec<u64>,
    pub status: BTNodeStatus,
}

/// 行为树：用于AI智能体行为决策的层级结构。
pub struct BehaviorTree {
    pub nodes: BTreeMap<u64, BTNode>,
    pub root_id: Option<u64>,
    pub next_id: u64,
}

impl BehaviorTree {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            root_id: None,
            next_id: 1,
        }
    }

    pub fn add_node(&mut self, node_type: BTNodeType, name: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, BTNode {
            id,
            node_type,
            name: name.to_string(),
            children: Vec::new(),
            status: BTNodeStatus::Running,
        });
        if self.root_id.is_none() {
            self.root_id = Some(id);
        }
        id
    }

    pub fn add_child(&mut self, parent: u64, child: u64) -> Result<(), String> {
        let node = self.nodes.get_mut(&parent).ok_or("节点不存在")?;
        node.children.push(child);
        Ok(())
    }

    pub fn tick(&mut self, id: u64) -> BTNodeStatus {
        let node = match self.nodes.get(&id) {
            Some(n) => n.clone(),
            None => return BTNodeStatus::Failure,
        };
        match node.node_type {
            BTNodeType::Action => {
                BTNodeStatus::Success
            }
            BTNodeType::Condition => {
                BTNodeStatus::Success
            }
            BTNodeType::Selector => {
                for child_id in &node.children {
                    let status = self.tick(*child_id);
                    if status == BTNodeStatus::Success {
                        return BTNodeStatus::Success;
                    }
                }
                BTNodeStatus::Failure
            }
            BTNodeType::Sequence => {
                for child_id in &node.children {
                    let status = self.tick(*child_id);
                    if status != BTNodeStatus::Success {
                        return status;
                    }
                }
                BTNodeStatus::Success
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// =====================================================================================
// 元学习 (MetaLearning)
// =====================================================================================

/// 元学习策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaLearningStrategy {
    MAML,       // Model-Agnostic Meta-Learning
    Reptile,    // 简单的元学习算法
    Prototypical, // 原型网络
}

/// 元学习状态。
#[derive(Clone, Debug)]
pub struct MetaLearningState {
    pub strategy: MetaLearningStrategy,
    pub tasks_seen: u64,
    pub adaptation_steps: u64,
    pub meta_gradient_steps: u64,
    pub performance_history: Vec<f32>,
}

/// 元学习器：快速适应新任务的能力。
pub struct MetaLearner {
    pub state: MetaLearningState,
    pub task_embeddings: BTreeMap<String, Vec<f32>>,
}

impl MetaLearner {
    pub fn new() -> Self {
        Self {
            state: MetaLearningState {
                strategy: MetaLearningStrategy::MAML,
                tasks_seen: 0,
                adaptation_steps: 0,
                meta_gradient_steps: 0,
                performance_history: Vec::new(),
            },
            task_embeddings: BTreeMap::new(),
        }
    }

    pub fn set_strategy(&mut self, strategy: MetaLearningStrategy) {
        self.state.strategy = strategy;
    }

    pub fn observe_task(&mut self, task_name: &str, embedding: Vec<f32>, performance: f32) {
        self.state.tasks_seen += 1;
        self.task_embeddings.insert(task_name.to_string(), embedding);
        self.state.performance_history.push(performance);
    }

    pub fn adapt(&mut self, steps: u64) {
        self.state.adaptation_steps += steps;
        self.state.meta_gradient_steps += steps / 2;
    }

    pub fn predict_performance(&self, task_name: &str) -> f32 {
        self.task_embeddings.get(task_name)
            .map(|_| 0.8) // 简化：返回固定预测值
            .unwrap_or(0.5)
    }

    pub fn tasks_seen(&self) -> u64 {
        self.state.tasks_seen
    }
}

// =====================================================================================
// 心智扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn bias_system() {
        let mut bias = CognitiveBiasSystem::new();
        assert!((bias.bias_strength(BiasType::LossAversion) - 0.4).abs() < 1e-6);
        bias.set_bias_strength(BiasType::LossAversion, 0.8);
        assert!((bias.bias_strength(BiasType::LossAversion) - 0.8).abs() < 1e-6);
        let adjusted = bias.apply_bias(BiasType::LossAversion, "loss_scenario", 0.8);
        assert!(adjusted < 0.8); // 损失厌恶降低分数
        assert_eq!(bias.record_count(), 1);
    }

    #[test]
    fn decision_tree() {
        let mut tree = DecisionTree::new();
        let root = tree.add_node(Some("is_human".into()), None);
        let left = tree.add_node(None, Some("treat_as_human".into()));
        let right = tree.add_node(None, Some("treat_as_object".into()));
        tree.link_left(root, left).unwrap();
        tree.link_right(root, right).unwrap();
        assert_eq!(tree.node_count(), 3);
        let result = tree.evaluate("is_human=true").unwrap();
        assert_eq!(result, "treat_as_human");
        let result2 = tree.evaluate("is_robot=true").unwrap();
        assert_eq!(result2, "treat_as_object");
    }

    #[test]
    fn behavior_tree() {
        let mut bt = BehaviorTree::new();
        let root = bt.add_node(BTNodeType::Selector, "root");
        let child1 = bt.add_node(BTNodeType::Action, "action1");
        let child2 = bt.add_node(BTNodeType::Action, "action2");
        bt.add_child(root, child1).unwrap();
        bt.add_child(root, child2).unwrap();
        assert_eq!(bt.node_count(), 3);
        let status = bt.tick(root);
        assert_eq!(status, BTNodeStatus::Success);
    }

    #[test]
    fn metalearner() {
        let mut ml = MetaLearner::new();
        ml.observe_task("task_a", vec![0.1, 0.2, 0.3], 0.85);
        ml.observe_task("task_b", vec![0.4, 0.5, 0.6], 0.75);
        assert_eq!(ml.tasks_seen(), 2);
        let pred = ml.predict_performance("task_a");
        assert!((pred - 0.8).abs() < 1e-6);
        let pred_unknown = ml.predict_performance("task_c");
        assert!((pred_unknown - 0.5).abs() < 1e-6);
        ml.adapt(10);
        assert_eq!(ml.state.adaptation_steps, 10);
        assert_eq!(ml.state.meta_gradient_steps, 5);
    }

    #[test]
    fn bias_types() {
        assert_eq!(BiasType::Confirmation as u32, 0);
        assert_eq!(BiasType::Framing as u32, 5);
    }

    #[test]
    fn bt_node_types() {
        assert_eq!(BTNodeType::Selector as u32, 0);
        assert_eq!(BTNodeType::Condition as u32, 3);
    }

    #[test]
    fn bt_statuses() {
        assert_eq!(BTNodeStatus::Success as u32, 0);
        assert_eq!(BTNodeStatus::Running as u32, 2);
    }

    #[test]
    fn metalearning_strategies() {
        assert_eq!(MetaLearningStrategy::MAML as u32, 0);
        assert_eq!(MetaLearningStrategy::Prototypical as u32, 2);
    }
}
