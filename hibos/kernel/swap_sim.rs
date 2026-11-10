//! swap 区模拟（内存文件作为后端）。
//!
//! 以宿主文件系统文件作为 swap 后端，模拟页交换进出：
//!
//! * `SwapArea::new(path, max_pages)` —— 创建或打开 swap 文件；
//! * `swap_out(page_addr, data) -> SwapSlot` —— 将一页数据换出到 swap；
//! * `swap_in(slot) -> Vec<u8>` —— 从 swap 换入一页数据；
//! * `free_slot(slot)` —— 释放 swap 插槽；
//! * `usage()` —— 返回已用插槽数与总插槽数；
//! * `page_fault_handler(addr)` —— 页故障处理：若页在 swap 中则加载入内存。
//!
//! 本模块不做真实内存管理，仅模拟 swap 后端的读写、插槽分配与页故障
//! 处理流程，适合内存管理子系统的原型验证。

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 交换插槽与错误
// =====================================================================================

/// 页大小（字节），与 page_cache 保持一致。
pub const SWAP_PAGE_SIZE: usize = 4096;

/// swap 插槽标识符：在 swap 文件中的页索引。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SwapSlot(pub usize);

impl SwapSlot {
    pub fn index(self) -> usize {
        self.0
    }
}

/// 交换后端的页映射：物理页地址 -> swap 插槽。
#[derive(Clone, Debug)]
pub struct SwapEntry {
    pub slot: SwapSlot,
    pub page_addr: usize,
    pub swapped_out_at: u64, // Unix 秒
}

/// swap 错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapError {
    /// swap 区未初始化。
    NotInitialized,
    /// 插槽超出最大页数。
    SlotOutOfRange,
    /// 插槽未被使用（无数据）。
    SlotUnused,
    /// 插槽已被占用。
    SlotOccupied,
    /// 文件 IO 错误。
    IoError,
    /// 页不在 swap 中（页故障时无法换入）。
    PageNotSwapped,
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SwapError::NotInitialized => "swap 区未初始化".to_string(),
            SwapError::SlotOutOfRange => "插槽超出最大页数".to_string(),
            SwapError::SlotUnused => "插槽未被使用".to_string(),
            SwapError::SlotOccupied => "插槽已被占用".to_string(),
            SwapError::IoError => "文件 IO 错误".to_string(),
            SwapError::PageNotSwapped => "页不在 swap 中".to_string(),
        };
        write!(f, "{}", s)
    }
}

// =====================================================================================
// SwapArea —— swap 区
// =====================================================================================

/// swap 区：以宿主文件系统文件作为后端，管理固定数量的页插槽。
///
/// swap 文件按 `SWAP_PAGE_SIZE` 字节一页组织，第 `i` 个插槽对应文件偏移
/// `i * SWAP_PAGE_SIZE`。内存中维护：
/// * `slot_to_page` —— 插槽 -> 物理页地址（已占用插槽）；
/// * `page_to_slot` —— 物理页地址 -> 插槽（反向查找，支持页故障）；
/// * `free_slots` —— 空闲插槽栈；
/// * 统计：换出次数、换入次数、释放次数、页故障次数。
pub struct SwapArea {
    pub path: PathBuf,
    pub max_pages: usize,
    /// 插槽 -> 物理页地址。
    slot_to_page: BTreeMap<usize, usize>,
    /// 物理页地址 -> 插槽索引。
    page_to_slot: HashMap<usize, usize>,
    /// 空闲插槽索引栈。
    free_slots: Vec<usize>,
    /// 下一个尝试分配的插槽索引（顺序分配，免去栈空时的搜索）。
    next_slot: usize,
    /// 统计。
    swap_out_count: u64,
    swap_in_count: u64,
    free_slot_count: u64,
    page_fault_count: u64,
}

impl SwapArea {
    /// 创建或打开 swap 区。`path` 为宿主文件系统路径，`max_pages` 为最大页数。
    ///
    /// 若文件不存在则创建并按 `max_pages * SWAP_PAGE_SIZE` 预分配空间；若存在
    /// 则直接打开（不清空，以保留既有 swap 内容，模拟重启后 swap 保留）。
    pub fn new(path: &Path, max_pages: usize) -> Result<Self, SwapError> {
        let path = path.to_path_buf();
        // 确保父目录存在。
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // 创建或打开文件。
        let _file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)
            .map_err(|_| SwapError::IoError)?;
        // 预扩展文件到 max_pages 页。
        let expected_len = max_pages * SWAP_PAGE_SIZE;
        let metadata = fs::metadata(&path).map_err(|_| SwapError::IoError)?;
        if metadata.len() < expected_len as u64 {
            let mut f = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(|_| SwapError::IoError)?;
            f.seek(SeekFrom::End(0)).map_err(|_| SwapError::IoError)?;
            let mut extra = expected_len as i64 - metadata.len() as i64;
            while extra > 0 {
                let chunk = extra.min(1 << 20);
                let buf = vec![0u8; chunk as usize];
                f.write_all(&buf).map_err(|_| SwapError::IoError)?;
                extra -= chunk;
            }
            f.flush().map_err(|_| SwapError::IoError)?;
        }
        let mut free_slots: Vec<usize> = (0..max_pages).rev().collect(); // 反转，pop 从小到大
        Ok(Self {
            path,
            max_pages,
            slot_to_page: BTreeMap::new(),
            page_to_slot: HashMap::new(),
            free_slots,
            next_slot: 0,
            swap_out_count: 0,
            swap_in_count: 0,
            free_slot_count: 0,
            page_fault_count: 0,
        })
    }

    /// 返回最大页数。
    pub fn max_pages(&self) -> usize {
        self.max_pages
    }

    /// 返回已用插槽数。
    pub fn used_slots(&self) -> usize {
        self.slot_to_page.len()
    }

    /// 返回空闲插槽数。
    pub fn free_slots_count(&self) -> usize {
        self.max_pages - self.slot_to_page.len()
    }

    /// 返回 swap 使用率（0.0~1.0）。
    pub fn usage(&self) -> f64 {
        if self.max_pages == 0 {
            return 0.0;
        }
        self.slot_to_page.len() as f64 / self.max_pages as f64
    }

    /// 将一页数据换出到 swap，返回分配的 `SwapSlot`。
    ///
    /// 若该物理页已经换出，则复用原插槽并更新数据；若 swap 已满，返回
    /// `SlotOutOfRange`（实际中应由页替换算法腾出插槽，此处简化）。
    pub fn swap_out(&mut self, page_addr: usize, data: &[u8]) -> Result<SwapSlot, SwapError> {
        // 若该页已换出，复用原插槽。
        if let Some(&slot_idx) = self.page_to_slot.get(&page_addr) {
            self.write_slot(slot_idx, data)?;
            self.swap_out_count += 1;
            return Ok(SwapSlot(slot_idx));
        }
        // 分配新插槽。
        let slot_idx = self.allocate_slot()?;
        self.write_slot(slot_idx, data)?;
        self.slot_to_page.insert(slot_idx, page_addr);
        self.page_to_slot.insert(page_addr, slot_idx);
        self.swap_out_count += 1;
        Ok(SwapSlot(slot_idx))
    }

    /// 从 swap 换入指定插槽的数据。
    ///
    /// 换入后插槽保留（不自动释放），由调用方决定何时 `free_slot`。
    pub fn swap_in(&self, slot: SwapSlot) -> Result<Vec<u8>, SwapError> {
        let slot_idx = slot.index();
        if slot_idx >= self.max_pages {
            return Err(SwapError::SlotOutOfRange);
        }
        if !self.slot_to_page.contains_key(&slot_idx) {
            return Err(SwapError::SlotUnused);
        }
        self.read_slot(slot_idx)
    }

    /// 释放指定 swap 插槽。
    pub fn free_slot(&mut self, slot: SwapSlot) -> Result<(), SwapError> {
        let slot_idx = slot.index();
        if slot_idx >= self.max_pages {
            return Err(SwapError::SlotOutOfRange);
        }
        let page_addr = match self.slot_to_page.remove(&slot_idx) {
            Some(a) => a,
            None => return Err(SwapError::SlotUnused),
        };
        self.page_to_slot.remove(&page_addr);
        self.free_slots.push(slot_idx);
        self.free_slot_count += 1;
        Ok(())
    }

    /// 查询物理页是否已换出到 swap，若是则返回对应插槽。
    pub fn is_swapped_out(&self, page_addr: usize) -> Option<SwapSlot> {
        self.page_to_slot
            .get(&page_addr)
            .map(|&s| SwapSlot(s))
    }

    /// 页故障处理：若物理页 `addr` 已在 swap 中，则将其换入并返回数据；
    // 否则返回 `PageNotSwapped`。
    ///
    /// 这是缺页中断的简化模型：页故障时检查 swap 表，命中则从 swap 文件读入。
    pub fn page_fault_handler(&mut self, addr: usize) -> Result<Vec<u8>, SwapError> {
        self.page_fault_count += 1;
        if let Some(slot) = self.page_to_slot.get(&addr).copied() {
            self.swap_in_count += 1;
            self.swap_in(SwapSlot(slot))
        } else {
            Err(SwapError::PageNotSwapped)
        }
    }

    /// 返回换出总次数。
    pub fn swap_out_count(&self) -> u64 {
        self.swap_out_count
    }

    /// 返回换入总次数。
    pub fn swap_in_count(&self) -> u64 {
        self.swap_in_count
    }

    /// 返回页故障总次数。
    pub fn page_fault_count(&self) -> u64 {
        self.page_fault_count
    }

    /// 释放插槽总次数。
    pub fn freed_slot_count(&self) -> u64 {
        self.free_slot_count
    }

    /// 分配一个空闲插槽索引。
    fn allocate_slot(&mut self) -> Result<usize, SwapError> {
        if let Some(idx) = self.free_slots.pop() {
            return Ok(idx);
        }
        // 栈为空，尝试顺序分配未使用的插槽。
        while self.next_slot < self.max_pages {
            let idx = self.next_slot;
            self.next_slot += 1;
            if !self.slot_to_page.contains_key(&idx) {
                return Ok(idx);
            }
        }
        Err(SwapError::SlotOutOfRange)
    }

    /// 向指定插槽写入数据（按页大小对齐，不足补零）。
    fn write_slot(&mut self, slot_idx: usize, data: &[u8]) -> Result<(), SwapError> {
        let offset = slot_idx * SWAP_PAGE_SIZE;
        let mut f = OpenOptions::new()
            .write(true)
            .open(&self.path)
            .map_err(|_| SwapError::IoError)?;
        f.seek(SeekFrom::Start(offset as u64)).map_err(|_| SwapError::IoError)?;
        let write_len = data.len().min(SWAP_PAGE_SIZE);
        f.write_all(&data[..write_len]).map_err(|_| SwapError::IoError)?;
        // 若数据不足一页，补零到页尾。
        if write_len < SWAP_PAGE_SIZE {
            let zeros = vec![0u8; SWAP_PAGE_SIZE - write_len];
            f.write_all(&zeros).map_err(|_| SwapError::IoError)?;
        }
        f.flush().map_err(|_| SwapError::IoError)?;
        Ok(())
    }

    /// 从指定插槽读取一页数据。
    fn read_slot(&self, slot_idx: usize) -> Result<Vec<u8>, SwapError> {
        let offset = slot_idx * SWAP_PAGE_SIZE;
        let mut f = OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|_| SwapError::IoError)?;
        f.seek(SeekFrom::Start(offset as u64)).map_err(|_| SwapError::IoError)?;
        let mut buf = vec![0u8; SWAP_PAGE_SIZE];
        f.read_exact(&mut buf).map_err(|_| SwapError::IoError)?;
        Ok(buf)
    }

    /// 返回 swap 区文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "swap path={:?} max_pages={} used={} free={} usage={:.2} swap_out={} swap_in={} page_faults={} freed_slots={}",
            self.path,
            self.max_pages,
            self.used_slots(),
            self.free_slots_count(),
            self.usage(),
            self.swap_out_count,
            self.swap_in_count,
            self.page_fault_count,
            self.free_slot_count,
        )
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SWAP_AREA: OnceLock<Mutex<Option<SwapArea>>> = OnceLock::new();

fn swap_area() -> &'static Mutex<Option<SwapArea>> {
    SWAP_AREA.get_or_init(|| Mutex::new(None))
}

/// 初始化全局 swap 区（路径 + 最大页数）。
pub fn init_swap(path: &Path, max_pages: usize) -> Result<(), SwapError> {
    let area = SwapArea::new(path, max_pages)?;
    *swap_area().lock().unwrap() = Some(area);
    Ok(())
}

/// 将一页换出到全局 swap 区。
pub fn swap_out(page_addr: usize, data: &[u8]) -> Result<SwapSlot, SwapError> {
    swap_area()
        .lock()
        .unwrap()
        .as_mut()
        .ok_or(SwapError::NotInitialized)?
        .swap_out(page_addr, data)
}

/// 从全局 swap 区换入指定插槽。
pub fn swap_in(slot: SwapSlot) -> Result<Vec<u8>, SwapError> {
    swap_area()
        .lock()
        .unwrap()
        .as_ref()
        .ok_or(SwapError::NotInitialized)?
        .swap_in(slot)
}

/// 释放全局 swap 区中的插槽。
pub fn free_swap_slot(slot: SwapSlot) -> Result<(), SwapError> {
    swap_area()
        .lock()
        .unwrap()
        .as_mut()
        .ok_or(SwapError::NotInitialized)?
        .free_slot(slot)
}

/// 页故障处理器（全局 swap 区）。
pub fn handle_page_fault(addr: usize) -> Result<Vec<u8>, SwapError> {
    swap_area()
        .lock()
        .unwrap()
        .as_mut()
        .ok_or(SwapError::NotInitialized)?
        .page_fault_handler(addr)
}

/// 返回全局 swap 区的使用率。
pub fn swap_usage() -> Result<f64, SwapError> {
    Ok(swap_area()
        .lock()
        .unwrap()
        .as_ref()
        .ok_or(SwapError::NotInitialized)?
        .usage())
}

/// 返回全局 swap 区摘要。
pub fn swap_summary() -> Result<String, SwapError> {
    Ok(swap_area()
        .lock()
        .unwrap()
        .as_ref()
        .ok_or(SwapError::NotInitialized)?
        .summary())
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn test_swap_path() -> PathBuf {
        temp_dir().join(format!("hibos_swap_test_{}.swp", std::process::id()))
    }

    #[test]
    fn swap_out_and_in() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 64).unwrap();
        let data = vec![0xAAu8; 4096];
        let slot = area.swap_out(100, &data).unwrap();
        assert_eq!(slot.index(), 0);
        assert_eq!(area.used_slots(), 1);
        let read_back = area.swap_in(slot).unwrap();
        assert_eq!(read_back, data);
        assert_eq!(area.swap_out_count(), 1);
        assert_eq!(area.swap_in_count(), 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_page_alignment_padding() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 16).unwrap();
        let short_data = vec![0x55u8; 100];
        let slot = area.swap_out(200, &short_data).unwrap();
        let read_back = area.swap_in(slot).unwrap();
        assert_eq!(read_back.len(), SWAP_PAGE_SIZE);
        assert_eq!(read_back[0], 0x55);
        assert_eq!(read_back[100], 0x00);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_free_slot_reuse() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 16).unwrap();
        let data = vec![0x11u8; 4096];
        let slot1 = area.swap_out(1, &data).unwrap();
        area.free_slot(slot1).unwrap();
        assert_eq!(area.used_slots(), 0);
        assert_eq!(area.free_slots_count(), 16);
        // 释放后的插槽可被重新分配
        let slot2 = area.swap_out(2, &data).unwrap();
        assert_eq!(slot2.index(), slot1.index());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_page_fault_handler_hit_and_miss() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 32).unwrap();
        let data = vec![0x22u8; 4096];
        area.swap_out(300, &data).unwrap();
        // 页故障命中
        let loaded = area.page_fault_handler(300).unwrap();
        assert_eq!(loaded, data);
        assert_eq!(area.page_fault_count(), 1);
        // 页故障未命中
        let err = area.page_fault_handler(400).unwrap_err();
        assert_eq!(err, SwapError::PageNotSwapped);
        assert_eq!(area.page_fault_count(), 2);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_usage_rate() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 100).unwrap();
        assert!((area.usage() - 0.0).abs() < 1e-6);
        let data = vec![0u8; 4096];
        for i in 0..10 {
            let _ = area.swap_out(i, &data).unwrap();
        }
        assert!((area.usage() - 0.1).abs() < 1e-6);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_slot_out_of_range() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 4).unwrap();
        let data = vec![0u8; 4096];
        // 填满 4 个插槽
        for i in 0..4 {
            let _ = area.swap_out(i, &data).unwrap();
        }
        // 第 5 次应失败（SlotOutOfRange）
        assert_eq!(area.swap_out(99, &data).unwrap_err(), SwapError::SlotOutOfRange);
        // 无效插槽 swap_in
        assert_eq!(
            area.swap_in(SwapSlot(99)).unwrap_err(),
            SwapError::SlotUnused
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn swap_summary_contains_fields() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        let mut area = SwapArea::new(&path, 16).unwrap();
        let data = vec![0u8; 4096];
        let _ = area.swap_out(5, &data).unwrap();
        let s = area.summary();
        assert!(s.contains("max_pages="));
        assert!(s.contains("usage="));
        assert!(s.contains("swap_out="));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn global_swap_init_and_usage() {
        let path = test_swap_path();
        let _ = fs::remove_file(&path);
        assert!(init_swap(&path, 32).is_ok());
        let data = vec![0x33u8; 4096];
        assert!(swap_out(7, &data).is_ok());
        let u = swap_usage().unwrap();
        assert!(u > 0.0);
        let s = swap_summary().unwrap();
        assert!(s.contains("swap_out="));
        let _ = fs::remove_file(&path);
    }
}
