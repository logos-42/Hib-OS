//! 离线巩固层 —— AI 原生 OS 的「梦境与巩固」 (Dream & Consolidation)
//!
//! 本模块实现 AI 原生 OS 的离线记忆巩固与创造性联想，对应传统 OS 中的
//! 「后台任务 / 定时维护 / 日志轮转」概念，但重新映射到梦境域：
//!
//! * DreamCycle (梦境周期) —— 离线处理周期，等价于后台维护窗口；
//! * MemoryReplay (记忆重播) —— 将短时记忆重新激活并巩固，等价于日志回放；
//! * MemorySynthesis (记忆合成) —— 将碎片记忆合成为新知识，等价于数据压缩；
//! * CreativeAssociation (创造性联想) —— 远距离概念关联，等价于创新引擎；
//! * Insight (洞察) —— 从巩固中产生的新认知，等价于系统优化建议；
//! * DreamEngine (梦境引擎) —— 统一入口，通过 dream://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 梦境周期 (DreamCycle)
// =====================================================================================

/// 梦境阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DreamStage {
    N1,    // 浅睡
    N2,    // 稳定睡眠
    N3,    // 深睡（慢波睡眠，记忆巩固主要阶段）
    REM,   // 快速眼动期（做梦，创造性联想）
    Wake,  // 唤醒
}

/// 梦境周期：离线处理的周期性循环。
pub struct DreamCycle {
    pub stage: DreamStage,
    pub cycle_count: u64,
    pub stage_duration_ns: u128,
    pub total_duration_ns: u128,
    pub started_at: Option<u128>,
}

impl DreamCycle {
    pub fn new() -> Self {
        Self {
            stage: DreamStage::Wake,
            cycle_count: 0,
            stage_duration_ns: 0,
            total_duration_ns: 0,
            started_at: None,
        }
    }

    pub fn enter_stage(&mut self, stage: DreamStage, now_ns: u128) {
        if let Some(started) = self.started_at {
            self.stage_duration_ns = now_ns - started;
            self.total_duration_ns += self.stage_duration_ns;
        }
        self.stage = stage;
        self.started_at = Some(now_ns);
        if stage == DreamStage::Wake && self.started_at.is_some() {
            self.cycle_count += 1;
        }
    }

    pub fn current_stage(&self) -> DreamStage {
        self.stage
    }

    pub fn cycle_count(&self) -> u64 {
        self.cycle_count
    }
}

// =====================================================================================
// 记忆重播 (MemoryReplay)
// =====================================================================================

/// 重播事件：从记忆中提取并重新激活的事件。
#[derive(Clone, Debug)]
pub struct ReplayEvent {
    pub id: u64,
    pub source_memory_id: u64,
    pub content: String,
    pub salience: f32,
    pub replay_count: u64,
    pub last_replayed_ns: Option<u128>,
}

/// 记忆重播：将短时记忆重新激活并巩固到长时记忆。
pub struct MemoryReplay {
    replay_queue: VecDeque<ReplayEvent>,
    next_id: u64,
    pub total_replayed: u64,
    pub max_queue_size: usize,
}

impl MemoryReplay {
    pub fn new() -> Self {
        Self {
            replay_queue: VecDeque::new(),
            next_id: 1,
            total_replayed: 0,
            max_queue_size: 10000,
        }
    }

    pub fn enqueue(&mut self, source_memory_id: u64, content: &str, salience: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if self.replay_queue.len() >= self.max_queue_size {
            self.replay_queue.pop_front();
        }
        self.replay_queue.push_back(ReplayEvent {
            id,
            source_memory_id,
            content: content.to_string(),
            salience: salience.clamp(0.0, 1.0),
            replay_count: 0,
            last_replayed_ns: None,
        });
        id
    }

    pub fn replay_next(&mut self, now_ns: u128) -> Option<ReplayEvent> {
        if let Some(mut event) = self.replay_queue.pop_front() {
            event.replay_count += 1;
            event.last_replayed_ns = Some(now_ns);
            self.total_replayed += 1;
            Some(event)
        } else {
            None
        }
    }

    pub fn queue_len(&self) -> usize {
        self.replay_queue.len()
    }

    pub fn replay_count(&self) -> u64 {
        self.total_replayed
    }
}

// =====================================================================================
// 记忆合成 (MemorySynthesis)
// =====================================================================================

/// 合成知识：从多个记忆片段合成的新知识单元。
#[derive(Clone, Debug)]
pub struct SynthesizedKnowledge {
    pub id: u64,
    pub source_ids: Vec<u64>,
    pub concept: String,
    pub confidence: f32,
    pub created_at: u128,
}

/// 记忆合成：将碎片记忆合成为新知识。
pub struct MemorySynthesis {
    knowledge: BTreeMap<u64, SynthesizedKnowledge>,
    next_id: u64,
    pub synthesis_count: u64,
}

impl MemorySynthesis {
    pub fn new() -> Self {
        Self {
            knowledge: BTreeMap::new(),
            next_id: 1,
            synthesis_count: 0,
        }
    }

    pub fn synthesize(
        &mut self,
        source_ids: Vec<u64>,
        concept: &str,
        confidence: f32,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.knowledge.insert(
            id,
            SynthesizedKnowledge {
                id,
                source_ids,
                concept: concept.to_string(),
                confidence: confidence.clamp(0.0, 1.0),
                created_at: crate::ai_os::native::now_ns(),
            },
        );
        self.synthesis_count += 1;
        id
    }

    pub fn get(&self, id: u64) -> Option<&SynthesizedKnowledge> {
        self.knowledge.get(&id)
    }

    pub fn count(&self) -> usize {
        self.knowledge.len()
    }

    pub fn top_confidence(&self, k: usize) -> Vec<&SynthesizedKnowledge> {
        let mut v: Vec<&SynthesizedKnowledge> = self.knowledge.values().collect();
        v.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }
}

// =====================================================================================
// 创造性联想 (CreativeAssociation)
// =====================================================================================

/// 联想链：两个看似不相关概念间的关联路径。
#[derive(Clone, Debug)]
pub struct AssociationChain {
    pub id: u64,
    pub source: String,
    pub target: String,
    pub intermediate: Vec<String>,
    pub strength: f32,
    pub novelty: f32,   // 0~1，新颖度
}

/// 创造性联想：远距离概念关联，产生新洞察。
pub struct CreativeAssociation {
    associations: BTreeMap<u64, AssociationChain>,
    next_id: u64,
    pub association_count: u64,
}

impl CreativeAssociation {
    pub fn new() -> Self {
        Self {
            associations: BTreeMap::new(),
            next_id: 1,
            association_count: 0,
        }
    }

    pub fn associate(
        &mut self,
        source: &str,
        target: &str,
        intermediate: &[&str],
        strength: f32,
        novelty: f32,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.associations.insert(
            id,
            AssociationChain {
                id,
                source: source.to_string(),
                target: target.to_string(),
                intermediate: intermediate.iter().map(|s| s.to_string()).collect(),
                strength: strength.clamp(0.0, 1.0),
                novelty: novelty.clamp(0.0, 1.0),
            },
        );
        self.association_count += 1;
        id
    }

    pub fn get(&self, id: u64) -> Option<&AssociationChain> {
        self.associations.get(&id)
    }

    pub fn count(&self) -> usize {
        self.associations.len()
    }

    pub fn top_novelty(&self, k: usize) -> Vec<&AssociationChain> {
        let mut v: Vec<&AssociationChain> = self.associations.values().collect();
        v.sort_by(|a, b| b.novelty.partial_cmp(&a.novelty).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }
}

// =====================================================================================
// 洞察 (Insight)
// =====================================================================================

/// 洞察：从梦境巩固中产生的新认知。
#[derive(Clone, Debug)]
pub struct Insight {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub source_dream_stage: DreamStage,
    pub confidence: f32,
    pub impact: f32,      // 潜在影响 0~1
    pub created_at: u128,
}

/// 洞察库。
pub struct InsightLibrary {
    insights: BTreeMap<u64, Insight>,
    next_id: u64,
}

impl InsightLibrary {
    pub fn new() -> Self {
        Self {
            insights: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add(
        &mut self,
        title: &str,
        content: &str,
        stage: DreamStage,
        confidence: f32,
        impact: f32,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.insights.insert(
            id,
            Insight {
                id,
                title: title.to_string(),
                content: content.to_string(),
                source_dream_stage: stage,
                confidence: confidence.clamp(0.0, 1.0),
                impact: impact.clamp(0.0, 1.0),
                created_at: crate::ai_os::native::now_ns(),
            },
        );
        id
    }

    pub fn get(&self, id: u64) -> Option<&Insight> {
        self.insights.get(&id)
    }

    pub fn count(&self) -> usize {
        self.insights.len()
    }

    pub fn top_impact(&self, k: usize) -> Vec<&Insight> {
        let mut v: Vec<&Insight> = self.insights.values().collect();
        v.sort_by(|a, b| b.impact.partial_cmp(&a.impact).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }
}

// =====================================================================================
// 梦境引擎 (DreamEngine) —— 统一入口
// =====================================================================================

/// 梦境引擎：整合梦境周期、记忆重播、记忆合成、创造性联想、洞察库。
pub struct DreamEngine {
    pub cycle: DreamCycle,
    pub replay: MemoryReplay,
    pub synthesis: MemorySynthesis,
    pub association: CreativeAssociation,
    pub insights: InsightLibrary,
    pub enabled: bool,
    pub total_cycles: u64,
}

impl DreamEngine {
    pub fn new() -> Self {
        Self {
            cycle: DreamCycle::new(),
            replay: MemoryReplay::new(),
            synthesis: MemorySynthesis::new(),
            association: CreativeAssociation::new(),
            insights: InsightLibrary::new(),
            enabled: true,
            total_cycles: 0,
        }
    }

    pub fn start_cycle(&mut self, now_ns: u128) {
        self.cycle.enter_stage(DreamStage::N1, now_ns);
        self.total_cycles += 1;
    }

    pub fn advance_stage(&mut self, stage: DreamStage, now_ns: u128) {
        self.cycle.enter_stage(stage, now_ns);
    }

    pub fn end_cycle(&mut self, now_ns: u128) {
        self.cycle.enter_stage(DreamStage::Wake, now_ns);
    }

    pub fn enqueue_replay(&mut self, source_memory_id: u64, content: &str, salience: f32) -> u64 {
        self.replay.enqueue(source_memory_id, content, salience)
    }

    pub fn replay_next(&mut self, now_ns: u128) -> Option<String> {
        self.replay.replay_next(now_ns).map(|e| e.content)
    }

    pub fn synthesize_knowledge(
        &mut self,
        source_ids: Vec<u64>,
        concept: &str,
        confidence: f32,
    ) -> u64 {
        self.synthesis.synthesize(source_ids, concept, confidence)
    }

    pub fn create_association(
        &mut self,
        source: &str,
        target: &str,
        intermediate: &[&str],
        strength: f32,
        novelty: f32,
    ) -> u64 {
        self.association.associate(source, target, intermediate, strength, novelty)
    }

    pub fn add_insight(
        &mut self,
        title: &str,
        content: &str,
        stage: DreamStage,
        confidence: f32,
        impact: f32,
    ) -> u64 {
        self.insights.add(title, content, stage, confidence, impact)
    }

    pub fn cycle_count(&self) -> u64 {
        self.cycle.cycle_count()
    }

    pub fn current_stage(&self) -> DreamStage {
        self.cycle.current_stage()
    }

    pub fn replay_count(&self) -> u64 {
        self.replay.replay_count()
    }

    pub fn synthesis_count(&self) -> usize {
        self.synthesis.count()
    }

    pub fn association_count(&self) -> usize {
        self.association.count()
    }

    pub fn insight_count(&self) -> usize {
        self.insights.count()
    }
}

impl Default for DreamEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static DREAM: OnceLock<Mutex<DreamEngine>> = OnceLock::new();

pub fn dream_engine() -> &'static Mutex<DreamEngine> {
    DREAM.get_or_init(|| Mutex::new(DreamEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = dream_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = dream_engine().lock().unwrap();
    format!(
        "dream: cycles={} stage={:?} replays={} syntheses={} associations={} insights={} total_cycles={}",
        eng.cycle_count(),
        eng.current_stage(),
        eng.replay_count(),
        eng.synthesis_count(),
        eng.association_count(),
        eng.insight_count(),
        eng.total_cycles
    )
}

pub fn dream_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn dream_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("start-cycle") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = dream_engine().lock().unwrap();
            eng.start_cycle(now);
            Ok(format!("已开始梦境周期 now={} 周期={}", now, eng.total_cycles))
        }
        Some("advance-stage") => {
            let stage_str = it.next().unwrap_or("n1");
            let stage = match stage_str {
                "n1" => DreamStage::N1,
                "n2" => DreamStage::N2,
                "n3" => DreamStage::N3,
                "rem" => DreamStage::REM,
                "wake" => DreamStage::Wake,
                _ => DreamStage::N1,
            };
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = dream_engine().lock().unwrap();
            eng.advance_stage(stage, now);
            Ok(format!("已进入梦境阶段 {:?} now={}", stage, now))
        }
        Some("end-cycle") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = dream_engine().lock().unwrap();
            eng.end_cycle(now);
            Ok(format!("已结束梦境周期 now={}", now))
        }
        Some("enqueue-replay") => {
            let source_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少源记忆id")?;
            let content: String = it.collect::<Vec<&str>>().join(" ");
            let salience = 0.5;
            let mut eng = dream_engine().lock().unwrap();
            let id = eng.enqueue_replay(source_id, &content, salience);
            Ok(format!("已入队记忆重播 id={} 内容={}", id, content))
        }
        Some("replay-next") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = dream_engine().lock().unwrap();
            match eng.replay_next(now) {
                Some(content) => Ok(format!("重播内容: {}", content)),
                None => Ok("重播队列为空".into()),
            }
        }
        Some("synthesize") => {
            let concept: String = it.collect::<Vec<&str>>().join(" ");
            if concept.is_empty() {
                return Err("缺少概念".into());
            }
            let mut eng = dream_engine().lock().unwrap();
            let id = eng.synthesize_knowledge(vec![1, 2], &concept, 0.8);
            Ok(format!("已合成知识 id={} 概念={}", id, concept))
        }
        Some("associate") => {
            let source = it.next().ok_or("缺少源概念")?;
            let target = it.next().ok_or("缺少目标概念")?;
            let intermediate: Vec<&str> = it.collect();
            let strength = 0.5;
            let novelty = 0.7;
            let mut eng = dream_engine().lock().unwrap();
            let id = eng.create_association(source, target, &intermediate, strength, novelty);
            Ok(format!("已创建联想 id={} {} ~ {} 新颖度={:.2}", id, source, target, novelty))
        }
        Some("add-insight") => {
            let title = it.next().ok_or("缺少标题")?;
            let content: String = it.collect::<Vec<&str>>().join(" ");
            let stage = DreamStage::REM;
            let mut eng = dream_engine().lock().unwrap();
            let id = eng.add_insight(title, &content, stage, 0.8, 0.6);
            Ok(format!("已添加洞察 id={} 标题={}", id, title))
        }
        Some("top-insights") => {
            let k = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(5);
            let eng = dream_engine().lock().unwrap();
            let insights = eng.insights.top_impact(k);
            let mut out = String::from("高影响洞察:\n");
            for ins in insights {
                out.push_str(&format!("  id={} 标题={} 影响={:.2} 置信度={:.2}\n", ins.id, ins.title, ins.impact, ins.confidence));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 dream 动作: {}", other)),
    }
}

pub fn dream_act_dispatch(payload: &str) -> Result<String, String> {
    dream_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dream_cycle_stage() {
        let mut cycle = DreamCycle::new();
        cycle.enter_stage(DreamStage::N1, 1000);
        assert_eq!(cycle.current_stage(), DreamStage::N1);
        cycle.enter_stage(DreamStage::REM, 2000);
        assert_eq!(cycle.current_stage(), DreamStage::REM);
        cycle.enter_stage(DreamStage::Wake, 3000);
        assert_eq!(cycle.current_stage(), DreamStage::Wake);
        assert_eq!(cycle.cycle_count(), 1);
    }

    #[test]
    fn dream_cycle_duration() {
        let mut cycle = DreamCycle::new();
        cycle.enter_stage(DreamStage::N1, 1000);
        cycle.enter_stage(DreamStage::N2, 5000);
        assert_eq!(cycle.stage_duration_ns, 4000);
        assert_eq!(cycle.total_duration_ns, 4000);
    }

    #[test]
    fn memory_replay_enqueue() {
        let mut replay = MemoryReplay::new();
        let id = replay.enqueue(1, "test memory", 0.8);
        assert_eq!(id, 1);
        assert_eq!(replay.queue_len(), 1);
    }

    #[test]
    fn memory_replay_next() {
        let mut replay = MemoryReplay::new();
        replay.enqueue(1, "memory1", 0.8);
        replay.enqueue(2, "memory2", 0.6);
        let event = replay.replay_next(1000).unwrap();
        assert_eq!(event.content, "memory1");
        assert_eq!(event.replay_count, 1);
        assert_eq!(event.last_replayed_ns, Some(1000));
        assert_eq!(replay.replay_count(), 1);
    }

    #[test]
    fn memory_synthesis() {
        let mut synth = MemorySynthesis::new();
        let id = synth.synthesize(vec![1, 2, 3], "new concept", 0.9);
        assert_eq!(id, 1);
        let knowledge = synth.get(id).unwrap();
        assert_eq!(knowledge.concept, "new concept");
        assert!((knowledge.confidence - 0.9).abs() < 1e-6);
        assert_eq!(synth.count(), 1);
    }

    #[test]
    fn synthesis_top_confidence() {
        let mut synth = MemorySynthesis::new();
        synth.synthesize(vec![1], "low", 0.3);
        synth.synthesize(vec![2], "high", 0.9);
        synth.synthesize(vec![3], "mid", 0.6);
        let top = synth.top_confidence(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].concept, "high");
    }

    #[test]
    fn creative_association() {
        let mut assoc = CreativeAssociation::new();
        let id = assoc.associate("apple", "computer", &["fruit", "technology"], 0.7, 0.8);
        assert_eq!(id, 1);
        let chain = assoc.get(id).unwrap();
        assert_eq!(chain.source, "apple");
        assert_eq!(chain.target, "computer");
        assert_eq!(chain.intermediate, vec!["fruit", "technology"]);
        assert_eq!(assoc.count(), 1);
    }

    #[test]
    fn association_top_novelty() {
        let mut assoc = CreativeAssociation::new();
        assoc.associate("a", "b", &[], 0.5, 0.3);
        assoc.associate("c", "d", &[], 0.5, 0.9);
        let top = assoc.top_novelty(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].source, "c");
    }

    #[test]
    fn insight_library() {
        let mut lib = InsightLibrary::new();
        let id = lib.add("insight1", "content1", DreamStage::REM, 0.8, 0.7);
        assert_eq!(id, 1);
        let insight = lib.get(id).unwrap();
        assert_eq!(insight.title, "insight1");
        assert_eq!(insight.source_dream_stage, DreamStage::REM);
        assert_eq!(lib.count(), 1);
    }

    #[test]
    fn insight_top_impact() {
        let mut lib = InsightLibrary::new();
        lib.add("low", "c1", DreamStage::N1, 0.5, 0.2);
        lib.add("high", "c2", DreamStage::REM, 0.8, 0.9);
        let top = lib.top_impact(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].title, "high");
    }

    #[test]
    fn dream_engine_cycle() {
        let mut eng = DreamEngine::new();
        eng.start_cycle(1000);
        assert_eq!(eng.cycle_count(), 1);
        assert_eq!(eng.current_stage(), DreamStage::N1);
        eng.advance_stage(DreamStage::REM, 2000);
        assert_eq!(eng.current_stage(), DreamStage::REM);
        eng.end_cycle(3000);
        assert_eq!(eng.current_stage(), DreamStage::Wake);
    }

    #[test]
    fn dream_engine_replay() {
        let mut eng = DreamEngine::new();
        eng.enqueue_replay(1, "memory content", 0.8);
        assert_eq!(eng.replay_count(), 0); // 还没 replay
        let content = eng.replay_next(1000).unwrap();
        assert_eq!(content, "memory content");
        assert_eq!(eng.replay_count(), 1);
    }

    #[test]
    fn dream_engine_synthesize() {
        let mut eng = DreamEngine::new();
        let id = eng.synthesize_knowledge(vec![1, 2], "dream concept", 0.9);
        assert_eq!(id, 1);
        assert_eq!(eng.synthesis_count(), 1);
    }

    #[test]
    fn dream_engine_associate() {
        let mut eng = DreamEngine::new();
        let id = eng.create_association("cat", "dog", &["pet"], 0.6, 0.7);
        assert_eq!(id, 1);
        assert_eq!(eng.association_count(), 1);
    }

    #[test]
    fn dream_engine_insight() {
        let mut eng = DreamEngine::new();
        let id = eng.add_insight("梦境洞察", "内容", DreamStage::REM, 0.8, 0.6);
        assert_eq!(id, 1);
        assert_eq!(eng.insight_count(), 1);
    }

    #[test]
    fn dream_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("dream:"));
        assert!(s.contains("cycles="));
    }

    #[test]
    fn dream_act_start_cycle() {
        init();
        let res = dream_act("start-cycle 1000").unwrap();
        assert!(res.contains("已开始梦境周期"));
    }

    #[test]
    fn dream_act_advance_stage() {
        init();
        let res = dream_act("advance-stage rem 2000").unwrap();
        assert!(res.contains("已进入梦境阶段"));
    }

    #[test]
    fn dream_act_end_cycle() {
        init();
        let res = dream_act("end-cycle 3000").unwrap();
        assert!(res.contains("已结束梦境周期"));
    }

    #[test]
    fn dream_act_enqueue_replay() {
        init();
        let res = dream_act("enqueue-replay 1 记忆内容").unwrap();
        assert!(res.contains("已入队记忆重播"));
    }

    #[test]
    fn dream_act_replay_next() {
        init();
        dream_act("enqueue-replay 1 内容").unwrap();
        let res = dream_act("replay-next 1000").unwrap();
        assert!(res.contains("重播内容"));
    }

    #[test]
    fn dream_act_synthesize() {
        init();
        let res = dream_act("synthesize 新概念").unwrap();
        assert!(res.contains("已合成知识"));
    }

    #[test]
    fn dream_act_associate() {
        init();
        let res = dream_act("associate cat dog pet").unwrap();
        assert!(res.contains("已创建联想"));
    }

    #[test]
    fn dream_act_add_insight() {
        init();
        let res = dream_act("add-insight 洞察标题 内容").unwrap();
        assert!(res.contains("已添加洞察"));
    }

    #[test]
    fn dream_act_top_insights() {
        init();
        dream_act("add-insight 高影响 内容").unwrap();
        let res = dream_act("top-insights 3").unwrap();
        assert!(res.contains("高影响洞察"));
    }

    #[test]
    fn dream_act_status() {
        init();
        let res = dream_act("status").unwrap();
        assert!(res.contains("dream:"));
    }

    #[test]
    fn dream_stages() {
        assert_eq!(DreamStage::N1 as u32, 0);
        assert_eq!(DreamStage::REM as u32, 3);
        assert_eq!(DreamStage::Wake as u32, 4);
    }
}

// =====================================================================================
// 梦境叙事 (DreamNarrative)
// =====================================================================================

/// 梦境元素。
#[derive(Clone, Debug)]
pub struct DreamElement {
    pub id: u64,
    pub element_type: String,
    pub content: String,
    pub salience: f32,
    pub connected_to: Vec<u64>,
}

/// 梦境叙事：梦境中的故事线。
pub struct DreamNarrative {
    pub id: u64,
    pub title: String,
    pub elements: Vec<DreamElement>,
    pub coherence: f32,
    pub emotional_arc: Vec<f32>,
    pub created_at: u128,
}

/// 梦境叙事构建器。
pub struct DreamNarrativeBuilder {
    pub narratives: BTreeMap<u64, DreamNarrative>,
    pub next_id: u64,
}

impl DreamNarrativeBuilder {
    pub fn new() -> Self {
        Self {
            narratives: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, title: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.narratives.insert(id, DreamNarrative {
            id,
            title: title.to_string(),
            elements: Vec::new(),
            coherence: 0.0,
            emotional_arc: Vec::new(),
            created_at: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn add_element(&mut self, narrative_id: u64, element_type: &str, content: &str, salience: f32) -> u64 {
        if let Some(narrative) = self.narratives.get_mut(&narrative_id) {
            let element_id = narrative.elements.len() as u64 + 1;
            narrative.elements.push(DreamElement {
                id: element_id,
                element_type: element_type.to_string(),
                content: content.to_string(),
                salience: salience.clamp(0.0, 1.0),
                connected_to: Vec::new(),
            });
            return element_id;
        }
        0
    }

    pub fn connect_elements(&mut self, narrative_id: u64, from: u64, to: u64) -> bool {
        if let Some(narrative) = self.narratives.get_mut(&narrative_id) {
            if let Some(elem) = narrative.elements.iter_mut().find(|e| e.id == from) {
                if !elem.connected_to.contains(&to) {
                    elem.connected_to.push(to);
                    return true;
                }
            }
        }
        false
    }

    pub fn compute_coherence(&mut self, narrative_id: u64) -> f32 {
        if let Some(narrative) = self.narratives.get_mut(&narrative_id) {
            if narrative.elements.is_empty() {
                return 0.0;
            }
            let total_connections: usize = narrative.elements.iter().map(|e| e.connected_to.len()).sum();
            let max_possible = narrative.elements.len() * (narrative.elements.len() - 1);
            let coherence = if max_possible > 0 {
                total_connections as f32 / max_possible as f32
            } else {
                0.0
            };
            narrative.coherence = coherence.clamp(0.0, 1.0);
            narrative.coherence
        } else {
            0.0
        }
    }

    pub fn narrative_count(&self) -> usize {
        self.narratives.len()
    }
}

// =====================================================================================
// 创造力评估 (CreativityAssessment)
// =====================================================================================

/// 创造力维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreativityDimension {
    Novelty,      // 新颖度
    Usefulness,   // 有用性
    Surprise,     // 惊喜度
    Elegance,     // 优雅度
}

/// 创造力评估结果。
#[derive(Clone, Debug)]
pub struct CreativityAssessment {
    pub id: u64,
    pub dimensions: BTreeMap<CreativityDimension, f32>,
    pub overall_score: f32,
    pub timestamp_ns: u128,
}

/// 创造力评估器：评估梦境产出的创造力。
pub struct CreativityAssessor {
    pub assessments: Vec<CreativityAssessment>,
    pub next_id: u64,
}

impl CreativityAssessor {
    pub fn new() -> Self {
        Self {
            assessments: Vec::new(),
            next_id: 1,
        }
    }

    pub fn assess(&mut self, dimensions: BTreeMap<CreativityDimension, f32>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let overall: f32 = dimensions.values().sum::<f32>() / dimensions.len().max(1) as f32;
        self.assessments.push(CreativityAssessment {
            id,
            dimensions,
            overall_score: overall.clamp(0.0, 1.0),
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn get_assessment(&self, id: u64) -> Option<&CreativityAssessment> {
        self.assessments.iter().find(|a| a.id == id)
    }

    pub fn assessment_count(&self) -> usize {
        self.assessments.len()
    }

    pub fn avg_creativity(&self) -> f32 {
        if self.assessments.is_empty() {
            0.0
        } else {
            self.assessments.iter().map(|a| a.overall_score).sum::<f32>() / self.assessments.len() as f32
        }
    }
}

// =====================================================================================
// 记忆整合 (MemoryConsolidationDream)
// =====================================================================================

/// 整合类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsolidationType {
    Synaptic,     // 突触整合
    Systems,      // 系统整合
    Reconsolidation, // 再巩固
}

/// 记忆整合记录。
#[derive(Clone, Debug)]
pub struct ConsolidationRecord {
    pub id: u64,
    pub source_memory_id: u64,
    pub consolidation_type: ConsolidationType,
    pub strength_change: f32,
    pub timestamp_ns: u128,
}

/// 记忆整合器：梦境中的记忆整合过程。
pub struct MemoryConsolidator {
    pub records: Vec<ConsolidationRecord>,
    pub next_id: u64,
    pub total_consolidated: u64,
}

impl MemoryConsolidator {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            total_consolidated: 0,
        }
    }

    pub fn consolidate(&mut self, source_memory_id: u64, r#type: ConsolidationType, strength_change: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push(ConsolidationRecord {
            id,
            source_memory_id,
            consolidation_type: r#type,
            strength_change: strength_change.clamp(-1.0, 1.0),
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        self.total_consolidated += 1;
        id
    }

    pub fn get_record(&self, id: u64) -> Option<&ConsolidationRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn avg_strength_change(&self) -> f32 {
        if self.records.is_empty() {
            0.0
        } else {
            self.records.iter().map(|r| r.strength_change).sum::<f32>() / self.records.len() as f32
        }
    }
}

// =====================================================================================
// 梦境分析 (DreamAnalysis)
// =====================================================================================

/// 分析维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisDimension {
    Emotional,    // 情感维度
    Symbolic,     // 象征维度
    Narrative,    // 叙事维度
    Cognitive,    // 认知维度
}

/// 梦境分析结果。
#[derive(Clone, Debug)]
pub struct DreamAnalysis {
    pub id: u64,
    pub dream_id: u64,
    pub dimensions: BTreeMap<AnalysisDimension, f32>,
    pub insights: Vec<String>,
    pub created_at: u128,
}

/// 梦境分析器：分析梦境内容和意义。
pub struct DreamAnalyzer {
    pub analyses: BTreeMap<u64, DreamAnalysis>,
    pub next_id: u64,
}

impl DreamAnalyzer {
    pub fn new() -> Self {
        Self {
            analyses: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn analyze(&mut self, dream_id: u64, dimensions: BTreeMap<AnalysisDimension, f32>, insights: Vec<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.analyses.insert(id, DreamAnalysis {
            id,
            dream_id,
            dimensions,
            insights,
            created_at: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn get_analysis(&self, id: u64) -> Option<&DreamAnalysis> {
        self.analyses.get(&id)
    }

    pub fn analysis_count(&self) -> usize {
        self.analyses.len()
    }
}

// =====================================================================================
// 梦境扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn dream_narrative() {
        let mut builder = DreamNarrativeBuilder::new();
        let id = builder.create("飞行梦境".into());
        assert_eq!(id, 1);
        let elem_id = builder.add_element(id, "场景", "天空", 0.8).unwrap();
        assert_eq!(elem_id, 1);
        builder.add_element(id, "动作", "飞翔", 0.9).unwrap();
        builder.connect_elements(id, 1, 2).unwrap();
        let coherence = builder.compute_coherence(id);
        assert!(coherence >= 0.0);
        assert_eq!(builder.narrative_count(), 1);
    }

    #[test]
    fn creativity_assessment() {
        let mut assessor = CreativityAssessor::new();
        let mut dims = BTreeMap::new();
        dims.insert(CreativityDimension::Novelty, 0.8);
        dims.insert(CreativityDimension::Usefulness, 0.6);
        dims.insert(CreativityDimension::Surprise, 0.7);
        let id = assessor.assess(dims);
        assert_eq!(id, 1);
        let assessment = assessor.get_assessment(id).unwrap();
        assert!(assessment.overall_score > 0.5);
        assert_eq!(assessor.assessment_count(), 1);
        assert!(assessor.avg_creativity() > 0.5);
    }

    #[test]
    fn memory_consolidator() {
        let mut consolidator = MemoryConsolidator::new();
        let id = consolidator.consolidate(1, ConsolidationType::Synaptic, 0.3).unwrap();
        assert_eq!(id, 1);
        let record = consolidator.get_record(id).unwrap();
        assert_eq!(record.source_memory_id, 1);
        assert!((record.strength_change - 0.3).abs() < 1e-6);
        assert_eq!(consolidator.record_count(), 1);
        assert_eq!(consolidator.total_consolidated, 1);
    }

    #[test]
    fn dream_analyzer() {
        let mut analyzer = DreamAnalyzer::new();
        let mut dims = BTreeMap::new();
        dims.insert(AnalysisDimension::Emotional, 0.7);
        dims.insert(AnalysisDimension::Symbolic, 0.5);
        let id = analyzer.analyze(1, dims, vec!["恐惧反映现实压力".into()]);
        assert_eq!(id, 1);
        let analysis = analyzer.get_analysis(id).unwrap();
        assert_eq!(analysis.insights.len(), 1);
        assert_eq!(analyzer.analysis_count(), 1);
    }

    #[test]
    fn dream_element_types() {
        // 测试 DreamStage 枚举
        assert_eq!(DreamStage::N1 as u32, 0);
        assert_eq!(DreamStage::Wake as u32, 4);
    }

    #[test]
    fn creativity_dimensions() {
        assert_eq!(CreativityDimension::Novelty as u32, 0);
        assert_eq!(CreativityDimension::Elegance as u32, 3);
    }

    #[test]
    fn consolidation_types() {
        assert_eq!(ConsolidationType::Synaptic as u32, 0);
        assert_eq!(ConsolidationType::Reconsolidation as u32, 2);
    }

    #[test]
    fn analysis_dimensions() {
        assert_eq!(AnalysisDimension::Emotional as u32, 0);
        assert_eq!(AnalysisDimension::Cognitive as u32, 3);
    }
}
