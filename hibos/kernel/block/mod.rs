//! 块设备层 —— P0-4 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - I/O 设备 / 驱动 / 中断 → 工具调用 (Tool Use, MCP 风格注册表)
//! - 磁盘调度 (SCAN/C-SCAN) → 推理请求调度 + KV 缓存预取
//! - 块设备队列与请求调度 → 通道化块 I/O 请求
//! 
//! 通道：`block://<设备>`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod block_device;
pub mod request_queue;
pub mod io_scheduler;

// 公共类型
pub use block_device::{BlockDevice, BlockDeviceType, block_device_register, block_device_lookup};
pub use request_queue::{BlockRequest, RequestQueue, block_submit, block_complete};

// 块设备类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockDeviceType {
    Disk,
    Ssd,
    Nvme,
    RamDisk,
    Loopback,
}

/// 块设备。
#[derive(Clone, Debug)]
pub struct BlockDevice {
    pub name: String,
    pub dev_type: BlockDeviceType,
    pub block_size: u64,
    pub total_blocks: u64,
    pub read_only: bool,
    pub online: bool,
}

impl BlockDevice {
    pub fn new(name: &str, dev_type: BlockDeviceType, block_size: u64, total_blocks: u64) -> Self {
        Self {
            name: name.to_string(),
            dev_type,
            block_size,
            total_blocks,
            read_only: false,
            online: true,
        }
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.block_size * self.total_blocks
    }
}

/// 块 I/O 请求。
#[derive(Clone, Debug)]
pub struct BlockRequest {
    pub id: u64,
    pub device: String,
    pub block: u64,
    pub count: u64,
    pub write: bool,
    pub data: Vec<u8>,
    pub completed: bool,
}

impl BlockRequest {
    pub fn new(id: u64, device: &str, block: u64, count: u64, write: bool, data: Vec<u8>) -> Self {
        Self {
            id,
            device: device.to_string(),
            block,
            count,
            write,
            data,
            completed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_device_creation() {
        let dev = BlockDevice::new("sda", BlockDeviceType::Nvme, 512, 1_000_000);
        assert_eq!(dev.name, "sda");
        assert_eq!(dev.capacity_bytes(), 512_000_000);
        assert!(dev.online);
        assert!(!dev.read_only);
    }

    #[test]
    fn block_request_creation() {
        let req = BlockRequest::new(1, "sda", 100, 4, true, vec![0u8; 2048]);
        assert_eq!(req.id, 1);
        assert_eq!(req.block, 100);
        assert_eq!(req.count, 4);
        assert!(req.write);
        assert!(!req.completed);
    }

    #[test]
    fn block_device_type_variants() {
        let types = [
            BlockDeviceType::Disk,
            BlockDeviceType::Ssd,
            BlockDeviceType::Nvme,
            BlockDeviceType::RamDisk,
            BlockDeviceType::Loopback,
        ];
        assert_eq!(types.len(), 5);
    }
}
