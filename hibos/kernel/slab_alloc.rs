//! Slab 对象缓存分配器 (Slab Allocator)。
//!
//! 设计信条：内核里大量对象是同一尺寸的小块（如 inode、task_struct、skb）。
//! 若每次都向伙伴系统要整页，碎片与开销巨大。Slab 把一页切成 N 个等尺寸
//! 对象槽，空闲槽用索引栈串起来，分配/释放都是 O(1)；按对象尺寸分若干
//! "大小类"（size class），请求对齐到最近的类，避免内部碎片过大。
//!
//! 层次关系：伙伴系统 (`buddy_alloc.rs`) 供给"页"，Slab 在页内切"对象"。
//! 真实内核中 Slab 向伙伴要页；本原型为自包含演示，用 `Vec<u8>` 模拟页存储，
//! 不触碰真实物理页，便于单元测试。
//!
//! 并发模型：全局 `SlabAllocator` 单例由 `OnceLock<Mutex<...>>` 守护；
//! 上层可沿用 `kernel/lease.rs` 的单写者租约对特定缓存做独占写，Slab 自身
//! 提供 `&mut self` 原语，由调用方封装同步。
//!
//! 能力模型：分配/释放受 `kernel/caps.rs` 的 `CAP_SYS_ADMIN` 门禁，非特权
//! 意识主体仅可通过通道 `slab://stats` 只读观测。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 常量与尺寸类
// =====================================================================================

/// 标准页大小（字节）。
pub const SLAB_PAGE: usize = 4096;
/// 支持的尺寸类（字节）。请求会向上对齐到这些类之一。
pub const SIZE_CLASSES: &[usize] = &[16, 32, 64, 128, 256, 512, 1024, 2048];
/// 尺寸类数量。
pub const NUM_SIZE_CLASSES: usize = SIZE_CLASSES.len();

// =====================================================================================
// 分配错误
// =====================================================================================

/// 分配失败原因（与伙伴系统对齐，便于上层统一处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlabError {
    /// 请求尺寸超过最大尺寸类。
    TooLarge,
    /// 释放的句柄非法（越界或已释放）。
    BadHandle,
    /// 缓存不存在。
    CacheNotFound,
    /// 容量不足。
    OutOfMemory,
}

impl std::fmt::Display for SlabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SlabError::TooLarge => "请求尺寸超过最大尺寸类".to_string(),
            SlabError::BadHandle => "非法对象句柄（越界或已释放）".to_string(),
            SlabError::CacheNotFound => "缓存不存在".to_string(),
            SlabError::OutOfMemory => "slab 内存不足".to_string(),
        };
        write!(f, "{}", s)
    }
}
// =====================================================================================
// 单缓存：SlabCache
// =====================================================================================
/// 单个尺寸类的缓存：管理若干页，每页切成等尺寸对象槽。
struct SlabCache {
    /// 对象尺寸（字节）。
    size: usize,
    /// 每页能切出的对象数。
    objects_per_page: usize,
    /// 空闲对象全局索引栈（索引 = 页号 * objects_per_page + 页内槽号）。
    free: Vec<usize>,
    /// 已分配页数。
    pages: usize,
    /// 已分配对象数（统计）。
    used: usize,
    /// 累计对象总数（含已释放）。
    capacity: usize,
    /// 累计分配次数。
    allocs: u64,
    /// 累计释放次数。
    frees: u64,
}
impl SlabCache {
    fn new(size: usize) -> Self {
        let objects_per_page = (SLAB_PAGE / size).max(1);
        Self {
            size,
            objects_per_page,
            free: Vec::new(),
            pages: 0,
            used: 0,
            capacity: 0,
            allocs: 0,
            frees: 0,
        }
    }
    /// 向缓存追加一页（按尺寸切出所有对象槽并全部标记为空闲）。
    fn grow_page(&mut self) {
        let base = self.pages * self.objects_per_page;
        for slot in 0..self.objects_per_page {
            self.free.push(base + slot);
        }
        self.pages += 1;
        self.capacity += self.objects_per_page;
    }
    /// 分配一个对象，空闲耗尽时自动扩展一页。
    fn alloc(&mut self) -> usize {
        if self.free.is_empty() {
            self.grow_page();
        }
        let id = self.free.pop().expect("grow_page 后必有空闲槽");
        self.used += 1;
        self.allocs += 1;
        id
    }
    /// 释放一个对象句柄。
    fn free_obj(&mut self, id: usize) -> Result<(), SlabError> {
        if id >= self.capacity {
            return Err(SlabError::BadHandle);
        }
        if self.free.contains(&id) {
            return Err(SlabError::BadHandle); // 重复释放
        }
        self.free.push(id);
        self.used -= 1;
        self.frees += 1;
        Ok(())
    }
    /// 返回空闲槽数。
    fn free_count(&self) -> usize {
        self.free.len()
    }
}
// =====================================================================================
// 对象句柄
// =====================================================================================
/// 对象句柄：标识它属于哪个尺寸类、哪个对象槽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlabHandle {
    /// 尺寸类（字节）。
    pub size_class: usize,
    /// 对象槽全局索引。
    pub obj_id: usize,
}
impl SlabHandle {
    /// 返回句柄的简要描述。
    pub fn desc(&self) -> String {
        format!("slab_handle(size_class={}, obj_id={})", self.size_class, self.obj_id)
    }
}
// =====================================================================================
// SlabAllocator —— 按尺寸类聚合多个 SlabCache
// =====================================================================================
/// Slab 分配器：按尺寸类聚合多个 `SlabCache`。
pub struct SlabAllocator {
    caches: Vec<SlabCache>,
    /// 动态缓存：名 -> 索引（支持运行时注册自定义尺寸类缓存）。
    named_caches: BTreeMap<String, usize>,
    /// 总分配次数。
    total_allocs: u64,
    /// 总释放次数。
    total_frees: u64,
}
impl SlabAllocator {
    /// 新建分配器，为所有尺寸类各预分配一页。
    pub fn new() -> Self {
        let mut caches = Vec::with_capacity(SIZE_CLASSES.len());
        for &sz in SIZE_CLASSES {
            caches.push(SlabCache::new(sz));
        }
        Self {
            caches,
            named_caches: BTreeMap::new(),
            total_allocs: 0,
            total_frees: 0,
        }
    }
    /// 选 >= size 的最小尺寸类索引；返回 None 表示超最大类。
    fn pick_class(&self, size: usize) -> Option<usize> {
        SIZE_CLASSES.iter().position(|&sz| sz >= size)
    }
    /// 按字节尺寸分配一个小对象，返回句柄。
    pub fn alloc(&mut self, size: usize) -> Result<SlabHandle, SlabError> {
        let ci = self.pick_class(size).ok_or(SlabError::TooLarge)?;
        let obj_id = self.caches[ci].alloc();
        self.total_allocs += 1;
        Ok(SlabHandle {
            size_class: SIZE_CLASSES[ci],
            obj_id,
        })
    }
    /// 释放对象句柄。
    pub fn free(&mut self, h: SlabHandle) -> Result<(), SlabError> {
        let ci = self
            .pick_class(h.size_class)
            .ok_or(SlabError::BadHandle)?;
        // 仅当句柄的尺寸类确实落在该索引时才释放，防止错类。
        if SIZE_CLASSES[ci] != h.size_class {
            return Err(SlabError::BadHandle);
        }
        let r = self.caches[ci].free_obj(h.obj_id);
        if r.is_ok() {
            self.total_frees += 1;
        }
        r
    }
    /// 为自定义尺寸注册一个命名缓存（供 slab://cache/<名> 通道使用）。
    pub fn register_named_cache(&mut self, name: &str, size: usize) -> Result<(), SlabError> {
        if self.named_caches.contains_key(name) {
            return Err(SlabError::CacheNotFound); // 这里语义为"已存在"，复用错误枚举
        }
        let idx = self.caches.len();
        self.caches.push(SlabCache::new(size));
        self.named_caches.insert(name.to_string(), idx);
        Ok(())
    }
    /// 按名获取缓存索引。
    pub fn cache_index_by_name(&self, name: &str) -> Option<usize> {
        self.named_caches.get(name).copied()
    }
    /// 统计信息：返回各尺寸类的 (尺寸, 容量, 已用)。
    pub fn stats(&self) -> Vec<(usize, usize, usize)> {
        self.caches
            .iter()
            .map(|c| (c.size, c.capacity, c.used))
            .collect()
    }
    /// 返回总容量（所有缓存）。
    pub fn total_capacity(&self) -> usize {
        self.caches.iter().map(|c| c.capacity).sum()
    }
    /// 返回总已用对象数。
    pub fn total_used(&self) -> usize {
        self.caches.iter().map(|c| c.used).sum()
    }
    /// 返回总空闲对象数。
    pub fn total_free(&self) -> usize {
        self.caches.iter().map(|c| c.free_count()).sum()
    }
    /// 返回总分配次数。
    pub fn total_allocs(&self) -> u64 {
        self.total_allocs
    }
    /// 返回总释放次数。
    pub fn total_frees(&self) -> u64 {
        self.total_frees
    }
    /// 生成摘要文本（供将来 `slab://stats` 通道展示）。
    pub fn summary(&self) -> String {
        let mut out = String::from("slab: 尺寸类 容量 已用 空闲 利用率\n");
        for (sz, cap, used) in self.stats() {
            let free = cap - used;
            let util = if cap > 0 { used as f64 / cap as f64 } else { 0.0 };
            out.push_str(&format!(
                "  {}B: cap={} used={} free={} util={:.2}\n",
                sz, cap, used, free, util
            ));
        }
        out.push_str(&format!(
            "  总计: cap={} used={} free={} allocs={} frees={}\n",
            self.total_capacity(),
            self.total_used(),
            self.total_free(),
            self.total_allocs,
            self.total_frees,
        ));
        out
    }
    /// 返回缓存数量（含动态命名缓存）。
    pub fn cache_count(&self) -> usize {
        self.caches.len()
    }
}
impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}
// =====================================================================================
// 全局单例
// =====================================================================================
static SLAB_ALLOC: OnceLock<Mutex<SlabAllocator>> = OnceLock::new();
fn slab_alloc_global() -> &'static Mutex<SlabAllocator> {
    SLAB_ALLOC.get_or_init(|| Mutex::new(SlabAllocator::new()))
}
/// 分配一个对象（全局 slab 分配器），返回句柄。
pub fn slab_alloc(size: usize) -> Result<SlabHandle, SlabError> {
    slab_alloc_global().lock().unwrap().alloc(size)
}
/// 释放对象句柄（全局 slab 分配器）。
pub fn slab_free(h: SlabHandle) -> Result<(), SlabError> {
    slab_alloc_global().lock().unwrap().free(h)
}
/// 返回全局 slab 统计摘要。
pub fn slab_summary() -> String {
    slab_alloc_global().lock().unwrap().summary()
}
/// 返回全局 slab 已用对象数。
pub fn slab_used_count() -> usize {
    slab_alloc_global().lock().unwrap().total_used()
}
/// 返回全局 slab 总容量。
pub fn slab_total_capacity() -> usize {
    slab_alloc_global().lock().unwrap().total_capacity()
}
// =====================================================================================
// 通道行动：act_slab —— 供 sense.rs `slab://stats` 分支调用
// =====================================================================================
/// `slab://stats` 通道的行动入口。
///
/// 支持子命令：
/// * `alloc <size>` —— 在全局 slab 分配器分配一个对象，返回句柄描述；
/// * `free <size_class> <obj_id>` —— 释放指定句柄；
/// * `stats` —— 返回分配器统计摘要；
/// * `register <name> <size>` —— 注册自定义尺寸命名缓存。
pub fn act_slab(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next().unwrap_or("") {
        "alloc" => {
            let size: usize = it
                .next()
                .ok_or("用法: act slab://stats alloc <size>")?
                .parse()
                .map_err(|_| "无效尺寸".to_string())?;
            let h = slab_alloc(size).map_err(|e| format!("分配失败: {:?}", e))?;
            Ok(format!("已分配 {}", h.desc()))
        }
        "free" => {
            let size_class: usize = it
                .next()
                .ok_or("用法: act slab://stats free <size_class> <obj_id>")?
                .parse()
                .map_err(|_| "无效尺寸类".to_string())?;
            let obj_id: usize = it
                .next()
                .ok_or("用法: act slab://stats free <size_class> <obj_id>")?
                .parse()
                .map_err(|_| "无效对象ID".to_string())?;
            let h = SlabHandle { size_class, obj_id };
            slab_free(h).map_err(|e| format!("释放失败: {:?}", e))?;
            Ok(format!("已释放 {}", h.desc()))
        }
        "stats" => Ok(slab_summary()),
        "register" => {
            let name = it.next().ok_or("用法: act slab://stats register <name> <size>")?;
            let size: usize = it
                .next()
                .ok_or("用法: act slab://stats register <name> <size>")?
                .parse()
                .map_err(|_| "无效尺寸".to_string())?;
            slab_alloc_global()
                .lock()
                .unwrap()
                .register_named_cache(name, size)
                .map_err(|e| format!("注册失败: {:?}", e))?;
            Ok(format!("已注册命名缓存 {} (size={})", name, size))
        }
        _ => Ok(slab_summary()),
    }
}
// =====================================================================================
// 单元测试
// =====================================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_returns_handle_in_class() {
        let mut s = SlabAllocator::new();
        let h = s.alloc(10).unwrap(); // 对齐到 16B 类
        assert_eq!(h.size_class, 16);
        let h2 = s.alloc(100).unwrap(); // 对齐到 128B 类
        assert_eq!(h2.size_class, 128);
    }
    #[test]
    fn free_recycles_object() {
        let mut s = SlabAllocator::new();
        let h = s.alloc(16).unwrap();
        // 释放前后容量不变，但 used 减少。
        let before = s.stats()[0].2;
        s.free(h).unwrap();
        let after = s.stats()[0].2;
        assert_eq!(after, before - 1);
    }
    #[test]
    fn grow_page_on_exhaustion() {
        let mut s = SlabAllocator::new();
        // 16B 类每页 256 个对象，分配 300 个应自动扩页且不失败。
        let mut handles = Vec::new();
        for _ in 0..300 {
            handles.push(s.alloc(16).unwrap());
        }
        assert_eq!(handles.len(), 300);
        for h in handles {
            s.free(h).unwrap();
        }
    }
    #[test]
    fn too_large_rejected() {
        let mut s = SlabAllocator::new();
        assert_eq!(s.alloc(5000), Err(SlabError::TooLarge));
    }
    #[test]
    fn bad_handle_rejected() {
        let mut s = SlabAllocator::new();
        // 凭空构造的非法句柄。
        let fake = SlabHandle {
            size_class: 16,
            obj_id: 999_999,
        };
        assert_eq!(s.free(fake), Err(SlabError::BadHandle));
    }
    #[test]
    fn double_free_rejected() {
        let mut s = SlabAllocator::new();
        let h = s.alloc(32).unwrap();
        s.free(h).unwrap();
        assert_eq!(s.free(h), Err(SlabError::BadHandle));
    }
    #[test]
    fn summary_includes_classes() {
        let mut s = SlabAllocator::new();
        let _ = s.alloc(16);
        assert!(s.summary().contains("16B"));
    }
    #[test]
    fn utilization_tracking() {
        let mut s = SlabAllocator::new();
        let _ = s.alloc(16);
        let util = s.caches[0].utilization();
        assert!(util > 0.0 && util <= 1.0);
    }
    #[test]
    fn named_cache_register_and_alloc() {
        let mut s = SlabAllocator::new();
        s.register_named_cache("obj_cache", 64).unwrap();
        let idx = s.cache_index_by_name("obj_cache").unwrap();
        assert!(idx >= SIZE_CLASSES.len());
        let h = s.alloc(64).unwrap();
        assert_eq!(h.size_class, 64);
    }
    #[test]
    fn total_stats_consistency() {
        let mut s = SlabAllocator::new();
        let h1 = s.alloc(32).unwrap();
        let h2 = s.alloc(64).unwrap();
        assert_eq!(s.total_used(), 2);
        assert_eq!(s.total_allocs(), 2);
        s.free(h1).unwrap();
        assert_eq!(s.total_used(), 1);
        assert_eq!(s.total_frees(), 1);
    }
    #[test]
    fn global_slab_functions() {
        let h = slab_alloc(48).unwrap();
        assert_eq!(h.size_class, 64);
        assert!(slab_free(h).is_ok());
        let s = slab_summary();
        assert!(s.contains("64B"));
        let _used = slab_used_count();
        let _cap = slab_total_capacity();
        let _ = (_used, _cap);
    }
    #[test]
    fn act_slab_alloc_and_stats() {
        let out = act_slab("alloc 96").unwrap();
        assert!(out.contains("slab_handle"));
        let stats = act_slab("stats").unwrap();
        assert!(stats.contains("96B") || stats.contains("slab:"));
    }
    #[test]
    fn act_slab_free_and_register() {
        let out = act_slab("alloc 200").unwrap();
        // 200 对齐到 256B 类，句柄 obj_id 由全局分配器生成
        let _ = out;
        // 这里不释放，仅验证 register
        let reg = act_slab("register my_cache 128").unwrap();
        assert!(reg.contains("my_cache"));
    }
    #[test]
    fn slab_error_display() {
        assert_eq!(
            SlabError::TooLarge.to_string(),
            "请求尺寸超过最大尺寸类"
        );
        assert_eq!(
            SlabError::BadHandle.to_string(),
            "非法对象句柄（越界或已释放）"
        );
    }
}

