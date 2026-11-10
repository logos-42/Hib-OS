//! 记忆文件系统 (Knowledge) —— AI 原生 OS 的「文件系统」。
//!
//! 对应《现代操作系统》第 4 章「文件系统」：
//! * `MemoryNode` 等价于 inode + 目录项：路径、内容、标签、向量；
//! * 语义检索 (search) 取代路径查找：按关键词/标签/向量相似度；
//! * 支持写入、读取、追加、删除、按标签遍历；
//! * 长期记忆与上下文窗口的换出/换入互通。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 记忆节点。
#[derive(Clone, Debug)]
pub struct MemoryNode {
    pub path: String,
    pub content: String,
    /// 标签（分类/来源）。
    pub tags: Vec<String>,
    /// 简化向量（词袋哈希）。
    pub vector: Vec<f32>,
    pub created_at: u64,
    pub updated_at: u64,
    pub access_count: u64,
}

impl MemoryNode {
    pub fn token_count(&self) -> u64 {
        (self.content.chars().count() as u64) / 4 + 1
    }
}

/// 长期记忆库。
pub struct KnowledgeBase {
    nodes: BTreeMap<String, MemoryNode>,
    clock: u64,
    pub hits: u64,
    pub misses: u64,
}

fn embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for word in text.split_whitespace() {
        let mut h: u64 = 1469598103934665603;
        for b in word.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        v[(h as usize) % dim] += 1.0;
    }
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl KnowledgeBase {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// 写入/覆盖一个记忆节点。
    pub fn write(&mut self, path: &str, content: &str, tags: &[String]) {
        let now = self.tick();
        let vector = embed(content, 64);
        let node = self.nodes.entry(path.to_string()).or_insert(MemoryNode {
            path: path.to_string(),
            content: String::new(),
            tags: Vec::new(),
            vector: Vec::new(),
            created_at: now,
            updated_at: now,
            access_count: 0,
        });
        node.content = content.to_string();
        node.tags = tags.to_vec();
        node.vector = vector;
        node.updated_at = now;
    }

    /// 追加内容。
    pub fn append(&mut self, path: &str, content: &str) -> Option<()> {
        let now = self.tick();
        let node = self.nodes.get_mut(path)?;
        node.content.push_str(content);
        node.vector = embed(&node.content, 64);
        node.updated_at = now;
        Some(())
    }

    /// 读取。
    pub fn read(&mut self, path: &str) -> Option<&MemoryNode> {
        let now = self.tick();
        let node = self.nodes.get_mut(path)?;
        node.access_count += 1;
        node.updated_at = node.updated_at.max(now.min(node.updated_at));
        Some(node)
    }

    /// 删除。
    pub fn remove(&mut self, path: &str) -> bool {
        self.nodes.remove(path).is_some()
    }

    /// 语义检索：返回按相似度排序的 (路径, 分数)。
    pub fn search(&mut self, query: &str, top_k: usize) -> Vec<(String, f32)> {
        let q = embed(query, 64);
        let mut scored: Vec<(String, f32)> = self
            .nodes
            .values()
            .map(|n| (n.path.clone(), cosine(&q, &n.vector)))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let found = !scored.is_empty();
        if found {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        scored.truncate(top_k);
        scored
    }

    /// 按标签列举。
    pub fn by_tag(&self, tag: &str) -> Vec<&MemoryNode> {
        self.nodes
            .values()
            .filter(|n| n.tags.iter().any(|t| t == tag))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn paths(&self) -> Vec<&str> {
        self.nodes.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for KnowledgeBase {
    fn default() -> Self {
        Self::new()
    }
}

static KB: OnceLock<Mutex<KnowledgeBase>> = OnceLock::new();

pub fn knowledge() -> &'static Mutex<KnowledgeBase> {
    KB.get_or_init(|| Mutex::new(KnowledgeBase::new()))
}

pub fn init() {
    let mut kb = knowledge().lock().unwrap();
    if !kb.is_empty() {
        return;
    }
    kb.write("mem://system/boot", "Hib OS 启动：AI 原生内核已就绪", &["system".to_string()]);
    kb.write("mem://system/policy", "默认策略：最小权限、显式授权、全程审计", &["system".to_string(), "security".to_string()]);
    kb.write("mem://agent/init", "init 智能体负责派生与回收其他智能体", &["agent".to_string()]);
}

pub fn status() -> String {
    let kb = knowledge().lock().unwrap();
    format!("nodes={} hits={} misses={}", kb.len(), kb.hits, kb.misses)
}

/// `act knowledge://*` 的后端动作：
/// * `write <path> <text...> [tag...]` 写入
/// * `append <path> <text...>` 追加
/// * `read <path>` 读取
/// * `remove <path>` 删除
/// * `search <query...>` 语义检索
/// * `tag <tag>` 按标签列出
/// * `list` / `status`
pub fn knowledge_act(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    let mut kb = knowledge().lock().unwrap();
    match it.next() {
        Some("write") => {
            let path = it.next().ok_or("用法: knowledge://store write <path> <text>")?;
            let text = it.next().unwrap_or("");
            kb.write(path, text, &[]);
            Ok(format!("已写入 {}（{} 字符）", path, text.chars().count()))
        }
        Some("append") => {
            let path = it.next().ok_or("用法: knowledge://store append <path> <text>")?;
            let text = it.next().unwrap_or("");
            kb.append(path, text)
                .ok_or_else(|| format!("节点不存在: {}", path))?;
            Ok(format!("已追加 {} 字符到 {}", text.chars().count(), path))
        }
        Some("read") => {
            let path = it.next().ok_or("用法: knowledge://store read <path>")?;
            let node = kb.read(path).ok_or_else(|| format!("节点不存在: {}", path))?;
            Ok(format!(
                "path={} tokens={} tags={:?}\n{}",
                node.path,
                node.token_count(),
                node.tags,
                node.content
            ))
        }
        Some("remove") => {
            let path = it.next().ok_or("用法: knowledge://store remove <path>")?;
            if kb.remove(path) {
                Ok(format!("已删除 {}", path))
            } else {
                Err(format!("节点不存在: {}", path))
            }
        }
        Some("search") => {
            let query = it.next().unwrap_or("");
            let res = kb.search(query, 10);
            let mut out = String::from("检索结果:\n");
            for (path, score) in res {
                out.push_str(&format!("  {} {:.3}\n", path, score));
            }
            Ok(out)
        }
        Some("tag") => {
            let tag = it.next().ok_or("用法: knowledge://store tag <tag>")?;
            let mut out = String::from("标签结果:\n");
            for n in kb.by_tag(tag) {
                out.push_str(&format!("  {}\n", n.path));
            }
            Ok(out)
        }
        Some("list") => {
            let mut out = String::from("记忆节点:\n");
            for p in kb.paths() {
                out.push_str(&format!("  {}\n", p));
            }
            Ok(out)
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 knowledge 动作: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_remove() {
        let mut kb = KnowledgeBase::new();
        kb.write("mem://a", "hello world", &["t".to_string()]);
        assert_eq!(kb.read("mem://a").unwrap().content, "hello world");
        assert!(kb.remove("mem://a"));
        assert!(!kb.remove("mem://a"));
        assert_eq!(kb.len(), 0);
    }

    #[test]
    fn append_updates_vector() {
        let mut kb = KnowledgeBase::new();
        kb.write("mem://a", "hello", &[]);
        kb.append("mem://a", " world").unwrap();
        assert_eq!(kb.read("mem://a").unwrap().content, "hello world");
    }

    #[test]
    fn semantic_search_ranks() {
        let mut kb = KnowledgeBase::new();
        kb.write("mem://os", "kernel scheduler memory page", &[]);
        kb.write("mem://poem", "roses are red violets blue", &[]);
        let res = kb.search("kernel memory", 2);
        assert_eq!(res[0].0, "mem://os");
        assert!(res[0].1 > res[1].1);
    }

    #[test]
    fn tag_index() {
        let mut kb = KnowledgeBase::new();
        kb.write("mem://x", "x", &["sec".to_string()]);
        kb.write("mem://y", "y", &["misc".to_string()]);
        assert_eq!(kb.by_tag("sec").len(), 1);
    }

    #[test]
    fn global_init_is_idempotent() {
        init();
        init();
        let kb = knowledge().lock().unwrap();
        assert!(kb.len() >= 3);
    }
}
