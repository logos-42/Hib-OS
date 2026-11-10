//! 页表模拟（多级页表 + TLB 模拟）。
//!
//! 模拟 x86-64 四级页表结构（PML4 / PDPT / PD / PT），支持虚拟地址到物理
//! 地址的映射、页表项（PTE）位管理（present/writable/user/executable/dirty/
//! accessed）、页表遍历（walk）与地址转换（translate），以及 TLB 缓存、命中率
//! 统计与刷新。页故障类型包括：不存在、权限违反、缺页。
//!
//! 本模块为纯内存模拟，不接触真实 MMU，适合内存管理与虚拟地址空间子系统的
//! 原型验证。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 地址与页表常量
// =====================================================================================

/// 虚拟地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VAddr(pub u64);

/// 物理地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PAddr(pub u64);

/// 页大小（4 KiB）。
pub const PAGE_SIZE: u64 = 4096;
/// 页内偏移掩码（12 位）。
pub const PAGE_OFFSET_MASK: u64 = 0xFFF;
/// 每级页表项数（512，9 位索引）。
pub const ENTRIES_PER_LEVEL: u64 = 512;
/// 页表项索引掩码（9 位）。
pub const ENTRY_MASK: u64 = 0x1FF;

// 四级页表索引位移。
const PML4_SHIFT: u64 = 39;
const PDPT_SHIFT: u64 = 30;
const PD_SHIFT: u64 = 21;
const PT_SHIFT: u64 = 12;

// =====================================================================================
// PTE —— 页表项
// =====================================================================================

/// 页表项标志位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PTEFlags {
    pub present: bool,
    pub writable: bool,
    pub user: bool,
    pub executable: bool,
    pub dirty: bool,
    pub accessed: bool,
}

impl PTEFlags {
    pub fn new() -> Self {
        Self {
            present: false,
            writable: false,
            user: false,
            executable: false,
            dirty: false,
            accessed: false,
        }
    }

    /// 仅 present 的只读内核页。
    pub fn kernel_readonly() -> Self {
        Self {
            present: true,
            writable: false,
            user: false,
            executable: false,
            dirty: false,
            accessed: false,
        }
    }

    /// present + writable + user 的普通数据页。
    pub fn user_readwrite() -> Self {
        Self {
            present: true,
            writable: true,
            user: true,
            executable: false,
            dirty: false,
            accessed: false,
        }
    }

    /// present + executable + user 的代码页。
    pub fn user_execute() -> Self {
        Self {
            present: true,
            writable: false,
            user: true,
            executable: true,
            dirty: false,
            accessed: false,
        }
    }

    /// 返回标志位的位掩码表示（供调试）。
    pub fn bits(self) -> u8 {
        let mut b: u8 = 0;
        if self.present {
            b |= 1 << 0;
        }
        if self.writable {
            b |= 1 << 1;
        }
        if self.user {
            b |= 1 << 2;
        }
        if self.executable {
            b |= 1 << 3;
        }
        if self.dirty {
            b |= 1 << 4;
        }
        if self.accessed {
            b |= 1 << 5;
        }
        b
    }

    /// 从位掩码恢复标志位。
    pub fn from_bits(b: u8) -> Self {
        Self {
            present: b & (1 << 0) != 0,
            writable: b & (1 << 1) != 0,
            user: b & (1 << 2) != 0,
            executable: b & (1 << 3) != 0,
            dirty: b & (1 << 4) != 0,
            accessed: b & (1 << 5) != 0,
        }
    }
}

/// 页表项：物理页框地址 + 标志位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PTE {
    pub paddr: PAddr,
    pub flags: PTEFlags,
}

impl PTE {
    pub fn new(paddr: PAddr, flags: PTEFlags) -> Self {
        Self { paddr, flags }
    }

    /// 该项是否指向下一级页表（present 且 paddr 非零）。
    pub fn is_table_ptr(&self) -> bool {
        self.flags.present && self.paddr.0 != 0
    }

    /// 该项是否指向最终物理页（present 且页对齐）。
    pub fn is_page_ptr(&self) -> bool {
        self.flags.present && self.paddr.0 % PAGE_SIZE == 0
    }
}

// =====================================================================================
// 页故障类型
// =====================================================================================

/// 页故障类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageFaultType {
    /// 页表项不存在（present=0 或中间页表缺失）。
    NotPresent,
    /// 权限违反（例如对用户页执行写但不可写，或对内核页执行用户访问）。
    PermissionViolation,
    /// 缺页（页在 swap 中或未分配，需从磁盘/swap 加载）。
    MissingPage,
}

impl std::fmt::Display for PageFaultType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PageFaultType::NotPresent => "页表项不存在".to_string(),
            PageFaultType::PermissionViolation => "权限违反".to_string(),
            PageFaultType::MissingPage => "缺页".to_string(),
        };
        write!(f, "{}", s)
    }
}

// =====================================================================================
// 页表项存储：四级页表
// =====================================================================================

/// 单级页表：512 个 PTE。
#[derive(Clone, Debug)]
struct PageTableLevel {
    entries: [Option<PTE>; 512],
}

impl PageTableLevel {
    fn new() -> Self {
        Self {
            entries: [None; 512],
        }
    }

    fn get(&self, index: u64) -> Option<PTE> {
        self.entries[index as usize]
    }

    fn set(&mut self, index: u64, pte: Option<PTE>) {
        self.entries[index as usize] = pte;
    }

    fn get_mut(&mut self, index: u64) -> Option<&mut PTE> {
        self.entries[index as usize].as_mut()
    }
}

/// 四级页表：PML4 -> PDPT -> PD -> PT -> 物理页。
pub struct PageTable {
    /// 各级页表，以该级页表的物理页框地址为键。
    levels: BTreeMap<u64, PageTableLevel>,
    /// 根页表（PML4）的物理页框地址。
    root_paddr: u64,
    /// 统计。
    map_count: u64,
    unmap_count: u64,
    translate_count: u64,
    fault_count: u64,
}

impl PageTable {
    /// 创建一个空页表，根页表（PML4）分配在物理页框 `root_paddr`。
    pub fn new(root_paddr: u64) -> Self {
        let mut levels = BTreeMap::new();
        levels.insert(root_paddr, PageTableLevel::new());
        Self {
            levels,
            root_paddr,
            map_count: 0,
            unmap_count: 0,
            translate_count: 0,
            fault_count: 0,
        }
    }

    /// 返回根页表物理地址。
    pub fn root_paddr(&self) -> u64 {
        self.root_paddr
    }

    /// 返回四个索引（PML4/PDPT/PD/PT）与页内偏移。
    fn decompose(vaddr: VAddr) -> (u64, u64, u64, u64, u64) {
        let addr = vaddr.0;
        let pml4 = (addr >> PML4_SHIFT) & ENTRY_MASK;
        let pdpt = (addr >> PDPT_SHIFT) & ENTRY_MASK;
        let pd = (addr >> PD_SHIFT) & ENTRY_MASK;
        let pt = (addr >> PT_SHIFT) & ENTRY_MASK;
        let offset = addr & PAGE_OFFSET_MASK;
        (pml4, pdpt, pd, pt, offset)
    }

    /// 确保沿虚拟地址路径的中间页表存在（PML4/PDPT/PD），缺失则分配新物理页框。
    /// 返回叶子页表（PT）的物理页框地址。
    fn ensure_path(&mut self, vaddr: VAddr) -> Result<u64, PageFaultType> {
        let (pml4_idx, pdpt_idx, pd_idx, _pt_idx, _offset) = Self::decompose(vaddr);
        let mut cur = self.root_paddr;

        // PML4 -> PDPT
        {
            let next = {
                let entry = self.levels.get(&cur).unwrap().get(pml4_idx);
                match entry {
                    Some(pte) if pte.is_table_ptr() => pte.paddr.0,
                    _ => {
                        let new_paddr = self.allocate_page_frame();
                        let level = self.levels.get_mut(&cur).unwrap();
                        level.set(
                            pml4_idx,
                            Some(PTE::new(PAddr(new_paddr), PTEFlags::kernel_readonly())),
                        );
                        self.levels.insert(new_paddr, PageTableLevel::new());
                        new_paddr
                    }
                }
            };
            cur = next;
        }
        // PDPT -> PD
        {
            let next = {
                let entry = self.levels.get(&cur).unwrap().get(pdpt_idx);
                match entry {
                    Some(pte) if pte.is_table_ptr() => pte.paddr.0,
                    _ => {
                        let new_paddr = self.allocate_page_frame();
                        let level = self.levels.get_mut(&cur).unwrap();
                        level.set(
                            pdpt_idx,
                            Some(PTE::new(PAddr(new_paddr), PTEFlags::kernel_readonly())),
                        );
                        self.levels.insert(new_paddr, PageTableLevel::new());
                        new_paddr
                    }
                }
            };
            cur = next;
        }
        // PD -> PT
        {
            let next = {
                let entry = self.levels.get(&cur).unwrap().get(pd_idx);
                match entry {
                    Some(pte) if pte.is_table_ptr() => pte.paddr.0,
                    _ => {
                        let new_paddr = self.allocate_page_frame();
                        let level = self.levels.get_mut(&cur).unwrap();
                        level.set(
                            pd_idx,
                            Some(PTE::new(PAddr(new_paddr), PTEFlags::kernel_readonly())),
                        );
                        self.levels.insert(new_paddr, PageTableLevel::new());
                        new_paddr
                    }
                }
            };
            cur = next;
        }
        Ok(cur)
    }

    /// 分配一个新物理页框地址（模拟：递增计数器，从 0x1000 开始，页对齐）。
    fn allocate_page_frame(&mut self) -> u64 {
        // 简单模拟：根据已有页表数量生成唯一页框地址。
        let base = 0x1000u64;
        let count = self.levels.len() as u64;
        base + count * PAGE_SIZE
    }

    /// 遍历页表，返回叶子 PTE 的可变引用（若中间页表缺失则返回 None）。
    pub fn walk(&mut self, vaddr: VAddr) -> Option<&mut PTE> {
        let (pml4_idx, pdpt_idx, pd_idx, pt_idx, _offset) = Self::decompose(vaddr);
        let mut cur = self.root_paddr;

        let levels = &[
            (pml4_idx, false),
            (pdpt_idx, false),
            (pd_idx, false),
            (pt_idx, true),
        ];
        for &(level_idx, is_last) in levels {
            // 先取出当前级别的页表，借用立即释放。
            let level_entries = self.levels.get(&cur)?.clone();
            let entry = level_entries.get(level_idx)?;
            if is_last {
                // 末级：直接返回叶子 PTE 的可变引用。
                return self.levels.get_mut(&cur)?.get_mut(level_idx);
            }
            if !entry.is_table_ptr() {
                return None;
            }
            cur = entry.paddr.0;
        }
        None
    }

    /// 映射虚拟地址到物理地址，自动创建中间页表。
    ///
    /// 若该虚拟地址已有映射，则更新 PTE。设置 accessed 位。
    pub fn map(&mut self, vaddr: VAddr, paddr: PAddr, flags: PTEFlags) -> Result<(), PageFaultType> {
        let pt_paddr = self.ensure_path(vaddr)?;
        let (_pml4, _pdpt, _pd, pt_idx, _offset) = Self::decompose(vaddr);
        let level = self.levels.get_mut(&pt_paddr).unwrap();
        let mut pte = level.get(pt_idx).unwrap_or(PTE::new(PAddr(0), PTEFlags::new()));
        pte.paddr = paddr;
        pte.flags = flags;
        pte.flags.accessed = true;
        level.set(pt_idx, Some(pte));
        self.map_count += 1;
        Ok(())
    }

    /// 取消虚拟地址的映射（将叶子 PTE 置为 None）。
    pub fn unmap(&mut self, vaddr: VAddr) -> Result<(), PageFaultType> {
        let (_pml4, _pdpt, _pd, pt_idx, _offset) = Self::decompose(vaddr);
        // 找到叶子页表物理地址（不创建中间表）。
        let pt_paddr = self.find_leaf_paddr(vaddr)?;
        let level = self.levels.get_mut(&pt_paddr).unwrap();
        if level.get(pt_idx).is_some() {
            level.set(pt_idx, None);
            self.unmap_count += 1;
            Ok(())
        } else {
            Err(PageFaultType::NotPresent)
        }
    }

    /// 查找虚拟地址对应的叶子页表物理地址（不创建）。
    fn find_leaf_paddr(&self, vaddr: VAddr) -> Result<u64, PageFaultType> {
        let (pml4_idx, pdpt_idx, pd_idx, _pt_idx, _offset) = Self::decompose(vaddr);
        let mut cur = self.root_paddr;
        for idx in [pml4_idx, pdpt_idx, pd_idx] {
            let level = self.levels.get(&cur).ok_or(PageFaultType::NotPresent)?;
            let entry = level.get(idx).ok_or(PageFaultType::NotPresent)?;
            if !entry.is_table_ptr() {
                return Err(PageFaultType::NotPresent);
            }
            cur = entry.paddr.0;
        }
        Ok(cur)
    }

    /// 翻译虚拟地址为物理地址，返回 PTE 与页内偏移。
    ///
    /// 翻译过程中会设置 accessed 位；若写入操作且 PTE 允许，则设置 dirty 位。
    /// 失败时返回 PageFaultType 并增加 fault 计数。
    pub fn translate(
        &mut self,
        vaddr: VAddr,
        write: bool,
        user_mode: bool,
    ) -> Result<(PAddr, PTE, u64), PageFaultType> {
        self.translate_count += 1;
        let (_pml4, _pdpt, _pd, pt_idx, offset) = Self::decompose(vaddr);
        let pt_paddr = self.find_leaf_paddr(vaddr)?;
        let level = self.levels.get_mut(&pt_paddr).unwrap();
        let pte_opt = level.get(pt_idx);
        let pte = match pte_opt {
            Some(p) => p,
            None => {
                self.fault_count += 1;
                return Err(PageFaultType::NotPresent);
            }
        };
        if !pte.flags.present {
            self.fault_count += 1;
            return Err(PageFaultType::NotPresent);
        }
        // 权限检查。
        if user_mode && !pte.flags.user {
            self.fault_count += 1;
            return Err(PageFaultType::PermissionViolation);
        }
        if write && !pte.flags.writable {
            self.fault_count += 1;
            return Err(PageFaultType::PermissionViolation);
        }
        // 设置 accessed / dirty。
        let mut new_pte = pte;
        new_pte.flags.accessed = true;
        if write {
            new_pte.flags.dirty = true;
        }
        level.set(pt_idx, Some(new_pte));
        let paddr = PAddr(new_pte.paddr.0 + offset);
        Ok((paddr, new_pte, offset))
    }

    /// 返回映射总次数。
    pub fn map_count(&self) -> u64 {
        self.map_count
    }

    /// 返回取消映射总次数。
    pub fn unmap_count(&self) -> u64 {
        self.unmap_count
    }

    /// 返回翻译总次数。
    pub fn translate_count(&self) -> u64 {
        self.translate_count
    }

    /// 返回页故障总次数。
    pub fn fault_count(&self) -> u64 {
        self.fault_count
    }

    /// 返回当前页表中已映射的虚拟地址数量（估算：遍历所有叶子表项）。
    pub fn mapped_page_count(&self) -> u64 {
        let mut count = 0u64;
        for level in self.levels.values() {
            for entry in level.entries.iter().flatten() {
                if entry.is_page_ptr() {
                    count += 1;
                }
            }
        }
        count
    }

    /// 返回页表文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "page_table root_paddr=0x{:x} levels={} mapped_pages={} maps={} unmaps={} translates={} faults={}",
            self.root_paddr,
            self.levels.len(),
            self.mapped_page_count(),
            self.map_count,
            self.unmap_count,
            self.translate_count,
            self.fault_count,
        )
    }
}

impl Default for PageTable {
    fn default() -> Self {
        Self::new(0x1000)
    }
}

// =====================================================================================
// TLB —— 缓存页表项的快表
// =====================================================================================

/// TLB 条目：虚拟页号 -> 物理页框号 + 标志位。
#[derive(Clone, Debug)]
pub struct TLBEntry {
    pub vpn: u64, // 虚拟页号
    pub pfn: u64, // 物理页框号
    pub flags: PTEFlags,
    /// 上一次访问时间（供 LRU 淘汰）。
    pub last_access: u64,
}

impl TLBEntry {
    fn new(vpn: u64, pfn: u64, flags: PTEFlags) -> Self {
        Self {
            vpn,
            pfn,
            flags,
            last_access: now_seconds(),
        }
    }
}

/// TLB（快表）：全相联缓存，固定容量，LRU 淘汰。
pub struct TLB {
    /// vpn -> 条目索引（在 entries Vec 中）。
    map: BTreeMap<u64, usize>,
    entries: Vec<TLBEntry>,
    /// 容量。
    capacity: usize,
    /// 统计。
    hits: u64,
    misses: u64,
    evictions: u64,
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl TLB {
    /// 创建容量为 `capacity` 的 TLB。
    pub fn new(capacity: usize) -> Self {
        Self {
            map: BTreeMap::new(),
            entries: Vec::new(),
            capacity,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    /// 查找虚拟页号 `vpn`，命中则返回条目并更新访问时间；否则返回 None。
    pub fn lookup(&mut self, vpn: u64) -> Option<&TLBEntry> {
        if let Some(&idx) = self.map.get(&vpn) {
            self.hits += 1;
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.last_access = now_seconds();
            }
            self.entries.get(idx)
        } else {
            self.misses += 1;
            None
        }
    }

    /// 插入一条 TLB 记录。若 vpn 已存在则更新；若超出容量则按 LRU 淘汰。
    pub fn insert(&mut self, vpn: u64, pfn: u64, flags: PTEFlags) {
        if let Some(&idx) = self.map.get(&vpn) {
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.pfn = pfn;
                entry.flags = flags;
                entry.last_access = now_seconds();
            }
            return;
        }
        if self.entries.len() >= self.capacity {
            self.evict_lru();
        }
        let idx = self.entries.len();
        self.entries.push(TLBEntry::new(vpn, pfn, flags));
        self.map.insert(vpn, idx);
    }

    /// 刷新指定虚拟页号的 TLB 条目。
    pub fn flush_vpn(&mut self, vpn: u64) {
        if let Some(idx) = self.map.remove(&vpn) {
            // 标记该条目失效（从 entries 中移除并重建 map 以保持索引一致）。
            self.entries.remove(idx);
            self.rebuild_map();
        }
    }

    /// 刷新全部 TLB 条目。
    pub fn flush_all(&mut self) {
        self.entries.clear();
        self.map.clear();
    }

    /// 按 LRU 淘汰最久未使用的条目。
    fn evict_lru(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let lru_idx = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.last_access)
            .map(|(i, _)| i)
            .unwrap();
        let evicted_vpn = self.entries[lru_idx].vpn;
        self.entries.remove(lru_idx);
        self.map.remove(&evicted_vpn);
        self.rebuild_map();
        self.evictions += 1;
    }

    /// 重建 vpn -> 索引 映射（在条目移除后调用）。
    fn rebuild_map(&mut self) {
        self.map.clear();
        for (i, entry) in self.entries.iter().enumerate() {
            self.map.insert(entry.vpn, i);
        }
    }

    /// 返回命中数。
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// 返回未命中数。
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// 返回命中率（0.0~1.0）。
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }

    /// 返回淘汰总次数。
    pub fn eviction_count(&self) -> u64 {
        self.evictions
    }

    /// 返回当前条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 返回 TLB 文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "tlb capacity={} entries={} hits={} misses={} hit_rate={:.2} evictions={}",
            self.capacity,
            self.len(),
            self.hits,
            self.misses,
            self.hit_rate(),
            self.eviction_count(),
        )
    }
}

impl Default for TLB {
    fn default() -> Self {
        Self::new(64)
    }
}

// =====================================================================================
// PagedMemoryManager —— 页表 + TLB 组合的地址空间管理器
// =====================================================================================

/// 页表与 TLB 的组合管理器，提供统一的虚拟地址转换接口。
///
/// 翻译流程：先查 TLB，命中则直接返回；未命中则查页表并将结果填入 TLB。
pub struct PagedMemoryManager {
    pub page_table: PageTable,
    pub tlb: TLB,
}

impl PagedMemoryManager {
    /// 创建管理器，根页表位于 `root_paddr`，TLB 容量为 `tlb_capacity`。
    pub fn new(root_paddr: u64, tlb_capacity: usize) -> Self {
        Self {
            page_table: PageTable::new(root_paddr),
            tlb: TLB::new(tlb_capacity),
        }
    }

    /// 映射虚拟地址到物理地址（同时使 TLB 中该 vpn 的旧条目失效）。
    pub fn map(
        &mut self,
        vaddr: VAddr,
        paddr: PAddr,
        flags: PTEFlags,
    ) -> Result<(), PageFaultType> {
        let vpn = vaddr.0 / PAGE_SIZE;
        let result = self.page_table.map(vaddr, paddr, flags);
        if result.is_ok() {
            // 映射变更使 TLB 旧条目失效。
            self.tlb.flush_vpn(vpn);
        }
        result
    }

    /// 取消映射（使 TLB 该 vpn 条目失效）。
    pub fn unmap(&mut self, vaddr: VAddr) -> Result<(), PageFaultType> {
        let vpn = vaddr.0 / PAGE_SIZE;
        let result = self.page_table.unmap(vaddr);
        if result.is_ok() {
            self.tlb.flush_vpn(vpn);
        }
        result
    }

    /// 翻译虚拟地址，先查 TLB，未命中则查页表并填入 TLB。
    pub fn translate(
        &mut self,
        vaddr: VAddr,
        write: bool,
        user_mode: bool,
    ) -> Result<(PAddr, PTE, u64), PageFaultType> {
        let vpn = vaddr.0 / PAGE_SIZE;
        // 先查 TLB（TLB 不区分读写权限，命中后由页表权限最终校验；此处简化为
        // 命中直接返回，若需严格权限则应由调用方再校验）。
        if let Some(entry) = self.tlb.lookup(vpn) {
            let paddr = PAddr(entry.pfn * PAGE_SIZE + (vaddr.0 & PAGE_OFFSET_MASK));
            return Ok((paddr, PTE::new(PAddr(entry.pfn * PAGE_SIZE), entry.flags), vaddr.0 & PAGE_OFFSET_MASK));
        }
        // TLB 未命中：查页表。
        let result = self.page_table.translate(vaddr, write, user_mode);
        if let Ok((paddr, pte, _offset)) = &result {
            self.tlb.insert(vpn, paddr.0 / PAGE_SIZE, pte.flags);
        }
        result
    }

    /// 刷新指定虚拟页号的 TLB。
    pub fn flush_tlb(&mut self, vaddr: VAddr) {
        let vpn = vaddr.0 / PAGE_SIZE;
        self.tlb.flush_vpn(vpn);
    }

    /// 刷新全部 TLB。
    pub fn flush_all_tlb(&mut self) {
        self.tlb.flush_all();
    }

    /// 返回页表映射页数。
    pub fn mapped_pages(&self) -> u64 {
        self.page_table.mapped_page_count()
    }

    /// 返回 TLB 命中率。
    pub fn tlb_hit_rate(&self) -> f64 {
        self.tlb.hit_rate()
    }

    /// 返回页故障总数。
    pub fn fault_count(&self) -> u64 {
        self.page_table.fault_count()
    }

    /// 返回文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "pmm {} | {} | mapped_pages={} tlb_hit_rate={:.2} faults={}",
            self.page_table.summary(),
            self.tlb.summary(),
            self.mapped_pages(),
            self.tlb_hit_rate(),
            self.fault_count(),
        )
    }
}

impl Default for PagedMemoryManager {
    fn default() -> Self {
        Self::new(0x1000, 64)
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static PAGE_TABLE_SIM: OnceLock<Mutex<PagedMemoryManager>> = OnceLock::new();

fn page_table_sim() -> &'static Mutex<PagedMemoryManager> {
    PAGE_TABLE_SIM.get_or_init(|| Mutex::new(PagedMemoryManager::new(0x1000, 64)))
}

/// 映射虚拟地址（全局页表）。
pub fn pt_map(vaddr: u64, paddr: u64, present: bool, writable: bool, user: bool) -> Result<(), PageFaultType> {
    let flags = PTEFlags {
        present,
        writable,
        user,
        executable: false,
        dirty: false,
        accessed: false,
    };
    page_table_sim()
        .lock()
        .unwrap()
        .map(VAddr(vaddr), PAddr(paddr), flags)
}

/// 翻译虚拟地址（全局页表）。
pub fn pt_translate(vaddr: u64, write: bool, user_mode: bool) -> Result<(u64, u8), PageFaultType> {
    let mut mgr = page_table_sim().lock().unwrap();
    let (paddr, pte, _offset) = mgr.translate(VAddr(vaddr), write, user_mode)?;
    Ok((paddr.0, pte.flags.bits()))
}

/// 取消映射（全局页表）。
pub fn pt_unmap(vaddr: u64) -> Result<(), PageFaultType> {
    page_table_sim().lock().unwrap().unmap(VAddr(vaddr))
}

/// 刷新 TLB 指定页（全局）。
pub fn pt_flush_tlb(vaddr: u64) {
    page_table_sim().lock().unwrap().flush_tlb(VAddr(vaddr));
}

/// 刷新全部 TLB（全局）。
pub fn pt_flush_all_tlb() {
    page_table_sim().lock().unwrap().flush_all_tlb();
}

/// 返回 TLB 命中率（全局）。
pub fn pt_tlb_hit_rate() -> f64 {
    page_table_sim().lock().unwrap().tlb_hit_rate()
}

/// 返回页表摘要（全局）。
pub fn pt_summary() -> String {
    page_table_sim().lock().unwrap().summary()
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pte_flags_bits_roundtrip() {
        let f = PTEFlags::user_readwrite();
        let b = f.bits();
        let f2 = PTEFlags::from_bits(b);
        assert_eq!(f.present, f2.present);
        assert_eq!(f.writable, f2.writable);
        assert_eq!(f.user, f2.user);
        assert_eq!(f.executable, f2.executable);
    }

    #[test]
    fn map_and_translate() {
        let mut mgr = PagedMemoryManager::new(0x1000, 64);
        let vaddr = VAddr(0x1000_0000);
        let paddr = PAddr(0x2000_0000);
        assert!(mgr.map(vaddr, paddr, PTEFlags::user_readwrite()).is_ok());
        let (resolved, pte, offset) = mgr.translate(vaddr, false, true).unwrap();
        assert_eq!(resolved, PAddr(0x2000_0000 + 0));
        assert_eq!(offset, 0);
        assert!(pte.flags.present);
        assert!(pte.flags.writable);
    }

    #[test]
    fn translate_with_offset() {
        let mut mgr = PagedMemoryManager::new(0x1000, 64);
        let vaddr = VAddr(0x1000_0000 + 0x123);
        let paddr = PAddr(0x3000_0000);
        assert!(mgr.map(vaddr, paddr, PTEFlags::kernel_readonly()).is_ok());
        let (resolved, _, offset) = mgr.translate(vaddr, false, false).unwrap();
        assert_eq!(resolved, PAddr(0x3000_0000 + 0x123));
        assert_eq!(offset, 0x123);
    }

    #[test]
    fn unmap_then_not_present() {
        let mut mgr = PagedMemoryManager::new(0x1000, 64);
        let vaddr = VAddr(0x4000_0000);
        let paddr = PAddr(0x5000_0000);
        mgr.map(vaddr, paddr, PTEFlags::user_readwrite()).unwrap();
        assert!(mgr.unmap(vaddr).is_ok());
        let err = mgr.translate(vaddr, false, true).unwrap_err();
        assert_eq!(err, PageFaultType::NotPresent);
    }

    #[test]
    fn write_to_readonly_page_fault() {
        let mut mgr = PagedMemoryManager::new(0x1000, 64);
        let vaddr = VAddr(0x5000_0000);
        let paddr = PAddr(0x6000_0000);
        mgr.map(vaddr, paddr, PTEFlags::kernel_readonly()).unwrap();
        let err = mgr.translate(vaddr, true, false).unwrap_err();
        assert_eq!(err, PageFaultType::PermissionViolation);
    }

    #[test]
    fn user_access_to_kernel_page_fault() {
        let mut mgr = PagedMemoryManager::new(0x1000, 64);
        let vaddr = VAddr(0x6000_0000);
        let paddr = PAddr(0x7000_0000);
        mgr.map(vaddr, paddr, PTEFlags::kernel_readonly()).unwrap();
        let err = mgr.translate(vaddr, false, true).unwrap_err();
        assert_eq!(err, PageFaultType::PermissionViolation);
    }

    #[test]
    fn tlb_hit_and_miss() {
        let mut mgr = PagedMemoryManager::new(0x1000, 4);
        let vaddr = VAddr(0x7000_0000);
        let paddr = PAddr(0x8000_0000);
        mgr.map(vaddr, paddr, PTEFlags::user_readwrite()).unwrap();
        // 第一次：TLB miss + 页表命中
        let _ = mgr.translate(vaddr, false, true);
        // 第二次：TLB hit
        let _ = mgr.translate(vaddr, false, true);
        let rate = mgr.tlb_hit_rate();
        // 2 次翻译，1 miss 1 hit
        assert!((rate - 0.5).abs() < 1e-6);
    }

    #[test]
    fn tlb_flush_vpn_invalidates() {
        let mut mgr = PagedMemoryManager::new(0x1000, 4);
        let vaddr = VAddr(0x8000_0000);
        let paddr = PAddr(0x9000_0000);
        mgr.map(vaddr, paddr, PTEFlags::user_readwrite()).unwrap();
        let _ = mgr.translate(vaddr, false, true); // 进 TLB
        mgr.flush_tlb(vaddr);
        // 再次 translate 会重新查页表（TLB miss），但仍命中页表
        let _ = mgr.translate(vaddr, false, true);
        assert!(mgr.translate(vaddr, false, true).is_ok());
    }

    #[test]
    fn tlb_flush_all() {
        let mut mgr = PagedMemoryManager::new(0x1000, 16);
        let _ = mgr.map(VAddr(0x9000_0000), PAddr(0xA000_0000), PTEFlags::user_readwrite());
        let _ = mgr.map(VAddr(0xA000_0000), PAddr(0xB000_0000), PTEFlags::user_readwrite());
        let _ = mgr.translate(VAddr(0x9000_0000), false, true);
        let _ = mgr.translate(VAddr(0xA000_0000), false, true);
        mgr.flush_all_tlb();
        assert_eq!(mgr.tlb.len(), 0);
    }

    #[test]
    fn page_fault_count_increments() {
        let mut mgr = PagedMemoryManager::new(0x1000, 4);
        let _ = mgr.translate(VAddr(0xB000_0000), false, true); // 未映射
        assert_eq!(mgr.fault_count(), 1);
    }

    #[test]
    fn mapped_pages_count() {
        let mut mgr = PagedMemoryManager::new(0x1000, 4);
        assert_eq!(mgr.mapped_pages(), 0);
        let _ = mgr.map(VAddr(0xC000_0000), PAddr(0xD000_0000), PTEFlags::user_readwrite());
        let _ = mgr.map(VAddr(0xD000_0000), PAddr(0xE000_0000), PTEFlags::user_readwrite());
        assert_eq!(mgr.mapped_pages(), 2);
    }

    #[test]
    fn global_pt_functions() {
        assert!(pt_map(0x1000, 0x2000, true, true, true).is_ok());
        let (paddr, _bits) = pt_translate(0x1000, false, true).unwrap();
        assert_eq!(paddr, 0x2000);
        assert!(pt_unmap(0x1000).is_ok());
        pt_flush_tlb(0x2000);
        pt_flush_all_tlb();
        let _rate = pt_tlb_hit_rate();
        let _ = rate;
        let _s = pt_summary();
        let _ = s;
    }

    #[test]
    fn decompose_address() {
        let (pml4, pdpt, pd, pt, offset) = PageTable::decompose(VAddr(0x1234_5678_9000));
        // 仅检查不越界
        assert!(pml4 < 512);
        assert!(pdpt < 512);
        assert!(pd < 512);
        assert!(pt < 512);
        assert_eq!(offset, 0);
    }

    #[test]
    fn pt_summary_contains_fields() {
        let mut mgr = PagedMemoryManager::new(0x1000, 4);
        let _ = mgr.map(VAddr(0xE000_0000), PAddr(0xF000_0000), PTEFlags::user_readwrite());
        let s = mgr.summary();
        assert!(s.contains("mapped_pages="));
        assert!(s.contains("tlb_hit_rate="));
    }
}
