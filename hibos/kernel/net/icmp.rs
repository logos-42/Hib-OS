//! ICMP 报文处理（kernel/net/icmp.rs）。
//!
//! 说明：ICMP 的报文结构（`IcmpPacket`）与 echo 逻辑当前由 `ipv4.rs` 承担，
//! 并在 `net/mod.rs` 中通过 `pub use ipv4::{IcmpPacket, ICMP_ECHO_REQUEST,
//! ICMP_ECHO_REPLY, ...}` 对外暴露。本模块为协议栈分层预留独立入口，
//! 后续可在此扩展 ICMP 错误报文、时间戳、地址掩码等高级能力。
//!
//! 当前为占位骨架：不引入额外符号，避免与 `ipv4::IcmpPacket` 冲突。
