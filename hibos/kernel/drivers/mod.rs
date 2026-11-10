//! 设备驱动框架 —— P1-2 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - I/O 设备 / 驱动 / 中断 → 工具调用 (Tool Use, MCP 风格注册表)
//! - PCI 枚举 → 设备发现与注册
//! - 驱动模块注册/卸载 → 驱动热插拔
//! - 设备树 / ACPI → 设备层级树
//! 
//! 通道：`dev://drivers`、`dev://pci/*`、`irq://*`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod driver_registry;
pub mod pci_enumerator;
pub mod irq_dispatcher;
pub mod devtree;
pub mod irq_ctrl;
pub mod dma;

// 公共类型（已在本模块定义）
pub use pci_enumerator::{PciDevice, PciClass, pci_enumerate, pci_lookup};
pub use irq_dispatcher::{IrqLine, IrqHandler, irq_bind, irq_fire, irq_unbind};
pub use devtree::{DeviceTree, devtree_insert, devtree_lookup};

// 驱动状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverState {
    Unloaded,
    Loading,
    Loaded,
    Active,
    Failed,
    Unloading,
}

/// 设备驱动。
#[derive(Clone, Debug)]
pub struct Driver {
    pub name: String,
    pub version: String,
    pub state: DriverState,
    pub supported_devices: Vec<String>,
    pub init_fn: Option<String>,
    pub cleanup_fn: Option<String>,
}

impl Driver {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            state: DriverState::Unloaded,
            supported_devices: Vec::new(),
            init_fn: None,
            cleanup_fn: None,
        }
    }
}

/// PCI 设备类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PciClass {
    Storage,
    Network,
    Display,
    Bridge,
    Multimedia,
    Communication,
    System,
    Input,
    Docking,
    Processor,
    SerialBus,
    Wireless,
    Other,
}

/// PCI 设备。
#[derive(Clone, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: PciClass,
    pub irq_line: u8,
    pub bar0: u64,
}

/// IRQ 线。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IrqLine(pub u32);

/// IRQ 处理器签名。
pub type IrqHandler = fn(IrqLine);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_creation() {
        let driver = Driver::new("nvme-drv", "1.0.0");
        assert_eq!(driver.name, "nvme-drv");
        assert_eq!(driver.version, "1.0.0");
        assert_eq!(driver.state, DriverState::Unloaded);
    }

    #[test]
    fn pci_device_creation() {
        let dev = PciDevice {
            bus: 0,
            device: 5,
            function: 0,
            vendor_id: 0x8086,
            device_id: 0x1234,
            class_code: PciClass::Storage,
            irq_line: 11,
            bar0: 0xF000_0000,
        };
        assert_eq!(dev.bus, 0);
        assert_eq!(dev.device, 5);
        assert_eq!(dev.class_code, PciClass::Storage);
    }

    #[test]
    fn irq_line_creation() {
        let irq = IrqLine(42);
        assert_eq!(irq.0, 42);
    }

    #[test]
    fn pci_class_variants() {
        let classes = [
            PciClass::Storage,
            PciClass::Network,
            PciClass::Display,
            PciClass::Bridge,
            PciClass::Other,
        ];
        assert_eq!(classes.len(), 5);
    }
}
