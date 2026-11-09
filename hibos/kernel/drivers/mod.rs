//! 设备驱动框架 —— `dev://drivers` 通道后端。
//!
//! Hib OS 内核扩展路线图 P1-2 部分。
//! 提供 PCI 枚举、设备注册表、IRQ 投递、设备树四大子系统，
//! 统一暴露为 Hib OS 感知通道，可被 `sense`/`act` 操作。
//!
//! 通道规范：
//!   - `dev://drivers`            设备注册表与驱动绑定状态
//!   - `dev://pci/<bus>:<device>.<function>`   PCI 设备配置空间快照
//!   - `irq://<设备>`             设备中断 signal 事件通道
//!   - `dev://tree`               设备层级树（类似 Linux devicetree）

pub mod device;
pub mod devtree;
pub mod irq;
pub mod pci;
pub mod net_driver;
pub mod pci_enumerate;
pub mod usb_stack;

// 重导出，供上层（sense.rs / shell）直接调用。
pub use device::{bind_driver, register_device, unbind_driver};
pub use devtree::{attach_devtree, detach_devtree, devtree_list};
pub use irq::{bind_irq, fire_irq, unbind_irq};
pub use pci::{enumerate_pci, pci_channel};
