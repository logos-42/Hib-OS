//! 多模态感知 —— 图像/文本/音频特征提取与模态融合。
//!
//! `MultimodalSense` 提供 `sense_image` / `sense_text` / `sense_audio` / `fuse`，
//! 把不同模态的感知统一为 `UnifiedPerception`，带注意力权重与主要模态判断。
//! 特征提取为简化版：图像直方图、文本词袋、音频频谱摘要。
//! 与 `kernel/ai_token.rs` 的概念图互补：感知结果可注册为概念节点。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 模态
// ============================================================================

/// 感知模态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modality {
    Image,
    Text,
    Audio,
    Video,
    Sensor,
}

impl Modality {
    pub fn name(self) -> &'static str {
        match self {
            Modality::Image => "image",
            Modality::Text => "text",
            Modality::Audio => "audio",
            Modality::Video => "video",
            Modality::Sensor => "sensor",
        }
    }
}

// ============================================================================
// 感知
// ============================================================================

/// 单模态感知结果。
#[derive(Clone, Debug)]
pub struct Perception {
    pub modality: Modality,
    pub confidence: f64, // 0.0-1.0
    pub features: Vec<f32>, // 特征向量
    pub metadata: BTreeMap<String, String>,
    pub timestamp: u64, // ns
}

impl Perception {
    pub fn new(modality: Modality, confidence: f64, features: Vec<f32>) -> Self {
        Self {
            modality,
            confidence: confidence.clamp(0.0, 1.0),
            features,
            metadata: BTreeMap::new(),
            timestamp: crate::monitor::get_time_ns(),
        }
    }

    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(key.to_string(), value.to_string());
        self
    }

    /// 特征向量 L2 范数。
    pub fn norm(&self) -> f64 {
        self.features.iter().map(|f| (*f as f64).powi(2)).sum::<f64>().sqrt()
    }

    /// 归一化特征（原地）。
    pub fn normalize(&mut self) {
        let n = self.norm();
        if n > 0.0 {
            for f in self.features.iter_mut() {
                *f = (*f as f64 / n) as f32;
            }
        }
    }
}

/// 融合后的统一感知。
#[derive(Clone, Debug)]
pub struct UnifiedPerception {
    pub unified_vector: Vec<f32>,
    pub attention_weights: BTreeMap<Modality, f64>,
    pub primary_modality: Modality,
    pub confidence: f64, // 0.0-1.0
    pub timestamp: u64,
    pub source_count: usize,
}

impl UnifiedPerception {
    pub fn new(
        unified_vector: Vec<f32>,
        attention_weights: BTreeMap<Modality, f64>,
        primary_modality: Modality,
        confidence: f64,
    ) -> Self {
        Self {
            unified_vector,
            attention_weights,
            primary_modality,
            confidence: confidence.clamp(0.0, 1.0),
            timestamp: crate::monitor::get_time_ns(),
            source_count: attention_weights.len(),
        }
    }

    /// 返回主模态的注意力权重。
    pub fn primary_weight(&self) -> f64 {
        self.attention_weights
            .get(&self.primary_modality)
            .copied()
            .unwrap_or(0.0)
    }
}

// ============================================================================
// 特征提取（简化版）
// ============================================================================

/// 图像特征：灰度直方图（32 桶）+ 均值方差。
fn extract_image_features(data: &[u8]) -> Vec<f32> {
    let mut hist = [0u32; 32];
    let mut sum: u64 = 0;
    let mut count: usize = 0;
    for &b in data {
        let bucket = (b as usize) / 8;
        hist[bucket.min(31)] += 1;
        sum += b as u64;
        count += 1;
    }
    let mean = if count > 0 { sum as f64 / count as f64 } else { 0.0 };
    let mut var: f64 = 0.0;
    for &b in data {
        let d = b as f64 - mean;
        var += d * d;
    }
    if count > 0 {
        var /= count as f64;
    }
    let mut features: Vec<f32> = hist.iter().map(|&h| h as f32).collect();
    features.push(mean as f32);
    features.push(var as f32);
    features
}

/// 文本特征：词袋（前 64 个高频词哈希）+ 字符数 + 行数。
fn extract_text_features(text: &str) -> Vec<f32> {
    let mut bucket = [0u32; 64];
    let mut char_count = 0usize;
    let mut line_count = 1usize;
    for ch in text.chars() {
        char_count += 1;
        if ch == '\n' {
            line_count += 1;
        }
        let idx = (ch as u32) as usize % 64;
        bucket[idx] += 1;
    }
    let mut features: Vec<f32> = bucket.iter().map(|&b| b as f32).collect();
    features.push(char_count as f32);
    features.push(line_count as f32);
    features
}

/// 音频特征：频谱摘要（简化：按字节做简单的能量分桶）。
fn extract_audio_features(data: &[u8]) -> Vec<f32> {
    let mut energy = [0u32; 16];
    for (i, &b) in data.iter().enumerate() {
        let bucket = i % 16;
        energy[bucket] += b as u32;
    }
    let total: u32 = energy.iter().sum();
    let mut features: Vec<f32> = energy.iter().map(|&e| e as f32).collect();
    features.push(total as f32);
    features.push(data.len() as f32);
    features
}

// ============================================================================
// 多模态感知器
// ============================================================================

/// 多模态感知器：分模态提取特征，再做注意力融合。
pub struct MultimodalSense {
    /// 最近一次每模态感知。
    last_perceptions: BTreeMap<Modality, Perception>,
    /// 最近一次融合结果。
    last_fusion: Option<UnifiedPerception>,
    /// 累计感知次数。
    sense_count: u64,
    /// 融合次数。
    fusion_count: u64,
}

impl Default for MultimodalSense {
    fn default() -> Self {
        Self::new()
    }
}

impl MultimodalSense {
    pub fn new() -> Self {
        Self {
            last_perceptions: BTreeMap::new(),
            last_fusion: None,
            sense_count: 0,
            fusion_count: 0,
        }
    }

    /// 感知图像数据。
    pub fn sense_image(&mut self, data: &[u8]) -> Perception {
        let features = extract_image_features(data);
        // 置信度基于数据长度与特征非零比例。
        let nonzero = features.iter().filter(|&&f| f > 0.0).count() as f64 / features.len() as f64;
        let confidence = (nonzero.min(1.0) * 0.7 + 0.3).clamp(0.0, 1.0);
        let mut p = Perception::new(Modality::Image, confidence, features);
        p = p.with_metadata("size_bytes", &data.len().to_string());
        p = p.with_metadata("feature_dim", &features.len().to_string());
        self.last_perceptions.insert(Modality::Image, p.clone());
        self.sense_count += 1;
        p
    }

    /// 感知文本数据。
    pub fn sense_text(&mut self, text: &str) -> Perception {
        let features = extract_text_features(text);
        let nonempty = if text.is_empty() { 0.0 } else { 1.0 };
        let confidence = (nonempty * 0.8 + 0.2).clamp(0.0, 1.0);
        let mut p = Perception::new(Modality::Text, confidence, features);
        p = p.with_metadata("char_count", &text.chars().count().to_string());
        p = p.with_metadata("byte_count", &text.len().to_string());
        self.last_perceptions.insert(Modality::Text, p.clone());
        self.sense_count += 1;
        p
    }

    /// 感知音频数据。
    pub fn sense_audio(&mut self, data: &[u8]) -> Perception {
        let features = extract_audio_features(data);
        let nonzero = features.iter().filter(|&&f| f > 0.0).count() as f64 / features.len() as f64;
        let confidence = (nonzero.min(1.0) * 0.6 + 0.4).clamp(0.0, 1.0);
        let mut p = Perception::new(Modality::Audio, confidence, features);
        p = p.with_metadata("size_bytes", &data.len().to_string());
        self.last_perceptions.insert(Modality::Audio, p.clone());
        self.sense_count += 1;
        p
    }

    /// 融合多个感知为统一感知（注意力机制）。
    pub fn fuse(&mut self, perceptions: Vec<Perception>) -> UnifiedPerception {
        if perceptions.is_empty() {
            let empty = UnifiedPerception::new(
                Vec::new(),
                BTreeMap::new(),
                Modality::Text,
                0.0,
            );
            self.last_fusion = Some(empty.clone());
            self.fusion_count += 1;
            return empty;
        }

        // 归一化各感知特征。
        let mut normalized: Vec<(Modality, Vec<f32>, f64)> = Vec::new();
        for p in &perceptions {
            let mut features = p.features.clone();
            let mut tmp = Perception {
                modality: p.modality,
                confidence: p.confidence,
                features,
                metadata: p.metadata.clone(),
                timestamp: p.timestamp,
            };
            tmp.normalize();
            normalized.push((p.modality, tmp.features.clone(), p.confidence));
        }

        // 注意力权重：基于置信度做 softmax。
        let total_conf: f64 = normalized.iter().map(|(_, _, c)| c).sum();
        let mut attention_weights = BTreeMap::new();
        let mut primary = perceptions[0].modality;
        let mut primary_weight = 0.0f64;
        for (modality, _, conf) in &normalized {
            let w = if total_conf > 0.0 { *conf / total_conf } else { 1.0 / normalized.len() as f64 };
            attention_weights.insert(*modality, w);
            if *w > primary_weight {
                primary_weight = *w;
                primary = *modality;
            }
        }

        // 统一向量：按注意力加权拼接（取最大特征维度，填充对齐）。
        let max_dim = normalized.iter().map(|(_, f, _)| f.len()).max().unwrap_or(0);
        let mut unified_vector = vec![0.0f32; max_dim];
        let aw = attention_weights.clone();
        for (modality, features, conf) in &normalized {
            let w = aw[modality];
            for (i, f) in features.iter().enumerate() {
                if i < max_dim {
                    unified_vector[i] += (*f as f64 * w) as f32;
                }
            }
        }
        let unified_vector: Vec<f32> = unified_vector.into_iter().map(|v| v as f32).collect();

        // 综合置信度：加权平均。
        let confidence = normalized
            .iter()
            .map(|(_, _, c)| c)
            .sum::<f64>()
            / normalized.len() as f64;

        let fusion = UnifiedPerception::new(
            unified_vector,
            attention_weights,
            primary,
            confidence,
        );
        self.last_fusion = Some(fusion.clone());
        self.fusion_count += 1;
        fusion
    }

    /// 使用最近一次各模态感知做融合。
    pub fn fuse_last(&mut self) -> Option<UnifiedPerception> {
        let perceptions: Vec<Perception> = self.last_perceptions.values().cloned().collect();
        if perceptions.is_empty() {
            return None;
        }
        Some(self.fuse(perceptions))
    }

    /// 返回最近一次融合结果。
    pub fn last_fusion(&self) -> Option<&UnifiedPerception> {
        self.last_fusion.as_ref()
    }

    /// 返回指定模态最近感知。
    pub fn last_perception(&self, modality: Modality) -> Option<&Perception> {
        self.last_perceptions.get(&modality)
    }

    /// 返回感知总次数。
    pub fn sense_count(&self) -> u64 {
        self.sense_count
    }

    /// 返回融合总次数。
    pub fn fusion_count(&self) -> u64 {
        self.fusion_count
    }

    /// 模态对齐：检查各模态时间戳是否在容差内（视为对齐）。
    pub fn is_aligned(&self, tolerance_ns: u64) -> bool {
        let times: Vec<u64> = self
            .last_perceptions
            .values()
            .map(|p| p.timestamp)
            .collect();
        if times.len() < 2 {
            return true;
        }
        let min = times.iter().min().copied().unwrap_or(0);
        let max = times.iter().max().copied().unwrap_or(0);
        max - min <= tolerance_ns
    }
}

// ============================================================================
// 全局 MultimodalSense 单例（供 kernel/sense.rs 调用）
// ============================================================================

static MULTIMODAL_SENSE: OnceLock<Mutex<MultimodalSense>> = OnceLock::new();

/// 获取全局多模态感知器。
pub fn multimodal_sense() -> &'static Mutex<MultimodalSense> {
    MULTIMODAL_SENSE.get_or_init(|| Mutex::new(MultimodalSense::new()))
}

/// `sense ai://multimodal`：返回多模态感知器状态摘要。
pub fn multimodal_sense_status() -> String {
    let m = multimodal_sense().lock().unwrap();
    let mut lines = vec![format!(
        "sense_count={} fusion_count={} modalities={}",
        m.sense_count(),
        m.fusion_count(),
        m.last_perceptions.len()
    )];
    for (modality, p) in &m.last_perceptions {
        lines.push(format!(
            "  {:?}: confidence={:.2} features={}",
            modality.name(),
            p.confidence,
            p.features.len()
        ));
    }
    if let Some(f) = m.last_fusion() {
        lines.push(format!(
            "  fusion: primary={} confidence={:.2} dim={}",
            f.primary_modality.name(),
            f.confidence,
            f.unified_vector.len()
        ));
    }
    lines.join("\n")
}

/// `act ai://multimodal <op> ...`：对多模态感知器执行行动。
pub fn multimodal_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let m = multimodal_sense();
    let mut m = m.lock().unwrap();
    match it.next().unwrap_or("") {
        "sense_image" => {
            let data = it.next().ok_or("缺少数据")?;
            let p = m.sense_image(data.as_bytes());
            Ok(format!(
                "image: confidence={:.2} features={}",
                p.confidence,
                p.features.len()
            ))
        }
        "sense_text" => {
            let text = it.next().ok_or("缺少文本")?;
            let p = m.sense_text(text);
            Ok(format!(
                "text: confidence={:.2} features={}",
                p.confidence,
                p.features.len()
            ))
        }
        "sense_audio" => {
            let data = it.next().ok_or("缺少数据")?;
            let p = m.sense_audio(data.as_bytes());
            Ok(format!(
                "audio: confidence={:.2} features={}",
                p.confidence,
                p.features.len()
            ))
        }
        "fuse" => {
            // 简化：融合最近各模态感知。
            if let Some(f) = m.fuse_last() {
                Ok(format!(
                    "fused: primary={} confidence={:.2} dim={}",
                    f.primary_modality.name(),
                    f.confidence,
                    f.unified_vector.len()
                ))
            } else {
                Err("无可用感知进行融合".to_string())
            }
        }
        "status" => Ok(multimodal_sense_status()),
        _ => Err(
            "用法: multimodal sense_image|sense_text|sense_audio|fuse|status".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_names() {
        assert_eq!(Modality::Image.name(), "image");
        assert_eq!(Modality::Text.name(), "text");
        assert_eq!(Modality::Audio.name(), "audio");
        assert_eq!(Modality::Video.name(), "video");
        assert_eq!(Modality::Sensor.name(), "sensor");
    }

    #[test]
    fn perception_new_and_norm() {
        let p = Perception::new(Modality::Text, 0.9, vec![1.0, 2.0, 3.0]);
        assert!((p.confidence - 0.9).abs() < f64::EPSILON);
        assert!((p.norm() - (14.0_f64).sqrt()).abs() < f64::EPSILON);
        let mut p2 = p.clone();
        p2.normalize();
        assert!((p2.norm() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn extract_image_features_nonzero() {
        let data: Vec<u8> = (0..256).collect();
        let features = extract_image_features(&data);
        assert_eq!(features.len(), 34); // 32 buckets + mean + var
        assert!(features.iter().any(|&f| f > 0.0));
    }

    #[test]
    fn extract_text_features() {
        let text = "hello world\nhello rust";
        let features = extract_text_features(text);
        assert_eq!(features.len(), 66);
        assert!(features.iter().any(|&f| f > 0.0));
    }

    #[test]
    fn extract_audio_features() {
        let data: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let features = extract_audio_features(&data);
        assert_eq!(features.len(), 18);
    }

    #[test]
    fn sense_image_text_audio() {
        let mut m = MultimodalSense::new();
        let p = m.sense_image(&[1, 2, 3]);
        assert_eq!(p.modality, Modality::Image);
        assert!(p.confidence > 0.0);
        let p = m.sense_text("hello");
        assert_eq!(p.modality, Modality::Text);
        let p = m.sense_audio(&[10, 20]);
        assert_eq!(p.modality, Modality::Audio);
        assert_eq!(m.sense_count(), 3);
    }

    #[test]
    fn fuse_perceptions() {
        let mut m = MultimodalSense::new();
        let p1 = m.sense_text("hello world");
        let p2 = m.sense_image(&[1, 2, 3, 4]);
        let fused = m.fuse(vec![p1, p2]);
        assert!(!fused.unified_vector.is_empty());
        assert!(fused.confidence > 0.0);
        assert!(fused.primary_weight() > 0.0);
        assert_eq!(m.fusion_count(), 1);
    }

    #[test]
    fn fuse_empty() {
        let mut m = MultimodalSense::new();
        let fused = m.fuse(Vec::new());
        assert!(fused.unified_vector.is_empty());
        assert_eq!(fused.confidence, 0.0);
    }

    #[test]
    fn fuse_last() {
        let mut m = MultimodalSense::new();
        m.sense_text("hi");
        let fused = m.fuse_last();
        assert!(fused.is_some());
        assert_eq!(m.last_fusion().unwrap().primary_modality, Modality::Text);
    }

    #[test]
    fn is_aligned() {
        let mut m = MultimodalSense::new();
        // 只有一个模态，视为对齐。
        m.sense_text("hi");
        assert!(m.is_aligned(1_000_000));
    }

    #[test]
    fn unified_perception_primary_weight() {
        let mut weights = BTreeMap::new();
        weights.insert(Modality::Image, 0.7);
        weights.insert(Modality::Text, 0.3);
        let up = UnifiedPerception::new(
            vec![1.0, 2.0],
            weights,
            Modality::Image,
            0.8,
        );
        assert!((up.primary_weight() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn multimodal_sense_and_act() {
        let mut m = multimodal_sense().lock().unwrap();
        m.sense_text("hello");
        drop(m);
        let status = multimodal_sense_status();
        assert!(status.contains("sense_count="));
        let res = multimodal_act("sense_text hello").unwrap();
        assert!(res.contains("text:"));
        let res = multimodal_act("fuse").unwrap();
        assert!(res.contains("fused:"));
        let res = multimodal_act("status").unwrap();
        assert!(status.contains("modalities="));
    }

    #[test]
    fn confidence_clamp() {
        let mut m = MultimodalSense::new();
        // 置信度在 0-1 之间。
        let p = m.sense_text("x");
        assert!(p.confidence >= 0.0 && p.confidence <= 1.0);
    }
}
