//! 多模态感知层 —— AI 原生 OS 的「感知系统」 (Multi-modal Perception)
//!
//! 本模块实现 AI 原生 OS 的多模态感知，对应传统 OS 中的「中断控制器 /
//! I/O 设备驱动」概念，但重新映射到感知域：
//!
//! * PerceptionChannel (感知通道) —— 单一感官输入通道，等价于中断请求线 (IRQ)；
//! * SensoryModality (感觉模态) —— 视觉/听觉/触觉/嗅觉/味觉/本体感/内脏感；
//! * PerceptionEvent (感知事件) —— 来自感官的原始事件，等价于中断向量；
//! * PerceptionPipeline (感知管线) —— 原始信号 → 特征提取 → 语义化；
//! * FeatureExtractor (特征提取器) —— 边缘/频率/纹理/语义特征；
//! * MultiModalFusion (多模态融合) —— 将不同模态信号融合为统一感知；
//! * PerceptionBinding (感知绑定) —— 将分散特征绑定为单一感知对象；
//! * PerceptionAttention (感知注意) —— 感知层面的选择性过滤；
//! * SensorRegistry (传感器注册表) —— 可用感知设备的注册与发现；
//! * PerceptionThreshold (感知阈值) —— 各通道的激活阈值，等价于中断屏蔽；
//! * PerceptionLatency (感知延迟) —— 各通道端到端延迟监控；
//! * PerceptionEngine (感知引擎) —— 统一入口，通过 perception://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 感觉模态 (SensoryModality)
// =====================================================================================

/// 感觉模态：感知系统的输入类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SensoryModality {
    /// 视觉（光、颜色、形状、运动、深度）。
    Visual,
    /// 听觉（声波、频率、音量、方位）。
    Auditory,
    /// 触觉（压力、温度、疼痛、纹理、振动）。
    Tactile,
    /// 嗅觉（气味分子特征）。
    Olfactory,
    /// 味觉（甜、酸、苦、咸、鲜）。
    Gustatory,
    /// 本体感觉（身体位置、肌张力、平衡、关节角度）。
    Proprioceptive,
    /// 内脏感觉（心跳、呼吸、饥饿、口渴、血氧）。
    Interoceptive,
    /// 前庭感觉（加速度、重力、头部方位）。
    Vestibular,
}

impl SensoryModality {
    pub fn name(self) -> &'static str {
        match self {
            SensoryModality::Visual => "visual",
            SensoryModality::Auditory => "auditory",
            SensoryModality::Tactile => "tactile",
            SensoryModality::Olfactory => "olfactory",
            SensoryModality::Gustatory => "gustatory",
            SensoryModality::Proprioceptive => "proprioceptive",
            SensoryModality::Interoceptive => "interoceptive",
            SensoryModality::Vestibular => "vestibular",
        }
    }

    /// 默认感知阈值（0~1，低于此值的信号被滤除）。
    pub fn default_threshold(self) -> f32 {
        match self {
            SensoryModality::Visual => 0.3,
            SensoryModality::Auditory => 0.25,
            SensoryModality::Tactile => 0.2,
            SensoryModality::Olfactory => 0.4,
            SensoryModality::Gustatory => 0.4,
            SensoryModality::Proprioceptive => 0.15,
            SensoryModality::Interoceptive => 0.3,
            SensoryModality::Vestibular => 0.35,
        }
    }
}

// =====================================================================================
// 感知通道 (PerceptionChannel)
// =====================================================================================

/// 感知通道：单一感官的输入通道，等价于 IRQ 线。
#[derive(Clone, Debug)]
pub struct PerceptionChannel {
    pub id: u64,
    pub name: String,
    pub modality: SensoryModality,
    pub source: String,            // 传感器设备名
    pub threshold: f32,            // 激活阈值
    pub enabled: bool,
    pub event_count: u64,
    pub last_event_ns: Option<u128>,
    pub avg_latency_ns: u128,
    pub dropped_events: u64,
}

impl PerceptionChannel {
    pub fn new(id: u64, name: &str, modality: SensoryModality, source: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            modality,
            source: source.to_string(),
            threshold: modality.default_threshold(),
            enabled: true,
            event_count: 0,
            last_event_ns: None,
            avg_latency_ns: 0,
            dropped_events: 0,
        }
    }

    pub fn set_threshold(&mut self, t: f32) {
        self.threshold = t.clamp(0.0, 1.0);
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn record_event(&mut self, latency_ns: u128) {
        self.event_count += 1;
        self.last_event_ns = Some(crate::ai_os::native::now_ns());
        self.avg_latency_ns = (self.avg_latency_ns + latency_ns) / 2;
    }

    pub fn record_drop(&mut self) {
        self.dropped_events += 1;
    }

    pub fn is_saturated(&self) -> bool {
        self.dropped_events > 100
    }
}

// =====================================================================================
// 感知事件 (PerceptionEvent)
// =====================================================================================

/// 感知事件：来自感官的原始信号包。
#[derive(Clone, Debug)]
pub struct PerceptionEvent {
    pub id: u64,
    pub channel_id: u64,
    pub modality: SensoryModality,
    pub timestamp_ns: u128,
    pub payload: Vec<f32>,         // 原始特征向量
    pub intensity: f32,            // 信号强度 0~1
    pub confidence: f32,           // 置信度 0~1
    pub spatial: Option<[f32; 3]>, // 空间位置 (x, y, z)
    pub metadata: BTreeMap<String, String>,
}

impl PerceptionEvent {
    pub fn new(channel_id: u64, modality: SensoryModality, payload: Vec<f32>) -> Self {
        let intensity = payload.iter().fold(0.0f32, |acc, &x| acc + x.abs()) / payload.len() as f32;
        Self {
            id: 0,
            channel_id,
            modality,
            timestamp_ns: crate::ai_os::native::now_ns(),
            payload,
            intensity: intensity.clamp(0.0, 1.0),
            confidence: 1.0,
            spatial: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_confidence(mut self, c: f32) -> Self {
        self.confidence = c.clamp(0.0, 1.0);
        self
    }

    pub fn with_spatial(mut self, x: f32, y: f32, z: f32) -> Self {
        self.spatial = Some([x, y, z]);
        self
    }

    pub fn passes_threshold(&self, threshold: f32) -> bool {
        self.intensity >= threshold && self.confidence >= threshold
    }
}

// =====================================================================================
// 特征提取器 (FeatureExtractor)
// =====================================================================================

/// 特征类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FeatureType {
    Edge,       // 边缘特征
    Frequency,  // 频率特征
    Texture,    // 纹理特征
    Semantic,   // 语义特征
    Temporal,   // 时序特征
    Spatial,    // 空间特征
}

/// 特征提取器：将原始感知信号转换为结构化特征。
pub struct FeatureExtractor {
    pub enabled_features: BTreeSet<FeatureType>,
    pub extraction_count: u64,
}

impl FeatureExtractor {
    pub fn new() -> Self {
        Self {
            enabled_features: BTreeSet::new(),
            extraction_count: 0,
        }
    }

    pub fn enable(&mut self, ft: FeatureType) {
        self.enabled_features.insert(ft);
    }

    pub fn disable(&mut self, ft: FeatureType) {
        self.enabled_features.remove(&ft);
    }

    pub fn extract(&mut self, event: &PerceptionEvent) -> BTreeMap<FeatureType, Vec<f32>> {
        self.extraction_count += 1;
        let mut features = BTreeMap::new();

        if self.enabled_features.contains(&FeatureType::Edge) {
            // 简化：计算 payload 中相邻元素差值的绝对和作为边缘强度
            let edge: Vec<f32> = event
                .payload
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .collect();
            features.insert(FeatureType::Edge, edge);
        }

        if self.enabled_features.contains(&FeatureType::Frequency) {
            // 简化：payload 中非零元素比例作为频率特征
            let freq: Vec<f32> = event
                .payload
                .iter()
                .map(|x| if *x != 0.0 { 1.0 } else { 0.0 })
                .collect();
            features.insert(FeatureType::Frequency, freq);
        }

        if self.enabled_features.contains(&FeatureType::Texture) {
            // 简化：payload 方差作为纹理特征
            let mean = event.payload.iter().sum::<f32>() / event.payload.len() as f32;
            let var = event
                .payload
                .iter()
                .map(|x| (x - mean) * (x - mean))
                .sum::<f32>()
                / event.payload.len() as f32;
            features.insert(FeatureType::Texture, vec![var]);
        }

        if self.enabled_features.contains(&FeatureType::Semantic) {
            // 简化：将 payload 归一化为语义向量
            let norm: f32 = event.payload.iter().map(|x| x * x).sum::<f32>().sqrt();
            let semantic: Vec<f32> = if norm > 0.0 {
                event.payload.iter().map(|x| x / norm).collect()
            } else {
                event.payload.clone()
            };
            features.insert(FeatureType::Semantic, semantic);
        }

        if self.enabled_features.contains(&FeatureType::Temporal) {
            // 简化：时间戳作为时序特征
            features.insert(
                FeatureType::Temporal,
                vec![event.timestamp_ns as f32 / 1_000_000_000.0],
            );
        }

        if self.enabled_features.contains(&FeatureType::Spatial) {
            if let Some([x, y, z]) = event.spatial {
                features.insert(FeatureType::Spatial, vec![x, y, z]);
            }
        }

        features
    }

    pub fn count(&self) -> u64 {
        self.extraction_count
    }
}

impl Default for FeatureExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 多模态融合 (MultiModalFusion)
// =====================================================================================

/// 融合策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FusionStrategy {
    /// 早期融合：在特征级别直接拼接。
    Early,
    /// 晚期融合：在决策级别加权融合。
    Late,
    /// 注意力融合：基于各模态可靠性动态加权。
    Attention,
}

/// 多模态融合器：将不同模态的特征融合为统一感知表征。
pub struct MultiModalFusion {
    pub strategy: FusionStrategy,
    pub modality_weights: BTreeMap<SensoryModality, f32>,
    pub fusion_count: u64,
}

impl MultiModalFusion {
    pub fn new() -> Self {
        let mut weights = BTreeMap::new();
        weights.insert(SensoryModality::Visual, 0.35);
        weights.insert(SensoryModality::Auditory, 0.25);
        weights.insert(SensoryModality::Tactile, 0.15);
        weights.insert(SensoryModality::Olfactory, 0.05);
        weights.insert(SensoryModality::Gustatory, 0.05);
        weights.insert(SensoryModality::Proprioceptive, 0.1);
        weights.insert(SensoryModality::Interoceptive, 0.05);
        weights.insert(SensoryModality::Vestibular, 0.05);
        Self {
            strategy: FusionStrategy::Attention,
            modality_weights: weights,
            fusion_count: 0,
        }
    }

    pub fn set_weight(&mut self, modality: SensoryModality, weight: f32) {
        self.modality_weights.insert(modality, weight.clamp(0.0, 1.0));
    }

    pub fn fuse(
        &mut self,
        features: &BTreeMap<SensoryModality, BTreeMap<FeatureType, Vec<f32>>>,
    ) -> Vec<f32> {
        self.fusion_count += 1;
        let mut fused: Vec<f32> = Vec::new();
        let dim = 64; // 固定融合维度

        for (modality, feat_map) in features {
            let w = self.modality_weights.get(modality).copied().unwrap_or(0.1);
            let mut modality_vec = vec![0.0f32; dim];
            for (_, feats) in feat_map {
                for (i, f) in feats.iter().enumerate() {
                    modality_vec[i % dim] += f * w;
                }
            }
            for (i, v) in modality_vec.iter().enumerate() {
                if i < fused.len() {
                    fused[i] += v;
                } else {
                    fused.push(*v);
                }
            }
        }

        // 归一化
        let norm: f32 = fused.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            fused = fused.iter().map(|x| x / norm).collect();
        }

        fused
    }

    pub fn count(&self) -> u64 {
        self.fusion_count
    }
}

impl Default for MultiModalFusion {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 感知绑定 (PerceptionBinding)
// =====================================================================================

/// 感知对象：经过绑定的统一感知对象。
#[derive(Clone, Debug)]
pub struct PerceptionObject {
    pub id: u64,
    pub name: String,
    pub modalities: BTreeSet<SensoryModality>,
    pub features: BTreeMap<FeatureType, Vec<f32>>,
    pub salience: f32,
    pub created_ns: u128,
    pub last_updated_ns: u128,
}

/// 感知绑定器：将分散的多模态特征绑定为单一感知对象。
pub struct PerceptionBinding {
    objects: BTreeMap<u64, PerceptionObject>,
    next_id: u64,
    pub binding_window_ns: u128,
}

impl PerceptionBinding {
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            next_id: 1,
            binding_window_ns: 100_000_000, // 100ms
        }
    }

    pub fn bind(&mut self, modality: SensoryModality, features: BTreeMap<FeatureType, Vec<f32>>) -> u64 {
        // 简化：同一时间窗口内的不同模态特征绑定为一个对象
        let now = crate::ai_os::native::now_ns();
        let mut best_obj_id = None;
        let mut best_salience = 0.0f32;

        for (id, obj) in self.objects.iter() {
            if now - obj.last_updated_ns > self.binding_window_ns {
                continue;
            }
            let salience = obj.features.values().map(|v| v.iter().sum::<f32>()).sum::<f32>();
            if salience > best_salience {
                best_salience = salience;
                best_obj_id = Some(*id);
            }
        }

        match best_obj_id {
            Some(id) => {
                if let Some(obj) = self.objects.get_mut(&id) {
                    obj.modalities.insert(modality);
                    for (ft, feats) in features {
                        obj.features.insert(ft, feats);
                    }
                    obj.last_updated_ns = now;
                    obj.salience += 0.1;
                }
                id
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                let salience = features.values().map(|v| v.iter().sum::<f32>()).sum::<f32>();
                self.objects.insert(
                    id,
                    PerceptionObject {
                        id,
                        name: format!("object_{}", id),
                        modalities: BTreeSet::from([modality]),
                        features,
                        salience,
                        created_ns: now,
                        last_updated_ns: now,
                    },
                );
                id
            }
        }
    }

    pub fn get(&self, id: u64) -> Option<&PerceptionObject> {
        self.objects.get(&id)
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn top_salient(&self, k: usize) -> Vec<&PerceptionObject> {
        let mut v: Vec<&PerceptionObject> = self.objects.values().collect();
        v.sort_by(|a, b| b.salience.partial_cmp(&a.salience).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(k);
        v
    }
}

// =====================================================================================
// 感知注意 (PerceptionAttention)
// =====================================================================================

/// 感知注意：感知层面的选择性过滤。
pub struct PerceptionAttention {
    pub enabled_channels: BTreeSet<u64>,
    pub blocked_channels: BTreeSet<u64>,
    pub focus_channel: Option<u64>,
    pub filter_count: u64,
    pub pass_count: u64,
}

impl PerceptionAttention {
    pub fn new() -> Self {
        Self {
            enabled_channels: BTreeSet::new(),
            blocked_channels: BTreeSet::new(),
            focus_channel: None,
            filter_count: 0,
            pass_count: 0,
        }
    }

    pub fn enable_channel(&mut self, id: u64) {
        self.enabled_channels.insert(id);
        self.blocked_channels.remove(&id);
    }

    pub fn block_channel(&mut self, id: u64) {
        self.blocked_channels.insert(id);
        self.enabled_channels.remove(&id);
    }

    pub fn set_focus(&mut self, id: Option<u64>) {
        self.focus_channel = id;
    }

    pub fn filter(&mut self, channel_id: u64, intensity: f32, threshold: f32) -> bool {
        self.filter_count += 1;
        if self.blocked_channels.contains(&channel_id) {
            return false;
        }
        if let Some(focus) = self.focus_channel {
            if channel_id != focus {
                // 非焦点通道需更高强度
                if intensity < threshold * 1.5 {
                    return false;
                }
            }
        }
        if intensity < threshold {
            return false;
        }
        self.pass_count += 1;
        true
    }

    pub fn pass_rate(&self) -> f32 {
        if self.filter_count == 0 {
            0.0
        } else {
            self.pass_count as f32 / self.filter_count as f32
        }
    }
}

// =====================================================================================
// 传感器注册表 (SensorRegistry)
// =====================================================================================

/// 传感器类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorType {
    Camera,
    Microphone,
    TouchSensor,
    GasSensor,
    TasteSensor,
    PositionSensor,
    HeartRateSensor,
    Accelerometer,
}

/// 传感器信息。
#[derive(Clone, Debug)]
pub struct SensorInfo {
    pub id: u64,
    pub name: String,
    pub sensor_type: SensorType,
    pub modality: SensoryModality,
    pub sample_rate_hz: f32,
    pub resolution: String,
    pub online: bool,
}

/// 传感器注册表：可用感知设备的注册与发现。
pub struct SensorRegistry {
    sensors: BTreeMap<u64, SensorInfo>,
    next_id: u64,
    channel_map: BTreeMap<u64, u64>, // sensor_id -> channel_id
}

impl SensorRegistry {
    pub fn new() -> Self {
        Self {
            sensors: BTreeMap::new(),
            next_id: 1,
            channel_map: BTreeMap::new(),
        }
    }

    pub fn register(
        &mut self,
        name: &str,
        sensor_type: SensorType,
        modality: SensoryModality,
        sample_rate_hz: f32,
        resolution: &str,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.sensors.insert(
            id,
            SensorInfo {
                id,
                name: name.to_string(),
                sensor_type,
                modality,
                sample_rate_hz,
                resolution: resolution.to_string(),
                online: true,
            },
        );
        id
    }

    pub fn link_channel(&mut self, sensor_id: u64, channel_id: u64) {
        self.channel_map.insert(sensor_id, channel_id);
    }

    pub fn get(&self, id: u64) -> Option<&SensorInfo> {
        self.sensors.get(&id)
    }

    pub fn online_count(&self) -> u64 {
        self.sensors.values().filter(|s| s.online).count() as u64
    }

    pub fn total_count(&self) -> u64 {
        self.sensors.len() as u64
    }

    pub fn by_modality(&self, modality: SensoryModality) -> Vec<&SensorInfo> {
        self.sensors.values().filter(|s| s.modality == modality).collect()
    }
}

// =====================================================================================
// 感知延迟监控 (PerceptionLatency)
// =====================================================================================

/// 通道延迟记录。
#[derive(Clone, Debug)]
pub struct LatencyRecord {
    pub channel_id: u64,
    pub latency_ns: u128,
    pub timestamp_ns: u128,
}

/// 感知延迟监控器：各通道端到端延迟监控。
pub struct PerceptionLatency {
    records: VecDeque<LatencyRecord>,
    max_records: usize,
    pub avg_latency_ns: u128,
    pub max_latency_ns: u128,
}

impl PerceptionLatency {
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            max_records: 10000,
            avg_latency_ns: 0,
            max_latency_ns: 0,
        }
    }

    pub fn record(&mut self, channel_id: u64, latency_ns: u128) {
        self.records.push_back(LatencyRecord {
            channel_id,
            latency_ns,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        if self.records.len() > self.max_records {
            self.records.pop_front();
        }

        let sum: u128 = self.records.iter().map(|r| r.latency_ns).sum();
        self.avg_latency_ns = sum / self.records.len() as u128;
        self.max_latency_ns = self
            .records
            .iter()
            .map(|r| r.latency_ns)
            .max()
            .unwrap_or(0);
    }

    pub fn is_high_latency(&self, threshold_ns: u128) -> bool {
        self.avg_latency_ns > threshold_ns
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 感知引擎 (PerceptionEngine) —— 统一入口
// =====================================================================================

/// 感知引擎：整合通道、事件、特征提取、多模态融合、感知绑定、感知注意、传感器注册表、延迟监控。
pub struct PerceptionEngine {
    pub channels: BTreeMap<u64, PerceptionChannel>,
    pub events: VecDeque<PerceptionEvent>,
    pub next_event_id: u64,
    pub feature_extractor: FeatureExtractor,
    pub fusion: MultiModalFusion,
    pub binding: PerceptionBinding,
    pub attention: PerceptionAttention,
    pub sensors: SensorRegistry,
    pub latency: PerceptionLatency,
    pub max_events: usize,
    pub enabled: bool,
    pub total_events: u64,
    pub total_drops: u64,
}

impl PerceptionEngine {
    pub fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            events: VecDeque::new(),
            next_event_id: 1,
            feature_extractor: FeatureExtractor::new(),
            fusion: MultiModalFusion::new(),
            binding: PerceptionBinding::new(),
            attention: PerceptionAttention::new(),
            sensors: SensorRegistry::new(),
            latency: PerceptionLatency::new(),
            max_events: 100000,
            enabled: true,
            total_events: 0,
            total_drops: 0,
        }
    }

    pub fn add_channel(&mut self, name: &str, modality: SensoryModality, source: &str) -> u64 {
        let id = self.channels.len() as u64 + 1;
        let channel = PerceptionChannel::new(id, name, modality, source);
        self.channels.insert(id, channel);
        id
    }

    pub fn inject_event(&mut self, channel_id: u64, payload: Vec<f32>) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        let channel = self.channels.get(&channel_id)?;
        if !channel.enabled {
            return None;
        }
        let mut event = PerceptionEvent::new(channel_id, channel.modality, payload);
        event.id = self.next_event_id;
        self.next_event_id += 1;

        // 感知注意过滤
        let threshold = channel.threshold;
        if !self.attention.filter(channel_id, event.intensity, threshold) {
            self.total_drops += 1;
            if let Some(ch) = self.channels.get_mut(&channel_id) {
                ch.record_drop();
            }
            return None;
        }

        // 特征提取
        let features = self.feature_extractor.extract(&event);

        // 多模态融合输入准备：按模态分组
        let mut modality_features: BTreeMap<SensoryModality, BTreeMap<FeatureType, Vec<f32>>> =
            BTreeMap::new();
        modality_features.insert(channel.modality, features.clone());

        // 多模态融合
        let fused = self.fusion.fuse(&modality_features);

        // 感知绑定
        let obj_id = self.binding.bind(channel.modality, features);

        // 延迟记录
        let now = crate::ai_os::native::now_ns();
        let latency = now.saturating_sub(event.timestamp_ns);
        self.latency.record(channel_id, latency);

        // 更新通道统计
        if let Some(ch) = self.channels.get_mut(&channel_id) {
            ch.record_event(latency);
        }

        // 存储事件
        self.events.push_back(event);
        if self.events.len() > self.max_events {
            self.events.pop_front();
        }
        self.total_events += 1;

        Some(obj_id)
    }

    pub fn get_event(&self, id: u64) -> Option<&PerceptionEvent> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn recent_events(&self, n: usize) -> Vec<&PerceptionEvent> {
        self.events.iter().rev().take(n).collect()
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn object_count(&self) -> usize {
        self.binding.len()
    }

    pub fn tick(&mut self, now_ns: u128) {
        // 感知引擎周期性维护
        let _ = now_ns;
    }
}

impl Default for PerceptionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static PERCEPTION: OnceLock<Mutex<PerceptionEngine>> = OnceLock::new();

pub fn perception_engine() -> &'static Mutex<PerceptionEngine> {
    PERCEPTION.get_or_init(|| Mutex::new(PerceptionEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = perception_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = perception_engine().lock().unwrap();
    format!(
        "perception: channels={} events={} drops={} objects={} fusion_count={} extraction_count={} pass_rate={:.2} avg_latency_ns={} sensors_online={}/{}",
        eng.channel_count(),
        eng.total_events,
        eng.total_drops,
        eng.object_count(),
        eng.fusion.count(),
        eng.feature_extractor.count(),
        eng.attention.pass_rate(),
        eng.latency.avg_latency_ns,
        eng.sensors.online_count(),
        eng.sensors.total_count()
    )
}

pub fn perception_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn perception_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("add-channel") => {
            let name = it.next().ok_or("用法: perception:// add-channel <名称> <模态> <来源>")?;
            let modality_str = it.next().unwrap_or("visual");
            let modality = match modality_str {
                "visual" => SensoryModality::Visual,
                "auditory" => SensoryModality::Auditory,
                "tactile" => SensoryModality::Tactile,
                "olfactory" => SensoryModality::Olfactory,
                "gustatory" => SensoryModality::Gustatory,
                "proprioceptive" => SensoryModality::Proprioceptive,
                "interoceptive" => SensoryModality::Interoceptive,
                "vestibular" => SensoryModality::Vestibular,
                _ => SensoryModality::Visual,
            };
            let source = it.next().unwrap_or("sensor");
            let mut eng = perception_engine().lock().unwrap();
            let id = eng.add_channel(name, modality, source);
            Ok(format!("已添加感知通道 id={} 名称={} 模态={:?}", id, name, modality))
        }
        Some("inject") => {
            let channel_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let values: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
            if values.is_empty() {
                return Err("缺少感知数值".into());
            }
            let mut eng = perception_engine().lock().unwrap();
            match eng.inject_event(channel_id, values) {
                Some(obj_id) => Ok(format!("感知事件已处理 绑定对象id={}", obj_id)),
                None => Err("感知事件被过滤或通道不存在".into()),
            }
        }
        Some("channel-threshold") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let t = it.next().and_then(|s| s.parse::<f32>().ok()).ok_or("缺少阈值")?;
            let mut eng = perception_engine().lock().unwrap();
            if let Some(ch) = eng.channels.get_mut(&id) {
                ch.set_threshold(t);
                Ok(format!("通道 {} 阈值已设置 {:.2}", id, t))
            } else {
                Err(format!("通道 {} 不存在", id))
            }
        }
        Some("channel-enable") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let mut eng = perception_engine().lock().unwrap();
            if let Some(ch) = eng.channels.get_mut(&id) {
                ch.enable();
                Ok(format!("通道 {} 已启用", id))
            } else {
                Err(format!("通道 {} 不存在", id))
            }
        }
        Some("channel-disable") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let mut eng = perception_engine().lock().unwrap();
            if let Some(ch) = eng.channels.get_mut(&id) {
                ch.disable();
                Ok(format!("通道 {} 已禁用", id))
            } else {
                Err(format!("通道 {} 不存在", id))
            }
        }
        Some("focus-channel") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let mut eng = perception_engine().lock().unwrap();
            eng.attention.set_focus(Some(id));
            Ok(format!("感知注意焦点已设置: 通道{}", id))
        }
        Some("focus-clear") => {
            let mut eng = perception_engine().lock().unwrap();
            eng.attention.set_focus(None);
            Ok("感知注意焦点已清除".into())
        }
        Some("block-channel") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let mut eng = perception_engine().lock().unwrap();
            eng.attention.block_channel(id);
            Ok(format!("通道 {} 已阻塞", id))
        }
        Some("unblock-channel") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少通道id")?;
            let mut eng = perception_engine().lock().unwrap();
            eng.attention.enable_channel(id);
            Ok(format!("通道 {} 已启用", id))
        }
        Some("feature-enable") => {
            let ft_str = it.next().unwrap_or("semantic");
            let ft = match ft_str {
                "edge" => FeatureType::Edge,
                "frequency" => FeatureType::Frequency,
                "texture" => FeatureType::Texture,
                "semantic" => FeatureType::Semantic,
                "temporal" => FeatureType::Temporal,
                "spatial" => FeatureType::Spatial,
                _ => FeatureType::Semantic,
            };
            let mut eng = perception_engine().lock().unwrap();
            eng.feature_extractor.enable(ft);
            Ok(format!("特征提取器已启用: {:?}", ft))
        }
        Some("fusion-weight") => {
            let mod_str = it.next().unwrap_or("visual");
            let modality = match mod_str {
                "visual" => SensoryModality::Visual,
                "auditory" => SensoryModality::Auditory,
                "tactile" => SensoryModality::Tactile,
                _ => SensoryModality::Visual,
            };
            let w = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = perception_engine().lock().unwrap();
            eng.fusion.set_weight(modality, w);
            Ok(format!("多模态融合权重已设置 {:?}={:.2}", modality, w))
        }
        Some("sensor-register") => {
            let name = it.next().ok_or("缺少传感器名称")?;
            let type_str = it.next().unwrap_or("camera");
            let sensor_type = match type_str {
                "camera" => SensorType::Camera,
                "microphone" => SensorType::Microphone,
                "touch" => SensorType::TouchSensor,
                "gas" => SensorType::GasSensor,
                "taste" => SensorType::TasteSensor,
                "position" => SensorType::PositionSensor,
                "heartrate" => SensorType::HeartRateSensor,
                "accelerometer" => SensorType::Accelerometer,
                _ => SensorType::Camera,
            };
            let mod_str = it.next().unwrap_or("visual");
            let modality = match mod_str {
                "visual" => SensoryModality::Visual,
                "auditory" => SensoryModality::Auditory,
                "tactile" => SensoryModality::Tactile,
                _ => SensoryModality::Visual,
            };
            let rate = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(30.0);
            let resolution = it.next().unwrap_or("1080p");
            let mut eng = perception_engine().lock().unwrap();
            let id = eng.sensors.register(name, sensor_type, modality, rate, resolution);
            Ok(format!("已注册传感器 id={} 名称={}", id, name))
        }
        Some("top-objects") => {
            let k = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(5);
            let eng = perception_engine().lock().unwrap();
            let objs = eng.binding.top_salient(k);
            let mut out = String::from("顶部感知对象:\n");
            for obj in objs {
                out.push_str(&format!(
                    "  id={} 名称={} 显著性={:.3} 模态数={}\n",
                    obj.id,
                    obj.name,
                    obj.salience,
                    obj.modalities.len()
                ));
            }
            Ok(out)
        }
        Some("tick") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = perception_engine().lock().unwrap();
            eng.tick(now);
            Ok(format!("感知引擎已推进 now={}", now))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 perception 动作: {}", other)),
    }
}

pub fn perception_act_dispatch(payload: &str) -> Result<String, String> {
    perception_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_names() {
        assert_eq!(SensoryModality::Visual.name(), "visual");
        assert_eq!(SensoryModality::Auditory.name(), "auditory");
        assert_eq!(SensoryModality::Tactile.name(), "tactile");
        assert_eq!(SensoryModality::Proprioceptive.name(), "proprioceptive");
    }

    #[test]
    fn modality_default_threshold() {
        assert!((SensoryModality::Visual.default_threshold() - 0.3).abs() < 1e-6);
        assert!((SensoryModality::Proprioceptive.default_threshold() - 0.15).abs() < 1e-6);
    }

    #[test]
    fn channel_create_and_threshold() {
        let mut ch = PerceptionChannel::new(1, "cam1".into(), SensoryModality::Visual, "camera0");
        assert_eq!(ch.id, 1);
        assert!(ch.enabled);
        ch.set_threshold(0.7);
        assert!((ch.threshold - 0.7).abs() < 1e-6);
    }

    #[test]
    fn channel_enable_disable() {
        let mut ch = PerceptionChannel::new(1, "cam1".into(), SensoryModality::Visual, "camera0");
        ch.disable();
        assert!(!ch.enabled);
        ch.enable();
        assert!(ch.enabled);
    }

    #[test]
    fn channel_record_event() {
        let mut ch = PerceptionChannel::new(1, "cam1".into(), SensoryModality::Visual, "camera0");
        ch.record_event(5_000_000);
        assert_eq!(ch.event_count, 1);
        assert_eq!(ch.avg_latency_ns, 5_000_000);
    }

    #[test]
    fn channel_record_drop() {
        let mut ch = PerceptionChannel::new(1, "cam1".into(), SensoryModality::Visual, "camera0");
        ch.record_drop();
        assert_eq!(ch.dropped_events, 1);
    }

    #[test]
    fn event_creation() {
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.5, 0.3, 0.8]);
        assert_eq!(ev.channel_id, 1);
        assert_eq!(ev.modality, SensoryModality::Visual);
        assert!(ev.intensity > 0.0);
    }

    #[test]
    fn event_passes_threshold() {
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.9, 0.9, 0.9]);
        assert!(ev.passes_threshold(0.5));
        assert!(!ev.passes_threshold(0.95));
    }

    #[test]
    fn event_with_confidence() {
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.5]).with_confidence(0.7);
        assert!((ev.confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn event_with_spatial() {
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.5]).with_spatial(1.0, 2.0, 3.0);
        assert_eq!(ev.spatial, Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn feature_extractor_edge() {
        let mut fe = FeatureExtractor::new();
        fe.enable(FeatureType::Edge);
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.0, 1.0, 0.0, 1.0]);
        let feats = fe.extract(&ev);
        assert!(feats.contains_key(&FeatureType::Edge));
        assert_eq!(fe.count(), 1);
    }

    #[test]
    fn feature_extractor_all() {
        let mut fe = FeatureExtractor::new();
        for ft in [
            FeatureType::Edge,
            FeatureType::Frequency,
            FeatureType::Texture,
            FeatureType::Semantic,
            FeatureType::Temporal,
            FeatureType::Spatial,
        ] {
            fe.enable(ft);
        }
        let ev = PerceptionEvent::new(1, SensoryModality::Visual, vec![0.5, 0.5, 0.5])
            .with_spatial(1.0, 2.0, 3.0);
        let feats = fe.extract(&ev);
        assert_eq!(feats.len(), 6);
    }

    #[test]
    fn fusion_early() {
        let mut fus = MultiModalFusion::new();
        fus.strategy = FusionStrategy::Early;
        let mut features = BTreeMap::new();
        features.insert(
            SensoryModality::Visual,
            BTreeMap::from([(FeatureType::Semantic, vec![1.0, 0.0, 0.0])]),
        );
        let result = fus.fuse(&features);
        assert!(!result.is_empty());
        assert_eq!(fus.count(), 1);
    }

    #[test]
    fn fusion_attention() {
        let mut fus = MultiModalFusion::new();
        fus.set_weight(SensoryModality::Visual, 0.8);
        fus.set_weight(SensoryModality::Auditory, 0.2);
        let mut features = BTreeMap::new();
        features.insert(
            SensoryModality::Visual,
            BTreeMap::from([(FeatureType::Semantic, vec![1.0; 64])]),
        );
        features.insert(
            SensoryModality::Auditory,
            BTreeMap::from([(FeatureType::Semantic, vec![0.0; 64])]),
        );
        let result = fus.fuse(&features);
        assert!(!result.is_empty());
    }

    #[test]
    fn binding_create_object() {
        let mut bind = PerceptionBinding::new();
        let feats = BTreeMap::from([(FeatureType::Semantic, vec![0.5, 0.5])]);
        let id = bind.bind(SensoryModality::Visual, feats);
        assert_eq!(id, 1);
        let obj = bind.get(id).unwrap();
        assert_eq!(obj.modalities.len(), 1);
        assert_eq!(bind.len(), 1);
    }

    #[test]
    fn binding_merge_modalities() {
        let mut bind = PerceptionBinding::new();
        let now = crate::ai_os::native::now_ns();
        bind.binding_window_ns = u128::MAX; // 确保同一对象
        let id1 = bind.bind(
            SensoryModality::Visual,
            BTreeMap::from([(FeatureType::Semantic, vec![0.5])]),
        );
        let id2 = bind.bind(
            SensoryModality::Auditory,
            BTreeMap::from([(FeatureType::Semantic, vec![0.5])]),
        );
        // 在同一时间窗口内应绑定到同一对象
        assert_eq!(id1, id2);
        let obj = bind.get(id1).unwrap();
        assert_eq!(obj.modalities.len(), 2);
    }

    #[test]
    fn binding_top_salient() {
        let mut bind = PerceptionBinding::new();
        bind.bind(
            SensoryModality::Visual,
            BTreeMap::from([(FeatureType::Semantic, vec![10.0])]),
        );
        bind.bind(
            SensoryModality::Auditory,
            BTreeMap::from([(FeatureType::Semantic, vec![1.0])]),
        );
        let top = bind.top_salient(1);
        assert_eq!(top.len(), 1);
        assert!(top[0].salience > 5.0);
    }

    #[test]
    fn perception_attention_filter() {
        let mut att = PerceptionAttention::new();
        att.enable_channel(1);
        assert!(att.filter(1, 0.8, 0.5));
        att.block_channel(1);
        assert!(!att.filter(1, 0.9, 0.5));
    }

    #[test]
    fn perception_attention_focus() {
        let mut att = PerceptionAttention::new();
        att.set_focus(Some(1));
        // 非焦点通道需更高强度
        assert!(!att.filter(2, 0.6, 0.5));
        assert!(att.filter(1, 0.6, 0.5));
    }

    #[test]
    fn sensor_registry() {
        let mut sr = SensorRegistry::new();
        let id = sr.register("cam0", SensorType::Camera, SensoryModality::Visual, 30.0, "1080p");
        assert_eq!(id, 1);
        let info = sr.get(id).unwrap();
        assert_eq!(info.name, "cam0");
        assert_eq!(sr.online_count(), 1);
        assert_eq!(sr.total_count(), 1);
        assert_eq!(sr.by_modality(SensoryModality::Visual).len(), 1);
    }

    #[test]
    fn sensor_link_channel() {
        let mut sr = SensorRegistry::new();
        let sid = sr.register("s1", SensorType::Camera, SensoryModality::Visual, 30.0, "1080p");
        sr.link_channel(sid, 10);
        assert!(sr.channel_map.contains_key(&sid));
    }

    #[test]
    fn latency_record() {
        let mut lat = PerceptionLatency::new();
        lat.record(1, 1_000_000);
        lat.record(1, 2_000_000);
        assert_eq!(lat.record_count(), 2);
        assert!(lat.avg_latency_ns > 0);
        assert_eq!(lat.max_latency_ns, 2_000_000);
    }

    #[test]
    fn engine_add_channel() {
        let mut eng = PerceptionEngine::new();
        let id = eng.add_channel("cam1".into(), SensoryModality::Visual, "camera0");
        assert_eq!(id, 1);
        assert_eq!(eng.channel_count(), 1);
    }

    #[test]
    fn engine_inject_event() {
        let mut eng = PerceptionEngine::new();
        let ch_id = eng.add_channel("cam1".into(), SensoryModality::Visual, "camera0");
        let obj_id = eng.inject_event(ch_id, vec![0.8, 0.2, 0.9]);
        assert!(obj_id.is_some());
        assert!(eng.total_events > 0);
    }

    #[test]
    fn engine_inject_filtered() {
        let mut eng = PerceptionEngine::new();
        let ch_id = eng.add_channel("cam1".into(), SensoryModality::Visual, "camera0");
        eng.channels.get_mut(&ch_id).unwrap().set_threshold(0.95);
        let obj_id = eng.inject_event(ch_id, vec![0.1, 0.1]);
        assert!(obj_id.is_none());
        assert_eq!(eng.total_drops, 1);
    }

    #[test]
    fn engine_recent_events() {
        let mut eng = PerceptionEngine::new();
        let ch_id = eng.add_channel("cam1".into(), SensoryModality::Visual, "camera0");
        eng.inject_event(ch_id, vec![0.5]);
        eng.inject_event(ch_id, vec![0.6]);
        let recent = eng.recent_events(2);
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn perception_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("perception:"));
        assert!(s.contains("channels="));
    }

    #[test]
    fn perception_act_add_channel() {
        init();
        let res = perception_act("add-channel cam1 visual camera0").unwrap();
        assert!(res.contains("已添加感知通道"));
    }

    #[test]
    fn perception_act_inject() {
        init();
        perception_act("add-channel cam1 visual camera0").unwrap();
        let res = perception_act("inject 1 0.8 0.2 0.9").unwrap();
        assert!(res.contains("感知事件已处理"));
    }

    #[test]
    fn perception_act_threshold() {
        init();
        perception_act("add-channel cam1 visual camera0").unwrap();
        let res = perception_act("channel-threshold 1 0.7").unwrap();
        assert!(res.contains("阈值已设置"));
    }

    #[test]
    fn perception_act_focus() {
        init();
        perception_act("add-channel cam1 visual camera0").unwrap();
        let res = perception_act("focus-channel 1").unwrap();
        assert!(res.contains("焦点已设置"));
        let res = perception_act("focus-clear").unwrap();
        assert!(res.contains("焦点已清除"));
    }

    #[test]
    fn perception_act_feature() {
        init();
        let res = perception_act("feature-enable semantic").unwrap();
        assert!(res.contains("特征提取器已启用"));
    }

    #[test]
    fn perception_act_fusion_weight() {
        init();
        let res = perception_act("fusion-weight visual 0.8").unwrap();
        assert!(res.contains("多模态融合权重已设置"));
    }

    #[test]
    fn perception_act_sensor() {
        init();
        let res = perception_act("sensor-register cam0 camera visual 30.0 1080p").unwrap();
        assert!(res.contains("已注册传感器"));
    }

    #[test]
    fn perception_act_top_objects() {
        init();
        perception_act("add-channel cam1 visual camera0").unwrap();
        perception_act("inject 1 0.8 0.2 0.9").unwrap();
        let res = perception_act("top-objects 3").unwrap();
        assert!(res.contains("顶部感知对象"));
    }

    #[test]
    fn perception_act_tick() {
        init();
        let res = perception_act("tick 1000000000").unwrap();
        assert!(res.contains("感知引擎已推进"));
    }

    #[test]
    fn perception_act_status() {
        init();
        let res = perception_act("status").unwrap();
        assert!(res.contains("perception:"));
    }

    #[test]
    fn fusion_strategy_names() {
        assert_eq!(FusionStrategy::Early as u32, 0);
        assert_eq!(FusionStrategy::Late as u32, 1);
        assert_eq!(FusionStrategy::Attention as u32, 2);
    }

    #[test]
    fn sensor_type_names() {
        assert_eq!(SensorType::Camera as u32, 0);
        assert_eq!(SensorType::Microphone as u32, 1);
    }
}


