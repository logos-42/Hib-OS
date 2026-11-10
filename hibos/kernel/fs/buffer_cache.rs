//! 缓冲区缓存（Buffer Cache，LRU 淘汰 + 脏页写回）。
//!
//! 以扇区为单位缓存块设备数据，提供 `BufferHandle` 句柄与缓冲区锁
//!（锁定/引用计数）。脏块在 `mark_dirty` 后标记，由 `sync_all` /
//! `sync_device` 回写到底层设备。容量满时按 LRU 淘汰最久未使用的干净
//! 缓冲区；脏缓冲区须先回写方可淘汰。
//!
//! ## 设计信条
//! - **缓冲区接口**：`get_blk(sector)` / `brelse(buf)` / `mark_dirty(buf)` / `sync()`，
//!   供文件系统与块设备层调用。
//! - **单写者租约 + epoch CAS**：缓冲区脏页回写前先 `lease::acquire` 获取写租约，
//!   `sync` 时用 `lease::commit` 做乐观校验，避免多回写线程竞争。读取路径
//!   直接快照读，不阻塞。
//! - **能力位权限**：通过 `caps::CAP_SYS_RAWIO` / `caps::CAP_SYS_ADMIN` 校验缓存
//!   回写与设备注册权限，而非 uid。
//! - **通道接口**：提供 `probe_buffer_cache` / `act_buffer_cache`，供 `sense`/`act`
//!   框架以 `buffercache://<设备>` 通道形式观测缓存统计。
//!
//! 设计参考经典 Unix buf cache：每个扇区对应一个 `Buffer`，句柄持有
//! 引用计数，释放句柄后缓冲区重新进入 LRU 链表。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};
use crate::lease::{self, Epoch};
use crate::caps;

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

// ============================================================================
// 并发控制：基于 lease.rs 的单写者租约 + epoch CAS
// ============================================================================

/// 基于 lease 的缓冲区缓存包装器：对 `sync_device`/`sync_all` 的脏页回写
/// 加单写者租约保护，避免多回写线程竞争同一设备的脏块。
///
/// 读取路径（get_blk）不需要租约；回写路径必须先 `acquire_sync_lease`，
/// 提交时用 `commit_sync` 做乐观 epoch CAS。
pub struct BufferCacheLeased {
    inner: BufferCache,
    channel: String,
}

impl BufferCacheLeased {
    /// 创建租约保护的缓冲区缓存。
    pub fn new(max_buffers: usize, channel: &str) -> Self {
        Self {
            inner: BufferCache::new(max_buffers),
            channel: channel.to_string(),
        }
    }

    /// 注册设备（委托内部缓存）。
    pub fn register_device(
        &self,
        device: Box<dyn BlockDevice + Send>,
    ) -> u64 {
        self.inner.register_device(device)
    }

    /// 获取缓冲区句柄（读路径，无需租约）。
    pub fn get_blk(
        &self,
        dev_id: u64,
        sector: u64,
    ) -> Result<crate::fs::buffer_cache::BufferHandle, crate::fs::buffer_cache::BufferError> {
        self.inner.get_blk(dev_id, sector)
    }

    /// 申请回写租约，返回当前 epoch。
    pub fn acquire_sync_lease(&self, owner: u64) -> Result<Epoch, crate::lease::LeaseConflict> {
        lease::acquire(&self.channel, crate::exec::ExecId(owner), None)
    }

    /// 释放回写租约。
    pub fn release_sync_lease(&self, owner: u64) -> bool {
        lease::release(&self.channel, crate::exec::ExecId(owner))
    }

    /// 在租约保护下回写指定设备的脏块，然后 commit 推进 epoch。
    pub fn sync_device_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        dev_id: u64,
    ) -> Result<Epoch, crate::lease::CommitError> {
        // 先确保持有租约。
        let _ = self.acquire_sync_lease(owner)?;
        let done = self.inner.sync_device(dev_id)?;
        // 提交：epoch 必须等于 expected，否则拒绝（乐观 CAS）。
        lease::commit(
            &self.channel,
            crate::exec::ExecId(owner),
            expected_epoch,
            &format!("sync_device dev={} wrote={}", dev_id, done),
        )
    }

    /// 回写全部设备并 commit。
    pub fn sync_all_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
    ) -> Result<Epoch, crate::lease::CommitError> {
        let _ = self.acquire_sync_lease(owner)?;
        let done = self.inner.sync_all()?;
        lease::commit(
            &self.channel,
            crate::exec::ExecId(owner),
            expected_epoch,
            &format!("sync_all wrote={}", done),
        )
    }

    /// 强制刷新（force 模式，绕过租约，用于 root/治理路径）。
    pub fn sync_all_force(&self, owner: u64) -> Epoch {
        let _ = self.inner.sync_all();
        lease::force(&self.channel, &format!("sync_all_force owner={}", owner))
    }

    /// 返回缓存统计。
    pub fn cache_stats(&self) -> crate::fs::buffer_cache::CacheStats {
        self.inner.cache_stats()
    }

    /// 返回当前 epoch。
    pub fn epoch(&self) -> Epoch {
        lease::epoch(&self.channel)
    }
}

/// 能力位权限包装器：在缓冲区缓存操作上加 `caps` 校验。
///
/// `CAP_SYS_RAWIO` 允许读取缓存；`CAP_SYS_ADMIN` 允许回写（sync）与注册设备。
pub struct BufferCacheCaps {
    inner: BufferCache,
    caps: Arc<Mutex<caps::Caps>>,
}

impl BufferCacheCaps {
    /// 包装一个已有缓存，绑定全局能力表。
    pub fn new(cache: BufferCache) -> Self {
        Self {
            inner: cache,
            caps: Arc::new(Mutex::new(caps::caps().lock().unwrap().clone())),
        }
    }

    fn check_rawio(&self) -> Result<(), String> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_RAWIO") {
            Ok(())
        } else {
            Err("缺少 CAP_SYS_RAWIO 能力".to_string())
        }
    }

    fn check_admin(&self) -> Result<(), String> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_ADMIN") {
            Ok(())
        } else {
            Err("缺少 CAP_SYS_ADMIN 能力".to_string())
        }
    }

    /// 注册设备（需 CAP_SYS_ADMIN）。
    pub fn register_device(
        &self,
        device: Box<dyn BlockDevice + Send>,
    ) -> Result<u64, String> {
        self.check_admin()?;
        Ok(self.inner.register_device(device))
    }

    /// 获取缓冲区（需 CAP_SYS_RAWIO）。
    pub fn get_blk(
        &self,
        dev_id: u64,
        sector: u64,
    ) -> Result<crate::fs::buffer_cache::BufferHandle, String> {
        self.check_rawio()?;
        self.inner.get_blk(dev_id, sector).map_err(|e| e.to_string())
    }

    /// 标记脏（需 CAP_SYS_RAWIO，因脏页最终要回写）。
    pub fn mark_dirty(&self, handle: &mut crate::fs::buffer_cache::BufferHandle) -> Result<(), String> {
        self.check_rawio()?;
        self.inner.mark_dirty(handle);
        Ok(())
    }

    /// 回写指定设备（需 CAP_SYS_ADMIN）。
    pub fn sync_device(&self, dev_id: u64) -> Result<usize, String> {
        self.check_admin()?;
        self.inner.sync_device(dev_id).map_err(|e| e.to_string())
    }

    /// 回写全部（需 CAP_SYS_ADMIN）。
    pub fn sync_all(&self) -> Result<usize, String> {
        self.check_admin()?;
        self.inner.sync_all().map_err(|e| e.to_string())
    }

    /// 返回统计（无需能力，仅元数据）。
    pub fn cache_stats(&self) -> crate::fs::buffer_cache::CacheStats {
        self.inner.cache_stats()
    }
}

// ============================================================================
// sense/act 通道接口：buffercache://<设备名>
// ============================================================================

/// 感知缓冲区缓存通道：返回缓存统计快照。
////// 通道名形式：`buffercache://<设备名>`（设备名可为 "all" 查看全部）
pub fn probe_buffer_cache(name: &str) -> Option<String> {
    // 此处为无状态探针：返回通道级元数据与当前 epoch。
    // 实际缓存实例由调用方持有，探针仅报告通道存在性与版本。
    let channel = format!("buffercache://{}", name);
    let epoch = lease::epoch(&channel);
    Some(format!(
        "channel={} type=buffer_cache epoch={} ops=get_blk|brelse|mark_dirty|sync_device|sync_all",
        name, epoch
    ))
}
/// 对缓冲区缓存通道执行行动。
////// 支持子命令：
/// - `stats` —— 返回缓存统计摘要（占位，实际需关联缓存实例）
/// - `epoch` —— 返回当前 epoch
pub fn act_buffer_cache(name: &str, payload: &str) -> Option<Result<String, String>> {
    let mut tokens = payload.split_whitespace();
    let sub = tokens.next().unwrap_or("");
    let channel = format!("buffercache://{}", name);
    match sub {
        "stats" => {
            let _ = tokens.next(); // 占位：实际应关联具体 BufferCache 实例
            Some(Ok(format!(
                "buffercache://{} stats=placeholder epoch={}",
                name,
                lease::epoch(&channel)
            )))
        }
        "epoch" => Some(Ok(format!("epoch={}", lease::epoch(&channel)))),
        _ => Some(Err("用法: stats | epoch".to_string())),
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

    // ----- lease 并发控制测试 -----

    #[test]
    fn buffer_cache_leased_single_writer() {
        let cache = BufferCacheLeased::new(16, "buffercache://test-lease");
        cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        assert!(cache.acquire_sync_lease(101).is_ok());
        let err = cache.acquire_sync_lease(102).unwrap_err();
        assert_eq!(err.holder, crate::exec::ExecId(101));
        assert!(cache.release_sync_lease(101));
        assert!(cache.acquire_sync_lease(102).is_ok());
        assert!(cache.release_sync_lease(102));
    }

    #[test]
    fn buffer_cache_leased_sync_commit_epoch() {
        let cache = BufferCacheLeased::new(16, "buffercache://test-sync");
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector_inner(&cache, dev_id, 3, 0x00);
        let ep0 = cache.epoch();
        let ep1 = cache.acquire_sync_lease(201).unwrap();
        assert_eq!(ep1, ep0); // acquire 不推进 epoch
        let mut data = [0u8; BLOCK_SIZE];
        data[0] = 0xAA;
        // 直接写设备（模拟脏页），然后 sync commit。
        {
            let mut dev = cache.inner.devices.lock().unwrap().get_mut(&dev_id).unwrap();
            dev.write_at(3, &data).unwrap();
        }
        let new_ep = cache.sync_device_under_lease(201, ep0, dev_id).unwrap();
        assert_eq!(new_ep, ep0 + 1);
        assert!(cache.release_sync_lease(201));
    }

    #[test]
    fn buffer_cache_leased_stale_epoch_rejected() {
        let cache = BufferCacheLeased::new(16, "buffercache://test-stale");
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        let ep = cache.acquire_sync_lease(301).unwrap();
        let _ = cache.sync_device_under_lease(301, ep, dev_id).unwrap();
        // 再用旧 epoch commit 应被拒绝。
        let ep2 = cache.acquire_sync_lease(301).unwrap();
        let err = cache.sync_device_under_lease(301, ep, dev_id).unwrap_err();
        assert!(matches!(err, crate::lease::CommitError::StaleEpoch { .. }));
        let _ = cache.release_sync_lease(301);
    }

    #[test]
    fn buffer_cache_leased_force_sync() {
        let cache = BufferCacheLeased::new(16, "buffercache://test-force");
        let ep0 = cache.epoch();
        let ep1 = cache.sync_all_force(401);
        assert!(ep1 > ep0);
    }

    // ----- caps 权限测试 -----

    #[test]
    fn buffer_cache_caps_rawio_required() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        let caps_cache = BufferCacheCaps::new(cache);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_RAWIO");
        }
        assert!(caps_cache.get_blk(dev_id, 0).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_RAWIO");
        }
        assert!(caps_cache.get_blk(dev_id, 0).is_ok());
    }

    #[test]
    fn buffer_cache_caps_admin_required_for_sync() {
        let cache = BufferCache::new(16);
        let dev_id = cache.register_device(Box::new(MemBlockDevice::new("ram0", 64)));
        write_sector(&cache, dev_id, 0, 0x00);
        let mut handle = cache.get_blk(dev_id, 0).unwrap();
        handle.data_mut(|d| d[0] = 0x55);
        cache.mark_dirty(&mut handle);
        let caps_cache = BufferCacheCaps::new(cache);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_cache.sync_device(dev_id).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_cache.sync_device(dev_id).is_ok());
    }

    // ----- sense/act 通道测试 -----

    #[test]
    fn buffer_cache_probe_channel() {
        let snap = probe_buffer_cache("ram0").unwrap();
        assert!(snap.contains("buffercache://ram0"));
        assert!(snap.contains("epoch="));
    }

    #[test]
    fn buffer_cache_act_channel_epoch() {
        let r = act_buffer_cache("ram0", "epoch").unwrap().unwrap();
        assert!(r.contains("epoch="));
        let r = act_buffer_cache("ram0", "stats").unwrap().unwrap();
        assert!(r.contains("stats=placeholder"));
    }
}

/// 向缓存的设备直接写入一个扇区（测试辅助，绕过句柄）。
fn write_sector_inner(cache: &BufferCacheLeased, dev_id: u64, sector: u64, byte: u8) {
    let mut dev = cache
        .inner
        .devices
        .lock()
        .unwrap()
        .get_mut(&dev_id)
        .expect("device must exist");
    let mut data = [byte; BLOCK_SIZE];
    dev.write_at(sector, &data).unwrap();
}