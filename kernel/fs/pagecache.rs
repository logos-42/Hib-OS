//! 页缓存与脏页回写。
//!
//! 以页（4 KiB）为单位缓存文件数据，标记脏页，支持回写到块设备
//! 与缓存压力下的 LRU 回收。

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};

/// 页大小（4 KiB，与块大小一致）。
pub const PAGE_SIZE: usize = BLOCK_SIZE;

/// 页状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageState {
    #[default]
    Clean,
    Dirty,
}

/// 缓存页。
#[derive(Clone, Debug)]
pub struct Page {
    pub data: [u8; PAGE_SIZE],
    pub state: PageState,
}

impl Page {
    fn clean(data: [u8; PAGE_SIZE]) -> Self {
        Self {
            data,
            state: PageState::Clean,
        }
    }
}

type CacheKey = (u64, u64); // (inode, page_index)

/// 页缓存：哈希表 + LRU 列表 + 脏页跟踪。
///
/// `lru` 尾部为最近使用，头部为最久未使用。容量满时优先驱逐干净页；
/// 脏页须先回写才能被驱逐。
pub struct PageCache {
    pages: std::collections::BTreeMap<CacheKey, Page>,
    lru: Vec<CacheKey>,
    capacity: usize,
}

impl PageCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            pages: std::collections::BTreeMap::new(),
            lru: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn contains(&self, key: CacheKey) -> bool {
        self.pages.contains_key(&key)
    }

    /// 获取页的可变引用（命中则触发 LRU，移至尾部）。未命中返回 None。
    pub fn get(&mut self, key: CacheKey) -> Option<&mut [u8; PAGE_SIZE]> {
        if self.pages.contains_key(&key) {
            self.touch(key);
        }
        self.pages.get_mut(&key).map(|p| &mut p.data)
    }

    /// 只读访问页数据（不触发 LRU）。
    pub fn page_data(&self, key: CacheKey) -> Option<&[u8; PAGE_SIZE]> {
        self.pages.get(&key).map(|p| &p.data)
    }

    /// 获取页的可变引用并标记为最近使用（不改变脏位）。
    pub fn page_data_mut(&mut self, key: CacheKey) -> Option<&mut [u8; PAGE_SIZE]> {
        self.touch(key);
        self.pages.get_mut(&key).map(|p| &mut p.data)
    }

    pub fn state(&self, key: CacheKey) -> Option<PageState> {
        self.pages.get(&key).map(|p| p.state)
    }

    pub fn is_dirty(&self, key: CacheKey) -> bool {
        matches!(self.state(key), Some(PageState::Dirty))
    }

    /// 标记页为脏页。
    pub fn mark_dirty(&mut self, key: CacheKey) -> bool {
        if let Some(p) = self.pages.get_mut(&key) {
            p.state = PageState::Dirty;
            true
        } else {
            false
        }
    }

    /// 标记页为干净（回写完成后调用）。
    pub fn set_clean(&mut self, key: CacheKey) {
        if let Some(p) = self.pages.get_mut(&key) {
            p.state = PageState::Clean;
        }
    }

    /// 插入一页（标干净）。若容量不足，优先驱逐 LRU 头部的干净页。
    pub fn insert(&mut self, key: CacheKey, data: [u8; PAGE_SIZE]) -> Option<Page> {
        let existed = self.pages.contains_key(&key);
        if !existed {
            while self.pages.len() >= self.capacity {
                if let Some(lru_key) = self.lru.first().cloned() {
                    if self.pages.get(&lru_key).map(|p| p.state == PageState::Clean).unwrap_or(false)
                    {
                        self.pages.remove(&lru_key);
                        self.lru.remove(0);
                    } else {
                        // 头部是脏页，不强行驱逐；停止 evict
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        self.touch(key);
        self.pages.insert(key, Page::clean(data))
    }

    /// 返回全部脏页的键（按 LRU 顺序，旧在前）。
    pub fn dirty_keys(&self) -> Vec<CacheKey> {
        self.lru
            .iter()
            .filter(|k| matches!(self.pages.get(*k), Some(p) if p.state == PageState::Dirty))
            .cloned()
            .collect()
    }

    pub fn dirty_count(&self) -> usize {
        self.dirty_keys().len()
    }

    pub fn clean_count(&self) -> usize {
        self.pages.len() - self.dirty_count()
    }

    /// 回写全部脏页：对每个脏页调用 `write_fn(key, data)`，成功后标干净。
    /// 返回成功回写的页数。
    pub fn writeback<F>(&mut self, mut write_fn: F) -> Result<usize, BlockError>
    where
        F: FnMut(CacheKey, &[u8; PAGE_SIZE]) -> Result<(), BlockError>,
    {
        let dirty = self.dirty_keys();
        let mut done = 0;
        for key in dirty {
            let page = self.pages.get(&key).expect("dirty page must exist");
            write_fn(key, &page.data)?;
            self.set_clean(key);
            done += 1;
        }
        Ok(done)
    }

    /// 缓存压力回收：释放干净页直至剩余 `target` 页；脏页不被回收。
    /// 返回释放的页数。
    pub fn reclaim(&mut self, target: usize) -> usize {
        let mut released = 0;
        let lru_snapshot = self.lru.clone();
        for key in lru_snapshot {
            if self.pages.len() <= target {
                break;
            }
            if self
                .pages
                .get(&key)
                .map(|p| p.state == PageState::Clean)
                .unwrap_or(false)
            {
                self.pages.remove(&key);
                released += 1;
            }
        }
        self.lru.retain(|k| self.pages.contains_key(k));
        released
    }

    /// 清空缓存（回写脏页后调用方应确保已持久化）。
    pub fn clear(&mut self) {
        self.pages.clear();
        self.lru.clear();
    }

    fn touch(&mut self, key: CacheKey) {
        self.lru.retain(|k| *k != key);
        self.lru.push(key);
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new(64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_with_byte(byte: u8) -> [u8; PAGE_SIZE] {
        let mut d = [0u8; PAGE_SIZE];
        d[0] = byte;
        d
    }

    #[test]
    fn page_insert_and_get_roundtrip() {
        let mut cache = PageCache::new(16);
        let key = (10, 0);
        assert!(!cache.contains(key));
        cache.insert(key, page_with_byte(0x01));
        assert!(cache.contains(key));
        let data = cache.get(key).unwrap();
        assert_eq!(data[0], 0x01);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.dirty_count(), 0);
    }

    #[test]
    fn page_mark_dirty_and_writeback() {
        let mut cache = PageCache::new(16);
        let key = (10, 1);
        cache.insert(key, page_with_byte(0x02));
        cache.mark_dirty(key);
        assert!(cache.is_dirty(key));
        assert_eq!(cache.dirty_count(), 1);

        let mut writes: Vec<(CacheKey, u8)> = Vec::new();
        let result = cache.writeback(|k, data| {
            writes.push((k, data[0]));
            Ok(())
        });
        assert!(result.is_ok());
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, 0x02);
        assert!(!cache.is_dirty(key));
        assert_eq!(cache.dirty_count(), 0);
    }

    #[test]
    fn page_reclaim_keeps_dirty_pages() {
        let mut cache = PageCache::new(2);
        cache.insert((1, 0), page_with_byte(0x10)); // clean
        cache.insert((1, 1), page_with_byte(0x20)); // clean
        cache.mark_dirty((1, 1)); // 第二个脏
        assert_eq!(cache.len(), 2);

        // 压力回收到 1 页：只能回收干净页 (1,0)
        let released = cache.reclaim(1);
        assert_eq!(released, 1);
        assert_eq!(cache.len(), 1);
        assert!(!cache.contains((1, 0)));
        assert!(cache.contains((1, 1))); // 脏页被保留
        assert_eq!(cache.dirty_count(), 1);
    }

    #[test]
    fn page_lru_eviction_on_capacity() {
        let mut cache = PageCache::new(2);
        cache.insert((1, 0), page_with_byte(0x10));
        cache.insert((1, 1), page_with_byte(0x20));
        // 访问 (1,0) 使其最近，(1,1) 变成 LRU 头部
        let _ = cache.get((1, 0));
        // 插入第三页，应驱逐干净的 (1,1)
        cache.insert((1, 2), page_with_byte(0x30));
        assert_eq!(cache.len(), 2);
        assert!(!cache.contains((1, 1)));
        assert!(cache.contains((1, 0)));
        assert!(cache.contains((1, 2)));
    }

    #[test]
    fn page_writeback_error_propagates() {
        let mut cache = PageCache::new(4);
        cache.insert((1, 0), page_with_byte(0x99));
        cache.mark_dirty((1, 0));
        let result = cache.writeback(|_k, _data| Err(BlockError::IoError("disk full".into())));
        assert_eq!(result.unwrap_err(), BlockError::IoError("disk full".into()));
    }
}

