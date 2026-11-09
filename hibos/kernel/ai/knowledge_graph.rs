//! 知识图谱 —— 三元组存储与图查询。
//!
//! `KnowledgeGraph` 提供 `add_triple` / `remove_triple` / `query` /
//! `query_by_subject` / `query_by_object` / `traverse` / `shortest_path`。
//! 支持模式匹配、递归遍历、最短路径（BFS）与简化社区发现。
//! 与 `kernel/ai_token.rs` 的 `AIConsciousness` 概念图互补：这里侧重
//! 实例级三元组事实，而非概念本体。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 三元组
// ============================================================================

/// 知识三元组：(主语, 谓语, 宾语)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f64, // 0.0-1.0
    pub source: String,
    pub timestamp: u64, // ns
}

impl Triple {
    pub fn new(
        subject: &str,
        predicate: &str,
        object: &str,
        confidence: f64,
        source: &str,
    ) -> Self {
        Self {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            source: source.to_string(),
            timestamp: crate::monitor::get_time_ns(),
        }
    }

    pub fn key(&self) -> (String, String, String) {
        (self.subject.clone(), self.predicate.clone(), self.object.clone())
    }
}

// ============================================================================
// 节点与边
// ============================================================================

/// 图节点。
#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub attributes: BTreeMap<String, String>,
    pub node_type: String,
}

impl Node {
    pub fn new(id: &str, label: &str, node_type: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            attributes: BTreeMap::new(),
            node_type: node_type.to_string(),
        }
    }

    pub fn set_attr(&mut self, key: &str, value: &str) {
        self.attributes.insert(key.to_string(), value.to_string());
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).map(|s| s.as_str())
    }
}

/// 图边。
#[derive(Clone, Debug)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub weight: f64,
}

impl Edge {
    pub fn new(source: &str, target: &str, relation: &str, weight: f64) -> Self {
        Self {
            source: source.to_string(),
            target: target.to_string(),
            relation: relation.to_string(),
            weight: weight.clamp(0.0, 1.0),
        }
    }
}

// ============================================================================
// 知识图谱
// ============================================================================

/// 知识图谱：三元组索引（按 subject / predicate / object 三向索引）。
pub struct KnowledgeGraph {
    /// 三元组存储，按 key (s,p,o) 去重。
    triples: BTreeMap<(String, String, String), Triple>,
    /// subject ->  triple keys。
    by_subject: HashMap<String, BTreeSet<(String, String, String)>>,
    /// predicate -> triple keys。
    by_predicate: HashMap<String, BTreeSet<(String, String, String)>>,
    /// object -> triple keys。
    by_object: HashMap<String, BTreeSet<(String, String, String)>>,
    /// 节点注册表。
    nodes: BTreeMap<String, Node>,
    /// 边注册表。
    edges: BTreeMap<(String, String, String), Edge>,
    /// 三元组总数计数。
    count: u64,
}

impl Default for KnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            triples: BTreeMap::new(),
            by_subject: HashMap::new(),
            by_predicate: HashMap::new(),
            by_object: HashMap::new(),
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            count: 0,
        }
    }

    /// 添加三元组。若已存在相同 (s,p,o) 则更新置信度与时间戳。
    pub fn add_triple(&mut self, triple: Triple) {
        let key = triple.key();
        let existed = self.triples.contains_key(&key);
        self.triples.insert(key.clone(), triple.clone());
        self.by_subject
            .entry(triple.subject.clone())
            .or_default()
            .insert(key.clone());
        self.by_predicate
            .entry(triple.predicate.clone())
            .or_default()
            .insert(key.clone());
        self.by_object
            .entry(triple.object.clone())
            .or_default()
            .insert(key.clone());
        // 维护节点与边。
        self.nodes
            .entry(triple.subject.clone())
            .or_insert_with(|| Node::new(&triple.subject, &triple.subject, "entity"));
        self.nodes
            .entry(triple.object.clone())
            .or_insert_with(|| Node::new(&triple.object, &triple.object, "entity"));
        self.edges.insert(
            (triple.subject.clone(), triple.object.clone(), triple.predicate.clone()),
            Edge::new(&triple.subject, &triple.object, &triple.predicate, triple.confidence),
        );
        if !existed {
            self.count += 1;
        }
    }

    /// 便捷添加三元组（字符串参数）。
    pub fn add_triple_simple(
        &mut self,
        subject: &str,
        predicate: &str,
        object: &str,
        confidence: f64,
        source: &str,
    ) {
        self.add_triple(Triple::new(subject, predicate, object, confidence, source));
    }

    /// 移除三元组。
    pub fn remove_triple(&mut self, s: &str, p: &str, o: &str) -> bool {
        let key = (s.to_string(), p.to_string(), o.to_string());
        if self.triples.remove(&key).is_some() {
            self.by_subject.get_mut(s).and_then(|set| set.remove(&key));
            self.by_predicate.get_mut(p).and_then(|set| set.remove(&key));
            self.by_object.get_mut(o).and_then(|set| set.remove(&key));
            self.count = self.count.saturating_sub(1);
            true
        } else {
            false
        }
    }

    /// 查询三元组（字段可为空表示通配符）。
    pub fn query(&self, s: Option<&str>, p: Option<&str>, o: Option<&str>) -> Vec<Triple> {
        // 选择最小的候选集作为起点。
        let candidates: Vec<&Triple> = match (s, p, o) {
            (Some(s), None, None) => self
                .by_subject
                .get(s)
                .map(|set| set.iter().filter_map(|k| self.triples.get(k)).collect())
                .unwrap_or_default(),
            (None, Some(p), None) => self
                .by_predicate
                .get(p)
                .map(|set| set.iter().filter_map(|k| self.triples.get(k)).collect())
                .unwrap_or_default(),
            (None, None, Some(o)) => self
                .by_object
                .get(o)
                .map(|set| set.iter().filter_map(|k| self.triples.get(k)).collect())
                .unwrap_or_default(),
            (Some(s), Some(p), None) => {
                let subj_set = self.by_subject.get(s);
                let pred_set = self.by_predicate.get(p);
                match (subj_set, pred_set) {
                    (Some(a), Some(b)) => a
                        .intersection(b)
                        .filter_map(|k| self.triples.get(k))
                        .collect(),
                    _ => Vec::new(),
                }
            }
            (Some(s), None, Some(o)) => {
                let subj_set = self.by_subject.get(s);
                let obj_set = self.by_object.get(o);
                match (subj_set, obj_set) {
                    (Some(a), Some(b)) => a
                        .intersection(b)
                        .filter_map(|k| self.triples.get(k))
                        .collect(),
                    _ => Vec::new(),
                }
            }
            (None, Some(p), Some(o)) => {
                let pred_set = self.by_predicate.get(p);
                let obj_set = self.by_object.get(o);
                match (pred_set, obj_set) {
                    (Some(a), Some(b)) => a
                        .intersection(b)
                        .filter_map(|k| self.triples.get(k))
                        .collect(),
                    _ => Vec::new(),
                }
            }
            (Some(s), Some(p), Some(o)) => {
                let key = (s.to_string(), p.to_string(), o.to_string());
                self.triples
                    .get(&key)
                    .map(|t| vec![t])
                    .unwrap_or_default()
            }
            (None, None, None) => self.triples.values().collect(),
        };
        candidates.into_iter().cloned().collect()
    }

    /// 按主语查询。
    pub fn query_by_subject(&self, s: &str) -> Vec<Triple> {
        self.query(Some(s), None, None)
    }

    /// 按宾语查询。
    pub fn query_by_object(&self, o: &str) -> Vec<Triple> {
        self.query(None, None, Some(o))
    }

    /// 按谓语查询。
    pub fn query_by_predicate(&self, p: &str) -> Vec<Triple> {
        self.query(None, Some(p), None)
    }

    /// 返回三元组总数。
    pub fn triple_count(&self) -> u64 {
        self.count
    }

    /// 返回节点数。
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 获取节点。
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// 设置节点属性。
    pub fn set_node_attr(&mut self, id: &str, key: &str, value: &str) {
        if let Some(n) = self.nodes.get_mut(id) {
            n.set_attr(key, value);
        }
    }

    /// 递归遍历：从根节点出发，沿 direction 方向扩展 depth 层。
    /// direction: "out" (subject->object) / "in" (object->subject) / "both"。
    pub fn traverse(&self, root: &str, depth: usize, direction: &str) -> Vec<Node> {
        let mut visited = BTreeSet::new();
        let mut result = Vec::new();
        let mut frontier = vec![root.to_string()];
        visited.insert(root.to_string());
        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                match direction {
                    "out" => {
                        for t in self.query_by_subject(node_id) {
                            if visited.insert(t.object.clone()) {
                                result.push(self.node_or_stub(&t.object));
                                next_frontier.push(t.object);
                            }
                        }
                    }
                    "in" => {
                        for t in self.query_by_object(node_id) {
                            if visited.insert(t.subject.clone()) {
                                result.push(self.node_or_stub(&t.subject));
                                next_frontier.push(t.subject);
                            }
                        }
                    }
                    _ => {
                        for t in self.query_by_subject(node_id) {
                            if visited.insert(t.object.clone()) {
                                result.push(self.node_or_stub(&t.object));
                                next_frontier.push(t.object);
                            }
                        }
                        for t in self.query_by_object(node_id) {
                            if visited.insert(t.subject.clone()) {
                                result.push(self.node_or_stub(&t.subject));
                                next_frontier.push(t.subject);
                            }
                        }
                    }
                }
            }
            frontier = next_frontier;
            if frontier.is_empty() {
                break;
            }
        }
        result
    }

    fn node_or_stub(&self, id: &str) -> Node {
        self.nodes
            .get(id)
            .cloned()
            .unwrap_or_else(|| Node::new(id, id, "unknown"))
    }

    /// 最短路径（BFS），direction 同 traverse。
    pub fn shortest_path(&self, src: &str, dst: &str) -> Option<Vec<Edge>> {
        if src == dst {
            return Some(Vec::new());
        }
        let mut visited = BTreeSet::new();
        visited.insert(src.to_string());
        // 队列元素：(当前节点，路径边列表)。
        let mut queue: VecDeque<(String, Vec<Edge>)> = VecDeque::new();
        queue.push_back((src.to_string(), Vec::new()));
        while let Some((current, path)) = queue.pop_front() {
            // 找出 current 作为 subject 的边。
            for t in self.query_by_subject(&current) {
                if t.object == dst {
                    let mut p = path.clone();
                    p.push(Edge::new(&current, &t.object, &t.predicate, t.confidence));
                    return Some(p);
                }
                if visited.insert(t.object.clone()) {
                    let mut p = path.clone();
                    p.push(Edge::new(&current, &t.object, &t.predicate, t.confidence));
                    queue.push_back((t.object, p));
                }
            }
            // 找出 current 作为 object 的边（反向）。
            for t in self.query_by_object(&current) {
                if t.subject == dst {
                    let mut p = path.clone();
                    p.push(Edge::new(&t.subject, &current, &t.predicate, t.confidence));
                    return Some(p);
                }
                if visited.insert(t.subject.clone()) {
                    let mut p = path.clone();
                    p.push(Edge::new(&t.subject, &current, &t.predicate, t.confidence));
                    queue.push_back((t.subject, p));
                }
            }
        }
        None
    }

    /// 简化社区发现：基于连通分量的并查集。
    pub fn communities(&self) -> Vec<Vec<String>> {
        let mut parent: HashMap<String, String> = HashMap::new();
        for (s, _p, o) in self.triples.keys() {
            parent.entry(s.clone()).or_insert_with(|| s.clone());
            parent.entry(o.clone()).or_insert_with(|| o.clone());
            self.union(&mut parent, s, o);
        }
        let nodes: Vec<String> = parent.keys().cloned().collect();
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for node in nodes {
            let root = self.find(&mut parent, &node);
            groups.entry(root).or_default().push(node);
        }
        let mut result: Vec<Vec<String>> = groups.into_values().collect();
        result.sort_by(|a, b| b.len().cmp(&a.len()));
        result
    }

    fn find(&self, parent: &mut HashMap<String, String>, x: &str) -> String {
        if parent.get(x) != Some(&x.to_string()) {
            let p = parent.get(x).unwrap().clone();
            let root = self.find(parent, &p);
            parent.insert(x.to_string(), root.clone());
            root
        } else {
            x.to_string()
        }
    }

    fn union(&self, parent: &mut HashMap<String, String>, x: &str, y: &str) {
        let rx = self.find(parent, x);
        let ry = self.find(parent, y);
        if rx != ry {
            parent.insert(rx, ry);
        }
    }

    /// 返回全部三元组。
    pub fn all_triples(&self) -> Vec<Triple> {
        self.triples.values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }
}

// ============================================================================
// 全局 KnowledgeGraph 单例（供 kernel/sense.rs 调用）
// ============================================================================

static KNOWLEDGE_GRAPH: OnceLock<Mutex<KnowledgeGraph>> = OnceLock::new();

/// 获取全局知识图谱。
pub fn knowledge_graph() -> &'static Mutex<KnowledgeGraph> {
    KNOWLEDGE_GRAPH.get_or_init(|| Mutex::new(KnowledgeGraph::new()))
}

/// `sense ai://kgraph`：返回知识图谱状态摘要。
pub fn kgraph_sense() -> String {
    let g = knowledge_graph().lock().unwrap();
    let comms = g.communities();
    format!(
        "triples={} nodes={} communities={} largest_community={}",
        g.triple_count(),
        g.node_count(),
        comms.len(),
        comms.first().map(|c| c.len()).unwrap_or(0)
    )
}

/// `act ai://kgraph <op> ...`：对知识图谱执行行动。
pub fn kgraph_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let g = knowledge_graph();
    let mut g = g.lock().unwrap();
    match it.next().unwrap_or("") {
        "add" => {
            let s = it.next().ok_or("缺少 subject")?;
            let p = it.next().ok_or("缺少 predicate")?;
            let o = it.next().ok_or("缺少 object")?;
            let conf: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let src = it.next().unwrap_or("cli");
            g.add_triple_simple(s, p, o, conf, src);
            Ok(format!("已添加三元组 ({}, {}, {})", s, p, o))
        }
        "remove" => {
            let s = it.next().ok_or("缺少 subject")?;
            let p = it.next().ok_or("缺少 predicate")?;
            let o = it.next().ok_or("缺少 object")?;
            if g.remove_triple(s, p, o) {
                Ok(format!("已移除三元组 ({}, {}, {})", s, p, o))
            } else {
                Err("三元组不存在".to_string())
            }
        }
        "query" => {
            let s = it.next().unwrap_or("");
            let p = it.next().unwrap_or("");
            let o = it.next().unwrap_or("");
            let results = g.query(
                if s.is_empty() { None } else { Some(s) },
                if p.is_empty() { None } else { Some(p) },
                if o.is_empty() { None } else { Some(o) },
            );
            let count = results.len();
            let mut out = String::new();
            for t in &results {
                out.push_str(&format!("({},{},{}) confidence={:.2}\n", t.subject, t.predicate, t.object, t.confidence));
            }
            Ok(format!("查询到 {} 条\n{}", count, out))
        }
        "traverse" => {
            let root = it.next().ok_or("缺少 root")?;
            let depth: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(2);
            let direction = it.next().unwrap_or("both");
            let nodes = g.traverse(root, depth, direction);
            Ok(format!(
                "从 {} 遍历 {} 层 ({}) 得到 {} 个节点",
                root, depth, direction, nodes.len()
            ))
        }
        "shortest_path" => {
            let src = it.next().ok_or("缺少 src")?;
            let dst = it.next().ok_or("缺少 dst")?;
            match g.shortest_path(src, dst) {
                Some(path) => {
                    let desc: Vec<String> = path
                        .iter()
                        .map(|e| format!("{} --{}--> {}", e.source, e.relation, e.target))
                        .collect();
                    Ok(format!("路径长度 {}：{}", path.len(), desc.join(" -> ")))
                }
                None => Ok(format!("{} 到 {} 无路径", src, dst)),
            }
        }
        "communities" => {
            let comms = g.communities();
            let desc: Vec<String> = comms
                .iter()
                .enumerate()
                .map(|(i, c)| format!("社区{}: {} 节点", i, c.len()))
                .collect();
            Ok(format!("社区数 {}：{}", comms.len(), desc.join("; ")))
        }
        "list" => Ok(kgraph_sense()),
        _ => Err(
            "用法: kgraph add|remove|query|traverse|shortest_path|communities|list".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_key() {
        let t = Triple::new("a", "knows", "b", 0.9, "test");
        assert_eq!(t.key(), ("a".to_string(), "knows".to_string(), "b".to_string()));
        assert!((t.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn add_and_query() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("Alice", "knows", "Bob", 0.9, "test");
        g.add_triple_simple("Bob", "knows", "Carol", 0.8, "test");
        g.add_triple_simple("Alice", "likes", "Pizza", 1.0, "test");

        assert_eq!(g.triple_count(), 3);
        let by_subj = g.query_by_subject("Alice");
        assert_eq!(by_subj.len(), 2);
        let by_obj = g.query_by_object("Bob");
        assert_eq!(by_obj.len(), 1);
        let both = g.query(Some("Alice"), Some("knows"), None);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].object, "Bob");
    }

    #[test]
    fn remove_triple() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("a", "p", "b", 1.0, "t");
        assert!(g.remove_triple("a", "p", "b"));
        assert!(!g.remove_triple("a", "p", "b"));
        assert_eq!(g.triple_count(), 0);
    }

    #[test]
    fn traverse() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("A", "knows", "B", 1.0, "t");
        g.add_triple_simple("B", "knows", "C", 1.0, "t");
        g.add_triple_simple("C", "knows", "D", 1.0, "t");
        let nodes = g.traverse("A", 2, "out");
        assert!(nodes.len() >= 2); // B, C
    }

    #[test]
    fn shortest_path() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("A", "knows", "B", 1.0, "t");
        g.add_triple_simple("B", "knows", "C", 1.0, "t");
        g.add_triple_simple("C", "knows", "D", 1.0, "t");
        let path = g.shortest_path("A", "D");
        assert!(path.is_some());
        assert_eq!(path.unwrap().len(), 3);
        assert!(g.shortest_path("A", "Z").is_none());
        assert_eq!(g.shortest_path("A", "A").unwrap().len(), 0);
    }

    #[test]
    fn communities() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("A", "knows", "B", 1.0, "t");
        g.add_triple_simple("B", "knows", "C", 1.0, "t");
        g.add_triple_simple("X", "knows", "Y", 1.0, "t");
        let comms = g.communities();
        assert_eq!(comms.len(), 2);
        assert_eq!(comms[0].len(), 3); // 更大的社区排在前。
    }

    #[test]
    fn node_attrs() {
        let mut g = KnowledgeGraph::new();
        g.add_triple_simple("Server01", "has", "IP", 1.0, "t");
        g.set_node_attr("Server01", "room", "A1");
        assert_eq!(g.node("Server01").unwrap().attr("room"), Some("A1"));
    }

    #[test]
    fn kgraph_sense_and_act() {
        let mut g = knowledge_graph().lock().unwrap();
        g.add_triple_simple("a", "knows", "b", 1.0, "t");
        drop(g);
        let sense = kgraph_sense();
        assert!(sense.contains("triples="));
        let res = kgraph_act("add x knows y 0.5 test").unwrap();
        assert!(res.contains("已添加"));
        let res = kgraph_act("query x knows y").unwrap();
        assert!(res.contains("x"));
        let res = kgraph_act("shortest_path a y").unwrap();
        assert!(res.contains("路径") || res.contains("无路径"));
        let res = kgraph_act("communities").unwrap();
        assert!(res.contains("社区"));
    }
}
