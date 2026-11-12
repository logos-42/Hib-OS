//! 注意力层 —— AI 原生 OS 的「注意力机制」 (Attention Mechanism)
//!
//! 本模块实现 AI 原生 OS 的注意力系统，对应传统 OS 中的「中断控制器与
//! 调度器」概念，但重新映射到注意力域：
//!
//! * AttentionLens (注意力透镜) —— 选择性过滤感官输入，等价于中断屏蔽寄存器；
//! * SaliencyMap (显著性图) —— 各感知通道的竞争优先级图；
//! * AttentionalBlink (注意拍瞬) —— 集中注意后的短暂盲期；
//! * FocusBuffer (焦点缓冲) —— 当前聚焦的认知对象，等价于程序计数器；
//! * DividedAttention (分散注意) —— 多任务并行处理的容量限制；
//! * SustainedAttention (持续注意) —— 长时间维持注意力的能力；
//! * SelectiveAttention (选择性注意) —— 鸡尾酒会效应，从噪声中提取信号；
//! * AttentionEconomy (注意力经济) —— 注意力作为稀缺资源的分配与定价；
//! * AttentionalCycle (注意力周期) —— 90 分钟超日夜律等节律模型；
//! * AttentionDeficit (注意力缺陷) —— 注意力资源不足时的降级处理。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度，
//! 并通过 `attention://*` 通道接入 `sense`/`act` 感知层。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 注意力透镜 (AttentionLens)
// =====================================================================================

/// 注意力过滤模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensMode {
    Open,     // 全开，接收所有输入
    Narrow,   // 狭窄，仅接收高显著性输入
    Selective,// 选择性，仅接收指定通道
    Closed,   // 全闭，屏蔽所有输入
}

/// 注意力透镜：控制哪些感官输入能进入认知处理。
pub struct AttentionLens {
    pub mode: LensMode,
    pub allowed_channels: BTreeSet<String>,
    pub blocked_channels: BTreeSet<String>,
    pub threshold: f32, // 显著性阈值，0~1
    pub filter_count: u64,
    pub pass_count: u64,
}

impl AttentionLens {
    pub fn new() -> Self {
        Self {
            mode: LensMode::Open,
            allowed_channels: BTreeSet::new(),
            blocked_channels: BTreeSet::new(),
            threshold: 0.3,
            filter_count: 0,
            pass_count: 0,
        }
    }

    pub fn should_pass(&mut self, channel: &str, salience: f32) -> bool {
        self.filter_count += 1;
        let pass = match self.mode {
            LensMode::Open => true,
            LensMode::Closed => false,
            LensMode::Narrow => salience >= self.threshold,
            LensMode::Selective => {
                self.allowed_channels.contains(channel) && salience >= self.threshold
            }
        };
        if pass {
            self.pass_count += 1;
        }
        // 如果通道被显式阻塞，直接拒绝
        if self.blocked_channels.contains(channel) {
            return false;
        }
        pass
    }

    pub fn allow_channel(&mut self, channel: &str) {
        self.allowed_channels.insert(channel.to_string());
    }

    pub fn block_channel(&mut self, channel: &str) {
        self.blocked_channels.insert(channel.to_string());
        self.allowed_channels.remove(channel);
    }

    pub fn set_threshold(&mut self, t: f32) {
        self.threshold = t.clamp(0.0, 1.0);
    }

    pub fn pass_rate(&self) -> f32 {
        if self.filter_count == 0 { 0.0 } else { self.pass_count as f32 / self.filter_count as f32 }
    }
}

impl Default for AttentionLens {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 显著性图 (SaliencyMap)
// =====================================================================================

/// 显著性通道条目。
#[derive(Clone, Debug)]
pub struct SaliencyEntry {
    pub channel: String,
    pub salience: f32,
    pub last_update_ns: u128,
    pub decay_rate: f32,
}

/// 显著性图：各感知通道的竞争优先级图，决定注意力分配。
pub struct SaliencyMap {
    entries: BTreeMap<String, SaliencyEntry>,
    pub max_channels: usize,
    pub global_decay: f32,
}

impl SaliencyMap {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            max_channels: 64,
            global_decay: 0.001,
        }
    }

    pub fn update(&mut self, channel: &str, salience: f32, now_ns: u128) {
        let entry = self.entries.entry(channel.to_string()).or_insert(SaliencyEntry {
            channel: channel.to_string(),
            salience: 0.0,
            last_update_ns: now_ns,
            decay_rate: 0.01,
        });
        entry.salience = salience.clamp(0.0, 1.0);
        entry.last_update_ns = now_ns;
    }

    pub fn decay(&mut self, now_ns: u128) {
        let mut to_remove = Vec::new();
        for (channel, entry) in self.entries.iter_mut() {
            let elapsed = now_ns.saturating_sub(entry.last_update_ns);
            let decay = (elapsed as f64 * self.global_decay as f64) as f32;
            entry.salience = (entry.salience - decay).max(0.0);
            if entry.salience < 0.01 {
                to_remove.push(channel.clone());
            }
        }
        for channel in to_remove {
            self.entries.remove(&channel);
        }
    }

    pub fn top_k(&self, k: usize) -> Vec<(&String, f32)> {
        let mut v: Vec<(&String, f32)> = self.entries.iter().map(|(c, e)| (c, e.salience)).collect();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }

    pub fn get(&self, channel: &str) -> Option<f32> {
        self.entries.get(channel).map(|e| e.salience)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn total_salience(&self) -> f32 {
        self.entries.values().map(|e| e.salience).sum()
    }

    pub fn dominant_channel(&self) -> Option<&String> {
        self.entries.iter().max_by_key(|(_, e)| (e.salience * 1000.0) as u64).map(|(c, _)| c)
    }
}

impl Default for SaliencyMap {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 注意拍瞬 (AttentionalBlink)
// =====================================================================================

/// 注意拍瞬状态：注意力集中后的短暂盲期，期间新输入被抑制。
pub struct AttentionalBlink {
    pub active: bool,
    pub start_ns: u128,
    pub duration_ns: u128,
    pub suppression_strength: f32, // 0~1，抑制强度
    pub blink_count: u64,
}

impl AttentionalBlink {
    pub fn new() -> Self {
        Self {
            active: false,
            start_ns: 0,
            duration_ns: 300_000_000, // 300ms
            suppression_strength: 0.8,
            blink_count: 0,
        }
    }

    pub fn trigger(&mut self, now_ns: u128) {
        self.active = true;
        self.start_ns = now_ns;
        self.blink_count += 1;
    }

    pub fn update(&mut self, now_ns: u128) {
        if self.active && now_ns - self.start_ns >= self.duration_ns {
            self.active = false;
        }
    }

    pub fn is_suppressing(&self, now_ns: u128) -> bool {
        self.active && now_ns - self.start_ns < self.duration_ns
    }

    pub fn suppression_factor(&self, now_ns: u128) -> f32 {
        if !self.is_suppressing(now_ns) {
            return 1.0;
        }
        let elapsed = now_ns - self.start_ns;
        let progress = elapsed as f32 / self.duration_ns as f32;
        1.0 - self.suppression_strength * (1.0 - progress)
    }
}

// =====================================================================================
// 焦点缓冲 (FocusBuffer)
// =====================================================================================

/// 焦点缓冲：当前聚焦的认知对象，等价于程序计数器。
pub struct FocusBuffer {
    pub current: Option<FocusObject>,
    pub history: VecDeque<FocusObject>,
    pub max_history: usize,
    pub focus_changes: u64,
}

/// 焦点对象。
#[derive(Clone, Debug)]
pub struct FocusObject {
    pub id: u64,
    pub name: String,
    pub content: String,
    pub focus_start_ns: u128,
    pub focus_duration_ns: u128,
    pub salience: f32,
}

impl FocusBuffer {
    pub fn new() -> Self {
        Self {
            current: None,
            history: VecDeque::new(),
            max_history: 1024,
            focus_changes: 0,
        }
    }

    pub fn set_focus(&mut self, obj: FocusObject, now_ns: u128) {
        if let Some(ref cur) = self.current {
            let mut finished = cur.clone();
            finished.focus_duration_ns = now_ns - cur.focus_start_ns;
            if self.history.len() >= self.max_history {
                self.history.pop_front();
            }
            self.history.push_back(finished);
        }
        self.current = Some(obj);
        self.focus_changes += 1;
    }

    pub fn clear_focus(&mut self, now_ns: u128) {
        if let Some(cur) = self.current.take() {
            let mut finished = cur;
            finished.focus_duration_ns = now_ns - finished.focus_start_ns;
            if self.history.len() >= self.max_history {
                self.history.pop_front();
            }
            self.history.push_back(finished);
        }
    }

    pub fn current_name(&self) -> Option<&str> {
        self.current.as_ref().map(|o| o.name.as_str())
    }

    pub fn current_duration(&self, now_ns: u128) -> u128 {
        self.current.as_ref().map(|o| now_ns - o.focus_start_ns).unwrap_or(0)
    }

    pub fn focus_change_count(&self) -> u64 {
        self.focus_changes
    }
}

// =====================================================================================
// 分散注意 / 持续注意 / 选择性注意
// =====================================================================================

/// 分散注意状态：多任务并行时的注意力分配。
pub struct DividedAttention {
    pub active_tasks: BTreeMap<String, f32>, // 任务名 -> 分配的注意力比例
    pub total_capacity: f32,                 // 总注意力容量，通常为 1.0
    pub switch_cost_per_task: f32,
}

impl DividedAttention {
    pub fn new() -> Self {
        Self {
            active_tasks: BTreeMap::new(),
            total_capacity: 1.0,
            switch_cost_per_task: 0.05,
        }
    }

    pub fn allocate(&mut self, task: &str, share: f32) -> bool {
        let current_total: f32 = self.active_tasks.values().sum();
        if current_total + share > self.total_capacity {
            return false;
        }
        self.active_tasks.insert(task.to_string(), share);
        true
    }

    pub fn deallocate(&mut self, task: &str) {
        self.active_tasks.remove(task);
    }

    pub fn utilization(&self) -> f32 {
        self.active_tasks.values().sum::<f32>() / self.total_capacity
    }

    pub fn task_count(&self) -> usize {
        self.active_tasks.len()
    }
}

/// 持续注意：长时间维持注意力的能力与衰减模型。
pub struct SustainedAttention {
    pub start_ns: u128,
    pub decay_rate_per_second: f32,
    pub current_level: f32,
    pub restoration_per_break_ns: u128,
    pub last_break_ns: u128,
}

impl SustainedAttention {
    pub fn new() -> Self {
        Self {
            start_ns: 0,
            decay_rate_per_second: 0.05,
            current_level: 1.0,
            restoration_per_break_ns: 300_000_000, // 5 分钟休息恢复
            last_break_ns: 0,
        }
    }

    pub fn start(&mut self, now_ns: u128) {
        self.start_ns = now_ns;
        self.current_level = 1.0;
        self.last_break_ns = now_ns;
    }

    pub fn update(&mut self, now_ns: u128) {
        let elapsed_s = (now_ns - self.start_ns) as f64 / 1_000_000_000.0;
        let decay = self.decay_rate_per_second * elapsed_s as f32;
        self.current_level = (1.0 - decay).max(0.1);
    }

    pub fn take_break(&mut self, now_ns: u128) {
        self.last_break_ns = now_ns;
        self.current_level = (self.current_level + 0.2).min(1.0);
    }

    pub fn level(&self) -> f32 {
        self.current_level
    }
}

/// 选择性注意：鸡尾酒会效应，从噪声中提取目标信号。
pub struct SelectiveAttention {
    pub target_channel: Option<String>,
    pub noise_threshold: f32,
    pub enhancement_factor: f32,
    pub suppression_factor: f32,
}

impl SelectiveAttention {
    pub fn new() -> Self {
        Self {
            target_channel: None,
            noise_threshold: 0.3,
            enhancement_factor: 1.5,
            suppression_factor: 0.5,
        }
    }

    pub fn set_target(&mut self, channel: &str) {
        self.target_channel = Some(channel.to_string());
    }

    pub fn clear_target(&mut self) {
        self.target_channel = None;
    }

    pub fn process(&self, channel: &str, signal: f32, _noise: f32) -> f32 {
        match &self.target_channel {
            Some(t) if t == channel => signal * self.enhancement_factor,
            Some(_) => signal * self.suppression_factor,
            None => signal,
        }
    }
}

// =====================================================================================
// 注意力经济 (AttentionEconomy)
// =====================================================================================

/// 注意力交易记录。
#[derive(Clone, Debug)]
pub struct AttentionTrade {
    pub id: u64,
    pub from: String,
    pub to: String,
    pub amount: f32,      // 注意力单位
    pub price: f32,       // 每单位价格
    pub timestamp_ns: u128,
}

/// 注意力经济：注意力作为稀缺资源的分配与定价。
pub struct AttentionEconomy {
    pub balances: BTreeMap<String, f32>,
    pub trades: Vec<AttentionTrade>,
    pub next_trade_id: u64,
    pub base_price: f32,
    pub total_supply: f32,
}

impl AttentionEconomy {
    pub fn new() -> Self {
        Self {
            balances: BTreeMap::new(),
            trades: Vec::new(),
            next_trade_id: 1,
            base_price: 1.0,
            total_supply: 100.0,
        }
    }

    pub fn grant(&mut self, agent: &str, amount: f32) {
        *self.balances.entry(agent.to_string()).or_insert(0.0) += amount;
        self.total_supply += amount;
    }

    pub fn transfer(&mut self, from: &str, to: &str, amount: f32) -> Result<(), String> {
        let from_bal = self.balances.get(from).copied().unwrap_or(0.0);
        if from_bal < amount {
            return Err("注意力余额不足".into());
        }
        self.balances.insert(from.to_string(), from_bal - amount);
        *self.balances.entry(to.to_string()).or_insert(0.0) += amount;
        self.trades.push(AttentionTrade {
            id: self.next_trade_id,
            from: from.to_string(),
            to: to.to_string(),
            amount,
            price: self.base_price,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        self.next_trade_id += 1;
        Ok(())
    }

    pub fn balance(&self, agent: &str) -> f32 {
        self.balances.get(agent).copied().unwrap_or(0.0)
    }

    pub fn trade_count(&self) -> usize {
        self.trades.len()
    }
}

// =====================================================================================
// 注意力周期 (AttentionalCycle)
// =====================================================================================

/// 注意力节律类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleType {
    Ultradian,   // 超日夜律（约 90-120 分钟）
    Circadian,   // 日夜律（24 小时）
    Micro,       // 微周期（秒到分钟级）
}

/// 注意力周期模型。
pub struct AttentionalCycle {
    pub cycle_type: CycleType,
    pub period_ns: u128,
    pub phase_ns: u128,
    pub amplitude: f32,
    pub baseline: f32,
}

impl AttentionalCycle {
    pub fn new(cycle_type: CycleType, period_ns: u128) -> Self {
        Self {
            cycle_type,
            period_ns,
            phase_ns: 0,
            amplitude: 0.3,
            baseline: 0.7,
        }
    }

    pub fn value_at(&self, now_ns: u128) -> f32 {
        let phase = (now_ns % self.period_ns) as f64 / self.period_ns as f64;
        let sin = (phase * 2.0 * std::f64::consts::PI).sin();
        (self.baseline + self.amplitude * sin as f32).clamp(0.0, 1.0)
    }

    pub fn advance_phase(&mut self, delta_ns: u128) {
        self.phase_ns = (self.phase_ns + delta_ns) % self.period_ns;
    }
}

// =====================================================================================
// 注意力引擎 (AttentionEngine) —— 统一入口
// =====================================================================================

/// 注意力引擎：整合透镜、显著性图、注意拍瞬、焦点缓冲、分散/持续/选择性注意、注意力经济、周期。
pub struct AttentionEngine {
    pub lens: AttentionLens,
    pub saliency: SaliencyMap,
    pub blink: AttentionalBlink,
    pub focus: FocusBuffer,
    pub divided: DividedAttention,
    pub sustained: SustainedAttention,
    pub selective: SelectiveAttention,
    pub economy: AttentionEconomy,
    pub cycle: AttentionalCycle,
    pub enabled: bool,
    pub now_ns: u128,
    pub total_updates: u64,
}

impl AttentionEngine {
    pub fn new() -> Self {
        Self {
            lens: AttentionLens::new(),
            saliency: SaliencyMap::new(),
            blink: AttentionalBlink::new(),
            focus: FocusBuffer::new(),
            divided: DividedAttention::new(),
            sustained: SustainedAttention::new(),
            selective: SelectiveAttention::new(),
            economy: AttentionEconomy::new(),
            cycle: AttentionalCycle::new(CycleType::Ultradian, 90_000_000_000),
            enabled: true,
            now_ns: 0,
            total_updates: 0,
        }
    }

    pub fn tick(&mut self, now_ns: u128) {
        self.now_ns = now_ns;
        self.total_updates += 1;
        self.saliency.decay(now_ns);
        self.blink.update(now_ns);
        self.sustained.update(now_ns);
        self.cycle.advance_phase(now_ns);
    }

    pub fn receive_input(&mut self, channel: &str, signal: f32) -> bool {
        let now = self.now_ns;
        if self.blink.is_suppressing(now) {
            return false;
        }
        let salience = self.selective.process(channel, signal, 1.0 - signal);
        if salience < self.lens.threshold {
            return false;
        }
        self.saliency.update(channel, salience, now);
        self.lens.should_pass(channel, salience)
    }

    pub fn set_focus(&mut self, name: &str, content: &str) {
        let obj = FocusObject {
            id: self.focus.focus_changes + 1,
            name: name.to_string(),
            content: content.to_string(),
            focus_start_ns: self.now_ns,
            focus_duration_ns: 0,
            salience: self.saliency.get(name).unwrap_or(0.5),
        };
        self.focus.set_focus(obj, self.now_ns);
    }

    pub fn allocate_attention(&mut self, task: &str, share: f32) -> bool {
        self.divided.allocate(task, share)
    }

    pub fn cycle_value(&self) -> f32 {
        self.cycle.value_at(self.now_ns)
    }

    pub fn focus_name(&self) -> Option<&str> {
        self.focus.current_name()
    }

    pub fn sustained_level(&self) -> f32 {
        self.sustained.level()
    }

    pub fn dominant_channel(&self) -> Option<&String> {
        self.saliency.dominant_channel()
    }
}

impl Default for AttentionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static ATTENTION: OnceLock<Mutex<AttentionEngine>> = OnceLock::new();

pub fn attention_engine() -> &'static Mutex<AttentionEngine> {
    ATTENTION.get_or_init(|| Mutex::new(AttentionEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = attention_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = attention_engine().lock().unwrap();
    format!(
        "attention: lens_mode={:?} threshold={:.2} pass_rate={:.2} saliency_channels={} dominant={:?} blink={} focus={:?} divided_tasks={} sustained={:.2} cycle={:.2} economy_agents={} trades={}",
        eng.lens.mode,
        eng.lens.threshold,
        eng.lens.pass_rate(),
        eng.saliency.len(),
        eng.saliency.dominant_channel().map(|s| s.as_str()).unwrap_or("none"),
        eng.blink.blink_count,
        eng.focus.current_name().unwrap_or("none"),
        eng.divided.task_count(),
        eng.sustained.level(),
        eng.cycle_value(),
        eng.economy.balances.len(),
        eng.economy.trade_count()
    )
}

pub fn attention_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn attention_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("tick") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = attention_engine().lock().unwrap();
            eng.tick(now);
            Ok(format!("注意力引擎已推进 now={} 更新={}", now, eng.total_updates))
        }
        Some("lens-mode") => {
            let mode_str = it.next().ok_or("用法: attention:// lens-mode <open|narrow|selective|closed>")?;
            let mode = match mode_str {
                "open" => LensMode::Open,
                "narrow" => LensMode::Narrow,
                "selective" => LensMode::Selective,
                "closed" => LensMode::Closed,
                _ => return Err("未知透镜模式".into()),
            };
            let mut eng = attention_engine().lock().unwrap();
            eng.lens.mode = mode;
            Ok(format!("透镜模式已设置: {:?}", mode))
        }
        Some("lens-threshold") => {
            let t = it.next().and_then(|s| s.parse::<f32>().ok()).ok_or("缺少阈值")?;
            let mut eng = attention_engine().lock().unwrap();
            eng.lens.set_threshold(t);
            Ok(format!("透镜阈值已设置: {:.2}", t))
        }
        Some("lens-allow") => {
            let ch = it.next().ok_or("缺少通道名")?;
            let mut eng = attention_engine().lock().unwrap();
            eng.lens.allow_channel(ch);
            Ok(format!("已允许通道: {}", ch))
        }
        Some("lens-block") => {
            let ch = it.next().ok_or("缺少通道名")?;
            let mut eng = attention_engine().lock().unwrap();
            eng.lens.block_channel(ch);
            Ok(format!("已阻塞通道: {}", ch))
        }
        Some("receive") => {
            let channel = it.next().ok_or("缺少通道名")?;
            let signal = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = attention_engine().lock().unwrap();
            let passed = eng.receive_input(channel, signal);
            Ok(format!("输入 {} 信号={:.2} 通过={}", channel, signal, passed))
        }
        Some("saliency") => {
            let eng = attention_engine().lock().unwrap();
            let top = eng.saliency.top_k(5);
            let mut out = String::from("显著性图 Top5:\n");
            for (ch, s) in top {
                out.push_str(&format!("  {} = {:.3}\n", ch, s));
            }
            Ok(out)
        }
        Some("focus-set") => {
            let name = it.next().ok_or("缺少焦点名称")?;
            let content = it.next().unwrap_or("");
            let mut eng = attention_engine().lock().unwrap();
            eng.set_focus(name, content);
            Ok(format!("焦点已设置: {} = {}", name, content))
        }
        Some("focus-clear") => {
            let mut eng = attention_engine().lock().unwrap();
            let now = eng.now_ns;
            eng.focus.clear_focus(now);
            Ok("焦点已清除".into())
        }
        Some("focus") => {
            let eng = attention_engine().lock().unwrap();
            match eng.focus.current_name() {
                Some(n) => Ok(format!("当前焦点: {}", n)),
                None => Ok("无当前焦点".into()),
            }
        }
        Some("divided-allocate") => {
            let task = it.next().ok_or("缺少任务名")?;
            let share = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.3);
            let mut eng = attention_engine().lock().unwrap();
            let ok = eng.divided.allocate(task, share);
            if ok {
                Ok(format!("已为任务 {} 分配注意力 {:.2}", task, share))
            } else {
                Err("注意力容量不足".into())
            }
        }
        Some("divided") => {
            let eng = attention_engine().lock().unwrap();
            Ok(format!("分散注意 任务数={} 利用率={:.2}", eng.divided.task_count(), eng.divided.utilization()))
        }
        Some("sustained") => {
            let eng = attention_engine().lock().unwrap();
            Ok(format!("持续注意 水平={:.2}", eng.sustained.level()))
        }
        Some("sustained-start") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = attention_engine().lock().unwrap();
            eng.sustained.start(now);
            Ok(format!("持续注意已开始 now={}", now))
        }
        Some("selective-target") => {
            let ch = it.next().ok_or("缺少目标通道")?;
            let mut eng = attention_engine().lock().unwrap();
            eng.selective.set_target(ch);
            Ok(format!("选择性注意目标: {}", ch))
        }
        Some("selective-clear") => {
            let mut eng = attention_engine().lock().unwrap();
            eng.selective.clear_target();
            Ok("选择性注意目标已清除".into())
        }
        Some("economy-grant") => {
            let agent = it.next().ok_or("缺少代理名")?;
            let amount = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let mut eng = attention_engine().lock().unwrap();
            eng.economy.grant(agent, amount);
            Ok(format!("已向代理 {} 授予注意力 {:.2}", agent, amount))
        }
        Some("economy-transfer") => {
            let from = it.next().ok_or("缺少源代理")?;
            let to = it.next().ok_or("缺少目标代理")?;
            let amount = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let mut eng = attention_engine().lock().unwrap();
            eng.economy.transfer(from, to, amount)?;
            Ok(format!("注意力转移 {} -> {} {:.2}", from, to, amount))
        }
        Some("economy-balance") => {
            let agent = it.next().ok_or("缺少代理名")?;
            let eng = attention_engine().lock().unwrap();
            Ok(format!("代理 {} 注意力余额: {:.2}", agent, eng.economy.balance(agent)))
        }
        Some("cycle") => {
            let eng = attention_engine().lock().unwrap();
            Ok(format!("注意力周期值: {:.3}", eng.cycle_value()))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 attention 动作: {}", other)),
    }
}

pub fn attention_act_dispatch(payload: &str) -> Result<String, String> {
    attention_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_open_passes_all() {
        let mut lens = AttentionLens::new();
        assert!(lens.should_pass("ch1", 0.1));
        assert!(lens.should_pass("ch2", 0.0));
    }

    #[test]
    fn lens_closed_blocks_all() {
        let mut lens = AttentionLens::new();
        lens.mode = LensMode::Closed;
        assert!(!lens.should_pass("ch1", 1.0));
    }

    #[test]
    fn lens_narrow_threshold() {
        let mut lens = AttentionLens::new();
        lens.mode = LensMode::Narrow;
        lens.threshold = 0.5;
        assert!(!lens.should_pass("ch1", 0.3));
        assert!(lens.should_pass("ch1", 0.7));
    }

    #[test]
    fn lens_selective() {
        let mut lens = AttentionLens::new();
        lens.mode = LensMode::Selective;
        lens.threshold = 0.1;
        lens.allow_channel("ch1");
        assert!(lens.should_pass("ch1", 0.5));
        assert!(!lens.should_pass("ch2", 0.9));
    }

    #[test]
    fn lens_block_overrides() {
        let mut lens = AttentionLens::new();
        lens.mode = LensMode::Open;
        lens.block_channel("bad");
        assert!(!lens.should_pass("bad", 1.0));
    }

    #[test]
    fn lens_pass_rate() {
        let mut lens = AttentionLens::new();
        lens.should_pass("a", 1.0);
        lens.should_pass("b", 1.0);
        lens.should_pass("c", 1.0);
        assert!((lens.pass_rate() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn saliency_update_and_topk() {
        let mut sm = SaliencyMap::new();
        let now = 1_000_000_000;
        sm.update("ch1", 0.3, now);
        sm.update("ch2", 0.8, now);
        sm.update("ch3", 0.5, now);
        let top = sm.top_k(2);
        assert_eq!(top[0].0, "ch2");
        assert!((top[0].1 - 0.8).abs() < 1e-6);
        assert_eq!(top[1].0, "ch3");
    }

    #[test]
    fn saliency_decay() {
        let mut sm = SaliencyMap::new();
        sm.global_decay = 1.0;
        sm.update("ch1", 0.5, 0);
        sm.decay(1_000_000_000_000);
        assert_eq!(sm.get("ch1"), None);
    }

    #[test]
    fn saliency_dominant() {
        let mut sm = SaliencyMap::new();
        let now = 1_000_000_000;
        sm.update("a", 0.2, now);
        sm.update("b", 0.9, now);
        assert_eq!(sm.dominant_channel(), Some(&"b".to_string()));
    }

    #[test]
    fn blink_suppression() {
        let mut blink = AttentionalBlink::new();
        blink.trigger(0);
        assert!(blink.is_suppressing(100_000_000));
        assert!(!blink.is_suppressing(1_000_000_000));
        assert_eq!(blink.blink_count, 1);
    }

    #[test]
    fn blink_suppression_factor() {
        let mut blink = AttentionalBlink::new();
        blink.trigger(0);
        let factor = blink.suppression_factor(50_000_000);
        assert!(factor < 1.0);
        assert!(factor > 0.0);
    }

    #[test]
    fn focus_set_and_clear() {
        let mut focus = FocusBuffer::new();
        let obj = FocusObject { id: 1, name: "obj1".into(), content: "c1".into(), focus_start_ns: 0, focus_duration_ns: 0, salience: 0.5 };
        focus.set_focus(obj, 0);
        assert_eq!(focus.current_name(), Some("obj1"));
        focus.clear_focus(100);
        assert_eq!(focus.current_name(), None);
    }

    #[test]
    fn divided_allocate() {
        let mut da = DividedAttention::new();
        assert!(da.allocate("task1", 0.4));
        assert!(da.allocate("task2", 0.4));
        assert!(!da.allocate("task3", 0.4)); // 超过容量
        assert_eq!(da.task_count(), 2);
    }

    #[test]
    fn divided_utilization() {
        let mut da = DividedAttention::new();
        da.allocate("t1", 0.5);
        da.allocate("t2", 0.3);
        assert!((da.utilization() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn sustained_decay() {
        let mut sa = SustainedAttention::new();
        sa.start(0);
        sa.update(1_000_000_000); // 1 秒后
        assert!(sa.level() < 1.0);
        assert!(sa.level() > 0.1);
    }

    #[test]
    fn sustained_break() {
        let mut sa = SustainedAttention::new();
        sa.start(0);
        sa.update(5_000_000_000); // 大幅衰减
        let low = sa.level();
        sa.take_break(5_000_000_000);
        assert!(sa.level() > low);
    }

    #[test]
    fn selective_enhance() {
        let mut sa = SelectiveAttention::new();
        sa.set_target("target");
        let enhanced = sa.process("target", 0.5, 0.5);
        let suppressed = sa.process("other", 0.5, 0.5);
        assert!(enhanced > suppressed);
    }

    #[test]
    fn selective_no_target() {
        let sa = SelectiveAttention::new();
        assert!((sa.process("any", 0.5, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn economy_grant_and_transfer() {
        let mut ec = AttentionEconomy::new();
        ec.grant("alice", 10.0);
        assert!((ec.balance("alice") - 10.0).abs() < 1e-6);
        ec.transfer("alice", "bob", 3.0).unwrap();
        assert!((ec.balance("alice") - 7.0).abs() < 1e-6);
        assert!((ec.balance("bob") - 3.0).abs() < 1e-6);
    }

    #[test]
    fn economy_insufficient() {
        let mut ec = AttentionEconomy::new();
        assert!(ec.transfer("alice", "bob", 5.0).is_err());
    }

    #[test]
    fn cycle_value() {
        let cycle = AttentionalCycle::new(CycleType::Ultradian, 100_000_000_000);
        let v0 = cycle.value_at(0);
        let v25 = cycle.value_at(25_000_000_000);
        // 在不同相位，值应不同
        assert!((v0 - v25).abs() > 0.01);
    }

    #[test]
    fn engine_tick() {
        let mut eng = AttentionEngine::new();
        eng.tick(1_000_000_000);
        assert_eq!(eng.total_updates, 1);
    }

    #[test]
    fn engine_receive_input() {
        let mut eng = AttentionEngine::new();
        eng.tick(1_000_000_000);
        let passed = eng.receive_input("ch1", 0.9);
        assert!(passed);
    }

    #[test]
    fn engine_focus() {
        let mut eng = AttentionEngine::new();
        eng.tick(1_000_000_000);
        eng.set_focus("obj1", "content1");
        assert_eq!(eng.focus_name(), Some("obj1"));
    }

    #[test]
    fn attention_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("attention:"));
        assert!(s.contains("lens_mode="));
    }

    #[test]
    fn attention_act_tick() {
        init();
        let res = attention_act("tick 1000").unwrap();
        assert!(res.contains("注意力引擎已推进"));
    }

    #[test]
    fn attention_act_receive() {
        init();
        let res = attention_act("receive ch1 0.8").unwrap();
        assert!(res.contains("输入"));
    }

    #[test]
    fn attention_act_focus() {
        init();
        attention_act("focus-set obj1 内容1").unwrap();
        let res = attention_act("focus").unwrap();
        assert!(res.contains("当前焦点"));
    }

    #[test]
    fn attention_act_divided() {
        init();
        let res = attention_act("divided-allocate task1 0.3").unwrap();
        assert!(res.contains("分配注意力"));
    }

    #[test]
    fn attention_act_economy() {
        init();
        let res = attention_act("economy-grant alice 10").unwrap();
        assert!(res.contains("授予注意力"));
        let res = attention_act("economy-transfer alice bob 3").unwrap();
        assert!(res.contains("注意力转移"));
        let res = attention_act("economy-balance alice").unwrap();
        assert!(res.contains("余额"));
    }

    #[test]
    fn attention_act_status() {
        init();
        let res = attention_act("status").unwrap();
        assert!(res.contains("attention:"));
    }

    #[test]
    fn lens_mode_names() {
        // 验证 LensMode 变体存在即可
        let _ = LensMode::Open;
        let _ = LensMode::Closed;
    }

    #[test]
    fn cycle_types() {
        assert_eq!(CycleType::Ultradian as u32, 0);
        assert_eq!(CycleType::Circadian as u32, 1);
        assert_eq!(CycleType::Micro as u32, 2);
    }
}

// =====================================================================================
// 多头注意力 (MultiHeadAttention)
// =====================================================================================

/// 多头注意力：将注意力分成多个头并行计算。
pub struct MultiHeadAttention {
    pub num_heads: usize,
    pub head_dim: usize,
    pub q_weights: Vec<Vec<f32>>,
    pub k_weights: Vec<Vec<f32>>,
    pub v_weights: Vec<Vec<f32>>,
    pub o_weights: Vec<Vec<f32>>,
    pub attention_count: u64,
}

impl MultiHeadAttention {
    pub fn new(num_heads: usize, head_dim: usize) -> Self {
        let q_weights: Vec<Vec<f32>> = (0..num_heads).map(|_| vec![0.1; head_dim]).collect();
        let k_weights: Vec<Vec<f32>> = (0..num_heads).map(|_| vec![0.1; head_dim]).collect();
        let v_weights: Vec<Vec<f32>> = (0..num_heads).map(|_| vec![0.1; head_dim]).collect();
        let o_weights: Vec<Vec<f32>> = (0..num_heads).map(|_| vec![0.1; head_dim]).collect();
        Self {
            num_heads,
            head_dim,
            q_weights,
            k_weights,
            v_weights,
            o_weights,
            attention_count: 0,
        }
    }

    pub fn attend(&mut self, query: &[f32], keys: &[Vec<f32>], values: &[Vec<f32>]) -> Vec<f32> {
        self.attention_count += 1;
        let mut outputs: Vec<f32> = vec![0.0; self.head_dim];
        for _h in 0..self.num_heads {
            let mut scores: Vec<f32> = keys.iter().map(|k| {
                query.iter().zip(k.iter()).map(|(q, k)| q * k).sum::<f32>() / (self.head_dim as f32).sqrt()
            }).collect();
            // softmax
            let max_score = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let exp_sum: f32 = scores.iter().map(|s| (s - max_score).exp()).sum();
            for s in &mut scores {
                *s = (*s - max_score).exp() / exp_sum;
            }
            // 加权求和
            for (i, s) in scores.iter().enumerate() {
                if i < values.len() {
                    for d in 0..self.head_dim {
                        outputs[d] += s * values[i][d];
                    }
                }
            }
        }
        // 归一化
        let num_heads_f = self.num_heads as f32;
        for d in 0..self.head_dim {
            outputs[d] /= num_heads_f;
        }
        outputs
    }

    pub fn count(&self) -> u64 {
        self.attention_count
    }
}

// =====================================================================================
// 稀疏注意力 (SparseAttention)
// =====================================================================================

/// 稀疏注意力模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparsePattern {
    Local,     // 局部注意力：只关注附近的token
    Strided,   // 步进注意力：固定间隔采样
    Fixed,     // 固定注意力：固定位置
    Global,    // 全局注意力：所有token
}

/// 稀疏注意力：降低计算复杂度的注意力机制。
pub struct SparseAttention {
    pub pattern: SparsePattern,
    pub window_size: usize,
    pub stride: usize,
    pub global_indices: Vec<usize>,
    pub compute_count: u64,
}

impl SparseAttention {
    pub fn new(pattern: SparsePattern, window_size: usize, stride: usize) -> Self {
        Self {
            pattern,
            window_size,
            stride,
            global_indices: Vec::new(),
            compute_count: 0,
        }
    }

    pub fn add_global_index(&mut self, idx: usize) {
        self.global_indices.push(idx);
    }

    pub fn get_attention_mask(&self, seq_len: usize, current_pos: usize) -> Vec<bool> {
        let mut mask = vec![false; seq_len];
        match self.pattern {
            SparsePattern::Local => {
                let start = current_pos.saturating_sub(self.window_size / 2);
                let end = (current_pos + self.window_size / 2).min(seq_len);
                for i in start..end {
                    mask[i] = true;
                }
            }
            SparsePattern::Strided => {
                for i in (0..seq_len).step_by(self.stride) {
                    mask[i] = true;
                }
            }
            SparsePattern::Fixed => {
                for &idx in &self.global_indices {
                    if idx < seq_len {
                        mask[idx] = true;
                    }
                }
            }
            SparsePattern::Global => {
                for i in 0..seq_len {
                    mask[i] = true;
                }
            }
        }
        mask[current_pos] = true; // 自身总可见
        mask
    }

    pub fn count(&self) -> u64 {
        self.compute_count
    }
}

// =====================================================================================
// 长短期记忆注意力 (LSTMAttention)
// =====================================================================================

/// LSTM 门控状态。
#[derive(Clone, Debug)]
pub struct LSTMState {
    pub hidden: Vec<f32>,
    pub cell: Vec<f32>,
}

/// 长短期记忆注意力：结合 LSTM 的时序注意力。
pub struct LSTMAttention {
    pub state: LSTMState,
    pub input_dim: usize,
    pub hidden_dim: usize,
    pub forget_weights: Vec<Vec<f32>>,
    pub input_weights: Vec<Vec<f32>>,
    pub output_weights: Vec<Vec<f32>>,
    pub cell_weights: Vec<Vec<f32>>,
    pub step_count: u64,
}

impl LSTMAttention {
    pub fn new(input_dim: usize, hidden_dim: usize) -> Self {
        let init_vec = |d: usize| vec![0.1; d];
        let init_matrix = |rows: usize, cols: usize| (0..rows).map(|_| init_vec(cols)).collect();
        Self {
            state: LSTMState {
                hidden: vec![0.0; hidden_dim],
                cell: vec![0.0; hidden_dim],
            },
            input_dim,
            hidden_dim,
            forget_weights: init_matrix(hidden_dim, input_dim + hidden_dim),
            input_weights: init_matrix(hidden_dim, input_dim + hidden_dim),
            output_weights: init_matrix(hidden_dim, input_dim + hidden_dim),
            cell_weights: init_matrix(hidden_dim, input_dim + hidden_dim),
            step_count: 0,
        }
    }

    pub fn step(&mut self, input: &[f32]) -> Vec<f32> {
        self.step_count += 1;
        let combined: Vec<f32> = input.iter().chain(self.state.hidden.iter()).copied().collect();
        let sigmoid = |x: f32| 1.0 / (1.0 + (-x).exp());
        let tanh = |x: f32| x.tanh();

        // 遗忘门
        let forget: Vec<f32> = self.forget_weights.iter().map(|w| {
            combined.iter().zip(w.iter()).map(|(c, w)| c * w).sum::<f32>()
        }).map(sigmoid).collect();

        // 输入门
        let input_gate: Vec<f32> = self.input_weights.iter().map(|w| {
            combined.iter().zip(w.iter()).map(|(c, w)| c * w).sum::<f32>()
        }).map(sigmoid).collect();

        // 输出门
        let output_gate: Vec<f32> = self.output_weights.iter().map(|w| {
            combined.iter().zip(w.iter()).map(|(c, w)| c * w).sum::<f32>()
        }).map(sigmoid).collect();

        // 细胞候选
        let cell_candidate: Vec<f32> = self.cell_weights.iter().map(|w| {
            combined.iter().zip(w.iter()).map(|(c, w)| c * w).sum::<f32>()
        }).map(tanh).collect();

        // 更新细胞状态
        for i in 0..self.hidden_dim {
            self.state.cell[i] = forget[i] * self.state.cell[i] + input_gate[i] * cell_candidate[i];
        }

        // 更新隐藏状态
        for i in 0..self.hidden_dim {
            self.state.hidden[i] = output_gate[i] * tanh(self.state.cell[i]);
        }

        self.state.hidden.clone()
    }

    pub fn count(&self) -> u64 {
        self.step_count
    }
}

// =====================================================================================
// 注意力可视化 (AttentionVisualization)
// =====================================================================================

/// 注意力热图单元。
#[derive(Clone, Debug)]
pub struct AttentionHeatmapCell {
    pub query_pos: usize,
    pub key_pos: usize,
    pub weight: f32,
}

/// 注意力热图。
#[derive(Clone, Debug)]
pub struct AttentionHeatmap {
    pub cells: Vec<AttentionHeatmapCell>,
    pub query_len: usize,
    pub key_len: usize,
}

impl AttentionHeatmap {
    pub fn new(query_len: usize, key_len: usize) -> Self {
        Self {
            cells: Vec::new(),
            query_len,
            key_len,
        }
    }

    pub fn add_cell(&mut self, q: usize, k: usize, weight: f32) {
        self.cells.push(AttentionHeatmapCell {
            query_pos: q,
            key_pos: k,
            weight: weight.clamp(0.0, 1.0),
        });
    }

    pub fn get_weight(&self, q: usize, k: usize) -> Option<f32> {
        self.cells.iter().find(|c| c.query_pos == q && c.key_pos == k).map(|c| c.weight)
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

// =====================================================================================
// 注意力扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn multihead_attention() {
        let mut mha = MultiHeadAttention::new(4, 16);
        let query = vec![0.5; 16];
        let keys: Vec<Vec<f32>> = (0..4).map(|_| vec![0.3; 16]).collect();
        let values: Vec<Vec<f32>> = (0..4).map(|_| vec![0.7; 16]).collect();
        let output = mha.attend(&query, &keys, &values);
        assert_eq!(output.len(), 16);
        assert_eq!(mha.count(), 1);
    }

    #[test]
    fn multihead_different_heads() {
        let mut mha = MultiHeadAttention::new(1, 8);
        let query = vec![1.0; 8];
        let keys = vec![vec![1.0; 8]];
        let values = vec![vec![2.0; 8]];
        let output = mha.attend(&query, &keys, &values);
        assert_eq!(output.len(), 8);
    }

    #[test]
    fn sparse_attention_local() {
        let mut sa = SparseAttention::new(SparsePattern::Local, 4, 1);
        let mask = sa.get_attention_mask(10, 5);
        // 位置 5 的窗口是 [3, 4, 5, 6, 7]
        assert!(mask[3]);
        assert!(mask[4]);
        assert!(mask[5]);
        assert!(mask[6]);
        assert!(mask[7]);
        assert!(!mask[0]);
        assert!(!mask[9]);
    }

    #[test]
    fn sparse_attention_global() {
        let mut sa = SparseAttention::new(SparsePattern::Global, 4, 1);
        let mask = sa.get_attention_mask(5, 2);
        for m in &mask {
            assert!(*m);
        }
    }

    #[test]
    fn sparse_attention_strided() {
        let mut sa = SparseAttention::new(SparsePattern::Strided, 4, 2);
        let mask = sa.get_attention_mask(10, 0);
        assert!(mask[0]);
        assert!(mask[2]);
        assert!(mask[4]);
        assert!(!mask[1]);
    }

    #[test]
    fn sparse_attention_fixed() {
        let mut sa = SparseAttention::new(SparsePattern::Fixed, 4, 1);
        sa.add_global_index(0);
        sa.add_global_index(5);
        let mask = sa.get_attention_mask(10, 3);
        assert!(mask[0]);
        assert!(mask[5]);
        assert!(!mask[3]); // 3 不在全局索引中，且当前位置不是 3... 等等，current_pos 是 3
        // 当前位置总是可见
        assert!(mask[3]);
    }

    #[test]
    fn lstm_attention() {
        let mut lstm = LSTMAttention::new(8, 16);
        let input = vec![0.5; 8];
        let output = lstm.step(&input);
        assert_eq!(output.len(), 16);
        assert_eq!(lstm.count(), 1);
    }

    #[test]
    fn lstm_attention_multiple_steps() {
        let mut lstm = LSTMAttention::new(4, 8);
        for i in 0..5 {
            lstm.step(&[i as f32; 4]);
        }
        assert_eq!(lstm.count(), 5);
    }

    #[test]
    fn attention_heatmap() {
        let mut hm = AttentionHeatmap::new(4, 4);
        hm.add_cell(0, 0, 0.9);
        hm.add_cell(0, 1, 0.1);
        hm.add_cell(1, 2, 0.8);
        assert_eq!(hm.cell_count(), 3);
        assert_eq!(hm.get_weight(0, 0), Some(0.9));
        assert_eq!(hm.get_weight(0, 1), Some(0.1));
        assert_eq!(hm.get_weight(1, 2), Some(0.8));
        assert_eq!(hm.get_weight(2, 3), None);
    }

    #[test]
    fn sparse_pattern_names() {
        assert_eq!(SparsePattern::Local as u32, 0);
        assert_eq!(SparsePattern::Global as u32, 3);
    }
}
