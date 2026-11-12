//! 块设备抽象与请求调度。
//!
//! 建模为 `block://<设备>` 通道。块设备按固定大小块读写，内置请求队列
//! 与简易电梯调度器。内存设备（测试 / ram 盘）与文件设备（磁盘映像）
//! 均实现 `BlockDevice`。

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use crate::fs::diskimg::DiskImage;

/// 标准块大小（4 KiB）。
pub const BLOCK_SIZE: usize = 4096;

/// 块操作类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockOp {
    Read,
    Write,
}

/// 块请求。
#[derive(Clone, Debug)]
pub struct BlockRequest {
    pub device: String,
    pub op: BlockOp,
    pub block: u64,
    pub data: Vec<u8>,
}

impl BlockRequest {
    pub fn read(device: &str, block: u64) -> Self {
        Self {
            device: device.to_string(),
            op: BlockOp::Read,
            block,
            data: vec![0u8; BLOCK_SIZE],
        }
    }
    pub fn write(device: &str, block: u64, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), BLOCK_SIZE, "write data must be exactly one block");
        Self {
            device: device.to_string(),
            op: BlockOp::Write,
            block,
            data,
        }
    }
}

/// 块设备错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockError {
    NotFound,
    IoError(String),
    OutOfRange,
    ReadOnly,
}

impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockError::NotFound => write!(f, "块设备不存在"),
            BlockError::IoError(e) => write!(f, "块设备 IO 错误: {}", e),
            BlockError::OutOfRange => write!(f, "块索引超出范围"),
            BlockError::ReadOnly => write!(f, "块设备只读"),
        }
    }
}

/// 块设备能力。
pub trait BlockDevice {
    fn name(&self) -> &str;
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn block_count(&self) -> u64;
    fn read_at(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_at(&mut self, block: u64, buf: &[u8]) -> Result<(), BlockError>;
}

/// 内存块设备（随机访问，用于测试与 ram 盘）。
pub struct MemBlockDevice {
    name: String,
    data: Vec<u8>,
    read_only: bool,
}

impl MemBlockDevice {
    pub fn new(name: &str, blocks: u64) -> Self {
        Self {
            name: name.to_string(),
            data: vec![0u8; (blocks as usize) * BLOCK_SIZE],
            read_only: false,
        }
    }
    pub fn with_data(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.to_string(),
            data,
            read_only: false,
        }
    }
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }
}

impl BlockDevice for MemBlockDevice {
    fn name(&self) -> &str {
        &self.name
    }
    fn block_count(&self) -> u64 {
        (self.data.len() / BLOCK_SIZE) as u64
    }
    fn read_at(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let start = (block as usize) * BLOCK_SIZE;
        if start + buf.len() > self.data.len() {
            return Err(BlockError::OutOfRange);
        }
        buf.copy_from_slice(&self.data[start..start + buf.len()]);
        Ok(())
    }
    fn write_at(&mut self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        if self.read_only {
            return Err(BlockError::ReadOnly);
        }
        let start = (block as usize) * BLOCK_SIZE;
        if start + buf.len() > self.data.len() {
            return Err(BlockError::OutOfRange);
        }
        self.data[start..start + buf.len()].copy_from_slice(buf);
        Ok(())
    }
}

/// 文件块设备（磁盘映像文件）。内部委托 `DiskImage` 做文件级块读写。
pub struct FileBlockDevice {
    name: String,
    image: DiskImage,
}

impl FileBlockDevice {
    pub fn open(path: &Path, name: &str, read_only: bool) -> Result<Self, BlockError> {
        let image = DiskImage::open(path, read_only)
            .map_err(|e| BlockError::IoError(e.to_string()))?;
        Ok(Self {
            name: name.to_string(),
            image,
        })
    }

    pub fn create(path: &Path, name: &str, blocks: u64) -> Result<Self, BlockError> {
        let image = DiskImage::create(path, blocks)
            .map_err(|e| BlockError::IoError(e.to_string()))?;
        Ok(Self {
            name: name.to_string(),
            image,
        })
    }

    pub fn block_count(&self) -> u64 {
        self.image.block_count()
    }

    pub fn flush(&mut self) -> Result<(), BlockError> {
        self.image.flush().map_err(|e| BlockError::IoError(e.to_string()))
    }
}

impl BlockDevice for FileBlockDevice {
    fn name(&self) -> &str {
        &self.name
    }
    fn block_count(&self) -> u64 {
        self.image.block_count()
    }
    fn read_at(&mut self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.image.read_block_at(block, buf).map_err(|e| BlockError::IoError(e.to_string()))
    }
    fn write_at(&mut self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.image.write_block_at(block, buf).map_err(|e| BlockError::IoError(e.to_string()))
    }
}

/// 请求队列（FIFO）。
#[derive(Debug, Default)]
pub struct BlockQueue {
    queue: VecDeque<BlockRequest>,
}

impl BlockQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
    pub fn push(&mut self, req: BlockRequest) {
        self.queue.push_back(req);
    }
    pub fn pop(&mut self) -> Option<BlockRequest> {
        self.queue.pop_front()
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

/// 简易电梯调度：读优先，同类按块号升序（减少磁头寻道）。
#[derive(Debug, Default)]
pub struct BlockScheduler {
    pending: Vec<(String, BlockRequest)>,
}

impl BlockScheduler {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }
    pub fn push(&mut self, device: &str, req: BlockRequest) {
        self.pending.push((device.to_string(), req));
    }
    pub fn len(&self) -> usize {
        self.pending.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    /// 返回调度后的请求序列（取出全部），读在前、按块号升序。
    pub fn schedule(&mut self) -> Vec<(String, BlockRequest)> {
        let mut reads: Vec<(String, BlockRequest)> = Vec::new();
        let mut writes: Vec<(String, BlockRequest)> = Vec::new();
        for (device, req) in self.pending.drain(..) {
            match req.op {
                BlockOp::Read => reads.push((device, req)),
                BlockOp::Write => writes.push((device, req)),
            }
        }
        reads.sort_by(|a, b| a.1.block.cmp(&b.1.block));
        writes.sort_by(|a, b| a.1.block.cmp(&b.1.block));
        let mut out = reads;
        out.extend(writes);
        out
    }
}

/// 块设备管理器：按名称持有多块设备，提供 `block://<设备>` 视图，
/// 并拥有请求队列与调度器。
pub struct BlockManager {
    devices: BTreeMap<String, Box<dyn BlockDevice + Send>>,
    scheduler: BlockScheduler,
}

impl BlockManager {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            scheduler: BlockScheduler::new(),
        }
    }

    pub fn insert(&mut self, name: &str, device: Box<dyn BlockDevice + Send>) {
        self.devices.insert(name.to_string(), device);
    }

    pub fn remove(&mut self, name: &str) -> Option<Box<dyn BlockDevice + Send>> {
        self.devices.remove(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.devices.contains_key(name)
    }

    pub fn list(&self) -> Vec<String> {
        self.devices.keys().cloned().collect()
    }

    pub fn block_count(&self, name: &str) -> Option<u64> {
        self.devices.get(name).map(|d| d.block_count())
    }

    pub fn device(&mut self, name: &str) -> Option<&mut (dyn BlockDevice + Send)> {
        self.devices.get_mut(name).map(|d| d.as_mut())
    }

    /// 直接读取一块（绕过调度器，适用于简单读取）。
    pub fn read_block(&mut self, device: &str, block: u64) -> Result<Vec<u8>, BlockError> {
        let dev = self.device(device).ok_or(BlockError::NotFound)?;
        let mut buf = vec![0u8; dev.block_size()];
        dev.read_at(block, &mut buf)?;
        Ok(buf)
    }

    /// 直接写入一块（绕过调度器）。
    pub fn write_block(
        &mut self,
        device: &str,
        block: u64,
        data: &[u8],
    ) -> Result<(), BlockError> {
        let dev = self.device(device).ok_or(BlockError::NotFound)?;
        dev.write_at(block, data)
    }

    /// 提交请求到调度队列。
    pub fn submit(&mut self, device: &str, req: BlockRequest) {
        self.scheduler.push(device, req);
    }

    /// 调度并执行全部待处理请求，返回完成数。读请求的数据会填回
    /// `BlockRequest.data`，供提交方通过回调取出（此处返回读取的数据映射）。
    pub fn flush(
        &mut self,
        on_read: &mut impl FnMut(&str, u64, &[u8]),
    ) -> Result<usize, BlockError> {
        let reqs = self.scheduler.schedule();
        let mut done = 0;
        for (device, req) in reqs {
            let dev = self.device(&device).ok_or(BlockError::NotFound)?;
            match req.op {
                BlockOp::Read => {
                    let mut buf = vec![0u8; req.data.len()];
                    dev.read_at(req.block, &mut buf)?;
                    on_read(&device, req.block, &buf);
                }
                BlockOp::Write => {
                    dev.write_at(req.block, &req.data)?;
                }
            }
            done += 1;
        }
        Ok(done)
    }
}

impl Default for BlockManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_device(name: &str, blocks: u64) -> MemBlockDevice {
        MemBlockDevice::new(name, blocks)
    }

    #[test]
    fn mem_block_read_write_roundtrip() {
        let mut dev = make_device("test0", 16);
        let block = 3;
        let mut data = [0u8; BLOCK_SIZE];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        dev.write_at(block, &data).unwrap();
        let mut back = [0u8; BLOCK_SIZE];
        dev.read_at(block, &mut back).unwrap();
        assert_eq!(data, back);
        assert_eq!(dev.block_count(), 16);
    }

    #[test]
    fn block_queue_fifo_order() {
        let mut q = BlockQueue::new();
        q.push(BlockRequest::read("d0", 5));
        q.push(BlockRequest::read("d0", 1));
        q.push(BlockRequest::read("d0", 9));
        assert_eq!(q.len(), 3);
        assert_eq!(q.pop().unwrap().block, 5);
        assert_eq!(q.pop().unwrap().block, 1);
        assert_eq!(q.pop().unwrap().block, 9);
        assert!(q.is_empty());
    }

    #[test]
    fn block_scheduler_reads_before_writes_sorted() {
        let mut sched = BlockScheduler::new();
        sched.push("d0", BlockRequest::write("d0", 9, vec![1; BLOCK_SIZE]));
        sched.push("d0", BlockRequest::read("d0", 2));
        sched.push("d0", BlockRequest::write("d0", 1, vec![2; BLOCK_SIZE]));
        sched.push("d0", BlockRequest::read("d0", 7));
        let ordered = sched.schedule();
        // 两个读在前且按块升序：2, 7；随后两个写：1, 9
        let blocks: Vec<u64> = ordered.iter().map(|(_, r)| r.block).collect();
        assert_eq!(blocks, vec![2, 7, 1, 9]);
        let ops: Vec<BlockOp> = ordered.iter().map(|(_, r)| r.op).collect();
        assert_eq!(ops, vec![BlockOp::Read, BlockOp::Read, BlockOp::Write, BlockOp::Write]);
    }

    #[test]
    fn file_block_device_persists_across_reopen() {
        let path = std::env::temp_dir().join(format!("hibos_block_test_{}.img", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut dev = FileBlockDevice::create(&path, "img0", 32).unwrap();
            let mut data = [0u8; BLOCK_SIZE];
            data[0] = 0xAB;
            dev.write_at(10, &data).unwrap();
            dev.flush().unwrap();
        }
        // 重新打开，数据应当持久化
        let mut dev = FileBlockDevice::open(&path, "img0", false).unwrap();
        assert_eq!(dev.block_count(), 32);
        let mut back = [0u8; BLOCK_SIZE];
        dev.read_at(10, &mut back).unwrap();
        assert_eq!(back[0], 0xAB);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn block_manager_insert_read_write() {
        let mut mgr = BlockManager::new();
        mgr.insert("ram0", Box::new(MemBlockDevice::new("ram0", 64)));
        assert!(mgr.contains("ram0"));
        assert_eq!(mgr.block_count("ram0"), Some(64));
        let mut data = [0u8; BLOCK_SIZE];
        data[100] = 0x42;
        mgr.write_block("ram0", 7, &data).unwrap();
        let back = mgr.read_block("ram0", 7).unwrap();
        assert_eq!(back[100], 0x42);
        assert_eq!(mgr.read_block("missing", 0).unwrap_err(), BlockError::NotFound);
    }
}
