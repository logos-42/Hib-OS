//! PCI 枚举器 —— P1-2 子模块。

use crate::ksync::{SpinLock, OnceLock};
use alloc::vec::Vec;

use crate::drivers::pci::PciDevice;

/// PCI 设备注册表。
struct PciRegistry {
    devices: Vec<PciDevice>,
}

static PCI_REG: OnceLock<SpinLock<Option<PciRegistry>>> = OnceLock::new();

fn pci_reg() -> &'static SpinLock<Option<PciRegistry>> {
    PCI_REG.get_or_init(|| SpinLock::new(None))
}

/// 枚举 PCI 设备（模拟：返回一组预设设备）。
pub fn pci_enumerate() -> Vec<PciDevice> {
    let mut g = pci_reg().lock();
    if g.is_none() {
        let devices = vec![
            PciDevice {
                bus: 0,
                device: 0,
                function: 0,
                vendor_id: 0x8086,
                device_id: 0x1234,
                class_code: crate::drivers::pci::CLASS_STORAGE,
                subclass: 0,
                irq_line: 11,
                bar0: 0xF000_0000,
            },
            PciDevice {
                bus: 0,
                device: 1,
                function: 0,
                vendor_id: 0x10DE,
                device_id: 0x5678,
                class_code: crate::drivers::pci::CLASS_DISPLAY,
                subclass: 0,
                irq_line: 12,
                bar0: 0xE000_0000,
            },
            PciDevice {
                bus: 0,
                device: 2,
                function: 0,
                vendor_id: 0x8086,
                device_id: 0x9ABC,
                class_code: crate::drivers::pci::CLASS_NETWORK,
                subclass: 0,
                irq_line: 13,
                bar0: 0xD000_0000,
            },
        ];
        *g = Some(PciRegistry { devices });
    }
    g.as_ref().unwrap().devices.clone()
}

/// 查找 PCI 设备。
pub fn pci_lookup(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    pci_enumerate()
        .into_iter()
        .find(|d| d.vendor_id == vendor_id && d.device_id == device_id)
}

/// PCI 设备数量。
pub fn pci_device_count() -> usize {
    pci_enumerate().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate() {
        let devs = pci_enumerate();
        assert_eq!(devs.len(), 3);
    }

    #[test]
    fn lookup() {
        let dev = pci_lookup(0x8086, 0x1234);
        assert!(dev.is_some());
        assert_eq!(dev.unwrap().class_code, crate::drivers::pci::CLASS_STORAGE);
    }

    #[test]
    fn lookup_not_found() {
        assert!(pci_lookup(0xFFFF, 0xFFFF).is_none());
    }
}
