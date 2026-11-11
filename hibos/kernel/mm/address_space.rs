//! 地址空间（AddressSpace）：每进程/智能体独立的四级页表根。
//! 
//! AI 原生映射：地址空间 = 模型实例的运行态上下文空间；
//! 用户/内核空间分离 = 模型权重(只读代码段) + 上下文窗口(可读写数据段)。
//! 缺页处理 = 上下文窗口溢出 → 调入知识页。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::exec::ExecId;
use crate::mm::frame_alloc::{alloc_phys_frame, free_phys_frame};
use crate::mm::pagetable::{
    PageTable, PageTableEntry, PageTables, PageTableRoot, PteFlags, VAddr,
};

/// 地址空间：每个进程/智能体独立的四级页表根。
#[derive(Clone, Debug)]
pub struct AddressSpace {
    pub pid: ExecId,
    /// PGD 根页表的物理页框号。
    pub pgd_pfn: u64,
    /// 页表集合（物理页框号 -> 页表页）。
    pub tables: PageTables,
    /// 虚拟地址 -> PTE 的快速查找缓存（调试用，不参与硬件遍历）。
    pub mappings: BTreeMap<u64, PageTableEntry>,
    /// 缺页计数。
    pub fault_count: u64,
    /// COW 缺页计数。
    pub cow_fault_count: u64,
    /// 创建时间（纳秒）。
    pub created_ns: u64,
}

/// 全局地址空间注册表：pid -> AddressSpace。
pub(crate) struct AddressSpaceRegistry {
    pub(crate) spaces: BTreeMap<ExecId, AddressSpace>,
}

static AS_REGISTRY: OnceLock<Mutex<AddressSpaceRegistry>> = OnceLock::new();

pub(crate) fn as_registry() -> &'static Mutex<AddressSpaceRegistry> {
    AS_REGISTRY.get_or_init(|| Mutex::new(AddressSpaceRegistry {
        spaces: BTreeMap::new(),
    }))
}

/// 创建指定 pid 的地址空间（分配页表根页框）。
pub fn create_address_space(pid: ExecId) -> Result<AddressSpace, String> {
    let mut g = as_registry().lock().unwrap();
    if g.spaces.contains_key(&pid) {
        return Err(format!("地址空间已存在: pid={}", pid));
    }
    // 分配页表根页框。
    let pgd_pfn = alloc_phys_frame(pid).map_err(|e| format!("分配页表根失败: {}", e))?;
    let mut tables = PageTables::new();
    tables.insert(
        pgd_pfn,
        [PageTableEntry::new(); crate::mm::pagetable::PT_LEVEL_ENTRIES],
    );
    let _root = PageTableRoot(pgd_pfn);
    let now = crate::monitor::get_time_ns();
    let space = AddressSpace {
        pid,
        pgd_pfn,
        tables,
        mappings: BTreeMap::new(),
        fault_count: 0,
        cow_fault_count: 0,
        created_ns: now,
    };
    g.spaces.insert(pid, space.clone());
    Ok(space)
}

/// 销毁指定 pid 的地址空间（释放全部页框）。
pub fn destroy_address_space(pid: ExecId) -> Result<(), String> {
    let mut g = as_registry().lock().unwrap();
    let space = match g.spaces.remove(&pid) {
        Some(s) => s,
        None => return Err(format!("地址空间不存在: pid={}", pid)),
    };
    // 释放页表集合占用的页框。
    for pfn in space.tables.keys() {
        let _ = free_phys_frame(*pfn);
    }
    Ok(())
}

/// 获取指定 pid 的地址空间（克隆返回）。
pub fn get_address_space(pid: ExecId) -> Option<AddressSpace> {
    let g = as_registry().lock().unwrap();
    g.spaces.get(&pid).cloned()
}

/// 映射虚拟地址到物理页框。
pub fn map_vaddr(
    pid: ExecId,
    vaddr: u64,
    pfn: u64,
    flags: PteFlags,
) -> Result<(), String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    let vaddr = VAddr(vaddr);
    let paddr = crate::mm::pagetable::PAddr(pfn << crate::mm::pagetable::PAGE_SHIFT);
    let root = PageTableRoot(space.pgd_pfn);
    let mut alloc = || Ok(0u64); // 已有页框，无需额外分配
    PageTable::map(&mut space.tables, root, vaddr, paddr, flags, &mut alloc)?;
    space.mappings.insert(vaddr.0, PageTableEntry { pfn, flags });
    Ok(())
}

/// 取消映射虚拟地址。
pub fn unmap_vaddr(pid: ExecId, vaddr: u64) -> Result<(), String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    let vaddr = VAddr(vaddr);
    let root = PageTableRoot(space.pgd_pfn);
    PageTable::unmap(&mut space.tables, root, vaddr)?;
    space.mappings.remove(&vaddr.0);
    Ok(())
}

/// 翻译虚拟地址 → 物理地址。
pub fn translate_vaddr(pid: ExecId, vaddr: u64) -> Result<(u64, u64), String> {
    let g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    let vaddr = VAddr(vaddr);
    let root = PageTableRoot(space.pgd_pfn);
    let paddr = PageTable::translate(&space.tables, root, vaddr)?;
    let offset = vaddr.0 & (crate::mm::pagetable::PAGE_SIZE - 1);
    Ok((paddr.0, offset))
}

/// 查询 PTE（供调试）。
pub fn query_pte(pid: ExecId, vaddr: u64) -> Option<PageTableEntry> {
    let g = as_registry().lock().unwrap();
    let space = g.spaces.get(&pid)?;
    let vaddr = VAddr(vaddr);
    let root = PageTableRoot(space.pgd_pfn);
    PageTable::query_pte(&space.tables, root, vaddr)
}

/// 处理缺页中断：若未映射则分配页框并映射。
pub fn handle_fault(pid: ExecId, vaddr: u64, write: bool) -> Result<u64, String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;

    // 已映射则直接返回 pfn。
    if let Some(pte) = query_pte(pid, vaddr) {
        if pte.is_present() {
            // 标记 accessed / dirty。
            let _ = set_pte_flags(pid, vaddr, PteFlags::ACCESSED, true);
            if write {
                let _ = set_pte_flags(pid, vaddr, PteFlags::DIRTY, true);
            }
            return Ok(pte.pfn);
        }
    }

    // 未映射：分配页框并映射。
    space.fault_count += 1;
    drop(g);

    let pfn = alloc_phys_frame(pid)?;
    let mut flags = PteFlags::PRESENT | PteFlags::USER;
    if write {
        flags.insert(PteFlags::READ_WRITE);
    }
    map_vaddr(pid, vaddr, pfn, flags)?;
    Ok(pfn)
}

/// 处理 COW 缺页：将共享页框复制一份，映射为私有可写页。
pub fn handle_cow_fault(pid: ExecId, vaddr: u64) -> Result<u64, String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    space.cow_fault_count += 1;
    let pte = match query_pte(pid, vaddr) {
        Some(pte) => pte,
        None => return Err("COW 缺页但无 PTE".to_string()),
    };
    let old_pfn = pte.pfn;
    drop(g);

    // 分配新页框并复制内容（模拟：内容全零，实际应从 old_pfn 读取）。
    let new_pfn = alloc_phys_frame(pid)?;
    let _ = old_pfn; // 简化：不实际复制内容
    let flags = PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER;
    map_vaddr(pid, vaddr, new_pfn, flags)?;
    Ok(new_pfn)
}

/// 设置 PTE 标志位。
fn set_pte_flags(pid: ExecId, vaddr: u64, flag: PteFlags, set: bool) -> Result<(), String> {
    let mut g = as_registry().lock().unwrap();
    let space = g
        .spaces
        .get_mut(&pid)
        .ok_or_else(|| format!("地址空间不存在: pid={}", pid))?;
    let vaddr = VAddr(vaddr);
    let _root = PageTableRoot(space.pgd_pfn);
    // 直接修改映射缓存。
    if let Some(pte) = space.mappings.get_mut(&vaddr.0) {
        if set {
            pte.flags.insert(flag);
        } else {
            pte.flags.remove(flag);
        }
        Ok(())
    } else {
        Err("PTE 未找到".to_string())
    }
}

/// 地址空间统计。
pub fn as_stats() -> String {
    let g = as_registry().lock().unwrap();
    let spaces = g.spaces.len() as u64;
    let faults: u64 = g.spaces.values().map(|s| s.fault_count).sum();
    let cow_faults: u64 = g.spaces.values().map(|s| s.cow_fault_count).sum();
    format!(
        "spaces={} total_faults={} total_cow_faults={}",
        spaces, faults, cow_faults
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_translate() {
        let pid: ExecId = 200;
        let _space = create_address_space(pid).unwrap();
        // 先映射再翻译。
        map_vaddr(pid, 0x1000, 0x5000, PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER)
            .unwrap();
        let (paddr, offset) = translate_vaddr(pid, 0x1000 + 0x100).unwrap();
        assert_eq!(paddr, 0x5000 + 0x100);
        assert_eq!(offset, 0x100);
    }

    #[test]
    fn unmap_then_translate_fails() {
        let pid: ExecId = 201;
        let _space = create_address_space(pid).unwrap();
        map_vaddr(
            pid,
            0x2000,
            0x6000,
            PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER,
        )
        .unwrap();
        unmap_vaddr(pid, 0x2000).unwrap();
        assert!(translate_vaddr(pid, 0x2000).is_err());
    }

    #[test]
    fn destroy_address_space_works() {
        let pid: ExecId = 202;
        create_address_space(pid).unwrap();
        destroy_address_space(pid).unwrap();
        assert!(get_address_space(pid).is_none());
    }

    #[test]
    fn handle_fault_allocates() {
        let pid: ExecId = 203;
        create_address_space(pid).unwrap();
        let pfn = handle_fault(pid, 0x3000, false).unwrap();
        assert_ne!(pfn, 0);
        let (paddr, _) = translate_vaddr(pid, 0x3000).unwrap();
        assert_eq!(paddr, pfn << crate::mm::pagetable::PAGE_SHIFT);
    }

    #[test]
    fn as_stats_contains_expected_fields() {
        let s = as_stats();
        assert!(s.contains("spaces"));
        assert!(s.contains("total_faults"));
    }
}
