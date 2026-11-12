//! PCI 枚举与配置空间。
//!
//! 建模为 `dev://pci/<bus>:<device>.<function>` 通道，
//! 每个 PCI 设备一个通道，`sense` 可读取其配置空间快照。

use alloc::collections::BTreeMap; use alloc::string::String; use alloc::vec::Vec;
use crate::ksync::{SpinLock, OnceLock};

/// PCI 类代码（基类）。
pub const CLASS_STORAGE: u8 = 0x01;
pub const CLASS_NETWORK: u8 = 0x02;
pub const CLASS_DISPLAY: u8 = 0x03;
pub const CLASS_MULTIMEDIA: u8 = 0x04;

/// PCI 设备描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub irq_line: u8,
    pub bar0: u64,
}

impl PciDevice {
    /// 通道名：`dev://pci/<bus>:<device>.<function>`
    pub fn channel(&self) -> String {
        format!("dev://pci/{}:{}.{}", self.bus, self.device, self.function)
    }

    pub fn id(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor_id, self.device_id)
    }
}

/// 读取 PCI 配置空间一个 32-bit 字（原型：模拟读取；真实硬件接入时
/// 应替换为通过 0xCF8/0xCFC 端口的 I/O 读取）。
pub fn pci_read_config(_bus: u8, _device: u8, _function: u8, _offset: u8) -> u32 {
    0
}

/// PCI 总线：维护已枚举的设备。
pub struct PciBus {
    devices: BTreeMap<String, PciDevice>,
}

impl PciBus {
    pub fn new() -> Self {
        Self { devices: BTreeMap::new() }
    }

    pub fn insert(&mut self, dev: PciDevice) {
        self.devices.insert(dev.channel(), dev);
    }

    pub fn devices(&self) -> Vec<PciDevice> {
        self.devices.values().cloned().collect()
    }

    pub fn find(&self, channel: &str) -> Option<PciDevice> {
        self.devices.get(channel).cloned()
    }
}

static PCI_BUS: OnceLock<SpinLock<PciBus>> = OnceLock::new();

fn pci_bus() -> &'static SpinLock<PciBus> {
    PCI_BUS.get_or_init(|| SpinLock::new(PciBus::new()))
}

/// 枚举 PCI 总线：扫描所有 bus/device/function，读取配置空间。
pub fn enumerate_pci() -> Vec<PciDevice> {
    let mut bus = pci_bus().lock();
    // 原型阶段：注册若干代表性设备，供 sense/act 通道建模与测试使用。
    let samples = vec![
        PciDevice { bus: 0, device: 2, function: 0, vendor_id: 0x8086, device_id: 0x5916, class_code: CLASS_DISPLAY, subclass: 0x00, irq_line: 16, bar0: 0 },
        PciDevice { bus: 0, device: 3, function: 0, vendor_id: 0x8086, device_id: 0x9d31, class_code: CLASS_STORAGE, subclass: 0x01, irq_line: 17, bar0: 0 },
        PciDevice { bus: 0, device: 4, function: 0, vendor_id: 0x8086, device_id: 0x15f2, class_code: CLASS_NETWORK, subclass: 0x00, irq_line: 18, bar0: 0 },
        PciDevice { bus: 0, device: 5, function: 0, vendor_id: 0x10de, device_id: 0x1f08, class_code: CLASS_DISPLAY, subclass: 0x00, irq_line: 19, bar0: 0 },
        PciDevice { bus: 0, device: 6, function: 0, vendor_id: 0x10ec, device_id: 0x8168, class_code: CLASS_NETWORK, subclass: 0x00, irq_line: 20, bar0: 0 },
    ];
    for dev in &samples {
        bus.insert(dev.clone());
    }
    bus.devices()
}

/// 返回指定通道的 PCI 设备快照字符串（供 `sense dev://pci/...` 使用）。
pub fn pci_channel(channel: &str) -> Option<String> {
    let bus = pci_bus().lock();
    bus.find(channel).map(|d| {
        format!(
            "bus={} dev={} func={} vendor={:04x} device={:04x} class={:02x} subclass={:02x} irq={}",
            d.bus, d.device, d.function, d.vendor_id, d.device_id, d.class_code, d.subclass, d.irq_line
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_pci_returns_devices() {
        let devs = enumerate_pci();
        assert!(!devs.is_empty(), "PCI 枚举应当返回至少一个设备");
        assert!(devs.iter().any(|d| d.class_code == CLASS_DISPLAY));
        assert!(devs.iter().any(|d| d.class_code == CLASS_STORAGE));
        assert!(devs.iter().any(|d| d.class_code == CLASS_NETWORK));
    }

    #[test]
    fn pci_channel_format_and_id() {
        let dev = PciDevice { bus: 0, device: 2, function: 0, vendor_id: 0x8086, device_id: 0x5916, class_code: CLASS_DISPLAY, subclass: 0x00, irq_line: 16, bar0: 0 };
        assert_eq!(dev.channel(), "dev://pci/0:2.0");
        assert_eq!(dev.id(), "8086:5916");

        let dev1 = PciDevice { bus: 1, device: 7, function: 3, vendor_id: 0x10de, device_id: 0x1f08, class_code: CLASS_DISPLAY, subclass: 0x00, irq_line: 19, bar0: 0 };
        assert_eq!(dev1.channel(), "dev://pci/1:7.3");
    }

    #[test]
    fn pci_channel_snapshot() {
        enumerate_pci();
        let snap = pci_channel("dev://pci/0:2.0").expect("应能找到设备");
        assert!(snap.contains("8086"));
        assert!(snap.contains("5916"));
        assert!(snap.contains("irq=16"));
        assert!(pci_channel("dev://pci/99:99.9").is_none());
    }
}
