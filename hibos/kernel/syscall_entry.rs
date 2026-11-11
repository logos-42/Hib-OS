//! 内核系统调用入口（no_std）。
//!
//! 提供 `sense`/`act` 通道分发，作为用户态与内核的系统调用接口。
//! 当前仅引用已迁移到 no_std 的内核模块（fs, time），其余通道
//! 留给后续阶段（net、GPU、桌面、AI 等）逐步填充。

use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::fs::{fs_act_dispatch, fs_sense_snapshot};
use crate::ksync::{SpinLock, OnceLock};
use crate::time::get_time_ns;

// =======================================================================
// 全局系统调用锁
// =======================================================================

/// 系统调用结果。
pub type SysCallResult = Result<String, String>;

/// 全局系统调用锁（序列化所有系统调用，避免并发问题）。
static SYSCALL_LOCK: OnceLock<SpinLock<()>> = OnceLock::new();

fn syscall_lock() -> &'static SpinLock<()> {
    SYSCALL_LOCK.get_or_init(|| SpinLock::new(()))
}

// =======================================================================
// sense：通道感知（只读快照）
// =======================================================================

/// 感知指定通道，返回快照字符串。返回 None 表示通道未实现。
pub fn sys_sense(name: &str) -> Option<String> {
    let _guard = syscall_lock().lock();
    match name {
        "fs://vfs" => Some(fs_sense_snapshot()),
        "dev://timer" => Some(format!("now={}ns", get_time_ns())),
        "dev://null" => Some("null device".to_string()),
        _ => None,
    }
}

// =======================================================================
// act：通道行动（读写操作）
// =======================================================================

/// 执行指定通道的行动。返回 None 表示通道未实现。
pub fn sys_act(name: &str, payload: &str) -> Option<SysCallResult> {
    let _guard = syscall_lock().lock();
    match name {
        "fs://vfs" => Some(fs_act_dispatch(payload)),
        "dev://null" => Some(Ok(format!("已丢弃 {} 字节", payload.len()))),
        "dev://timer" => Some(Ok(format!("now={}ns", get_time_ns()))),
        _ => None,
    }
}

// =======================================================================
// 系统调用分发入口（供外部调用）
// =======================================================================

/// 系统调用分发：name 为通道名，payload 为负载。
/// 返回 Some(Ok/Err) 表示已处理，None 表示通道未知。
pub fn syscall(name: &str, payload: &str) -> Option<SysCallResult> {
    // 先尝试 sense（如果 payload 为空或以 "sense:" 开头）
    if payload.is_empty() || payload == "sense" {
        return sys_sense(name).map(Ok);
    }
    // 否则当作 act 处理
    sys_act(name, payload)
}
