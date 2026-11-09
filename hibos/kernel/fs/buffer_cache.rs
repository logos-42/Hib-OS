//! 缓冲区缓存（Buffer Cache，LRU 淘汰 + 脏页写回）。
//!
//! 以扇区为单位缓存块设备数据，提供 `BufferHandle` 句柄与缓冲区锁
//!（锁定/引用计数）。脏块在 `mark_dirty` 后标记，由 `sync_all` /
//! `sync_device` 回写到底层设备。容量满时按 LRU 淘汰最久未使用的干净
//! 缓冲区；脏缓冲区须先回写方可淘汰。
//!
//! 设计参考经典 Unix buf cache：每个扇区对应一个 `Buffer`，句柄持有
//! 引用计数，释放句柄后缓冲区重新进入 LRU 链表。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};

/// 缓存统计信息。
#[derive(Clone, Debug, Default)]
pub struct CacheStats {
    /// 缓存中缓冲区总数。
    pub total_buffers: usize,
    /// 当前被锁定（引用计数 > 0）的缓冲区数。
    pub locked_buffers: usize,
    /// 脏缓冲区数。
    pub dirty_buffers: usize,
    /// 命中次数。
    pub hits: u64,
    /// 未命中次数。
    pub misses: u64,
    /// 淘汰次数。
    pub evictions: u64,
    /// 脏页回写次数。
    pub writebacks: u64,
}

/// 缓冲区状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferState {
    /// 未锁定，可被淘汰。
    Free,
    /// 已锁定（有句柄持有）。
    Locked,
    /// 锁定且为脏（需回写后才能释放）。
    LockedDirty,
}

/// 缓冲区：数据、扇区号、脏标志、锁定标志、引用计数。
#[derive(Clone, Debug)]
pub struct Buffer {
    /// 所属设备 ID。
    pub dev_id: u64,
    /// 扇区号。
    pub sector: u64,
    /// 扇区数据。
    pub data: [u8; BLOCK_SIZE],
    /// 脏标志。
    pub dirty: bool,
    /// 锁定标志。
    pub locked: bool,
    /// 引用计数（句柄持有数）。
    pub refcount: u32,
}

impl Buffer {
    fn new(dev_id: u64, sector: u64) -> Self {
        Self {
            dev_id,
            sector,
            data: [0u8; BLOCK_SIZE],
            dirty: false,
            locked: false,
            refcount: 0,
        }
    }
}

/// 缓冲区句柄：持有对缓冲区的引用，Drop 时自动减引用计数。
pub struct BufferHandle {
    inner: Arc<Mutex<BufferCacheInner>>,
    key: (u64, u64), // (dev_id, sector)
}

impl BufferHandle {
    /// 返回缓冲区数据的副本（4 KiB，按值返回，避免跨锁持有引用）。
    pub fn data(&self) -> [u8; BLOCK_SIZE] {
        let inner = self.inner.lock().unwrap();
        inner.buffers[&self.key].data
    }

    /// 对缓冲区数据应用可变闭包，并自动标记脏。
    pub fn data_mut<F: FnOnce(&mut [u8; BLOCK_SIZE])>(&mut self, f: F) {
        let mut inner = self.inner.lock().unwrap();
        let buf = inner.buffers.get_mut(&self.key).expect("buffer must exist");
        buf.dirty = true;
        f(&mut buf.data);
    }

    /// 返回扇区号。
    pub fn sector(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner.buffers[&self.key].sector
    }

    /// 返回所属设备 ID。
    pub fn dev_id(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner.buffers[&self.key].dev_id
    }

    /// 判断缓冲区是否脏。
    pub fn is_dirty(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.buffers[&self.key].dirty
    }

    /// 返回当前引用计数。
    pub fn refcount(&self) -> u32 {
        let inner = self.inner.lock().unwrap();
        inner.buffers[&self.key].refcount
    }
}

impl Drop for BufferHandle {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        let (dev_id, sector) = {
            let buf = inner.buffers.get(&self.key).expect("buffer must exist");
            (buf.dev_id, buf.sector)
        };
        if let Some(buf) = inner.buffers.get_mut(&self.key) {
            if buf.refcount > 0 {
                buf.refcount -= 1;
            }
            // 引用归零后，缓冲区重新进入可淘汰状态。
            if buf.refcount == 0 {
                buf.locked = false;
                inner.touch_lru((dev_id, sector));
            }
        }
    }
}

/// 缓冲区缓存内部状态。
struct BufferCacheInner {
    /// 按 (dev_id, sector) 索引的缓冲区。
    buffers: HashMap<(u64, u64), Buffer>,
    /// LRU 链表：头部为最久未使用，尾部为最近使用。
    lru: VecDeque<(u64, u64)>,
    /// 容量（最大缓冲区数）。
    capacity: usize,
    stats: CacheStats,
}

impl BufferCacheInner {
    fn new(capacity: usize) -> Self {
        Self {
            buffers: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
            stats: CacheStats::default(),
        }
    }

    /// 将键移到 LRU 尾部（最近使用）。
    fn touch_lru(&mut self, key: (u64, u64)) {
        if let Some(pos) = self.lru.iter().position(|k| *k == key) {
            self.lru.remove(pos);
        }
        self.lru.push_back(key);
    }

    /// 淘汰一个 LRU 最久未使用的干净、非锁定缓冲区。
    /// 返回被淘汰的 (dev_id, sector)。
    fn evict_lru_inner(&mut self) -> Option<(u64, u64)> {
        let len = self.lru.len();
        for _ in 0..len {
            if let Some(key) = self.lru.pop_front() {
                if let Some(buf) = self.buffers.get(&key) {
                    if buf.refcount == 0 && !buf.locked && !buf.dirty {
                        self.buffers.remove(&key);
                        self.stats.evictions += 1;
                        return Some(key);
                    } else {
                        // 不可淘汰，放回尾部。
                        self.lru.push_back(key);
                    }
                }
            }
        }
        None
    }
}

/// 缓冲区缓存错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    /// 设备不存在。
    NoDevice,
    /// 扇区超出设备范围。
    OutOfRange,
    /// 缓冲区正被占用（引用计数 > 0 或锁定）。
    Busy,
    /// 缓冲区为脏，须先回写。
    Dirty,
    /// IO 错误。
    IoError(String),
    /// 找不到缓冲区。
    NotFound,
}

impl std::fmt::Display for BufferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferError::NoDevice => write!(f, "设备不存在"),
            BufferError::OutOfRange => write!(f, "扇区超出设备范围"),
            BufferError::Busy => write!(f, "缓冲区被占用"),
            BufferError::Dirty => write!(f, "缓冲区为脏，须先回写"),
            BufferError::IoError(e) => write!(f, "缓冲区 IO 错误: {}", e),
            BufferError::NotFound => write!(f, "缓冲区不存在"),
        }
    }
}

/// 缓冲区缓存：管理多设备扇区缓存，支持 LRU 淘汰与脏页写回。
pub struct BufferCache {
    inner: Arc<Mutex<BufferCacheInner>>,
    /// 设备表：dev_id -> 块设备。
    devices: Mutex<HashMap<u64, Box<dyn BlockDevice + Send>>>,
    next_dev_id: Mutex<u64>,
}

impl BufferCache {
    /// 创建一个容量为 `max_buffers` 的缓冲区缓存。
    pub fn new(max_buffers: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BufferCacheInner::new(max_buffers))),
            devices: Mutex::new(HashMap::new()),
            next_dev_id: Mutex::new(1),
        }
    }

    /// 注册一个块设备，返回分配的 dev_id。
    pub fn register_device(
        &self,
        device: Box<dyn BlockDevice + Send>,
    ) -> u64 {
        let id = {
            let mut next = self.next_dev_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        self.devices.lock().unwrap().insert(id, device);
        id
    }

    /// 注册设备并指定 dev_id。
    pub fn register_device_with_id(
        &self,
        dev_id: u64,
        device: Box<dyn BlockDevice + Send>,
    ) {
        self.devices.lock().unwrap().insert(dev_id, device);
    }

    /// 获取指定扇区的缓冲区句柄。不存在则从设备读入并缓存。
    ///
    /// 若容量已满，先淘汰 LRU 最久未使用的干净缓冲区。
    pub fn get_blk(
        &self,
        dev_id: u64,
        sector: u64,
    ) -> Result<BufferHandle, BufferError> {
        {
            let mut inner = self.inner.lock().unwrap();
            let key = (dev_id, sector);

            // 命中缓存。
            if let Some(buf) = inner.buffers.get(&key) {
                if buf.refcount > 0 || buf.locked {
                    return Err(BufferError::Busy);
                }
                let buf = inner.buffers.get_mut(&key).unwrap();
                buf.locked = true;
                buf.refcount += 1;
                inner.touch_lru(key);
                inner.stats.hits += 1;
                return Ok(BufferHandle {
                    inner: Arc::clone(&self.inner),
                    key,
                });
            }
        }

        // 未命中：从设备读入（释放缓存锁，避免 IO 时持锁）。
        {
            let mut inner = self.inner.lock().unwrap();
            inner.stats.misses += 1;
        }
        let device = {
            let devs = self.devices.lock().unwrap();
            devs.get(&dev_id)
                .cloned()
                .ok_or(BufferError::NoDevice)?
        };
        let mut data = [0u8; BLOCK_SIZE];
        device
            .read_at(sector, &mut data)
            .map_err(|e| BufferError::IoError(format!("读扇区 {} 失败: {}", sector, e)))?;

        let mut inner = self.inner.lock().unwrap();
        let key = (dev_id, sector);
        // 双重检查：另一线程可能已加载。
        if inner.buffers.contains_key(&key) {
            let buf = inner.buffers.get_mut(&key).unwrap();
            if buf.refcount > 0 || buf.locked {
                return Err(BufferError::Busy);
            }
            buf.locked = true;
            buf.refcount += 1;
            inner.touch_lru(key);
            inner.stats.hits += 1;
            return Ok(BufferHandle {
                inner: Arc::clone(&self.inner),
                key,
            });
        }

        // 容量不足时淘汰。
        while inner.buffers.len() >= inner.capacity {
            if inner.evict_lru_inner().is_none() {
                return Err(BufferError::Busy);
            }
        }
        let mut buf = Buffer::new(dev_id, sector);
        buf.data = data;
        buf.locked = true;
        buf.refcount = 1;
        inner.buffers.insert(key, buf.clone());
        inner.lru.push_back(key);
        Ok(BufferHandle {
            inner: Arc::clone(&self.inner),
            key,
        })
    }

    /// 释放缓冲区句柄（等价于 Drop，但显式调用更清晰）。
    pub fn brelse(&self, handle: BufferHandle) {
        drop(handle);
    }

    /// 标记缓冲区为脏（下次 sync 时回写）。
    pub fn mark_dirty(&self, handle: &mut BufferHandle) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(buf) = inner.buffers.get_mut(&handle.key) {
            buf.dirty = true;
        }
    }

    /// 回写指定设备的全部脏缓冲区（仅回写未被占用的脏块）。
    pub fn sync_device(&self, dev_id: u64) -> Result<usize, BufferError> {
        let mut inner = self.inner.lock().unwrap();
        let dirty_keys: Vec<(u64, u64)> = inner
            .buffers
            .iter()
            .filter(|((d, _), buf)| {
                *d == dev_id && buf.dirty && buf.refcount == 0 && !buf.locked
            })
            .map(|(k, _)| *k)
            .collect();
        let mut done = 0;
        for key in dirty_keys {
            let data = {
                let buf = inner.buffers.get(&key).expect("dirty buffer must exist");
                buf.data
            };
            let device = self
                .devices
                .lock()
                .unwrap()
                .get(&key.0)
                .cloned()
                .ok_or(BufferError::NoDevice)?;
            device
                .write_at(key.1, &data)
                .map_err(|e| BufferError::IoError(format!("回写扇区 {} 失败: {}", key.1, e)))?;
            if let Some(b) = inner.buffers.get_mut(&key) {
                b.dirty = false;
            }
            done += 1;
            inner.stats.writebacks += 1;
        }
        Ok(done)
    }

    /// 回写全部设备的全部脏缓冲区。
    pub fn sync_all(&self) -> Result<usize, BufferError> {
        let dev_ids: Vec<u64> = {
            let inner = self.inner.lock().unwrap();
            inner
                .buffers
                .values()
                .map(|b| b.dev_id)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        let mut total = 0;
        for dev_id in dev_ids {
            total += self.sync_device(dev_id)?;
        }
        Ok(total)
    }

    /// 淘汰一个 LRU 最久未使用的干净缓冲区。
    pub fn evict_lru(&self) -> Result<Option<(u64, u64)>, BufferError> {
        let mut inner = self.inner.lock().unwrap();
        Ok(inner.evict_lru_inner())
    }

    /// 强制淘汰指定扇区的缓冲区（需不被引用、非锁定、非脏）。
    pub fn evict_buffer(&self, dev_id: u64, sector: u64) -> Result<(), BufferError> {
        let mut inner = self.inner.lock().unwrap();
        let key = (dev_id, sector);
        if let Some(buf) = inner.buffers.get(&key) {
            if buf.refcount > 0 || buf.locked {
                return Err(BufferError::Busy);
            }
            if buf.dirty {
                return Err(BufferError::Dirty);
            }
        }
        inner.lru.retain(|k| *k != key);
        inner.buffers.remove(&key);
        Ok(())
    }

    /// 返回缓存统计。
    pub fn cache_stats(&self) -> CacheStats {
        let inner = self.inner.lock().unwrap();
        let total = inner.buffers.len();
        let locked = inner
            .buffers
            .values()
            .filter(|b| b.refcount > 0 || b.locked)
            .count();
        let dirty = inner.buffers.values().filter(|b| b.dirty).count();
        CacheStats {
            total_buffers: total,
            locked_buffers: locked,
            dirty_buffers: dirty,
            hits: inner.stats.hits,
            misses: inner.stats.misses,
            evictions: inner.stats.evictions,
            writebacks: inner.stats.writebacks,
        }
    }

    /// 返回设备数量。
    pub fn device_count(&self) -> usize {
        self.devices.lock().unwrap().len()
    }
}

impl Default for BufferCache {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::block::MemBlockDevice;

    fn write_sector(cache: &BufferCache, dev_id: u64, sector: u64, byte: u8) {
        let mut dev = cache
            .devices
            .lock()
            .unwrap()
            .get_mut(&dev_id)
            .expect("device must exist");
        let mut data = [byte; BLOCK_SIZE];
        dev.write_at(sector, &data).unwrap();
    }

    #[test]
    fn buffer_cache_get_blk_read_write_roundtrip() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 3, 0x5A);
        {
            let mut dev = cache.devices.lock().unwrap().get_mut(&dev_id).unwrap();
            let mut data = [0u8; BLOCK_SIZE];
            data[10] = 0x7F;
            dev.write_at(3, &data).unwrap();
        }
        let handle = cache.get_blk(dev_id, 3).unwrap();
        assert_eq!(handle.data()[10], 0x7F);
        assert_eq!(handle.sector(), 3);
        assert_eq!(handle.dev_id(), dev_id);
        let stats = cache.cache_stats();
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.total_buffers, 1);
    }

    #[test]
    fn buffer_cache_lru_eviction() {
        let cache = BufferCache::new(2);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 0, 0x01);
        write_sector(&cache, dev_id, 1, 0x02);
        write_sector(&cache, dev_id, 2, 0x03);

        let h0 = cache.get_blk(dev_id, 0).unwrap();
        let h1 = cache.get_blk(dev_id, 1).unwrap();
        assert_eq!(h0.data()[0], 0x01);
        assert_eq!(h1.data()[0], 0x02);
        // 缓冲区都被占用，读第 2 扇区应返回 Busy。
        assert!(cache.get_blk(dev_id, 2).is_err());
        drop(h0);
        drop(h1);
        // 释放后可加载。
        let _ = cache.get_blk(dev_id, 2).unwrap();
        let stats = cache.cache_stats();
        assert!(stats.evictions >= 1);
    }

    #[test]
    fn buffer_cache_mark_dirty_and_sync() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 5, 0x00);
        let mut handle = cache.get_blk(dev_id, 5).unwrap();
        handle.data_mut(|d| d[0] = 0xA5);
        cache.mark_dirty(&mut handle);
        let stats = cache.cache_stats();
        assert_eq!(stats.dirty_buffers, 1);
        let written = cache.sync_device(dev_id).unwrap();
        assert_eq!(written, 1);
        let stats = cache.cache_stats();
        assert_eq!(stats.dirty_buffers, 0);
        let handle2 = cache.get_blk(dev_id, 5).unwrap();
        assert_eq!(handle2.data()[0], 0xA5);
    }

    #[test]
    fn buffer_cache_sync_all() {
        let cache = BufferCache::new(16);
        let d1 = cache.register_device(Box::new(MemBlockDevice::new("d1", 64)));
        let d2 = cache.register_device(Box::new(MemBlockDevice::new("d2", 64)));
        write_sector(&cache, d1, 0, 0x00);
        write_sector(&cache, d2, 0, 0x00);
        let mut h1 = cache.get_blk(d1, 0).unwrap();
        h1.data_mut(|d| d[0] = 0x33);
        cache.mark_dirty(&mut h1);
        let mut h2 = cache.get_blk(d2, 0).unwrap();
        h2.data_mut(|d| d[0] = 0x44);
        cache.mark_dirty(&mut h2);
        let total = cache.sync_all().unwrap();
        assert_eq!(total, 2);
        let stats = cache.cache_stats();
        assert_eq!(stats.dirty_buffers, 0);
        assert_eq!(stats.writebacks, 2);
    }

    #[test]
    fn buffer_cache_dirty_not_evicted() {
        let cache = BufferCache::new(2);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 0, 0x01);
        write_sector(&cache, dev_id, 1, 0x02);

        let mut h0 = cache.get_blk(dev_id, 0).unwrap();
        h0.data_mut(|d| d[0] = 0x09);
        cache.mark_dirty(&mut h0);
        let _h1 = cache.get_blk(dev_id, 1).unwrap();
        // h0 脏且占用，h1 占用，无可淘汰。
        assert!(cache.evict_lru().unwrap().is_none());
        drop(h0);
        // h0 释放但仍脏，不应被淘汰。
        assert!(cache.evict_lru().unwrap().is_none());
    }

    #[test]
    fn buffer_cache_brelse_releases() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 7, 0x00);
        let handle = cache.get_blk(dev_id, 7).unwrap();
        assert_eq!(cache.cache_stats().locked_buffers, 1);
        cache.brelse(handle);
        assert_eq!(cache.cache_stats().locked_buffers, 0);
    }

    #[test]
    fn buffer_cache_device_not_found() {
        let cache = BufferCache::new(16);
        assert_eq!(
            cache.get_blk(999, 0).unwrap_err(),
            BufferError::NoDevice
        );
    }

    #[test]
    fn buffer_cache_stats_default() {
        let cache = BufferCache::new(8);
        let stats = cache.cache_stats();
        assert_eq!(stats.total_buffers, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
    }

    #[test]
    fn buffer_cache_handle_data_mut_marks_dirty() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 9, 0x00);
        let mut handle = cache.get_blk(dev_id, 9).unwrap();
        handle.data_mut(|d| d[0] = 0x55);
        // data_mut 自动标记脏，即使不调 mark_dirty 也应视为脏。
        assert!(handle.is_dirty());
        let stats = cache.cache_stats();
        assert_eq!(stats.dirty_buffers, 1);
    }
}
