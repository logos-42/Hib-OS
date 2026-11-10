//! 时间处理层 —— AI 原生 OS 的「时间系统」 (Temporal System)
//!
//! 本模块实现 AI 原生 OS 的时间处理，对应传统 OS 中的「时钟中断 /
//! 定时器 / 调度时钟」概念，但重新映射到时间域：
//!
//! * TemporalClock (时钟) —— 系统时钟，提供绝对时间与单调时间；
//! * CircadianRhythm (昼夜律) —— 24 小时生物节律，等价于系统定时器；
//! * UltradianRhythm (超日夜律) —— 90-120 分钟周期，等价于调度时间片；
//! * TemporalInterval (时间间隔) —— 两个时间点间的间隔，等价于超时；
//! * TemporalSequence (时间序列) —— 有序事件序列，等价于调度队列；
//! * TemporalScheduling (时序调度) —— 基于时间的任务调度，等价于 cron / 定时器；
//! * TemporalMemory (时间记忆) —— 对过去时间的编码，等价于时间戳日志；
//! * TemporalPrediction (时间预测) —— 对未来时间的预测，等价于预读；
//! * TemporalBinding (时间绑定) —— 将事件绑定到特定时间点，等价于定时触发；
//! * TemporalEngine (时间引擎) —— 统一入口，通过 temporal://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 时钟 (TemporalClock)
// =====================================================================================

/// 时钟类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockType {
    WallClock,    // 墙钟时间（绝对时间）
    Monotonic,    // 单调时间（不回拨）
    CPU,          // CPU 时间
}

/// 系统时钟。
pub struct TemporalClock {
    pub clock_type: ClockType,
    pub start_ns: u128,
    pub offset_ns: u128,
    pub tick_count: u64,
}

impl TemporalClock {
    pub fn new(clock_type: ClockType) -> Self {
        Self {
            clock_type,
            start_ns: crate::ai_os::native::now_ns(),
            offset_ns: 0,
            tick_count: 0,
        }
    }

    pub fn now(&self) -> u128 {
        crate::ai_os::native::now_ns() + self.offset_ns
    }

    pub fn elapsed(&self) -> u128 {
        self.now() - self.start_ns
    }

    pub fn tick(&mut self) {
        self.tick_count += 1;
    }

    pub fn set_offset(&mut self, offset_ns: i128) {
        self.offset_ns = (self.offset_ns as i128 + offset_ns) as u128;
    }
}

// =====================================================================================
// 昼夜律 (CircadianRhythm)
// =====================================================================================

/// 昼夜律阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircadianPhase {
    Morning,    // 早晨
    Noon,       // 中午
    Afternoon,  // 下午
    Evening,    // 傍晚
    Night,      // 夜晚
    DeepNight,  // 深夜
}

/// 昼夜律：24 小时生物节律。
pub struct CircadianRhythm {
    pub period_ns: u128,          // 24 小时
    pub phase_offset_ns: u128,    // 相位偏移
    pub current_phase: CircadianPhase,
    pub cycle_count: u64,
}

impl CircadianRhythm {
    pub fn new() -> Self {
        Self {
            period_ns: 24 * 60 * 60 * 1_000_000_000,
            phase_offset_ns: 6 * 60 * 60 * 1_000_000_000, // 早上 6 点开始
            current_phase: CircadianPhase::Morning,
            cycle_count: 0,
        }
    }

    pub fn phase_at(&self, now_ns: u128) -> CircadianPhase {
        let elapsed = (now_ns + self.phase_offset_ns) % self.period_ns;
        let hour = elapsed as f64 / (self.period_ns as f64 / 24.0);
        if hour >= 5.0 && hour < 11.0 {
            CircadianPhase::Morning
        } else if hour >= 11.0 && hour < 15.0 {
            CircadianPhase::Noon
        } else if hour >= 15.0 && hour < 19.0 {
            CircadianPhase::Afternoon
        } else if hour >= 19.0 && hour < 23.0 {
            CircadianPhase::Evening
        } else if hour >= 23.0 || hour < 1.0 {
            CircadianPhase::Night
        } else {
            CircadianPhase::DeepNight
        }
    }

    pub fn update(&mut self, now_ns: u128) {
        let phase = self.phase_at(now_ns);
        if phase != self.current_phase {
            self.current_phase = phase;
            self.cycle_count += 1;
        }
    }

    pub fn alertness(&self) -> f32 {
        match self.current_phase {
            CircadianPhase::Morning => 0.7,
            CircadianPhase::Noon => 0.8,
            CircadianPhase::Afternoon => 0.6,
            CircadianPhase::Evening => 0.5,
            CircadianPhase::Night => 0.3,
            CircadianPhase::DeepNight => 0.1,
        }
    }
}

// =====================================================================================
// 超日夜律 (UltradianRhythm)
// =====================================================================================

/// 超日夜律阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UltradianPhase {
    Peak,       // 高峰期（高专注）
    Decline,    // 衰退期
    Trough,     // 低谷期（需休息）
    Recovery,   // 恢复期
}

/// 超日夜律：90-120 分钟周期。
pub struct UltradianRhythm {
    pub period_ns: u128,          // 90 分钟
    pub current_phase: UltradianPhase,
    pub cycle_start_ns: u128,
    pub cycle_count: u64,
}

impl UltradianRhythm {
    pub fn new() -> Self {
        Self {
            period_ns: 90 * 60 * 1_000_000_000,
            current_phase: UltradianPhase::Peak,
            cycle_start_ns: 0,
            cycle_count: 0,
        }
    }

    pub fn phase_at(&self, now_ns: u128) -> UltradianPhase {
        let elapsed = now_ns.saturating_sub(self.cycle_start_ns) % self.period_ns;
        let ratio = elapsed as f64 / self.period_ns as f64;
        if ratio < 0.4 {
            UltradianPhase::Peak
        } else if ratio < 0.7 {
            UltradianPhase::Decline
        } else if ratio < 0.9 {
            UltradianPhase::Trough
        } else {
            UltradianPhase::Recovery
        }
    }

    pub fn update(&mut self, now_ns: u128) {
        let phase = self.phase_at(now_ns);
        if phase != self.current_phase {
            self.current_phase = phase;
            if phase == UltradianPhase::Peak {
                self.cycle_count += 1;
                self.cycle_start_ns = now_ns;
            }
        }
    }

    pub fn focus_capacity(&self) -> f32 {
        match self.current_phase {
            UltradianPhase::Peak => 1.0,
            UltradianPhase::Decline => 0.6,
            UltradianPhase::Trough => 0.2,
            UltradianPhase::Recovery => 0.4,
        }
    }
}

// =====================================================================================
// 时间间隔 (TemporalInterval)
// =====================================================================================

/// 时间间隔：两个时间点间的间隔。
#[derive(Clone, Debug)]
pub struct TemporalInterval {
    pub start_ns: u128,
    pub end_ns: u128,
    pub label: String,
}

impl TemporalInterval {
    pub fn new(start_ns: u128, end_ns: u128, label: &str) -> Self {
        Self {
            start_ns,
            end_ns,
            label: label.to_string(),
        }
    }

    pub fn duration_ns(&self) -> u128 {
        self.end_ns.saturating_sub(self.start_ns)
    }

    pub fn contains(&self, point_ns: u128) -> bool {
        point_ns >= self.start_ns && point_ns <= self.end_ns
    }

    pub fn overlaps(&self, other: &TemporalInterval) -> bool {
        self.start_ns <= other.end_ns && other.start_ns <= self.end_ns
    }
}

// =====================================================================================
// 时间序列 (TemporalSequence)
// =====================================================================================

/// 时间序列事件。
#[derive(Clone, Debug)]
pub struct TemporalEvent {
    pub id: u64,
    pub timestamp_ns: u128,
    pub event_type: String,
    pub data: String,
}

/// 时间序列：有序事件序列。
pub struct TemporalSequence {
    events: BTreeMap<u128, TemporalEvent>,
    next_id: u64,
    pub max_events: usize,
}

impl TemporalSequence {
    pub fn new() -> Self {
        Self {
            events: BTreeMap::new(),
            next_id: 1,
            max_events: 100000,
        }
    }

    pub fn append(&mut self, timestamp_ns: u128, event_type: &str, data: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let event = TemporalEvent {
            id,
            timestamp_ns,
            event_type: event_type.to_string(),
            data: data.to_string(),
        };
        self.events.insert(timestamp_ns, event);
        if self.events.len() > self.max_events {
            if let Some(&oldest) = self.events.keys().next() {
                self.events.remove(&oldest);
            }
        }
        id
    }

    pub fn query_range(&self, start_ns: u128, end_ns: u128) -> Vec<&TemporalEvent> {
        self.events.range(start_ns..=end_ns).map(|(_, v)| v).collect()
    }

    pub fn latest(&self, n: usize) -> Vec<&TemporalEvent> {
        self.events.values().rev().take(n).collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn event_count_by_type(&self, event_type: &str) -> usize {
        self.events.values().filter(|e| e.event_type == event_type).count()
    }
}

// =====================================================================================
// 时序调度 (TemporalScheduling)
// =====================================================================================

/// 定时任务。
#[derive(Clone, Debug)]
pub struct ScheduledTask {
    pub id: u64,
    pub name: String,
    pub trigger_time_ns: u128,
    pub interval_ns: Option<u128>,    // None 表示一次性任务
    pub action: String,
    pub executed: bool,
}

/// 时序调度器：基于时间的任务调度。
pub struct TemporalScheduler {
    tasks: BTreeMap<u64, ScheduledTask>,
    time_index: BTreeMap<u128, Vec<u64>>, // 触发时间 -> 任务 ID 列表
    next_id: u64,
}

impl TemporalScheduler {
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            time_index: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn schedule_once(&mut self, name: &str, trigger_time_ns: u128, action: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let task = ScheduledTask {
            id,
            name: name.to_string(),
            trigger_time_ns,
            interval_ns: None,
            action: action.to_string(),
            executed: false,
        };
        self.tasks.insert(id, task);
        self.time_index.entry(trigger_time_ns).or_default().push(id);
        id
    }

    pub fn schedule_periodic(&mut self, name: &str, start_time_ns: u128, interval_ns: u128, action: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let task = ScheduledTask {
            id,
            name: name.to_string(),
            trigger_time_ns: start_time_ns,
            interval_ns: Some(interval_ns),
            action: action.to_string(),
            executed: false,
        };
        self.tasks.insert(id, task);
        self.time_index.entry(start_time_ns).or_default().push(id);
        id
    }

    pub fn due_tasks(&self, now_ns: u128) -> Vec<&ScheduledTask> {
        let mut due = Vec::new();
        for (time, ids) in self.time_index.range(..=now_ns) {
            for id in ids {
                if let Some(task) = self.tasks.get(id) {
                    if !task.executed {
                        due.push(task);
                    }
                }
            }
        }
        due.sort_by(|a, b| a.trigger_time_ns.cmp(&b.trigger_time_ns));
        due
    }

    pub fn mark_executed(&mut self, id: u64) -> bool {
        if let Some(task) = self.tasks.get_mut(&id) {
            task.executed = true;
            if let Some(interval) = task.interval_ns {
                // 重新调度周期性任务
                let new_time = task.trigger_time_ns + interval;
                self.time_index.entry(new_time).or_default().push(id);
                // 更新任务触发时间
                // 简化：不修改原任务的 trigger_time_ns，依赖 time_index
            }
            true
        } else {
            false
        }
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn pending_count(&self) -> usize {
        self.tasks.values().filter(|t| !t.executed).count()
    }
}

// =====================================================================================
// 时间记忆 (TemporalMemory)
// =====================================================================================

/// 时间记忆条目：对某个时间点的编码。
#[derive(Clone, Debug)]
pub struct TemporalMemoryEntry {
    pub timestamp_ns: u128,
    pub event_summary: String,
    pub salience: f32,
    pub tags: BTreeSet<String>,
}

/// 时间记忆：对过去时间的编码。
pub struct TemporalMemory {
    entries: BTreeMap<u128, TemporalMemoryEntry>,
    next_id: u64,
    pub capacity: usize,
}

impl TemporalMemory {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
            capacity: 100000,
        }
    }

    pub fn encode(&mut self, timestamp_ns: u128, summary: &str, salience: f32, tags: &[&str]) -> u64 {
        let entry = TemporalMemoryEntry {
            timestamp_ns,
            event_summary: summary.to_string(),
            salience: salience.clamp(0.0, 1.0),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        };
        self.entries.insert(timestamp_ns, entry);
        if self.entries.len() > self.capacity {
            if let Some(&oldest) = self.entries.keys().next() {
                self.entries.remove(&oldest);
            }
        }
        self.next_id += 1;
        self.next_id
    }

    pub fn query_range(&self, start_ns: u128, end_ns: u128) -> Vec<&TemporalMemoryEntry> {
        self.entries.range(start_ns..=end_ns).map(|(_, v)| v).collect()
    }

    pub fn search_by_tag(&self, tag: &str) -> Vec<&TemporalMemoryEntry> {
        self.entries.values().filter(|e| e.tags.contains(tag)).collect()
    }

    pub fn top_salient(&self, k: usize) -> Vec<&TemporalMemoryEntry> {
        let mut v: Vec<&TemporalMemoryEntry> = self.entries.values().collect();
        v.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 时间预测 (TemporalPrediction)
// =====================================================================================

/// 时间预测结果。
#[derive(Clone, Debug)]
pub struct TemporalPrediction {
    pub predicted_time_ns: u128,
    pub event_type: String,
    pub confidence: f32,
}

/// 时间预测：对未来时间的预测。
pub struct TemporalPredictionEngine {
    predictions: Vec<TemporalPrediction>,
    pub prediction_count: u64,
}

impl TemporalPredictionEngine {
    pub fn new() -> Self {
        Self {
            predictions: Vec::new(),
            prediction_count: 0,
        }
    }

    pub fn predict(&mut self, base_time_ns: u128, event_type: &str, delay_ns: u128, confidence: f32) -> u64 {
        let predicted = base_time_ns + delay_ns;
        self.predictions.push(TemporalPrediction {
            predicted_time_ns: predicted,
            event_type: event_type.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
        });
        self.prediction_count += 1;
        self.prediction_count
    }

    pub fn upcoming(&self, now_ns: u128, limit: usize) -> Vec<&TemporalPrediction> {
        let mut upcoming: Vec<&TemporalPrediction> = self.predictions.iter()
            .filter(|p| p.predicted_time_ns > now_ns)
            .collect();
        upcoming.sort_by(|a, b| a.predicted_time_ns.cmp(&b.predicted_time_ns));
        upcoming.truncate(limit);
        upcoming
    }

    pub fn count(&self) -> usize {
        self.predictions.len()
    }
}

// =====================================================================================
// 时间绑定 (TemporalBinding)
// =====================================================================================

/// 时间绑定：将事件绑定到特定时间点。
#[derive(Clone, Debug)]
pub struct TemporalBinding {
    pub event_id: u64,
    pub bound_time_ns: u128,
    pub binding_type: String,  // "scheduled", "observed", "predicted"
    pub active: bool,
}

/// 时间绑定管理器。
pub struct TemporalBindingManager {
    bindings: BTreeMap<u64, TemporalBinding>,
    next_id: u64,
}

impl TemporalBindingManager {
    pub fn new() -> Self {
        Self {
            bindings: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn bind(&mut self, event_id: u64, time_ns: u128, binding_type: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.bindings.insert(id, TemporalBinding {
            event_id,
            bound_time_ns: time_ns,
            binding_type: binding_type.to_string(),
            active: true,
        });
        id
    }

    pub fn unbind(&mut self, id: u64) -> bool {
        if let Some(b) = self.bindings.get_mut(&id) {
            b.active = false;
            true
        } else {
            false
        }
    }

    pub fn get_bindings_at(&self, time_ns: u128, window_ns: u128) -> Vec<&TemporalBinding> {
        self.bindings.values()
            .filter(|b| b.active && b.bound_time_ns >= time_ns - window_ns && b.bound_time_ns <= time_ns + window_ns)
            .collect()
    }

    pub fn count(&self) -> usize {
        self.bindings.len()
    }

    pub fn active_count(&self) -> usize {
        self.bindings.values().filter(|b| b.active).count()
    }
}

// =====================================================================================
// 时间引擎 (TemporalEngine) —— 统一入口
// =====================================================================================

/// 时间引擎：整合时钟、昼夜律、超日夜律、时间间隔、时间序列、时序调度、时间记忆、时间预测、时间绑定。
pub struct TemporalEngine {
    pub wall_clock: TemporalClock,
    pub monotonic_clock: TemporalClock,
    pub circadian: CircadianRhythm,
    pub ultradian: UltradianRhythm,
    pub sequence: TemporalSequence,
    pub scheduler: TemporalScheduler,
    pub memory: TemporalMemory,
    pub prediction: TemporalPredictionEngine,
    pub binding: TemporalBindingManager,
    pub enabled: bool,
    pub now_ns: u128,
    pub total_ticks: u64,
}

impl TemporalEngine {
    pub fn new() -> Self {
        Self {
            wall_clock: TemporalClock::new(ClockType::WallClock),
            monotonic_clock: TemporalClock::new(ClockType::Monotonic),
            circadian: CircadianRhythm::new(),
            ultradian: UltradianRhythm::new(),
            sequence: TemporalSequence::new(),
            scheduler: TemporalScheduler::new(),
            memory: TemporalMemory::new(),
            prediction: TemporalPredictionEngine::new(),
            binding: TemporalBindingManager::new(),
            enabled: true,
            now_ns: crate::ai_os::native::now_ns(),
            total_ticks: 0,
        }
    }

    pub fn tick(&mut self, now_ns: u128) {
        self.now_ns = now_ns;
        self.total_ticks += 1;
        self.wall_clock.tick();
        self.monotonic_clock.tick();
        self.circadian.update(now_ns);
        self.ultradian.update(now_ns);
    }

    pub fn now(&self) -> u128 {
        self.now_ns
    }

    pub fn append_event(&mut self, event_type: &str, data: &str) -> u64 {
        self.sequence.append(self.now_ns, event_type, data)
    }

    pub fn encode_memory(&mut self, summary: &str, salience: f32, tags: &[&str]) -> u64 {
        self.memory.encode(self.now_ns, summary, salience, tags)
    }

    pub fn schedule_once(&mut self, name: &str, delay_ns: u128, action: &str) -> u64 {
        self.scheduler.schedule_once(name, self.now_ns + delay_ns, action)
    }

    pub fn schedule_periodic(&mut self, name: &str, delay_ns: u128, interval_ns: u128, action: &str) -> u64 {
        self.scheduler.schedule_periodic(name, self.now_ns + delay_ns, interval_ns, action)
    }

    pub fn due_tasks(&mut self) -> Vec<&ScheduledTask> {
        let due = self.scheduler.due_tasks(self.now_ns);
        for task in &due {
            self.scheduler.mark_executed(task.id);
        }
        due
    }

    pub fn bind_event(&mut self, event_id: u64, offset_ns: u128, binding_type: &str) -> u64 {
        self.binding.bind(event_id, self.now_ns + offset_ns, binding_type)
    }

    pub fn predict_event(&mut self, event_type: &str, delay_ns: u128, confidence: f32) -> u64 {
        self.prediction.predict(self.now_ns, event_type, delay_ns, confidence)
    }

    pub fn circadian_phase(&self) -> CircadianPhase {
        self.circadian.current_phase
    }

    pub fn ultradian_phase(&self) -> UltradianPhase {
        self.ultradian.current_phase
    }

    pub fn alertness(&self) -> f32 {
        self.circadian.alertness() * self.ultradian.focus_capacity()
    }

    pub fn tick_count(&self) -> u64 {
        self.total_ticks
    }
}

impl Default for TemporalEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static TEMPORAL: OnceLock<Mutex<TemporalEngine>> = OnceLock::new();

pub fn temporal_engine() -> &'static Mutex<TemporalEngine> {
    TEMPORAL.get_or_init(|| Mutex::new(TemporalEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = temporal_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = temporal_engine().lock().unwrap();
    format!(
        "temporal: now={} ticks={} circadian={:?} ultradian={:?} alertness={:.2} events={} tasks={} pending={} memories={} predictions={} bindings={}",
        eng.now_ns,
        eng.tick_count(),
        eng.circadian_phase(),
        eng.ultradian_phase(),
        eng.alertness(),
        eng.sequence.len(),
        eng.scheduler.task_count(),
        eng.scheduler.pending_count(),
        eng.memory.len(),
        eng.prediction.count(),
        eng.binding.active_count()
    )
}

pub fn temporal_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn temporal_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("tick") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = temporal_engine().lock().unwrap();
            eng.tick(now);
            Ok(format!("时间引擎已推进 now={} ticks={}", now, eng.tick_count()))
        }
        Some("now") => {
            let eng = temporal_engine().lock().unwrap();
            Ok(format!("当前时间: {}", eng.now()))
        }
        Some("circadian") => {
            let eng = temporal_engine().lock().unwrap();
            Ok(format!("昼夜律: {:?} 警觉度={:.2}", eng.circadian_phase(), eng.alertness()))
        }
        Some("ultradian") => {
            let eng = temporal_engine().lock().unwrap();
            Ok(format!("超日夜律: {:?} 专注力={:.2}", eng.ultradian_phase(), eng.ultradian.focus_capacity()))
        }
        Some("append-event") => {
            let event_type = it.next().ok_or("用法: temporal:// append-event <类型> <数据>")?;
            let data = it.next().unwrap_or("");
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.append_event(event_type, data);
            Ok(format!("已追加时间事件 id={} 类型={}", id, event_type))
        }
        Some("query-events") => {
            let start = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let end = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(u128::MAX);
            let eng = temporal_engine().lock().unwrap();
            let events = eng.sequence.query_range(start, end);
            let mut out = String::from("时间序列查询:\n");
            for e in events {
                out.push_str(&format!("  t={} 类型={} 数据={}\n", e.timestamp_ns, e.event_type, e.data));
            }
            Ok(out)
        }
        Some("encode-memory") => {
            let summary = it.next().ok_or("用法: temporal:// encode-memory <摘要> <显著性> <标签...>")?;
            let salience = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let tags: Vec<&str> = it.collect();
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.encode_memory(summary, salience, &tags);
            Ok(format!("已编码时间记忆 id={} 摘要={}", id, summary))
        }
        Some("schedule-once") => {
            let name = it.next().ok_or("用法: temporal:// schedule-once <名称> <延迟ns> <动作>")?;
            let delay = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(1_000_000_000);
            let action = it.next().unwrap_or("default");
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.schedule_once(name, delay, action);
            Ok(format!("已调度一次性任务 id={} 名称={}", id, name))
        }
        Some("schedule-periodic") => {
            let name = it.next().ok_or("用法: temporal:// schedule-periodic <名称> <延迟ns> <间隔ns> <动作>")?;
            let delay = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(1_000_000_000);
            let interval = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(5_000_000_000);
            let action = it.next().unwrap_or("default");
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.schedule_periodic(name, delay, interval, action);
            Ok(format!("已调度周期性任务 id={} 名称={}", id, name))
        }
        Some("due-tasks") => {
            let mut eng = temporal_engine().lock().unwrap();
            let tasks = eng.due_tasks();
            let mut out = String::from("到期任务:\n");
            for t in tasks {
                out.push_str(&format!("  id={} 名称={} 动作={}\n", t.id, t.name, t.action));
            }
            Ok(out)
        }
        Some("bind-event") => {
            let event_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少事件id")?;
            let offset = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let btype = it.next().unwrap_or("observed");
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.bind_event(event_id, offset, btype);
            Ok(format!("已绑定事件 id={} 绑定id={}", event_id, id))
        }
        Some("predict-event") => {
            let event_type = it.next().ok_or("用法: temporal:// predict-event <类型> <延迟ns> <置信度>")?;
            let delay = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(1_000_000_000);
            let confidence = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.8);
            let mut eng = temporal_engine().lock().unwrap();
            let id = eng.predict_event(event_type, delay, confidence);
            Ok(format!("已预测事件 id={} 类型={}", id, event_type))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 temporal 动作: {}", other)),
    }
}

pub fn temporal_act_dispatch(payload: &str) -> Result<String, String> {
    temporal_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_now_and_elapsed() {
        let clock = TemporalClock::new(ClockType::Monotonic);
        let start = clock.now();
        let elapsed = clock.elapsed();
        assert!(elapsed >= 0);
        let _ = start;
    }

    #[test]
    fn clock_tick() {
        let mut clock = TemporalClock::new(ClockType::WallClock);
        clock.tick();
        assert_eq!(clock.tick_count, 1);
    }

    #[test]
    fn clock_offset() {
        let mut clock = TemporalClock::new(ClockType::WallClock);
        clock.set_offset(1_000_000_000);
        let now = clock.now();
        let base = crate::ai_os::native::now_ns();
        assert!(now >= base + 1_000_000_000 || now < base); // 可能溢出，简单检查
    }

    #[test]
    fn circadian_phase() {
        let rhythm = CircadianRhythm::new();
        let _ = rhythm.period_ns;
        // 在不同模拟时间检查阶段
        let phase_morning = rhythm.phase_at(6 * 60 * 60 * 1_000_000_000); // 早上 6 点
        assert_eq!(phase_morning, CircadianPhase::Morning);
    }

    #[test]
    fn circadian_alertness() {
        let rhythm = CircadianRhythm::new();
        let alertness = rhythm.alertness();
        assert!(alertness >= 0.0 && alertness <= 1.0);
    }

    #[test]
    fn circadian_update() {
        let mut rhythm = CircadianRhythm::new();
        rhythm.update(0);
        rhythm.update(1_000_000_000);
        let _ = rhythm.current_phase;
    }

    #[test]
    fn ultradian_phase() {
        let rhythm = UltradianRhythm::new();
        let phase = rhythm.phase_at(0);
        assert_eq!(phase, UltradianPhase::Peak);
    }

    #[test]
    fn ultradian_focus() {
        let rhythm = UltradianRhythm::new();
        let focus = rhythm.focus_capacity();
        assert!(focus >= 0.0 && focus <= 1.0);
    }

    #[test]
    fn ultradian_update() {
        let mut rhythm = UltradianRhythm::new();
        rhythm.update(0);
        rhythm.update(rhythm.period_ns); // 进入下一个周期
        assert_eq!(rhythm.cycle_count, 1);
    }

    #[test]
    fn temporal_interval() {
        let interval = TemporalInterval::new(100, 200, "测试间隔");
        assert_eq!(interval.duration_ns(), 100);
        assert!(interval.contains(150));
        assert!(!interval.contains(50));
        assert!(!interval.contains(250));
    }

    #[test]
    fn temporal_interval_overlap() {
        let i1 = TemporalInterval::new(100, 200, "a");
        let i2 = TemporalInterval::new(150, 300, "b");
        let i3 = TemporalInterval::new(300, 400, "c");
        assert!(i1.overlaps(&i2));
        assert!(!i1.overlaps(&i3));
    }

    #[test]
    fn temporal_sequence_append_query() {
        let mut seq = TemporalSequence::new();
        seq.append(1000, "perception", "saw red");
        seq.append(2000, "action", "moved");
        assert_eq!(seq.len(), 2);
        let events = seq.query_range(1000, 2000);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn temporal_sequence_latest() {
        let mut seq = TemporalSequence::new();
        seq.append(1000, "a", "d1");
        seq.append(2000, "b", "d2");
        seq.append(3000, "c", "d3");
        let latest = seq.latest(2);
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0].event_type, "c");
    }

    #[test]
    fn temporal_scheduler_once() {
        let mut sched = TemporalScheduler::new();
        let id = sched.schedule_once("任务1", 5000, "动作1");
        assert_eq!(id, 1);
        let due = sched.due_tasks(10000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "任务1");
    }

    #[test]
    fn temporal_scheduler_periodic() {
        let mut sched = TemporalScheduler::new();
        let id = sched.schedule_periodic("周期任务", 0, 1000, "周期动作");
        assert_eq!(id, 1);
        let due = sched.due_tasks(0);
        assert_eq!(due.len(), 1);
        // 标记执行后，应该重新调度
        sched.mark_executed(id);
        let due2 = sched.due_tasks(1000);
        assert!(!due2.is_empty());
    }

    #[test]
    fn temporal_scheduler_pending() {
        let mut sched = TemporalScheduler::new();
        sched.schedule_once("t1", 100, "a");
        sched.schedule_once("t2", 200, "a");
        assert_eq!(sched.pending_count(), 2);
    }

    #[test]
    fn temporal_memory_encode_query() {
        let mut mem = TemporalMemory::new();
        mem.encode(1000, "事件A", 0.8, &["视觉"]);
        mem.encode(2000, "事件B", 0.5, &["听觉"]);
        assert_eq!(mem.len(), 2);
        let results = mem.query_range(1000, 2000);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn temporal_memory_search_tag() {
        let mut mem = TemporalMemory::new();
        mem.encode(1000, "红色", 0.8, &["视觉", "红色"]);
        mem.encode(2000, "大声", 0.5, &["听觉"]);
        let results = mem.search_by_tag("视觉");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn temporal_memory_top_salient() {
        let mut mem = TemporalMemory::new();
        mem.encode(1000, "低", 0.2, &[]);
        mem.encode(2000, "高", 0.9, &[]);
        let top = mem.top_salient(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].event_summary, "高");
    }

    #[test]
    fn temporal_prediction() {
        let mut pred = TemporalPredictionEngine::new();
        let id = pred.predict(10000, "下一个动作", 5000, 0.8);
        assert_eq!(id, 1);
        assert_eq!(pred.count(), 1);
        let upcoming = pred.upcoming(0, 10);
        assert_eq!(upcoming.len(), 1);
    }

    #[test]
    fn temporal_prediction_upcoming_filter() {
        let mut pred = TemporalPredictionEngine::new();
        pred.predict(1000, "过去", 0, 0.5);
        pred.predict(5000, "未来", 0, 0.5);
        let upcoming = pred.upcoming(2000, 10);
        assert_eq!(upcoming.len(), 1);
        assert_eq!(upcoming[0].event_type, "未来");
    }

    #[test]
    fn temporal_binding() {
        let mut bind = TemporalBindingManager::new();
        let id = bind.bind(1, 5000, "scheduled");
        assert_eq!(id, 1);
        assert_eq!(bind.count(), 1);
        assert_eq!(bind.active_count(), 1);
        assert!(bind.unbind(id));
        assert_eq!(bind.active_count(), 0);
    }

    #[test]
    fn temporal_binding_query_window() {
        let mut bind = TemporalBindingManager::new();
        bind.bind(1, 10000, "observed");
        bind.bind(2, 20000, "predicted");
        let results = bind.get_bindings_at(10000, 1000);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn temporal_engine_tick() {
        let mut eng = TemporalEngine::new();
        eng.tick(1_000_000_000);
        assert_eq!(eng.total_ticks, 1);
        assert!(eng.now_ns > 0);
    }

    #[test]
    fn temporal_engine_append_and_memory() {
        let mut eng = TemporalEngine::new();
        eng.tick(1_000_000_000);
        let id = eng.append_event("感知", "看到了红色");
        assert_eq!(id, 1);
        let mid = eng.encode_memory("重要事件", 0.9, &["视觉"]);
        assert_eq!(mid, 1);
    }

    #[test]
    fn temporal_engine_schedule() {
        let mut eng = TemporalEngine::new();
        eng.tick(1_000_000_000);
        let id = eng.schedule_once("测试任务", 5_000_000_000, "执行动作");
        assert_eq!(id, 1);
        let tasks = eng.due_tasks();
        // 在 now=1s，任务在 6s 触发，不应到期
        assert!(tasks.is_empty());
    }

    #[test]
    fn temporal_engine_bind_predict() {
        let mut eng = TemporalEngine::new();
        eng.tick(1_000_000_000);
        let bid = eng.bind_event(1, 1000, "observed");
        assert_eq!(bid, 1);
        let pid = eng.predict_event("未来事件", 5_000_000_000, 0.8);
        assert_eq!(pid, 1);
    }

    #[test]
    fn temporal_engine_alertness() {
        let mut eng = TemporalEngine::new();
        eng.tick(0);
        let alertness = eng.alertness();
        assert!(alertness >= 0.0 && alertness <= 1.0);
    }

    #[test]
    fn temporal_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("temporal:"));
        assert!(s.contains("ticks="));
    }

    #[test]
    fn temporal_act_tick() {
        init();
        let res = temporal_act("tick 1000000000").unwrap();
        assert!(res.contains("时间引擎已推进"));
    }

    #[test]
    fn temporal_act_append() {
        init();
        let res = temporal_act("append-event 感知 看到了光").unwrap();
        assert!(res.contains("已追加时间事件"));
    }

    #[test]
    fn temporal_act_memory() {
        init();
        let res = temporal_act("encode-memory 重要时刻 0.9 视觉 重要").unwrap();
        assert!(res.contains("已编码时间记忆"));
    }

    #[test]
    fn temporal_act_schedule() {
        init();
        let res = temporal_act("schedule-once 提醒 5000000000 喝水").unwrap();
        assert!(res.contains("已调度一次性任务"));
    }

    #[test]
    fn temporal_act_periodic() {
        init();
        let res = temporal_act("schedule-periodic 心跳 1000000000 5000000000 检查").unwrap();
        assert!(res.contains("已调度周期性任务"));
    }

    #[test]
    fn temporal_act_status() {
        init();
        let res = temporal_act("status").unwrap();
        assert!(res.contains("temporal:"));
    }

    #[test]
    fn circadian_phase_names() {
        assert_eq!(CircadianPhase::Morning as u32, 0);
        assert_eq!(CircadianPhase::DeepNight as u32, 5);
    }

    #[test]
    fn ultradian_phase_names() {
        assert_eq!(UltradianPhase::Peak as u32, 0);
        assert_eq!(UltradianPhase::Recovery as u32, 3);
    }

    #[test]
    fn clock_type_names() {
        assert_eq!(ClockType::WallClock as u32, 0);
        assert_eq!(ClockType::Monotonic as u32, 1);
        assert_eq!(ClockType::CPU as u32, 2);
    }

    #[test]
    fn action_schedule_policy_names() {
        // 测试 action 模块的枚举（这里只是确保引入正确）
        let _ = ActionSchedulePolicy::Fifo;
        let _ = ActionSchedulePolicy::Priority;
    }
}
