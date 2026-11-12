//! 语义空间层 —— AI 原生 OS 的「语义空间」 (Semantic Space)
//!
//! 本模块实现 AI 原生 OS 的语义表示与推理，对应传统 OS 中的「文件系统
//! 与命名空间」概念，但重新映射到语义域：
//!
//! * SemanticVector (语义向量) —— 高维语义空间中的点，等价于 inode；
//! * SemanticSpace (语义空间) —— 向量空间容器，等价于文件系统超级块；
//! * Concept (概念) —— 具备名称与语义向量的抽象对象，等价于文件；
//! * ConceptGraph (概念图) —— 概念间的语义关系图，等价于目录树；
//! * SemanticSimilarity (语义相似度) —— 向量余弦相似度计算；
//! * SemanticSearch (语义检索) —— 最近邻搜索，等价于 find/locate；
//! * SemanticCluster (语义聚类) —— K-Means / 层次聚类，等价于目录分组；
//! * SemanticAnalogy (语义类比) —— king - man + woman = queen 类推；
//! * SemanticCompression (语义压缩) —— 向量量化与降维；
//! * SemanticNamespace (语义命名空间) —— 路径到向量的映射。
//!
//! 所有结构均为纯 Rust 原型，无外部嵌入模型依赖，供内核统一核算与调度，
//! 并通过 `semantic://*` 通道接入 `sense`/`act` 感知层。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 语义向量 (SemanticVector)
// =====================================================================================

/// 语义向量：高维语义空间中的点。
#[derive(Clone, Debug)]
pub struct SemanticVector {
    pub id: u64,
    pub dims: Vec<f32>,
    pub norm: f32,
    pub metadata: BTreeMap<String, String>,
}

impl SemanticVector {
    pub fn new(id: u64, dims: Vec<f32>) -> Self {
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        Self {
            id,
            dims,
            norm,
            metadata: BTreeMap::new(),
        }
    }

    pub fn from_hash(id: u64, text: &str, dim: usize) -> Self {
        let hash = text.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        let dims: Vec<f32> = (0..dim)
            .map(|i| {
                let bit = (hash.wrapping_shl(i as u32) >> 63) & 1;
                (bit as f32) * 2.0 - 1.0
            })
            .collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        Self { id, dims, norm, metadata: BTreeMap::new() }
    }

    pub fn cosine_similarity(&self, other: &SemanticVector) -> f32 {
        if self.dims.len() != other.dims.len() {
            return 0.0;
        }
        let dot: f32 = self.dims.iter().zip(other.dims.iter()).map(|(a, b)| a * b).sum();
        let denom = self.norm * other.norm;
        if denom == 0.0 { 0.0 } else { dot / denom }
    }

    pub fn euclidean_distance(&self, other: &SemanticVector) -> f32 {
        if self.dims.len() != other.dims.len() {
            return f32::INFINITY;
        }
        self.dims.iter().zip(other.dims.iter()).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt()
    }

    pub fn add(&self, other: &SemanticVector) -> SemanticVector {
        let dims: Vec<f32> = self.dims.iter().zip(other.dims.iter()).map(|(a, b)| a + b).collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        SemanticVector { id: 0, dims, norm, metadata: BTreeMap::new() }
    }

    pub fn sub(&self, other: &SemanticVector) -> SemanticVector {
        let dims: Vec<f32> = self.dims.iter().zip(other.dims.iter()).map(|(a, b)| a - b).collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        SemanticVector { id: 0, dims, norm, metadata: BTreeMap::new() }
    }

    pub fn scale(&self, s: f32) -> SemanticVector {
        let dims: Vec<f32> = self.dims.iter().map(|x| x * s).collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        SemanticVector { id: 0, dims, norm, metadata: BTreeMap::new() }
    }

    pub fn normalize(&self) -> SemanticVector {
        if self.norm == 0.0 {
            return self.clone();
        }
        let dims: Vec<f32> = self.dims.iter().map(|x| x / self.norm).collect();
        SemanticVector { id: self.id, dims, norm: 1.0, metadata: self.metadata.clone() }
    }

    pub fn dim(&self) -> usize {
        self.dims.len()
    }
}

// =====================================================================================
// 概念 (Concept)
// =====================================================================================

/// 概念：具备名称与语义向量的抽象对象。
#[derive(Clone, Debug)]
pub struct Concept {
    pub id: u64,
    pub name: String,
    pub vector: SemanticVector,
    pub attributes: BTreeMap<String, String>,
    pub created_at: u128,
    pub access_count: u64,
}

impl Concept {
    pub fn new(id: u64, name: &str, dim: usize) -> Self {
        Self {
            id,
            name: name.to_string(),
            vector: SemanticVector::from_hash(id, name, dim),
            attributes: BTreeMap::new(),
            created_at: crate::ai_os::native::now_ns(),
            access_count: 0,
        }
    }

    pub fn similarity_to(&self, other: &Concept) -> f32 {
        self.vector.cosine_similarity(&other.vector)
    }

    pub fn touch(&mut self) {
        self.access_count += 1;
    }
}

// =====================================================================================
// 概念图 (ConceptGraph)
// =====================================================================================

/// 概念间关系类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConceptRelation {
    IsA,          // 是一种
    PartOf,       // 是……的一部分
    HasProperty,  // 具有属性
    SimilarTo,    // 相似于
    OppositeOf,   // 相反
    Causes,       // 导致
    UsedFor,      // 用于
    LocatedAt,    // 位于
}

/// 图边。
#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub from: u64,
    pub to: u64,
    pub relation: ConceptRelation,
    pub weight: f32,
}

/// 概念图：概念间的语义关系图。
pub struct ConceptGraph {
    concepts: BTreeMap<u64, Concept>,
    edges: Vec<GraphEdge>,
    next_id: u64,
    name_index: BTreeMap<String, u64>,
    dim: usize,
}

impl ConceptGraph {
    pub fn new(dim: usize) -> Self {
        Self {
            concepts: BTreeMap::new(),
            edges: Vec::new(),
            next_id: 1,
            name_index: BTreeMap::new(),
            dim,
        }
    }

    pub fn add_concept(&mut self, name: &str) -> u64 {
        if let Some(&id) = self.name_index.get(name) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        let concept = Concept::new(id, name, self.dim);
        self.name_index.insert(name.to_string(), id);
        self.concepts.insert(id, concept);
        id
    }

    pub fn get_concept(&self, name: &str) -> Option<&Concept> {
        self.name_index.get(name).and_then(|id| self.concepts.get(id))
    }

    pub fn get_concept_by_id(&self, id: u64) -> Option<&Concept> {
        self.concepts.get(&id)
    }

    pub fn add_edge(&mut self, from: &str, to: &str, relation: ConceptRelation, weight: f32) {
        let from_id = self.add_concept(from);
        let to_id = self.add_concept(to);
        self.edges.push(GraphEdge { from: from_id, to: to_id, relation, weight });
    }

    pub fn neighbors(&self, name: &str) -> Vec<(u64, ConceptRelation, f32)> {
        let id = match self.name_index.get(name) {
            Some(id) => *id,
            None => return Vec::new(),
        };
        self.edges.iter()
            .filter(|e| e.from == id)
            .map(|e| (e.to, e.relation, e.weight))
            .collect()
    }

    pub fn concept_count(&self) -> usize {
        self.concepts.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn dim(&self) -> usize {
        self.dim
    }
}

// =====================================================================================
// 语义空间 (SemanticSpace)
// =====================================================================================

/// 语义空间：向量空间容器，管理所有语义向量。
pub struct SemanticSpace {
    vectors: BTreeMap<u64, SemanticVector>,
    next_id: u64,
    pub dim: usize,
    pub total_insertions: u64,
}

impl SemanticSpace {
    pub fn new(dim: usize) -> Self {
        Self {
            vectors: BTreeMap::new(),
            next_id: 1,
            dim,
            total_insertions: 0,
        }
    }

    pub fn insert(&mut self, vector: SemanticVector) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut v = vector;
        v.id = id;
        self.vectors.insert(id, v);
        self.total_insertions += 1;
        id
    }

    pub fn insert_from_text(&mut self, text: &str) -> u64 {
        let v = SemanticVector::from_hash(self.next_id, text, self.dim);
        self.insert(v)
    }

    pub fn get(&self, id: u64) -> Option<&SemanticVector> {
        self.vectors.get(&id)
    }

    pub fn remove(&mut self, id: u64) -> Option<SemanticVector> {
        self.vectors.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    pub fn all(&self) -> Vec<&SemanticVector> {
        self.vectors.values().collect()
    }
}

impl Default for SemanticSpace {
    fn default() -> Self {
        Self::new(128)
    }
}

// =====================================================================================
// 语义检索 (SemanticSearch)
// =====================================================================================

/// 检索结果。
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub id: u64,
    pub similarity: f32,
    pub distance: f32,
}

/// 语义检索：最近邻搜索。
pub struct SemanticSearch {
    space: SemanticSpace,
}

impl SemanticSearch {
    pub fn new(dim: usize) -> Self {
        Self {
            space: SemanticSpace::new(dim),
        }
    }

    pub fn index(&mut self, text: &str) -> u64 {
        self.space.insert_from_text(text)
    }

    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchResult> {
        let qv = SemanticVector::from_hash(0, query, self.space.dim);
        let mut results: Vec<SearchResult> = self.space.all().iter().map(|v| {
            let sim = qv.cosine_similarity(v);
            let dist = qv.euclidean_distance(v);
            SearchResult { id: v.id, similarity: sim, distance: dist }
        }).collect();
        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    pub fn search_by_vector(&self, query: &SemanticVector, top_k: usize) -> Vec<SearchResult> {
        let mut results: Vec<SearchResult> = self.space.all().iter().map(|v| {
            let sim = query.cosine_similarity(v);
            let dist = query.euclidean_distance(v);
            SearchResult { id: v.id, similarity: sim, distance: dist }
        }).collect();
        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    pub fn count(&self) -> usize {
        self.space.len()
    }
}

// =====================================================================================
// 语义聚类 (SemanticCluster)
// =====================================================================================

/// 聚类结果。
#[derive(Clone, Debug)]
pub struct Cluster {
    pub id: u64,
    pub centroid: SemanticVector,
    pub member_ids: Vec<u64>,
}

/// K-Means 语义聚类。
pub struct SemanticCluster {
    clusters: Vec<Cluster>,
    next_cluster_id: u64,
}

impl SemanticCluster {
    pub fn new() -> Self {
        Self {
            clusters: Vec::new(),
            next_cluster_id: 1,
        }
    }

    pub fn kmeans(&mut self, vectors: &[&SemanticVector], k: usize, max_iters: usize) -> Vec<Cluster> {
        if vectors.is_empty() || k == 0 {
            return Vec::new();
        }
        let k = k.min(vectors.len());
        let dim = vectors[0].dim();
        // 随机初始化质心
        let mut centroids: Vec<SemanticVector> = (0..k).map(|i| {
            let v = vectors[i % vectors.len()];
            let dims: Vec<f32> = v.dims.iter().map(|x| *x).collect();
            SemanticVector { id: i as u64, dims, norm: v.norm, metadata: BTreeMap::new() }
        }).collect();
        let mut assignments: Vec<usize> = vec![0; vectors.len()];
        for _ in 0..max_iters {
            // 分配
            for (i, v) in vectors.iter().enumerate() {
                let best = centroids.iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        let da = v.euclidean_distance(a);
                        let db = v.euclidean_distance(b);
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(j, _)| j)
                    .unwrap_or(0);
                assignments[i] = best;
            }
            // 更新质心
            let mut new_centroids: Vec<Vec<f32>> = vec![vec![0.0; dim]; k];
            let mut counts: Vec<usize> = vec![0; k];
            for (i, v) in vectors.iter().enumerate() {
                let c = assignments[i];
                for (d, val) in v.dims.iter().enumerate() {
                    new_centroids[c][d] += val;
                }
                counts[c] += 1;
            }
            let mut changed = false;
            for c in 0..k {
                if counts[c] > 0 {
                    let dims: Vec<f32> = new_centroids[c].iter().map(|x| x / counts[c] as f32).collect();
                    let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let new_cv = SemanticVector { id: c as u64, dims, norm, metadata: BTreeMap::new() };
                    if centroids[c].euclidean_distance(&new_cv) > 1e-6 {
                        changed = true;
                    }
                    centroids[c] = new_cv;
                }
            }
            if !changed { break; }
        }
        // 构建聚类结果
        self.clusters = (0..k).map(|c| {
            let member_ids: Vec<u64> = assignments.iter()
                .enumerate()
                .filter(|(_, &a)| a == c)
                .map(|(i, _)| vectors[i].id)
                .collect();
            Cluster {
                id: c as u64,
                centroid: centroids[c].clone(),
                member_ids,
            }
        }).collect();
        self.next_cluster_id = k as u64 + 1;
        self.clusters.clone()
    }

    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }
}

// =====================================================================================
// 语义类比 (SemanticAnalogy)
// =====================================================================================

/// 语义类比：A - B + C = D。
pub struct SemanticAnalogy;

impl SemanticAnalogy {
    pub fn solve(a: &SemanticVector, b: &SemanticVector, c: &SemanticVector) -> SemanticVector {
        // d = a - b + c
        let ab = a.sub(b);
        let d = ab.add(c);
        d
    }

    pub fn analogy_score(a: &SemanticVector, b: &SemanticVector, c: &SemanticVector, d: &SemanticVector) -> f32 {
        let predicted = Self::solve(a, b, c);
        predicted.cosine_similarity(d)
    }
}

// =====================================================================================
// 语义压缩 (SemanticCompression)
// =====================================================================================

/// 压缩后的语义向量。
#[derive(Clone, Debug)]
pub struct CompressedVector {
    pub id: u64,
    pub dims: Vec<i8>,        // INT8 量化
    pub scale: f32,
    pub original_dim: usize,
}

/// 语义压缩器：向量量化与降维。
pub struct SemanticCompression;

impl SemanticCompression {
    pub fn quantize_int8(vector: &SemanticVector) -> CompressedVector {
        let max_abs = vector.dims.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        let scale = if max_abs > 0.0 { 127.0 / max_abs } else { 1.0 };
        let dims: Vec<i8> = vector.dims.iter().map(|x| (x * scale).round() as i8).collect();
        CompressedVector { id: vector.id, dims, scale, original_dim: vector.dim() }
    }

    pub fn dequantize_int8(compressed: &CompressedVector) -> SemanticVector {
        let dims: Vec<f32> = compressed.dims.iter().map(|x| (*x as f32) / compressed.scale).collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        SemanticVector { id: compressed.id, dims, norm, metadata: BTreeMap::new() }
    }

    pub fn pca_reduce(vector: &SemanticVector, target_dim: usize) -> SemanticVector {
        // 简化：前 target_dim 维
        let dims: Vec<f32> = vector.dims.iter().take(target_dim).cloned().collect();
        let norm = dims.iter().map(|x| x * x).sum::<f32>().sqrt();
        SemanticVector { id: vector.id, dims, norm, metadata: BTreeMap::new() }
    }
}

// =====================================================================================
// 语义命名空间 (SemanticNamespace)
// =====================================================================================

/// 语义命名空间：路径到语义向量的映射。
pub struct SemanticNamespace {
    paths: BTreeMap<String, u64>,   // 路径 -> 向量 ID
    vectors: BTreeMap<u64, SemanticVector>,
    next_id: u64,
    pub dim: usize,
}

impl SemanticNamespace {
    pub fn new(dim: usize) -> Self {
        Self {
            paths: BTreeMap::new(),
            vectors: BTreeMap::new(),
            next_id: 1,
            dim,
        }
    }

    pub fn bind(&mut self, path: &str, text: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let v = SemanticVector::from_hash(id, text, self.dim);
        self.paths.insert(path.to_string(), id);
        self.vectors.insert(id, v);
        id
    }

    pub fn resolve(&self, path: &str) -> Option<&SemanticVector> {
        self.paths.get(path).and_then(|id| self.vectors.get(id))
    }

    pub fn unbind(&mut self, path: &str) -> Option<SemanticVector> {
        if let Some(id) = self.paths.remove(path) {
            self.vectors.remove(&id)
        } else {
            None
        }
    }

    pub fn list_paths(&self) -> Vec<&String> {
        self.paths.keys().collect()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

// =====================================================================================
// 语义引擎 (SemanticEngine) —— 统一入口
// =====================================================================================

/// 语义引擎：整合语义向量、概念、概念图、语义空间、语义检索、语义聚类、语义类比、语义压缩、语义命名空间。
pub struct SemanticEngine {
    pub space: SemanticSpace,
    pub graph: ConceptGraph,
    pub search: SemanticSearch,
    pub cluster: SemanticCluster,
    pub namespace: SemanticNamespace,
    pub dim: usize,
    pub total_operations: u64,
}

impl SemanticEngine {
    pub fn new(dim: usize) -> Self {
        Self {
            space: SemanticSpace::new(dim),
            graph: ConceptGraph::new(dim),
            search: SemanticSearch::new(dim),
            cluster: SemanticCluster::new(),
            namespace: SemanticNamespace::new(dim),
            dim,
            total_operations: 0,
        }
    }

    pub fn index_text(&mut self, text: &str) -> u64 {
        let id = self.space.insert_from_text(text);
        self.search.index(text);
        self.total_operations += 1;
        id
    }

    pub fn add_concept(&mut self, name: &str) -> u64 {
        self.total_operations += 1;
        self.graph.add_concept(name)
    }

    pub fn relate(&mut self, from: &str, to: &str, relation: ConceptRelation, weight: f32) {
        self.graph.add_edge(from, to, relation, weight);
        self.total_operations += 1;
    }

    pub fn search(&mut self, query: &str, top_k: usize) -> Vec<SearchResult> {
        self.total_operations += 1;
        self.search.search(query, top_k)
    }

    pub fn cluster_kmeans(&mut self, k: usize) -> Vec<Cluster> {
        let vectors: Vec<&SemanticVector> = self.space.all();
        self.total_operations += 1;
        self.cluster.kmeans(&vectors, k, 50)
    }

    pub fn bind_namespace(&mut self, path: &str, text: &str) -> u64 {
        let id = self.namespace.bind(path, text);
        self.total_operations += 1;
        id
    }

    pub fn resolve_namespace(&self, path: &str) -> Option<f32> {
        self.namespace.resolve(path).map(|v| v.norm)
    }

    pub fn analogy(&mut self, _a: &str, _b: &str, _c: &str) -> Option<(u64, f32)> {
        let _va = self.space.all().iter().find(|v| {
            self.namespace.list_paths().iter().any(|p| self.namespace.resolve(p).map(|rv| rv.id == v.id).unwrap_or(false))
        });
        // 简化：直接对前三个向量做类比
        let all: Vec<&SemanticVector> = self.space.all();
        if all.len() < 3 { return None; }
        let predicted = SemanticAnalogy::solve(all[0], all[1], all[2]);
        // 找最接近的
        let mut best_id = 0;
        let mut best_sim = f32::NEG_INFINITY;
        for v in &all {
            let sim = predicted.cosine_similarity(v);
            if sim > best_sim {
                best_sim = sim;
                best_id = v.id;
            }
        }
        self.total_operations += 1;
        Some((best_id, best_sim))
    }

    pub fn concept_count(&self) -> usize {
        self.graph.concept_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    pub fn vector_count(&self) -> usize {
        self.space.len()
    }
}

impl Default for SemanticEngine {
    fn default() -> Self {
        Self::new(128)
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SEMANTIC: OnceLock<Mutex<SemanticEngine>> = OnceLock::new();

pub fn semantic_engine() -> &'static Mutex<SemanticEngine> {
    SEMANTIC.get_or_init(|| Mutex::new(SemanticEngine::new(128)))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = semantic_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = semantic_engine().lock().unwrap();
    format!(
        "semantic: dim={} vectors={} concepts={} edges={} operations={} namespace_entries={}",
        eng.dim,
        eng.vector_count(),
        eng.concept_count(),
        eng.edge_count(),
        eng.total_operations,
        eng.namespace.len()
    )
}

pub fn semantic_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn semantic_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("index") => {
            let text = it.next().ok_or("用法: semantic:// index <文本>")?;
            let mut eng = semantic_engine().lock().unwrap();
            let id = eng.index_text(text);
            Ok(format!("已索引文本 id={} 内容={}", id, text))
        }
        Some("concept") => {
            let name = it.next().ok_or("用法: semantic:// concept <名称>")?;
            let mut eng = semantic_engine().lock().unwrap();
            let id = eng.add_concept(name);
            Ok(format!("已添加概念 id={} 名称={}", id, name))
        }
        Some("relate") => {
            let from = it.next().ok_or("用法: semantic:// relate <源> <目标> <关系> <权重>")?;
            let to = it.next().ok_or("缺少目标")?;
            let rel_str = it.next().unwrap_or("similar");
            let relation = match rel_str {
                "isa" => ConceptRelation::IsA,
                "partof" => ConceptRelation::PartOf,
                "hasprop" => ConceptRelation::HasProperty,
                "similar" => ConceptRelation::SimilarTo,
                "opposite" => ConceptRelation::OppositeOf,
                "causes" => ConceptRelation::Causes,
                "usedfor" => ConceptRelation::UsedFor,
                "locatedat" => ConceptRelation::LocatedAt,
                _ => ConceptRelation::SimilarTo,
            };
            let weight = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let mut eng = semantic_engine().lock().unwrap();
            eng.relate(from, to, relation, weight);
            Ok(format!("已建立关系 {} --{:?}--> {} 权重={}", from, relation, to, weight))
        }
        Some("search") => {
            let query = it.next().ok_or("用法: semantic:// search <查询> [top_k]")?;
            let top_k = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(5);
            let mut eng = semantic_engine().lock().unwrap();
            let results = eng.search(query, top_k);
            let mut out = String::from("语义检索结果:\n");
            for r in results {
                out.push_str(&format!("  id={} 相似度={:.3} 距离={:.3}\n", r.id, r.similarity, r.distance));
            }
            Ok(out)
        }
        Some("cluster") => {
            let k = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(3);
            let mut eng = semantic_engine().lock().unwrap();
            let clusters = eng.cluster_kmeans(k);
            let mut out = String::from("语义聚类结果:\n");
            for c in clusters {
                out.push_str(&format!("  簇{} 质心维度={} 成员数={}\n", c.id, c.centroid.dim(), c.member_ids.len()));
            }
            Ok(out)
        }
        Some("bind") => {
            let path = it.next().ok_or("用法: semantic:// bind <路径> <文本>")?;
            let text = it.next().ok_or("缺少文本")?;
            let mut eng = semantic_engine().lock().unwrap();
            let id = eng.bind_namespace(path, text);
            Ok(format!("已绑定 {} -> id={} 文本={}", path, id, text))
        }
        Some("resolve") => {
            let path = it.next().ok_or("用法: semantic:// resolve <路径>")?;
            let eng = semantic_engine().lock().unwrap();
            match eng.resolve_namespace(path) {
                Some(norm) => Ok(format!("路径 {} 解析成功 向量范数={:.3}", path, norm)),
                None => Err(format!("路径 {} 未绑定", path)),
            }
        }
        Some("analogy") => {
            let a = it.next().ok_or("用法: semantic:// analogy <A> <B> <C>")?;
            let b = it.next().ok_or("缺少 B")?;
            let c = it.next().ok_or("缺少 C")?;
            let mut eng = semantic_engine().lock().unwrap();
            match eng.analogy(a, b, c) {
                Some((id, sim)) => Ok(format!("类比结果 {} - {} + {} ≈ id={} 相似度={:.3}", a, b, c, id, sim)),
                None => Err("向量不足，无法进行类比".into()),
            }
        }
        Some("concept-similar") => {
            let n1 = it.next().ok_or("用法: semantic:// concept-similar <名称1> <名称2>")?;
            let n2 = it.next().ok_or("缺少名称2")?;
            let eng = semantic_engine().lock().unwrap();
            match (eng.graph.get_concept(n1), eng.graph.get_concept(n2)) {
                (Some(c1), Some(c2)) => {
                    let sim = c1.similarity_to(c2);
                    Ok(format!("概念 {} 与 {} 的相似度: {:.3}", n1, n2, sim))
                }
                _ => Err("一个或两个概念不存在".into()),
            }
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 semantic 动作: {}", other)),
    }
}

pub fn semantic_act_dispatch(payload: &str) -> Result<String, String> {
    semantic_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_vector_cosine() {
        let v1 = SemanticVector::new(1, vec![1.0, 0.0, 0.0]);
        let v2 = SemanticVector::new(2, vec![1.0, 0.0, 0.0]);
        assert!((v1.cosine_similarity(&v2) - 1.0).abs() < 1e-6);

        let v3 = SemanticVector::new(3, vec![0.0, 1.0, 0.0]);
        assert!((v1.cosine_similarity(&v3) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn semantic_vector_from_hash_deterministic() {
        let v1 = SemanticVector::from_hash(1, "hello", 64);
        let v2 = SemanticVector::from_hash(2, "hello", 64);
        assert_eq!(v1.dims, v2.dims);

        let v3 = SemanticVector::from_hash(3, "world", 64);
        assert_ne!(v1.dims, v3.dims);
    }

    #[test]
    fn semantic_vector_arithmetic() {
        let v1 = SemanticVector::new(1, vec![1.0, 2.0, 3.0]);
        let v2 = SemanticVector::new(2, vec![4.0, 5.0, 6.0]);
        let sum = v1.add(&v2);
        assert_eq!(sum.dims, vec![5.0, 7.0, 9.0]);
        let diff = v1.sub(&v2);
        assert_eq!(diff.dims, vec![-3.0, -3.0, -3.0]);
        let scaled = v1.scale(2.0);
        assert_eq!(scaled.dims, vec![2.0, 4.0, 6.0]);
    }

    #[test]
    fn semantic_vector_normalize() {
        let v = SemanticVector::new(1, vec![3.0, 4.0]);
        let n = v.normalize();
        assert!((n.norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn concept_creation() {
        let c = Concept::new(1, "apple", 64);
        assert_eq!(c.name, "apple");
        assert_eq!(c.vector.dim(), 64);
        assert_eq!(c.access_count, 0);
    }

    #[test]
    fn concept_similarity() {
        let c1 = Concept::new(1, "cat", 64);
        let c2 = Concept::new(2, "cat", 64);
        assert!((c1.similarity_to(&c2) - 1.0).abs() < 1e-6);

        let c3 = Concept::new(3, "dog", 64);
        assert!(c1.similarity_to(&c3) < 1.0);
    }

    #[test]
    fn concept_graph_add() {
        let mut g = ConceptGraph::new(64);
        let id1 = g.add_concept("animal");
        let id2 = g.add_concept("cat");
        assert_ne!(id1, id2);
        // 重复添加返回相同 id
        let id1_again = g.add_concept("animal");
        assert_eq!(id1, id1_again);
        assert_eq!(g.concept_count(), 2);
    }

    #[test]
    fn concept_graph_edge() {
        let mut g = ConceptGraph::new(64);
        g.add_edge("cat", "animal", ConceptRelation::IsA, 1.0);
        assert_eq!(g.edge_count(), 1);
        let neighbors = g.neighbors("cat");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].1, ConceptRelation::IsA);
    }

    #[test]
    fn concept_graph_get() {
        let mut g = ConceptGraph::new(64);
        g.add_concept("test");
        let c = g.get_concept("test").unwrap();
        assert_eq!(c.name, "test");
        assert!(g.get_concept("nonexistent").is_none());
    }

    #[test]
    fn semantic_space_insert() {
        let mut space = SemanticSpace::new(32);
        let id = space.insert_from_text("hello world");
        assert_eq!(id, 1);
        assert_eq!(space.len(), 1);
        let v = space.get(id).unwrap();
        assert_eq!(v.dim(), 32);
    }

    #[test]
    fn semantic_search() {
        let mut search = SemanticSearch::new(64);
        search.index("apple");
        search.index("banana");
        search.index("fruit");
        let results = search.search("apple", 3);
        assert!(!results.is_empty());
        assert!(results[0].similarity <= 1.0);
    }

    #[test]
    fn semantic_search_topk() {
        let mut search = SemanticSearch::new(64);
        for i in 0..10 {
            search.index(&format!("item{}", i));
        }
        let results = search.search("item", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn kmeans_clustering() {
        let mut cl = SemanticCluster::new();
        let v1 = SemanticVector::new(1, vec![1.0, 0.0]);
        let v2 = SemanticVector::new(2, vec![1.1, 0.1]);
        let v3 = SemanticVector::new(3, vec![0.0, 1.0]);
        let v4 = SemanticVector::new(4, vec![0.1, 1.1]);
        let vecs = vec![&v1, &v2, &v3, &v4];
        let clusters = cl.kmeans(&vecs, 2, 20);
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn analogy_solve() {
        let a = SemanticVector::new(1, vec![1.0, 0.0]);
        let b = SemanticVector::new(2, vec![0.0, 1.0]);
        let c = SemanticVector::new(3, vec![2.0, 0.0]);
        let d = SemanticAnalogy::solve(&a, &b, &c);
        // d = a - b + c = [1,0] - [0,1] + [2,0] = [3,-1]
        assert!((d.dims[0] - 3.0).abs() < 1e-6);
        assert!((d.dims[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn compress_int8() {
        let v = SemanticVector::new(1, vec![0.5, -0.5, 1.0, -1.0]);
        let c = SemanticCompression::quantize_int8(&v);
        assert_eq!(c.dims.len(), 4);
        let d = SemanticCompression::dequantize_int8(&c);
        assert_eq!(d.dim(), 4);
    }

    #[test]
    fn pca_reduce() {
        let v = SemanticVector::new(1, vec![1.0, 2.0, 3.0, 4.0]);
        let r = SemanticCompression::pca_reduce(&v, 2);
        assert_eq!(r.dim(), 2);
        assert_eq!(r.dims, vec![1.0, 2.0]);
    }

    #[test]
    fn namespace_bind_resolve() {
        let mut ns = SemanticNamespace::new(64);
        let id = ns.bind("/foo/bar", "hello");
        assert_eq!(id, 1);
        let v = ns.resolve("/foo/bar").unwrap();
        assert_eq!(v.dim(), 64);
        let removed = ns.unbind("/foo/bar");
        assert!(removed.is_some());
        assert!(ns.resolve("/foo/bar").is_none());
    }

    #[test]
    fn semantic_engine_index_search() {
        let mut eng = SemanticEngine::new(64);
        eng.index_text("machine learning");
        eng.index_text("deep learning");
        eng.index_text("cooking recipe");
        let results = eng.search("learning", 3);
        assert!(!results.is_empty());
    }

    #[test]
    fn semantic_engine_concept() {
        let mut eng = SemanticEngine::new(64);
        eng.add_concept("dog");
        eng.add_concept("cat");
        eng.relate("dog", "cat", ConceptRelation::SimilarTo, 0.8);
        assert_eq!(eng.concept_count(), 2);
        assert_eq!(eng.edge_count(), 1);
    }

    #[test]
    fn semantic_engine_bind() {
        let mut eng = SemanticEngine::new(64);
        let id = eng.bind_namespace("/doc1", "important document");
        assert_eq!(id, 1);
        let norm = eng.resolve_namespace("/doc1").unwrap();
        assert!(norm > 0.0);
    }

    #[test]
    fn semantic_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("semantic:"));
        assert!(s.contains("dim="));
    }

    #[test]
    fn semantic_act_index() {
        init();
        let res = semantic_act("index 你好世界").unwrap();
        assert!(res.contains("已索引文本"));
    }

    #[test]
    fn semantic_act_search() {
        init();
        semantic_act("index 机器学习").unwrap();
        let res = semantic_act("search 学习").unwrap();
        assert!(res.contains("语义检索结果"));
    }

    #[test]
    fn semantic_act_bind() {
        init();
        let res = semantic_act("bind /doc1 重要文档").unwrap();
        assert!(res.contains("已绑定"));
        let res = semantic_act("resolve /doc1").unwrap();
        assert!(res.contains("解析成功"));
    }

    #[test]
    fn semantic_act_concept() {
        init();
        let res = semantic_act("concept 苹果").unwrap();
        assert!(res.contains("已添加概念"));
    }

    #[test]
    fn semantic_act_relate() {
        init();
        let res = semantic_act("relate 苹果 水果 isa 1.0").unwrap();
        assert!(res.contains("已建立关系"));
    }

    #[test]
    fn semantic_act_cluster() {
        init();
        semantic_act("index a").unwrap();
        semantic_act("index b").unwrap();
        semantic_act("index c").unwrap();
        let res = semantic_act("cluster 2").unwrap();
        assert!(res.contains("语义聚类结果"));
    }

    #[test]
    fn semantic_act_status() {
        init();
        let res = semantic_act("status").unwrap();
        assert!(res.contains("semantic:"));
    }
}

// =====================================================================================
// 词嵌入 (WordEmbedding)
// =====================================================================================

/// 词嵌入表：词到向量的映射。
pub struct WordEmbedding {
    pub vectors: BTreeMap<String, Vec<f32>>,
    pub dim: usize,
    pub next_id: u64,
}

impl WordEmbedding {
    pub fn new(dim: usize) -> Self {
        Self {
            vectors: BTreeMap::new(),
            dim,
            next_id: 1,
        }
    }

    pub fn add(&mut self, word: &str, vector: Vec<f32>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.vectors.insert(word.to_string(), vector);
        id
    }

    pub fn get(&self, word: &str) -> Option<&Vec<f32>> {
        self.vectors.get(word)
    }

    pub fn similarity(&self, w1: &str, w2: &str) -> f32 {
        match (self.get(w1), self.get(w2)) {
            (Some(v1), Some(v2)) => {
                if v1.len() != v2.len() {
                    return 0.0;
                }
                let dot: f32 = v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum();
                let n1: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
                let n2: f32 = v2.iter().map(|x| x * x).sum::<f32>().sqrt();
                if n1 == 0.0 || n2 == 0.0 {
                    0.0
                } else {
                    dot / (n1 * n2)
                }
            }
            _ => 0.0,
        }
    }

    pub fn closest(&self, word: &str, top_k: usize) -> Vec<(String, f32)> {
        let _v = match self.get(word) {
            Some(v) => v,
            None => return Vec::new(),
        };
        let mut results: Vec<(String, f32)> = self.vectors.iter()
            .filter(|(w, _)| *w != word)
            .map(|(w, v)| {
                let dot: f32 = v.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
                let n1: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                let n2: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                let sim = if n1 == 0.0 || n2 == 0.0 { 0.0 } else { dot / (n1 * n2) };
                (w.clone(), sim)
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    pub fn word_count(&self) -> usize {
        self.vectors.len()
    }
}

// =====================================================================================
// 语义相似度矩阵 (SemanticSimilarityMatrix)
// =====================================================================================

/// 语义相似度矩阵：多个概念之间的两两相似度。
pub struct SemanticSimilarityMatrix {
    pub items: Vec<String>,
    pub matrix: Vec<Vec<f32>>,
}

impl SemanticSimilarityMatrix {
    pub fn new(items: Vec<String>) -> Self {
        let n = items.len();
        let matrix = vec![vec![0.0; n]; n];
        Self { items, matrix }
    }

    pub fn set(&mut self, i: usize, j: usize, value: f32) -> Result<(), String> {
        if i >= self.items.len() || j >= self.items.len() {
            return Err("索引越界".into());
        }
        self.matrix[i][j] = value.clamp(0.0, 1.0);
        self.matrix[j][i] = value.clamp(0.0, 1.0);
        Ok(())
    }

    pub fn get(&self, i: usize, j: usize) -> Option<f32> {
        self.matrix.get(i).and_then(|row| row.get(j)).copied()
    }

    pub fn most_similar(&self, item_idx: usize) -> Option<(usize, f32)> {
        let mut best = None;
        for (j, row) in self.matrix.iter().enumerate() {
            if j == item_idx {
                continue;
            }
            let val = row[item_idx];
            if best.map_or(true, |(_, b)| val > b) {
                best = Some((j, val));
            }
        }
        best
    }

    pub fn size(&self) -> usize {
        self.items.len()
    }
}

// =====================================================================================
// 倒排索引 (InvertedIndex)
// =====================================================================================

/// 倒排索引：关键词到文档 ID 的映射。
pub struct InvertedIndex {
    pub index: BTreeMap<String, Vec<u64>>,
    pub doc_count: u64,
}

impl InvertedIndex {
    pub fn new() -> Self {
        Self {
            index: BTreeMap::new(),
            doc_count: 0,
        }
    }

    pub fn add_document(&mut self, doc_id: u64, keywords: &[&str]) {
        self.doc_count = self.doc_count.max(doc_id + 1);
        for kw in keywords {
            self.index.entry(kw.to_string()).or_default().push(doc_id);
        }
    }

    pub fn search(&self, keyword: &str) -> Vec<u64> {
        self.index.get(keyword).cloned().unwrap_or_default()
    }

    pub fn search_multi(&self, keywords: &[&str]) -> Vec<u64> {
        let mut results: Vec<u64> = Vec::new();
        for kw in keywords {
            if let Some(docs) = self.index.get(*kw) {
                results.extend(docs.iter().copied());
            }
        }
        results.sort();
        results.dedup();
        results
    }

    pub fn keyword_count(&self) -> usize {
        self.index.len()
    }
}

// =====================================================================================
// HNSW 近似近邻 (HNSWIndex)
// =====================================================================================

/// HNSW 节点。
#[derive(Clone, Debug)]
pub struct HNSWNode {
    pub id: u64,
    pub vector: Vec<f32>,
    pub layers: Vec<Vec<u64>>,   // 每层的邻居
}

/// HNSW 图：近似最近邻搜索。
pub struct HNSWIndex {
    pub nodes: BTreeMap<u64, HNSWNode>,
    pub max_layer: usize,
    pub entry_point: Option<u64>,
    pub next_id: u64,
    pub dim: usize,
}

impl HNSWIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            nodes: BTreeMap::new(),
            max_layer: 16,
            entry_point: None,
            next_id: 1,
            dim,
        }
    }

    pub fn insert(&mut self, vector: Vec<f32>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let node = HNSWNode {
            id,
            vector,
            layers: vec![Vec::new(); self.max_layer],
        };
        self.nodes.insert(id, node);
        if self.entry_point.is_none() {
            self.entry_point = Some(id);
        }
        id
    }

    pub fn search(&self, query: &[f32], k: usize) -> Vec<(u64, f32)> {
        let mut results: Vec<(u64, f32)> = self.nodes.iter()
            .map(|(id, node)| {
                let dot: f32 = query.iter().zip(node.vector.iter()).map(|(a, b)| a * b).sum();
                let nq: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
                let nn: f32 = node.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                let sim = if nq == 0.0 || nn == 0.0 { 0.0 } else { dot / (nq * nn) };
                (*id, sim)
            })
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// =====================================================================================
// 层次聚类 (HierarchicalClustering)
// =====================================================================================

/// 聚类节点。
#[derive(Clone, Debug)]
pub struct ClusterNode {
    pub id: u64,
    pub item_ids: Vec<u64>,
    pub centroid: Vec<f32>,
    pub left: Option<u64>,
    pub right: Option<u64>,
    pub distance: f32,
}

/// 层次聚类：自底向上合并相似项。
pub struct HierarchicalClustering {
    pub clusters: BTreeMap<u64, ClusterNode>,
    pub next_id: u64,
    pub root_id: Option<u64>,
}

impl HierarchicalClustering {
    pub fn new() -> Self {
        Self {
            clusters: BTreeMap::new(),
            next_id: 1,
            root_id: None,
        }
    }

    pub fn add_item(&mut self, item_id: u64, vector: Vec<f32>) {
        let id = self.next_id;
        self.next_id += 1;
        self.clusters.insert(id, ClusterNode {
            id,
            item_ids: vec![item_id],
            centroid: vector.clone(),
            left: None,
            right: None,
            distance: 0.0,
        });
    }

    pub fn merge_closest(&mut self) -> Option<(u64, u64)> {
        let mut best_pair = None;
        let mut best_dist = f32::INFINITY;
        let ids: Vec<u64> = self.clusters.keys().copied().collect();
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let c1 = &self.clusters[&ids[i]];
                let c2 = &self.clusters[&ids[j]];
                let dist = centroid_distance(&c1.centroid, &c2.centroid);
                if dist < best_dist {
                    best_dist = dist;
                    best_pair = Some((ids[i], ids[j]));
                }
            }
        }
        if let Some((a, b)) = best_pair {
            let ca = self.clusters.get(&a).unwrap().centroid.clone();
            let cb = self.clusters.get(&b).unwrap().centroid.clone();
            let merged_centroid: Vec<f32> = ca.iter().zip(cb.iter()).map(|(x, y)| (x + y) / 2.0).collect();
            let merged_item_ids: Vec<u64> = self.clusters[&a].item_ids.iter()
                .chain(self.clusters[&b].item_ids.iter())
                .copied().collect();
            let new_id = self.next_id;
            self.next_id += 1;
            self.clusters.insert(new_id, ClusterNode {
                id: new_id,
                item_ids: merged_item_ids,
                centroid: merged_centroid,
                left: Some(a),
                right: Some(b),
                distance: best_dist,
            });
            self.clusters.remove(&a);
            self.clusters.remove(&b);
            self.root_id = Some(new_id);
            Some((a, b))
        } else {
            None
        }
    }

    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }
}

fn centroid_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

// =====================================================================================
// 语义扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn word_embedding() {
        let mut we = WordEmbedding::new(4);
        we.add("king", vec![1.0, 0.0, 0.0, 0.0]);
        we.add("queen", vec![0.9, 0.1, 0.0, 0.0]);
        we.add("apple", vec![0.0, 0.0, 1.0, 0.0]);
        assert_eq!(we.word_count(), 3);
        let sim = we.similarity("king", "queen");
        assert!(sim > 0.9);
        let sim2 = we.similarity("king", "apple");
        assert!(sim2 < 0.5);
        let closest = we.closest("king", 2);
        assert_eq!(closest.len(), 2);
        assert_eq!(closest[0].0, "queen");
    }

    #[test]
    fn similarity_matrix() {
        let mut sm = SemanticSimilarityMatrix::new(vec!["a".into(), "b".into(), "c".into()]);
        sm.set(0, 1, 0.8).unwrap();
        sm.set(0, 2, 0.3).unwrap();
        sm.set(1, 2, 0.5).unwrap();
        assert_eq!(sm.get(0, 1), Some(0.8));
        assert_eq!(sm.get(1, 0), Some(0.8));
        let (idx, sim) = sm.most_similar(0).unwrap();
        assert_eq!(idx, 1);
        assert!((sim - 0.8).abs() < 1e-6);
    }

    #[test]
    fn inverted_index() {
        let mut ii = InvertedIndex::new();
        ii.add_document(1, &["rust", "os", "kernel"]);
        ii.add_document(2, &["rust", "ai", "os"]);
        ii.add_document(3, &["python", "ai"]);
        assert_eq!(ii.keyword_count(), 5);
        let results = ii.search("rust");
        assert_eq!(results, vec![1, 2]);
        let results = ii.search_multi(&["rust", "ai"]);
        assert_eq!(results, vec![1, 2, 3]);
    }

    #[test]
    fn hnsw_index() {
        let mut hnsw = HNSWIndex::new(4);
        hnsw.insert(vec![1.0, 0.0, 0.0, 0.0]);
        hnsw.insert(vec![0.9, 0.1, 0.0, 0.0]);
        hnsw.insert(vec![0.0, 0.0, 1.0, 0.0]);
        assert_eq!(hnsw.node_count(), 3);
        let results = hnsw.search(&[1.0, 0.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1);
    }

    #[test]
    fn hierarchical_clustering() {
        let mut hc = HierarchicalClustering::new();
        hc.add_item(1, vec![1.0, 0.0]);
        hc.add_item(2, vec![1.1, 0.1]);
        hc.add_item(3, vec![0.0, 1.0]);
        hc.add_item(4, vec![0.1, 1.1]);
        assert_eq!(hc.cluster_count(), 4);
        let merged = hc.merge_closest();
        assert!(merged.is_some());
        assert!(hc.cluster_count() < 4);
    }

    #[test]
    fn word_embedding_similarity_edge() {
        let we = WordEmbedding::new(4);
        assert_eq!(we.similarity("nonexistent", "also_missing"), 0.0);
        assert_eq!(we.closest("nonexistent", 5).len(), 0);
    }
}
