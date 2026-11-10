//! 四级页表（PGD → PUD → PMD → PTE → 4KB 物理页）。
//! 
//! 48 位虚拟地址，每级 512 项（9 位），每项 8 字节。
//! 页表项语义按 Intel SDM：P / R/W / U/S / A / D / NX，外加软件位 COW。
//! 
//! AI 原生映射：页表 = 语义映射树（token → 上下文页/KV 缓存）；
//! 缺页 = 通道事件（`mem://fault`）。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 物理页大小（4 KiB）。
pub const PAGE_SIZE: u64 = 4096;
pub const PAGE_SHIFT: u64 = 12;

/// 48 位虚拟地址。
pub const VADDR_BITS: u64 = 48;
/// 用户空间顶端（0x0000_7FFF_FFFF_FFFF）。
pub const USER_VADDR_TOP: u64 = 0x0000_7FFF_FFFF_FFFF;
/// 内核空间基址（0xFFFF_8000_0000_0000）。
pub const KERNEL_VADDR_BASE: u64 = 0xFFFF_8000_0000_0000;
/// 虚拟地址掩码（48 位）。
pub const VADDR_MASK: u64 = (1u64 << VADDR_BITS) - 1;

/// 每级页表项数（512 = 2^9）。
pub const PT_LEVEL_ENTRIES: usize = 512;
/// 页表索引掩码（9 位）。
pub const PT_INDEX_MASK: usize = PT_LEVEL_ENTRIES - 1;

// ---------------------------------------------------------------
// PTE 标志位（按 Intel SDM 语义，外加软件位 COW）
// ---------------------------------------------------------------

/// 页表项标志位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PteFlags(pub u64);

impl PteFlags {
    pub const PRESENT: Self = Self(1 << 0);
    pub const READ_WRITE: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);
    pub const ACCESSED: Self = Self(1 << 5);
    pub const DIRTY: Self = Self(1 << 6);
    pub const GLOBAL: Self = Self(1 << 8);
    pub const NX: Self = Self(1 << 63);
    /// 软件位：写时复制（页表遍历硬件不可见，仅内核使用）。
    pub const COW: Self = Self(1 << 9);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }

    pub fn insert(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    pub fn remove(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }
}

impl std::ops::BitOr for PteFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for PteFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 页表项：物理页框号（PFN，位 12~51）+ 标志位。
#[derive(Clone, Copy, Debug, Default)]
pub struct PageTableEntry {
    pub pfn: u64,
    pub flags: PteFlags,
}

impl PageTableEntry {
    pub const fn new() -> Self {
        Self { pfn: 0, flags: PteFlags::empty() }
    }

    pub fn is_present(self) -> bool {
        self.flags.contains(PteFlags::PRESENT)
    }

    pub fn is_writable(self) -> bool {
        self.flags.contains(PteFlags::READ_WRITE)
    }

    pub fn is_user(self) -> bool {
        self.flags.contains(PteFlags::USER)
    }

    pub fn is_cow(self) -> bool {
        self.flags.contains(PteFlags::COW)
    }

    pub fn is_nx(self) -> bool {
        self.flags.contains(PteFlags::NX)
    }

    pub fn set_present(&mut self, v: bool) {
        self.flags.insert(PteFlags::PRESENT);
        if !v {
            self.flags.remove(PteFlags::PRESENT);
        }
    }

    pub fn set_cow(&mut self, v: bool) {
        if v {
            self.flags.insert(PteFlags::COW);
        } else {
            self.flags.remove(PteFlags::COW);
        }
    }
}

/// 虚拟地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VAddr(pub u64);

/// 物理地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PAddr(pub u64);

// ---------------------------------------------------------------
// 地址分解：48 位虚拟地址 → [PML4(9)][PDPT(9)][PD(9)][PT(9)][Offset(12)]
// ---------------------------------------------------------------

impl VAddr {
    /// 分解为四级索引与页内偏移。
    pub fn decompose(self) -> (usize, usize, usize, usize, u64) {
        let v = self.0 & VADDR_MASK;
        let pt_idx = ((v >> PAGE_SHIFT) & PT_INDEX_MASK as u64) as usize;
        let pd_idx = ((v >> (PAGE_SHIFT + 9)) & PT_INDEX_MASK as u64) as usize;
        let pdpt_idx = ((v >> (PAGE_SHIFT + 18)) & PT_INDEX_MASK as u64) as usize;
        let pml4_idx = ((v >> (PAGE_SHIFT + 27)) & PT_INDEX_MASK as u64) as usize;
        let offset = v & (PAGE_SIZE - 1);
        (pml4_idx, pdpt_idx, pd_idx, pt_idx, offset)
    }

    /// 是否为用户地址（低半部）。
    pub fn is_user(self) -> bool {
        self.0 <= USER_VADDR_TOP
    }

    /// 是否为内核地址（高半部）。
    pub fn is_kernel(self) -> bool {
        self.0 >= KERNEL_VADDR_BASE
    }
}

// ---------------------------------------------------------------
// 页表：BTreeMap<物理页框号, [PTE; 512]>
// ---------------------------------------------------------------

/// 页表集合：物理页框号 -> 页表页（512 个 PTE）。
pub type PageTables = BTreeMap<u64, [PageTableEntry; PT_LEVEL_ENTRIES]>;

/// 页表根（PGD 所在物理页框号）。
#[derive(Clone, Copy, Debug, Default)]
pub struct PageTableRoot(pub u64);

/// 四级页表操作。
pub struct PageTable;

impl PageTable {
    /// 创建一个新的空页表根（分配一页作为 PGD）。
    pub fn new_root() -> PageTableRoot {
        // 简化：使用固定页框号作为根，实际由 frame_alloc 分配。
        PageTableRoot(0x1000)
    }

    /// 遍历页表，返回叶子 PTE 的可变引用（若中间页表缺失则返回 None）。
    pub fn walk(
        tables: &mut PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Option<&mut PageTableEntry> {
        let (pml4_idx, pdpt_idx, pd_idx, pt_idx, _offset) = vaddr.decompose();
        let mut cur_pfn = root.0;

        // 前三级：只读取下一级页框号，每轮立即结束对 tables 的可变借用。
        for level_idx in [pml4_idx, pdpt_idx, pd_idx] {
            let next_pfn = {
                let level = tables.get_mut(&cur_pfn)?;
                let entry = level.get(level_idx)?;
                if !entry.is_present() {
                    return None;
                }
                entry.pfn
            };
            cur_pfn = next_pfn;
        }
        // 最后一级：返回叶子 PTE 的可变引用。
        let level = tables.get_mut(&cur_pfn)?;
        level.get_mut(pt_idx)
    }

    /// 只读遍历，返回叶子 PTE 的不可变引用。
    pub fn walk_ref(
        tables: &PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Option<&PageTableEntry> {
        let (pml4_idx, pdpt_idx, pd_idx, pt_idx, _offset) = vaddr.decompose();
        let mut cur_pfn = root.0;

        for (level_idx, is_last) in [
            (pml4_idx, false),
            (pdpt_idx, false),
            (pd_idx, false),
            (pt_idx, true),
        ] {
            let level = tables.get(&cur_pfn)?;
            if is_last {
                return level.get(pt_idx);
            }
            let entry = level.get(level_idx)?;
            if !entry.is_present() {
                return None;
            }
            cur_pfn = entry.pfn;
        }
        None
    }

    /// 确保路径存在（缺失的中间页表自动分配页框）。返回叶子页表的物理页框号。
    pub fn ensure_path(
        tables: &mut PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
        alloc_frame: &mut dyn FnMut() -> Result<u64, String>,
    ) -> Result<u64, String> {
        let (pml4_idx, pdpt_idx, pd_idx, pt_idx, _offset) = vaddr.decompose();
        let mut cur_pfn = root.0;

        // 确保 PGD 存在。
        if !tables.contains_key(&cur_pfn) {
            let new_pfn = alloc_frame()?;
            tables.insert(new_pfn, [PageTableEntry::new(); PT_LEVEL_ENTRIES]);
        }

        let levels = [
            (pml4_idx, false),
            (pdpt_idx, false),
            (pd_idx, false),
            (pt_idx, true),
        ];

        for &(idx, is_last) in &levels {
            let entry = {
                let level = tables.get(&cur_pfn).ok_or("页表页缺失")?;
                level[idx]
            };
            if is_last {
                // 叶子页表已经在 cur_pfn 中。
                return Ok(cur_pfn);
            }
            if !entry.is_present() {
                // 分配新页表页并填入中级项。
                let new_pfn = alloc_frame()?;
                tables.insert(new_pfn, [PageTableEntry::new(); PT_LEVEL_ENTRIES]);
                tables.get_mut(&cur_pfn).unwrap()[idx] = PageTableEntry {
                    pfn: new_pfn,
                    flags: PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER,
                };
                cur_pfn = new_pfn;
            } else {
                cur_pfn = entry.pfn;
            }
        }
        Ok(cur_pfn)
    }

    /// 将虚拟地址映射到物理页框（自动创建中间页表）。
    pub fn map(
        tables: &mut PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
        paddr: PAddr,
        flags: PteFlags,
        alloc_frame: &mut dyn FnMut() -> Result<u64, String>,
    ) -> Result<(), String> {
        let _pt_paddr = Self::ensure_path(tables, root, vaddr, alloc_frame)?;
        let (_pml4, _pdpt, _pd, pt_idx, _offset) = vaddr.decompose();
        // 找到叶子页表页框。
        let pt_pfn = Self::leaf_pt_pfn(tables, root, vaddr)?;
        let level = tables.get_mut(&pt_pfn).ok_or("叶子页表缺失")?;
        level[pt_idx] = PageTableEntry {
            pfn: paddr.0 >> PAGE_SHIFT,
            flags,
        };
        Ok(())
    }

    /// 获取叶子页表的物理页框号。
    fn leaf_pt_pfn(
        tables: &PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Result<u64, String> {
        let (pml4_idx, pdpt_idx, pd_idx, _pt_idx, _offset) = vaddr.decompose();
        let mut cur_pfn = root.0;

        for &(idx, is_last) in &[
            (pml4_idx, false),
            (pdpt_idx, false),
            (pd_idx, false),
        ] {
            let entry = tables
                .get(&cur_pfn)
                .and_then(|l| l.get(idx))
                .ok_or("中级页表缺失")?;
            if !entry.is_present() {
                return Err("中级页表项未呈现".to_string());
            }
            cur_pfn = entry.pfn;
        }
        Ok(cur_pfn)
    }

    /// 取消映射虚拟地址（仅清 PTE，不释放页框）。
    pub fn unmap(
        tables: &mut PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Result<(), String> {
        let (pml4_idx, pdpt_idx, pd_idx, pt_idx, _offset) = vaddr.decompose();
        let mut cur_pfn = root.0;

        for &(idx, is_last) in &[
            (pml4_idx, false),
            (pdpt_idx, false),
            (pd_idx, false),
            (pt_idx, true),
        ] {
            let level = tables.get_mut(&cur_pfn).ok_or("页表页缺失")?;
            if is_last {
                level[pt_idx] = PageTableEntry::new();
                return Ok(());
            }
            let entry = level.get(idx).ok_or("中级页表项缺失")?;
            if !entry.is_present() {
                return Err("未映射".to_string());
            }
            cur_pfn = entry.pfn;
        }
        Err("未到达叶子".to_string())
    }

    /// 翻译虚拟地址 → 物理地址。
    pub fn translate(
        tables: &PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Result<PAddr, String> {
        let (_pml4, _pdpt, _pd, pt_idx, offset) = vaddr.decompose();
        let entry = Self::walk_ref(tables, root, vaddr).ok_or("地址未映射")?;
        if !entry.is_present() {
            return Err("页表项未呈现".to_string());
        }
        let paddr = (entry.pfn << PAGE_SHIFT) | offset;
        Ok(PAddr(paddr))
    }

    /// 查询 PTE（供调试）。
    pub fn query_pte(
        tables: &PageTables,
        root: PageTableRoot,
        vaddr: VAddr,
    ) -> Option<PageTableEntry> {
        Self::walk_ref(tables, root, vaddr).copied()
    }
}

// ---------------------------------------------------------------
// 全局页表注册表：exec-id -> (根页框号, 页表集合)
// ---------------------------------------------------------------

/// 全局页表注册表。
struct PageTableRegistry {
    /// exec-id -> (根页框号, 页表集合)。
    spaces: BTreeMap<u64, (PageTableRoot, PageTables)>,
    /// 下一个可用页框号（模拟物理页框分配）。
    next_pfn: u64,
}

static PT_REGISTRY: OnceLock<Mutex<PageTableRegistry>> = OnceLock::new();

fn pt_registry() -> &'static Mutex<PageTableRegistry> {
    PT_REGISTRY.get_or_init(|| {
        Mutex::new(PageTableRegistry {
            spaces: BTreeMap::new(),
            next_pfn: 0x2000, // 从 0x2000 开始分配页表页
        })
    })
}

/// 为指定 exec-id 注册一个新地址空间（分配页表根）。
pub fn register_address_space(pid: u64) -> Result<PageTableRoot, String> {
    let mut g = pt_registry().lock().unwrap();
    if g.spaces.contains_key(&pid) {
        return Err(format!("地址空间已存在: pid={}", pid));
    }
    let root_pfn = g.next_pfn;
    g.next_pfn += 1;
    let root = PageTableRoot(root_pfn);
    g.spaces.insert(
        pid,
        (
            root,
            BTreeMap::from([(root_pfn, [PageTableEntry::new(); PT_LEVEL_ENTRIES])]),
        ),
    );
    Ok(root)
}

/// 获取指定 exec-id 的页表引用。
pub fn get_page_tables(pid: u64) -> Option<PageTableRoot> {
    let g = pt_registry().lock().unwrap();
    g.spaces.get(&pid).map(|(root, _)| *root)
}

/// 销毁指定 exec-id 的地址空间。
pub fn destroy_address_space(pid: u64) -> Result<(), String> {
    let mut g = pt_registry().lock().unwrap();
    if g.spaces.remove(&pid).is_none() {
        return Err(format!("地址空间不存在: pid={}", pid));
    }
    Ok(())
}

/// 页表统计。
pub fn pt_stats() -> String {
    let g = pt_registry().lock().unwrap();
    let spaces = g.spaces.len() as u64;
    let total_ptes: usize = g
        .spaces
        .values()
        .map(|(_, tables)| tables.len() * PT_LEVEL_ENTRIES)
        .sum();
    format!(
        "spaces={} total_page_table_pages={} total_pte_slots={} next_pfn=0x{:x}",
        spaces,
        g.spaces.values().map(|(_, t)| t.len()).sum::<usize>(),
        total_ptes,
        g.next_pfn
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_alloc_frame() -> Result<u64, String> {
        Ok(0x3000)
    }

    #[test]
    fn map_and_translate() {
        let pid = 100;
        let _root = register_address_space(pid).unwrap();
        let mut tables = BTreeMap::new();
        tables.insert(
            0x1000,
            [PageTableEntry::new(); PT_LEVEL_ENTRIES],
        );
        let root = PageTableRoot(0x1000);
        let vaddr = VAddr(0x1000);
        let paddr = PAddr(0x5000);
        let flags = PteFlags::PRESENT | PteFlags::READ_WRITE | PteFlags::USER;
        let mut alloc = || test_alloc_frame();
        PageTable::map(&mut tables, root, vaddr, paddr, flags, &mut alloc).unwrap();
        let result = PageTable::translate(&tables, root, vaddr).unwrap();
        assert_eq!(result, PAddr(0x1000 + 0));
        // 页内偏移：vaddr 0x1000 偏移为 0，pfn<<12 = 0x5000
        assert_eq!(result.0, 0x5000);
    }

    #[test]
    fn unmap_then_translate_fails() {
        let pid = 101;
        let _root = register_address_space(pid).unwrap();
        let mut tables = BTreeMap::new();
        tables.insert(
            0x1000,
            [PageTableEntry::new(); PT_LEVEL_ENTRIES],
        );
        let root = PageTableRoot(0x1000);
        let vaddr = VAddr(0x2000);
        let paddr = PAddr(0x6000);
        let flags = PteFlags::PRESENT | PteFlags::READ_WRITE;
        let mut alloc = || Ok(0x3000);
        PageTable::map(&mut tables, root, vaddr, paddr, flags, &mut alloc).unwrap();
        assert!(PageTable::translate(&tables, root, vaddr).is_ok());
        PageTable::unmap(&mut tables, root, vaddr).unwrap();
        assert!(PageTable::translate(&tables, root, vaddr).is_err());
    }

    #[test]
    fn pt_stats_contains_expected_fields() {
        let s = pt_stats();
        assert!(s.contains("spaces"));
        assert!(s.contains("total_page_table_pages"));
    }
}
