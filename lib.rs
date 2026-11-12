//! Hib OS —— 库入口。
//!
//! 二进制目标 `hibos` 以 `kernel/mod.rs` 为 crate root；集成测试
//! (`tests/`)、示例 (`examples/`) 与基准 (`benches/`) 无法直接引用 bin，
//! 因此这里额外提供一个库目标，把整个内核树以 `hibos::kernel::*` 暴露出去。
//!
//! 为了让 `kernel` 模块内部的 `crate::fs` / `crate::net` 等绝对路径在库
//! crate 中同样成立，根模块把 `kernel::*`（含全部子模块名）再导出一次。
//!
//! `alloc` crate 供内核 no_std 风格子模块（fs, net, caps 等）使用。

extern crate alloc;

pub mod kernel;

// 关键：把内核模块名提升到 crate 根，`crate::fs::...` 这类绝对路径才可用。
pub use kernel::*;
