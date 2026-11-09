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

/// 标准页大小（字节）。
pub const SLAB_PAGE: usize = 4096;
/// 支持的尺寸类（字节）。请求会向上对齐到这些类之一。
pub const SIZE_CLASSES: &[usize] = &[16, 32, 64, 128, 256, 512, 1024, 2048];

/// 分配失败原因（与伙伴系统对齐，便于上层统一处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlabError {
    /// 请求尺寸超过最大尺寸类。
    TooLarge,
    /// 释放的句柄非法（越界或已释放）。
    BadHandle,
}

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
        Ok(())
    }
}

/// 对象句柄：标识它属于哪个尺寸类、哪个对象槽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlabHandle {
    /// 尺寸类（字节）。
    pub size_class: usize,
    /// 对象槽全局索引。
    pub obj_id: usize,
}

/// Slab 分配器：按尺寸类聚合多个 `SlabCache`。
pub struct SlabAllocator {
    caches: Vec<SlabCache>,
}

impl SlabAllocator {
    /// 新建分配器，为所有尺寸类各预分配一页。
    pub fn new() -> Self {
        let mut caches = Vec::with_capacity(SIZE_CLASSES.len());
        for &sz in SIZE_CLASSES {
            caches.push(SlabCache::new(sz));
        }
        Self { caches }
    }

    /// 选 >= size 的最小尺寸类索引；返回 None 表示超最大类。
    fn pick_class(&self, size: usize) -> Option<usize> {
        SIZE_CLASSES.iter().position(|&sz| sz >= size)
    }

    /// 按字节尺寸分配一个小对象，返回句柄。
    pub fn alloc(&mut self, size: usize) -> Result<SlabHandle, SlabError> {
        let ci = self.pick_class(size).ok_or(SlabError::TooLarge)?;
        let obj_id = self.caches[ci].alloc();
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
        self.caches[ci].free_obj(h.obj_id)
    }

    /// 统计信息：返回各尺寸类的 (容量, 已用)。
    pub fn stats(&self) -> Vec<(usize, usize, usize)> {
        self.caches
            .iter()
            .map(|c| (c.size, c.capacity, c.used))
            .collect()
    }

    /// 生成摘要文本（供将来 `slab://stats` 通道展示）。
    pub fn summary(&self) -> String {
        let mut out = String::from("slab: 尺寸类 容量 已用\n");
        for (sz, cap, used) in self.stats() {
            out.push_str(&format!("  {}B: {}/{} 已用\n", sz, used, cap));
        }
        out
    }
}

impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

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
}
