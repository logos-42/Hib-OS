//! 上下文内存子系统 —— AI 原生 OS 的「虚拟地址空间」。
//!
//! 对应《现代操作系统》第 3 章「存储管理」：
//! * 上下文窗口等价于地址空间；
//! * 语义页 (SemanticPage) 等价于页框，存放 token 段与向量；
//! * KV 缓存占用等价于物理内存；
//! * 窗口溢出触发「页面置换」：按 LRU 换出到长期记忆（knowledge）；
//! * 缺失语义页触发「缺页中断」：从长期记忆换入。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 语义页 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(pub u64);

/// 一页语义数据。
#[derive(Clone, Debug)]
pub struct SemanticPage {
    pub id: PageId,
    /// 该页对应的 token 段文本。
    pub text: String,
    /// 粗略 token 数。
    pub tokens: u64,
    /// 最近访问时间（逻辑时钟）。
    pub last_access: u64,
    /// 是否脏（需要写回长期记忆）。
    pub dirty: bool,
    /// 是否常驻（不可换出）。
    pub pinned: bool,
}

/// 一个上下文窗口（地址空间）。
pub struct CtxWindow {
    pub id: u64,
    pub owner: u64,
    /// 容量（token）。
    pub capacity: u64,
    /// 已用 token。
    pub used: u64,
    pages: BTreeMap<PageId, SemanticPage>,
    next_page: u64,
    pub faults: u64,
    pub evictions: u64,
    clock: u64,
}

/// 上下文内存错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CtxError {
    WindowNotFound { id: u64 },
    PageNotFound { page: PageId },
    OutOfCapacity { need: u64, free: u64 },
    Pinned { page: PageId },
}

impl CtxWindow {
    pub fn new(id: u64, owner: u64, capacity: u64) -> Self {
        Self {
            id,
            owner,
            capacity,
            used: 0,
            pages: BTreeMap::new(),
            next_page: 1,
            faults: 0,
            evictions: 0,
            clock: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// 分配一个新语义页并写入。
    pub fn alloc_page(&mut self, text: &str, tokens: u64) -> Result<PageId, CtxError> {
        if self.used + tokens > self.capacity {
            return Err(CtxError::OutOfCapacity {
                need: tokens,
                free: self.capacity.saturating_sub(self.used),
            });
        }
        let id = PageId(self.next_page);
        self.next_page += 1;
        let now = self.tick();
        self.pages.insert(
            id,
            SemanticPage {
                id,
                text: text.to_string(),
                tokens,
                last_access: now,
                dirty: true,
                pinned: false,
            },
        );
        self.used += tokens;
        Ok(id)
    }

    /// 访问一页（更新 LRU）。页不在则记一次缺页。
    pub fn access(&mut self, id: PageId) -> Result<&SemanticPage, CtxError> {
        let now = self.tick();
        let p = self.pages.get_mut(&id).ok_or(CtxError::PageNotFound { page: id })?;
        p.last_access = now;
        Ok(p)
    }

    /// 换出：选择最久未访问且未 pinned 的页。
    pub fn evict_lru(&mut self, tokens_needed: u64) -> Result<Vec<PageId>, CtxError> {
        let mut evicted = Vec::new();
        let mut freed = 0u64;
        while freed < tokens_needed {
            let victim = self
                .pages
                .values()
                .filter(|p| !p.pinned)
                .min_by_key(|p| p.last_access)
                .map(|p| p.id);
            match victim {
                Some(pid) => {
                    if let Some(p) = self.pages.remove(&pid) {
                        freed += p.tokens;
                        self.used = self.used.saturating_sub(p.tokens);
                        self.evictions += 1;
                        evicted.push(pid);
                    }
                }
                None => {
                    return Err(CtxError::OutOfCapacity {
                        need: tokens_needed,
                        free: freed,
                    })
                }
            }
        }
        Ok(evicted)
    }

    /// 缺页处理：换入一页（由长期记忆提供内容）。
    pub fn fault_in(&mut self, text: &str, tokens: u64) -> Result<PageId, CtxError> {
        self.faults += 1;
        if self.used + tokens > self.capacity {
            self.evict_lru(tokens)?;
        }
        self.alloc_page(text, tokens)
    }

    pub fn page(&self, id: PageId) -> Option<&SemanticPage> {
        self.pages.get(&id)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// 全局上下文管理器。
pub struct CtxManager {
    windows: BTreeMap<u64, CtxWindow>,
    next_window: u64,
}

impl CtxManager {
    pub fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            next_window: 1,
        }
    }

    pub fn create(&mut self, owner: u64, capacity: u64) -> u64 {
        let id = self.next_window;
        self.next_window += 1;
        self.windows.insert(id, CtxWindow::new(id, owner, capacity));
        id
    }

    pub fn window(&self, id: u64) -> Option<&CtxWindow> {
        self.windows.get(&id)
    }

    pub fn window_mut(&mut self, id: u64) -> Option<&mut CtxWindow> {
        self.windows.get_mut(&id)
    }

    pub fn destroy(&mut self, id: u64) -> Result<(), CtxError> {
        self.windows
            .remove(&id)
            .map(|_| ())
            .ok_or(CtxError::WindowNotFound { id })
    }

    pub fn total_used(&self) -> u64 {
        self.windows.values().map(|w| w.used).sum()
    }

    pub fn count(&self) -> usize {
        self.windows.len()
    }
}

impl Default for CtxManager {
    fn default() -> Self {
        Self::new()
    }
}

static CTX_MGR: OnceLock<Mutex<CtxManager>> = OnceLock::new();

pub fn ctx_mgr() -> &'static Mutex<CtxManager> {
    CTX_MGR.get_or_init(|| Mutex::new(CtxManager::new()))
}

pub fn init() {
    let mut m = ctx_mgr().lock().unwrap();
    if m.count() == 0 {
        m.create(1, 32_000);
    }
}

pub fn status() -> String {
    let m = ctx_mgr().lock().unwrap();
    format!("windows={} used_tokens={}", m.count(), m.total_used())
}

/// `act ctx://*` 的后端动作：
/// * `create <owner> <capacity>` 新建窗口
/// * `alloc <window> <tokens> <text...>` 分配语义页
/// * `access <window> <page>` 访问页
/// * `evict <window> <tokens>` 换出
/// * `fault <window> <tokens> <text...>` 缺页换入
/// * `destroy <window>` 销毁
/// * `status` 摘要
pub fn ctx_act(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(4, ' ');
    let mut m = ctx_mgr().lock().unwrap();
    match it.next() {
        Some("create") => {
            let owner = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let cap = it.next().and_then(|s| s.parse().ok()).unwrap_or(4096);
            let id = m.create(owner, cap);
            Ok(format!("已创建上下文窗口 ctx={} capacity={}", id, cap))
        }
        Some("alloc") => {
            let wid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: ctx://kv alloc <window> <tokens> <text>")?;
            let mut rest = it.next().unwrap_or("").splitn(2, ' ');
            let tokens = rest.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let text = rest.next().unwrap_or("");
            let w = m.window_mut(wid).ok_or("窗口不存在")?;
            let page = w
                .alloc_page(text, tokens)
                .map_err(|e| format!("分配失败: {:?}", e))?;
            Ok(format!("已分配语义页 page={} tokens={}", page.0, tokens))
        }
        Some("access") => {
            let wid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: ctx://kv access <window> <page>")?;
            let pid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .map(PageId)
                .ok_or("用法: ctx://kv access <window> <page>")?;
            let w = m.window_mut(wid).ok_or("窗口不存在")?;
            let page = w.access(pid).map_err(|e| format!("访问失败: {:?}", e))?;
            Ok(format!("page={} tokens={} dirty={}", page.id.0, page.tokens, page.dirty))
        }
        Some("evict") => {
            let wid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: ctx://kv evict <window> <tokens>")?;
            let tokens = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let w = m.window_mut(wid).ok_or("窗口不存在")?;
            let ev = w
                .evict_lru(tokens)
                .map_err(|e| format!("换出失败: {:?}", e))?;
            Ok(format!("已换出 {} 页", ev.len()))
        }
        Some("fault") => {
            let wid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: ctx://kv fault <window> <tokens> <text>")?;
            let mut rest = it.next().unwrap_or("").splitn(2, ' ');
            let tokens = rest.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let text = rest.next().unwrap_or("");
            let w = m.window_mut(wid).ok_or("窗口不存在")?;
            let page = w
                .fault_in(text, tokens)
                .map_err(|e| format!("缺页处理失败: {:?}", e))?;
            Ok(format!("缺页处理完成 page={}", page.0))
        }
        Some("destroy") => {
            let wid = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: ctx://kv destroy <window>")?;
            m.destroy(wid).map_err(|e| format!("销毁失败: {:?}", e))?;
            Ok(format!("已销毁上下文窗口 ctx={}", wid))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 ctx 动作: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_access() {
        let mut w = CtxWindow::new(1, 1, 100);
        let p = w.alloc_page("hello", 10).unwrap();
        assert_eq!(w.used, 10);
        assert_eq!(w.access(p).unwrap().text, "hello");
        assert_eq!(w.page_count(), 1);
    }

    #[test]
    fn capacity_error() {
        let mut w = CtxWindow::new(1, 1, 10);
        assert!(matches!(
            w.alloc_page("too big", 20),
            Err(CtxError::OutOfCapacity { .. })
        ));
    }

    #[test]
    fn lru_eviction() {
        let mut w = CtxWindow::new(1, 1, 30);
        let a = w.alloc_page("a", 10).unwrap();
        let b = w.alloc_page("b", 10).unwrap();
        let c = w.alloc_page("c", 10).unwrap();
        // 访问 a，使 b 成为最久未用。
        w.access(a).unwrap();
        let evicted = w.evict_lru(10).unwrap();
        assert_eq!(evicted, vec![b]);
        assert_eq!(w.evictions, 1);
        assert!(w.page(c).is_some());
    }

    #[test]
    fn fault_in_and_pinned() {
        let mut w = CtxWindow::new(1, 1, 20);
        let a = w.alloc_page("a", 10).unwrap();
        w.pages.get_mut(&a).unwrap().pinned = true;
        w.fault_in("b", 15).unwrap();
        assert_eq!(w.faults, 1);
        assert!(w.page(a).is_some(), "pinned 页不应被换出");
    }

    #[test]
    fn manager_lifecycle() {
        let mut m = CtxManager::new();
        let id = m.create(7, 1000);
        assert_eq!(m.count(), 1);
        m.window_mut(id).unwrap().alloc_page("x", 5).unwrap();
        assert_eq!(m.total_used(), 5);
        m.destroy(id).unwrap();
        assert_eq!(m.count(), 0);
    }
}
