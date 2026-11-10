//! 页框位图分配器（伙伴系统简化版）。
//! 
//! 管理物理页框的分配与释放，为四级页表与地址空间提供底层页框供给。
//! AI 原生映射：页框 = 上下文页/KV 缓存的物理载体；分配 = 上下文窗口扩容；
//! 释放 = 上下文页淘汰/换出。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

/// 物理页大小（4 KiB）。
pub const PAGE_FRAME_SIZE: u64 = 4096;

/// 物理内存总量（模拟 4 GiB，1M 个页框）。
pub const TOTAL_FRAMES: u64 = 1_048_576;

/// 页框分配状态。
struct FrameAllocatorState {
    /// 已分配页框集合（pfn -> 所有者 exec-id）。
    allocated: BTreeMap<u64, u64>,
    /// 空闲页框集合（pfn）。
    free: BTreeSet<u64>,
    /// 统计。
    alloc_count: u64,
    free_count: u64,
}

impl FrameAllocatorState {
    fn new() -> Self {
        Self {
            allocated: BTreeMap::new(),
            free: (0..TOTAL_FRAMES).collect(),
            alloc_count: 0,
            free_count: 0,
        }
    }
}

static FRAME_ALLOC: OnceLock<Mutex<FrameAllocatorState>> = OnceLock::new();

fn frame_alloc_state() -> &'static Mutex<FrameAllocatorState> {
    FRAME_ALLOC.get_or_init(|| Mutex::new(FrameAllocatorState::new()))
}

/// 页框分配器句柄。
pub struct FrameAllocator;

impl FrameAllocator {
    /// 分配一个物理页框，返回 (pfn, owner)。
    pub fn alloc(owner: u64) -> Result<u64, String> {
        let mut g = frame_alloc_state().lock().unwrap();
        let pfn = g
            .free
            .iter()
            .next()
            .copied()
            .ok_or_else(|| "OOM: 无可用物理页框".to_string())?;
        g.free.remove(&pfn);
        g.allocated.insert(pfn, owner);
        g.alloc_count += 1;
        Ok(pfn)
    }

    /// 释放一个物理页框。
    pub fn free(pfn: u64) -> Result<(), String> {
        let mut g = frame_alloc_state().lock().unwrap();
        if g.allocated.remove(&pfn).is_none() {
            return Err(format!("页框 {} 未分配", pfn));
        }
        g.free.insert(pfn);
        g.free_count += 1;
        Ok(())
    }

    /// 查询页框的所有者。
    pub fn owner(pfn: u64) -> Option<u64> {
        let g = frame_alloc_state().lock().unwrap();
        g.allocated.get(&pfn).copied()
    }

    /// 统计信息。
    pub fn stats() -> String {
        let g = frame_alloc_state().lock().unwrap();
        let allocated = g.allocated.len() as u64;
        let free = g.free.len() as u64;
        format!(
            "total_frames={} allocated={} free={} allocs={} frees={}",
            TOTAL_FRAMES, allocated, free, g.alloc_count, g.free_count
        )
    }

    /// 回收指定所有者的全部页框。
    pub fn reclaim_owner(owner: u64) -> usize {
        let mut g = frame_alloc_state().lock().unwrap();
        let to_free: Vec<u64> = g
            .allocated
            .iter()
            .filter(|(_, &o)| o == owner)
            .map(|(&pfn, _)| pfn)
            .collect();
        let n = to_free.len();
        for pfn in to_free {
            g.allocated.remove(&pfn);
            g.free.insert(pfn);
        }
        g.free_count += n as u64;
        n
    }
}

/// 全局页框分配器单例。
pub fn frame_allocator() -> &'static Mutex<FrameAllocatorState> {
    frame_alloc_state()
}

/// 分配一个物理页框（便捷函数）。
pub fn alloc_phys_frame(owner: u64) -> Result<u64, String> {
    FrameAllocator::alloc(owner)
}

/// 释放一个物理页框（便捷函数）。
pub fn free_phys_frame(pfn: u64) -> Result<(), String> {
    FrameAllocator::free(pfn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_free_frame() {
        let pfn = FrameAllocator::alloc(1).unwrap();
        assert!(FrameAllocator::owner(pfn) == Some(1));
        FrameAllocator::free(pfn).unwrap();
        assert_eq!(FrameAllocator::owner(pfn), None);
    }

    #[test]
    fn reclaim_owner_frees_all() {
        let p1 = FrameAllocator::alloc(10).unwrap();
        let p2 = FrameAllocator::alloc(10).unwrap();
        let n = FrameAllocator::reclaim_owner(10);
        assert_eq!(n, 2);
        assert_eq!(FrameAllocator::owner(p1), None);
        assert_eq!(FrameAllocator::owner(p2), None);
    }

    #[test]
    fn stats_contains_expected_fields() {
        let s = FrameAllocator::stats();
        assert!(s.contains("total_frames"));
        assert!(s.contains("allocated"));
        assert!(s.contains("free"));
    }
}
