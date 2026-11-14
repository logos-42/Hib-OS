//! 文件锁（顾问锁 + 强制锁 + 范围锁）。
//!
//! 提供对打开文件描述符（fd）的字节范围加锁：
//! - 顾问锁（默认）：仅在通过本管理器检查时生效，不强制阻塞实际 IO。
//! - 强制锁：强制拒绝与之冲突的读写请求。
//! - 范围锁：锁定文件的指定字节区间 [start, start+len)。
//! - 死锁检测：简化为超时机制（等待超时则返回错误）。
//!
//! 与 `kernel/lease.rs` 的 epoch 并发模型配合，供 `fs/` / `file://` 通道使用。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::lease::{self, Epoch};
use crate::caps;

/// 锁类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockType {
    /// 读锁（共享）：多个读者可同时持有，互斥写锁。
    RdLock,
    /// 写锁（排他）：独占，互斥所有读写锁。
    WrLock,
    /// 解锁。
    Unlock,
}

/// 锁信息：描述某个范围锁的持有情况。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockInfo {
    pub lock_type: LockType,
    /// 持有者进程 ID。
    pub pid: u64,
    /// 锁定起始字节（含）。
    pub start: u64,
    /// 锁定长度（字节）。
    pub len: u64,
}

/// 锁错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockError {
    /// 文件描述符不存在。
    NoFd,
    /// 锁冲突（被另一个持有者占用）。
    Conflict,
    /// 指定范围上不存在锁。
    NotFound,
    /// 参数无效（如 start/len 超范围）。
    Invalid,
    /// 等待超时（简化的死锁检测）。
    Timeout,
    /// 不支持强制锁（文件未设置强制锁属性）。
    NotMandatory,
    /// 内部错误。
    Internal(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::NoFd => write!(f, "文件描述符不存在"),
            LockError::Conflict => write!(f, "锁冲突：范围已被他人占用"),
            LockError::NotFound => write!(f, "指定范围上不存在锁"),
            LockError::Invalid => write!(f, "无效的锁参数"),
            LockError::Timeout => write!(f, "等待锁超时（可能死锁）"),
            LockError::NotMandatory => write!(f, "文件未启用强制锁"),
            LockError::Internal(e) => write!(f, "锁内部错误: {}", e),
        }
    }
}

/// 单个范围锁记录。
#[derive(Clone, Debug)]
struct RangeLock {
    pub lock_type: LockType,
    pub pid: u64,
    pub start: u64,
    /// 结束字节（不含），start + len。
    pub end: u64,
    /// 是否强制锁。
    pub mandatory: bool,
    /// 锁上时间。
    pub acquired_at: Instant,
}

/// 文件级锁状态：每个 fd 的锁表。
#[derive(Clone, Debug, Default)]
struct FdLockState {
    /// 是否启用强制锁。
    mandatory: bool,
    /// 该文件上的所有范围锁。
    locks: Vec<RangeLock>,
}

/// 文件锁管理器：管理多个 fd 的范围锁，支持顾问/强制锁与超时死锁检测。
pub struct FileLockManager {
    /// 按 fd 索引的锁状态。
    fds: Mutex<BTreeMap<u64, FdLockState>>,
    /// 默认等待超时。
    default_timeout: Duration,
}

impl FileLockManager {
    /// 创建文件锁管理器。
    pub fn new() -> Self {
        Self {
            fds: Mutex::new(BTreeMap::new()),
            default_timeout: Duration::from_secs(5),
        }
    }

    /// 设置默认等待超时。
    pub fn set_default_timeout(&mut self, timeout: Duration) {
        self.default_timeout = timeout;
    }

    /// 确保 fd 存在（创建空锁状态）。
    fn ensure_fd(&self, g: &mut std::sync::MutexGuard<'_, BTreeMap<u64, FdLockState>>, fd: u64) {
        g.entry(fd).or_default();
    }

    /// 判断两个范围是否重叠。
    fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
        a_start < b_end && b_start < a_end
    }

    /// 判断两个锁类型是否兼容（可共存）。
    fn compatible(a: LockType, b: LockType) -> bool {
        // 读锁与读锁兼容；写锁与任何锁都不兼容。
        matches!((a, b), (LockType::RdLock, LockType::RdLock))
    }

    /// 检查指定范围上是否可加锁（不考虑自身持有的锁，除外 pid）。
    fn conflict_with(
        state: &FdLockState,
        pid: u64,
        start: u64,
        end: u64,
        lock_type: LockType,
    ) -> Option<RangeLock> {
        for lock in &state.locks {
            if lock.pid == pid {
                continue; // 同一进程的锁视为可替换/兼容
            }
            if !Self::ranges_overlap(start, end, lock.start, lock.end) {
                continue;
            }
            if !Self::compatible(lock_type, lock.lock_type) {
                return Some(lock.clone());
            }
        }
        None
    }

    /// 加锁。
    ///
    /// 若范围已被冲突锁占用：
    /// - 顾问锁：立即返回 Conflict。
    /// - 强制锁：若文件启用强制锁，也返回 Conflict；否则按顾问锁处理。
    pub fn lock(
        &self,
        fd: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
    ) -> Result<(), LockError> {
        if len == 0 {
            return Err(LockError::Invalid);
        }
        let end = start.checked_add(len).ok_or(LockError::Invalid)?;
        let mut g = self.fds.lock().unwrap();
        self.ensure_fd(&mut g, fd);
        let state = g.get_mut(&fd).unwrap();

        // 检查冲突。
        if let Some(conflict) = Self::conflict_with(state, 0, start, end, lock_type) {
            if state.mandatory || conflict.mandatory {
                return Err(LockError::Conflict);
            }
            // 顾问锁：简化为直接返回 Conflict（实际可选择等待或失败）。
            return Err(LockError::Conflict);
        }

        // 移除同一 pid 在该范围上的旧锁（替换语义）。
        state.locks.retain(|l| !(l.pid == 0 && Self::ranges_overlap(l.start, l.end, start, end)));
        // 注意：pid 0 为内部占位，实际使用时应传入真实 pid。此处用 0 表示“当前调用者”。
        state.locks.push(RangeLock {
            lock_type,
            pid: 0,
            start,
            end,
            mandatory: state.mandatory,
            acquired_at: Instant::now(),
        });
        Ok(())
    }

    /// 带 pid 的加锁（精确控制持有者）。
    pub fn lock_with_pid(
        &self,
        fd: u64,
        pid: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
    ) -> Result<(), LockError> {
        if len == 0 {
            return Err(LockError::Invalid);
        }
        let end = start.checked_add(len).ok_or(LockError::Invalid)?;
        let mut g = self.fds.lock().unwrap();
        self.ensure_fd(&mut g, fd);
        let state = g.get_mut(&fd).unwrap();

        if let Some(conflict) = Self::conflict_with(state, pid, start, end, lock_type) {
            if state.mandatory || conflict.mandatory {
                return Err(LockError::Conflict);
            }
            return Err(LockError::Conflict);
        }

        state.locks.retain(|l| !(l.pid == pid && Self::ranges_overlap(l.start, l.end, start, end)));
        state.locks.push(RangeLock {
            lock_type,
            pid,
            start,
            end,
            mandatory: state.mandatory,
            acquired_at: Instant::now(),
        });
        Ok(())
    }

    /// 带超时的加锁：若冲突则阻塞等待，超时返回 `Timeout`（简化的死锁检测）。
    pub fn lock_with_timeout(
        &self,
        fd: u64,
        pid: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
        timeout: Duration,
    ) -> Result<(), LockError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.lock_with_pid(fd, pid, lock_type, start, len) {
                Ok(()) => return Ok(()),
                Err(LockError::Conflict) => {
                    if Instant::now() >= deadline {
                        return Err(LockError::Timeout);
                    }
                    // 简化：短暂让出后重试（实际内核会挂起等待唤醒）。
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 解锁指定范围（pid 必须与持有者一致，或 pid 为 0 表示清空该范围所有锁）。
    pub fn unlock(&self, fd: u64, pid: u64, start: u64, len: u64) -> Result<(), LockError> {
        if len == 0 {
            return Err(LockError::Invalid);
        }
        let end = start.checked_add(len).ok_or(LockError::Invalid)?;
        let mut g = self.fds.lock().unwrap();
        let state = g.get_mut(&fd).ok_or(LockError::NoFd)?;
        let before = state.locks.len();
        state.locks.retain(|l| {
            let same_owner = pid == 0 || l.pid == pid;
            let overlaps = Self::ranges_overlap(l.start, l.end, start, end);
            !(same_owner && overlaps)
        });
        if state.locks.len() == before {
            return Err(LockError::NotFound); // 将 NotFound 加入枚举？这里用 Internal 代替
        }
        Ok(())
    }

    /// 测试锁：返回与 [start, start+len) 冲突的锁信息，若无冲突返回 None。
    pub fn test_lock(
        &self,
        fd: u64,
        pid: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
    ) -> Result<Option<LockInfo>, LockError> {
        if len == 0 {
            return Err(LockError::Invalid);
        }
        let end = start.checked_add(len).ok_or(LockError::Invalid)?;
        let g = self.fds.lock().unwrap();
        let state = g.get(&fd).ok_or(LockError::NoFd)?;
        if let Some(conflict) = Self::conflict_with(state, pid, start, end, lock_type) {
            return Ok(Some(LockInfo {
                lock_type: conflict.lock_type,
                pid: conflict.pid,
                start: conflict.start,
                len: conflict.end.saturating_sub(conflict.start),
            }));
        }
        Ok(None)
    }

    /// 设置文件的强制锁属性。
    pub fn set_mandatory(&self, fd: u64, mandatory: bool) -> Result<(), LockError> {
        let mut g = self.fds.lock().unwrap();
        let state = g.get_mut(&fd).ok_or(LockError::NoFd)?;
        state.mandatory = mandatory;
        Ok(())
    }

    /// 查询文件是否启用强制锁。
    pub fn is_mandatory(&self, fd: u64) -> Result<bool, LockError> {
        let g = self.fds.lock().unwrap();
        let state = g.get(&fd).ok_or(LockError::NoFd)?;
        Ok(state.mandatory)
    }

    /// 释放 fd 上的全部锁（进程退出时调用）。
    pub fn release_fd(&self, fd: u64) -> Result<(), LockError> {
        let mut g = self.fds.lock().unwrap();
        if g.remove(&fd).is_some() {
            Ok(())
        } else {
            Err(LockError::NoFd)
        }
    }

    /// 释放指定 pid 在 fd 上的全部锁。
    pub fn release_pid(&self, fd: u64, pid: u64) -> Result<usize, LockError> {
        let mut g = self.fds.lock().unwrap();
        let state = g.get_mut(&fd).ok_or(LockError::NoFd)?;
        let before = state.locks.len();
        state.locks.retain(|l| l.pid != pid);
        Ok(before - state.locks.len())
    }

    /// 列出 fd 上的全部锁信息。
    pub fn list_locks(&self, fd: u64) -> Result<Vec<LockInfo>, LockError> {
        let g = self.fds.lock().unwrap();
        let state = g.get(&fd).ok_or(LockError::NoFd)?;
        Ok(state
            .locks
            .iter()
            .map(|l| LockInfo {
                lock_type: l.lock_type,
                pid: l.pid,
                start: l.start,
                len: l.end.saturating_sub(l.start),
            })
            .collect())
    }

    /// 注册一个 fd（初始化空锁状态）。
    pub fn register_fd(&self, fd: u64) -> Result<(), LockError> {
        let mut g = self.fds.lock().unwrap();
        if g.contains_key(&fd) {
            return Err(LockError::Internal("fd 已存在".into()));
        }
        g.insert(fd, FdLockState::default());
        Ok(())
    }

    /// 返回管理的 fd 数量。
    pub fn fd_count(&self) -> usize {
        self.fds.lock().unwrap().len()
    }
}

// ============================================================================
// 并发控制：基于 lease.rs 的单写者租约 + epoch CAS
// ============================================================================

/// 基于 lease 的文件锁管理器包装器：对跨 fd 的锁操作加单写者租约，
/// 避免多线程并发加锁/解锁同一 fd 的范围时发生竞争。
pub struct FileLockManagerLeased {
    inner: FileLockManager,
    channel: String,
}

impl FileLockManagerLeased {
    /// 创建租约保护的文件锁管理器。
    pub fn new(channel: &str) -> Self {
        Self {
            inner: FileLockManager::new(),
            channel: channel.to_string(),
        }
    }

    /// 申请锁操作租约，返回当前 epoch。
    pub fn acquire_lock_lease(&self, owner: u64) -> Result<Epoch, crate::lease::LeaseConflict> {
        lease::acquire(&self.channel, owner, None)
    }

    /// 释放锁操作租约。
    pub fn release_lock_lease(&self, owner: u64) -> bool {
        lease::release(&self.channel, owner)
    }

    /// 注册 fd（委托内部管理器，无需租约）。
    pub fn register_fd(&self, fd: u64) -> Result<(), LockError> {
        self.inner.register_fd(fd)
    }

    /// 在租约保护下加锁，然后 commit 推进 epoch。
    pub fn lock_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        fd: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
        pid: u64,
    ) -> Result<Epoch, LockError> {
        let _ = self.acquire_lock_lease(owner)
            .map_err(|c| LockError::Internal(format!("租约冲突: holder={:?}", c.holder)))?;
        self.inner.lock_with_pid(fd, pid, lock_type, start, len)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("file_lock lock fd={} start={} len={}", fd, start, len),
        )
        .map_err(|e| LockError::Internal(format!("lease commit failed: {:?}", e)))?;
        Ok(new_ep)
    }

    /// 在租约保护下解锁，然后 commit 推进 epoch。
    pub fn unlock_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        fd: u64,
        pid: u64,
        start: u64,
        len: u64,
    ) -> Result<Epoch, LockError> {
        let _ = self.acquire_lock_lease(owner)
            .map_err(|c| LockError::Internal(format!("租约冲突: holder={:?}", c.holder)))?;
        self.inner.unlock(fd, pid, start, len)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("file_lock unlock fd={} start={} len={}", fd, start, len),
        )
        .map_err(|e| LockError::Internal(format!("lease commit failed: {:?}", e)))?;
        Ok(new_ep)
    }

    /// 释放 fd 上的全部锁（进程退出时调用，需租约）。
    pub fn release_fd_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        fd: u64,
    ) -> Result<Epoch, LockError> {
        let _ = self.acquire_lock_lease(owner)
            .map_err(|c| LockError::Internal(format!("租约冲突: holder={:?}", c.holder)))?;
        self.inner.release_fd(fd)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("file_lock release_fd fd={}", fd),
        )
        .map_err(|e| LockError::Internal(format!("lease commit failed: {:?}", e)))?;
        Ok(new_ep)
    }

    /// 列出 fd 上的全部锁（只读，无需租约）。
    pub fn list_locks(&self, fd: u64) -> Result<Vec<LockInfo>, LockError> {
        self.inner.list_locks(fd)
    }

    /// 强制刷新（force 模式，绕过租约）。
    pub fn force_unlock_all(&self, owner: u64) -> Epoch {
        lease::force(&self.channel, &format!("file_lock force_unlock_all owner={}", owner))
    }

    /// 返回当前 epoch。
    pub fn epoch(&self) -> Epoch {
        lease::epoch(&self.channel)
    }
}

/// 能力位权限包装器：在文件锁操作上加 `caps` 校验。
///
/// `CAP_SYS_ADMIN` 允许所有锁操作；无此能力则拒绝加锁/解锁。
pub struct FileLockManagerCaps {
    inner: FileLockManager,
    caps: Arc<Mutex<caps::Caps>>,
}

impl FileLockManagerCaps {
    /// 包装一个已有锁管理器，绑定全局能力表。
    pub fn new(mgr: FileLockManager) -> Self {
        Self {
            inner: mgr,
            caps: Arc::new(Mutex::new(caps::caps().lock().unwrap().clone())),
        }
    }

    fn check_admin(&self) -> Result<(), String> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_ADMIN") {
            Ok(())
        } else {
            Err("缺少 CAP_SYS_ADMIN 能力".to_string())
        }
    }

    /// 注册 fd（需 CAP_SYS_ADMIN）。
    pub fn register_fd(&self, fd: u64) -> Result<(), String> {
        self.check_admin()?;
        self.inner.register_fd(fd).map_err(|e| e.to_string())
    }

    /// 加锁（需 CAP_SYS_ADMIN）。
    pub fn lock_with_pid(
        &self,
        fd: u64,
        pid: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
    ) -> Result<(), String> {
        self.check_admin()?;
        self.inner.lock_with_pid(fd, pid, lock_type, start, len).map_err(|e| e.to_string())
    }

    /// 解锁（需 CAP_SYS_ADMIN）。
    pub fn unlock(&self, fd: u64, pid: u64, start: u64, len: u64) -> Result<(), String> {
        self.check_admin()?;
        self.inner.unlock(fd, pid, start, len).map_err(|e| e.to_string())
    }

    /// 测试锁（只读，无需能力）。
    pub fn test_lock(
        &self,
        fd: u64,
        pid: u64,
        lock_type: LockType,
        start: u64,
        len: u64,
    ) -> Result<Option<LockInfo>, String> {
        self.inner.test_lock(fd, pid, lock_type, start, len).map_err(|e| e.to_string())
    }

    /// 列出锁（只读，无需能力）。
    pub fn list_locks(&self, fd: u64) -> Result<Vec<LockInfo>, String> {
        self.inner.list_locks(fd).map_err(|e| e.to_string())
    }

    /// 释放 fd（需 CAP_SYS_ADMIN）。
    pub fn release_fd(&self, fd: u64) -> Result<(), String> {
        self.check_admin()?;
        self.inner.release_fd(fd).map_err(|e| e.to_string())
    }
}

// ============================================================================
// sense/act 通道接口：filelock/<设备名>
// ============================================================================

/// 感知文件锁通道：返回锁管理器状态摘要。
///
/// 通道名形式：`filelock/<设备名>`
pub fn probe_file_lock(name: &str) -> Option<String> {
    let channel = format!("filelock/{}", name);
    let epoch = lease::epoch(&channel);
    Some(format!(
        "channel={} type=file_lock epoch={} ops=lock|unlock|test_lock|list_locks|release_fd",
        name, epoch
    ))
}

/// 对文件锁通道执行行动。
///
/// 支持子命令：
/// - `status` —— 返回锁状态摘要（占位）
/// - `epoch` —— 返回当前 epoch
pub fn act_file_lock(name: &str, payload: &str) -> Option<Result<String, String>> {
    let mut tokens = payload.split_whitespace();
    let sub = tokens.next().unwrap_or("");
    let channel = format!("filelock/{}", name);
    match sub {
        "status" => Some(Ok(format!(
            "filelock/{} status=active epoch={}",
            name,
            lease::epoch(&channel)
        ))),
        "epoch" => Some(Ok(format!("epoch={}", lease::epoch(&channel)))),
        _ => Some(Err("用法: status | epoch".to_string())),
    }
}

impl Default for FileLockManager {
    fn default() -> Self {
        Self::new()
    }
}

// NotFound 变体未在枚举中定义；unlock 使用 Internal 代替，保持枚举简洁。

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn file_lock_rdwr_conflict() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        // 读锁可共享。
        assert!(mgr.lock_with_pid(1, 10, LockType::RdLock, 0, 100).is_ok());
        assert!(mgr.lock_with_pid(1, 11, LockType::RdLock, 0, 100).is_ok());
        // 写锁与读锁冲突。
        assert_eq!(
            mgr.lock_with_pid(1, 12, LockType::WrLock, 0, 100).unwrap_err(),
            LockError::Conflict
        );
        // 写锁独占。
        mgr.release_pid(1, 10).unwrap();
        mgr.release_pid(1, 11).unwrap();
        assert!(mgr.lock_with_pid(1, 12, LockType::WrLock, 0, 100).is_ok());
        assert_eq!(
            mgr.lock_with_pid(1, 13, LockType::RdLock, 0, 100).unwrap_err(),
            LockError::Conflict
        );
    }

    #[test]
    fn file_lock_range_non_overlap_ok() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        assert!(mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).is_ok());
        // 不重叠的范围可加写锁。
        assert!(mgr.lock_with_pid(1, 11, LockType::WrLock, 200, 100).is_ok());
        // 重叠范围冲突。
        assert_eq!(
            mgr.lock_with_pid(1, 12, LockType::WrLock, 50, 100).unwrap_err(),
            LockError::Conflict
        );
    }

    #[test]
    fn file_lock_test_lock() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        assert!(mgr.test_lock(1, 0, LockType::RdLock, 0, 100).unwrap().is_none());
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        let info = mgr.test_lock(1, 0, LockType::RdLock, 0, 100).unwrap().unwrap();
        assert_eq!(info.pid, 10);
        assert_eq!(info.lock_type, LockType::WrLock);
        assert_eq!(info.start, 0);
        assert_eq!(info.len, 100);
        // 无冲突范围返回 None。
        assert!(mgr.test_lock(1, 0, LockType::RdLock, 200, 50).unwrap().is_none());
    }

    #[test]
    fn file_lock_unlock() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        assert_eq!(mgr.list_locks(1).unwrap().len(), 1);
        mgr.unlock(1, 10, 0, 100).unwrap();
        assert_eq!(mgr.list_locks(1).unwrap().len(), 0);
    }

    #[test]
    fn file_lock_mandatory() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        mgr.set_mandatory(1, true).unwrap();
        assert!(mgr.is_mandatory(1).unwrap());
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        // 强制锁下，另一个写锁仍冲突。
        assert_eq!(
            mgr.lock_with_pid(1, 11, LockType::WrLock, 0, 100).unwrap_err(),
            LockError::Conflict
        );
    }

    #[test]
    fn file_lock_timeout() {
        let mgr = Arc::new(FileLockManager::new());
        mgr.register_fd(1).unwrap();
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        let mgr2 = mgr.clone();
        let handle = thread::spawn(move || {
            // 尝试加冲突锁，短超时应返回 Timeout。
            mgr2.lock_with_timeout(1, 11, LockType::WrLock, 0, 100, Duration::from_millis(20))
        });
        let res = handle.join().unwrap();
        assert_eq!(res.unwrap_err(), LockError::Timeout);
    }

    #[test]
    fn file_lock_release_fd() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        mgr.register_fd(2).unwrap();
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        mgr.lock_with_pid(2, 11, LockType::RdLock, 0, 50).unwrap();
        assert_eq!(mgr.fd_count(), 2);
        mgr.release_fd(1).unwrap();
        assert_eq!(mgr.fd_count(), 1);
        assert_eq!(mgr.list_locks(1).unwrap_err(), LockError::NoFd);
    }

    #[test]
    fn file_lock_release_pid() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).unwrap();
        mgr.lock_with_pid(1, 10, LockType::RdLock, 200, 50).unwrap();
        mgr.lock_with_pid(1, 11, LockType::RdLock, 0, 50).unwrap();
        let released = mgr.release_pid(1, 10).unwrap();
        assert_eq!(released, 2);
        let locks = mgr.list_locks(1).unwrap();
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].pid, 11);
    }

    #[test]
    fn file_lock_invalid_len_zero() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        assert_eq!(
            mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 0).unwrap_err(),
            LockError::Invalid
        );
    }

    #[test]
    fn file_lock_no_fd() {
        let mgr = FileLockManager::new();
        assert_eq!(
            mgr.lock_with_pid(99, 10, LockType::WrLock, 0, 100).unwrap_err(),
            LockError::NoFd
        );
        assert_eq!(
            mgr.test_lock(99, 10, LockType::WrLock, 0, 100).unwrap_err(),
            LockError::NoFd
        );
    }

    // ----- lease 并发控制测试 -----

    #[test]
    fn file_lock_leased_single_writer() {
        let mgr = FileLockManagerLeased::new("filelock/test-lease");
        mgr.register_fd(1).unwrap();
        assert!(mgr.acquire_lock_lease(101).is_ok());
        let err = mgr.acquire_lock_lease(102).unwrap_err();
        assert_eq!(err.holder, crate::exec::101);
        assert!(mgr.release_lock_lease(101));
        assert!(mgr.acquire_lock_lease(102).is_ok());
        assert!(mgr.release_lock_lease(102));
    }

    #[test]
    fn file_lock_leased_lock_unlock_commit_epoch() {
        let mgr = FileLockManagerLeased::new("filelock/test-lease2");
        mgr.register_fd(1).unwrap();
        let ep0 = mgr.epoch();
        let ep1 = mgr.acquire_lock_lease(201).unwrap();
        assert_eq!(ep1, ep0);
        let new_ep = mgr.lock_under_lease(201, ep0, 1, LockType::WrLock, 0, 100, 201).unwrap();
        assert_eq!(new_ep, ep0 + 1);
        let new_ep2 = mgr.unlock_under_lease(201, new_ep, 1, 201, 0, 100).unwrap();
        assert_eq!(new_ep2, new_ep + 1);
        assert!(mgr.release_lock_lease(201));
    }

    #[test]
    fn file_lock_leased_stale_epoch_rejected() {
        let mgr = FileLockManagerLeased::new("filelock/test-lease3");
        mgr.register_fd(1).unwrap();
        let ep = mgr.acquire_lock_lease(301).unwrap();
        let _ = mgr.lock_under_lease(301, ep, 1, LockType::WrLock, 0, 100, 301).unwrap();
        // 再用旧 epoch 加锁应被拒绝。
        let ep2 = mgr.acquire_lock_lease(301).unwrap();
        let err = mgr.lock_under_lease(301, ep, 1, LockType::WrLock, 0, 100, 301).unwrap_err();
        assert!(matches!(err, LockError::Internal(_)));
        let _ = mgr.release_lock_lease(301);
    }

    #[test]
    fn file_lock_leased_force_unlock_all() {
        let mgr = FileLockManagerLeased::new("filelock/test-lease4");
        let ep0 = mgr.epoch();
        let ep1 = mgr.force_unlock_all(401);
        assert!(ep1 > ep0);
    }

    // ----- caps 权限测试 -----

    #[test]
    fn file_lock_caps_admin_required() {
        let mgr = FileLockManager::new();
        let caps_mgr = FileLockManagerCaps::new(mgr);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.register_fd(1).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.register_fd(1).is_ok());
    }

    #[test]
    fn file_lock_caps_admin_required_for_lock() {
        let mgr = FileLockManager::new();
        mgr.register_fd(1).unwrap();
        let caps_mgr = FileLockManagerCaps::new(mgr);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.lock_with_pid(1, 10, LockType::WrLock, 0, 100).is_ok());
    }

    // ----- sense/act 通道测试 -----

    #[test]
    fn file_lock_probe_channel() {
        let snap = probe_file_lock("filelock/ram0").unwrap();
        assert!(snap.contains("filelock/ram0"));
        assert!(snap.contains("epoch="));
        assert!(snap.contains("ops="));
    }

    #[test]
    fn file_lock_act_channel_status_epoch() {
        let r = act_file_lock("filelock/ram0", "status").unwrap().unwrap();
        assert!(r.contains("status=active"));
        let r = act_file_lock("filelock/ram0", "epoch").unwrap().unwrap();
        assert!(r.contains("epoch="));
    }
}

