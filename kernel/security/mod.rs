//! 安全子系统 —— 沙箱、审计与加密原语。
//!
//! 本目录下包含三个并行模块：
//! * `sandbox` —— 能力沙箱、系统调用过滤（seccomp 简化版）、文件系统沙箱；
//! * `audit` —— 审计日志（JSON、查询过滤、轮转、统计）；
//! * `crypto` —— 消息摘要、对称加密、随机数、HMAC、Base64。
//!
//! 与 `kernel/caps.rs` 的能力模型配合，与 `kernel/syscall` 的系统调用层并行存在，不替换。

pub mod sandbox;
pub mod audit;
pub mod crypto;
