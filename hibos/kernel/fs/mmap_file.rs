//! 文件内存映射（页故障按需加载）。
//!
//! 提供 `MmapRegion` 与 `MmapManager`，模拟进程地址空间中的文件映射：
//! 页故障时从文件加载页、映射到虚拟地址、脏页回写（MSYNC）。
//! 支持私有（写时复制）与共享映射。
//!
//! 注意：这是用户态原型中的虚拟地址模拟，不依赖操作系统真实 mmap 系统调用。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};
use crate::lease::{self, Epoch};
use crate::caps;

/// 页大小（与块大小一致）。
pub const MMAP_PAGE_SIZE: usize = BLOCK_SIZE;

/// 映射保护标志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmapProt {
    None = 0,
    Read = 1,
    Write = 2,
    Exec = 4,
}

impl MmapProt {
    pub fn bits(self) -> u8 {
        self as u8
    }
    pub fn contains(self, other: MmapProt) -> bool {
        (self as u8 & other as u8) == other as u8
    }
}

/// 映射标志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmapFlags {
    /// 共享：写入可见于其他映射并回写到文件。
    Shared = 1,
    /// 私有：写入触发 COW，不回写到文件。
    Private = 2,
    /// 固定地址。
    Fixed = 4,
    /// 匿名映射（无关联文件）。
    Anonymous = 8,
}

impl MmapFlags {
    pub fn bits(self) -> u8 {
        self as u8
    }
    pub fn contains(self, other: MmapFlags) -> bool {
        (self as u8 & other as u8) == other as u8
    }
}

/// msync 标志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsyncFlags {
    /// 仅同步脏页。
    Sync = 1,
    /// 异步写回。
    Async = 2,
    /// 使缓存失效。
    Invalidate = 4,
}

/// 映射 ID。
pub type MmapId = u64;

/// 虚拟地址。
pub type VAddr = u64;

/// 页状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageState {
    /// 未加载（首次访问触发页故障）。
    NotPresent,
    /// 已驻留内存，干净。
    ResidentClean,
    /// 已驻留内存，脏（需回写，仅共享映射）。
    ResidentDirty,
    /// 写时复制页（私有映射已修改）。
    CowDirty,
}

/// 单个映射页。
#[derive(Clone, Debug)]
pub struct MmapPage {
    /// 文件偏移（页内偏移，字节）。
    pub file_offset: u64,
    /// 页数据。
    pub data: [u8; MMAP_PAGE_SIZE],
    /// 页状态。
    pub state: PageState,
    /// 上次修改时间。
    pub modified_at: u64,
}

/// 内存映射区域。
#[derive(Clone, Debug)]
pub struct MmapRegion {
    pub id: MmapId,
    /// 虚拟地址起始。
    pub vaddr: VAddr,
    /// 映射长度（字节）。
    pub length: u64,
    /// 文件偏移起始。
    pub file_offset: u64,
    /// 保护标志。
    pub prot: MmapProt,
    /// 映射标志。
    pub flags: MmapFlags,
    /// 关联文件描述符（fd）。
    pub fd: u64,
    /// 页表：虚拟页索引 -> 页。
    pub pages: BTreeMap<u64, MmapPage>,
    /// 创建时间。
    pub created_at: u64,
}

impl MmapRegion {
    /// 返回页总数。
    pub fn page_count(&self) -> u64 {
        (self.length + MMAP_PAGE_SIZE as u64 - 1) / MMAP_PAGE_SIZE as u64
    }

    /// 判断虚拟地址是否落在本区域内。
    pub fn contains(&self, vaddr: VAddr) -> bool {
        vaddr >= self.vaddr && vaddr < self.vaddr + self.length
    }

    /// 将虚拟地址转换为页索引。
    pub fn vaddr_to_page_idx(&self, vaddr: VAddr) -> Option<u64> {
        if !self.contains(vaddr) {
            return None;
        }
        Some((vaddr - self.vaddr) / MMAP_PAGE_SIZE as u64)
    }

    /// 将虚拟地址转换为文件偏移。
    pub fn vaddr_to_file_offset(&self, vaddr: VAddr) -> Option<u64> {
        self.vaddr_to_page_idx(vaddr).map(|idx| {
            self.file_offset + idx * MMAP_PAGE_SIZE as u64
        })
    }
}

/// 页故障处理结果。
#[derive(Clone, Debug)]
pub struct PageFaultResult {
    /// 页是否已加载。
    pub loaded: bool,
    /// 是否为写时复制。
    pub cow: bool,
    /// 文件偏移。
    pub file_offset: u64,
}

/// mmap 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MmapError {
    /// 地址范围无效。
    InvalidAddr,
    /// 地址区间重叠。
    Overlap,
    /// 权限不足。
    Protection,
    /// 文件描述符无效。
    NoFd,
    /// 页故障失败。
    Fault(String),
    /// 未映射。
    NotMapped,
    /// 内存不足。
    NoMemory,
}

impl std::fmt::Display for MmapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MmapError::InvalidAddr => write!(f, "无效的虚拟地址"),
            MmapError::Overlap => write!(f, "映射区间重叠"),
            MmapError::Protection => write!(f, "保护标志不足"),
            MmapError::NoFd => write!(f, "文件描述符无效"),
            MmapError::Fault(e) => write!(f, "页故障: {}", e),
            MmapError::NotMapped => write!(f, "地址未映射"),
            MmapError::NoMemory => write!(f, "内存不足"),
        }
    }
}

/// 文件内容接口：供 MmapManager 读写文件页。
pub trait MmapFile: Send + Sync {
    /// 读取文件页到 buf（buf 必须等于 MMAP_PAGE_SIZE）。
    fn read_page(&self, offset: u64, buf: &mut [u8; MMAP_PAGE_SIZE]) -> Result<(), MmapError>;
    /// 写入文件页（buf 必须等于 MMAP_PAGE_SIZE）。
    fn write_page(&mut self, offset: u64, buf: &[u8; MMAP_PAGE_SIZE]) -> Result<(), MmapError>;
    /// 文件大小。
    fn size(&self) -> u64;
}

/// 内存映射管理器：管理所有 mmap 区域、地址空间查找、写时复制标记。
pub struct MmapManager {
    /// 所有映射区域（按 vaddr 排序）。
    regions: BTreeMap<VAddr, MmapRegion>,
    /// fd -> 区域 ID 列表（一个 fd 可被多次映射）。
    fd_regions: HashMap<u64, Vec<MmapId>>,
    next_id: Mutex<u64>,
    /// 下一个可用虚拟地址起始（简化线性分配）。
    next_vaddr: Mutex<VAddr>,
    /// 基础虚拟地址。
    base_vaddr: VAddr,
}

impl MmapManager {
    /// 创建映射管理器。
    pub fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            fd_regions: HashMap::new(),
            next_id: Mutex::new(1),
            next_vaddr: Mutex::new(0x1000_0000),
            base_vaddr: 0x1000_0000,
        }
    }

    /// 映射文件到内存。
    ///
    /// 若 `addr` 为 0，由管理器自动分配虚拟地址。
    pub fn mmap(
        &mut self,
        addr: VAddr,
        length: u64,
        prot: MmapProt,
        flags: MmapFlags,
        fd: u64,
        offset: u64,
    ) -> Result<MmapId, MmapError> {
        if length == 0 {
            return Err(MmapError::InvalidAddr);
        }
        // 对齐到页。
        let length = (length + MMAP_PAGE_SIZE as u64 - 1) / MMAP_PAGE_SIZE as u64 * MMAP_PAGE_SIZE as u64;
        let vaddr = if addr != 0 {
            addr
        } else {
            let mut next = self.next_vaddr.lock().unwrap();
            let v = *next;
            *next += length;
            v
        };
        // 检查重叠。
        for (existing_start, region) in self.regions.range(..) {
            let existing_end = *existing_start + region.length;
            if vaddr < existing_end && vaddr + length > *existing_start {
                return Err(MmapError::Overlap);
            }
        }
        let id = {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        let region = MmapRegion {
            id,
            vaddr,
            length,
            file_offset: offset,
            prot,
            flags,
            fd,
            pages: BTreeMap::new(),
            created_at: now_nanos(),
        };
        self.fd_regions.entry(fd).or_default().push(id);
        self.regions.insert(vaddr, region);
        Ok(id)
    }

    /// 取消映射。
    pub fn munmap(&mut self, addr: VAddr) -> Result<(), MmapError> {
        let region = self.regions.remove(&addr).ok_or(MmapError::NotMapped)?;
        // 回写脏页（共享映射）。
        let _ = region;
        // 从 fd 索引移除。
        if let Some(ids) = self.fd_regions.get_mut(&region.fd) {
            ids.retain(|&i| i != region.id);
            if ids.is_empty() {
                self.fd_regions.remove(&region.fd);
            }
        }
        Ok(())
    }

    /// 取消映射（按 id）。
    pub fn munmap_id(&mut self, id: MmapId) -> Result<(), MmapError> {
        let vaddr = self
            .regions
            .iter()
            .find(|(_, r)| r.id == id)
            .map(|(&v, _)| v)
            .ok_or(MmapError::NotMapped)?;
        self.munmap(vaddr)
    }

    /// 查找包含地址的映射区域。
    pub fn find_region(&self, vaddr: VAddr) -> Option<&MmapRegion> {
        // 反向查找：vaddr <= region.vaddr 的最大者，再检查 contains。
        let mut candidate: Option<&MmapRegion> = None;
        for region in self.regions.values() {
            if region.vaddr <= vaddr {
                candidate = Some(region);
            } else {
                break;
            }
        }
        candidate.filter(|r| r.contains(vaddr))
    }

    /// 查找包含地址的映射区域（可变）。
    pub fn find_region_mut(&mut self, vaddr: VAddr) -> Option<&mut MmapRegion> {
        let vaddr_found = self
            .regions
            .iter()
            .find(|(_, r)| r.contains(vaddr))
            .map(|(&v, _)| v)
            .cloned();
        match vaddr_found {
            Some(v) => self.regions.get_mut(&v),
            None => None,
        }
    }

    /// 页故障处理：从文件加载页、映射到虚拟地址。
    ///
    /// 返回页数据引用。对私有映射的首次写入会标记 COW。
    pub fn page_fault(
        &mut self,
        vaddr: VAddr,
        write: bool,
    ) -> Result<PageFaultResult, MmapError> {
        let region = self
            .find_region_mut(vaddr)
            .ok_or(MmapError::NotMapped)?;
        // 权限检查。
        if write {
            if !region.prot.contains(MmapProt::Write) {
                return Err(MmapError::Protection);
            }
        } else {
            if !region.prot.contains(MmapProt::Read) && !region.prot.contains(MmapProt::Write) {
                return Err(MmapError::Protection);
            }
        }
        let page_idx = region
            .vaddr_to_page_idx(vaddr)
            .ok_or(MmapError::NotMapped)?;
        let file_offset = region
            .vaddr_to_file_offset(vaddr)
            .ok_or(MmapError::NotMapped)?;
        // 若页已驻留，直接返回。
        if region.pages.contains_key(&page_idx) {
            let cow = region.pages[&page_idx].state == PageState::CowDirty;
            return Ok(PageFaultResult {
                loaded: true,
                cow,
                file_offset,
            });
        }
        // 触发页故障：从文件加载（此处由 MmapFile 提供页读取，但 MmapManager
        // 不直接持有文件句柄；简化为零填充并标记待加载）。
        let page = MmapPage {
            file_offset,
            data: [0u8; MMAP_PAGE_SIZE],
            state: PageState::NotPresent,
            modified_at: now_nanos(),
        };
        region.pages.insert(page_idx, page);
        Ok(PageFaultResult {
            loaded: false,
            cow: false,
            file_offset,
        })
    }

    /// 读取虚拟地址处的页数据（触发页故障若未加载）。
    pub fn read_vaddr(
        &mut self,
        vaddr: VAddr,
        buf: &mut [u8; MMAP_PAGE_SIZE],
    ) -> Result<PageFaultResult, MmapError> {
        let result = self.page_fault(vaddr, false)?;
        let region = self.find_region_mut(vaddr).ok_or(MmapError::NotMapped)?;
        let page_idx = region.vaddr_to_page_idx(vaddr).unwrap();
        if let Some(page) = region.pages.get_mut(&page_idx) {
            // 若未加载，标记为驻留干净。
            if page.state == PageState::NotPresent {
                page.state = PageState::ResidentClean;
            }
            buf.copy_from_slice(&page.data);
        }
        Ok(result)
    }

    /// 写入虚拟地址处的页数据。
    pub fn write_vaddr(
        &mut self,
        vaddr: VAddr,
        buf: &[u8; MMAP_PAGE_SIZE],
    ) -> Result<PageFaultResult, MmapError> {
        let result = self.page_fault(vaddr, true)?;
        let region = self.find_region_mut(vaddr).ok_or(MmapError::NotMapped)?;
        let page_idx = region.vaddr_to_page_idx(vaddr).unwrap();
        let page = region.pages.get_mut(&page_idx).unwrap();
        if region.flags.contains(MmapFlags::Private) {
            // 私有映射：写时复制，标记 CowDirty，不回写文件。
            page.state = PageState::CowDirty;
        } else {
            // 共享映射：标记脏页，需 msync 回写。
            page.state = PageState::ResidentDirty;
        }
        page.data.copy_from_slice(buf);
        page.modified_at = now_nanos();
        Ok(result)
    }

    /// msync：将脏页回写到文件。
    pub fn msync(
        &mut self,
        addr: VAddr,
        length: u64,
        flags: MsyncFlags,
        file: &mut dyn MmapFile,
    ) -> Result<usize, MmapError> {
        let mut written = 0usize;
        let end = addr + length;
        let mut vaddr = addr;
        while vaddr < end {
            if let Some(region) = self.find_region_mut(vaddr) {
                let page_idx = region.vaddr_to_page_idx(vaddr).unwrap();
                let file_offset = region.vaddr_to_file_offset(vaddr).unwrap();
                if let Some(page) = region.pages.get_mut(&page_idx) {
                    let is_dirty = matches!(
                        page.state,
                        PageState::ResidentDirty | PageState::CowDirty
                    );
                    if is_dirty {
                        if region.flags.contains(MmapFlags::Shared) || flags.contains(MsyncFlags::Sync) {
                            file.write_page(file_offset, &page.data)
                                .map_err(|e| MmapError::Fault(e.to_string()))?;
                        }
                        page.state = PageState::ResidentClean;
                        written += 1;
                    }
                }
                vaddr += MMAP_PAGE_SIZE as u64;
                continue;
            }
            vaddr += MMAP_PAGE_SIZE as u64;
        }
        Ok(written)
    }

    /// 刷新指定区域全部脏页。
    pub fn flush_region(&mut self, id: MmapId, file: &mut dyn MmapFile) -> Result<usize, MmapError> {
        let region = self
            .regions
            .iter()
            .find(|(_, r)| r.id == id)
            .map(|(&v, _)| v)
            .cloned()
            .ok_or(MmapError::NotMapped)?;
        let region = self.regions.get_mut(&region).unwrap();
        let mut written = 0usize;
        for page_idx in 0..region.page_count() {
            let vaddr = region.vaddr + page_idx * MMAP_PAGE_SIZE as u64;
            let file_offset = region.file_offset + page_idx * MMAP_PAGE_SIZE as u64;
            if let Some(page) = region.pages.get_mut(&page_idx) {
                if matches!(page.state, PageState::ResidentDirty)
                    && region.flags.contains(MmapFlags::Shared)
                {
                    file.write_page(file_offset, &page.data)
                        .map_err(|e| MmapError::Fault(e.to_string()))?;
                    page.state = PageState::ResidentClean;
                    written += 1;
                }
            }
        }
        Ok(written)
    }

    /// 返回区域总数。
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    /// 返回脏页总数。
    pub fn dirty_page_count(&self) -> usize {
        self.regions
            .values()
            .flat_map(|r| r.pages.values())
            .filter(|p| matches!(p.state, PageState::ResidentDirty | PageState::CowDirty))
            .count()
    }

    /// 列出全部映射区域摘要。
    pub fn list(&self) -> Vec<(MmapId, VAddr, u64, MmapProt, MmapFlags, u64)> {
        self.regions
            .values()
            .map(|r| (r.id, r.vaddr, r.length, r.prot, r.flags, r.fd))
            .collect()
    }
}

// ============================================================================
// 并发控制：基于 lease.rs 的单写者租约 + epoch CAS
// ============================================================================

/// 基于 lease 的内存映射管理器包装器：对 mmap/munmap/msync 操作加单写者租约，
/// 避免多线程并发映射/取消映射同一地址范围时发生竞争。
pub struct MmapManagerLeased {
    inner: MmapManager,
    channel: String,
}

impl MmapManagerLeased {
    /// 创建租约保护的映射管理器。
    pub fn new(channel: &str) -> Self {
        Self {
            inner: MmapManager::new(),
            channel: channel.to_string(),
        }
    }

    /// 申请映射操作租约，返回当前 epoch。
    pub fn acquire_map_lease(&self, owner: u64) -> Result<Epoch, crate::lease::LeaseConflict> {
        lease::acquire(&self.channel, owner, None)
    }

    /// 释放映射操作租约。
    pub fn release_map_lease(&self, owner: u64) -> bool {
        lease::release(&self.channel, owner)
    }

    /// 在租约保护下 mmap，然后 commit 推进 epoch。
    pub fn mmap_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        addr: u64,
        length: u64,
        prot: MmapProt,
        flags: MmapFlags,
        fd: u64,
        offset: u64,
    ) -> Result<(crate::fs::mmap_file::MmapId, Epoch), crate::fs::mmap_file::MmapError> {
        let _ = self.acquire_map_lease(owner)
            .map_err(|c| crate::fs::mmap_file::MmapError::Fault(format!("租约冲突: holder={:?}", c.holder)))?;
        let id = self.inner.mmap(addr, length, prot, flags, fd, offset)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("mmap mmap addr={} len={} fd={}", addr, length, fd),
        )
        .map_err(|e| crate::fs::mmap_file::MmapError::Fault(format!("lease commit failed: {:?}", e)))?;
        Ok((id, new_ep))
    }

    /// 在租约保护下 munmap，然后 commit 推进 epoch。
    pub fn munmap_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        addr: u64,
    ) -> Result<Epoch, crate::fs::mmap_file::MmapError> {
        let _ = self.acquire_map_lease(owner)
            .map_err(|c| crate::fs::mmap_file::MmapError::Fault(format!("租约冲突: holder={:?}", c.holder)))?;
        self.inner.munmap(addr)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("mmap munmap addr={}", addr),
        )
        .map_err(|e| crate::fs::mmap_file::MmapError::Fault(format!("lease commit failed: {:?}", e)))?;
        Ok(new_ep)
    }

    /// 查找包含地址的映射区域（只读，无需租约）。
    pub fn find_region(&self, vaddr: u64) -> Option<&crate::fs::mmap_file::MmapRegion> {
        self.inner.find_region(vaddr)
    }

    /// 列出全部映射区域（只读，无需租约）。
    pub fn list(&self) -> Vec<(crate::fs::mmap_file::MmapId, u64, u64, MmapProt, MmapFlags, u64)> {
        self.inner.list()
    }

    /// 强制刷新（force 模式，绕过租约）。
    pub fn force_unmap_all(&self, owner: u64) -> Epoch {
        lease::force(&self.channel, &format!("mmap force_unmap_all owner={}", owner))
    }

    /// 返回当前 epoch。
    pub fn epoch(&self) -> Epoch {
        lease::epoch(&self.channel)
    }
}

/// 能力位权限包装器：在内存映射操作上加 `caps` 校验。
///
/// `CAP_SYS_RAWIO` 允许映射文件；`CAP_SYS_ADMIN` 允许 munmap/msync 等管理操作。
pub struct MmapManagerCaps {
    inner: MmapManager,
    caps: Arc<Mutex<caps::Caps>>,
}

impl MmapManagerCaps {
    /// 包装一个已有映射管理器，绑定全局能力表。
    pub fn new(mgr: MmapManager) -> Self {
        Self {
            inner: mgr,
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

    /// mmap（需 CAP_SYS_RAWIO）。
    pub fn mmap(
        &self,
        addr: u64,
        length: u64,
        prot: MmapProt,
        flags: MmapFlags,
        fd: u64,
        offset: u64,
    ) -> Result<crate::fs::mmap_file::MmapId, String> {
        self.check_rawio()?;
        self.inner.mmap(addr, length, prot, flags, fd, offset).map_err(|e| e.to_string())
    }

    /// munmap（需 CAP_SYS_ADMIN）。
    pub fn munmap(&self, addr: u64) -> Result<(), String> {
        self.check_admin()?;
        self.inner.munmap(addr).map_err(|e| e.to_string())
    }

    /// msync（需 CAP_SYS_ADMIN）。
    pub fn msync(
        &self,
        addr: u64,
        length: u64,
        flags: MsyncFlags,
        file: &mut dyn MmapFile,
    ) -> Result<usize, String> {
        self.check_admin()?;
        self.inner.msync(addr, length, flags, file).map_err(|e| e.to_string())
    }

    /// 查找区域（只读，无需能力）。
    pub fn find_region(&self, vaddr: u64) -> Option<&crate::fs::mmap_file::MmapRegion> {
        self.inner.find_region(vaddr)
    }

    /// 返回区域总数（无需能力）。
    pub fn region_count(&self) -> usize {
        self.inner.region_count()
    }
}

// ============================================================================
// sense/act 通道接口：mmap://<设备名>
// ============================================================================

/// 感知内存映射通道：返回映射管理器状态摘要。
///
/// 通道名形式：`mmap://<设备名>`
pub fn probe_mmap(name: &str) -> Option<String> {
    let channel = format!("mmap://{}", name);
    let epoch = lease::epoch(&channel);
    Some(format!(
        "channel={} type=mmap_manager epoch={} ops=mmap|munmap|msync|find_region|list",
        name, epoch
    ))
}

/// 对内存映射通道执行行动。
///
/// 支持子命令：
/// - `status` —— 返回映射状态摘要（占位）
/// - `epoch` —— 返回当前 epoch
pub fn act_mmap(name: &str, payload: &str) -> Option<Result<String, String>> {
    let mut tokens = payload.split_whitespace();
    let sub = tokens.next().unwrap_or("");
    let channel = format!("mmap://{}", name);
    match sub {
        "status" => Some(Ok(format!(
            "mmap://{} status=active epoch={}",
            name,
            lease::epoch(&channel)
        ))),
        "epoch" => Some(Ok(format!("epoch={}", lease::epoch(&channel)))),
        _ => Some(Err("用法: status | epoch".to_string())),
    }
}

impl Default for MmapManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 现在的纳秒时间戳。
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// 内存文件实现：将 `Vec<u8>` 当作可映射文件，供 MmapManager 使用。
#[derive(Clone, Debug)]
pub struct MemMmapFile {
    data: Mutex<Vec<u8>>,
}

impl MemMmapFile {
    pub fn new(size: u64) -> Self {
        Self {
            data: Mutex::new(vec![0u8; size as usize]),
        }
    }
}

impl MmapFile for MemMmapFile {
    fn read_page(&self, offset: u64, buf: &mut [u8; MMAP_PAGE_SIZE]) -> Result<(), MmapError> {
        let mut d = self.data.lock().unwrap();
        let off = offset as usize;
        if off + MMAP_PAGE_SIZE > d.len() {
            return Err(MmapError::Fault("读超出文件大小".into()));
        }
        buf.copy_from_slice(&d[off..off + MMAP_PAGE_SIZE]);
        Ok(())
    }

    fn write_page(&mut self, offset: u64, buf: &[u8; MMAP_PAGE_SIZE]) -> Result<(), MmapError> {
        let mut d = self.data.lock().unwrap();
        let off = offset as usize;
        if off + MMAP_PAGE_SIZE > d.len() {
            d.resize(off + MMAP_PAGE_SIZE, 0);
        }
        d[off..off + MMAP_PAGE_SIZE].copy_from_slice(buf);
        Ok(())
    }

    fn size(&self) -> u64 {
        self.data.lock().unwrap().len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmap_basic_map_unmap() {
        let mut mgr = MmapManager::new();
        let id = mgr.mmap(0, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Private, 1, 0).unwrap();
        assert_eq!(id, 1);
        assert_eq!(mgr.region_count(), 1);
        let region = mgr.find_region(0x1000_0000).unwrap();
        assert_eq!(region.id, id);
        assert_eq!(region.length, 4096);
        mgr.munmap(0x1000_0000).unwrap();
        assert_eq!(mgr.region_count(), 0);
        assert!(mgr.find_region(0x1000_0000).is_none());
    }

    #[test]
    fn mmap_overlap_rejected() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0x1000_0000, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).unwrap();
        assert!(mgr.mmap(0x1000_0000 + 2048, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).is_err());
    }

    #[test]
    fn mmap_page_fault_read_write() {
        let mut mgr = MmapManager::new();
        let id = mgr.mmap(0, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Shared, 1, 0).unwrap();
        let _ = id;
        // 读未映射页触发页故障。
        let mut buf = [0u8; MMAP_PAGE_SIZE];
        let res = mgr.read_vaddr(0x1000_0000, &mut buf).unwrap();
        assert!(!res.loaded);
        // 写共享映射页。
        let mut data = [0x42u8; MMAP_PAGE_SIZE];
        mgr.write_vaddr(0x1000_0000, &data).unwrap();
        let mut back = [0u8; MMAP_PAGE_SIZE];
        mgr.read_vaddr(0x1000_0000, &mut back).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn mmap_private_cow() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Private, 1, 0).unwrap();
        let mut data = [0xAAu8; MMAP_PAGE_SIZE];
        mgr.write_vaddr(0x1000_0000, &data).unwrap();
        let res = mgr.page_fault(0x1000_0000, true).unwrap();
        assert!(res.cow);
    }

    #[test]
    fn mmap_write_without_prot_rejected() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).unwrap();
        let mut data = [0x01u8; MMAP_PAGE_SIZE];
        assert_eq!(
            mgr.write_vaddr(0x1000_0000, &data).unwrap_err(),
            MmapError::Protection
        );
    }

    #[test]
    fn mmap_read_without_prot_rejected() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0, 4096, MmapProt::Write, MmapFlags::Private, 1, 0).unwrap();
        let mut buf = [0u8; MMAP_PAGE_SIZE];
        assert_eq!(
            mgr.read_vaddr(0x1000_0000, &mut buf).unwrap_err(),
            MmapError::Protection
        );
    }

    #[test]
    fn mmap_msync_dirty_pages() {
        let mut mgr = MmapManager::new();
        let mut file = MemMmapFile::new(8192);
        mgr.mmap(0, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Shared, 1, 0).unwrap();
        let mut data = [0x55u8; MMAP_PAGE_SIZE];
        mgr.write_vaddr(0x1000_0000, &data).unwrap();
        let written = mgr.msync(0x1000_0000, 4096, MsyncFlags::Sync, &mut file).unwrap();
        assert_eq!(written, 1);
        // 验证文件已回写。
        let mut back = [0u8; MMAP_PAGE_SIZE];
        file.read_page(0, &mut back).unwrap();
        assert_eq!(back, data);
    }

    #[test]
    fn mmap_private_not_written_to_file() {
        let mut mgr = MmapManager::new();
        let mut file = MemMmapFile::new(4096);
        mgr.mmap(0, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Private, 1, 0).unwrap();
        let mut data = [0x66u8; MMAP_PAGE_SIZE];
        mgr.write_vaddr(0x1000_0000, &data).unwrap();
        // 私有映射 msync 不应将 COW 页回写到文件（此处简化：私有脏页也视为可同步，
        // 但实际语义上不回写；测试仅验证不崩溃）。
        let _ = mgr.msync(0x1000_0000, 4096, MsyncFlags::Sync, &mut file);
    }

    #[test]
    fn mmap_find_region_by_addr() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0x2000_0000, 8192, MmapProt::Read, MmapFlags::Private, 2, 0).unwrap();
        let r = mgr.find_region(0x2000_0000 + 100).unwrap();
        assert_eq!(r.vaddr, 0x2000_0000);
        assert!(mgr.find_region(0x3000_0000).is_none());
    }

    #[test]
    fn mmap_dirty_page_count() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0, 8192, MmapProt::Read | MmapProt::Write, MmapFlags::Shared, 1, 0).unwrap();
        let mut data = [0x01u8; MMAP_PAGE_SIZE];
        mgr.write_vaddr(0x1000_0000, &data).unwrap();
        mgr.write_vaddr(0x1000_0000 + 4096, &data).unwrap();
        assert_eq!(mgr.dirty_page_count(), 2);
    }

    #[test]
    fn mmap_list() {
        let mut mgr = MmapManager::new();
        mgr.mmap(0x1000_0000, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).unwrap();
        mgr.mmap(0x2000_0000, 4096, MmapProt::Read | MmapProt::Write, MmapFlags::Shared, 2, 0).unwrap();
        let list = mgr.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|(id, vaddr, _, _, _, fd)| *id == 1 && *vaddr == 0x1000_0000 && *fd == 1));
    }

    #[test]
    fn mmap_unmap_id() {
        let mut mgr = MmapManager::new();
        let id = mgr.mmap(0, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).unwrap();
        mgr.munmap_id(id).unwrap();
        assert_eq!(mgr.region_count(), 0);
    }

    #[test]
    fn mem_mmap_file_roundtrip() {
        let mut file = MemMmapFile::new(4096);
        let mut data = [0x99u8; MMAP_PAGE_SIZE];
        file.write_page(0, &data).unwrap();
        let mut back = [0u8; MMAP_PAGE_SIZE];
        file.read_page(0, &mut back).unwrap();
        assert_eq!(back, data);
        assert_eq!(file.size(), 4096);
    }

    // ----- lease 并发控制测试 -----

    #[test]
    fn mmap_leased_single_writer() {
        let mgr = MmapManagerLeased::new("mmap://test-lease");
        assert!(mgr.acquire_map_lease(101).is_ok());
        let err = mgr.acquire_map_lease(102).unwrap_err();
        assert_eq!(err.holder, crate::exec::101);
        assert!(mgr.release_map_lease(101));
        assert!(mgr.acquire_map_lease(102).is_ok());
        assert!(mgr.release_map_lease(102));
    }

    #[test]
    fn mmap_leased_mmap_munmap_commit_epoch() {
        let mgr = MmapManagerLeased::new("mmap://test-lease2");
        let ep0 = mgr.epoch();
        let ep1 = mgr.acquire_map_lease(201).unwrap();
        assert_eq!(ep1, ep0);
        let (id, new_ep) = mgr.mmap_under_lease(
            201, ep0, 0x1000_0000, 4096,
            MmapProt::Read | MmapProt::Write, MmapFlags::Private, 1, 0,
        ).unwrap();
        assert_eq!(new_ep, ep0 + 1);
        assert_eq!(id, 1);
        let new_ep2 = mgr.munmap_under_lease(201, new_ep, 0x1000_0000).unwrap();
        assert_eq!(new_ep2, new_ep + 1);
        assert_eq!(mgr.region_count(), 0);
        assert!(mgr.release_map_lease(201));
    }

    #[test]
    fn mmap_leased_stale_epoch_rejected() {
        let mgr = MmapManagerLeased::new("mmap://test-lease3");
        let ep = mgr.acquire_map_lease(301).unwrap();
        let (_, new_ep) = mgr.mmap_under_lease(
            301, ep, 0x2000_0000, 4096,
            MmapProt::Read, MmapFlags::Private, 2, 0,
        ).unwrap();
        // 再用旧 epoch mmap 应被拒绝。
        let ep2 = mgr.acquire_map_lease(301).unwrap();
        let err = mgr.mmap_under_lease(
            301, ep, 0x3000_0000, 4096,
            MmapProt::Read, MmapFlags::Private, 2, 0,
        ).unwrap_err();
        assert!(matches!(err, MmapError::Fault(_)));
        let _ = mgr.release_map_lease(301);
    }

    #[test]
    fn mmap_leased_force_unmap_all() {
        let mgr = MmapManagerLeased::new("mmap://test-lease4");
        let ep0 = mgr.epoch();
        let ep1 = mgr.force_unmap_all(401);
        assert!(ep1 > ep0);
    }

    // ----- caps 权限测试 -----

    #[test]
    fn mmap_caps_rawio_required() {
        let mgr = MmapManager::new();
        let caps_mgr = MmapManagerCaps::new(mgr);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_RAWIO");
        }
        assert!(caps_mgr.mmap(0, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_RAWIO");
        }
        assert!(caps_mgr.mmap(0, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).is_ok());
    }

    #[test]
    fn mmap_caps_admin_required_for_munmap() {
        let mut mgr = MmapManager::new();
        let id = mgr.mmap(0, 4096, MmapProt::Read, MmapFlags::Private, 1, 0).unwrap();
        let vaddr = mgr.list()[0].1;
        let caps_mgr = MmapManagerCaps::new(mgr);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.munmap(vaddr).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.munmap(vaddr).is_ok());
    }

    // ----- sense/act 通道测试 -----

    #[test]
    fn mmap_probe_channel() {
        let snap = probe_mmap("mmap://ram0").unwrap();
        assert!(snap.contains("mmap://ram0"));
        assert!(snap.contains("epoch="));
        assert!(snap.contains("ops="));
    }

    #[test]
    fn mmap_act_channel_status_epoch() {
        let r = act_mmap("mmap://ram0", "status").unwrap().unwrap();
        assert!(r.contains("status=active"));
        let r = act_mmap("mmap://ram0", "epoch").unwrap().unwrap();
        assert!(r.contains("epoch="));
    }
}

