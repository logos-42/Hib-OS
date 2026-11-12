//! x86_64 架构模块入口（重导出 x86_64 子模块）。

pub mod x86_64;
pub use x86_64::gdt;
pub use x86_64::idt;
