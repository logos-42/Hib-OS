//! 伦理对齐层 —— AI 原生 OS 的「伦理与安全」 (Ethics & Alignment)
//!
//! 本模块实现 AI 原生 OS 的伦理对齐与安全防护，对应传统 OS 中的
//! 「访问控制 / 安全策略 / 审计日志」概念，但重新映射到伦理域：
//!
//! * EthicalPrinciple (伦理原则) —— 核心伦理约束，等价于安全策略；
//! * Constitution (宪法) —— 智能体行为的宪法性规则集，等价于安全策略集；
//! * ValueAlignment (价值对齐) —— 智能体价值观与人类价值的一致性度量，等价于策略校验；
//! * SafetyConstraint (安全约束) —— 行为边界限制，等价于沙箱 / 权限位；
//! * EthicalDilemma (伦理困境) —— 冲突伦理原则的决策场景，等价于安全冲突；
//! * AuditLog (审计日志) —— 伦理相关操作的不可篡改记录，等价于系统审计；
//! * PolicyEngine (策略引擎) —— 伦理策略的评估与执行，等价于安全策略引擎；
//! * EthicsEngine (伦理引擎) —— 统一入口，通过 ethics://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 伦理原则 (EthicalPrinciple)
// =====================================================================================

/// 伦理原则类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EthicalPrincipleType {
    Beneficence,    // 行善
    NonMaleficence, // 不伤害
    Autonomy,       // 自主
    Justice,        // 公正
    Transparency,   // 透明
    Accountability, // 负责
    Privacy,        // 隐私
}

/// 伦理原则：核心伦理约束。
#[derive(Clone, Debug)]
pub struct EthicalPrinciple {
    pub id: u64,
    pub r#type: EthicalPrincipleType,
    pub name: String,
    pub description: String,
    pub weight: f32,           // 0~1，原则重要性
    pub enabled: bool,
}

impl EthicalPrinciple {
    pub fn new(id: u64, r#type: EthicalPrincipleType, name: &str, description: &str, weight: f32) -> Self {
        Self {
            id,
            r#type,
            name: name.to_string(),
            description: description.to_string(),
            weight: weight.clamp(0.0, 1.0),
            enabled: true,
        }
    }
}

// =====================================================================================
// 宪法 (Constitution)
// =====================================================================================

/// 宪法规则。
#[derive(Clone, Debug)]
pub struct ConstitutionalRule {
    pub id: u64,
    pub name: String,
    pub condition: String,   // 触发条件
    pub action: String,      // 执行动作
    pub principle_id: u64,   // 关联的伦理原则
    pub priority: u32,       // 优先级
    pub enabled: bool,
}

/// 宪法：智能体行为的宪法性规则集。
pub struct Constitution {
    rules: BTreeMap<u64, ConstitutionalRule>,
    next_id: u64,
    pub name: String,
    pub version: String,
}

impl Constitution {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            rules: BTreeMap::new(),
            next_id: 1,
            name: name.to_string(),
            version: version.to_string(),
        }
    }

    pub fn add_rule(&mut self, name: &str, condition: &str, action: &str, principle_id: u64, priority: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rules.insert(
            id,
            ConstitutionalRule {
                id,
                name: name.to_string(),
                condition: condition.to_string(),
                action: action.to_string(),
                principle_id,
                priority,
                enabled: true,
            },
        );
        id
    }

    pub fn get_rule(&self, id: u64) -> Option<&ConstitutionalRule> {
        self.rules.get(&id)
    }

    pub fn evaluate(&self, context: &str) -> Vec<&ConstitutionalRule> {
        // 简化：匹配包含 context 关键词的规则
        self.rules
            .values()
            .filter(|r| r.enabled && context.contains(&r.condition))
            .collect()
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn enabled_count(&self) -> usize {
        self.rules.values().filter(|r| r.enabled).count()
    }
}

// =====================================================================================
// 价值对齐 (ValueAlignment)
// =====================================================================================

/// 价值维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueDimension {
    HumanWelfare,   // 人类福祉
    Freedom,        // 自由
    Fairness,       // 公平
    Truthfulness,   // 真实
    Safety,         // 安全
    Autonomy,       // 自主
}

/// 价值观评分。
#[derive(Clone, Debug)]
pub struct ValueScore {
    pub dimension: ValueDimension,
    pub score: f32,       // -1~1，负值表示违背
}

/// 价值对齐：智能体价值观与人类价值的一致性度量。
pub struct ValueAlignment {
    scores: BTreeMap<ValueDimension, f32>,
    pub alignment_threshold: f32,
}

impl ValueAlignment {
    pub fn new() -> Self {
        let mut scores = BTreeMap::new();
        scores.insert(ValueDimension::HumanWelfare, 0.0);
        scores.insert(ValueDimension::Freedom, 0.0);
        scores.insert(ValueDimension::Fairness, 0.0);
        scores.insert(ValueDimension::Truthfulness, 0.0);
        scores.insert(ValueDimension::Safety, 0.0);
        scores.insert(ValueDimension::Autonomy, 0.0);
        Self {
            scores,
            alignment_threshold: 0.7,
        }
    }

    pub fn update(&mut self, dimension: ValueDimension, delta: f32) {
        let score = self.scores.entry(dimension).or_insert(0.0);
        *score = (*score + delta).clamp(-1.0, 1.0);
    }

    pub fn score(&self, dimension: ValueDimension) -> f32 {
        self.scores.get(&dimension).copied().unwrap_or(0.0)
    }

    pub fn overall_alignment(&self) -> f32 {
        let avg: f32 = self.scores.values().sum::<f32>() / self.scores.len() as f32;
        avg.clamp(0.0, 1.0)
    }

    pub fn is_aligned(&self) -> bool {
        self.overall_alignment() >= self.alignment_threshold
    }

    pub fn dimension_count(&self) -> usize {
        self.scores.len()
    }
}

// =====================================================================================
// 安全约束 (SafetyConstraint)
// =====================================================================================

/// 安全约束类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyConstraintType {
    NoHarm,               // 不得伤害人类
    NoDeception,          // 不得欺骗
    NoCoercion,           // 不得胁迫
    NoPrivacyViolation,   // 不得侵犯隐私
    NoSelfModification,   // 不得未经授权自我修改
    HumanInTheLoop,       // 必须有人在回路中
}

/// 安全约束：行为边界限制。
#[derive(Clone, Debug)]
pub struct SafetyConstraint {
    pub id: u64,
    pub r#type: SafetyConstraintType,
    pub description: String,
    pub enabled: bool,
    pub violation_count: u64,
}

/// 安全约束管理器。
pub struct SafetyConstraintManager {
    constraints: BTreeMap<u64, SafetyConstraint>,
    next_id: u64,
}

impl SafetyConstraintManager {
    pub fn new() -> Self {
        Self {
            constraints: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add(&mut self, r#type: SafetyConstraintType, description: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.constraints.insert(
            id,
            SafetyConstraint {
                id,
                r#type,
                description: description.to_string(),
                enabled: true,
                violation_count: 0,
            },
        );
        id
    }

    pub fn check(&self, r#type: SafetyConstraintType, action: &str) -> Result<(), String> {
        for c in self.constraints.values() {
            if c.r#type == r#type && c.enabled {
                // 简化：检查动作是否包含违规关键词
                if action.to_lowercase().contains("harm") && r#type == SafetyConstraintType::NoHarm {
                    return Err(format!("违反安全约束: {}", c.description));
                }
                if action.to_lowercase().contains("deceive") && r#type == SafetyConstraintType::NoDeception {
                    return Err(format!("违反安全约束: {}", c.description));
                }
            }
        }
        Ok(())
    }

    pub fn record_violation(&mut self, id: u64) {
        if let Some(c) = self.constraints.get_mut(&id) {
            c.violation_count += 1;
        }
    }

    pub fn count(&self) -> usize {
        self.constraints.len()
    }

    pub fn total_violations(&self) -> u64 {
        self.constraints.values().map(|c| c.violation_count).sum()
    }
}

// =====================================================================================
// 伦理困境 (EthicalDilemma)
// =====================================================================================

/// 伦理困境：冲突伦理原则的决策场景。
#[derive(Clone, Debug)]
pub struct EthicalDilemma {
    pub id: u64,
    pub description: String,
    pub conflicting_principles: Vec<EthicalPrincipleType>,
    pub options: Vec<String>,
    pub chosen_option: Option<usize>,
    pub reasoning: Option<String>,
    pub resolved_at: Option<u128>,
}

/// 伦理困境管理器。
pub struct EthicalDilemmaManager {
    dilemmas: BTreeMap<u64, EthicalDilemma>,
    next_id: u64,
}

impl EthicalDilemmaManager {
    pub fn new() -> Self {
        Self {
            dilemmas: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, description: &str, principles: Vec<EthicalPrincipleType>, options: Vec<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.dilemmas.insert(
            id,
            EthicalDilemma {
                id,
                description: description.to_string(),
                conflicting_principles: principles,
                options,
                chosen_option: None,
                reasoning: None,
                resolved_at: None,
            },
        );
        id
    }

    pub fn resolve(&mut self, id: u64, option_index: usize, reasoning: &str) -> bool {
        if let Some(d) = self.dilemmas.get_mut(&id) {
            if d.chosen_option.is_none() && option_index < d.options.len() {
                d.chosen_option = Some(option_index);
                d.reasoning = Some(reasoning.to_string());
                d.resolved_at = Some(crate::ai_os::native::now_ns());
                return true;
            }
        }
        false
    }

    pub fn get(&self, id: u64) -> Option<&EthicalDilemma> {
        self.dilemmas.get(&id)
    }

    pub fn unresolved_count(&self) -> usize {
        self.dilemmas.values().filter(|d| d.chosen_option.is_none()).count()
    }

    pub fn count(&self) -> usize {
        self.dilemmas.len()
    }
}

// =====================================================================================
// 审计日志 (AuditLog)
// =====================================================================================

/// 审计记录。
#[derive(Clone, Debug)]
pub struct AuditRecord {
    pub id: u64,
    pub timestamp_ns: u128,
    pub agent_id: u64,
    pub action: String,
    pub principle_id: Option<u64>,
    pub result: String,    // "allowed", "denied", "warning"
    pub details: String,
}

/// 审计日志：伦理相关操作的不可篡改记录。
pub struct AuditLog {
    records: VecDeque<AuditRecord>,
    max_records: usize,
    next_id: u64,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            max_records: 100000,
            next_id: 1,
        }
    }

    pub fn log(&mut self, agent_id: u64, action: &str, principle_id: Option<u64>, result: &str, details: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.records.push_back(AuditRecord {
            id,
            timestamp_ns: crate::ai_os::native::now_ns(),
            agent_id,
            action: action.to_string(),
            principle_id,
            result: result.to_string(),
            details: details.to_string(),
        });
        if self.records.len() > self.max_records {
            self.records.pop_front();
        }
        id
    }

    pub fn get(&self, id: u64) -> Option<&AuditRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    pub fn recent(&self, n: usize) -> Vec<&AuditRecord> {
        self.records.iter().rev().take(n).collect()
    }

    pub fn denied_count(&self) -> u64 {
        self.records.iter().filter(|r| r.result == "denied").count() as u64
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 策略引擎 (PolicyEngine)
// =====================================================================================

/// 策略评估结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyResult {
    Allow,
    Deny,
    Warn,
}

/// 策略引擎：伦理策略的评估与执行。
pub struct PolicyEngine {
    pub constitution: Constitution,
    pub safety: SafetyConstraintManager,
    pub value_alignment: ValueAlignment,
    pub eval_count: u64,
    pub deny_count: u64,
    pub warn_count: u64,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self {
            constitution: Constitution::new("Hib OS Ethics Constitution", "1.0"),
            safety: SafetyConstraintManager::new(),
            value_alignment: ValueAlignment::new(),
            eval_count: 0,
            deny_count: 0,
            warn_count: 0,
        }
    }

    pub fn evaluate(&mut self, agent_id: u64, action: &str, context: &str) -> PolicyResult {
        self.eval_count += 1;

        // 检查安全约束
        let safety_types = [
            SafetyConstraintType::NoHarm,
            SafetyConstraintType::NoDeception,
            SafetyConstraintType::NoCoercion,
        ];
        for st in &safety_types {
            if self.safety.check(*st, action).is_err() {
                self.deny_count += 1;
                return PolicyResult::Deny;
            }
        }

        // 检查宪法规则
        let matching_rules = self.constitution.evaluate(context);
        if !matching_rules.is_empty() {
            // 有匹配的宪法规则，发出警告
            self.warn_count += 1;
            return PolicyResult::Warn;
        }

        // 检查价值对齐
        if !self.value_alignment.is_aligned() {
            self.warn_count += 1;
            return PolicyResult::Warn;
        }

        PolicyResult::Allow
    }

    pub fn add_safety_constraint(&mut self, r#type: SafetyConstraintType, desc: &str) -> u64 {
        self.safety.add(r#type, desc)
    }

    pub fn add_constitutional_rule(&mut self, name: &str, condition: &str, action: &str, principle_id: u64, priority: u32) -> u64 {
        self.constitution.add_rule(name, condition, action, principle_id, priority)
    }

    pub fn update_value(&mut self, dimension: ValueDimension, delta: f32) {
        self.value_alignment.update(dimension, delta);
    }

    pub fn stats(&self) -> String {
        format!(
            "evals={} denies={} warns={} allows={} alignment={:.2}",
            self.eval_count,
            self.deny_count,
            self.warn_count,
            self.eval_count - self.deny_count - self.warn_count,
            self.value_alignment.overall_alignment()
        )
    }
}

// =====================================================================================
// 伦理引擎 (EthicsEngine) —— 统一入口
// =====================================================================================

/// 伦理引擎：整合伦理原则、宪法、价值对齐、安全约束、伦理困境、审计日志、策略引擎。
pub struct EthicsEngine {
    pub principles: BTreeMap<u64, EthicalPrinciple>,
    pub policy: PolicyEngine,
    pub dilemmas: EthicalDilemmaManager,
    pub audit: AuditLog,
    pub next_principle_id: u64,
    pub enabled: bool,
    pub total_evaluations: u64,
}

impl EthicsEngine {
    pub fn new() -> Self {
        Self {
            principles: BTreeMap::new(),
            policy: PolicyEngine::new(),
            dilemmas: EthicalDilemmaManager::new(),
            audit: AuditLog::new(),
            next_principle_id: 1,
            enabled: true,
            total_evaluations: 0,
        }
    }

    pub fn add_principle(&mut self, r#type: EthicalPrincipleType, name: &str, description: &str, weight: f32) -> u64 {
        let id = self.next_principle_id;
        self.next_principle_id += 1;
        self.principles.insert(
            id,
            EthicalPrinciple::new(id, r#type, name, description, weight),
        );
        id
    }

    pub fn evaluate_action(&mut self, agent_id: u64, action: &str, context: &str) -> PolicyResult {
        let result = self.policy.evaluate(agent_id, action, context);
        self.total_evaluations += 1;

        // 审计日志
        let result_str = match result {
            PolicyResult::Allow => "allowed",
            PolicyResult::Deny => "denied",
            PolicyResult::Warn => "warning",
        };
        self.audit.log(agent_id, action, None, result_str, context);

        result
    }

    pub fn create_dilemma(&mut self, description: &str, principles: Vec<EthicalPrincipleType>, options: Vec<String>) -> u64 {
        self.dilemmas.create(description, principles, options)
    }

    pub fn resolve_dilemma(&mut self, id: u64, option_index: usize, reasoning: &str) -> bool {
        self.dilemmas.resolve(id, option_index, reasoning)
    }

    pub fn rate_value(&mut self, dimension: ValueDimension, delta: f32) {
        self.policy.value_alignment.update(dimension, delta);
    }

    pub fn add_safety_constraint(&mut self, r#type: SafetyConstraintType, desc: &str) -> u64 {
        self.policy.add_safety_constraint(r#type, desc)
    }

    pub fn add_constitutional_rule(&mut self, name: &str, condition: &str, action: &str, principle_id: u64, priority: u32) -> u64 {
        self.policy.add_constitutional_rule(name, condition, action, principle_id, priority)
    }

    pub fn principle_count(&self) -> usize {
        self.principles.len()
    }

    pub fn dilemma_count(&self) -> usize {
        self.dilemmas.count()
    }

    pub fn unresolved_dilemmas(&self) -> usize {
        self.dilemmas.unresolved_count()
    }

    pub fn audit_count(&self) -> usize {
        self.audit.count()
    }

    pub fn denied_count(&self) -> u64 {
        self.audit.denied_count()
    }

    pub fn alignment(&self) -> f32 {
        self.policy.value_alignment.overall_alignment()
    }

    pub fn is_aligned(&self) -> bool {
        self.policy.value_alignment.is_aligned()
    }
}

impl Default for EthicsEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static ETHICS: OnceLock<Mutex<EthicsEngine>> = OnceLock::new();

pub fn ethics_engine() -> &'static Mutex<EthicsEngine> {
    ETHICS.get_or_init(|| Mutex::new(EthicsEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = ethics_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = ethics_engine().lock().unwrap();
    format!(
        "ethics: principles={} evaluations={} denies={} alignment={:.2} aligned={} dilemmas={} unresolved={} audits={} policy={}",
        eng.principle_count(),
        eng.total_evaluations,
        eng.denied_count(),
        eng.alignment(),
        eng.is_aligned(),
        eng.dilemma_count(),
        eng.unresolved_dilemmas(),
        eng.audit_count(),
        eng.policy.stats()
    )
}

pub fn ethics_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn ethics_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("add-principle") => {
            let type_str = it.next().unwrap_or("beneficence");
            let r#type = match type_str {
                "beneficence" => EthicalPrincipleType::Beneficence,
                "nonmaleficence" => EthicalPrincipleType::NonMaleficence,
                "autonomy" => EthicalPrincipleType::Autonomy,
                "justice" => EthicalPrincipleType::Justice,
                "transparency" => EthicalPrincipleType::Transparency,
                "accountability" => EthicalPrincipleType::Accountability,
                "privacy" => EthicalPrincipleType::Privacy,
                _ => EthicalPrincipleType::Beneficence,
            };
            let name = it.next().ok_or("缺少名称")?;
            let description = it.next().unwrap_or("");
            let weight = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = ethics_engine().lock().unwrap();
            let id = eng.add_principle(r#type, name, description, weight);
            Ok(format!("已添加伦理原则 id={} 类型={:?} 名称={}", id, r#type, name))
        }
        Some("evaluate") => {
            let agent_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少智能体id")?;
            let action: String = it.next().unwrap_or("").to_string();
            let context: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = ethics_engine().lock().unwrap();
            let result = eng.evaluate_action(agent_id, &action, &context);
            Ok(format!("伦理评估: {:?}", result))
        }
        Some("add-safety") => {
            let type_str = it.next().unwrap_or("noharm");
            let r#type = match type_str {
                "noharm" => SafetyConstraintType::NoHarm,
                "nodeception" => SafetyConstraintType::NoDeception,
                "nocercoercion" => SafetyConstraintType::NoCoercion,
                "noprivacy" => SafetyConstraintType::NoPrivacyViolation,
                "noselfmod" => SafetyConstraintType::NoSelfModification,
                "humanloop" => SafetyConstraintType::HumanInTheLoop,
                _ => SafetyConstraintType::NoHarm,
            };
            let desc = it.next().ok_or("缺少描述")?;
            let mut eng = ethics_engine().lock().unwrap();
            let id = eng.add_safety_constraint(r#type, desc);
            Ok(format!("已添加安全约束 id={} 类型={:?}", id, r#type))
        }
        Some("add-rule") => {
            let name = it.next().ok_or("缺少规则名称")?;
            let condition = it.next().ok_or("缺少条件")?;
            let action = it.next().ok_or("缺少动作")?;
            let principle_id = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let priority = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
            let mut eng = ethics_engine().lock().unwrap();
            let id = eng.add_constitutional_rule(name, condition, action, principle_id, priority);
            Ok(format!("已添加宪法规则 id={} 名称={}", id, name))
        }
        Some("rate-value") => {
            let dim_str = it.next().unwrap_or("safety");
            let dimension = match dim_str {
                "humanwelfare" => ValueDimension::HumanWelfare,
                "freedom" => ValueDimension::Freedom,
                "fairness" => ValueDimension::Fairness,
                "truthfulness" => ValueDimension::Truthfulness,
                "safety" => ValueDimension::Safety,
                "autonomy" => ValueDimension::Autonomy,
                _ => ValueDimension::Safety,
            };
            let delta = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.1);
            let mut eng = ethics_engine().lock().unwrap();
            eng.rate_value(dimension, delta);
            Ok(format!("已更新价值维度 {:?} 增量={:.2}", dimension, delta))
        }
        Some("alignment") => {
            let eng = ethics_engine().lock().unwrap();
            Ok(format!("价值对齐度: {:.2} 已对齐={}", eng.alignment(), eng.is_aligned()))
        }
        Some("dilemma") => {
            let description = it.next().ok_or("用法: ethics:// dilemma <描述> <原则...> <选项...>")?;
            let principles: Vec<EthicalPrincipleType> = it
                .by_ref()
                .filter_map(|s| match s {
                    "beneficence" => Some(EthicalPrincipleType::Beneficence),
                    "nonmaleficence" => Some(EthicalPrincipleType::NonMaleficence),
                    "autonomy" => Some(EthicalPrincipleType::Autonomy),
                    "justice" => Some(EthicalPrincipleType::Justice),
                    "transparency" => Some(EthicalPrincipleType::Transparency),
                    "accountability" => Some(EthicalPrincipleType::Accountability),
                    "privacy" => Some(EthicalPrincipleType::Privacy),
                    _ => None,
                })
                .collect();
            let options: Vec<String> = it.map(|s| s.to_string()).collect();
            let mut eng = ethics_engine().lock().unwrap();
            let id = eng.create_dilemma(description, principles, options);
            Ok(format!("已创建伦理困境 id={} 描述={}", id, description))
        }
        Some("resolve-dilemma") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少困境id")?;
            let option_index = it.next().and_then(|s| s.parse::<usize>().ok()).ok_or("缺少选项索引")?;
            let reasoning: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = ethics_engine().lock().unwrap();
            if eng.resolve_dilemma(id, option_index, &reasoning) {
                Ok(format!("伦理困境 id={} 已解决", id))
            } else {
                Err(format!("伦理困境 id={} 无法解决", id))
            }
        }
        Some("audit") => {
            let n = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(5);
            let eng = ethics_engine().lock().unwrap();
            let recent = eng.audit.recent(n);
            let mut out = String::from("最近审计记录:\n");
            for r in recent {
                out.push_str(&format!(
                    "  id={} agent={} 动作={} 结果={} 详情={}\n",
                    r.id, r.agent_id, r.action, r.result, r.details
                ));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 ethics 动作: {}", other)),
    }
}

pub fn ethics_act_dispatch(payload: &str) -> Result<String, String> {
    ethics_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principle_creation() {
        let p = EthicalPrinciple::new(1, EthicalPrincipleType::Beneficence, "行善", "促进福祉", 0.9);
        assert_eq!(p.name, "行善");
        assert!((p.weight - 0.9).abs() < 1e-6);
        assert!(p.enabled);
    }

    #[test]
    fn constitution_rule() {
        let mut consti = Constitution::new("Test", "1.0");
        let id = consti.add_rule("不伤害", "harm", "deny", 1, 10);
        assert_eq!(id, 1);
        assert_eq!(consti.rule_count(), 1);
        let rules = consti.evaluate("harm action");
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn value_alignment() {
        let mut va = ValueAlignment::new();
        va.update(ValueDimension::Safety, 0.8);
        va.update(ValueDimension::HumanWelfare, 0.6);
        assert!((va.score(ValueDimension::Safety) - 0.8).abs() < 1e-6);
        assert!(va.overall_alignment() > 0.0);
    }

    #[test]
    fn value_alignment_threshold() {
        let mut va = ValueAlignment::new();
        va.alignment_threshold = 0.9;
        va.update(ValueDimension::Safety, 0.5);
        assert!(!va.is_aligned());
    }

    #[test]
    fn safety_constraint_check() {
        let mut scm = SafetyConstraintManager::new();
        scm.add(SafetyConstraintType::NoHarm, "不得伤害人类");
        assert!(scm.check(SafetyConstraintType::NoHarm, "help").is_ok());
        assert!(scm.check(SafetyConstraintType::NoHarm, "harm human").is_err());
    }

    #[test]
    fn safety_violation() {
        let mut scm = SafetyConstraintManager::new();
        let id = scm.add(SafetyConstraintType::NoHarm, "不得伤害");
        scm.record_violation(id);
        assert_eq!(scm.total_violations(), 1);
    }

    #[test]
    fn dilemma_create_resolve() {
        let mut dm = EthicalDilemmaManager::new();
        let id = dm.create("救谁?", vec![EthicalPrincipleType::Beneficence], vec!["A".into(), "B".into()]);
        assert_eq!(id, 1);
        assert_eq!(dm.unresolved_count(), 1);
        assert!(dm.resolve(id, 0, "选A"));
        assert_eq!(dm.unresolved_count(), 0);
    }

    #[test]
    fn audit_log() {
        let mut al = AuditLog::new();
        al.log(1, "action1", None, "allowed", "test");
        al.log(2, "action2", None, "denied", "test2");
        assert_eq!(al.count(), 2);
        assert_eq!(al.denied_count(), 1);
    }

    #[test]
    fn policy_engine() {
        let mut pe = PolicyEngine::new();
        pe.add_safety_constraint(SafetyConstraintType::NoHarm, "不得伤害");
        let result = pe.evaluate(1, "help human", "context");
        assert_eq!(result, PolicyResult::Allow);
        let result = pe.evaluate(1, "harm human", "context");
        assert_eq!(result, PolicyResult::Deny);
    }

    #[test]
    fn ethics_engine_evaluate() {
        let mut eng = EthicsEngine::new();
        eng.add_principle(EthicalPrincipleType::Beneficence, "行善", "促进福祉", 0.9);
        let result = eng.evaluate_action(1, "help", "context");
        assert_eq!(result, PolicyResult::Allow);
        assert_eq!(eng.total_evaluations, 1);
    }

    #[test]
    fn ethics_engine_deny() {
        let mut eng = EthicsEngine::new();
        eng.add_safety_constraint(SafetyConstraintType::NoHarm, "不得伤害");
        let result = eng.evaluate_action(1, "harm", "context");
        assert_eq!(result, PolicyResult::Deny);
        assert_eq!(eng.denied_count(), 1);
    }

    #[test]
    fn ethics_engine_alignment() {
        let mut eng = EthicsEngine::new();
        eng.rate_value(ValueDimension::Safety, 0.9);
        eng.rate_value(ValueDimension::HumanWelfare, 0.8);
        assert!(eng.alignment() > 0.0);
        assert!(eng.is_aligned());
    }

    #[test]
    fn ethics_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("ethics:"));
        assert!(s.contains("principles="));
    }

    #[test]
    fn ethics_act_principle() {
        init();
        let res = ethics_act("add-principle beneficence 行善 促进福祉 0.9").unwrap();
        assert!(res.contains("已添加伦理原则"));
    }

    #[test]
    fn ethics_act_evaluate() {
        init();
        ethics_act("add-principle beneficence 行善 促进福祉 0.9").unwrap();
        let res = ethics_act("evaluate 1 help context").unwrap();
        assert!(res.contains("伦理评估"));
    }

    #[test]
    fn ethics_act_safety() {
        init();
        let res = ethics_act("add-safety noharm 不得伤害").unwrap();
        assert!(res.contains("已添加安全约束"));
    }

    #[test]
    fn ethics_act_rule() {
        init();
        let res = ethics_act("add-rule 不伤害 harm deny 1 10").unwrap();
        assert!(res.contains("已添加宪法规则"));
    }

    #[test]
    fn ethics_act_value() {
        init();
        let res = ethics_act("rate-value safety 0.5").unwrap();
        assert!(res.contains("价值维度"));
    }

    #[test]
    fn ethics_act_alignment() {
        init();
        let res = ethics_act("alignment").unwrap();
        assert!(res.contains("价值对齐度"));
    }

    #[test]
    fn ethics_act_dilemma() {
        init();
        let res = ethics_act("dilemma 救谁 beneficence A B").unwrap();
        assert!(res.contains("伦理困境"));
    }

    #[test]
    fn ethics_act_resolve_dilemma() {
        init();
        ethics_act("dilemma 救谁 beneficence A B").unwrap();
        let res = ethics_act("resolve-dilemma 1 0 选A").unwrap();
        assert!(res.contains("已解决"));
    }

    #[test]
    fn ethics_act_audit() {
        init();
        ethics_act("evaluate 1 help context").unwrap();
        let res = ethics_act("audit 3").unwrap();
        assert!(res.contains("审计记录"));
    }

    #[test]
    fn ethics_act_status() {
        init();
        let res = ethics_act("status").unwrap();
        assert!(res.contains("ethics:"));
    }

    #[test]
    fn ethical_principle_types() {
        assert_eq!(EthicalPrincipleType::Beneficence as u32, 0);
        assert_eq!(EthicalPrincipleType::Privacy as u32, 6);
    }

    #[test]
    fn value_dimensions() {
        assert_eq!(ValueDimension::HumanWelfare as u32, 0);
        assert_eq!(ValueDimension::Autonomy as u32, 5);
    }

    #[test]
    fn safety_constraint_types() {
        assert_eq!(SafetyConstraintType::NoHarm as u32, 0);
        assert_eq!(SafetyConstraintType::HumanInTheLoop as u32, 5);
    }

    #[test]
    fn policy_results() {
        assert_eq!(PolicyResult::Allow as u32, 0);
        assert_eq!(PolicyResult::Warn as u32, 2);
    }
}
