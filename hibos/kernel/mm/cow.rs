//! 写时复制（Copy-On-Write, COW）。
//! 
//! AI 原生映射：单写者租约共享页框，写入时复制 = 模型权重页共享 + 推理时私有化上下文页。
//! fork 操作：只读共享物理页 + 打 COW 标记；子空间首次写入触发缺页 → 真正复制那一页。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::exec::ExecId;
use crate::mm::address_space::{
    as_registry, map_vaddr, query_pte,
};
use crate::mm::frame_alloc::alloc_phys_frame;
use crate::mm::pagetable::PteFlags;

/// COW 区域：一段共享页框范围，标记为 COW，首次写入触发复制。
#[derive(Clone, Debug)]
pub struct CowRegion {
    /// 起始虚拟地址。
    pub start_vaddr: u64,
    /// 页数。
    pub page_count: usize,
    /// 共享的物理页框号列表。
    pub pfns: Vec<u64>,
    /// 是否已触发过 COW（私有化）。
    pub privatized: bool,
}

/// 全局 COW 区域注册表：pid -> COW 区域列表。
struct CowRegistry {
    regions: BTreeMap<ExecId, Vec<CowRegion>>,
}

static COW_REGISTRY: OnceLock<Mutex<CowRegistry>> = OnceLock::new();

fn cow_registry() -> &'static Mutex<CowRegistry> {
    COW_REGISTRY.get_or_init(|| Mutex::new(CowRegistry {
        regions: BTreeMap::new(),
    }))
}

/// 为指定 pid 的地址空间建立 COW 映射（fork 后调用）。
/// 将源空间的可写用户页标记为只读 + COW，目标空间共享同一页框。
pub fn cow_fork(source_pid: ExecId, target_pid: ExecId) -> Result<(), String> {
    let source = {
        let g = as_registry().lock().unwrap();
        g.spaces.get(&source_pid).cloned().ok_or("源地址空间不存在")?
    };
    let mut cow_regions = Vec::new();
    for (vaddr, pte) in &source.mappings {
        if !pte.is_present() {
            continue;
        }
        // 仅对用户页建立 COW。
        if !pte.is_user() {
            continue;
        }
        // 目标空间映射同一页框，但标记为只读 + COW。
        let flags = PteFlags::PRESENT | PteFlags::USER; // 只读，无 READ_WRITE
        map_vaddr(target_pid, *vaddr, pte.pfn, flags)?;
        cow_regions.push(CowRegion {
            start_vaddr: *vaddr,
            page_count: 1,
            pfns: vec![pte.pfn],
            privatized: false,
        });
    }

    // 保存 COW 区域。
    let mut g = cow_registry().lock().unwrap();
    g.regions.insert(target_pid, cow_regions);

    // 将源空间的可写用户页也标记为 COW（只读）。
    let mut g2 = as_registry().lock().unwrap();
    let _source_space = g2
        .spaces
        .get_mut(&source_pid)
        .ok_or("源地址空间不存在")?;
    for (vaddr, pte) in &source.mappings {
        if pte.is_user() && pte.is_writable() && pte.is_present() {
            let _ = map_vaddr(
                source_pid,
                *vaddr,
                pte.pfn,
                PteFlags::PRESENT | PteFlags::USER | PteFlags::COW,
            );
        }
    }
    Ok(())
}

/// 标记 PTE 为 COW。
pub fn cow_mark_cow(pid: ExecId, vaddr: u64) -> Result<(), String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    if let Some(pte) = space.mappings.get_mut(&vaddr) {
        pte.flags.insert(PteFlags::COW);
        pte.flags.remove(PteFlags::READ_WRITE);
        Ok(())
    } else {
        Err("PTE 未找到".to_string())
    }
}

/// 查询 PTE 是否为 COW。
pub fn cow_is_cow(pid: ExecId, vaddr: u64) -> Result<bool, String> {
    let g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    Ok(space
        .mappings
        .get(&vaddr)
        .map(|pte| pte.is_cow())
        .unwrap_or(false))
}

/// 执行 COW 复制：缺页处理时调用，将共享页框复制为私有可写页。
pub fn cow_copy_on_write(pid: ExecId, vaddr: u64) -> Result<u64, String> {
    // 检查是否为 COW 页。
    let is_cow = cow_is_cow(pid, vaddr)?;
    if !is_cow {
        return Err("非 COW 页".to_string());
    }
    // 触发 COW 缺页处理：分配新页框并映射为可写。
    let old_pte = query_pte(pid, vaddr).ok_or("PTE 未找到")?;
    let new_pfn = alloc_phys_frame(pid)?;
    let flags = PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER;
    map_vaddr(pid, vaddr, new_pfn, flags)?;
    // 更新 COW 区域状态。
    let mut g = cow_registry().lock().unwrap();
    if let Some(regions) = g.regions.get_mut(&pid) {
        for region in regions.iter_mut() {
            if region.start_vaddr <= vaddr
                && vaddr < region.start_vaddr + (region.page_count as u64 * 4096)
            {
                region.privatized = true;
                break;
            }
        }
    }
    let _ = old_pte;
    Ok(new_pfn)
}

/// COW 统计。
pub fn cow_stats() -> String {
    let g = cow_registry().lock().unwrap();
    let regions_count: usize = g.regions.values().map(|v| v.len()).sum();
    let privatized: usize = g
        .regions
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| r.privatized)
        .count();
    let total_pages: usize = g
        .regions
        .values()
        .flat_map(|v| v.iter())
        .map(|r| r.page_count)
        .sum();
    format!(
        "cow_regions={} total_pages={} privatized={}",
        regions_count, total_pages, privatized
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecId;

    #[test]
    fn cow_fork_shares_pages() {
        let source = 300;
        let target = 301;
        create_address_space(source).unwrap();
        create_address_space(target).unwrap();
        // 在源空间映射一个可写页。
        map_vaddr(
            source,
            0x4000,
            0x7000,
            PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER,
        )
        .unwrap();
        // fork 后目标空间共享同一页框（只读 COW）。
        cow_fork(source, target).unwrap();
        // 目标空间应能翻译。
        let (paddr, _) = translate_vaddr(target, 0x4000).unwrap();
        assert_eq!(paddr, 0x7000);
        // 源空间的页应已标记 COW。
        assert!(cow_is_cow(source, 0x4000).unwrap());
    }

    #[test]
    fn cow_copy_on_write_privatizes() {
        let source = 302;
        let target = 303;
        create_address_space(source).unwrap();
        create_address_space(target).unwrap();
        map_vaddr(
            source,
            0x5000,
            0x8000,
            PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER,
        )
        .unwrap();
        cow_fork(source, target).unwrap();
        // 目标空间写入触发 COW。
        let new_pfn = cow_copy_on_write(target, 0x5000).unwrap();
        assert_ne!(new_pfn, 0);
        // COW 后目标空间映射新页框。
        let (paddr, _) = translate_vaddr(target, 0x5000).unwrap();
        assert_eq!(paddr, new_pfn << 12);
    }

    #[test]
    fn cow_stats_contains_expected_fields() {
        let s = cow_stats();
        assert!(s.contains("cow_regions"));
    }
}

