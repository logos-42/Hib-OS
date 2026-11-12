//! 驱动模块：串口 + VGA 文本 + DMA/PCI/IRQ/设备树（最早可用的输出，供调试和启动画面）。

pub mod serial;
pub mod vga_text;
pub mod dma;
pub mod devtree;
pub mod irq_ctrl;
pub mod pci_enumerate;
pub mod irq;
pub mod irq_dispatcher;
pub mod net_driver;
pub mod usb_stack;
pub mod device;
pub mod pci;
pub mod pci_enumerator;

// 向上层模块重导出常用中断类型，供 `crate::drivers::{IrqLine, IrqHandler}` 路径使用。
pub use irq_ctrl::{IrqHandler, IrqLine};
pub use pci::PciDevice;
