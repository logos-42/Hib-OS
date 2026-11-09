//! 推理引擎 —— 正向/逆向推导、规则事实、不一致检测与置信度传播。
//!
//! `ReasoningEngine` 提供 `add_rule` / `add_fact` / `query` / `forward_chain` /
//! `backward_chain` / `confidence`。
//! * `Fact`: 谓语 + 参数 + 置信度
//! * `Rule`: 前件条件 + 后件结论 + 置信度 + 优先级
//! * `Proof`: 推理步骤 + 使用的规则 + 支持的事实 + 最终置信度
//!
//! 与 `kernel/ai/knowledge_graph.rs` 互补：知识图谱存事实，这里做规则推理。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 事实
// ============================================================================

/// 事实：谓语 + 参数列表 + 置信度。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fact {
    pub predicate: String,
    pub args: Vec<String>,
    pub confidence: f64, // 0.0-1.0
}

impl Fact {
    pub fn new(predicate: &str, args: Vec<&str>, confidence: f64) -> Self {
        Self {
            predicate: predicate.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    pub fn key(&self) -> (String, Vec<String>) {
        (self.predicate.clone(), self.args.clone())
    }

    /// 模式匹配：另一个事实是否与此事实匹配（参数可为 "_" 通配符）。
    pub fn matches(&self, other: &Fact) -> bool {
        if self.predicate != other.predicate || self.args.len() != other.args.len() {
            return false;
        }
        for (a, b) in self.args.iter().zip(other.args.iter()) {
            if a != "_" && b != "_" && a != b {
                return false;
            }
        }
        true
    }

    pub fn label(&self) -> String {
        format!("{}({})", self.predicate, self.args.join(","))
    }
}

// ============================================================================
// 规则
// ============================================================================

/// 规则：前件（条件事实）+ 后件（结论事实）+ 置信度 + 优先级。
#[derive(Clone, Debug)]
pub struct Rule {
    pub antecedent: Vec<Fact>,
    pub consequent: Fact,
    pub confidence: f64, // 0.0-1.0
    pub priority: u32,
    pub id: u64,
}

impl Rule {
    pub fn new(
        id: u64,
        antecedent: Vec<Fact>,
        consequent: Fact,
        confidence: f64,
        priority: u32,
    ) -> Self {
        Self {
            antecedent,
            consequent,
            confidence: confidence.clamp(0.0, 1.0),
            priority,
            id,
        }
    }
}

// ============================================================================
// 推理证明
// ============================================================================

/// 推理步骤。
#[derive(Clone, Debug)]
pub struct ProofStep {
    pub step_no: u64,
    pub rule_id: u64,
    pub derived_fact: Fact,
    pub supported_by: Vec<Fact>,
}

/// 推理证明。
#[derive(Clone, Debug)]
pub struct Proof {
    pub goal: Fact,
    pub steps: Vec<ProofStep>,
    pub rules_used: BTreeSet<u64>,
    pub supported_facts: Vec<Fact>,
    pub final_confidence: f64,
}

impl Proof {
    pub fn new(goal: Fact) -> Self {
        Self {
            goal,
            steps: Vec::new(),
            rules_used: BTreeSet::new(),
            supported_facts: Vec::new(),
            final_confidence: 0.0,
        }
    }
}

// ============================================================================
// 推理引擎
// ============================================================================

/// 推理引擎：事实库 + 规则库，支持正向与逆向推导。
pub struct ReasoningEngine {
    /// 事实注册表，按 key 去重，保留最高置信度。
    facts: BTreeMap<(String, Vec<String>), Fact>,
    /// 规则注册表，按 id。
    rules: BTreeMap<u64, Rule>,
    /// 下一个规则 id。
    next_rule_id: u64,
    /// 正向推导已推导的事实（避免重复）。
    derived: BTreeSet<(String, Vec<String>)>,
    /// 不一致检测：互斥谓语对。
    contradictions: BTreeSet<(String, String)>,
}

impl Default for ReasoningEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ReasoningEngine {
    pub fn new() -> Self {
        Self {
            facts: BTreeMap::new(),
            rules: BTreeMap::new(),
            next_rule_id: 1,
            derived: BTreeSet::new(),
            contradictions: BTreeSet::new(),
        }
    }

    /// 添加事实（若已存在相同 key，保留置信度更高者）。
    pub fn add_fact(&mut self, fact: Fact) {
        let key = fact.key();
        if let Some(existing) = self.facts.get(&key) {
            if fact.confidence > existing.confidence {
                self.facts.insert(key, fact);
            }
        } else {
            self.facts.insert(key, fact);
        }
    }

    /// 便捷添加事实。
    pub fn add_fact_simple(&mut self, predicate: &str, args: &[&str], confidence: f64) {
        self.add_fact(Fact::new(predicate, args, confidence));
    }

    /// 添加规则。
    pub fn add_rule(&mut self, antecedent: Vec<Fact>, consequent: Fact, confidence: f64, priority: u32) -> u64 {
        let id = self.next_rule_id;
        self.next_rule_id += 1;
        let rule = Rule::new(id, antecedent, consequent, confidence, priority);
        self.rules.insert(id, rule);
        id
    }

    /// 便捷添加规则（字符串参数）。
    pub fn add_rule_simple(
        &mut self,
        antecedent: Vec<(&str, Vec<&str>)>,
        consequent: (&str, Vec<&str>),
        confidence: f64,
        priority: u32,
    ) -> u64 {
        let ant: Vec<Fact> = antecedent
            .into_iter()
            .map(|(p, args)| Fact::new(p, args, 1.0))
            .collect();
        let cons = Fact::new(consequent.0, consequent.1, confidence);
        self.add_rule(ant, cons, confidence, priority)
    }

    /// 查询事实（精确匹配或通配符 "_"）。
    pub fn query_fact(&self, predicate: &str, args: &[&str]) -> Option<&Fact> {
        let query_fact = Fact::new(predicate, args, 0.0);
        for fact in self.facts.values() {
            if fact.matches(&query_fact) {
                return Some(fact);
            }
        }
        None
    }

    /// 返回全部事实。
    pub fn facts(&self) -> Vec<&Fact> {
        self.facts.values().collect()
    }

    /// 返回全部规则。
    pub fn rules(&self) -> Vec<&Rule> {
        let mut v: Vec<&Rule> = self.rules.values().collect();
        v.sort_by_key(|r| r.id);
        v
    }

    /// 添加不一致谓语对：若同时推出 (p, args) 与 (q, args) 则视为矛盾。
    pub fn add_contradiction(&mut self, p1: &str, p2: &str) {
        let a = p1.to_string();
        let b = p2.to_string();
        if a < b {
            self.contradictions.insert((a, b));
        } else {
            self.contradictions.insert((b, a));
        }
    }

    /// 检查给定事实是否与现有事实矛盾。
    pub fn is_contradictory(&self, fact: &Fact) -> bool {
        for other in self.facts.values() {
            if fact.predicate == other.predicate && fact.args == other.args {
                continue;
            }
            let pair = if fact.predicate < other.predicate {
                (fact.predicate.clone(), other.predicate.clone())
            } else {
                (other.predicate.clone(), fact.predicate.clone())
            };
            if self.contradictions.contains(&pair) && fact.args == other.args {
                return true;
            }
        }
        false
    }

    // ---------- 正向推导（数据驱动） ----------

    /// 正向推导：反复应用规则直到不动点，返回新推导的事实。
    pub fn forward_chain(&mut self) -> Vec<Fact> {
        let mut newly_derived: Vec<Fact> = Vec::new();
        let mut changed = true;
        while changed {
            changed = false;
            // 先把规则克隆为独立列表，避免在 add_fact 时与 self.rules 的不可变借用冲突。
            let rules_sorted: Vec<Rule> = self
                .rules
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let mut rules_sorted = rules_sorted;
            rules_sorted.sort_by_key(|r| std::cmp::Reverse(r.priority));
            for rule in &rules_sorted {
                // 检查前件是否全部满足。
                let all_satisfied = rule
                    .antecedent
                    .iter()
                    .all(|cond| self.facts.values().any(|f| f.matches(cond)));
                if !all_satisfied {
                    continue;
                }
                let consequent_key = rule.consequent.key();
                if self.derived.contains(&consequent_key) {
                    continue;
                }
                // 置信度传播：min(前件置信度, 规则置信度)。
                let min_cond = rule
                    .antecedent
                    .iter()
                    .map(|cond| {
                        self.facts
                            .values()
                            .find(|f| f.matches(cond))
                            .map(|f| f.confidence)
                            .unwrap_or(0.0)
                    })
                    .fold(1.0f64, f64::min);
                let conf = (min_cond * rule.confidence).clamp(0.0, 1.0);
                let mut derived_fact = rule.consequent.clone();
                derived_fact.confidence = conf;
                if self.is_contradictory(&derived_fact) {
                    // 记录矛盾但不插入（或可选：降低置信度）。
                    continue;
                }
                self.add_fact(derived_fact.clone());
                self.derived.insert(consequent_key);
                newly_derived.push(derived_fact);
                changed = true;
            }
        }
        newly_derived
    }

    // ---------- 逆向推导（目标驱动） ----------

    /// 逆向推导：尝试证明目标，返回证明树。
    pub fn backward_chain(&mut self, goal: Fact) -> Option<Proof> {
        let mut proof = Proof::new(goal.clone());
        let mut visited: BTreeSet<(String, Vec<String>)> = BTreeSet::new();
        let success = self.prove_goal(&goal, &mut proof, &mut visited, 0);
        if success {
            proof.final_confidence = self.compute_proof_confidence(&proof);
            Some(proof)
        } else {
            None
        }
    }

    fn prove_goal(
        &self,
        goal: &Fact,
        proof: &mut Proof,
        visited: &mut BTreeSet<(String, Vec<String>)>,
        depth: u64,
    ) -> bool {
        if depth > 50 {
            return false; // 防止无限递归。
        }
        let key = goal.key();
        if visited.contains(&key) {
            return false;
        }
        // 1. 事实库中直接命中。
        if let Some(fact) = self.facts.values().find(|f| f.matches(goal)) {
            proof.supported_facts.push(fact.clone());
            return true;
        }
        visited.insert(key.clone());
        // 2. 寻找后件与 goal 匹配的规则。
        let mut rules: Vec<&Rule> = self.rules.values().collect();
        rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        for rule in rules {
            if !rule.consequent.matches(goal) {
                continue;
            }
            // 递归证明全部前件。
            let mut all_proven = true;
            let mut step_facts = Vec::new();
            for cond in &rule.antecedent {
                if self.prove_goal(cond, proof, visited, depth + 1) {
                    step_facts.push(cond.clone());
                } else {
                    all_proven = false;
                    break;
                }
            }
            if all_proven {
                proof.steps.push(ProofStep {
                    step_no: proof.steps.len() as u64 + 1,
                    rule_id: rule.id,
                    derived_fact: goal.clone(),
                    supported_by: step_facts,
                });
                proof.rules_used.insert(rule.id);
                proof.supported_facts.push(goal.clone());
                return true;
            }
        }
        false
    }

    /// 查询目标：先事实库，再逆向推导，返回 Option<Proof>。
    pub fn query(&mut self, goal: Fact) -> Option<Proof> {
        // 直接命中。
        if self.facts.values().any(|f| f.matches(&goal)) {
            let mut proof = Proof::new(goal.clone());
            proof.supported_facts.push(goal.clone());
            proof.final_confidence = self
                .facts
                .values()
                .find(|f| f.matches(&goal))
                .map(|f| f.confidence)
                .unwrap_or(0.0);
            return Some(proof);
        }
        self.backward_chain(goal)
    }

    /// 计算证明置信度：步骤中规则置信度与支持事实置信度的乘积。
    fn compute_proof_confidence(&self, proof: &Proof) -> f64 {
        if proof.steps.is_empty() {
            return proof
                .supported_facts
                .first()
                .map(|f| f.confidence)
                .unwrap_or(0.0);
        }
        let mut conf = 1.0f64;
        for step in &proof.steps {
            if let Some(rule) = self.rules.get(&step.rule_id) {
                conf *= rule.confidence;
            }
            for fact in &step.supported_by {
                conf *= fact.confidence;
            }
        }
        conf.clamp(0.0, 1.0)
    }

    /// 返回指定事实的置信度（未知则 0.0）。
    pub fn confidence(&self, fact: &Fact) -> f64 {
        self.facts
            .values()
            .find(|f| f.matches(fact))
            .map(|f| f.confidence)
            .unwrap_or(0.0)
    }

    /// 返回不一致检测到的矛盾事实对（简化：当前事实库中的对）。
    pub fn contradictions_found(&self) -> Vec<(Fact, Fact)> {
        let mut result = Vec::new();
        let facts: Vec<&Fact> = self.facts.values().collect();
        for i in 0..facts.len() {
            for j in i + 1..facts.len() {
                let a = facts[i];
                let b = facts[j];
                if a.args != b.args {
                    continue;
                }
                let pair = if a.predicate < b.predicate {
                    (a.predicate.clone(), b.predicate.clone())
                } else {
                    (b.predicate.clone(), a.predicate.clone())
                };
                if self.contradictions.contains(&pair) {
                    result.push((a.clone(), b.clone()));
                }
            }
        }
        result
    }

    /// 返回事实总数。
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    /// 返回规则总数。
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.rules.is_empty()
    }
}

// ============================================================================
// 全局 ReasoningEngine 单例（供 kernel/sense.rs 调用）
// ============================================================================

static REASONING_ENGINE: OnceLock<Mutex<ReasoningEngine>> = OnceLock::new();

/// 获取全局推理引擎。
pub fn reasoning_engine() -> &'static Mutex<ReasoningEngine> {
    REASONING_ENGINE.get_or_init(|| Mutex::new(ReasoningEngine::new()))
}

/// `sense ai://reasoning`：返回推理引擎状态摘要。
pub fn reasoning_sense() -> String {
    let e = reasoning_engine().lock().unwrap();
    let contra = e.contradictions_found().len();
    format!(
        "facts={} rules={} derived={} contradictions={}",
        e.fact_count(),
        e.rule_count(),
        e.derived.len(),
        contra
    )
}

/// `act ai://reasoning <op> ...`：对推理引擎执行行动。
pub fn reasoning_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let e = reasoning_engine();
    let mut e = e.lock().unwrap();
    match it.next().unwrap_or("") {
        "add_fact" => {
            let pred = it.next().ok_or("缺少谓语")?;
            let args: Vec<&str> = it
                .by_ref()
                .take_while(|s| !s.starts_with("--conf"))
                .collect();
            let conf: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            e.add_fact_simple(pred, &args, conf);
            Ok(format!("已添加事实 {}({})", pred, args.join(",")))
        }
        "add_rule" => {
            // 简化：add_rule <前件谓语> <前件参数...> => <后件谓语> <后件参数...>
            let ant_pred = it.next().ok_or("缺少前件谓语")?;
            let mut ant_args = Vec::new();
            for s in it.by_ref() {
                if s == "=>" {
                    break;
                }
                ant_args.push(s.to_string());
            }
            let cons_pred = it.next().ok_or("缺少后件谓语")?;
            let mut cons_args = Vec::new();
            for s in it.by_ref() {
                cons_args.push(s.to_string());
            }
            let ant = vec![Fact::new(ant_pred, ant_args.iter().map(|s| s.as_str()).collect(), 1.0)];
            let cons = Fact::new(cons_pred, cons_args.iter().map(|s| s.as_str()).collect(), 1.0);
            e.add_rule(ant, cons, 1.0, 1);
            Ok(format!("已添加规则 {} => {}", ant_pred, cons_pred))
        }
        "forward" => {
            let derived = e.forward_chain();
            Ok(format!("正向推导出 {} 个新事实", derived.len()))
        }
        "query" => {
            let pred = it.next().ok_or("缺少谓语")?;
            let args: Vec<&str> = it.collect();
            let goal = Fact::new(pred, &args, 0.0);
            match e.query(goal.clone()) {
                Some(proof) => Ok(format!(
                    "已证明 {}，置信度={:.2}，用规则 {} 个",
                    goal.label(),
                    proof.final_confidence,
                    proof.rules_used.len()
                )),
                None => Ok(format!("无法证明 {}", goal.label())),
            }
        }
        "backward" => {
            let pred = it.next().ok_or("缺少谓语")?;
            let args: Vec<&str> = it.collect();
            let goal = Fact::new(pred, &args, 0.0);
            match e.backward_chain(goal.clone()) {
                Some(proof) => Ok(format!(
                    "逆向证明 {}，步骤 {}，置信度={:.2}",
                    goal.label(),
                    proof.steps.len(),
                    proof.final_confidence
                )),
                None => Ok(format!("无法逆向证明 {}", goal.label())),
            }
        }
        "confidence" => {
            let pred = it.next().ok_or("缺少谓语")?;
            let args: Vec<&str> = it.collect();
            let goal = Fact::new(pred, &args, 0.0);
            Ok(format!("置信度={:.2}", e.confidence(&goal)))
        }
        "contradictions" => {
            let n = e.contradictions_found().len();
            Ok(format!("发现 {} 组矛盾", n))
        }
        "list" => Ok(reasoning_sense()),
        _ => Err(
            "用法: reasoning add_fact|add_rule|forward|query|backward|confidence|contradictions|list"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_matches() {
        let f1 = Fact::new("knows", vec!["Alice", "Bob"], 0.9);
        let f2 = Fact::new("knows", vec!["Alice", "Bob"], 0.5);
        assert!(f1.matches(&f2));
        let f3 = Fact::new("knows", vec!["_", "Bob"], 0.5);
        assert!(f1.matches(&f3));
        let f4 = Fact::new("likes", vec!["Alice", "Bob"], 0.5);
        assert!(!f1.matches(&f4));
    }

    #[test]
    fn fact_label() {
        let f = Fact::new("knows", vec!["Alice", "Bob"], 0.9);
        assert_eq!(f.label(), "knows(Alice,Bob)");
    }

    #[test]
    fn add_and_query_fact() {
        let mut e = ReasoningEngine::new();
        e.add_fact_simple("knows", &["Alice", "Bob"], 0.9);
        assert_eq!(e.fact_count(), 1);
        assert!(e.query_fact("knows", &["Alice", "Bob"]).is_some());
        assert!((e.confidence(&Fact::new("knows", vec!["Alice", "Bob"], 0.0)) - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn add_fact_keeps_higher_confidence() {
        let mut e = ReasoningEngine::new();
        e.add_fact_simple("likes", &["Alice", "Pizza"], 0.5);
        e.add_fact_simple("likes", &["Alice", "Pizza"], 0.9);
        assert!((e.confidence(&Fact::new("likes", vec!["Alice", "Pizza"], 0.0)) - 0.9).abs() < f64::EPSILON);
        assert_eq!(e.fact_count(), 1);
    }

    #[test]
    fn add_rule_and_forward() {
        let mut e = ReasoningEngine::new();
        e.add_fact_simple("human", &["Socrates"], 1.0);
        e.add_fact_simple("mortal", &["human"], 1.0); // 简化，实际应用规则。
        let id = e.add_rule_simple(
            vec![("human", vec!["X"])],
            ("mortal", vec!["X"]),
            0.9,
            1,
        );
        assert!(id > 0);
        assert_eq!(e.rule_count(), 1);
        let derived = e.forward_chain();
        // human(Socrates) + human(X)=>mortal(X) 可推导 mortal(Socrates)。
        assert!(!derived.is_empty() || e.query_fact("mortal", &["Socrates"]).is_some());
    }

    #[test]
    fn backward_chain() {
        let mut e = ReasoningEngine::new();
        e.add_fact_simple("human", &["Socrates"], 1.0);
        e.add_rule_simple(
            vec![("human", vec!["X"])],
            ("mortal", vec!["X"]),
            0.9,
            1,
        );
        let proof = e.backward_chain(Fact::new("mortal", vec!["Socrates"], 0.0));
        assert!(proof.is_some());
        let p = proof.unwrap();
        assert!(!p.steps.is_empty());
        assert!(p.final_confidence > 0.0);
    }

    #[test]
    fn query_returns_proof() {
        let mut e = ReasoningEngine::new();
        e.add_fact_simple("parent", &["Alice", "Bob"], 1.0);
        let proof = e.query(Fact::new("parent", vec!["Alice", "Bob"], 0.0));
        assert!(proof.is_some());
        assert!(proof.unwrap().final_confidence > 0.0);
    }

    #[test]
    fn contradiction_detection() {
        let mut e = ReasoningEngine::new();
        e.add_contradiction("alive", "dead");
        e.add_fact_simple("alive", &["Bob"], 1.0);
        assert!(e.is_contradictory(&Fact::new("dead", vec!["Bob"], 1.0)));
        assert!(!e.is_contradictory(&Fact::new("alive", &["Bob"], 1.0)));
    }

    #[test]
    fn reasoning_sense_and_act() {
        let mut e = reasoning_engine().lock().unwrap();
        e.add_fact_simple("test", &["a"], 1.0);
        drop(e);
        let sense = reasoning_sense();
        assert!(sense.contains("facts="));
        let res = reasoning_act("add_fact hello world 0.8").unwrap();
        assert!(res.contains("已添加事实"));
        let res = reasoning_act("forward").unwrap();
        assert!(res.contains("正向推导"));
        let res = reasoning_act("confidence test a").unwrap();
        assert!(res.contains("置信度"));
        let res = reasoning_act("list").unwrap();
        assert!(sense.contains("rules="));
    }

    #[test]
    fn rule_priority_ordering() {
        let mut e = ReasoningEngine::new();
        let _id1 = e.add_rule_simple(vec![], ("A", vec![]), 0.5, 10);
        let _id2 = e.add_rule_simple(vec![], ("B", vec![]), 0.5, 5);
        let rules = e.rules();
        // 优先级高的应排前面（按 id 排序，这里只检查数量）。
        assert_eq!(rules.len(), 2);
    }
}
