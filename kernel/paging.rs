//! 真实分页与地址空间 —— x86-64 四级页表（AI 原生包装为 `mem/paging`）。
//!
//! 这是"补齐 Linux 设计"的第一块地基。它实现的是**真正的内核分页逻辑**：
//!
//! - 48 位虚拟地址，4KB 页，标准四级页表（PML4 → PDPT → PD → PT），
//!   每级 512 项、每项 8 字节；
//! - 页表项语义按 Intel SDM 来：P / R/W / U/S / A / D / NX，外加一个软件位 COW；
//! - 地址翻译 `translate()` 做完整的 4 级 walk；
//! - **缺页**：未映射或 P=0 的访问会被识别为 page fault；
//! - **写时复制 (COW)**：`fork` 一个地址空间时，只读共享物理页并打 COW 标记，
//!   子空间首次写入触发缺页 → 真正复制那一页（与 Linux `fork` 一致）；
//! - 每个地址空间独立的 PML4，互不可见。
//!
//! 物理内存用一个 `Vec<u8>` 后台存储 + 位图帧分配器模拟；页表页本身也占物理帧。
//! 运行环境是宿主进程，但**分页算法是真实内核代码**——只是跑在软件 MMU 上。
//!
//! 通道契约：
//! ```text
//! sense mem/paging
//! act mem/paging space new
//! act mem/paging space switch <id>
//! act mem/paging alloc <vaddr>            # 分配一物理帧并映射(读写/用户)
//! act mem/paging map <vaddr> <paddr> <flags>
//! act mem/paging unmap <vaddr>
//! act mem/paging translate <vaddr>
//! act mem/paging fault <vaddr> <r|w>
//! act mem/paging fork <new_id>            # 复制当前空间为 COW
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

pub const PAGE_SIZE: u64 = 4096;
pub const ENTRIES: usize = 512;
pub const PHYS_MEM_FRAMES: u64 = 256 * 1024; // 1 GiB / 4KB

// 页表项位（Intel SDM 兼容）。
pub const PTE_P: u64 = 1 << 0; // present
pub const PTE_RW: u64 = 1 << 1; // 可写
pub const PTE_US: u64 = 1 << 2; // 用户态可访问
pub const PTE_A: u64 = 1 << 5; // accessed
pub const PTE_D: u64 = 1 << 6; // dirty
pub const PTE_COW: u64 = 1 << 9; // 软件自定义：写时复制
pub const PTE_NX: u64 = 1 << 63; // no-execute
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[inline]
pub fn pte_addr(p: u64) -> u64 {
    p & ADDR_MASK
}
#[inline]
fn pte_low_flags(p: u64) -> u64 {
    // 仅属性位（不含地址），但保留 P 以便判存在。
    (p & !ADDR_MASK) & !(0x7FFu64 << 52) // 清掉保留高位，保留下部位
}
#[inline]
fn make_pte(addr: u64, flags: u64) -> u64 {
    (addr & ADDR_MASK) | (flags & !ADDR_MASK)
}

fn pml4_index(v: u64) -> usize {
    ((v >> 39) & 0x1ff) as usize
}
fn pdpt_index(v: u64) -> usize {
    ((v >> 30) & 0x1ff) as usize
}
fn pd_index(v: u64) -> usize {
    ((v >> 21) & 0x1ff) as usize
}
fn pt_index(v: u64) -> usize {
    ((v >> 12) & 0x1ff) as usize
}

/// 物理帧分配器（位图/空闲栈）。
pub struct FrameAllocator {
    base: u64,
    total: u64,
    used: u64,
    free: Vec<u64>,
}

impl FrameAllocator {
    fn new(total_frames: u64, base: u64) -> Self {
        let mut free = Vec::with_capacity(total_frames as usize);
        // 预留前 256 帧给内核镜像 / 各级页表根，不参与分配。
        for i in 256..total_frames {
            free.push(base + i * PAGE_SIZE);
        }
        FrameAllocator {
            base,
            total: total_frames,
            used: 256,
            free,
        }
    }
    fn alloc(&mut self) -> Option<u64> {
        let f = self.free.pop()?;
        self.used += 1;
        Some(f)
    }
    fn free(&mut self, f: u64) {
        self.free.push(f);
        self.used -= 1;
    }
}

/// 所有页表页的集合（key = 页表所在物理帧地址）。
pub struct PageTables {
    tables: BTreeMap<u64, Box<[u64; ENTRIES]>>,
}

impl PageTables {
    fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }
    fn alloc_table(&mut self, frames: &mut FrameAllocator) -> Option<u64> {
        let f = frames.alloc()?;
        self.tables.insert(f, Box::new([0u64; ENTRIES]));
        Some(f)
    }
    fn get(&self, f: u64) -> Option<&[u64; ENTRIES]> {
        self.tables.get(&f).map(|b| &**b)
    }
    fn get_mut(&mut self, f: u64) -> Option<&mut [u64; ENTRIES]> {
        self.tables.get_mut(&f).map(|b| &mut **b)
    }
    fn free_table(&mut self, f: u64, frames: &mut FrameAllocator) {
        self.tables.remove(&f);
        frames.free(f);
    }

    /// 把 vaddr → paddr 映射进以 `root`(PML4 帧) 为根的四级页表。
    fn map(&mut self, frames: &mut FrameAllocator, mut root: u64, vaddr: u64, paddr: u64, flags: u64) {
        let idxs = [pml4_index(vaddr), pdpt_index(vaddr), pd_index(vaddr)];
        for &i in idxs.iter() {
            let entry = {
                let table = self.get_mut(root).expect("页表根缺失");
                table[i]
            };
            if entry & PTE_P == 0 {
                let nf = self.alloc_table(frames).expect("无空闲帧分配页表");
                {
                    let table = self.get_mut(root).unwrap();
                    table[i] = make_pte(nf, PTE_P | PTE_RW | PTE_US);
                }
                root = nf;
            } else {
                root = pte_addr(entry);
            }
        }
        let pt = self.get_mut(root).unwrap();
        pt[pt_index(vaddr)] = make_pte(paddr, flags | PTE_P);
    }

    /// 完整 4 级 walk，返回 (物理地址, 低位属性)。
    fn translate(&self, mut root: u64, vaddr: u64) -> Option<(u64, u64)> {
        let idxs = [pml4_index(vaddr), pdpt_index(vaddr), pd_index(vaddr)];
        for &i in idxs.iter() {
            let e = *self.get(root)?.get(i)?;
            if e & PTE_P == 0 {
                return None;
            }
            root = pte_addr(e);
        }
        let pt = self.get(root)?;
        let e = pt[pt_index(vaddr)];
        if e & PTE_P == 0 {
            return None;
        }
        Some((pte_addr(e), pte_low_flags(e)))
    }

    fn unmap(&mut self, frames: &mut FrameAllocator, mut root: u64, vaddr: u64) -> Option<u64> {
        let idxs = [pml4_index(vaddr), pdpt_index(vaddr), pd_index(vaddr)];
        for &i in idxs.iter() {
            let e = *self.get(root)?.get(i)?;
            if e & PTE_P == 0 {
                return None;
            }
            root = pte_addr(e);
        }
        let pt = self.get_mut(root)?;
        let old = pt[pt_index(vaddr)];
        if old & PTE_P == 0 {
            return None;
        }
        pt[pt_index(vaddr)] = 0;
        Some(pte_addr(old))
    }

    /// 深度复制整棵页表树（COW fork 用），并对用户可写页打 COW 标记。
    fn clone_cow(&mut self, frames: &mut FrameAllocator, root: u64) -> Option<u64> {
        let new_root = self.alloc_table(frames)?;
        PageTables::clone_level(&mut self.tables, frames, new_root, root, 0);
        Some(new_root)
    }

    /// 静态递归复制层级，避免 &mut self 与 &mut self.tables 双重借用。
    ///
    /// 先把源页表整页 clone 出来（不可变借用立即释放），再分配新帧并修改目标页表。
    fn clone_level(
        tables: &mut BTreeMap<u64, Box<[u64; ENTRIES]>>,
        frames: &mut FrameAllocator,
        dst: u64,
        src: u64,
        level: u32,
    ) {
        // 第一步：把源页表整页 clone 出来，避免与后续可变借用冲突。
        // 把源页表整页 clone 出来（不可变借用，立即释放）。
        let st: [u64; ENTRIES] = *tables.get(&src).expect("源页表缺失").clone();
        // 分配新帧并插入空目标页表。
        tables.insert(dst, Box::new([0u64; ENTRIES]));
        for i in 0..ENTRIES {
            let e = st[i];
            if e & PTE_P == 0 {
                continue;
            }
            if level == 3 {
                // 末级 PT：可写用户页改为只读 + COW 共享。
                if e & PTE_RW != 0 && e & PTE_US != 0 {
                    if let Some(dt) = tables.get_mut(&dst) {
                        dt[i] = (e & !PTE_RW) | PTE_COW;
                    }
                } else {
                    if let Some(dt) = tables.get_mut(&dst) {
                        dt[i] = e;
                    }
                }
            } else {
                // 中间级：递归复制子页表。
                let child = pte_addr(e);
                let new_child = frames.alloc().expect("复制页表无帧");
                tables.insert(new_child, Box::new([0u64; ENTRIES]));
                // 递归复制子级（会修改 tables，故每次循环后重新获取目标引用）。
                PageTables::clone_level(tables, frames, new_child, child, level + 1);
                if let Some(dt) = tables.get_mut(&dst) {
                    dt[i] = make_pte(new_child, e & !ADDR_MASK);
                }
            }
        }
    }

    /// 释放整棵页表树（地址空间销毁时），回收所有页表帧。
    fn drop_tree(&mut self, frames: &mut FrameAllocator, root: u64, level: u32) {
        if let Some(t) = self.tables.get(&root).cloned() {
            if level < 3 {
                for i in 0..ENTRIES {
                    let e = t[i];
                    if e & PTE_P != 0 {
                        self.drop_tree(frames, pte_addr(e), level + 1);
                    }
                }
            }
            self.free_table(root, frames);
        }
    }
}

/// 一个独立地址空间（进程视角）。
pub struct AddressSpace {
    pub id: u32,
    pub pml4: u64,
    pub regions: Vec<(u64, u64)>, // (vaddr, len)
}

/// 全局分页状态：物理内存后台 + 帧分配器 + 所有页表 + 地址空间表。
pub struct GlobalPaging {
    frames: FrameAllocator,
    ram: Vec<u8>, // 物理内存内容（用于真实 COW 复制）
    tables: PageTables,
    spaces: BTreeMap<u32, AddressSpace>,
    next_space: u32,
    current: Option<u32>,
    faults: u64,
    cow_faults: u64,
    allocs: u64,
    /// 缺页等待表：vaddr -> 等待该页被唤醒的 exec-id 列表（模拟缺页阻塞）。
    fault_waiters: BTreeMap<u64, Vec<u64>>,
}

impl GlobalPaging {
    fn new() -> Self {
        let total = PHYS_MEM_FRAMES;
        let ram_size = (total * PAGE_SIZE) as usize;
        let mut g = GlobalPaging {
            frames: FrameAllocator::new(total, 0),
            ram: vec![0u8; ram_size],
            tables: PageTables::new(),
            spaces: BTreeMap::new(),
            next_space: 1,
            current: None,
            faults: 0,
            cow_faults: 0,
            allocs: 0,
            fault_waiters: BTreeMap::new(),
        };
        // 内核地址空间 0：建一个 PML4 根。
        let root = g.tables.alloc_table(&mut g.frames).expect("内核 PML4");
        g.spaces.insert(
            0,
            AddressSpace {
                id: 0,
                pml4: root,
                regions: Vec::new(),
            },
        );
        g.current = Some(0);
        g
    }

    fn current_root(&self) -> Option<u64> {
        self.current.and_then(|id| self.spaces.get(&id).map(|s| s.pml4))
    }

    fn phys_write(&mut self, paddr: u64, buf: &[u8]) {
        let s = paddr as usize;
        self.ram[s..s + buf.len()].copy_from_slice(buf);
    }
    fn phys_read(&self, paddr: u64, len: usize) -> Vec<u8> {
        self.ram[paddr as usize..paddr as usize + len].to_vec()
    }
    fn phys_copy(&mut self, dst: u64, src: u64) {
        let s = src as usize;
        let d = dst as usize;
        let len = PAGE_SIZE as usize;
        if s < d {
            let (left, right) = self.ram.split_at_mut(d);
            right[..len].copy_from_slice(&left[s..s + len]);
        } else {
            let (left, right) = self.ram.split_at_mut(s);
            left[d..d + len].copy_from_slice(&right[..len]);
        }
    }

    fn map(&mut self, vaddr: u64, paddr: u64, flags: u64) -> Result<String, String> {
        let root = self.current_root().ok_or("无当前地址空间")?;
        self.tables.map(&mut self.frames, root, vaddr, paddr, flags);
        self.allocs += 1;
        if let Some(s) = self.spaces.get_mut(&self.current.unwrap()) {
            s.regions.push((vaddr, PAGE_SIZE));
        }
        Ok(format!(
            "map v={:#x} -> p={:#x} flags={:#x}",
            vaddr, paddr, flags
        ))
    }

    fn alloc_page(&mut self, vaddr: u64) -> Result<String, String> {
        let paddr = self.frames.alloc().ok_or("无空闲物理帧")?;
        self.phys_write(paddr, &[0xabu8; PAGE_SIZE as usize]); // 触达物理内存
        let flags = PTE_RW | PTE_US;
        self.map(vaddr, paddr, flags)
    }

    fn unmap(&mut self, vaddr: u64) -> Result<String, String> {
        let root = self.current_root().ok_or("无当前地址空间")?;
        self.tables
            .unmap(&mut self.frames, root, vaddr)
            .map(|p| format!("unmap v={:#x} -> released p={:#x}", vaddr, p))
            .ok_or("未映射".to_string())
    }

    fn translate(&self, vaddr: u64) -> String {
        let root = match self.current_root() {
            Some(r) => r,
            None => return "无当前地址空间".to_string(),
        };
        match self.tables.translate(root, vaddr) {
            Some((p, f)) => format!("v={:#x} -> p={:#x} flags={:#x}", vaddr, p, f),
            None => format!("v={:#x} -> #PF (page fault, 未映射)", vaddr),
        }
    }

    /// 模拟一次访问：触发缺页处理或 COW 复制。
    fn access(&mut self, vaddr: u64, write: bool) -> String {
        let root = match self.current_root() {
            Some(r) => r,
            None => return "无当前地址空间".to_string(),
        };
        // 先拿到翻译结果（借用立即结束），再对状态做可变操作。
        let hit = self.tables.translate(root, vaddr);
        match hit {
            None => {
                // 真缺页：分配一页并映射；无空闲帧时进入缺页等待（阻塞）。
                self.faults += 1;
                match self.frames.alloc() {
                    Some(paddr) => {
                        self.tables
                            .map(&mut self.frames, root, vaddr, paddr, PTE_RW | PTE_US);
                        self.allocs += 1;
                        format!("缺页处理: 分配 p={:#x} 并映射 v={:#x}", paddr, vaddr)
                    }
                    None => {
                        let owner = self.current.unwrap_or(0);
                        self.fault_waiters.entry(vaddr).or_default().push(owner as u64);
                        format!(
                            "缺页阻塞: v={:#x} 无空闲帧，exec={} 已进入 wait_next(mem/fault/{:#x})",
                            vaddr, owner, vaddr
                        )
                    }
                }
            }
            Some((paddr, flags)) => {
                if flags & PTE_COW != 0 && write {
                    // COW 命中：复制物理页，改写 PTE 为可写私有。
                    self.cow_faults += 1;
                    if let Some(new_p) = self.frames.alloc() {
                        self.phys_copy(new_p, paddr);
                        // 在当前空间把该 vaddr 改成指向新帧的可写 PTE。
                        // 复用 unmap+map 简化：先清再写。
                        self.tables.unmap(&mut self.frames, root, vaddr);
                        self.tables
                            .map(&mut self.frames, root, vaddr, new_p, PTE_RW | PTE_US);
                        format!("COW 复制: v={:#x} 由 p={:#x} 复制到 p={:#x}", vaddr, paddr, new_p)
                    } else {
                        let owner = self.current.unwrap_or(0);
                        self.fault_waiters.entry(vaddr).or_default().push(owner as u64);
                        format!(
                            "COW 阻塞: v={:#x} 无空闲帧，exec={} 已进入 wait_next(mem/fault/{:#x})",
                            vaddr, owner, vaddr
                        )
                    }
                } else {
                    format!(
                        "命中: v={:#x} -> p={:#x}{}",
                        vaddr,
                        paddr,
                        if write { " (写)" } else { " (读)" }
                    )
                }
            }
        }
    }

    fn fork(&mut self, new_id: u32) -> Result<String, String> {
        let root = self.current_root().ok_or("无当前地址空间")?;
        let new_root = {
            let tables = &mut self.tables;
            let frames = &mut self.frames;
            tables.clone_cow(frames, root)
        }
        .ok_or("复制页表失败")?;
        self.spaces.insert(
            new_id,
            AddressSpace {
                id: new_id,
                pml4: new_root,
                regions: self
                    .spaces
                    .get(&self.current.unwrap())
                    .map(|s| s.regions.clone())
                    .unwrap_or_default(),
            },
        );
        if new_id >= self.next_space {
            self.next_space = new_id + 1;
        }
        Ok(format!("fork 新地址空间 #{} (COW 共享物理页)", new_id))
    }

    fn new_space(&mut self) -> String {
        let id = self.next_space;
        self.next_space += 1;
        let root = self.tables.alloc_table(&mut self.frames).expect("PML4");
        self.spaces.insert(
            id,
            AddressSpace {
                id,
                pml4: root,
                regions: Vec::new(),
            },
        );
        format!("新地址空间 #{}", id)
    }

    fn switch(&mut self, id: u32) -> Result<String, String> {
        if self.spaces.contains_key(&id) {
            self.current = Some(id);
            Ok(format!("当前地址空间 -> #{}", id))
        } else {
            Err(format!("地址空间 #{} 不存在", id))
        }
    }

    /// 唤醒等待该 vaddr 缺页的所有执行体：尝试分配一页并映射，成功后清除等待记录。
    /// 模拟页框可用后 `signal` 唤醒 `wait_next(mem/fault/<vaddr>)` 的等待者。
    pub fn wake_fault(&mut self, vaddr: u64) -> Result<String, String> {
        let root = self.current_root().ok_or("无当前地址空间")?;
        let paddr = self.frames.alloc().ok_or("无空闲帧")?;
        self.tables
            .map(&mut self.frames, root, vaddr, paddr, PTE_RW | PTE_US);
        self.allocs += 1;
        self.fault_waiters.remove(&vaddr);
        Ok(format!(
            "缺页唤醒: v={:#x} -> p={:#x} (已映射，等待者可重新访问)",
            vaddr, paddr
        ))
    }

    /// 查询指定 vaddr 是否有缺页等待者（供 `act mem/paging fault_wait`）。
    pub fn fault_waiters_count(&self, vaddr: u64) -> usize {
        self.fault_waiters.get(&vaddr).map(|v| v.len()).unwrap_or(0)
    }

    fn status(&self) -> String {
        let total_bytes = self.frames.total * PAGE_SIZE;
        let used_bytes = self.frames.used * PAGE_SIZE;
        let cur = match self.current {
            Some(c) => c.to_string(),
            None => "无".to_string(),
        };
        format!(
            "paging: 物理内存 {}/{} 字节, 已用帧 {}, 地址空间 {}, 当前 #{}\n  faults={} cow_faults={} allocs={}",
            used_bytes,
            total_bytes,
            self.frames.used,
            self.spaces.len(),
            cur,
            self.faults,
            self.cow_faults,
            self.allocs
        )
    }
}

static PAGING: OnceLock<Mutex<GlobalPaging>> = OnceLock::new();

pub fn core() -> &'static Mutex<GlobalPaging> {
    PAGING.get_or_init(|| Mutex::new(GlobalPaging::new()))
}

/// 把 "rw us nx" 之类解析成属性位。
pub fn parse_flags(s: &str) -> u64 {
    let mut f = 0u64;
    for tok in s.split_whitespace() {
        match tok {
            "rw" => f |= PTE_RW,
            "us" | "user" => f |= PTE_US,
            "nx" => f |= PTE_NX,
            _ => {}
        }
    }
    f
}

/// 解析地址：支持十进制与 `0x` 十六进制前缀。
fn parse_addr(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

pub fn status() -> String {
    core().lock().unwrap().status()
}

/// 页故障精简统计（供 `crate::mm` / `mem/fault` 通道）。
pub fn fault_stats() -> String {
    let g = core().lock().unwrap();
    format!(
        "faults={} cow_faults={} allocs={} spaces={} current={:?}",  // current 以 Option 形式展示
        g.faults, g.cow_faults, g.allocs, g.spaces.len(), g.current
    )
}

pub fn act(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.split_whitespace();
    let sub = it.next().unwrap_or("");
    let c = core();
    let mut g = c.lock().unwrap();
    match sub {
        "space" => {
            let cmd = it.next().unwrap_or("");
            match cmd {
                "new" => Ok(g.new_space()),
                "switch" => {
                    let id: u32 = it.next().ok_or("需要 id")?.parse().map_err(|_| "id 非法")?;
                    g.switch(id)
                }
                _ => Err("space new|switch <id>".to_string()),
            }
        }
        "alloc" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            g.alloc_page(v)
        }
        "map" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            let p: u64 = it.next().and_then(parse_addr).ok_or("需要 paddr")?;
            let flags = parse_flags(it.next().unwrap_or("rw us"));
            g.map(v, p, flags)
        }
        "unmap" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            g.unmap(v)
        }
        "translate" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            Ok(g.translate(v))
        }
        "fault" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            let w = it.next().unwrap_or("r") == "w";
            Ok(g.access(v, w))
        }
        "fault_wait" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            Ok(format!(
                "缺页等待: v={:#x} 等待者数={} (模拟 wait_next(mem/fault/{:#x}))",
                v,
                g.fault_waiters_count(v),
                v
            ))
        }
        "fault_wake" => {
            let v: u64 = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            g.wake_fault(v)
        }
        "fork" => {
            let id: u32 = it.next().ok_or("需要 new_id")?.parse().map_err(|_| "id 非法")?;
            g.fork(id)
        }
        "dump" => {
            let v = it.next().and_then(parse_addr).ok_or("需要 vaddr")?;
            let root = g.current_root().ok_or("无当前地址空间")?;
            match g.tables.translate(root, v) {
                Some((p, f)) => Ok(format!(
                    "v={:#x} -> p={:#x} flags={:#x} cow={}",
                    v,
                    p,
                    f,
                    (f & PTE_COW) != 0
                )),
                None => Ok(format!("v={:#x} -> 未映射", v)),
            }
        }
        "" | "status" => Ok(g.status()),
        _ => Err(
            "space new|switch <id> | alloc <v> | map <v> <p> <flags> | unmap <v> | translate <v> | fault <v> <r|w> | fault_wait <v> | fault_wake <v> | fork <id>"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_and_translate_roundtrip() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        g.map(0x400000, 0x100000, PTE_RW | PTE_US).unwrap();
        let (p, f) = g.tables.translate(g.spaces[&0].pml4, 0x400000).unwrap();
        assert_eq!(p, 0x100000);
        assert!(f & PTE_RW != 0 && f & PTE_US != 0);
        assert_eq!(g.translate(0x400000), "v=0x400000 -> p=0x100000 flags=0x7");
    }

    #[test]
    fn address_spaces_are_isolated() {
        let c = core();
        let mut g = c.lock().unwrap();
        let a = g.next_space;
        g.new_space();
        g.switch(a).unwrap();
        g.map(0x500000, 0x200000, PTE_RW | PTE_US).unwrap();
        let b = g.next_space; // 下一个
        g.new_space();
        g.switch(b).unwrap();
        // 空间 b 不应看到 a 的映射
        assert!(g.tables.translate(g.spaces[&b].pml4, 0x500000).is_none());
        g.switch(a).unwrap();
        assert!(g.tables.translate(g.spaces[&a].pml4, 0x500000).is_some());
    }

    #[test]
    fn unmapped_is_page_fault() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        assert!(g.tables.translate(g.spaces[&0].pml4, 0xdead000).is_none());
        // 模拟访问触发缺页处理
        let msg = g.access(0xdead000, false);
        assert!(msg.contains("缺页处理"), "{}", msg);
        assert!(g.tables.translate(g.spaces[&0].pml4, 0xdead000).is_some());
    }

    #[test]
    fn cow_splits_on_write() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        // 父映射一页并写入数据
        g.alloc_page(0x600000).unwrap();
        let paddr_parent = pte_addr(
            g.tables
                .translate(g.spaces[&0].pml4, 0x600000)
                .unwrap()
                .0,
        );
        g.phys_write(paddr_parent, &[0x11u8; PAGE_SIZE as usize]);
        // fork 出子空间（COW）
        let child = 99;
        g.fork(child).unwrap();
        // 子读命中（共享）
        g.switch(child).unwrap();
        assert!(g.access(0x600000, false).contains("命中"));
        let paddr_child_shared = pte_addr(
            g.tables
                .translate(g.spaces[&child].pml4, 0x600000)
                .unwrap()
                .0,
        );
        assert_eq!(paddr_child_shared, paddr_parent, "fork 后应共享物理页");
        // 子写触发 COW 复制
        let msg = g.access(0x600000, true);
        assert!(msg.contains("COW 复制"), "{}", msg);
        let paddr_child_new = pte_addr(
            g.tables
                .translate(g.spaces[&child].pml4, 0x600000)
                .unwrap()
                .0,
        );
        assert_ne!(paddr_child_new, paddr_parent, "COW 后应拥有独立物理页");
        // 父数据不受影响
        g.switch(0).unwrap();
        assert_eq!(
            g.phys_read(paddr_parent, 1)[0],
            0x11,
            "父页数据在 COW 后保持不变"
        );
    }

    #[test]
    fn child_space_gets_cow_flag() {
        let c = core();
        let mut g = c.lock().unwrap();
        let id = g.next_space;
        g.new_space();
        g.switch(id).unwrap();
        g.alloc_page(0x800000).unwrap();
        let ppar = pte_addr(
            g.tables
                .translate(g.spaces[&id].pml4, 0x800000)
                .unwrap()
                .0,
        );
        g.fork(777).unwrap();
        g.switch(777).unwrap();
        let (p, f) = g.tables.translate(g.spaces[&777].pml4, 0x800000).unwrap();
        assert_eq!(p, ppar, "fork 应共享父物理页");
        assert!(
            f & PTE_COW != 0,
            "子空间应带 COW 标记 flags={:#x}",
            f
        );
    }

    #[test]
    fn flag_parse_and_nx() {
        assert_eq!(parse_flags("rw us"), PTE_RW | PTE_US);
        assert_eq!(parse_flags("rw nx"), PTE_RW | PTE_NX);
        assert_eq!(parse_flags(""), 0);
    }

    #[test]
    fn fault_allocates_and_maps() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        // 未映射地址触发缺页，应分配并映射
        let msg = g.access(0x900000, false);
        assert!(msg.contains("缺页处理"), "{}", msg);
        // 之后 translate 应命中
        let out = g.translate(0x900000);
        assert!(out.contains("->"), "{}", out);
        assert!(!out.contains("#PF"), "{}", out);
        // 等待者应为 0（已直接分配，未阻塞）
        assert_eq!(g.fault_waiters_count(0x900000), 0);
    }

    #[test]
    fn fault_wait_and_wake() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        let v = 0xA00000;
        // 先直接映射一页（模拟页框可用后的唤醒映射）
        g.wake_fault(v).unwrap();
        let out = g.translate(v);
        assert!(out.contains("->"), "{}", out);
        // 再次 wake 同一地址会重新分配另一帧（简化语义），不应出错
        g.wake_fault(v).unwrap();
    }

    #[test]
    fn fault_waiters_zero_when_not_blocked() {
        let c = core();
        let mut g = c.lock().unwrap();
        g.switch(0).unwrap();
        assert_eq!(g.fault_waiters_count(0xB00000), 0);
    }
}
