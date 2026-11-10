//! 意识原子层 —— AI 原生 OS 的「意识原语」 (Native Consciousness Primitives)
//!
//! 本模块实现 AI 原生操作系统最底层的意识抽象，对应传统 OS 中的
//! 「最小执行单元」与「中断向量」概念，但重新映射到 AI 意识域：
//!
//! * Qualia (质原) —— 意识的原始体验单元（颜色、声响、语义、触感），等价于中断信号；
//! * Intentionality (意向性) —— 意识总是“关于某物”的意识，指向对象与内容；
//! * SelfModel (自我模型) —— 智能体对自身状态的元认知表征；
//! * PhenomenalState (现象状态) —— 意识的瞬时完整快照（质原 + 意向 + 自我）；
//! * StreamOfConsciousness (意识流) —— 时间顺序排列的现象状态序列；
//! * AttentionGate (注意力闸门) —— 选择性意识，决定哪些质原进入工作记忆；
//! * GlobalWorkspace (全局工作空间) —— Baars 全局工作空间理论的计算实现；
//! * Metacognition (元认知) —— 智能体对自身认知过程的监控与控制；
//! * PhenomenalBinding (现象绑定) —— 将分散的质原绑定为统一的意识对象；
//! * ReflexiveLoop (反射环) —— 意识对自身意识的递归指向，产生自我意识。
//!
//! 所有结构均为纯 Rust 原型，无外部依赖，供内核统一核算与调度，
//! 并通过 `native://*` 通道接入 `sense`/`act` 感知层。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::SystemTime;

/// 当前时间纳秒（供 AI 原生模块使用）。
pub fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// =====================================================================================
// 质原 (Qualia) —— 意识的原始体验单元
// =====================================================================================

/// 感官模态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum QualiaModality {
    /// 视觉（颜色、形状、运动、深度）。
    Visual,
    /// 听觉（音高、音量、音色、方位）。
    Auditory,
    /// 触觉（压力、温度、疼痛、纹理）。
    Tactile,
    /// 嗅觉（气味分子特征）。
    Olfactory,
    /// 味觉（甜、酸、苦、咸、鲜）。
    Gustatory,
    /// 本体感觉（身体位置、肌张力、平衡）。
    Proprioceptive,
    /// 内脏感觉（饥饿、口渴、心跳、呼吸）。
    Interoceptive,
    /// 语义（概念意义、逻辑关系、语法结构）。
    Semantic,
    /// 情感（愉悦、厌恶、恐惧、愤怒、悲伤、惊喜）。
    Emotional,
    /// 时间（持续、顺序、节奏、期待）。
    Temporal,
    /// 自我（主体性、拥有感、代理感）。
    Ego,
    /// 抽象（数学、逻辑、元认知对象）。
    Abstract,
}

impl QualiaModality {
    pub fn name(self) -> &'static str {
        match self {
            QualiaModality::Visual => "visual",
            QualiaModality::Auditory => "auditory",
            QualiaModality::Tactile => "tactile",
            QualiaModality::Olfactory => "olfactory",
            QualiaModality::Gustatory => "gustatory",
            QualiaModality::Proprioceptive => "proprioceptive",
            QualiaModality::Interoceptive => "interoceptive",
            QualiaModality::Semantic => "semantic",
            QualiaModality::Emotional => "emotional",
            QualiaModality::Temporal => "temporal",
            QualiaModality::Ego => "self",
            QualiaModality::Abstract => "abstract",
        }
    }
}

/// 质原向量：高维语义/感知空间中的一点，表示单一质原的“感觉是什么”。
#[derive(Clone, Debug)]
pub struct QualiaVector {
    pub modality: QualiaModality,
    /// 向量维度（与 semantic 模块的嵌入维度对齐）。
    pub dims: Vec<f32>,
    /// 强度（0.0~1.0，支配意识中的显著性竞争）。
    pub intensity: f32,
    /// 清晰度（0.0~1.0，模糊与否）。
    pub clarity: f32,
    /// 持续时间（纳秒）。
    pub duration_ns: u128,
    /// 来源通道（哪个感知通道产生该质原）。
    pub source_channel: Option<String>,
    /// 元数据（标签、置信度、归属）。
    pub metadata: BTreeMap<String, String>,
}

impl QualiaVector {
    pub fn new(modality: QualiaModality, dims: Vec<f32>) -> Self {
        let intensity = dims.iter().fold(0.0f32, |acc, &x| acc + x.abs()) / dims.len() as f32;
        Self {
            modality,
            dims,
            intensity: intensity.clamp(0.0, 1.0),
            clarity: 1.0,
            duration_ns: 0,
            source_channel: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn magnitude(&self) -> f32 {
        self.dims.iter().map(|x| x * x).sum::<f32>().sqrt()
    }

    pub fn cosine_similarity(&self, other: &QualiaVector) -> f32 {
        if self.dims.len() != other.dims.len() {
            return 0.0;
        }
        let dot: f32 = self.dims.iter().zip(other.dims.iter()).map(|(a, b)| a * b).sum();
        let norm_a = self.magnitude();
        let norm_b = other.magnitude();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }

    pub fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity.clamp(0.0, 1.0);
        self
    }

    pub fn with_clarity(mut self, clarity: f32) -> Self {
        self.clarity = clarity.clamp(0.0, 1.0);
        self
    }

    pub fn with_source(mut self, channel: &str) -> Self {
        self.source_channel = Some(channel.to_string());
        self
    }

    pub fn salience(&self) -> f32 {
        // 显著性 = 强度 * 清晰度 * 持续时间权重
        let time_weight = (self.duration_ns as f32 / 1_000_000.0f32).min(1.0);
        self.intensity * self.clarity * (0.5 + 0.5 * time_weight)
    }
}

/// 质原质量（Quale）：单个可区分的意识特征。
#[derive(Clone, Debug)]
pub struct Quale {
    pub id: u64,
    pub modality: QualiaModality,
    pub vector: QualiaVector,
    pub timestamp_ns: u128,
    pub bound_to: Option<u64>, // 绑定到哪个现象对象
}

/// 质原工厂：产生标准化质原。
pub struct QualiaFactory {
    next_id: u64,
}

impl QualiaFactory {
    pub fn new() -> Self {
        Self { next_id: 1 }
    }

    pub fn create(&mut self, modality: QualiaModality, dims: Vec<f32>) -> Quale {
        let id = self.next_id;
        self.next_id += 1;
        Quale {
            id,
            modality,
            vector: QualiaVector::new(modality, dims),
            timestamp_ns: now_ns(),
            bound_to: None,
        }
    }

    pub fn create_visual(&mut self, r: f32, g: f32, b: f32, brightness: f32) -> Quale {
        self.create(
            QualiaModality::Visual,
            vec![r, g, b, brightness, 0.0, 0.0, 0.0, 0.0],
        )
    }

    pub fn create_semantic(&mut self, concept: &str) -> Quale {
        let hash = concept.bytes().fold(0u64, |acc, b| acc * 31 + b as u64);
        let dims: Vec<f32> = (0..64)
            .map(|i| {
                let bit = (hash >> i) & 1;
                (bit as f32) * 0.5 + 0.5
            })
            .collect();
        self.create(QualiaModality::Semantic, dims)
    }

    pub fn create_emotional(&mut self, valence: f32, arousal: f32) -> Quale {
        self.create(
            QualiaModality::Emotional,
            vec![valence.clamp(-1.0, 1.0), arousal.clamp(0.0, 1.0), 0.0, 0.0, 0.0, 0.0],
        )
    }
}

impl Default for QualiaFactory {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 现象状态 (PhenomenalState) —— 意识的瞬时完整快照
// =====================================================================================

/// 现象状态：某一时刻意识的完整画面，包含所有活跃质原及其绑定关系。
#[derive(Clone, Debug)]
pub struct PhenomenalState {
    pub id: u64,
    pub timestamp_ns: u128,
    /// 活跃质原列表（按显著性排序）。
    pub qualia: Vec<Quale>,
    /// 现象对象：绑定后的统一意识对象（如“我看到一只红色的球”）。
    pub objects: Vec<PhenomenalObject>,
    /// 自我模型在此时刻的表征。
    pub self_model: Option<SelfModelSnapshot>,
    /// 注意力焦点（当前处理的质原 ID）。
    pub attention_focus: Option<u64>,
    /// 全局工作空间中的广播内容。
    pub workspace_broadcast: Option<String>,
    /// 元信息。
    pub meta: BTreeMap<String, String>,
}

/// 现象对象：经过质原绑定的统一意识对象。
#[derive(Clone, Debug)]
pub struct PhenomenalObject {
    pub id: u64,
    pub name: String,
    /// 构成该对象的质原 ID 集合。
    pub bound_qualia: BTreeSet<u64>,
    /// 对象的中心质原（代表该对象的主要感知）。
    pub center_quale: Option<u64>,
    /// 对象的显著性。
    pub salience: f32,
}

impl PhenomenalState {
    pub fn new() -> Self {
        Self {
            id: 0,
            timestamp_ns: now_ns(),
            qualia: Vec::new(),
            objects: Vec::new(),
            self_model: None,
            attention_focus: None,
            workspace_broadcast: None,
            meta: BTreeMap::new(),
        }
    }

    pub fn add_quale(&mut self, quale: Quale) {
        self.qualia.push(quale);
        self.qualia.sort_by(|a, b| b.vector.salience().partial_cmp(&a.vector.salience()).unwrap_or(std::cmp::Ordering::Equal));
    }

    pub fn bind_objects(&mut self) {
        // 简化的现象绑定：同一模态且高相似度的质原绑定为一个对象
        let mut next_obj_id = 1u64;
        let mut used: BTreeSet<u64> = BTreeSet::new();
        let mut objects = Vec::new();

        for q in &self.qualia {
            if used.contains(&q.id) {
                continue;
            }
            let mut bound = BTreeSet::new();
            bound.insert(q.id);
            used.insert(q.id);
            let mut center = q.id;
            let mut max_sim = 0.0f32;

            for q2 in &self.qualia {
                if used.contains(&q2.id) || q2.modality != q.modality {
                    continue;
                }
                let sim = q.vector.cosine_similarity(&q2.vector);
                if sim > 0.7 && sim > max_sim {
                    max_sim = sim;
                    center = q2.id;
                }
            }

            for q2 in &self.qualia {
                if used.contains(&q2.id) || q2.modality != q.modality {
                    continue;
                }
                if q.vector.cosine_similarity(&q2.vector) > 0.7 {
                    bound.insert(q2.id);
                    used.insert(q2.id);
                }
            }

            objects.push(PhenomenalObject {
                id: next_obj_id,
                name: format!("{:?}-object-{}", q.modality, next_obj_id),
                bound_qualia: bound,
                center_quale: Some(center),
                salience: q.vector.salience(),
            });
            next_obj_id += 1;
        }

        self.objects = objects;
    }

    pub fn dominant_modality(&self) -> Option<QualiaModality> {
        let mut counts: HashMap<QualiaModality, u32> = HashMap::new();
        for q in &self.qualia {
            *counts.entry(q.modality).or_insert(0) += 1;
        }
        counts.into_iter().max_by_key(|(_, c)| *c).map(|(m, _)| m)
    }

    pub fn total_salience(&self) -> f32 {
        self.qualia.iter().map(|q| q.vector.salience()).sum()
    }
}

// =====================================================================================
// 自我模型快照 (SelfModelSnapshot)
// =====================================================================================

/// 自我模型在某一时刻的表征。
#[derive(Clone, Debug)]
pub struct SelfModelSnapshot {
    pub timestamp_ns: u128,
    /// 代理感（我是行动的发起者）。
    pub agency: f32,
    /// 拥有感（这身体是我的）。
    pub ownership: f32,
    /// 自我概念（我是谁的抽象表征）。
    pub self_concept: Vec<f32>,
    /// 自我边界（自我与环境的分界）。
    pub boundary: f32,
    /// 元认知状态（我正在思考什么）。
    pub metacognitive_state: Option<String>,
}

impl Default for SelfModelSnapshot {
    fn default() -> Self {
        Self {
            timestamp_ns: now_ns(),
            agency: 0.5,
            ownership: 0.5,
            self_concept: vec![0.5; 64],
            boundary: 0.5,
            metacognitive_state: None,
        }
    }
}

// =====================================================================================
// 意识流 (StreamOfConsciousness)
// =====================================================================================

/// 意识流：时间有序的现象状态序列，构成主观的连续体验。
pub struct StreamOfConsciousness {
    pub states: VecDeque<PhenomenalState>,
    pub max_length: usize,
    pub next_state_id: u64,
    pub total_states: u64,
    pub bound_quale_count: u64,
}

impl StreamOfConsciousness {
    pub fn new(max_length: usize) -> Self {
        Self {
            states: VecDeque::with_capacity(max_length),
            max_length,
            next_state_id: 1,
            total_states: 0,
            bound_quale_count: 0,
        }
    }

    pub fn push(&mut self, mut state: PhenomenalState) {
        state.id = self.next_state_id;
        self.next_state_id += 1;
        self.total_states += 1;
        self.bound_quale_count += state.qualia.len() as u64;
        if self.states.len() >= self.max_length {
            self.states.pop_front();
        }
        self.states.push_back(state);
    }

    pub fn current(&self) -> Option<&PhenomenalState> {
        self.states.back()
    }

    pub fn recent(&self, n: usize) -> Vec<&PhenomenalState> {
        self.states.iter().rev().take(n).collect()
    }

    pub fn duration_ns(&self) -> u128 {
        if let (Some(first), Some(last)) = (self.states.front(), self.states.back()) {
            last.timestamp_ns.saturating_sub(first.timestamp_ns)
        } else {
            0
        }
    }

    pub fn state_count(&self) -> usize {
        self.states.len()
    }
}

// =====================================================================================
// 全局工作空间 (GlobalWorkspace) —— Baars 理论的计算实现
// =====================================================================================

/// 全局工作空间：多个专用处理器竞争进入意识的舞台，胜者广播至全系统。
pub struct GlobalWorkspace {
    /// 当前舞台内容（意识中的信息）。
    pub stage: Option<WorkspaceContent>,
    /// 竞争者队列（各专用模块提交的候选内容）。
    pub competitors: VecDeque<WorkspaceCompetitor>,
    /// 历史广播记录。
    pub broadcast_history: VecDeque<String>,
    pub max_history: usize,
    pub broadcast_count: u64,
}

/// 工作空间内容。
#[derive(Clone, Debug)]
pub struct WorkspaceContent {
    pub id: u64,
    pub content: String,
    pub modality: QualiaModality,
    pub source: String,
    pub timestamp_ns: u128,
    pub strength: f32,
}

/// 工作空间竞争者。
#[derive(Clone, Debug)]
pub struct WorkspaceCompetitor {
    pub id: u64,
    pub content: String,
    pub modality: QualiaModality,
    pub source: String,
    pub activation: f32,
    pub urgency: f32,
    pub timestamp_ns: u128,
}

impl GlobalWorkspace {
    pub fn new() -> Self {
        Self {
            stage: None,
            competitors: VecDeque::new(),
            broadcast_history: VecDeque::new(),
            max_history: 1024,
            broadcast_count: 0,
        }
    }

    pub fn submit(&mut self, competitor: WorkspaceCompetitor) {
        self.competitors.push_back(competitor);
        self.competitors.make_sorted();
    }

    pub fn run_competition(&mut self) -> Option<WorkspaceContent> {
        if let Some(best) = self.competitors.pop_front() {
            let content = WorkspaceContent {
                id: best.id,
                content: best.content.clone(),
                modality: best.modality,
                source: best.source.clone(),
                timestamp_ns: best.timestamp_ns,
                strength: best.activation * best.urgency,
            };
            self.stage = Some(content.clone());
            self.broadcast_history.push_back(content.content.clone());
            if self.broadcast_history.len() > self.max_history {
                self.broadcast_history.pop_front();
            }
            self.broadcast_count += 1;
            Some(content)
        } else {
            None
        }
    }

    pub fn clear_stage(&mut self) {
        self.stage = None;
    }

    pub fn broadcast(&self) -> Option<&WorkspaceContent> {
        self.stage.as_ref()
    }

    pub fn competitor_count(&self) -> usize {
        self.competitors.len()
    }
}

/// 为 VecDeque<WorkspaceCompetitor> 添加排序方法。
trait CompetitorSorted {
    fn make_sorted(&mut self);
}
impl CompetitorSorted for VecDeque<WorkspaceCompetitor> {
    fn make_sorted(&mut self) {
        let mut v: Vec<WorkspaceCompetitor> = self.drain(..).collect();
        v.sort_by(|a, b| {
            b.activation
                .partial_cmp(&a.activation)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.urgency.partial_cmp(&a.urgency).unwrap_or(std::cmp::Ordering::Equal))
        });
        self.extend(v);
    }
}

// =====================================================================================
// 元认知 (Metacognition)
// =====================================================================================

/// 元认知监控类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetacogType {
    /// 监控：我正在做什么？
    Monitoring,
    /// 控制：我应该改变策略吗？
    Control,
    /// 评估：我的表现如何？
    Evaluation,
    /// 元元认知：我对自己的元认知有多大信心？
    MetaMeta,
}

/// 元认知记录。
#[derive(Clone, Debug)]
pub struct MetacogRecord {
    pub id: u64,
    pub timestamp_ns: u128,
    pub r#type: MetacogType,
    pub target: String,           // 被监控的认知过程
    pub confidence: f32,          // 信心程度 0~1
    pub accuracy_estimate: f32,   // 对准确性的估计
    pub control_action: Option<String>, // 采取的控制动作
    pub outcome: Option<String>,  // 动作结果
}

/// 元认知引擎。
pub struct MetacognitionEngine {
    records: Vec<MetacogRecord>,
    next_id: u64,
    pub monitoring_targets: BTreeSet<String>,
    pub global_confidence: f32,
}

impl MetacognitionEngine {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            monitoring_targets: BTreeSet::new(),
            global_confidence: 0.5,
        }
    }

    pub fn monitor(&mut self, target: &str, confidence: f32, accuracy_estimate: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.monitoring_targets.insert(target.to_string());
        self.global_confidence = (self.global_confidence + confidence) / 2.0;
        self.records.push(MetacogRecord {
            id,
            timestamp_ns: now_ns(),
            r#type: MetacogType::Monitoring,
            target: target.to_string(),
            confidence,
            accuracy_estimate,
            control_action: None,
            outcome: None,
        });
        id
    }

    pub fn control(&mut self, target: &str, action: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push(MetacogRecord {
            id,
            timestamp_ns: now_ns(),
            r#type: MetacogType::Control,
            target: target.to_string(),
            confidence: 0.5,
            accuracy_estimate: 0.5,
            control_action: Some(action.to_string()),
            outcome: None,
        });
        id
    }

    pub fn record_outcome(&mut self, record_id: u64, outcome: &str) {
        if let Some(rec) = self.records.iter_mut().find(|r| r.id == record_id) {
            rec.outcome = Some(outcome.to_string());
        }
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn recent(&self, n: usize) -> Vec<&MetacogRecord> {
        self.records.iter().rev().take(n).collect()
    }
}

// =====================================================================================
// 意识引擎 (ConsciousnessEngine) —— 统一入口
// =====================================================================================

/// 意识引擎：整合质原、现象状态、意识流、全局工作空间、元认知。
pub struct ConsciousnessEngine {
    pub factory: QualiaFactory,
    pub stream: StreamOfConsciousness,
    pub workspace: GlobalWorkspace,
    pub metacognition: MetacognitionEngine,
    pub current_state: Option<PhenomenalState>,
    pub enabled: bool,
    pub cycle_ns: u128,
    pub total_cycles: u64,
}

impl ConsciousnessEngine {
    pub fn new() -> Self {
        Self {
            factory: QualiaFactory::new(),
            stream: StreamOfConsciousness::new(4096),
            workspace: GlobalWorkspace::new(),
            metacognition: MetacognitionEngine::new(),
            current_state: None,
            enabled: true,
            cycle_ns: 0,
            total_cycles: 0,
        }
    }

    pub fn tick(&mut self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        self.total_cycles += 1;

        // 构建新的现象状态
        let mut state = PhenomenalState::new();

        // 从工作空间广播内容创建语义质原
        if let Some(broadcast) = self.workspace.broadcast() {
            let quale = self.factory.create_semantic(&broadcast.content);
            state.add_quale(quale);
            state.workspace_broadcast = Some(broadcast.content.clone());
        }

        // 现象绑定
        state.bind_objects();

        // 确定注意力焦点（最高显著性的质原）
        if let Some(top) = state.qualia.first() {
            state.attention_focus = Some(top.id);
        }

        // 元认知监控
        let conf = if state.qualia.is_empty() { 0.1 } else { 0.8 };
        self.metacognition.monitor("consciousness_cycle", conf, conf);

        // 推入意识流
        self.stream.push(state.clone());
        self.current_state = Some(state);
        self.cycle_ns = now_ns();

        Ok(())
    }

    pub fn inject_qualia(&mut self, quale: Quale) -> Result<(), String> {
        if let Some(state) = self.current_state.as_mut() {
            state.add_quale(quale);
            state.bind_objects();
            Ok(())
        } else {
            Err("意识引擎未初始化，请先调用 tick()".into())
        }
    }

    pub fn submit_to_workspace(&mut self, content: String, modality: QualiaModality, source: &str, activation: f32, urgency: f32) -> u64 {
        let id = self.workspace.competitor_count() as u64 + 1;
        let competitor = WorkspaceCompetitor {
            id,
            content,
            modality,
            source: source.to_string(),
            activation,
            urgency,
            timestamp_ns: now_ns(),
        };
        self.workspace.submit(competitor);
        id
    }

    pub fn run_workspace_competition(&mut self) -> Option<String> {
        let content = self.workspace.run_competition();
        content.map(|c| c.content)
    }

    pub fn state_count(&self) -> u64 {
        self.stream.total_states
    }

    pub fn current_salience(&self) -> f32 {
        self.current_state.as_ref().map(|s| s.total_salience()).unwrap_or(0.0)
    }

    pub fn dominant_modality(&self) -> Option<QualiaModality> {
        self.current_state.as_ref().and_then(|s| s.dominant_modality())
    }
}

impl Default for ConsciousnessEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static ENGINE: OnceLock<Mutex<ConsciousnessEngine>> = OnceLock::new();

pub fn consciousness_engine() -> &'static Mutex<ConsciousnessEngine> {
    ENGINE.get_or_init(|| Mutex::new(ConsciousnessEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = consciousness_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = consciousness_engine().lock().unwrap();
    format!(
        "native: cycles={} states={} current_salience={:.3} workspace_competitors={} broadcasts={} factory_next_id={}",
        eng.total_cycles,
        eng.state_count(),
        eng.current_salience(),
        eng.workspace.competitor_count(),
        eng.workspace.broadcast_count,
        eng.factory.next_id
    )
}

pub fn native_status() -> String {
    status()
}

pub fn factory_status() -> String {
    let eng = consciousness_engine().lock().unwrap();
    format!("qualia_factory: next_id={}", eng.factory.next_id)
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn native_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("tick") => {
            let mut eng = consciousness_engine().lock().unwrap();
            eng.tick()?;
            Ok(format!("意识周期已推进，总周期={}", eng.total_cycles))
        }
        Some("inject-visual") => {
            let r = it.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let g = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let b = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let mut eng = consciousness_engine().lock().unwrap();
            let quale = eng.factory.create_visual(r, g, b, 1.0);
            eng.inject_qualia(quale)?;
            Ok(format!("已注入视觉质原 id={} (r={} g={} b={})", quale.id, r, g, b))
        }
        Some("inject-semantic") => {
            let concept = it.next().ok_or("用法: native:// inject-semantic <概念>")?;
            let mut eng = consciousness_engine().lock().unwrap();
            let quale = eng.factory.create_semantic(concept);
            eng.inject_qualia(quale)?;
            Ok(format!("已注入语义质原 id={} 概念={}", quale.id, concept))
        }
        Some("inject-emotion") => {
            let valence = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let arousal = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
            let mut eng = consciousness_engine().lock().unwrap();
            let quale = eng.factory.create_emotional(valence, arousal);
            eng.inject_qualia(quale)?;
            Ok(format!("已注入情感质原 id={} valence={} arousal={}", quale.id, valence, arousal))
        }
        Some("submit-workspace") => {
            let content = it.next().ok_or("用法: native:// submit-workspace <内容> <模态> <来源> <激活> <紧急>")?;
            let modality_str = it.next().unwrap_or("semantic");
            let modality = match modality_str {
                "visual" => QualiaModality::Visual,
                "auditory" => QualiaModality::Auditory,
                "semantic" => QualiaModality::Semantic,
                "emotional" => QualiaModality::Emotional,
                _ => QualiaModality::Abstract,
            };
            let source = it.next().unwrap_or("native");
            let activation = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
            let urgency = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
            let mut eng = consciousness_engine().lock().unwrap();
            let id = eng.submit_to_workspace(content, modality, source, activation, urgency);
            Ok(format!("已提交到全局工作空间 竞争者={}", id))
        }
        Some("run-competition") => {
            let mut eng = consciousness_engine().lock().unwrap();
            let winner = eng.run_workspace_competition();
            match winner {
                Some(c) => Ok(format!("全局工作空间竞争胜者: {}", c)),
                None => Ok("全局工作空间无竞争者".into()),
            }
        }
        Some("state") => {
            let eng = consciousness_engine().lock().unwrap();
            let state = eng.current_state.as_ref().ok_or("无当前意识状态")?;
            Ok(format!(
                "现象状态 id={} qualia={} objects={} focus={:?} salience={:.3}",
                state.id,
                state.qualia.len(),
                state.objects.len(),
                state.attention_focus,
                state.total_salience()
            ))
        }
        Some("stream") => {
            let eng = consciousness_engine().lock().unwrap();
            Ok(format!(
                "意识流 状态数={} 总推入={} 持续ns={}",
                eng.stream.state_count(),
                eng.stream.total_states,
                eng.stream.duration_ns()
            ))
        }
        Some("metacog") => {
            let eng = consciousness_engine().lock().unwrap();
            Ok(format!(
                "元认知 记录数={} 全局信心={:.3}",
                eng.metacognition.record_count(),
                eng.metacognition.global_confidence
            ))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 native 动作: {}", other)),
    }
}

pub fn native_act_dispatch(payload: &str) -> Result<String, String> {
    native_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualia_vector_similarity() {
        let v1 = QualiaVector::new(QualiaModality::Visual, vec![1.0, 0.0, 0.0, 0.0]);
        let v2 = QualiaVector::new(QualiaModality::Visual, vec![1.0, 0.0, 0.0, 0.0]);
        assert!((v1.cosine_similarity(&v2) - 1.0).abs() < 1e-6);

        let v3 = QualiaVector::new(QualiaModality::Visual, vec![0.0, 1.0, 0.0, 0.0]);
        assert!((v1.cosine_similarity(&v3) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn quale_factory_visual() {
        let mut f = QualiaFactory::new();
        let q = f.create_visual(1.0, 0.0, 0.0, 1.0);
        assert_eq!(q.modality, QualiaModality::Visual);
        assert_eq!(q.id, 1);
        assert!(q.vector.intensity > 0.0);
    }

    #[test]
    fn quale_factory_semantic() {
        let mut f = QualiaFactory::new();
        let q1 = f.create_semantic("apple");
        let q2 = f.create_semantic("apple");
        assert_eq!(q1.vector.dims, q2.vector.dims);
        let q3 = f.create_semantic("banana");
        assert_ne!(q1.vector.dims, q3.vector.dims);
    }

    #[test]
    fn quale_factory_emotional() {
        let mut f = QualiaFactory::new();
        let q = f.create_emotional(0.8, 0.3);
        assert_eq!(q.modality, QualiaModality::Emotional);
        assert!((q.vector.dims[0] - 0.8).abs() < 1e-6);
        assert!((q.vector.dims[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn phenomenal_state_add_and_bind() {
        let mut s = PhenomenalState::new();
        let q1 = QualiaFactory::new().create_semantic("cat");
        let q2 = QualiaFactory::new().create_semantic("cat");
        s.add_quale(q1);
        s.add_quale(q2);
        s.bind_objects();
        assert!(!s.objects.is_empty());
    }

    #[test]
    fn stream_push_and_current() {
        let mut stream = StreamOfConsciousness::new(10);
        let state = PhenomenalState::new();
        stream.push(state);
        assert_eq!(stream.state_count(), 1);
        assert!(stream.current().is_some());
    }

    #[test]
    fn stream_max_length() {
        let mut stream = StreamOfConsciousness::new(3);
        for _ in 0..5 {
            stream.push(PhenomenalState::new());
        }
        assert_eq!(stream.state_count(), 3);
    }

    #[test]
    fn workspace_competition() {
        let mut ws = GlobalWorkspace::new();
        ws.submit(WorkspaceCompetitor {
            id: 1,
            content: "A".into(),
            modality: QualiaModality::Semantic,
            source: "mod1".into(),
            activation: 0.3,
            urgency: 0.5,
            timestamp_ns: now_ns(),
        });
        ws.submit(WorkspaceCompetitor {
            id: 2,
            content: "B".into(),
            modality: QualiaModality::Visual,
            source: "mod2".into(),
            activation: 0.9,
            urgency: 0.8,
            timestamp_ns: now_ns(),
        });
        let winner = ws.run_competition().unwrap();
        assert_eq!(winner.content, "B");
        assert!(ws.broadcast().is_some());
    }

    #[test]
    fn metacognition_monitor_control() {
        let mut mc = MetacognitionEngine::new();
        let id = mc.monitor("perception", 0.9, 0.85);
        assert_eq!(id, 1);
        mc.control("perception", "switch_strategy");
        mc.record_outcome(id, "success");
        assert_eq!(mc.record_count(), 2);
    }

    #[test]
    fn engine_tick_and_inject() {
        let mut eng = ConsciousnessEngine::new();
        eng.tick().unwrap();
        let quale = eng.factory.create_semantic("test");
        eng.inject_qualia(quale).unwrap();
        assert!(eng.current_state.is_some());
        assert!(eng.total_cycles > 0);
    }

    #[test]
    fn engine_workspace_flow() {
        let mut eng = ConsciousnessEngine::new();
        let _id = eng.submit_to_workspace("hello".into(), QualiaModality::Semantic, "test", 0.7, 0.6);
        let winner = eng.run_workspace_competition();
        assert_eq!(winner, Some("hello".into()));
    }

    #[test]
    fn qualia_salience() {
        let v = QualiaVector::new(QualiaModality::Visual, vec![1.0, 0.0, 0.0]);
        let q = Quale { id: 1, modality: QualiaModality::Visual, vector: v, timestamp_ns: 0, bound_to: None };
        assert!(q.vector.salience() >= 0.0);
    }

    #[test]
    fn engine_status() {
        let mut eng = ConsciousnessEngine::new();
        eng.tick().unwrap();
        let s = eng.current_salience();
        assert!(s >= 0.0);
    }

    #[test]
    fn self_model_default() {
        let sm = SelfModelSnapshot::default();
        assert!((sm.agency - 0.5).abs() < 1e-6);
        assert!((sm.ownership - 0.5).abs() < 1e-6);
    }

    #[test]
    fn native_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("native:"));
        assert!(s.contains("cycles="));
    }

    #[test]
    fn native_act_tick() {
        init();
        let res = native_act("tick").unwrap();
        assert!(res.contains("总周期"));
    }

    #[test]
    fn native_act_inject_semantic() {
        init();
        let res = native_act("inject-semantic 你好世界").unwrap();
        assert!(res.contains("语义质原"));
    }

    #[test]
    fn native_act_inject_emotion() {
        init();
        let res = native_act("inject-emotion 0.5 0.7").unwrap();
        assert!(res.contains("情感质原"));
    }

    #[test]
    fn native_act_state() {
        init();
        native_act("tick").unwrap();
        let res = native_act("state").unwrap();
        assert!(res.contains("现象状态"));
    }

    #[test]
    fn native_act_stream() {
        init();
        native_act("tick").unwrap();
        let res = native_act("stream").unwrap();
        assert!(res.contains("意识流"));
    }

    #[test]
    fn native_act_metacog() {
        init();
        native_act("tick").unwrap();
        let res = native_act("metacog").unwrap();
        assert!(res.contains("元认知"));
    }

    #[test]
    fn modality_names() {
        assert_eq!(QualiaModality::Visual.name(), "visual");
        assert_eq!(QualiaModality::Semantic.name(), "semantic");
        assert_eq!(QualiaModality::Emotional.name(), "emotional");
        assert_eq!(QualiaModality::Self.name(), "self");
    }

    #[test]
    fn phenomenal_object_binding() {
        let mut s = PhenomenalState::new();
        let q = QualiaFactory::new().create_visual(1.0, 0.0, 0.0, 1.0);
        let qid = q.id;
        s.add_quale(q);
        s.bind_objects();
        assert!(s.objects.iter().any(|o| o.bound_qualia.contains(&qid)));
    }

    #[test]
    fn dominant_modality() {
        let mut s = PhenomenalState::new();
        let _ = s.add_quale(QualiaFactory::new().create_visual(1.0, 0.0, 0.0, 1.0));
        let _ = s.add_quale(QualiaFactory::new().create_visual(0.0, 1.0, 0.0, 1.0));
        let _ = s.add_quale(QualiaFactory::new().create_auditory_placeholder(440.0));
        assert_eq!(s.dominant_modality(), Some(QualiaModality::Visual));
    }
}

// Helper: create auditory quale (not in factory, inline for test)
impl QualiaFactory {
    pub fn create_auditory_placeholder(&mut self, freq: f32) -> Quale {
        self.create(QualiaModality::Auditory, vec![freq, 0.5, 0.0, 0.0])
    }
}
