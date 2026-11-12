//! 元认知 —— 自我监控、自我评估、自我修正与元学习。
//!
//! `Metacognition` 提供 `monitor`（思考/行动置信度） / `evaluate`（结果评估） /
//! `self_correct`（自我修正） / `learn_from_experience`（从经验学习）。
//! * `Confidence`: 数值 (0-1)、不确定度、置信区间
//! * `Evaluation`: 正确性、完整性、一致性、建议改进
//!
//! 与 `kernel/ai/reasoning.rs` 的推理引擎互补：推理引擎生成结论，元认知
//! 评估结论质量并触发修正。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 置信度
// ============================================================================

/// 置信度：数值 + 不确定度 + 置信区间。
#[derive(Clone, Debug, PartialEq)]
pub struct Confidence {
    pub value: f64,      // 0.0-1.0
    pub uncertainty: f64, // 0.0-1.0
    pub lower: f64,       // 区间下界
    pub upper: f64,       // 区间上界
}

impl Confidence {
    pub fn new(value: f64, uncertainty: f64) -> Self {
        let v = value.clamp(0.0, 1.0);
        let u = uncertainty.clamp(0.0, 1.0);
        let lower = (v - u).clamp(0.0, 1.0);
        let upper = (v + u).clamp(0.0, 1.0);
        Self {
            value: v,
            uncertainty: u,
            lower,
            upper,
        }
    }

    /// 是否可信（置信度高且不确定度低）。
    pub fn is_reliable(&self) -> bool {
        self.value >= 0.7 && self.uncertainty <= 0.3
    }

    /// 是否需要审查（低置信度或高不确定度）。
    pub fn needs_review(&self) -> bool {
        self.value < 0.5 || self.uncertainty > 0.5
    }

    pub fn label(&self) -> &'static str {
        if self.value >= 0.9 {
            "很高"
        } else if self.value >= 0.7 {
            "高"
        } else if self.value >= 0.5 {
            "中"
        } else if self.value >= 0.3 {
            "低"
        } else {
            "很低"
        }
    }
}

// ============================================================================
// 评估
// ============================================================================

/// 评估结果。
#[derive(Clone, Debug)]
pub struct Evaluation {
    pub correctness: f64,     // 0.0-1.0
    pub completeness: f64,    // 0.0-1.0
    pub consistency: f64,     // 0.0-1.0
    pub confidence: Confidence,
    pub suggestions: Vec<String>,
}

impl Evaluation {
    pub fn new(correctness: f64, completeness: f64, consistency: f64) -> Self {
        let c = correctness.clamp(0.0, 1.0);
        let cp = completeness.clamp(0.0, 1.0);
        let cs = consistency.clamp(0.0, 1.0);
        let avg = (c + cp + cs) / 3.0;
        let uncertainty = ((c - cp).abs() + (c - cs).abs() + (cp - cs).abs()) / 3.0 / 2.0;
        let suggestions = if avg < 0.6 {
            vec!["建议补充更多证据".to_string(), "建议重新验证推理步骤".to_string()]
        } else if avg < 0.8 {
            vec!["可进一步优化表述".to_string()]
        } else {
            Vec::new()
        };
        Self {
            correctness: c,
            completeness: cp,
            consistency: cs,
            confidence: Confidence::new(avg, uncertainty),
            suggestions,
        }
    }

    /// 综合得分。
    pub fn score(&self) -> f64 {
        (self.correctness + self.completeness + self.consistency) / 3.0
    }

    /// 是否通过（综合得分 >= 阈值）。
    pub fn passes(&self, threshold: f64) -> bool {
        self.score() >= threshold.clamp(0.0, 1.0)
    }
}

// ============================================================================
// 经验记录
// ============================================================================

/// 一条经验：思考内容、反馈、修正后结果、是否成功。
#[derive(Clone, Debug)]
pub struct Experience {
    pub thought: String,
    pub feedback: String,
    pub corrected: String,
    pub success: bool,
    pub timestamp: u64,
    pub confidence_at_time: Confidence,
}

impl Experience {
    pub fn new(thought: &str, feedback: &str, corrected: &str, success: bool, confidence: Confidence) -> Self {
        Self {
            thought: thought.to_string(),
            feedback: feedback.to_string(),
            corrected: corrected.to_string(),
            success,
            timestamp: crate::monitor::get_time_ns(),
            confidence_at_time: confidence,
        }
    }
}

// ============================================================================
// 元认知
// ============================================================================

/// 元认知：监控、评估、修正、元学习。
pub struct Metacognition {
    /// 累计监控次数。
    monitor_count: u64,
    /// 累计评估次数。
    evaluate_count: u64,
    /// 累计修正次数。
    self_correct_count: u64,
    /// 经验库。
    experiences: Vec<Experience>,
    /// 置信度校准表：领域 -> (预测置信度, 实际准确率)。
    calibration: BTreeMap<String, (f64, f64, usize)>,
    /// 自我修正阈值。
    pub correction_threshold: f64,
}

impl Default for Metacognition {
    fn default() -> Self {
        Self::new()
    }
}

impl Metacognition {
    pub fn new() -> Self {
        Self {
            monitor_count: 0,
            evaluate_count: 0,
            self_correct_count: 0,
            experiences: Vec::new(),
            calibration: BTreeMap::new(),
            correction_threshold: 0.6,
        }
    }

    /// 监控一段思考，返回置信度。
    pub fn monitor(&mut self, thought: &str) -> Confidence {
        self.monitor_count += 1;
        let len = thought.chars().count();
        let keyword_bonus = if thought.contains("因为") || thought.contains("因此") || thought.contains("所以") {
            0.15
        } else {
            0.0
        };
        let base = (len as f64 / 100.0).clamp(0.1, 0.7) + keyword_bonus;
        let value = base.clamp(0.0, 1.0);
        let uncertainty = (1.0 - value) * 0.5;
        let conf = Confidence::new(value, uncertainty);
        conf
    }

    /// 监控一个行动，返回置信度。
    pub fn monitor_action(&mut self, action: &str) -> Confidence {
        self.monitor_count += 1;
        let len = action.chars().count();
        let base = (len as f64 / 80.0).clamp(0.1, 0.8);
        let value = base.clamp(0.0, 1.0);
        Confidence::new(value, (1.0 - value) * 0.4)
    }

    /// 评估结果，返回 Evaluation。
    pub fn evaluate(&mut self, result: &str) -> Evaluation {
        self.evaluate_count += 1;
        let len = result.chars().count();
        let has_conclusion = result.contains("结论") || result.contains("=>") || result.contains("->");
        let has_evidence = result.contains("证据") || result.contains("因为") || result.contains("数据");
        let correctness = if has_conclusion { 0.8 } else { 0.4 } + (len as f64 / 200.0).clamp(0.0, 0.3);
        let completeness = if has_evidence { 0.8 } else { 0.5 } + (len as f64 / 300.0).clamp(0.0, 0.3);
        let consistency = 0.7 + (len as f64 / 400.0).clamp(0.0, 0.2);
        Evaluation::new(correctness.clamp(0.0, 1.0), completeness.clamp(0.0, 1.0), consistency.clamp(0.0, 1.0))
    }

    /// 自我修正：根据反馈修正思考内容，返回修正后文本。
    pub fn self_correct(&mut self, thought: &str, feedback: &str) -> String {
        self.self_correct_count += 1;
        let mut corrected = thought.to_string();
        // 简化修正：把反馈中的关键建议合并。
        if feedback.contains("错误") || feedback.contains("wrong") {
            corrected.push_str("\n[自我修正] 检测到错误，已重新推理：");
            corrected.push_str(feedback);
        }
        if feedback.contains("不完整") || feedback.contains("incomplete") {
            corrected.push_str("\n[自我修正] 补充遗漏信息：");
            corrected.push_str(feedback);
        }
        if corrected == thought {
            corrected.push_str("\n[自我修正] 未发现明显问题，保持原样。");
        }
        corrected
    }

    /// 从经验中学习：记录经验并更新置信度校准。
    pub fn learn_from_experience(&mut self, experience: Experience) {
        let key = if experience.thought.len() > 20 {
            experience.thought[..20].to_string()
        } else {
            experience.thought.clone()
        };
        let entry = self.calibration.entry(key).or_insert((0.0, 0.0, 0));
        let (pred_sum, acc_sum, count) = entry;
        *pred_sum += experience.confidence_at_time.value;
        *acc_sum += if experience.success { 1.0 } else { 0.0 };
        *count += 1;
        self.experiences.push(experience);
    }

    /// 返回某领域的置信度校准：(平均预测置信度, 平均准确率, 样本数)。
    pub fn calibration_for(&self, domain: &str) -> Option<(f64, f64, usize)> {
        self.calibration.get(domain).copied()
    }

    /// 返回全部经验数量。
    pub fn experience_count(&self) -> usize {
        self.experiences.len()
    }

    /// 返回成功经验数量。
    pub fn success_count(&self) -> usize {
        self.experiences.iter().filter(|e| e.success).count()
    }

    /// 返回累计监控次数。
    pub fn monitor_count(&self) -> u64 {
        self.monitor_count
    }

    /// 返回累计评估次数。
    pub fn evaluate_count(&self) -> u64 {
        self.evaluate_count
    }

    /// 返回累计修正次数。
    pub fn self_correct_count(&self) -> u64 {
        self.self_correct_count
    }

    /// 设置修正阈值。
    pub fn set_correction_threshold(&mut self, threshold: f64) {
        self.correction_threshold = threshold.clamp(0.0, 1.0);
    }

    /// 判断是否需要基于置信度做自我修正。
    pub fn needs_correction(&self, conf: &Confidence) -> bool {
        conf.value < self.correction_threshold || conf.needs_review()
    }
}

// ============================================================================
// 全局 Metacognition 单例（供 kernel/sense.rs 调用）
// ============================================================================

static METACOGNITION: OnceLock<Mutex<Metacognition>> = OnceLock::new();

/// 获取全局元认知。
pub fn metacognition() -> &'static Mutex<Metacognition> {
    METACOGNITION.get_or_init(|| Mutex::new(Metacognition::new()))
}

/// `sense ai://metacognition`：返回元认知状态摘要。
pub fn metacognition_sense() -> String {
    let m = metacognition().lock().unwrap();
    format!(
        "monitor_count={} evaluate_count={} self_correct_count={} experiences={} threshold={:.2}",
        m.monitor_count(),
        m.evaluate_count(),
        m.self_correct_count(),
        m.experience_count(),
        m.correction_threshold
    )
}

/// `act ai://metacognition <op> ...`：对元认知执行行动。
pub fn metacognition_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let m = metacognition();
    let mut m = m.lock().unwrap();
    match it.next().unwrap_or("") {
        "monitor" => {
            let text = it.next().ok_or("缺少文本")?;
            let conf = m.monitor(text);
            Ok(format!(
                "思考置信度={:.2} 不确定度={:.2} 区间=[{:.2},{:.2}] 标签={}",
                conf.value, conf.uncertainty, conf.lower, conf.upper, conf.label()
            ))
        }
        "monitor_action" => {
            let text = it.next().ok_or("缺少动作")?;
            let conf = m.monitor_action(text);
            Ok(format!(
                "动作置信度={:.2} 不确定度={:.2}",
                conf.value, conf.uncertainty
            ))
        }
        "evaluate" => {
            let text = it.next().ok_or("缺少结果")?;
            let ev = m.evaluate(text);
            Ok(format!(
                "评估: 正确性={:.2} 完整性={:.2} 一致性={:.2} 得分={:.2} 置信={:.2} 建议={}",
                ev.correctness,
                ev.completeness,
                ev.consistency,
                ev.score(),
                ev.confidence.value,
                ev.suggestions.len()
            ))
        }
        "self_correct" => {
            let thought = it.next().ok_or("缺少思考")?;
            let feedback = it.next().ok_or("缺少反馈")?;
            let corrected = m.self_correct(thought, feedback);
            Ok(format!("修正后：{}", corrected))
        }
        "learn" => {
            let thought = it.next().ok_or("缺少思考")?;
            let feedback = it.next().ok_or("缺少反馈")?;
            let corrected = it.next().unwrap_or("");
            let success: bool = it.next().unwrap_or("1") == "1";
            let conf = m.monitor(thought);
            let exp = Experience::new(thought, feedback, corrected, success, conf);
            m.learn_from_experience(exp);
            Ok(format!("已记录经验（成功={}）", success))
        }
        "calibration" => {
            let domain = it.next().ok_or("缺少领域")?;
            match m.calibration_for(domain) {
                Some((pred, acc, n)) => Ok(format!(
                    "领域 {}: 预测置信度={:.2} 准确率={:.2} 样本={}",
                    domain, pred, acc, n
                )),
                None => Ok(format!("领域 {} 暂无校准数据", domain)),
            }
        }
        "threshold" => {
            let t: f64 = it.next().ok_or("缺少阈值")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            m.set_correction_threshold(t);
            Ok(format!("修正阈值 -> {:.2}", t))
        }
        "needs_correction" => {
            let v: f64 = it.next().ok_or("缺少置信度")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let u: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.2);
            let conf = Confidence::new(v, u);
            Ok(format!("需要修正={}", m.needs_correction(&conf)))
        }
        "list" => Ok(metacognition_sense()),
        _ => Err(
            "用法: metacognition monitor|monitor_action|evaluate|self_correct|learn|calibration|threshold|needs_correction|list"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_new_and_label() {
        let c = Confidence::new(0.9, 0.05);
        assert!((c.value - 0.9).abs() < f64::EPSILON);
        assert_eq!(c.label(), "很高");
        assert!(c.is_reliable());
        assert!(!c.needs_review());
        let c2 = Confidence::new(0.3, 0.6);
        assert_eq!(c2.label(), "很低");
        assert!(c2.needs_review());
    }

    #[test]
    fn confidence_clamp() {
        let c = Confidence::new(1.5, -0.5);
        assert!((c.value - 1.0).abs() < f64::EPSILON);
        assert!((c.uncertainty - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluation_score() {
        let ev = Evaluation::new(0.9, 0.8, 0.7);
        assert!((ev.score() - 0.8).abs() < 1e-6);
        assert!(ev.passes(0.5));
        assert!(!ev.passes(0.9));
        assert!(!ev.suggestions.is_empty());
    }

    #[test]
    fn evaluation_high_quality() {
        let ev = Evaluation::new(0.95, 0.95, 0.95);
        assert!(ev.suggestions.is_empty());
        assert!(ev.passes(0.9));
    }

    #[test]
    fn monitor_thought() {
        let mut m = Metacognition::new();
        let conf = m.monitor("因为观察到 A 所以推断 B");
        assert!(conf.value > 0.5);
        assert_eq!(m.monitor_count(), 1);
    }

    #[test]
    fn monitor_action() {
        let mut m = Metacognition::new();
        let conf = m.monitor_action("打开文件");
        assert!(conf.value > 0.0);
    }

    #[test]
    fn evaluate_result() {
        let mut m = Metacognition::new();
        let ev = m.evaluate("结论：X 为 Y，因为有证据 Z");
        assert!(ev.correctness > 0.5);
        assert_eq!(m.evaluate_count(), 1);
    }

    #[test]
    fn self_correct() {
        let mut m = Metacognition::new();
        let fixed = m.self_correct("原来的思考", "这里有错误");
        assert!(fixed.contains("自我修正"));
        assert_eq!(m.self_correct_count(), 1);
    }

    #[test]
    fn learn_from_experience() {
        let mut m = Metacognition::new();
        let conf = Confidence::new(0.8, 0.1);
        let exp = Experience::new("测试思考", "反馈", "修正后", true, conf);
        m.learn_from_experience(exp);
        assert_eq!(m.experience_count(), 1);
        assert_eq!(m.success_count(), 1);
        let cal = m.calibration_for("测试思考");
        assert!(cal.is_some());
        assert!((cal.unwrap().0 - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn needs_correction() {
        let mut m = Metacognition::new();
        m.set_correction_threshold(0.5);
        let low = Confidence::new(0.2, 0.1);
        assert!(m.needs_correction(&low));
        let high = Confidence::new(0.9, 0.05);
        assert!(!m.needs_correction(&high));
    }

    #[test]
    fn metacognition_sense_and_act() {
        let mut m = metacognition().lock().unwrap();
        m.monitor("测试思考");
        drop(m);
        let sense = metacognition_sense();
        assert!(sense.contains("monitor_count="));
        let res = metacognition_act("monitor 思考一下").unwrap();
        assert!(res.contains("思考置信度"));
        let res = metacognition_act("evaluate 结果不错").unwrap();
        assert!(res.contains("评估:"));
        let res = metacognition_act("self_correct 旧思考 这里有错误").unwrap();
        assert!(res.contains("修正后"));
        let res = metacognition_act("learn 思考 反馈 修正 1").unwrap();
        assert!(res.contains("已记录经验"));
        let res = metacognition_act("list").unwrap();
        assert!(sense.contains("evaluate_count="));
    }
}
