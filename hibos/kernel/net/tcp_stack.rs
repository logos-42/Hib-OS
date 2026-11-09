//! 完整 TCP 协议栈（kernel/net/tcp_stack.rs）。
//!
//! 说明：轻量 TCP 实现位于 `tcp.rs`（`TcpLayer`/`TcpSegment`/`TcpState`），
//! 本模块为更完整的 TCP 状态机、连接管理与拥塞控制预留独立入口，与
//! `udp_stack.rs` 对称。
//!
//! 当前为占位骨架：不引入额外符号，避免与 `tcp::TcpLayer` 冲突。后续可在此
//! 填充拥塞控制、重传定时器、滑动窗口等更完整的能力。
