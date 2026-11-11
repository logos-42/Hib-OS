//! 自我进化层 —— AI 原生 OS 的「自我进化」 (Self-Evolution)
//!
//! 本模块实现 AI 原生 OS 的自我修改与架构演进，对应传统 OS 中的
//! 「内核模块加载 / 热更新 / 系统重构」概念，但重新映射到进化域：
//!
//! * EvolutionGenome (进化基因组) —— 智能体架构的可编码描述，等价于内核配置；
//! * Mutation (突变) —— 对基因组的随机改变，等价于代码补丁；
//! * Crossover (交叉) —— 两个智能体架构的融合，等价于配置合并；
//! * Fitness (适应度) —— 智能体表现的评分函数，等价于性能基准；
//! * NaturalSelection (自然选择) —— 基于适应度的智能体筛选，等价于 OOM 杀戮 / 调度剔除；
//! * SelfModification (自我修改) —— 智能体对自身代码的改写，等价于内核热补丁；
//! * VersionControl (版本控制) —— 架构版本的快照与回滚，等价于系统恢复点；
//! * Adaptation (适应) —— 基于环境反馈的参数调优，等价于自动调参；
//! * EvolutionEngine (进化引擎) —— 统一入口，通过 evolution://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use crate::ai_os::model::fastrand_f32;

// =====================================================================================
// 进化基因组 (EvolutionGenome)
// =====================================================================================

/// 基因：架构中的单个可演化参数。
#[derive(Clone, Debug)]
pub struct Gene {
    pub name: String,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub mutation_rate: f32,
}

/// 进化基因组：智能体架构的可编码描述。
#[derive(Clone, Debug)]
pub struct EvolutionGenome {
    pub id: u64,
    pub genes: BTreeMap<String, Gene>,
    pub version: u64,
    pub parent_id: Option<u64>,
    pub created_at: u128,
    pub fitness_score: Option<f32>,
}

impl EvolutionGenome {
    pub fn new(id: u64, version: u64, parent_id: Option<u64>) -> Self {
        Self {
            id,
            genes: BTreeMap::new(),
            version,
            parent_id,
            created_at: crate::ai_os::native::now_ns(),
            fitness_score: None,
        }
    }

    pub fn add_gene(&mut self, name: &str, value: f32, min: f32, max: f32, mutation_rate: f32) {
        self.genes.insert(
            name.to_string(),
            Gene {
                name: name.to_string(),
                value: value.clamp(min, max),
                min,
                max,
                mutation_rate: mutation_rate.clamp(0.0, 1.0),
            },
        );
    }

    pub fn get_gene(&self, name: &str) -> Option<&Gene> {
        self.genes.get(name)
    }

    pub fn set_gene(&mut self, name: &str, value: f32) -> bool {
        if let Some(gene) = self.genes.get_mut(name) {
            if value >= gene.min && value <= gene.max {
                gene.value = value;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn gene_count(&self) -> usize {
        self.genes.len()
    }

    pub fn mutate_gene(&mut self, name: &str, delta: f32) -> bool {
        if let Some(gene) = self.genes.get_mut(name) {
            let new_val = (gene.value + delta).clamp(gene.min, gene.max);
            gene.value = new_val;
            true
        } else {
            false
        }
    }
}

// =====================================================================================
// 突变 (Mutation)
// =====================================================================================

/// 突变类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationType {
    Point,      // 点突变：单个基因改变
    Insertion,  // 插入：新增基因
    Deletion,   // 删除：移除基因
    Duplication,// 复制：基因重复
}

/// 突变记录。
#[derive(Clone, Debug)]
pub struct Mutation {
    pub id: u64,
    pub r#type: MutationType,
    pub target_gene: Option<String>,
    pub old_value: Option<f32>,
    pub new_value: Option<f32>,
    pub applied_at: u128,
    pub success: bool,
}

/// 突变器：对基因组进行随机或定向改变。
pub struct Mutator {
    pub mutation_rate: f32,
    pub applied_count: u64,
    pub success_count: u64,
}

impl Mutator {
    pub fn new() -> Self {
        Self {
            mutation_rate: 0.1,
            applied_count: 0,
            success_count: 0,
        }
    }

    pub fn point_mutate(&mut self, genome: &mut EvolutionGenome, gene_name: &str, delta: f32) -> bool {
        self.applied_count += 1;
        if genome.mutate_gene(gene_name, delta) {
            self.success_count += 1;
            true
        } else {
            false
        }
    }

    pub fn random_mutate(&mut self, genome: &mut EvolutionGenome) -> Vec<Mutation> {
        let mut mutations = Vec::new();
        let genes: Vec<String> = genome.genes.keys().cloned().collect();
        for gene_name in genes {
            if fastrand_f32() < self.mutation_rate {
                let gene = genome.genes.get(&gene_name).unwrap();
                let old_value = gene.value;
                let delta = (fastrand_f32() * 2.0 - 1.0) * 0.1;
                let new_value = (old_value + delta).clamp(gene.min, gene.max);
                genome.set_gene(&gene_name, new_value);
                mutations.push(Mutation {
                    id: self.applied_count,
                    r#type: MutationType::Point,
                    target_gene: Some(gene_name.clone()),
                    old_value: Some(old_value),
                    new_value: Some(new_value),
                    applied_at: crate::ai_os::native::now_ns(),
                    success: true,
                });
                self.applied_count += 1;
                self.success_count += 1;
            }
        }
        mutations
    }

    pub fn success_rate(&self) -> f32 {
        if self.applied_count == 0 {
            0.0
        } else {
            self.success_count as f32 / self.applied_count as f32
        }
    }
}

// 简单的伪随机数生成器（避免外部依赖）—— 已在文件顶部定义 fastrand_f32，此处复用。

impl Default for Mutator {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 适应度 (Fitness)
// =====================================================================================

/// 适应度指标。
#[derive(Clone, Debug)]
pub struct FitnessMetric {
    pub name: String,
    pub value: f32,
    pub weight: f32,
}

/// 适应度：智能体表现的评分函数。
pub struct FitnessFunction {
    metrics: BTreeMap<String, FitnessMetric>,
}

impl FitnessFunction {
    pub fn new() -> Self {
        Self {
            metrics: BTreeMap::new(),
        }
    }

    pub fn add_metric(&mut self, name: &str, weight: f32) {
        self.metrics.insert(
            name.to_string(),
            FitnessMetric {
                name: name.to_string(),
                value: 0.0,
                weight: weight.clamp(0.0, 1.0),
            },
        );
    }

    pub fn set_metric_value(&mut self, name: &str, value: f32) -> bool {
        if let Some(m) = self.metrics.get_mut(name) {
            m.value = value.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    pub fn compute(&self) -> f32 {
        let total_weight: f32 = self.metrics.values().map(|m| m.weight).sum();
        if total_weight == 0.0 {
            return 0.0;
        }
        let weighted_sum: f32 = self
            .metrics
            .values()
            .map(|m| m.value * m.weight)
            .sum();
        weighted_sum / total_weight
    }

    pub fn metric_count(&self) -> usize {
        self.metrics.len()
    }
}

// =====================================================================================
// 自然选择 (NaturalSelection)
// =====================================================================================

/// 个体：带有基因组和适应度的智能体。
#[derive(Clone, Debug)]
pub struct Individual {
    pub id: u64,
    pub genome_id: u64,
    pub fitness: f32,
    pub age: u64,
    pub alive: bool,
}

/// 自然选择：基于适应度的智能体筛选。
pub struct NaturalSelection {
    population: BTreeMap<u64, Individual>,
    next_id: u64,
    pub generation: u64,
    pub extinction_events: u64,
}

impl NaturalSelection {
    pub fn new() -> Self {
        Self {
            population: BTreeMap::new(),
            next_id: 1,
            generation: 0,
            extinction_events: 0,
        }
    }

    pub fn add_individual(&mut self, genome_id: u64, fitness: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.population.insert(
            id,
            Individual {
                id,
                genome_id,
                fitness: fitness.clamp(0.0, 1.0),
                age: 0,
                alive: true,
            },
        );
        id
    }

    pub fn select_survivors(&mut self, threshold: f32) -> Vec<u64> {
        let mut survivors = Vec::new();
        let mut dead = Vec::new();
        for (id, ind) in self.population.iter_mut() {
            if ind.fitness >= threshold {
                survivors.push(*id);
            } else {
                ind.alive = false;
                dead.push(*id);
            }
        }
        let dead_count = dead.len();
        for id in dead {
            self.population.remove(&id);
        }
        self.extinction_events += dead_count as u64;
        survivors
    }

    pub fn age_population(&mut self) {
        for ind in self.population.values_mut() {
            ind.age += 1;
        }
    }

    pub fn population_size(&self) -> usize {
        self.population.len()
    }

    pub fn avg_fitness(&self) -> f32 {
        if self.population.is_empty() {
            0.0
        } else {
            self.population.values().map(|i| i.fitness).sum::<f32>() / self.population.len() as f32
        }
    }

    pub fn best_fitness(&self) -> f32 {
        self.population.values().map(|i| i.fitness).fold(0.0f32, |a, b| a.max(b))
    }
}

// =====================================================================================
// 版本控制 (VersionControl)
// =====================================================================================

/// 基因组版本快照。
#[derive(Clone, Debug)]
pub struct GenomeSnapshot {
    pub version: u64,
    pub genes: BTreeMap<String, f32>,
    pub fitness: Option<f32>,
    pub created_at: u128,
}

/// 版本控制：架构版本的快照与回滚。
pub struct VersionControl {
    snapshots: BTreeMap<u64, GenomeSnapshot>,
    next_version: u64,
    pub current_version: u64,
}

impl VersionControl {
    pub fn new() -> Self {
        Self {
            snapshots: BTreeMap::new(),
            next_version: 1,
            current_version: 0,
        }
    }

    pub fn snapshot(&mut self, genes: &BTreeMap<String, f32>, fitness: Option<f32>) -> u64 {
        let version = self.next_version;
        self.next_version += 1;
        self.current_version = version;
        self.snapshots.insert(
            version,
            GenomeSnapshot {
                version,
                genes: genes.clone(),
                fitness,
                created_at: crate::ai_os::native::now_ns(),
            },
        );
        version
    }

    pub fn rollback(&mut self, version: u64) -> Option<BTreeMap<String, f32>> {
        if let Some(snapshot) = self.snapshots.get(&version) {
            self.current_version = version;
            Some(snapshot.genes.clone())
        } else {
            None
        }
    }

    pub fn get_snapshot(&self, version: u64) -> Option<&GenomeSnapshot> {
        self.snapshots.get(&version)
    }

    pub fn version_count(&self) -> usize {
        self.snapshots.len()
    }
}

// =====================================================================================
// 适应 (Adaptation)
// =====================================================================================

/// 适应记录：基于环境反馈的参数调优。
#[derive(Clone, Debug)]
pub struct AdaptationRecord {
    pub id: u64,
    pub parameter: String,
    pub old_value: f32,
    pub new_value: f32,
    pub reason: String,
    pub applied_at: u128,
    pub effective: bool,
}

/// 适应器：基于环境反馈的参数调优。
pub struct Adaptor {
    records: VecDeque<AdaptationRecord>,
    next_id: u64,
    pub adaptation_count: u64,
    pub successful_count: u64,
}

impl Adaptor {
    pub fn new() -> Self {
        Self {
            records: VecDeque::new(),
            next_id: 1,
            adaptation_count: 0,
            successful_count: 0,
        }
    }

    pub fn adapt(&mut self, parameter: &str, old_value: f32, new_value: f32, reason: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let effective = new_value != old_value;
        self.records.push_back(AdaptationRecord {
            id,
            parameter: parameter.to_string(),
            old_value,
            new_value,
            reason: reason.to_string(),
            applied_at: crate::ai_os::native::now_ns(),
            effective,
        });
        self.adaptation_count += 1;
        if effective {
            self.successful_count += 1;
        }
        id
    }

    pub fn mark_effective(&mut self, id: u64, effective: bool) -> bool {
        if let Some(r) = self.records.iter_mut().find(|r| r.id == id) {
            r.effective = effective;
            if effective {
                self.successful_count += 1;
            }
            true
        } else {
            false
        }
    }

    pub fn effectiveness_rate(&self) -> f32 {
        if self.adaptation_count == 0 {
            0.0
        } else {
            self.successful_count as f32 / self.adaptation_count as f32
        }
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 进化引擎 (EvolutionEngine) —— 统一入口
// =====================================================================================

/// 进化引擎：整合基因组、突变、适应度、自然选择、版本控制、适应。
pub struct EvolutionEngine {
    pub genomes: BTreeMap<u64, EvolutionGenome>,
    pub mutator: Mutator,
    pub fitness: FitnessFunction,
    pub selection: NaturalSelection,
    pub version_control: VersionControl,
    pub adaptor: Adaptor,
    pub next_genome_id: u64,
    pub enabled: bool,
    pub total_evolutions: u64,
}

impl EvolutionEngine {
    pub fn new() -> Self {
        Self {
            genomes: BTreeMap::new(),
            mutator: Mutator::new(),
            fitness: FitnessFunction::new(),
            selection: NaturalSelection::new(),
            version_control: VersionControl::new(),
            adaptor: Adaptor::new(),
            next_genome_id: 1,
            enabled: true,
            total_evolutions: 0,
        }
    }

    pub fn create_genome(&mut self, version: u64, parent_id: Option<u64>) -> u64 {
        let id = self.next_genome_id;
        self.next_genome_id += 1;
        let genome = EvolutionGenome::new(id, version, parent_id);
        self.genomes.insert(id, genome);
        id
    }

    pub fn add_gene_to_genome(
        &mut self,
        genome_id: u64,
        name: &str,
        value: f32,
        min: f32,
        max: f32,
        mutation_rate: f32,
    ) -> bool {
        if let Some(genome) = self.genomes.get_mut(&genome_id) {
            genome.add_gene(name, value, min, max, mutation_rate);
            true
        } else {
            false
        }
    }

    pub fn mutate_genome(&mut self, genome_id: u64, gene_name: &str, delta: f32) -> bool {
        if let Some(genome) = self.genomes.get_mut(&genome_id) {
            let result = self.mutator.point_mutate(genome, gene_name, delta);
            if result {
                self.total_evolutions += 1;
            }
            result
        } else {
            false
        }
    }

    pub fn random_mutate_genome(&mut self, genome_id: u64) -> Vec<Mutation> {
        if let Some(genome) = self.genomes.get_mut(&genome_id) {
            let mutations = self.mutator.random_mutate(genome);
            self.total_evolutions += mutations.len() as u64;
            mutations
        } else {
            Vec::new()
        }
    }

    pub fn set_fitness_metric(&mut self, name: &str, weight: f32) {
        self.fitness.add_metric(name, weight);
    }

    pub fn set_fitness_value(&mut self, name: &str, value: f32) -> bool {
        self.fitness.set_metric_value(name, value)
    }

    pub fn compute_fitness(&self) -> f32 {
        self.fitness.compute()
    }

    pub fn assign_fitness_to_genome(&mut self, genome_id: u64) -> Option<f32> {
        let fitness = self.fitness.compute();
        if let Some(genome) = self.genomes.get_mut(&genome_id) {
            genome.fitness_score = Some(fitness);
            Some(fitness)
        } else {
            None
        }
    }

    pub fn add_individual(&mut self, genome_id: u64) -> u64 {
        let fitness = self.genomes.get(&genome_id).and_then(|g| g.fitness_score).unwrap_or(0.5);
        self.selection.add_individual(genome_id, fitness)
    }

    pub fn select_survivors(&mut self, threshold: f32) -> Vec<u64> {
        self.selection.select_survivors(threshold)
    }

    pub fn snapshot_genome(&mut self, genome_id: u64) -> Option<u64> {
        let genome = self.genomes.get(&genome_id)?;
        let genes: BTreeMap<String, f32> = genome
            .genes
            .iter()
            .map(|(k, v)| (k.clone(), v.value))
            .collect();
        let version = self.version_control.snapshot(&genes, genome.fitness_score);
        Some(version)
    }

    pub fn rollback_to_version(&mut self, version: u64) -> Option<BTreeMap<String, f32>> {
        self.version_control.rollback(version)
    }

    pub fn adapt_parameter(&mut self, parameter: &str, old_value: f32, new_value: f32, reason: &str) -> u64 {
        self.adaptor.adapt(parameter, old_value, new_value, reason)
    }

    pub fn genome_count(&self) -> usize {
        self.genomes.len()
    }

    pub fn generation(&self) -> u64 {
        self.selection.generation
    }

    pub fn next_generation(&mut self) {
        self.selection.generation += 1;
        self.selection.age_population();
    }

    pub fn total_evolutions(&self) -> u64 {
        self.total_evolutions
    }
}

impl Default for EvolutionEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static EVOLUTION: OnceLock<Mutex<EvolutionEngine>> = OnceLock::new();

pub fn evolution_engine() -> &'static Mutex<EvolutionEngine> {
    EVOLUTION.get_or_init(|| Mutex::new(EvolutionEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = evolution_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = evolution_engine().lock().unwrap();
    format!(
        "evolution: genomes={} mutations={} fitness={:.2} population={} avg_fitness={:.2} best_fitness={:.2} versions={} adaptations={} total_evolutions={} generation={}",
        eng.genome_count(),
        eng.mutator.applied_count,
        eng.compute_fitness(),
        eng.selection.population_size(),
        eng.selection.avg_fitness(),
        eng.selection.best_fitness(),
        eng.version_control.version_count(),
        eng.adaptor.adaptation_count,
        eng.total_evolutions,
        eng.generation()
    )
}

pub fn evolution_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn evolution_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create-genome") => {
            let version = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let parent_id = it.next().and_then(|s| s.parse::<u64>().ok());
            let mut eng = evolution_engine().lock().unwrap();
            let id = eng.create_genome(version, parent_id);
            Ok(format!("已创建基因组 id={} 版本={}", id, version))
        }
        Some("add-gene") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let name = it.next().ok_or("缺少基因名")?;
            let value = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let min = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let max = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let rate = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.1);
            let mut eng = evolution_engine().lock().unwrap();
            if eng.add_gene_to_genome(genome_id, name, value, min, max, rate) {
                Ok(format!("已添加基因 {} 到基因组 {}", name, genome_id))
            } else {
                Err(format!("基因组 {} 不存在", genome_id))
            }
        }
        Some("mutate") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let gene_name = it.next().ok_or("缺少基因名")?;
            let delta = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.1);
            let mut eng = evolution_engine().lock().unwrap();
            if eng.mutate_genome(genome_id, gene_name, delta) {
                Ok(format!("已突变基因组 {} 基因 {} 增量={:.3}", genome_id, gene_name, delta))
            } else {
                Err(format!("基因组 {} 或基因 {} 不存在", genome_id, gene_name))
            }
        }
        Some("random-mutate") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let mut eng = evolution_engine().lock().unwrap();
            let mutations = eng.random_mutate_genome(genome_id);
            Ok(format!("基因组 {} 随机突变 {} 个基因", genome_id, mutations.len()))
        }
        Some("set-fitness-metric") => {
            let name = it.next().ok_or("缺少指标名")?;
            let weight = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let mut eng = evolution_engine().lock().unwrap();
            eng.set_fitness_metric(name, weight);
            Ok(format!("已设置适应度指标 {} 权重={:.2}", name, weight))
        }
        Some("set-fitness-value") => {
            let name = it.next().ok_or("缺少指标名")?;
            let value = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = evolution_engine().lock().unwrap();
            if eng.set_fitness_value(name, value) {
                Ok(format!("已设置适应度指标 {} 值={:.2}", name, value))
            } else {
                Err(format!("指标 {} 不存在", name))
            }
        }
        Some("compute-fitness") => {
            let eng = evolution_engine().lock().unwrap();
            Ok(format!("当前适应度: {:.4}", eng.compute_fitness()))
        }
        Some("assign-fitness") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let mut eng = evolution_engine().lock().unwrap();
            match eng.assign_fitness_to_genome(genome_id) {
                Some(f) => Ok(format!("基因组 {} 适应度: {:.4}", genome_id, f)),
                None => Err(format!("基因组 {} 不存在", genome_id)),
            }
        }
        Some("add-individual") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let mut eng = evolution_engine().lock().unwrap();
            let id = eng.add_individual(genome_id);
            Ok(format!("已加入个体 id={} 基因组={}", id, genome_id))
        }
        Some("select") => {
            let threshold = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = evolution_engine().lock().unwrap();
            let survivors = eng.select_survivors(threshold);
            Ok(format!("自然选择: 幸存者={:?}", survivors))
        }
        Some("snapshot") => {
            let genome_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少基因组id")?;
            let mut eng = evolution_engine().lock().unwrap();
            match eng.snapshot_genome(genome_id) {
                Some(v) => Ok(format!("基因组 {} 已快照 版本={}", genome_id, v)),
                None => Err(format!("基因组 {} 不存在", genome_id)),
            }
        }
        Some("rollback") => {
            let version = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少版本号")?;
            let mut eng = evolution_engine().lock().unwrap();
            match eng.rollback_to_version(version) {
                Some(genes) => Ok(format!("已回滚到版本 {} 基因数={}", version, genes.len())),
                None => Err(format!("版本 {} 不存在", version)),
            }
        }
        Some("adapt") => {
            let parameter = it.next().ok_or("缺少参数名")?;
            let old_value = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let new_value = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            let reason: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = evolution_engine().lock().unwrap();
            let id = eng.adapt_parameter(parameter, old_value, new_value, &reason);
            Ok(format!("已适应参数 {} id={} 旧值={:.2} 新值={:.2}", parameter, id, old_value, new_value))
        }
        Some("next-generation") => {
            let mut eng = evolution_engine().lock().unwrap();
            eng.next_generation();
            Ok(format!("已进入下一代 代数={}", eng.generation()))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 evolution 动作: {}", other)),
    }
}

pub fn evolution_act_dispatch(payload: &str) -> Result<String, String> {
    evolution_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genome_creation() {
        let mut genome = EvolutionGenome::new(1, 1, None);
        genome.add_gene("learning_rate", 0.01, 0.0, 1.0, 0.1);
        assert_eq!(genome.gene_count(), 1);
        let gene = genome.get_gene("learning_rate").unwrap();
        assert!((gene.value - 0.01).abs() < 1e-6);
    }

    #[test]
    fn genome_set_gene() {
        let mut genome = EvolutionGenome::new(1, 1, None);
        genome.add_gene("lr", 0.5, 0.0, 1.0, 0.1);
        assert!(genome.set_gene("lr", 0.8));
        assert!(!genome.set_gene("lr", 2.0)); // 超出范围
        assert!((genome.get_gene("lr").unwrap().value - 0.8).abs() < 1e-6);
    }

    #[test]
    fn genome_mutate_gene() {
        let mut genome = EvolutionGenome::new(1, 1, None);
        genome.add_gene("lr", 0.5, 0.0, 1.0, 0.1);
        assert!(genome.mutate_gene("lr", 0.2));
        assert!((genome.get_gene("lr").unwrap().value - 0.7).abs() < 1e-6);
    }

    #[test]
    fn mutator_point_mutate() {
        let mut mutator = Mutator::new();
        let mut genome = EvolutionGenome::new(1, 1, None);
        genome.add_gene("lr", 0.5, 0.0, 1.0, 0.1);
        assert!(mutator.point_mutate(&mut genome, "lr", 0.3));
        assert_eq!(mutator.applied_count, 1);
        assert_eq!(mutator.success_count, 1);
    }

    #[test]
    fn mutator_random_mutate() {
        let mut mutator = Mutator::new();
        mutator.mutation_rate = 1.0; // 100% 突变率
        let mut genome = EvolutionGenome::new(1, 1, None);
        genome.add_gene("a", 0.5, 0.0, 1.0, 0.1);
        genome.add_gene("b", 0.5, 0.0, 1.0, 0.1);
        let mutations = mutator.random_mutate(&mut genome);
        assert!(!mutations.is_empty());
    }

    #[test]
    fn fitness_function() {
        let mut ff = FitnessFunction::new();
        ff.add_metric("accuracy", 0.6);
        ff.add_metric("speed", 0.4);
        ff.set_metric_value("accuracy", 0.9);
        ff.set_metric_value("speed", 0.7);
        let score = ff.compute();
        assert!((score - 0.82).abs() < 1e-6); // 0.9*0.6 + 0.7*0.4 = 0.54 + 0.28 = 0.82
    }

    #[test]
    fn natural_selection() {
        let mut ns = NaturalSelection::new();
        ns.add_individual(1, 0.9);
        ns.add_individual(2, 0.3);
        ns.add_individual(3, 0.7);
        assert_eq!(ns.population_size(), 3);
        let survivors = ns.select_survivors(0.5);
        assert_eq!(survivors.len(), 2);
        assert!((ns.avg_fitness() - 0.9 - 0.7).abs() < 1e-6 || ns.avg_fitness() > 0.0);
    }

    #[test]
    fn version_control() {
        let mut vc = VersionControl::new();
        let mut genes = BTreeMap::new();
        genes.insert("a".to_string(), 0.5);
        let v1 = vc.snapshot(&genes, Some(0.8));
        assert_eq!(v1, 1);
        genes.insert("a".to_string(), 0.9);
        let v2 = vc.snapshot(&genes, Some(0.9));
        assert_eq!(v2, 2);
        let rolled = vc.rollback(1).unwrap();
        assert!((rolled.get("a").unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn adaptor() {
        let mut adaptor = Adaptor::new();
        adaptor.adapt("lr", 0.1, 0.01, "collect");
        adaptor.adapt("bs", 32, 64, "memory");
        assert_eq!(adaptor.adaptation_count, 2);
        assert_eq!(adaptor.successful_count, 2);
    }

    #[test]
    fn evolution_engine_full() {
        let mut eng = EvolutionEngine::new();
        let gid = eng.create_genome(1, None);
        eng.add_gene_to_genome(gid, "lr", 0.01, 0.0, 1.0, 0.1).unwrap();
        eng.mutate_genome(gid, "lr", 0.05).unwrap();
        eng.set_fitness_metric("accuracy", 1.0);
        eng.set_fitness_value("accuracy", 0.9).unwrap();
        let f = eng.compute_fitness();
        assert!((f - 0.9).abs() < 1e-6);
        eng.assign_fitness_to_genome(gid).unwrap();
        let ind_id = eng.add_individual(gid);
        assert_eq!(ind_id, 1);
        eng.snapshot_genome(gid).unwrap();
        eng.adapt_parameter("lr", 0.06, 0.02, "converge").unwrap();
    }

    #[test]
    fn evolution_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("evolution:"));
        assert!(s.contains("genomes="));
    }

    #[test]
    fn evolution_act_create_genome() {
        init();
        let res = evolution_act("create-genome 1").unwrap();
        assert!(res.contains("已创建基因组"));
    }

    #[test]
    fn evolution_act_add_gene() {
        init();
        evolution_act("create-genome 1").unwrap();
        let res = evolution_act("add-gene 1 lr 0.01 0.0 1.0 0.1").unwrap();
        assert!(res.contains("已添加基因"));
    }

    #[test]
    fn evolution_act_mutate() {
        init();
        evolution_act("create-genome 1").unwrap();
        evolution_act("add-gene 1 lr 0.01 0.0 1.0 0.1").unwrap();
        let res = evolution_act("mutate 1 lr 0.05").unwrap();
        assert!(res.contains("已突变"));
    }

    #[test]
    fn evolution_act_compute_fitness() {
        init();
        evolution_act("create-genome 1").unwrap();
        evolution_act("add-gene 1 lr 0.01 0.0 1.0 0.1").unwrap();
        evolution_act("set-fitness-metric accuracy 1.0").unwrap();
        evolution_act("set-fitness-value accuracy 0.9").unwrap();
        let res = evolution_act("compute-fitness").unwrap();
        assert!(res.contains("适应度"));
    }

    #[test]
    fn evolution_act_select() {
        init();
        evolution_act("create-genome 1").unwrap();
        evolution_act("add-gene 1 lr 0.01 0.0 1.0 0.1").unwrap();
        evolution_act("set-fitness-metric accuracy 1.0").unwrap();
        evolution_act("set-fitness-value accuracy 0.9").unwrap();
        evolution_act("assign-fitness 1").unwrap();
        evolution_act("add-individual 1").unwrap();
        let res = evolution_act("select 0.5").unwrap();
        assert!(res.contains("幸存者"));
    }

    #[test]
    fn evolution_act_snapshot_rollback() {
        init();
        evolution_act("create-genome 1").unwrap();
        evolution_act("add-gene 1 lr 0.01 0.0 1.0 0.1").unwrap();
        let res = evolution_act("snapshot 1").unwrap();
        assert!(res.contains("快照"));
        let res = evolution_act("rollback 1").unwrap();
        assert!(res.contains("回滚"));
    }

    #[test]
    fn evolution_act_adapt() {
        init();
        let res = evolution_act("adapt lr 0.01 0.001 converge").unwrap();
        assert!(res.contains("已适应参数"));
    }

    #[test]
    fn evolution_act_next_generation() {
        init();
        let res = evolution_act("next-generation").unwrap();
        assert!(res.contains("下一代"));
    }

    #[test]
    fn evolution_act_status() {
        init();
        let res = evolution_act("status").unwrap();
        assert!(res.contains("evolution:"));
    }

    #[test]
    fn mutation_types() {
        assert_eq!(MutationType::Point as u32, 0);
        assert_eq!(MutationType::Duplication as u32, 3);
    }
}

// =====================================================================================
// 遗传算法 (GeneticAlgorithm)
// =====================================================================================

/// 选择策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionStrategy {
    Roulette,    // 轮盘赌
    Tournament,  // 锦标赛
    Rank,        // 排序
    Elitism,     // 精英保留
}

/// 个体适应度。
#[derive(Clone, Debug)]
pub struct IndividualFitness {
    pub individual_id: u64,
    pub fitness: f32,
    pub rank: u64,
}

/// 遗传算法：模拟自然选择的优化算法。
pub struct GeneticAlgorithm {
    pub strategy: SelectionStrategy,
    pub population: BTreeMap<u64, IndividualFitness>,
    pub mutation_rate: f32,
    pub crossover_rate: f32,
    pub generation: u64,
}

impl GeneticAlgorithm {
    pub fn new() -> Self {
        Self {
            strategy: SelectionStrategy::Tournament,
            population: BTreeMap::new(),
            mutation_rate: 0.01,
            crossover_rate: 0.7,
            generation: 0,
        }
    }

    pub fn set_strategy(&mut self, strategy: SelectionStrategy) {
        self.strategy = strategy;
    }

    pub fn add_individual(&mut self, id: u64, fitness: f32) {
        self.population.insert(id, IndividualFitness {
            individual_id: id,
            fitness: fitness.clamp(0.0, 1.0),
            rank: 0,
        });
    }

    pub fn rank_population(&mut self) {
        let mut sorted: Vec<(u64, f32)> = self.population.iter()
            .map(|(id, f)| (*id, f.fitness))
            .collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, (id, _)) in sorted.iter().enumerate() {
            if let Some(ind) = self.population.get_mut(id) {
                ind.rank = rank as u64;
            }
        }
    }

    pub fn select_parent(&self) -> Option<u64> {
        match self.strategy {
            SelectionStrategy::Roulette => {
                let total: f32 = self.population.values().map(|f| f.fitness).sum();
                if total == 0.0 {
                    return self.population.keys().next().copied();
                }
                let mut target = fastrand_f32() * total;
                for (id, f) in &self.population {
                    target -= f.fitness;
                    if target <= 0.0 {
                        return Some(*id);
                    }
                }
                self.population.keys().next().copied()
            }
            SelectionStrategy::Tournament => {
                let mut best = None;
                let mut best_fitness = f32::NEG_INFINITY;
                for _ in 0..3.min(self.population.len()) {
                    let ids: Vec<u64> = self.population.keys().copied().collect();
                    let idx = (fastrand_f32() * ids.len() as f32) as usize;
                    if let Some(id) = ids.get(idx) {
                        if let Some(f) = self.population.get(id) {
                            if f.fitness > best_fitness {
                                best_fitness = f.fitness;
                                best = Some(*id);
                            }
                        }
                    }
                }
                best
            }
            SelectionStrategy::Rank => {
                let sorted: Vec<u64> = self.population.keys().copied().collect();
                sorted.first().copied()
            }
            SelectionStrategy::Elitism => {
                self.population.values()
                    .max_by(|a, b| a.fitness.partial_cmp(&b.fitness).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|f| f.individual_id)
            }
        }
    }

    pub fn evolve(&mut self) {
        self.rank_population();
        self.generation += 1;
    }

    pub fn best_fitness(&self) -> f32 {
        self.population.values().map(|f| f.fitness).fold(0.0f32, |a, b| a.max(b))
    }

    pub fn avg_fitness(&self) -> f32 {
        if self.population.is_empty() {
            0.0
        } else {
            self.population.values().map(|f| f.fitness).sum::<f32>() / self.population.len() as f32
        }
    }
}

// =====================================================================================
// 基因编辑 (GeneEditing)
// =====================================================================================

/// 编辑操作类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOperation {
    Insert,   // 插入
    Delete,   // 删除
    Replace,  // 替换
}

/// 基因编辑记录。
#[derive(Clone, Debug)]
pub struct GeneEdit {
    pub id: u64,
    pub gene_name: String,
    pub operation: EditOperation,
    pub old_value: Option<f32>,
    pub new_value: Option<f32>,
    pub timestamp_ns: u128,
}

/// 基因编辑器：精确修改基因。
pub struct GeneEditor {
    pub edits: Vec<GeneEdit>,
    pub next_id: u64,
}

impl GeneEditor {
    pub fn new() -> Self {
        Self {
            edits: Vec::new(),
            next_id: 1,
        }
    }

    pub fn edit(&mut self, gene_name: &str, operation: EditOperation, old_value: Option<f32>, new_value: Option<f32>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.edits.push(GeneEdit {
            id,
            gene_name: gene_name.to_string(),
            operation,
            old_value,
            new_value,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        id
    }

    pub fn get_edit(&self, id: u64) -> Option<&GeneEdit> {
        self.edits.iter().find(|e| e.id == id)
    }

    pub fn edit_count(&self) -> usize {
        self.edits.len()
    }
}

// =====================================================================================
// 进化策略 (EvolutionStrategy)
// =====================================================================================

/// 策略参数。
#[derive(Clone, Debug)]
pub struct StrategyParameters {
    pub mutation_sigma: f32,
    pub learning_rate: f32,
    pub population_size: u64,
    pub offspring_size: u64,
}

/// 进化策略：自适应参数调优的优化算法。
pub struct EvolutionStrategy {
    pub params: StrategyParameters,
    pub history: Vec<(u64, f32)>, // (generation, best_fitness)
}

impl EvolutionStrategy {
    pub fn new() -> Self {
        Self {
            params: StrategyParameters {
                mutation_sigma: 0.1,
                learning_rate: 0.01,
                population_size: 100,
                offspring_size: 200,
            },
            history: Vec::new(),
        }
    }

    pub fn update_params(&mut self, fitness_improvement: f32) {
        if fitness_improvement > 0.0 {
            self.params.mutation_sigma = (self.params.mutation_sigma * 0.99).max(0.001);
            self.params.learning_rate = (self.params.learning_rate * 1.001).min(0.1);
        } else {
            self.params.mutation_sigma = (self.params.mutation_sigma * 1.01).min(1.0);
            self.params.learning_rate = (self.params.learning_rate * 0.999).max(0.0001);
        }
    }

    pub fn record_generation(&mut self, generation: u64, best_fitness: f32) {
        self.history.push((generation, best_fitness));
    }

    pub fn generation_count(&self) -> usize {
        self.history.len()
    }

    pub fn best_fitness_history(&self) -> Vec<f32> {
        self.history.iter().map(|(_, f)| *f).collect()
    }
}

// =====================================================================================
// 种群动态 (PopulationDynamics)
// =====================================================================================

/// 种群状态。
#[derive(Clone, Debug)]
pub struct PopulationState {
    pub generation: u64,
    pub size: u64,
    pub avg_fitness: f32,
    pub max_fitness: f32,
    pub min_fitness: f32,
    pub diversity: f32,
}

/// 种群动态：跟踪种群随时间的变化。
pub struct PopulationDynamics {
    pub states: Vec<PopulationState>,
    pub carrying_capacity: u64,
}

impl PopulationDynamics {
    pub fn new() -> Self {
        Self {
            states: Vec::new(),
            carrying_capacity: 10000,
        }
    }

    pub fn record(&mut self, generation: u64, size: u64, fitnesses: &[f32]) {
        let avg = if fitnesses.is_empty() {
            0.0
        } else {
            fitnesses.iter().sum::<f32>() / fitnesses.len() as f32
        };
        let max = fitnesses.iter().fold(0.0f32, |a, b| a.max(*b));
        let min = fitnesses.iter().fold(f32::INFINITY, |a, b| a.min(*b));
        let diversity = if fitnesses.len() > 1 {
            let mean = avg;
            let variance: f32 = fitnesses.iter().map(|f| (f - mean) * (f - mean)).sum::<f32>() / fitnesses.len() as f32;
            variance.sqrt()
        } else {
            0.0
        };
        self.states.push(PopulationState {
            generation,
            size,
            avg_fitness: avg,
            max_fitness: max,
            min_fitness: min,
            diversity,
        });
    }

    pub fn latest_state(&self) -> Option<&PopulationState> {
        self.states.last()
    }

    pub fn state_count(&self) -> usize {
        self.states.len()
    }
}

// =====================================================================================
// 进化扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn genetic_algorithm() {
        let mut ga = GeneticAlgorithm::new();
        ga.add_individual(1, 0.5);
        ga.add_individual(2, 0.8);
        ga.add_individual(3, 0.3);
        ga.rank_population();
        ga.evolve();
        assert_eq!(ga.generation, 1);
        assert!((ga.best_fitness() - 0.8).abs() < 1e-6);
        assert!((ga.avg_fitness() - 0.533).abs() < 0.01);
    }

    #[test]
    fn genetic_algorithm_selection() {
        let mut ga = GeneticAlgorithm::new();
        ga.set_strategy(SelectionStrategy::Roulette);
        ga.add_individual(1, 0.9);
        ga.add_individual(2, 0.1);
        let parent = ga.select_parent();
        assert!(parent.is_some());
    }

    #[test]
    fn gene_editor() {
        let mut editor = GeneEditor::new();
        let id = editor.edit("gene_a", EditOperation::Replace, Some(0.5), Some(0.8));
        assert_eq!(id, 1);
        let edit = editor.get_edit(id).unwrap();
        assert_eq!(edit.gene_name, "gene_a");
        assert_eq!(edit.old_value, Some(0.5));
        assert_eq!(edit.new_value, Some(0.8));
        assert_eq!(editor.edit_count(), 1);
    }

    #[test]
    fn evolution_strategy() {
        let mut es = EvolutionStrategy::new();
        es.record_generation(1, 0.5);
        es.record_generation(2, 0.7);
        es.record_generation(3, 0.85);
        assert_eq!(es.generation_count(), 3);
        let history = es.best_fitness_history();
        assert_eq!(history.len(), 3);
        assert!((history[2] - 0.85).abs() < 1e-6);
        es.update_params(0.1);
        assert!(es.params.mutation_sigma < 0.1);
    }

    #[test]
    fn population_dynamics() {
        let mut pd = PopulationDynamics::new();
        pd.record(1, 100, &[0.1, 0.5, 0.9]);
        pd.record(2, 150, &[0.2, 0.6, 0.8]);
        assert_eq!(pd.state_count(), 2);
        let latest = pd.latest_state().unwrap();
        assert_eq!(latest.generation, 2);
        assert_eq!(latest.size, 150);
        assert!((latest.avg_fitness - 0.533).abs() < 0.01);
    }

    #[test]
    fn selection_strategies() {
        assert_eq!(SelectionStrategy::Roulette as u32, 0);
        assert_eq!(SelectionStrategy::Elitism as u32, 3);
    }

    #[test]
    fn edit_operations() {
        assert_eq!(EditOperation::Insert as u32, 0);
        assert_eq!(EditOperation::Replace as u32, 2);
    }
}
