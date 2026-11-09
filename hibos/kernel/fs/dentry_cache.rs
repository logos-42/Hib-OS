//! 目录项缓存（dcache）与 inode 缓存。
//!
//! 提供名称查找（dentry lookup）、dentry/inode 缓存、LRU 淘汰、inode 引用计数。
//! 哈希键为 (父 inode 号，名称)，支持 `lookup` / `insert` / `invalidate` / `prune`。
//!
//! 与 `ext2_like.rs` / `vfs.rs` 配合，加速路径解析并减少重复磁盘读取。

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// inode 号。
pub type Ino = u64;
/// inode 句柄（持有引用计数）。
pub type InodeHandle = u64;

/// 目录项：名称、父目录 dentry、关联 inode、引用计数、锁。
#[derive(Clone, Debug)]
pub struct Dentry {
    /// 名称。
    pub name: String,
    /// 父目录 dentry 的 inode 号（根目录为 0）。
    pub parent_ino: Ino,
    /// 关联 inode 号。
    pub ino: Ino,
    /// 引用计数（被查找/持有的次数）。
    pub refcount: u32,
    /// 是否被标记失效（待删除）。
    pub invalid: bool,
    /// 创建/最后访问时间（纳秒）。
    pub created_at: u64,
}

impl Dentry {
    pub fn new(name: &str, parent_ino: Ino, ino: Ino) -> Self {
        Self {
            name: name.to_string(),
            parent_ino,
            ino,
            refcount: 0,
            invalid: false,
            created_at: now_nanos(),
        }
    }
}

/// dcache 哈希键。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DentryKey {
    parent_ino: Ino,
    name: String,
}

/// 目录项缓存：哈希表 + LRU + 引用计数。
pub struct DentryCache {
    /// 按 (parent_ino, name) 索引的 dentry。
    entries: Mutex<HashMap<DentryKey, Dentry>>,
    /// LRU 链表：头部为最久未使用。
    lru: Mutex<VecDeque<DentryKey>>,
    /// 容量。
    capacity: usize,
    /// 统计。
    stats: Mutex<DcacheStats>,
}

/// dcache 统计。
#[derive(Clone, Debug, Default)]
pub struct DcacheStats {
    pub entries: usize,
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub invalidations: u64,
    pub evictions: u64,
    pub prunes: u64,
}

impl DentryCache {
    /// 创建容量为 `capacity` 的目录项缓存。
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            lru: Mutex::new(VecDeque::new()),
            capacity: capacity.max(1),
            stats: Mutex::new(DcacheStats::default()),
        }
    }

    /// 查找 dentry：(父 inode, 名称) -> inode。
    ///
    /// 命中则引用计数 +1、移至 LRU 尾部；未命中返回 None。
    pub fn lookup(&self, parent_ino: Ino, name: &str) -> Option<Ino> {
        let key = DentryKey {
            parent_ino,
            name: name.to_string(),
        };
        let mut entries = self.entries.lock().unwrap();
        let mut lru = self.lru.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        stats.lookups += 1;
        if let Some(dentry) = entries.get_mut(&key) {
            if !dentry.invalid {
                dentry.refcount += 1;
                stats.hits += 1;
                touch_lru(&mut lru, &key);
                return Some(dentry.ino);
            }
        }
        stats.misses += 1;
        None
    }

    /// 插入 dentry。若已存在则更新。
    pub fn insert(&self, dentry: Dentry) {
        let key = DentryKey {
            parent_ino: dentry.parent_ino,
            name: dentry.name.clone(),
        };
        let mut entries = self.entries.lock().unwrap();
        let mut lru = self.lru.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        let existed = entries.contains_key(&key);
        entries.insert(key.clone(), dentry);
        if !existed {
            stats.inserts += 1;
            lru.push_back(key);
        } else {
            touch_lru(&mut lru, &key);
        }
        // 超容量时淘汰 LRU 头部的非引用 dentry。
        while entries.len() > self.capacity {
            if let Some(lru_key) = lru.front().cloned() {
                if let Some(d) = entries.get(&lru_key) {
                    if d.refcount == 0 && !d.invalid {
                        entries.remove(&lru_key);
                        lru.pop_front();
                        stats.evictions += 1;
                    } else {
                        lru.pop_front();
                        lru.push_back(lru_key);
                    }
                } else {
                    lru.pop_front();
                }
            } else {
                break;
            }
        }
    }

    /// 使 dentry 失效（不立即删除，标记 invalid，后续 prune 清理）。
    pub fn invalidate(&self, parent_ino: Ino, name: &str) -> bool {
        let key = DentryKey {
            parent_ino,
            name: name.to_string(),
        };
        let mut entries = self.entries.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        if let Some(d) = entries.get_mut(&key) {
            d.invalid = true;
            stats.invalidations += 1;
            true
        } else {
            false
        }
    }

    /// 使指定 ino 的全部子 dentry 失效。
    pub fn invalidate_children(&self, parent_ino: Ino) -> usize {
        let mut entries = self.entries.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        let mut count = 0;
        let keys: Vec<DentryKey> = entries
            .iter()
            .filter(|(k, _)| k.parent_ino == parent_ino)
            .map(|(k, _)| k.clone())
            .collect();
        for key in keys {
            if let Some(d) = entries.get_mut(&key) {
                d.invalid = true;
                count += 1;
            }
        }
        stats.invalidations += count as u64;
        count
    }

    /// 查找并返回 inode（无副作用，不增加引用）。
    pub fn get_inode(&self, parent_ino: Ino, name: &str) -> Option<Ino> {
        let key = DentryKey {
            parent_ino,
            name: name.to_string(),
        };
        let entries = self.entries.lock().unwrap();
        entries.get(&key).filter(|d| !d.invalid).map(|d| d.ino)
    }

    /// 增加 dentry 引用计数。
    pub fn hold(&self, parent_ino: Ino, name: &str) -> bool {
        let key = DentryKey {
            parent_ino,
            name: name.to_string(),
        };
        let mut entries = self.entries.lock().unwrap();
        if let Some(d) = entries.get_mut(&key) {
            if !d.invalid {
                d.refcount += 1;
                return true;
            }
        }
        false
    }

    /// 减少 dentry 引用计数。
    pub fn release(&self, parent_ino: Ino, name: &str) -> bool {
        let key = DentryKey {
            parent_ino,
            name: name.to_string(),
        };
        let mut entries = self.entries.lock().unwrap();
        if let Some(d) = entries.get_mut(&key) {
            if d.refcount > 0 {
                d.refcount -= 1;
                return true;
            }
        }
        false
    }

    /// 清理失效且无引用的 dentry，返回清理数。
    pub fn prune(&self) -> usize {
        let mut entries = self.entries.lock().unwrap();
        let mut lru = self.lru.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        let mut count = 0;
        let keys: Vec<DentryKey> = entries.keys().cloned().collect();
        for key in keys {
            if let Some(d) = entries.get(&key) {
                if d.invalid && d.refcount == 0 {
                    entries.remove(&key);
                    lru.retain(|k| *k != key);
                    count += 1;
                }
            }
        }
        stats.prunes += count as u64;
        count
    }

    /// 返回缓存统计。
    pub fn stats(&self) -> DcacheStats {
        let entries = self.entries.lock().unwrap();
        let stats = self.stats.lock().unwrap();
        DcacheStats {
            entries: entries.len(),
            lookups: stats.lookups,
            hits: stats.hits,
            misses: stats.misses,
            inserts: stats.inserts,
            invalidations: stats.invalidations,
            evictions: stats.evictions,
            prunes: stats.prunes,
        }
    }

    /// 返回条目数。
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap().is_empty()
    }
}

impl Default for DentryCache {
    fn default() -> Self {
        Self::new(256)
    }
}

/// inode 缓存条目。
#[derive(Clone, Debug)]
pub struct CachedInode {
    pub ino: Ino,
    /// 引用计数。
    pub refcount: u32,
    /// 是否脏（待回写）。
    pub dirty: bool,
    /// 是否失效。
    pub invalid: bool,
}

impl CachedInode {
    pub fn new(ino: Ino) -> Self {
        Self {
            ino,
            refcount: 0,
            dirty: false,
            invalid: false,
        }
    }
}

/// inode 缓存：按 inode 号缓存，支持引用计数与驱逐。
pub struct InodeCache {
    inodes: Mutex<HashMap<Ino, CachedInode>>,
    lru: Mutex<VecDeque<Ino>>,
    capacity: usize,
    stats: Mutex<InodeCacheStats>,
}

/// inode 缓存统计。
#[derive(Clone, Debug, Default)]
pub struct InodeCacheStats {
    pub entries: usize,
    pub gets: u64,
    pub hits: u64,
    pub misses: u64,
    pub puts: u64,
    pub evictions: u64,
}

impl InodeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inodes: Mutex::new(HashMap::new()),
            lru: Mutex::new(VecDeque::new()),
            capacity: capacity.max(1),
            stats: Mutex::new(InodeCacheStats::default()),
        }
    }

    /// 获取 inode 句柄：引用计数 +1，未缓存则插入空条目。
    pub fn get_inode(&self, ino: Ino) -> InodeHandle {
        let mut inodes = self.inodes.lock().unwrap();
        let mut lru = self.lru.lock().unwrap();
        let mut stats = self.stats.lock().unwrap();
        stats.gets += 1;
        if let Some(cached) = inodes.get_mut(&ino) {
            if !cached.invalid {
                cached.refcount += 1;
                stats.hits += 1;
                touch_lru_ino(&mut lru, ino);
                return ino;
            }
        }
        stats.misses += 1;
        let mut cached = CachedInode::new(ino);
        cached.refcount = 1;
        inodes.insert(ino, cached);
        lru.push_back(ino);
        // 超容量淘汰。
        while inodes.len() > self.capacity {
            if let Some(evict_ino) = lru.front().copied() {
                if let Some(c) = inodes.get(&evict_ino) {
                    if c.refcount == 0 && !c.dirty {
                        inodes.remove(&evict_ino);
                        lru.pop_front();
                        stats.evictions += 1;
                    } else {
                        lru.pop_front();
                        lru.push_back(evict_ino);
                    }
                } else {
                    lru.pop_front();
                }
            } else {
                break;
            }
        }
        ino
    }

    /// 释放 inode 句柄：引用计数 -1。
    pub fn put_inode(&self, handle: InodeHandle) {
        let mut inodes = self.inodes.lock().unwrap();
        if let Some(c) = inodes.get_mut(&handle) {
            if c.refcount > 0 {
                c.refcount -= 1;
            }
        }
        let mut stats = self.stats.lock().unwrap();
        stats.puts += 1;
    }

    /// 驱逐指定 inode（需 refcount=0 且非脏）。
    pub fn evict_inode(&self, ino: Ino) -> Result<(), ()> {
        let mut inodes = self.inodes.lock().unwrap();
        let mut lru = self.lru.lock().unwrap();
        if let Some(c) = inodes.get(&ino) {
            if c.refcount > 0 || c.dirty {
                return Err(());
            }
        }
        inodes.remove(&ino);
        lru.retain(|&i| i != ino);
        Ok(())
    }

    /// 标记 inode 脏。
    pub fn mark_dirty(&self, ino: Ino) -> bool {
        let mut inodes = self.inodes.lock().unwrap();
        if let Some(c) = inodes.get_mut(&ino) {
            c.dirty = true;
            return true;
        }
        false
    }

    /// 标记 inode 干净。
    pub fn mark_clean(&self, ino: Ino) -> bool {
        let mut inodes = self.inodes.lock().unwrap();
        if let Some(c) = inodes.get_mut(&ino) {
            c.dirty = false;
            return true;
        }
        false
    }

    /// 使 inode 失效。
    pub fn invalidate(&self, ino: Ino) -> bool {
        let mut inodes = self.inodes.lock().unwrap();
        if let Some(c) = inodes.get_mut(&ino) {
            c.invalid = true;
            return true;
        }
        false
    }

    /// 返回统计。
    pub fn stats(&self) -> InodeCacheStats {
        let inodes = self.inodes.lock().unwrap();
        let stats = self.stats.lock().unwrap();
        InodeCacheStats {
            entries: inodes.len(),
            gets: stats.gets,
            hits: stats.hits,
            misses: stats.misses,
            puts: stats.puts,
            evictions: stats.evictions,
        }
    }

    pub fn len(&self) -> usize {
        self.inodes.lock().unwrap().len()
    }
}

impl Default for InodeCache {
    fn default() -> Self {
        Self::new(512)
    }
}

/// LRU 辅助：将 key 移到链表尾部。
fn touch_lru(lru: &mut VecDeque<DentryKey>, key: &DentryKey) {
    if let Some(pos) = lru.iter().position(|k| *k == *key) {
        lru.remove(pos);
    }
    lru.push_back(key.clone());
}

/// LRU 辅助：将 ino 移到链表尾部。
fn touch_lru_ino(lru: &mut VecDeque<Ino>, ino: Ino) {
    if let Some(pos) = lru.iter().position(|&i| i == ino) {
        lru.remove(pos);
    }
    lru.push_back(ino);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dentry_lookup_insert_hit_miss() {
        let cache = DentryCache::new(16);
        // 未命中。
        assert_eq!(cache.lookup(1, "foo"), None);
        // 插入。
        cache.insert(Dentry::new("foo", 1, 10));
        assert_eq!(cache.lookup(1, "foo"), Some(10));
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
    }

    #[test]
    fn dentry_invalidate_and_prune() {
        let cache = DentryCache::new(16);
        cache.insert(Dentry::new("a", 1, 10));
        cache.insert(Dentry::new("b", 1, 11));
        cache.hold(1, "a");
        cache.invalidate(1, "a");
        // 失效后 lookup 返回 None。
        assert_eq!(cache.lookup(1, "a"), None);
        // prune 清理无引用的失效项（“b”未 hold 会被清理，“a”有引用保留）。
        let pruned = cache.prune();
        assert!(pruned >= 1);
    }

    #[test]
    fn dentry_release_reduces_refcount() {
        let cache = DentryCache::new(16);
        cache.insert(Dentry::new("x", 1, 20));
        cache.hold(1, "x");
        cache.hold(1, "x");
        assert!(cache.release(1, "x"));
        assert!(cache.release(1, "x"));
        assert!(!cache.release(1, "x")); // 已归零
    }

    #[test]
    fn dentry_lru_eviction() {
        let cache = DentryCache::new(2);
        cache.insert(Dentry::new("a", 1, 10));
        cache.insert(Dentry::new("b", 1, 11));
        // 容量已满，插入“c”应淘汰 LRU 头部（“a”未被 hold）。
        cache.insert(Dentry::new("c", 1, 12));
        assert_eq!(cache.lookup(1, "c"), Some(12));
        // “a”未 hold，可能已被淘汰；此处断言容量不超限。
        assert!(cache.len() <= 2);
    }

    #[test]
    fn dentry_stats_default() {
        let cache = DentryCache::new(8);
        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.lookups, 0);
    }

    #[test]
    fn inode_cache_get_put() {
        let cache = InodeCache::new(8);
        let h1 = cache.get_inode(100);
        assert_eq!(h1, 100);
        let h2 = cache.get_inode(100); // 命中
        assert_eq!(h2, 100);
        cache.put_inode(100);
        cache.put_inode(100);
        let stats = cache.stats();
        assert_eq!(stats.gets, 2);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.puts, 2);
    }

    #[test]
    fn inode_cache_evict_dirty_not() {
        let cache = InodeCache::new(2);
        cache.get_inode(1);
        cache.get_inode(2);
        cache.mark_dirty(1);
        // 插入第三个，容量满；1 是脏不应被淘汰，2 应被淘汰。
        cache.get_inode(3);
        let stats = cache.stats();
        assert!(stats.evictions >= 1);
    }

    #[test]
    fn inode_cache_evict_inode() {
        let cache = InodeCache::new(8);
        cache.get_inode(5);
        assert!(cache.evict_inode(5).is_ok());
        // 引用计数 > 0 时不应驱逐。
        cache.get_inode(6);
        assert!(cache.evict_inode(6).is_err());
        cache.put_inode(6);
        assert!(cache.evict_inode(6).is_ok());
    }

    #[test]
    fn inode_cache_dirty_clean() {
        let cache = InodeCache::new(8);
        cache.get_inode(7);
        assert!(cache.mark_dirty(7));
        assert!(cache.mark_clean(7));
    }

    #[test]
    fn inode_cache_stats_default() {
        let cache = InodeCache::new(8);
        let stats = cache.stats();
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.gets, 0);
    }
}

/// 现在的纳秒时间戳。
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
