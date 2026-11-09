//! 内存页故障模拟 —— `mem://fault` 的后端。
//!
//! Linux 思维：页表、缺页中断、OOM killer、交换分区。
//! AI 通道思维：内存 = `mem://ram` 通道；分配 = `lease acquire <内存通道>`；
//! 页故障 = `wait_next` 阻塞到内存可用；OOM = 租约冲突，治理域（root）强制回收。
//!
//! 本模块维护一个极简页框分配器，用于演示「页故障 → 阻塞 → 内存可用 → 唤醒」的闭环。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

/// 物理页框。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageFrame {
    pub pfn: u64,
    pub owner: Option<u64>, // 持有该页的 exec-id
    pub dirty: bool,
}

/// 页表项：虚拟页号 -> 物理页框。
type PageTable = BTreeMap<u64, u64>;

/// 内存管理器状态。
struct MmState {
    total_frames: u64,
    free_frames: BTreeSet<u64>,
    used_frames: BTreeMap<u64, PageFrame>, // pfn -> frame
    page_tables: BTreeMap<u64, PageTable>, // exec-id -> 页表
    fault_count: u64,
    alloc_count: u64,
    free_count: u64,
    oom_count: u64,
}

static MM: OnceLock<Mutex<MmState>> = OnceLock::new();

fn mm_state() -> &'static Mutex<MmState> {
    MM.get_or_init(|| {
        Mutex::new(MmState {
            total_frames: 65536, // 256 MiB @ 4KiB
            free_frames: (0..65536).collect(),
            used_frames: BTreeMap::new(),
            page_tables: BTreeMap::new(),
            fault_count: 0,
            alloc_count: 0,
            free_count: 0,
            oom_count: 0,
        })
    })
}

/// 为指定 exec-id 分配一个物理页框，建立虚拟页映射。
pub fn alloc_pfn(owner: u64, vaddr: u64) -> Result<u64, String> {
    let mut g = mm_state().lock().unwrap();
    let pfn = g
        .free_frames
        .iter()
        .next()
        .copied()
        .ok_or_else(|| {
            g.oom_count += 1;
            "OOM: 无可用物理页框".to_string()
        })?;
    g.free_frames.remove(&pfn);
    g.used_frames.insert(
        pfn,
        PageFrame {
            pfn,
            owner: Some(owner),
            dirty: false,
        },
    );
    g.page_tables
        .entry(owner)
        .or_default()
        .insert(vaddr, pfn);
    g.alloc_count += 1;
    Ok(pfn)
}

/// 释放指定 exec-id 的一个虚拟页映射及其物理页框。
pub fn free_pfn(owner: u64, vaddr: u64) -> Result<(), String> {
    let mut g = mm_state().lock().unwrap();
    let pt = g
        .page_tables
        .get_mut(&owner)
        .ok_or("该进程无页表")?;
    let pfn = pt
        .remove(&vaddr)
        .ok_or("该虚拟地址未映射")?;
    g.used_frames.remove(&pfn);
    g.free_frames.insert(pfn);
    g.free_count += 1;
    Ok(())
}

/// 模拟一次缺页中断：虚拟地址未映射，需要分配页框。
pub fn handle_fault(owner: u64, vaddr: u64) -> Result<u64, String> {
    let mut g = mm_state().lock().unwrap();
    // 已映射则直接返回。
    if let Some(pt) = g.page_tables.get(&owner) {
        if pt.contains_key(&vaddr) {
            return Ok(*pt.get(&vaddr).unwrap());
        }
    }
    g.fault_count += 1;
    drop(g);
    alloc_pfn(owner, vaddr)
}

/// 查询某虚拟地址当前映射的物理页框（无映射则返回 None）。
pub fn query_pfn(owner: u64, vaddr: u64) -> Option<u64> {
    let g = mm_state().lock().unwrap();
    g.page_tables
        .get(&owner)
        .and_then(|pt| pt.get(&vaddr).copied())
}

/// 统计信息。
pub fn stats() -> String {
    let g = mm_state().lock().unwrap();
    let used = g.used_frames.len() as u64;
    let free = g.free_frames.len() as u64;
    format!(
        "total_frames={} used={} free={} faults={} allocs={} frees={} ooms={}",
        g.total_frames, used, free, g.fault_count, g.alloc_count, g.free_count, g.oom_count
    )
}

/// 回收指定进程的全部页框（供 `proc://kill` 弹出时调用）。
pub fn reclaim_proc(owner: u64) -> usize {
    let mut g = mm_state().lock().unwrap();
    let pt = match g.page_tables.remove(&owner) {
        Some(pt) => pt,
        None => return 0,
    };
    let mut n = 0;
    for (_vaddr, pfn) in pt {
        g.used_frames.remove(&pfn);
        g.free_frames.insert(pfn);
        n += 1;
    }
    g.free_count += n as u64;
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_query_pfn() {
        let pfn = alloc_pfn(1, 0x1000).unwrap();
        assert_eq!(query_pfn(1, 0x1000), Some(pfn));
    }

    #[test]
    fn fault_allocates() {
        let pfn = handle_fault(2, 0x2000).unwrap();
        assert_eq!(query_pfn(2, 0x2000), Some(pfn));
        let s = stats();
        assert!(s.contains("faults=1"));
    }

    #[test]
    fn free_pfn_works() {
        let _pfn = alloc_pfn(3, 0x3000).unwrap();
        free_pfn(3, 0x3000).unwrap();
        assert_eq!(query_pfn(3, 0x3000), None);
    }

    #[test]
    fn reclaim_proc_frees_all() {
        alloc_pfn(4, 0x4000).unwrap();
        alloc_pfn(4, 0x5000).unwrap();
        let n = reclaim_proc(4);
        assert_eq!(n, 2);
        assert_eq!(query_pfn(4, 0x4000), None);
        assert_eq!(query_pfn(4, 0x5000), None);
    }
}
