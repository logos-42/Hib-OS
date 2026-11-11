//! 块设备实现 —— P0-4 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::block::{BlockDevice, BlockDeviceType};

/// 块设备注册表。
struct BlockDeviceRegistry {
    devices: BTreeMap<String, BlockDevice>,
}

static BLOCK_REGISTRY: OnceLock<Mutex<BlockDeviceRegistry>> = OnceLock::new();

fn block_registry() -> &'static Mutex<BlockDeviceRegistry> {
    BLOCK_REGISTRY.get_or_init(|| Mutex::new(BlockDeviceRegistry {
        devices: BTreeMap::new(),
    }))
}

/// 注册块设备。
pub fn block_device_register(dev: BlockDevice) -> Result<(), String> {
    let mut g = block_registry().lock().unwrap();
    if g.devices.contains_key(&dev.name) {
        return Err(format!("设备已存在: {}", dev.name));
    }
    g.devices.insert(dev.name.clone(), dev);
    Ok(())
}

/// 查找块设备。
pub fn block_device_lookup(name: &str) -> Option<BlockDevice> {
    let g = block_registry().lock().unwrap();
    g.devices.get(name).cloned()
}

/// 列出所有块设备。
pub fn block_device_list() -> Vec<BlockDevice> {
    let g = block_registry().lock().unwrap();
    g.devices.values().cloned().collect()
}

/// 读取块设备数据（模拟：返回零填充）。
pub fn block_read(device: &str, block: u64, count: u64) -> Result<Vec<u8>, String> {
    let _ = device;
    let _ = block;
    Ok(vec![0u8; (count * 512) as usize])
}

/// 写入块设备数据（模拟）。
pub fn block_write(device: &str, block: u64, data: &[u8]) -> Result<(), String> {
    let _ = device;
    let _ = block;
    let _ = data;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockDeviceType;

    #[test]
    fn register_and_lookup() {
        let dev = BlockDevice::new("sda", BlockDeviceType::Nvme, 512, 1000);
        assert!(block_device_register(dev).is_ok());
        assert!(block_device_lookup("sda").is_some());
        assert!(block_device_lookup("sdb").is_none());
    }

    #[test]
    fn register_duplicate() {
        let dev1 = BlockDevice::new("sda", BlockDeviceType::Disk, 512, 100);
        let dev2 = BlockDevice::new("sda", BlockDeviceType::Disk, 512, 100);
        assert!(block_device_register(dev1).is_ok());
        assert!(block_device_register(dev2).is_err());
    }

    #[test]
    fn block_read_write() {
        let dev = BlockDevice::new("ram", BlockDeviceType::RamDisk, 512, 10);
        block_device_register(dev).unwrap();
        let data = block_read("ram", 0, 1).unwrap();
        assert_eq!(data.len(), 512);
        assert!(block_write("ram", 0, &data).is_ok());
    }
}
