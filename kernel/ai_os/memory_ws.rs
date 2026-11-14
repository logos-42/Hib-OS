//! 工作记忆层 —— AI 原生 OS 的「记忆系统」 (Memory Working Space)
//!
//! 本模块实现 AI 原生 OS 的多层记忆架构，对应传统 OS 中的「缓存 / 主存 /
//! 交换分区 / 文件系统」概念，但重新映射到记忆域：
//!
//! * SensoryBuffer (感觉缓冲) —— 极短暂的感觉记忆，等价于 I/O 缓冲区；
//! * WorkingMemory (工作记忆) —— 短期持续处理的信息，等价于寄存器 / L1 缓存；
//! * ShortTermMemory (短时记忆) —— 分钟到小时级保持，等价于主存；
//! * LongTermMemory (长时记忆) —— 永久性语义/情景记忆，等价于磁盘 / 文件系统；
//! * EpisodicMemory (情景记忆) —— 带时间戳的个人经历，等价于日志 / journald；
//! * SemanticMemory (语义记忆) —— 事实与概念知识，等价于元数据 / 数据库；
//! * ProceduralMemory (程序记忆) —— 技能与操作流程，等价于二进制 / 脚本；
//! * MemoryConsolidation (记忆巩固) —— 短时转长时，等价于脏页回写；
//! * MemoryReconsolidation (记忆再巩固) —— 回忆时的修改与更新；
//! * MemoryForgetting (遗忘机制) —— 基于使用频率与时间的衰减，等价于 LRU / OOM；
//! * MemoryAssociation (记忆联想) —— 语义关联检索，等价于索引 / 搜索。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度，
//! 并通过 `memory/*` 通道接入 `sense`/`act` 感知层。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 感觉缓冲 (SensoryBuffer)
// =====================================================================================

/// 感觉模态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SensoryModality {
    Iconic,    // 视觉
    Echoic,    // 听觉
    Haptic,    // 触觉
    Olfactory, // 嗅觉
    Gustatory, // 味觉
}

/// 感觉缓冲条目：极短暂（<1 秒）的原始感觉拷贝。
#[derive(Clone, Debug)]
pub struct SensoryEntry {
    pub id: u64,
    pub modality: SensoryModality,
    pub data: Vec<u8>,
    pub timestamp_ns: u128,
    pub decay_ns: u128, // 保持时长
}

/// 感觉缓冲：感觉登记处，所有输入首先在此短暂停留。
pub struct SensoryBuffer {
    entries: VecDeque<SensoryEntry>,
    next_id: u64,
    pub capacity: usize,
    pub default_decay_ns: u128,
}

impl SensoryBuffer {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 1,
            capacity: 1024,
            default_decay_ns: 500_000_000, // 0.5 秒
        }
    }

    pub fn write(&mut self, modality: SensoryModality, data: Vec<u8>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        self.entries.push_back(SensoryEntry {
            id,
            modality,
            data,
            timestamp_ns: now,
            decay_ns: self.default_decay_ns,
        });
        if self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        id
    }

    pub fn read(&mut self, id: u64) -> Option<Vec<u8>> {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            Some(self.entries.remove(pos).unwrap().data)
        } else {
            None
        }
    }

    pub fn decay(&mut self, now_ns: u128) {
        let mut to_remove = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if now_ns - e.timestamp_ns > e.decay_ns {
                to_remove.push(i);
            }
        }
        for i in to_remove.into_iter().rev() {
            self.entries.remove(i);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 工作记忆 (WorkingMemory) —— 重新定义，区别于 mind 模块
// =====================================================================================

/// 工作记忆条目：正在被主动处理的信息块。
#[derive(Clone, Debug)]
pub struct WMEntry {
    pub id: u64,
    pub content: String,
    pub activation: f32,      // 激活水平 0~1
    pub last_access_ns: u128,
    pub rehearsal_count: u64, // 复述次数
    pub chunk_size: u64,      // 认知块大小
}

/// 工作记忆：米勒的 7±2 块容量限制。
pub struct WorkingMemory {
    chunks: BTreeMap<u64, WMEntry>,
    next_id: u64,
    pub capacity: usize,
    pub decay_rate: f32,
}

impl WorkingMemory {
    pub fn new() -> Self {
        Self {
            chunks: BTreeMap::new(),
            next_id: 1,
            capacity: 7,
            decay_rate: 0.1,
        }
    }

    pub fn store(&mut self, content: String) -> u64 {
        if self.chunks.len() >= self.capacity {
            // 驱逐最低激活的
            let evict = self.chunks.iter().min_by_key(|(_, e)| {
                (e.activation * 1000.0) as u64
            }).map(|(id, _)| *id);
            if let Some(id) = evict {
                self.chunks.remove(&id);
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        let chunk_size = content.len() as u64;
        self.chunks.insert(id, WMEntry {
            id,
            content,
            activation: 1.0,
            last_access_ns: now,
            rehearsal_count: 1,
            chunk_size,
        });
        id
    }

    pub fn retrieve(&mut self, id: u64) -> Option<String> {
        if let Some(e) = self.chunks.get_mut(&id) {
            e.last_access_ns = crate::ai_os::native::now_ns();
            e.rehearsal_count += 1;
            e.activation = (e.activation + 0.1).min(1.0);
            Some(e.content.clone())
        } else {
            None
        }
    }

    pub fn rehearse(&mut self, id: u64) -> bool {
        if let Some(e) = self.chunks.get_mut(&id) {
            e.rehearsal_count += 1;
            e.activation = (e.activation + 0.2).min(1.0);
            e.last_access_ns = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn decay(&mut self, now_ns: u128) {
        let mut to_remove = Vec::new();
        for (id, e) in self.chunks.iter_mut() {
            let elapsed = now_ns - e.last_access_ns;
            let decay = (elapsed as f64 * self.decay_rate as f64 / 1_000_000_000.0) as f32;
            e.activation = (e.activation - decay).max(0.0);
            if e.activation < 0.05 {
                to_remove.push(*id);
            }
        }
        for id in to_remove {
            self.chunks.remove(&id);
        }
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn utilization(&self) -> f32 {
        self.chunks.len() as f32 / self.capacity as f32
    }

    pub fn total_activation(&self) -> f32 {
        self.chunks.values().map(|e| e.activation).sum()
    }
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 短时记忆 (ShortTermMemory)
// =====================================================================================

/// 短时记忆条目：分钟到小时级保持。
#[derive(Clone, Debug)]
pub struct STMEntry {
    pub id: u64,
    pub content: String,
    pub encoded_at: u128,
    pub access_count: u64,
    pub importance: f32, // 0~1，决定是否转入长时记忆
}

/// 短时记忆：工作记忆经过编码后的中转站。
pub struct ShortTermMemory {
    entries: BTreeMap<u64, STMEntry>,
    next_id: u64,
    pub capacity: usize,
}

impl ShortTermMemory {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            capacity: 1000,
        }
    }

    pub fn encode(&mut self, content: String, importance: f32) -> u64 {
        if self.entries.len() >= self.capacity {
            // 移除最不重要的
            let evict = self.entries.iter().min_by_key(|(_, e)| {
                (e.importance * 1000.0) as u64
            }).map(|(id, _)| *id);
            if let Some(id) = evict {
                self.entries.remove(&id);
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(id, STMEntry {
            id,
            content,
            encoded_at: crate::ai_os::native::now_ns(),
            access_count: 0,
            importance: importance.clamp(0.0, 1.0),
        });
        id
    }

    pub fn access(&mut self, id: u64) -> Option<String> {
        if let Some(e) = self.entries.get_mut(&id) {
            e.access_count += 1;
            Some(e.content.clone())
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn avg_importance(&self) -> f32 {
        if self.entries.is_empty() { 0.0 } else {
            self.entries.values().map(|e| e.importance).sum::<f32>() / self.entries.len() as f32
        }
    }
}

// =====================================================================================
// 长时记忆 (LongTermMemory)
// =====================================================================================

/// 长时记忆类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LTMType {
    Episodic,   // 情景
    Semantic,   // 语义
    Procedural, // 程序
}

/// 长时记忆条目：永久性存储。
#[derive(Clone, Debug)]
pub struct LTMEntry {
    pub id: u64,
    pub r#type: LTMType,
    pub key: String,
    pub value: String,
    pub encoded_at: u128,
    pub last_access_ns: u128,
    pub access_count: u64,
    pub strength: f32, // 记忆痕迹强度 0~1
}

/// 长时记忆：持久化存储层。
pub struct LongTermMemory {
    entries: BTreeMap<u64, LTMEntry>,
    key_index: BTreeMap<String, u64>,
    next_id: u64,
}

impl LongTermMemory {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            key_index: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn store(&mut self, r#type: LTMType, key: &str, value: &str, strength: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        let entry = LTMEntry {
            id,
            r#type,
            key: key.to_string(),
            value: value.to_string(),
            encoded_at: now,
            last_access_ns: now,
            access_count: 0,
            strength: strength.clamp(0.0, 1.0),
        };
        self.key_index.insert(key.to_string(), id);
        self.entries.insert(id, entry);
        id
    }

    pub fn retrieve(&mut self, key: &str) -> Option<String> {
        let id = *self.key_index.get(key)?;
        if let Some(e) = self.entries.get_mut(&id) {
            e.last_access_ns = crate::ai_os::native::now_ns();
            e.access_count += 1;
            Some(e.value.clone())
        } else {
            None
        }
    }

    pub fn strengthen(&mut self, key: &str, delta: f32) -> bool {
        if let Some(id) = self.key_index.get(key) {
            if let Some(e) = self.entries.get_mut(id) {
                e.strength = (e.strength + delta).min(1.0);
                return true;
            }
        }
        false
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn by_type(&self, r#type: LTMType) -> Vec<&LTMEntry> {
        self.entries.values().filter(|e| e.r#type == r#type).collect()
    }

    pub fn avg_strength(&self) -> f32 {
        if self.entries.is_empty() { 0.0 } else {
            self.entries.values().map(|e| e.strength).sum::<f32>() / self.entries.len() as f32
        }
    }
}

// =====================================================================================
// 情景记忆 (EpisodicMemory)
// =====================================================================================

/// 情景事件：带时间、地点、人物的个人经历。
#[derive(Clone, Debug)]
pub struct Episode {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub occurred_at: u128,
    pub encoded_at: u128,
    pub participants: Vec<String>,
    pub location: Option<String>,
    pub emotion: f32, // -1~1
    pub salience: f32, // 0~1
}

/// 情景记忆：自传体经历的时间顺序存储。
pub struct EpisodicMemory {
    episodes: BTreeMap<u64, Episode>,
    timeline: VecDeque<u64>, // ID 按时间排序
    next_id: u64,
    pub capacity: usize,
}

impl EpisodicMemory {
    pub fn new() -> Self {
        Self {
            episodes: BTreeMap::new(),
            timeline: VecDeque::new(),
            next_id: 1,
            capacity: 100000,
        }
    }

    pub fn record(&mut self, title: &str, description: &str, occurred_at: u128, participants: &[&str], location: Option<&str>, emotion: f32, salience: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        let episode = Episode {
            id,
            title: title.to_string(),
            description: description.to_string(),
            occurred_at,
            encoded_at: now,
            participants: participants.iter().map(|s| s.to_string()).collect(),
            location: location.map(|s| s.to_string()),
            emotion: emotion.clamp(-1.0, 1.0),
            salience: salience.clamp(0.0, 1.0),
        };
        self.episodes.insert(id, episode);
        self.timeline.push_back(id);
        if self.timeline.len() > self.capacity {
            let old = self.timeline.pop_front().unwrap();
            self.episodes.remove(&old);
        }
        id
    }

    pub fn get(&self, id: u64) -> Option<&Episode> {
        self.episodes.get(&id)
    }

    pub fn recent(&self, n: usize) -> Vec<&Episode> {
        self.timeline.iter().rev().take(n).filter_map(|id| self.episodes.get(id)).collect()
    }

    pub fn search_by_emotion(&self, min_emotion: f32) -> Vec<&Episode> {
        self.episodes.values().filter(|e| e.emotion >= min_emotion).collect()
    }

    pub fn len(&self) -> usize {
        self.episodes.len()
    }
}

// =====================================================================================
// 语义记忆 (SemanticMemory)
// =====================================================================================

/// 语义事实：概念属性三元组。
#[derive(Clone, Debug)]
pub struct SemanticFact {
    pub id: u64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32, // 0~1
    pub learned_at: u128,
}

/// 语义记忆：事实与概念的陈述性知识。
pub struct SemanticMemory {
    facts: BTreeMap<u64, SemanticFact>,
    subject_index: BTreeMap<String, Vec<u64>>,
    next_id: u64,
}

impl SemanticMemory {
    pub fn new() -> Self {
        Self {
            facts: BTreeMap::new(),
            subject_index: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add_fact(&mut self, subject: &str, predicate: &str, object: &str, confidence: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let fact = SemanticFact {
            id,
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            learned_at: crate::ai_os::native::now_ns(),
        };
        self.subject_index.entry(subject.to_string()).or_default().push(id);
        self.facts.insert(id, fact);
        id
    }

    pub fn query_by_subject(&self, subject: &str) -> Vec<&SemanticFact> {
        self.subject_index.get(subject)
            .map(|ids| ids.iter().filter_map(|id| self.facts.get(id)).collect())
            .unwrap_or_default()
    }

    pub fn query_triple(&self, subject: &str, predicate: &str) -> Option<String> {
        for id in self.subject_index.get(subject).unwrap_or(&Vec::new()) {
            if let Some(f) = self.facts.get(id) {
                if f.predicate == predicate {
                    return Some(f.object.clone());
                }
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.facts.len()
    }
}

// =====================================================================================
// 程序记忆 (ProceduralMemory)
// =====================================================================================

/// 技能步骤。
#[derive(Clone, Debug)]
pub struct SkillStep {
    pub id: u64,
    pub action: String,
    pub precondition: String,
    pub effect: String,
}

/// 技能：程序性记忆中的操作流程。
#[derive(Clone, Debug)]
pub struct Skill {
    pub id: u64,
    pub name: String,
    pub steps: Vec<SkillStep>,
    pub proficiency: f32, // 0~1，熟练度
    pub usage_count: u64,
}

/// 程序记忆：技能与操作流程存储。
pub struct ProceduralMemory {
    skills: BTreeMap<u64, Skill>,
    name_index: BTreeMap<String, u64>,
    next_id: u64,
}

impl ProceduralMemory {
    pub fn new() -> Self {
        Self {
            skills: BTreeMap::new(),
            name_index: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn learn_skill(&mut self, name: &str, steps: Vec<SkillStep>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let skill = Skill {
            id,
            name: name.to_string(),
            steps,
            proficiency: 0.1,
            usage_count: 0,
        };
        self.name_index.insert(name.to_string(), id);
        self.skills.insert(id, skill);
        id
    }

    pub fn execute(&mut self, name: &str) -> Option<&Skill> {
        let id = *self.name_index.get(name)?;
        if let Some(s) = self.skills.get_mut(&id) {
            s.usage_count += 1;
            s.proficiency = (s.proficiency + 0.01).min(1.0);
            Some(s)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn avg_proficiency(&self) -> f32 {
        if self.skills.is_empty() { 0.0 } else {
            self.skills.values().map(|s| s.proficiency).sum::<f32>() / self.skills.len() as f32
        }
    }
}

// =====================================================================================
// 记忆巩固 (MemoryConsolidation)
// =====================================================================================

/// 巡回方案：哪些短时记忆需要巩固。
#[derive(Clone, Debug)]
pub struct ConsolidationPlan {
    pub stm_id: u64,
    pub target_type: LTMType,
    pub strength: f32,
    pub reason: String,
}

/// 记忆巩固：将短时记忆转移到长时记忆（类比脏页回写）。
pub struct MemoryConsolidation {
    pub consolidated_count: u64,
    pub failed_count: u64,
    pub last_consolidation_ns: Option<u128>,
}

impl MemoryConsolidation {
    pub fn new() -> Self {
        Self {
            consolidated_count: 0,
            failed_count: 0,
            last_consolidation_ns: None,
        }
    }

    pub fn consolidate(&mut self, stm_entry: &STMEntry, ltm: &mut LongTermMemory) -> Result<u64, String> {
        if stm_entry.importance < 0.3 {
            self.failed_count += 1;
            return Err("重要性不足，跳过巩固".into());
        }
        let r#type = if stm_entry.importance > 0.7 { LTMType::Episodic } else { LTMType::Semantic };
        let id = ltm.store(r#type, &format!("stm_{}", stm_entry.id), &stm_entry.content, stm_entry.importance);
        self.consolidated_count += 1;
        self.last_consolidation_ns = Some(crate::ai_os::native::now_ns());
        Ok(id)
    }

    pub fn batch_consolidate(&mut self, stm: &ShortTermMemory, ltm: &mut LongTermMemory) -> (u64, u64) {
        let success = 0;
        let fail = 0;
        // 简化：遍历所有 STM 条目（这里无法直接访问内部，返回计数占位）
        let _ = stm;
        let _ = ltm;
        (success, fail)
    }

    pub fn stats(&self) -> String {
        format!("巩固={} 失败={}", self.consolidated_count, self.failed_count)
    }
}

// =====================================================================================
// 记忆遗忘 (MemoryForgetting)
// =====================================================================================

/// 遗忘曲线参数。
#[derive(Clone, Debug)]
pub struct ForgettingCurve {
    pub initial_strength: f32,
    pub decay_rate: f32,
    pub repetitions: u64,
}

impl ForgettingCurve {
    /// R = e^(-t/S)，S 为相对强度
    pub fn retention(&self, elapsed_seconds: f64) -> f32 {
        let s = self.initial_strength.max(0.01);
        let t = elapsed_seconds;
        (-(t as f32) * self.decay_rate / s).exp()
    }
}

/// 遗忘机制：基于艾宾浩斯遗忘曲线的记忆衰减。
pub struct MemoryForgetting {
    pub forgotten_count: u64,
    pub curve: ForgettingCurve,
}

impl MemoryForgetting {
    pub fn new() -> Self {
        Self {
            forgotten_count: 0,
            curve: ForgettingCurve { initial_strength: 1.0, decay_rate: 0.1, repetitions: 0 },
        }
    }

    pub fn retention(&self, elapsed_seconds: f64) -> f32 {
        self.curve.retention(elapsed_seconds)
    }

    pub fn should_forget(&self, retention: f32, threshold: f32) -> bool {
        retention < threshold
    }

    pub fn record_forget(&mut self) {
        self.forgotten_count += 1;
        self.curve.repetitions += 1;
    }

    pub fn stats(&self) -> String {
        format!("遗忘={} 曲线强度={:.2}", self.forgotten_count, self.curve.initial_strength)
    }
}

// =====================================================================================
// 记忆联想 (MemoryAssociation)
// =====================================================================================

/// 联想结果。
#[derive(Clone, Debug)]
pub struct AssociationResult {
    pub target_id: u64,
    pub target_key: String,
    pub association_strength: f32,
}

/// 记忆联想：基于语义相似度的关联检索。
pub struct MemoryAssociation {
    associations: BTreeMap<String, Vec<(String, f32)>>, // key -> [(related_key, strength)]
}

impl MemoryAssociation {
    pub fn new() -> Self {
        Self {
            associations: BTreeMap::new(),
        }
    }

    pub fn link(&mut self, key1: &str, key2: &str, strength: f32) {
        self.associations.entry(key1.to_string()).or_default().push((key2.to_string(), strength.clamp(0.0, 1.0)));
        self.associations.entry(key2.to_string()).or_default().push((key1.to_string(), strength.clamp(0.0, 1.0)));
    }

    pub fn associate(&self, key: &str, top_k: usize) -> Vec<AssociationResult> {
        let results: Vec<AssociationResult> = self.associations.get(key)
            .map(|rels| {
                let mut v: Vec<AssociationResult> = rels.iter().map(|(k, s)| AssociationResult {
                    target_id: 0, // 简化
                    target_key: k.clone(),
                    association_strength: *s,
                }).collect();
                v.sort_by(|a, b| b.association_strength.partial_cmp(&a.association_strength).unwrap_or(std::cmp::Ordering::Equal));
                v.truncate(top_k);
                v
            })
            .unwrap_or_default();
        results
    }

    pub fn link_count(&self) -> usize {
        self.associations.len()
    }
}

// =====================================================================================
// 记忆引擎 (MemoryEngine) —— 统一入口
// =====================================================================================

/// 记忆引擎：整合感觉缓冲、工作记忆、短时记忆、长时记忆、情景记忆、语义记忆、程序记忆、记忆巩固、遗忘、联想。
pub struct MemoryEngine {
    pub sensory: SensoryBuffer,
    pub working: WorkingMemory,
    pub short_term: ShortTermMemory,
    pub long_term: LongTermMemory,
    pub episodic: EpisodicMemory,
    pub semantic: SemanticMemory,
    pub procedural: ProceduralMemory,
    pub consolidation: MemoryConsolidation,
    pub forgetting: MemoryForgetting,
    pub association: MemoryAssociation,
    pub enabled: bool,
    pub total_operations: u64,
}

impl MemoryEngine {
    pub fn new() -> Self {
        Self {
            sensory: SensoryBuffer::new(),
            working: WorkingMemory::new(),
            short_term: ShortTermMemory::new(),
            long_term: LongTermMemory::new(),
            episodic: EpisodicMemory::new(),
            semantic: SemanticMemory::new(),
            procedural: ProceduralMemory::new(),
            consolidation: MemoryConsolidation::new(),
            forgetting: MemoryForgetting::new(),
            association: MemoryAssociation::new(),
            enabled: true,
            total_operations: 0,
        }
    }

    pub fn sensory_write(&mut self, modality: SensoryModality, data: Vec<u8>) -> u64 {
        self.total_operations += 1;
        self.sensory.write(modality, data)
    }

    pub fn sensory_decay(&mut self, now_ns: u128) {
        self.sensory.decay(now_ns);
    }

    pub fn wm_store(&mut self, content: String) -> u64 {
        self.total_operations += 1;
        self.working.store(content)
    }

    pub fn wm_retrieve(&mut self, id: u64) -> Option<String> {
        self.total_operations += 1;
        self.working.retrieve(id)
    }

    pub fn wm_decay(&mut self, now_ns: u128) {
        self.working.decay(now_ns);
    }

    pub fn stm_encode(&mut self, content: String, importance: f32) -> u64 {
        self.total_operations += 1;
        self.short_term.encode(content, importance)
    }

    pub fn stm_access(&mut self, id: u64) -> Option<String> {
        self.total_operations += 1;
        self.short_term.access(id)
    }

    pub fn ltm_store(&mut self, r#type: LTMType, key: &str, value: &str, strength: f32) -> u64 {
        self.total_operations += 1;
        self.long_term.store(r#type, key, value, strength)
    }

    pub fn ltm_retrieve(&mut self, key: &str) -> Option<String> {
        self.total_operations += 1;
        self.long_term.retrieve(key)
    }

    pub fn episode_record(&mut self, title: &str, desc: &str, occurred_at: u128, participants: &[&str], location: Option<&str>, emotion: f32, salience: f32) -> u64 {
        self.total_operations += 1;
        self.episodic.record(title, desc, occurred_at, participants, location, emotion, salience)
    }

    pub fn semantic_add(&mut self, subject: &str, predicate: &str, object: &str, confidence: f32) -> u64 {
        self.total_operations += 1;
        self.semantic.add_fact(subject, predicate, object, confidence)
    }

    pub fn semantic_query(&self, subject: &str, predicate: &str) -> Option<String> {
        self.semantic.query_triple(subject, predicate)
    }

    pub fn skill_learn(&mut self, name: &str, steps: Vec<SkillStep>) -> u64 {
        self.total_operations += 1;
        self.procedural.learn_skill(name, steps)
    }

    pub fn skill_execute(&mut self, name: &str) -> Option<String> {
        self.total_operations += 1;
        self.procedural.execute(name).map(|s| format!("技能 {} 熟练度={:.2}", s.name, s.proficiency))
    }

    pub fn consolidate_wm_to_ltm(&mut self) -> u64 {
        let mut count = 0;
        // 简化：从工作记忆中激活度高的巩固到 LTM
        let _now = crate::ai_os::native::now_ns();
        let active: Vec<(u64, String, f32)> = self.working.chunks.iter()
            .filter(|(_, e)| e.activation > 0.5)
            .map(|(id, e)| (*id, e.content.clone(), e.activation))
            .collect();
        for (id, content, activation) in active {
            let _ = self.long_term.store(LTMType::Semantic, &format!("wm_{}", id), &content, activation);
            count += 1;
        }
        self.consolidation.consolidated_count += count;
        count
    }

    pub fn tick(&mut self, now_ns: u128) {
        self.sensory_decay(now_ns);
        self.wm_decay(now_ns);
    }

    pub fn total_operations(&self) -> u64 {
        self.total_operations
    }
}

impl Default for MemoryEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static MEMORY: OnceLock<Mutex<MemoryEngine>> = OnceLock::new();

pub fn memory_engine() -> &'static Mutex<MemoryEngine> {
    MEMORY.get_or_init(|| Mutex::new(MemoryEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = memory_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = memory_engine().lock().unwrap();
    format!(
        "memory: sensory={} working={} utilization={:.2} stm={} ltm={} episodic={} semantic_facts={} skills={} operations={} consolidation={}",
        eng.sensory.len(),
        eng.working.len(),
        eng.working.utilization(),
        eng.short_term.len(),
        eng.long_term.len(),
        eng.episodic.len(),
        eng.semantic.len(),
        eng.procedural.len(),
        eng.total_operations,
        eng.consolidation.stats()
    )
}

pub fn memory_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn memory_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("sensory-write") => {
            let modality_str = it.next().unwrap_or("iconic");
            let modality = match modality_str {
                "iconic" => SensoryModality::Iconic,
                "echoic" => SensoryModality::Echoic,
                "haptic" => SensoryModality::Haptic,
                "olfactory" => SensoryModality::Olfactory,
                "gustatory" => SensoryModality::Gustatory,
                _ => SensoryModality::Iconic,
            };
            let data: Vec<u8> = it.map(|s| s.as_bytes().iter().cloned().collect::<Vec<_>>().into_iter().next().unwrap_or(0)).collect();
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.sensory_write(modality, data);
            Ok(format!("感觉缓冲写入 id={}", id))
        }
        Some("sensory-decay") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = memory_engine().lock().unwrap();
            eng.sensory_decay(now);
            Ok(format!("感觉缓冲已衰减 now={}", now))
        }
        Some("wm-store") => {
            let content = it.next().ok_or("用法: memory/ wm-store <内容>")?;
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.wm_store(content.to_string());
            Ok(format!("工作记忆存储 id={}", id))
        }
        Some("wm-retrieve") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: memory/ wm-retrieve <id>")?;
            let mut eng = memory_engine().lock().unwrap();
            match eng.wm_retrieve(id) {
                Some(c) => Ok(format!("工作记忆[id={}]: {}", id, c)),
                None => Err(format!("工作记忆[id={}] 不存在", id)),
            }
        }
        Some("wm-decay") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = memory_engine().lock().unwrap();
            eng.wm_decay(now);
            Ok(format!("工作记忆已衰减 now={}", now))
        }
        Some("stm-encode") => {
            let content = it.next().ok_or("用法: memory/ stm-encode <内容> <重要性>")?;
            let importance = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.stm_encode(content.to_string(), importance);
            Ok(format!("短时记忆编码 id={}", id))
        }
        Some("stm-access") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: memory/ stm-access <id>")?;
            let mut eng = memory_engine().lock().unwrap();
            match eng.stm_access(id) {
                Some(c) => Ok(format!("短时记忆[id={}]: {}", id, c)),
                None => Err(format!("短时记忆[id={}] 不存在", id)),
            }
        }
        Some("ltm-store") => {
            let type_str = it.next().unwrap_or("semantic");
            let r#type = match type_str {
                "episodic" => LTMType::Episodic,
                "procedural" => LTMType::Procedural,
                _ => LTMType::Semantic,
            };
            let key = it.next().ok_or("缺少键")?;
            let value = it.next().ok_or("缺少值")?;
            let strength = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.ltm_store(r#type, key, value, strength);
            Ok(format!("长时记忆存储 id={} 类型={:?}", id, r#type))
        }
        Some("ltm-retrieve") => {
            let key = it.next().ok_or("用法: memory/ ltm-retrieve <键>")?;
            let mut eng = memory_engine().lock().unwrap();
            match eng.ltm_retrieve(key) {
                Some(v) => Ok(format!("长时记忆[{}] = {}", key, v)),
                None => Err(format!("长时记忆[{}] 未找到", key)),
            }
        }
        Some("episode-record") => {
            let title = it.next().ok_or("用法: memory/ episode-record <标题> <描述> <情绪> <显著性>")?;
            let desc = it.next().ok_or("缺少描述")?;
            let emotion = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let salience = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.episode_record(title, desc, 0, &[], None, emotion, salience);
            Ok(format!("情景记忆记录 id={}", id))
        }
        Some("semantic-add") => {
            let subject = it.next().ok_or("用法: memory/ semantic-add <主语> <谓语> <宾语> <置信度>")?;
            let predicate = it.next().ok_or("缺少谓语")?;
            let object = it.next().ok_or("缺少宾语")?;
            let confidence = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.8);
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.semantic_add(subject, predicate, object, confidence);
            Ok(format!("语义记忆事实 id={}", id))
        }
        Some("semantic-query") => {
            let subject = it.next().ok_or("用法: memory/ semantic-query <主语> <谓语>")?;
            let predicate = it.next().ok_or("缺少谓语")?;
            let eng = memory_engine().lock().unwrap();
            match eng.semantic_query(subject, predicate) {
                Some(obj) => Ok(format!("{} {} = {}", subject, predicate, obj)),
                None => Err("未找到事实".into()),
            }
        }
        Some("skill-learn") => {
            let name = it.next().ok_or("用法: memory/ skill-learn <名称> <步骤...>")?;
            let steps: Vec<SkillStep> = it.map(|s| SkillStep { id: 0, action: s.to_string(), precondition: String::new(), effect: String::new() }).collect();
            let mut eng = memory_engine().lock().unwrap();
            let id = eng.skill_learn(name, steps);
            Ok(format!("程序记忆技能 id={} 名称={}", id, name))
        }
        Some("skill-execute") => {
            let name = it.next().ok_or("用法: memory/ skill-execute <名称>")?;
            let mut eng = memory_engine().lock().unwrap();
            match eng.skill_execute(name) {
                Some(result) => Ok(result),
                None => Err(format!("技能 {} 未学习", name)),
            }
        }
        Some("consolidate") => {
            let count = {
                let mut eng = memory_engine().lock().unwrap();
                eng.consolidate_wm_to_ltm()
            };
            Ok(format!("记忆巩固完成 已巩固 {} 条", count))
        }
        Some("tick") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = memory_engine().lock().unwrap();
            eng.tick(now);
            Ok(format!("记忆引擎已推进 now={}", now))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 memory 动作: {}", other)),
    }
}

pub fn memory_act_dispatch(payload: &str) -> Result<String, String> {
    memory_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensory_write_read() {
        let mut sb = SensoryBuffer::new();
        let id = sb.write(SensoryModality::Iconic, vec![1, 2, 3]);
        assert_eq!(sb.read(id), Some(vec![1, 2, 3]));
    }

    #[test]
    fn sensory_decay() {
        let mut sb = SensoryBuffer::new();
        sb.default_decay_ns = 1;
        sb.write(SensoryModality::Echoic, vec![42]);
        sb.decay(u128::MAX);
        assert_eq!(sb.len(), 0);
    }

    #[test]
    fn wm_store_retrieve() {
        let mut wm = WorkingMemory::new();
        let id = wm.store("hello".into());
        assert_eq!(wm.retrieve(id), Some("hello".into()));
    }

    #[test]
    fn wm_capacity_eviction() {
        let mut wm = WorkingMemory::new();
        wm.capacity = 2;
        wm.store("a".into());
        wm.store("b".into());
        let id = wm.store("c".into()); // 应驱逐最低激活的
        assert!(id > 0);
        assert!(wm.len() <= 2);
    }

    #[test]
    fn wm_rehearse() {
        let mut wm = WorkingMemory::new();
        let id = wm.store("rehearse me".into());
        assert!(wm.rehearse(id));
        let e = wm.chunks.get(&id).unwrap();
        assert_eq!(e.rehearsal_count, 2);
    }

    #[test]
    fn wm_decay_removes() {
        let mut wm = WorkingMemory::new();
        wm.decay_rate = 10.0;
        wm.store("temp".into());
        wm.decay(u128::MAX);
        assert_eq!(wm.len(), 0);
    }

    #[test]
    fn stm_encode_access() {
        let mut stm = ShortTermMemory::new();
        let id = stm.encode("fact".into(), 0.8);
        assert_eq!(stm.access(id), Some("fact".into()));
    }

    #[test]
    fn stm_capacity() {
        let mut stm = ShortTermMemory::new();
        stm.capacity = 2;
        stm.encode("a".into(), 0.5);
        stm.encode("b".into(), 0.5);
        stm.encode("c".into(), 0.9); // 应驱逐低重要的
        assert!(stm.len() <= 2);
    }

    #[test]
    fn ltm_store_retrieve() {
        let mut ltm = LongTermMemory::new();
        let id = ltm.store(LTMType::Semantic, "capital", "Paris", 0.9);
        assert_eq!(ltm.retrieve("capital"), Some("Paris".into()));
        assert_eq!(id, 1);
    }

    #[test]
    fn ltm_strengthen() {
        let mut ltm = LongTermMemory::new();
        ltm.store(LTMType::Semantic, "k", "v", 0.3);
        assert!(ltm.strengthen("k", 0.5));
        let e = ltm.entries.get(&1).unwrap();
        assert!((e.strength - 0.8).abs() < 1e-6);
    }

    #[test]
    fn ltm_by_type() {
        let mut ltm = LongTermMemory::new();
        ltm.store(LTMType::Episodic, "e1", "v1", 0.5);
        ltm.store(LTMType::Semantic, "s1", "v2", 0.5);
        let eps = ltm.by_type(LTMType::Episodic);
        assert_eq!(eps.len(), 1);
    }

    #[test]
    fn episode_record() {
        let mut em = EpisodicMemory::new();
        let id = em.record("生日", "过生日了", 0, &["妈妈"], Some("家"), 0.8, 0.9);
        assert_eq!(id, 1);
        let ep = em.get(id).unwrap();
        assert_eq!(ep.title, "生日");
        assert_eq!(em.len(), 1);
    }

    #[test]
    fn episode_recent() {
        let mut em = EpisodicMemory::new();
        em.record("e1", "d1", 0, &[], None, 0.0, 0.5);
        em.record("e2", "d2", 0, &[], None, 0.0, 0.5);
        let recent = em.recent(1);
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn semantic_fact() {
        let mut sm = SemanticMemory::new();
        let id = sm.add_fact("法兰西", "首都是", "巴黎", 1.0);
        assert_eq!(id, 1);
        assert_eq!(sm.query_triple("法兰西", "首都是"), Some("巴黎".into()));
        assert_eq!(sm.query_by_subject("法兰西").len(), 1);
    }

    #[test]
    fn skill_learn_execute() {
        let mut pm = ProceduralMemory::new();
        let steps = vec![
            SkillStep { id: 1, action: "开门".into(), precondition: "门关闭".into(), effect: "门打开".into() },
        ];
        let id = pm.learn_skill("进门", steps);
        assert_eq!(id, 1);
        let result = pm.execute("进门").unwrap();
        assert_eq!(result.name, "进门");
        assert!(result.proficiency > 0.1);
    }

    #[test]
    fn consolidation() {
        let mut cons = MemoryConsolidation::new();
        let mut ltm = LongTermMemory::new();
        let stm = STMEntry { id: 1, content: "重要信息".into(), encoded_at: 0, access_count: 1, importance: 0.8 };
        let id = cons.consolidate(&stm, &mut ltm).unwrap();
        assert_eq!(id, 1);
        assert_eq!(cons.consolidated_count, 1);
    }

    #[test]
    fn consolidation_low_importance() {
        let mut cons = MemoryConsolidation::new();
        let mut ltm = LongTermMemory::new();
        let stm = STMEntry { id: 1, content: "无关信息".into(), encoded_at: 0, access_count: 1, importance: 0.1 };
        assert!(cons.consolidate(&stm, &mut ltm).is_err());
        assert_eq!(cons.failed_count, 1);
    }

    #[test]
    fn forgetting_retention() {
        let fc = ForgettingCurve { initial_strength: 1.0, decay_rate: 0.1, repetitions: 0 };
        let r = fc.retention(0.0);
        assert!((r - 1.0).abs() < 1e-6);
        let r2 = fc.retention(100.0);
        assert!(r2 < r);
    }

    #[test]
    fn association_link() {
        let mut ma = MemoryAssociation::new();
        ma.link("apple", "fruit", 0.9);
        ma.link("apple", "red", 0.7);
        let results = ma.associate("apple", 5);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn memory_engine_full_flow() {
        let mut eng = MemoryEngine::new();
        eng.sensory_write(SensoryModality::Iconic, vec![1]);
        let wid = eng.wm_store("active info".into());
        assert_eq!(eng.wm_retrieve(wid), Some("active info".into()));
        let sid = eng.stm_encode("encoded fact".into(), 0.7);
        assert_eq!(eng.stm_access(sid), Some("encoded fact".into()));
        let _lid = eng.ltm_store(LTMType::Semantic, "k", "v", 0.5);
        assert_eq!(eng.ltm_retrieve("k"), Some("v".into()));
        let eid = eng.episode_record("t", "d", 0, &[], None, 0.0, 0.5);
        assert!(eid > 0);
        let skid = eng.semantic_add("s", "p", "o", 0.9);
        assert!(skid > 0);
        assert_eq!(eng.semantic_query("s", "p"), Some("o".into()));
        let step = SkillStep { id: 1, action: "a".into(), precondition: "".into(), effect: "".into() };
        let sid2 = eng.skill_learn("skill1", vec![step]);
        assert!(sid2 > 0);
        let res = eng.skill_execute("skill1");
        assert!(res.is_some());
        let consolidated = eng.consolidate_wm_to_ltm();
        let _ = consolidated;
        eng.tick(1_000_000_000);
    }

    #[test]
    fn memory_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("memory:"));
        assert!(s.contains("sensory="));
    }

    #[test]
    fn memory_act_wm() {
        init();
        let res = memory_act("wm-store 测试内容").unwrap();
        assert!(res.contains("工作记忆存储"));
        let res = memory_act("wm-retrieve 1").unwrap();
        assert!(res.contains("测试内容"));
    }

    #[test]
    fn memory_act_ltm() {
        init();
        let res = memory_act("ltm-store semantic key1 value1 0.8").unwrap();
        assert!(res.contains("长时记忆存储"));
        let res = memory_act("ltm-retrieve key1").unwrap();
        assert!(res.contains("value1"));
    }

    #[test]
    fn memory_act_episode() {
        init();
        let res = memory_act("episode-record 标题 描述 0.3 0.7").unwrap();
        assert!(res.contains("情景记忆记录"));
    }

    #[test]
    fn memory_act_semantic() {
        init();
        let res = memory_act("semantic-add 猫 是 哺乳动物 1.0").unwrap();
        assert!(res.contains("语义记忆事实"));
        let res = memory_act("semantic-query 猫 是").unwrap();
        assert!(res.contains("哺乳动物"));
    }

    #[test]
    fn memory_act_skill() {
        init();
        let res = memory_act("skill-learn 跑步 热身 跑步 拉伸").unwrap();
        assert!(res.contains("程序记忆技能"));
        let res = memory_act("skill-execute 跑步").unwrap();
        assert!(res.contains("熟练度"));
    }

    #[test]
    fn memory_act_consolidate() {
        init();
        memory_act("wm-store 工作内容").unwrap();
        let res = memory_act("consolidate").unwrap();
        assert!(res.contains("记忆巩固完成"));
    }

    #[test]
    fn memory_act_tick() {
        init();
        let res = memory_act("tick 5000000000").unwrap();
        assert!(res.contains("记忆引擎已推进"));
    }

    #[test]
    fn memory_act_status() {
        init();
        let res = memory_act("status").unwrap();
        assert!(res.contains("memory:"));
    }
}

// =====================================================================================
// 情感记忆 (EmotionalMemory)
// =====================================================================================

/// 情感类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EmotionType {
    Joy,      // 喜悦
    Sadness,  // 悲伤
    Anger,    // 愤怒
    Fear,     // 恐惧
    Surprise, // 惊喜
    Disgust,  // 厌恶
}

/// 情感记忆条目：带情感标记的记忆。
#[derive(Clone, Debug)]
pub struct EmotionalMemoryEntry {
    pub id: u64,
    pub content: String,
    pub emotion: EmotionType,
    pub intensity: f32,     // 0~1
    pub timestamp_ns: u128,
    pub recall_count: u64,
}

/// 情感记忆：情感驱动的记忆系统。
pub struct EmotionalMemory {
    entries: BTreeMap<u64, EmotionalMemoryEntry>,
    next_id: u64,
    pub emotion_weights: BTreeMap<EmotionType, f32>,
}

impl EmotionalMemory {
    pub fn new() -> Self {
        let mut weights = BTreeMap::new();
        weights.insert(EmotionType::Joy, 0.5);
        weights.insert(EmotionType::Sadness, 0.3);
        weights.insert(EmotionType::Anger, 0.4);
        weights.insert(EmotionType::Fear, 0.6);
        weights.insert(EmotionType::Surprise, 0.5);
        weights.insert(EmotionType::Disgust, 0.2);
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            emotion_weights: weights,
        }
    }

    pub fn store(&mut self, content: &str, emotion: EmotionType, intensity: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(id, EmotionalMemoryEntry {
            id,
            content: content.to_string(),
            emotion,
            intensity: intensity.clamp(0.0, 1.0),
            timestamp_ns: crate::ai_os::native::now_ns(),
            recall_count: 0,
        });
        id
    }

    pub fn recall(&mut self, id: u64) -> Option<String> {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.recall_count += 1;
            Some(entry.content.clone())
        } else {
            None
        }
    }

    pub fn recall_by_emotion(&self, emotion: EmotionType) -> Vec<&EmotionalMemoryEntry> {
        self.entries.values()
            .filter(|e| e.emotion == emotion)
            .collect()
    }

    pub fn strongest_emotion(&self) -> Option<EmotionType> {
        let mut best = None;
        let mut best_score = f32::NEG_INFINITY;
        for entry in self.entries.values() {
            let score = entry.intensity * self.emotion_weights.get(&entry.emotion).copied().unwrap_or(0.5);
            if score > best_score {
                best_score = score;
                best = Some(entry.emotion);
            }
        }
        best
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 联想记忆增强 (EnhancedAssociation)
// =====================================================================================

/// 联想强度。
#[derive(Clone, Debug)]
pub struct EnhancedAssociation {
    pub source: String,
    pub target: String,
    pub strength: f32,
    pub context: String,
    pub created_at: u128,
}

/// 增强记忆联想：基于上下文的关联网络。
pub struct EnhancedAssociationNetwork {
    pub associations: BTreeMap<String, Vec<EnhancedAssociation>>,
}

impl EnhancedAssociationNetwork {
    pub fn new() -> Self {
        Self {
            associations: BTreeMap::new(),
        }
    }

    pub fn link(&mut self, source: &str, target: &str, strength: f32, context: &str) {
        self.associations.entry(source.to_string()).or_default().push(EnhancedAssociation {
            source: source.to_string(),
            target: target.to_string(),
            strength: strength.clamp(0.0, 1.0),
            context: context.to_string(),
            created_at: crate::ai_os::native::now_ns(),
        });
    }

    pub fn get_associations(&self, source: &str) -> Option<&Vec<EnhancedAssociation>> {
        self.associations.get(source)
    }

    pub fn strongest_association(&self, source: &str) -> Option<&EnhancedAssociation> {
        self.associations.get(source).and_then(|assocs| {
            assocs.iter().max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal))
        })
    }

    pub fn link_count(&self) -> usize {
        self.associations.values().map(|v| v.len()).sum()
    }

    pub fn source_count(&self) -> usize {
        self.associations.len()
    }
}

// =====================================================================================
// 记忆重排序 (MemoryReranking)
// =====================================================================================

/// 重排序策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RerankStrategy {
    Recency,     // 最新优先
    Frequency,   // 频率优先
    Salience,    // 显著性优先
    Hybrid,      // 混合策略
}

/// 记忆重排序器：重新排序检索结果。
pub struct MemoryReranker {
    pub strategy: RerankStrategy,
    pub recency_weight: f32,
    pub frequency_weight: f32,
    pub salience_weight: f32,
}

impl MemoryReranker {
    pub fn new() -> Self {
        Self {
            strategy: RerankStrategy::Hybrid,
            recency_weight: 0.4,
            frequency_weight: 0.3,
            salience_weight: 0.3,
        }
    }

    pub fn set_strategy(&mut self, strategy: RerankStrategy) {
        self.strategy = strategy;
    }

    pub fn rerank(&self, items: Vec<(u64, f32, u64, f32)>) -> Vec<(u64, f32)> {
        // items: (id, base_score, recall_count, salience)
        let now = crate::ai_os::native::now_ns();
        let mut scored: Vec<(u64, f32)> = items.into_iter().map(|(id, base_score, recall_count, salience)| {
            let recency_score = 1.0 / (1.0 + (now as f64 / 1_000_000_000.0) as f32 * 0.001);
            let freq_score = (recall_count as f32).ln_1p() / 10.0;
            let final_score = match self.strategy {
                RerankStrategy::Recency => recency_score,
                RerankStrategy::Frequency => freq_score,
                RerankStrategy::Salience => salience,
                RerankStrategy::Hybrid => {
                    self.recency_weight * recency_score +
                    self.frequency_weight * freq_score +
                    self.salience_weight * salience +
                    (1.0 - self.recency_weight - self.frequency_weight - self.salience_weight) * base_score
                }
            };
            (id, final_score)
        }).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }
}

// =====================================================================================
// 记忆压缩 (MemoryCompression)
// =====================================================================================

/// 压缩算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    LZ4,
    Zstd,
    Snappy,
    None,
}

/// 压缩结果。
#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub original_size: u64,
    pub compressed_size: u64,
    pub ratio: f32,
    pub algorithm: CompressionAlgorithm,
}

/// 记忆压缩器：压缩记忆内容以节省存储。
pub struct MemoryCompressor {
    pub algorithm: CompressionAlgorithm,
    pub total_original: u64,
    pub total_compressed: u64,
    pub compression_count: u64,
}

impl MemoryCompressor {
    pub fn new() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            total_original: 0,
            total_compressed: 0,
            compression_count: 0,
        }
    }

    pub fn set_algorithm(&mut self, algorithm: CompressionAlgorithm) {
        self.algorithm = algorithm;
    }

    pub fn compress(&mut self, data: &[u8]) -> CompressionResult {
        let original_size = data.len() as u64;
        // 简化模拟压缩：基于数据重复性估算
        let unique_bytes: std::collections::HashSet<u8> = data.iter().copied().collect();
        let entropy_factor = unique_bytes.len() as f32 / 256.0;
        let compressed_size = (original_size as f32 * (0.3 + 0.7 * entropy_factor)) as u64;
        let ratio = compressed_size as f32 / original_size.max(1) as f32;

        self.total_original += original_size;
        self.total_compressed += compressed_size;
        self.compression_count += 1;

        CompressionResult {
            original_size,
            compressed_size,
            ratio,
            algorithm: self.algorithm,
        }
    }

    pub fn overall_ratio(&self) -> f32 {
        if self.total_original == 0 {
            1.0
        } else {
            self.total_compressed as f32 / self.total_original as f32
        }
    }

    pub fn compression_count(&self) -> u64 {
        self.compression_count
    }
}

// =====================================================================================
// 记忆扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn emotional_memory() {
        let mut em = EmotionalMemory::new();
        let id1 = em.store("快乐的日子", EmotionType::Joy, 0.8);
        let id2 = em.store("愤怒的争吵", EmotionType::Anger, 0.9);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(em.entry_count(), 2);
        let content = em.recall(id1).unwrap();
        assert_eq!(content, "快乐的日子");
        let joy_entries = em.recall_by_emotion(EmotionType::Joy);
        assert_eq!(joy_entries.len(), 1);
        let strongest = em.strongest_emotion().unwrap();
        assert_eq!(strongest, EmotionType::Anger); // 0.9 * 0.4 > 0.8 * 0.5
    }

    #[test]
    fn enhanced_association() {
        let mut an = EnhancedAssociationNetwork::new();
        an.link("apple", "fruit", 0.9, "category");
        an.link("apple", "red", 0.7, "color");
        an.link("apple", "fruit", 0.5, "category"); // 再次链接
        assert_eq!(an.link_count(), 3);
        assert_eq!(an.source_count(), 1);
        let assocs = an.get_associations("apple").unwrap();
        assert_eq!(assocs.len(), 3);
        let strongest = an.strongest_association("apple").unwrap();
        assert_eq!(strongest.target, "fruit");
        assert!((strongest.strength - 0.9).abs() < 1e-6);
    }

    #[test]
    fn memory_reranker() {
        let mut reranker = MemoryReranker::new();
        reranker.set_strategy(RerankStrategy::Hybrid);
        let items = vec![
            (1, 0.5, 10, 0.8),
            (2, 0.9, 1, 0.5),
            (3, 0.7, 5, 0.6),
        ];
        let ranked = reranker.rerank(items);
        assert_eq!(ranked.len(), 3);
        assert!(ranked[0].1 >= ranked[1].1);
        assert!(ranked[1].1 >= ranked[2].1);
    }

    #[test]
    fn memory_compressor() {
        let mut mc = MemoryCompressor::new();
        mc.set_algorithm(CompressionAlgorithm::LZ4);
        let data = vec![0u8; 1000];
        let result = mc.compress(&data);
        assert_eq!(result.original_size, 1000);
        assert!(result.ratio > 0.0);
        assert_eq!(mc.compression_count(), 1);
        assert!(mc.overall_ratio() > 0.0);
    }

    #[test]
    fn emotion_types() {
        assert_eq!(EmotionType::Joy as u32, 0);
        assert_eq!(EmotionType::Disgust as u32, 5);
    }

    #[test]
    fn rerank_strategies() {
        assert_eq!(RerankStrategy::Recency as u32, 0);
        assert_eq!(RerankStrategy::Hybrid as u32, 3);
    }

    #[test]
    fn compression_algorithms() {
        assert_eq!(CompressionAlgorithm::LZ4 as u32, 0);
        assert_eq!(CompressionAlgorithm::None as u32, 3);
    }
}


