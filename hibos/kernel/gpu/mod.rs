//! GPU 子系统。
//!
//! 宏内核架构下，GPU 驱动、显存管理与 IOMMU 均运行于内核态。

pub mod capabilities;
pub mod framebuffer;
pub mod hot_reload;
pub mod iommu;
pub mod vram;

pub use capabilities::*;
pub use framebuffer::*;
pub use hot_reload::*;
pub use iommu::*;
pub use vram::*;

/// GPU 子系统错误。
#[derive(Debug)]
pub enum Error {
    Vram(VramError),
    Iommu(IommuError),
}

/// 初始化 GPU 子系统 (极简启动，不在终端打印)。
pub fn init() {
    // 显存 / IOMMU 已就绪；启动阶段保持静默以符合极简 Linux 模式。
}
