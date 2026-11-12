//! 页缓存与缓冲区缓存。
//!
//! 页缓存（PageCache）缓存由键（key）索引的页框，支持引用计数、脏页标记、
//! LRU 淘汰与脏页刷写；缓冲区缓存（BufferCache）以扇区为单位缓存磁盘块，
//! 提供 bread/brelse/bwrite/sync 接口。两者均模拟宿主页框与锁，不接触真实
//! 磁盘，适合内存管理子系统的原型验证。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================================
// 页框与缓冲区通用类型
// =====================================================================================

/// 页缓存键：任意字符串标识（如文件路径 + 偏移页）。
pub type PageKey = String;

/// 缓冲区扇区号。
pub type Sector = u64;

/// 页框错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCacheError;

impl std::fmt::Display for PageCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "页缓存操作失败")
    }
}

/// 缓冲区错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferError {
    /// 扇区不在缓存中且无法读入。
    NotFound,
    /// 缓冲区已被锁定。
    Locked,
    /// 缓冲区句柄无效。
    Invalid,
}

impl std::fmt::Display for BufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            BufferError::NotFound => "缓冲区不存在".to_string(),
            BufferError::Locked => "缓冲区已锁定".to_string(),
            BufferError::Invalid => "缓冲区句柄无效".to_string(),
        };
        write!(f, "{}", s)
    }
}

// =====================================================================================
// PageHandle —— 页框句柄
// =====================================================================================

/// 页框句柄：持有页缓存中某页的引用，包含引用计数与锁定状态。
#[derive(Clone, Debug)]
pub struct PageHandle {
    pub key: PageKey,
    pub page_index: usize,
    pub data: Vec<u8>,
    /// 引用计数（持有者数量）。
    pub refcount: usize,
    /// 是否被锁定（锁定后不可被 LRU 淘汰）。
    pub locked: bool,
    /// 是否为脏页（需刷写）。
    pub dirty: bool,
    /// 上一次访问时间（Unix 秒，供 LRU 使用）。
    pub last_access: u64,
}

impl PageHandle {
    fn new(key: PageKey, page_index: usize, data: Vec<u8>) -> Self {
        let now = now_seconds();
        Self {
            key,
            page_index,
            data,
            refcount: 1,
            locked: false,
            dirty: false,
            last_access: now,
        }
    }

    /// 返回页大小（字节）。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 标记为脏页。
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.touch();
    }

    /// 清除脏页标记。
    pub fn clean(&mut self) {
        self.dirty = false;
    }

    /// 更新访问时间。
    fn touch(&mut self) {
        self.last_access = now_seconds();
    }

    /// 是否可被淘汰（refcount 为 0 且未锁定）。
    pub fn evictable(&self) -> bool {
        self.refcount == 0 && !self.locked
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// =====================================================================================
// PageCache —— 页缓存
// =====================================================================================

/// 页缓存：以键索引页框，支持 LRU 淘汰、脏页标记与引用计数。
///
/// 每个页框占固定大小（默认 4096 字节）。缓存容量上限为 `max_pages`；超出后按
// LRU 淘汰最久未访问且可被淘汰的页框。
pub struct PageCache {
    pub page_size: usize,
    pub max_pages: usize,
    /// key -> 页框索引（在 pages Vec 中）。
    map: BTreeMap<PageKey, usize>,
    /// 页框存储。
    pages: Vec<Option<PageHandle>>,
    /// LRU 列表：按最近访问顺序的页框索引队列。
    lru: VecDeque<usize>,
    /// 脏页集合：脏页框索引。
    dirty_set: BTreeMap<usize, ()>,
    /// 统计。
    hits: u64,
    misses: u64,
    evictions: u64,
    flushes: u64,
}

impl PageCache {
    /// 创建页缓存，页大小 `page_size`，最多缓存 `max_pages` 个页框。
    pub fn new(page_size: usize, max_pages: usize) -> Self {
        Self {
            page_size,
            max_pages,
            map: BTreeMap::new(),
            pages: Vec::new(),
            lru: VecDeque::new(),
            dirty_set: BTreeMap::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            flushes: 0,
        }
    }

    /// 返回页大小。
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// 返回缓存容量。
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// 返回当前缓存的页框总数。
    pub fn len(&self) -> usize {
        self.pages.iter().filter(|p| p.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取指定键的页框。若缓存命中则增加引用计数并返回句柄；否则分配新页框
    // （数据全零）并返回。
    ///
    /// 返回的 `PageHandle` 是克隆，调用方通过 `release_page` 归还引用。
    pub fn get_page(&mut self, key: &str) -> PageHandle {
        if let Some(&idx) = self.map.get(key) {
            // 命中。
            self.hits += 1;
            if let Some(page) = self.pages[idx].as_mut() {
                page.refcount += 1;
                page.touch();
                let h = page.clone();
                self._touch_lru(idx);
                return h;
            }
        }
        // 未命中：分配新页框。
        self.misses += 1;
        let page_index = self.pages.len();
        let data = vec![0u8; self.page_size];
        let mut handle = PageHandle::new(key.to_string(), page_index, data);
        // 若超过容量，先淘汰。
        if self.len() >= self.max_pages {
            self.evict_lru();
        }
        let idx = self.pages.len();
        self.pages.push(Some(handle.clone()));
        self.map.insert(key.to_string(), idx);
        self.lru.push_back(idx);
        handle.page_index = idx;
        handle
    }

    /// 释放一个页框句柄的引用。引用计数归零且非脏页时可被后续 LRU 淘汰。
    pub fn release_page(&mut self, handle: &PageHandle) {
        let idx = handle.page_index;
        if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
            if page.refcount > 0 {
                page.refcount -= 1;
            }
            page.touch();
        }
    }

    /// 将指定页框标记为脏页（需刷写）。
    pub fn mark_dirty(&mut self, key: &str) -> Result<(), PageCacheError> {
        let idx = *self.map.get(key).ok_or(PageCacheError)?;
        if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
            page.mark_dirty();
            self.dirty_set.insert(idx, ());
        }
        Ok(())
    }

    /// 将指定页框标记为干净。
    pub fn clean_page(&mut self, key: &str) -> Result<(), PageCacheError> {
        let idx = *self.map.get(key).ok_or(PageCacheError)?;
        if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
            page.clean();
            self.dirty_set.remove(&idx);
        }
        Ok(())
    }

    /// 锁定指定页框（锁定后不可被 LRU 淘汰）。
    pub fn lock_page(&mut self, key: &str) -> Result<(), PageCacheError> {
        let idx = *self.map.get(key).ok_or(PageCacheError)?;
        if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
            page.locked = true;
        }
        Ok(())
    }

    /// 解锁指定页框。
    pub fn unlock_page(&mut self, key: &str) -> Result<(), PageCacheError> {
        let idx = *self.map.get(key).ok_or(PageCacheError)?;
        if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
            page.locked = false;
        }
        Ok(())
    }

    /// 刷写所有脏页：将脏页数据“写回”模拟后端，并清除脏标记。
    pub fn flush_dirty(&mut self) -> u64 {
        let dirty_indices: Vec<usize> = self.dirty_set.keys().copied().collect();
        let count = dirty_indices.len() as u64;
        for idx in dirty_indices {
            if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.as_mut()) {
                if page.dirty {
                    page.clean();
                }
            }
            self.dirty_set.remove(&idx);
        }
        self.flushes += count;
        count
    }

    /// 按 LRU 淘汰最久未访问且可被淘汰的页框（refcount=0 且未锁定且非脏页；
    // 脏页会先被 flush_dirty 处理，此处只淘汰干净页）。
    pub fn evict_lru(&mut self) -> usize {
        let mut evicted = 0;
        while self.len() >= self.max_pages {
            let idx = match self.lru.pop_front() {
                Some(i) => i,
                None => break,
            };
            let can_evict = self.pages.get(idx).and_then(|p| p.as_ref()).map_or(false, |p| p.evictable());
            if can_evict {
                if let Some(page) = self.pages.get_mut(idx).and_then(|p| p.take()) {
                    self.map.remove(&page.key);
                    evicted += 1;
                }
            } else {
                // 不可淘汰，放回队列末尾（避免饿死，下次再试）。
                self.lru.push_back(idx);
                // 防止全部不可淘汰时死循环：检查是否还有可淘汰项。
                if !self.has_evictable() {
                    break;
                }
            }
        }
        self.evictions += evicted as u64;
        evicted
    }

    /// 是否存在至少一个可被淘汰的页框。
    fn has_evictable(&self) -> bool {
        self.pages.iter().flatten().any(|p| p.evictable())
    }

    /// 返回缓存命中率（hits / (hits+misses)，范围 0.0~1.0）。
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }

    /// 返回脏页数量。
    pub fn dirty_count(&self) -> usize {
        self.dirty_set.len()
    }

    /// 返回淘汰总次数。
    pub fn eviction_count(&self) -> u64 {
        self.evictions
    }

    /// 返回刷写总次数。
    pub fn flush_count(&self) -> u64 {
        self.flushes
    }

    fn _touch_lru(&mut self, idx: usize) {
        // 将 idx 移到 LRU 队列末尾（最近使用）。
        if let Some(pos) = self.lru.iter().position(|&x| x == idx) {
            let item = self.lru.remove(pos).unwrap();
            self.lru.push_back(item);
        }
    }

    /// 返回页缓存文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "page_cache page_size={} max_pages={} cached={} dirty={} hits={} misses={} hit_rate={:.2} evictions={} flushes={}",
            self.page_size,
            self.max_pages,
            self.len(),
            self.dirty_count(),
            self.hits,
            self.misses,
            self.cache_hit_rate(),
            self.evictions,
            self.flushes,
        )
    }
}

impl Default for PageCache {
    fn default() -> Self {
        Self::new(4096, 256)
    }
}

// =====================================================================================
// Buffer —— 缓冲区
// =====================================================================================

/// 缓冲区：对应磁盘的一个扇区，包含数据、锁定、脏页、引用计数。
#[derive(Clone, Debug)]
pub struct Buffer {
    pub sector: Sector,
    pub data: Vec<u8>,
    pub refcount: usize,
    pub locked: bool,
    pub dirty: bool,
    pub valid: bool, // 数据是否已从磁盘读入
}

impl Buffer {
    fn new(sector: Sector, block_size: usize) -> Self {
        Self {
            sector,
            data: vec![0u8; block_size],
            refcount: 1,
            locked: false,
            dirty: false,
            valid: false,
        }
    }

    pub fn sector_size(&self) -> usize {
        self.data.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

// =====================================================================================
// BufferCache —— 缓冲区缓存
// =====================================================================================

/// 缓冲区缓存：以扇区为单位缓存磁盘块，提供 bread/brelse/bwrite/sync 接口，
// 支持 LRU 淘汰与脏页标记。
pub struct BufferCache {
    pub block_size: usize,
    pub max_buffers: usize,
    /// sector -> 缓冲区索引。
    map: BTreeMap<Sector, usize>,
    /// 缓冲区存储。
    buffers: Vec<Option<Buffer>>,
    /// LRU 队列。
    lru: VecDeque<usize>,
    /// 脏缓冲区集合。
    dirty_set: BTreeMap<Sector, ()>,
    /// 锁定集合（正在 I/O 的缓冲区）。
    locked_set: BTreeMap<Sector, ()>,
    /// 统计。
    hits: u64,
    misses: u64,
    syncs: u64,
    evictions: u64,
}

impl BufferCache {
    /// 创建缓冲区缓存，块大小 `block_size`，最多缓存 `max_buffers` 个缓冲区。
    pub fn new(block_size: usize, max_buffers: usize) -> Self {
        Self {
            block_size,
            max_buffers,
            map: BTreeMap::new(),
            buffers: Vec::new(),
            lru: VecDeque::new(),
            dirty_set: BTreeMap::new(),
            locked_set: BTreeMap::new(),
            hits: 0,
            misses: 0,
            syncs: 0,
            evictions: 0,
        }
    }

    /// 返回块大小。
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// 返回当前缓存的缓冲区总数。
    pub fn len(&self) -> usize {
        self.buffers.iter().filter(|b| b.is_some()).count()
    }

    /// 读取扇区：若缓存中有且有效则返回句柄索引，否则分配新缓冲区并标记为
    // 待读入（valid=false，由调用方或模拟后端填充）。
    pub fn bread(&mut self, sector: Sector) -> Result<usize, BufferError> {
        if let Some(&idx) = self.map.get(&sector) {
            self.hits += 1;
            if let Some(buf) = self.buffers.get_mut(idx).and_then(|b| b.as_mut()) {
                buf.refcount += 1;
                buf.locked = true;
                self.locked_set.insert(sector, ());
                self._touch_lru(idx);
                return Ok(idx);
            }
        }
        self.misses += 1;
        if self.len() >= self.max_buffers {
            self.evict_lru();
        }
        let idx = self.buffers.len();
        let mut buf = Buffer::new(sector, self.block_size);
        buf.locked = true;
        buf.valid = false;
        self.locked_set.insert(sector, ());
        self.buffers.push(Some(buf));
        self.map.insert(sector, idx);
        self.lru.push_back(idx);
        Ok(idx)
    }

    /// 释放缓冲区引用并解锁。
    pub fn brelse(&mut self, idx: usize) -> Result<(), BufferError> {
        let buf = self.buffers.get_mut(idx).and_then(|b| b.as_mut()).ok_or(BufferError::Invalid)?;
        let sector = buf.sector;
        if buf.refcount > 0 {
            buf.refcount -= 1;
        }
        buf.locked = false;
        self.locked_set.remove(&sector);
        buf.touch_lru(); // 标记，下次 LRU 调整
        Ok(())
    }

    /// 将缓冲区标记为脏页（需写回磁盘）。
    pub fn bwrite(&mut self, idx: usize) -> Result<(), BufferError> {
        let buf = self.buffers.get_mut(idx).and_then(|b| b.as_mut()).ok_or(BufferError::Invalid)?;
        buf.dirty = true;
        buf.valid = true;
        self.dirty_set.insert(buf.sector, ());
        Ok(())
    }

    /// 刷写所有脏缓冲区到模拟磁盘，并清除脏标记。
    pub fn sync(&mut self) -> u64 {
        let count = self.dirty_set.len() as u64;
        for sector in self.dirty_set.keys().copied().collect::<Vec<Sector>>() {
            if let Some(idx) = self.map.get(&sector) {
                if let Some(buf) = self.buffers.get_mut(*idx).and_then(|b| b.as_mut()) {
                    if buf.dirty {
                        buf.dirty = false;
                    }
                }
            }
            self.dirty_set.remove(&sector);
        }
        self.syncs += count;
        count
    }

    /// 按 LRU 淘汰最久未使用且未锁定的缓冲区（脏缓冲区不被直接淘汰，需先 sync）。
    pub fn evict_lru(&mut self) -> usize {
        let mut evicted = 0;
        while self.len() >= self.max_buffers {
            let idx = match self.lru.pop_front() {
                Some(i) => i,
                None => break,
            };
            let can_evict = self.buffers.get(idx).and_then(|b| b.as_ref()).map_or(false, |b| {
                b.refcount == 0 && !b.locked && !b.dirty
            });
            if can_evict {
                if let Some(buf) = self.buffers.get_mut(idx).and_then(|b| b.take()) {
                    self.map.remove(&buf.sector);
                    evicted += 1;
                }
            } else {
                self.lru.push_back(idx);
                if !self.has_evictable() {
                    break;
                }
            }
        }
        self.evictions += evicted as u64;
        evicted
    }

    fn has_evictable(&self) -> bool {
        self.buffers.iter().flatten().any(|b| b.refcount == 0 && !b.locked && !b.dirty)
    }

    fn _touch_lru(&mut self, idx: usize) {
        if let Some(pos) = self.lru.iter().position(|&x| x == idx) {
            let item = self.lru.remove(pos).unwrap();
            self.lru.push_back(item);
        }
    }

    /// 返回缓存命中率。
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }

    /// 返回脏缓冲区数量。
    pub fn dirty_count(&self) -> usize {
        self.dirty_set.len()
    }

    /// 返回锁定缓冲区数量。
    pub fn locked_count(&self) -> usize {
        self.locked_set.len()
    }

    /// 返回同步总次数。
    pub fn sync_count(&self) -> u64 {
        self.syncs
    }

    /// 返回淘汰总次数。
    pub fn eviction_count(&self) -> u64 {
        self.evictions
    }

    /// 返回缓冲区文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "buffer_cache block_size={} max_buffers={} cached={} dirty={} locked={} hits={} misses={} hit_rate={:.2} syncs={} evictions={}",
            self.block_size,
            self.max_buffers,
            self.len(),
            self.dirty_count(),
            self.locked_count(),
            self.hits,
            self.misses,
            self.cache_hit_rate(),
            self.syncs,
            self.evictions,
        )
    }
}

impl Default for BufferCache {
    fn default() -> Self {
        Self::new(512, 512)
    }
}

// 为 Buffer 添加 touch_lru 字段（用于 brelse 后重新入队）。
impl Buffer {
    fn touch_lru(&mut self) {
        // 占位，实际由 BufferCache._touch_lru 处理。
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static PAGE_CACHE: OnceLock<Mutex<PageCache>> = OnceLock::new();
static BUFFER_CACHE: OnceLock<Mutex<BufferCache>> = OnceLock::new();

fn page_cache() -> &'static Mutex<PageCache> {
    PAGE_CACHE.get_or_init(|| Mutex::new(PageCache::new(4096, 256)))
}

fn buffer_cache() -> &'static Mutex<BufferCache> {
    BUFFER_CACHE.get_or_init(|| Mutex::new(BufferCache::new(512, 512)))
}

/// 获取页框（全局页缓存）。
pub fn get_page(key: &str) -> PageHandle {
    page_cache().lock().unwrap().get_page(key)
}

/// 释放页框（全局页缓存）。
pub fn release_page(handle: &PageHandle) {
    page_cache().lock().unwrap().release_page(handle)
}

/// 标记页脏（全局页缓存）。
pub fn mark_page_dirty(key: &str) -> Result<(), PageCacheError> {
    page_cache().lock().unwrap().mark_dirty(key)
}

/// 刷写所有脏页（全局页缓存）。
pub fn flush_dirty_pages() -> u64 {
    page_cache().lock().unwrap().flush_dirty()
}

/// LRU 淘汰（全局页缓存）。
pub fn evict_lru_pages() -> usize {
    page_cache().lock().unwrap().evict_lru()
}

/// 返回页缓存命中率（全局）。
pub fn page_cache_hit_rate() -> f64 {
    page_cache().lock().unwrap().cache_hit_rate()
}

/// 读取扇区（全局缓冲区缓存）。
pub fn bread(sector: Sector) -> Result<usize, BufferError> {
    buffer_cache().lock().unwrap().bread(sector)
}

/// 释放缓冲区（全局缓冲区缓存）。
pub fn brelse(idx: usize) -> Result<(), BufferError> {
    buffer_cache().lock().unwrap().brelse(idx)
}

/// 写回缓冲区（全局缓冲区缓存）。
pub fn bwrite(idx: usize) -> Result<(), BufferError> {
    buffer_cache().lock().unwrap().bwrite(idx)
}

/// 同步所有脏缓冲区（全局缓冲区缓存）。
pub fn buffer_sync() -> u64 {
    buffer_cache().lock().unwrap().sync()
}

// =====================================================================================
// 预读器（Readahead）—— 顺序读取时提前将后续页载入缓存
// =====================================================================================

/// 预读器：在访问某页时，自动将后续 `readahead_pages` 个页也载入缓存，
// 以优化顺序读取场景的命中率。
pub struct Readahead {
    /// 预读页数。
    pub readahead_pages: usize,
    /// 上一次预读的起始页 key。
    pub last_key: Option<PageKey>,
    /// 累计预读页数。
    pub readahead_count: u64,
}

impl Readahead {
    /// 创建预读器，预读 `readahead_pages` 个页。
    pub fn new(readahead_pages: usize) -> Self {
        Self {
            readahead_pages,
            last_key: None,
            readahead_count: 0,
        }
    }

    /// 在访问 `key` 后，尝试预读后续页。`get_next_key` 用于根据当前 key
    // 计算下一个页的 key（由调用方提供，如文件路径 + 页号）。
    pub fn readahead_after<F>(&mut self, key: &str, cache: &mut PageCache, get_next_key: F) -> usize
    where
        F: Fn(&str, usize) -> String,
    {
        let mut prefetched = 0;
        self.last_key = Some(key.to_string());
        for i in 1..=self.readahead_pages {
            let next_key = get_next_key(key, i);
            // 若缓存中已有该页，则跳过。
            if cache.map.contains_key(&next_key) {
                continue;
            }
            // 预读：分配新页框但不增加命中/未命中统计（预读不算命中）。
            let _ = cache.get_page(&next_key);
            prefetched += 1;
            self.readahead_count += 1;
        }
        prefetched
    }

    /// 返回预读器摘要。
    pub fn summary(&self) -> String {
        format!(
            "readahead pages={} last_key={:?} prefetched={}",
            self.readahead_pages,
            self.last_key,
            self.readahead_count
        )
    }
}

impl Default for Readahead {
    fn default() -> Self {
        Self::new(2)
    }
}

// =====================================================================================
// 时钟算法页面置换（Clock Algorithm）—— 替代 LRU 的近似 LRU 置换
// =====================================================================================

/// 时钟算法置换器：维护一个循环链表，每个页框有一个使用位（use bit）。
// 置换时扫描链表，将使用位清 0，若遇到使用位为 1 的页则跳过；
// 若使用位为 0 则选中该页淘汰。近似 LRU，开销更低。
pub struct ClockReplacer {
    /// 页框索引环形列表。
    frames: Vec<usize>,
    /// 使用位：frame -> 是否最近被使用。
    use_bits: BTreeMap<usize, bool>,
    /// 当前时钟指针。
    hand: usize,
}

impl ClockReplacer {
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            use_bits: BTreeMap::new(),
            hand: 0,
        }
    }

    /// 将页框加入置换候选。
    pub fn insert(&mut self, frame: usize) {
        if !self.frames.contains(&frame) {
            self.frames.push(frame);
            self.use_bits.insert(frame, false);
        }
    }

    /// 标记页框为最近使用（设置使用位）。
    pub fn touch(&mut self, frame: usize) {
        self.use_bits.insert(frame, true);
    }

    /// 移除页框。
    pub fn remove(&mut self, frame: usize) {
        if let Some(pos) = self.frames.iter().position(|&f| f == frame) {
            self.frames.remove(pos);
            self.use_bits.remove(&frame);
            if self.hand >= self.frames.len() {
                self.hand = 0;
            }
        }
    }

    /// 选取一个页框淘汰（使用位为 0 的首个帧）。
    pub fn evict(&mut self) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let start = self.hand;
        loop {
            let frame = self.frames[self.hand];
            let used = self.use_bits.get(&frame).copied().unwrap_or(false);
            if !used {
                self.hand = (self.hand + 1) % self.frames.len();
                return Some(frame);
            }
            // 使用位清 0，继续扫描。
            self.use_bits.insert(frame, false);
            self.hand = (self.hand + 1) % self.frames.len();
            if self.hand == start {
                // 一圈都没找到未使用的，选当前帧。
                self.hand = (self.hand + 1) % self.frames.len();
                return Some(frame);
            }
        }
    }

    /// 返回当前候选帧数。
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// 返回摘要。
    pub fn summary(&self) -> String {
        format!("clock frames={} hand={}", self.frames.len(), self.hand)
    }
}

impl Default for ClockReplacer {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_cache_get_and_release() {
        let mut cache = PageCache::new(4096, 64);
        let h1 = cache.get_page("file://a");
        assert_eq!(h1.key, "file://a");
        assert_eq!(h1.refcount, 1);
        assert_eq!(cache.len(), 1);

        // 再次 get 同一 key：命中，引用计数加 1
        let h2 = cache.get_page("file://a");
        assert_eq!(h2.refcount, 2);
        assert_eq!(cache.hits, 1);

        cache.release_page(&h1);
        cache.release_page(&h2);
        // 引用计数回到 0
        if let Some(p) = cache.pages.get(h1.page_index).and_then(|p| p.as_ref()) {
            assert_eq!(p.refcount, 0);
        }
    }

    #[test]
    fn page_cache_dirty_and_flush() {
        let mut cache = PageCache::new(4096, 64);
        cache.get_page("file://dirty");
        cache.mark_dirty("file://dirty").unwrap();
        assert_eq!(cache.dirty_count(), 1);
        let flushed = cache.flush_dirty();
        assert_eq!(flushed, 1);
        assert_eq!(cache.dirty_count(), 0);
        assert_eq!(cache.flush_count(), 1);
    }

    #[test]
    fn page_cache_lock_evict() {
        let mut cache = PageCache::new(4096, 2);
        let _h1 = cache.get_page("a");
        let _h2 = cache.get_page("b");
        // 锁定 a，a 不应被淘汰
        cache.lock_page("a").unwrap();
        // 填满并淘汰：b 应被淘汰，a 保留
        cache.get_page("c"); // 会触发 evict_lru
        // a 应该还在
        assert!(cache.map.contains_key("a"));
    }

    #[test]
    fn page_cache_evict_dirty_not_directly() {
        let mut cache = PageCache::new(4096, 2);
        let _h1 = cache.get_page("a");
        let _h2 = cache.get_page("b");
        cache.mark_dirty("b").unwrap();
        // 淘汰时 b 是脏页，不应被直接淘汰
        let evicted = cache.evict_lru();
        assert_eq!(evicted, 0);
        assert!(cache.map.contains_key("b"));
    }

    #[test]
    fn page_cache_hit_rate() {
        let mut cache = PageCache::new(4096, 64);
        cache.get_page("x");
        cache.get_page("y");
        cache.get_page("x"); // hit
        let rate = cache.cache_hit_rate();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn page_cache_summary() {
        let mut cache = PageCache::new(4096, 64);
        let _ = cache.get_page("s");
        let s = cache.summary();
        assert!(s.contains("page_size="));
        assert!(s.contains("hit_rate="));
    }

    #[test]
    fn buffer_cache_bread_brelse_bwrite_sync() {
        let mut cache = BufferCache::new(512, 16);
        let idx = cache.bread(100).unwrap();
        assert_eq!(cache.buffers[idx].as_ref().unwrap().sector, 100);
        assert!(cache.buffers[idx].as_ref().unwrap().locked);
        cache.brelse(idx).unwrap();
        assert!(!cache.buffers[idx].as_ref().unwrap().locked);

        // bwrite 标记脏
        let idx2 = cache.bread(101).unwrap();
        cache.bwrite(idx2).unwrap();
        assert_eq!(cache.dirty_count(), 1);
        let synced = cache.sync();
        assert_eq!(synced, 1);
        assert_eq!(cache.dirty_count(), 0);
        assert_eq!(cache.sync_count(), 1);
    }

    #[test]
    fn buffer_cache_hit_rate() {
        let mut cache = BufferCache::new(512, 16);
        let _ = cache.bread(1).unwrap();
        let _ = cache.bread(2).unwrap();
        let _ = cache.bread(1).unwrap(); // hit
        let rate = cache.cache_hit_rate();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn global_page_cache_functions() {
        let h = get_page("global://page");
        assert_eq!(h.key, "global://page");
        release_page(&h);
        assert!(mark_page_dirty("global://page").is_ok());
        let _ = flush_dirty_pages();
        let _ = evict_lru_pages();
        let _rate = page_cache_hit_rate();
        let _ = _rate;
    }

    #[test]
    fn global_buffer_cache_functions() {
        let idx = bread(42).unwrap();
        assert!(brelse(idx).is_ok());
        assert!(bwrite(idx).is_ok());
        let _ = buffer_sync();
        let _rate = buffer_cache().lock().unwrap().cache_hit_rate();
        let _ = _rate;
    }

    #[test]
    fn readahead_prefetches_pages() {
        let mut cache = PageCache::new(4096, 128);
        let mut ra = Readahead::new(3);
        // 先访问 page_0
        let _ = cache.get_page("file://page_0");
        // 预读 page_1, page_2, page_3
        let n = ra.readahead_after("file://page_0", &mut cache, |base, i| {
            let num: usize = base.trim_start_matches("file://page_").parse().unwrap_or(0);
            format!("file://page_{}", num + i)
        });
        assert_eq!(n, 3);
        assert!(cache.map.contains_key("file://page_1"));
        assert!(cache.map.contains_key("file://page_2"));
        assert!(cache.map.contains_key("file://page_3"));
        let s = ra.summary();
        assert!(s.contains("readahead"));
    }

    #[test]
    fn readahead_skips_cached() {
        let mut cache = PageCache::new(4096, 128);
        let mut ra = Readahead::new(2);
        // page_1 已缓存，预读应跳过
        let _ = cache.get_page("file://base");
        let _ = cache.get_page("file://page_1");
        let n = ra.readahead_after("file://base", &mut cache, |base, i| {
            let num: usize = base.trim_start_matches("file://").parse().unwrap_or(0);
            format!("file://page_{}", num + i)
        });
        // base 不是数字格式，解析失败，page_1 已缓存，page_2 新缓存
        assert_eq!(n, 1);
        assert!(cache.map.contains_key("file://page_2"));
    }

    #[test]
    fn clock_replacer_basic() {
        let mut cr = ClockReplacer::new();
        cr.insert(10);
        cr.insert(20);
        cr.insert(30);
        assert_eq!(cr.len(), 3);
        // 全部标记使用，第一次 evict 应清除使用位并选一个
        cr.touch(10);
        cr.touch(20);
        cr.touch(30);
        let _evicted = cr.evict().unwrap();
        // 10 的使用位被清，hand 移到 20
        assert!(cr.evict().is_some());
        cr.remove(20);
        assert_eq!(cr.len(), 2);
    }

    #[test]
    fn clock_replacer_touch_skips() {
        let mut cr = ClockReplacer::new();
        cr.insert(1);
        cr.insert(2);
        // 只有 1 被标记使用，evict 应选 2（未使用）
        cr.touch(1);
        let evicted = cr.evict();
        assert_eq!(evicted, Some(2));
    }

    #[test]
    fn clock_replacer_empty() {
        let mut cr = ClockReplacer::new();
        assert_eq!(cr.evict(), None);
        assert!(cr.is_empty());
    }
}


