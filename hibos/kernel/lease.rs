//! 通道租约 + 世界版本 (epoch) —— AI 原生的并发模型。
//!
//! Linux 用「锁」保护共享内存，因为它不知道数据是什么意思。
//! 我们知道：每个通道都是一个语义实体。所以并发可以更聪明：
//!
//! 1. **单写者租约**：同一时刻只有一个意识主体能更改某条通道（避免写冲突）。
//! 2. **快照 + epoch**：读者拿到的不是裸数据，而是「第 N 版世界观」。
//! 3. **乐观提交**：`commit(channel, expected_epoch, data)`，epoch 不符就失败，
//!    调用方重新 `sense` 再决定 —— 也就是用 CAS 取代悲观锁。
//! 4. **提交即事件**：`commit` 成功后自动 `wake(channel)`，
//!    所有在 `block_on(channel)` 上等待的意识主体被唤醒。
//!
//! 这三件事合起来，就是「并发 = 通道的并发」：没有全局锁，
//! 只有「谁在写哪条通道、世界是第几版、下一版谁来写」。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::exec::{self, ExecId};
use crate::scheduler::CpuPriority;

/// 世界版本号。
pub type Epoch = u64;

/// 某条通道在某个 epoch 的只读快照。
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub channel: String,
    pub epoch: Epoch,
    pub seq: u64,
    pub data: Arc<str>,
    pub writer: Option<ExecId>,
}

/// 写租约。
#[derive(Clone, Debug)]
pub struct Lease {
    pub owner: ExecId,
    pub epoch: Epoch,
    pub expires_at: Option<Instant>,
    pub renewals: u64,
}

impl Lease {
    fn expired(&self, now: Instant) -> bool {
        matches!(self.expires_at, Some(t) if now >= t)
    }
}

/// 租约冲突：告诉你谁占着，方便做优先级继承。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseConflict {
    pub channel: String,
    pub holder: ExecId,
    pub epoch: Epoch,
}

impl std::fmt::Display for LeaseConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "通道 {} 已被 exec={} 占用 (epoch={})",
            self.channel, self.holder, self.epoch
        )
    }
}

/// 提交失败原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// 没有持租约。
    NotLeaseOwner { holder: Option<ExecId> },
    /// 世界已经变了。
    StaleEpoch { expected: Epoch, actual: Epoch },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitError::NotLeaseOwner { holder } => {
                write!(f, "没有该通道的写租约 (当前持有者: {:?})", holder)
            }
            CommitError::StaleEpoch { expected, actual } => write!(
                f,
                "epoch 已过期: 期望 {} 实际 {} —— 需要重新 sense",
                expected, actual
            ),
        }
    }
}

struct SlotState {
    epoch: Epoch,
    seq: u64,
    lease: Option<Lease>,
    snapshot: Arc<Snapshot>,
    reads: u64,
    writes: u64,
    stale_rejects: u64,
    conflicts: u64,
}

/// 一条通道的并发控制单元。
pub struct Slot {
    channel: String,
    state: Mutex<SlotState>,
}

impl Slot {
    fn new(channel: &str) -> Self {
        let snap = Arc::new(Snapshot {
            channel: channel.to_string(),
            epoch: 0,
            seq: 0,
            data: Arc::from(""),
            writer: None,
        });
        Self {
            channel: channel.to_string(),
            state: Mutex::new(SlotState {
                epoch: 0,
                seq: 0,
                lease: None,
                snapshot: snap,
                reads: 0,
                writes: 0,
                stale_rejects: 0,
                conflicts: 0,
            }),
        }
    }

    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// 当前世界版本。
    pub fn epoch(&self) -> Epoch {
        self.state.lock().unwrap().epoch
    }

    /// 读快照（短锁 + Arc 克隆，读者永远不会被写者长阻塞）。
    pub fn snapshot(&self) -> Arc<Snapshot> {
        let mut g = self.state.lock().unwrap();
        g.reads += 1;
        g.snapshot.clone()
    }

    /// 当前租约持有者。
    pub fn holder(&self) -> Option<ExecId> {
        let g = self.state.lock().unwrap();
        match &g.lease {
            Some(l) if !l.expired(Instant::now()) => Some(l.owner),
            _ => None,
        }
    }

    fn acquire(&self, owner: ExecId, ttl: Option<Duration>) -> Result<Epoch, LeaseConflict> {
        let now = Instant::now();
        let mut g = self.state.lock().unwrap();
        if let Some(l) = &g.lease {
            let alive = !l.expired(now);
            if alive && l.owner != owner {
                let holder = l.owner;
                let epoch = g.epoch;
                g.conflicts += 1;
                return Err(LeaseConflict {
                    channel: self.channel.clone(),
                    holder,
                    epoch,
                });
            }
            if alive && l.owner == owner {
                let renewals = l.renewals + 1;
                let epoch = g.epoch;
                g.lease = Some(Lease {
                    owner,
                    epoch,
                    expires_at: ttl.map(|d| now + d),
                    renewals,
                });
                return Ok(epoch);
            }
        }
        let epoch = g.epoch;
        g.lease = Some(Lease {
            owner,
            epoch,
            expires_at: ttl.map(|d| now + d),
            renewals: 0,
        });
        Ok(epoch)
    }

    fn release(&self, owner: ExecId) -> bool {
        let mut g = self.state.lock().unwrap();
        match &g.lease {
            Some(l) if l.owner == owner => {
                g.lease = None;
                true
            }
            _ => false,
        }
    }

    fn commit(&self, owner: ExecId, expected: Epoch, data: &str) -> Result<Epoch, CommitError> {
        let mut g = self.state.lock().unwrap();
        match &g.lease {
            Some(l) if l.owner == owner => {}
            other => {
                return Err(CommitError::NotLeaseOwner {
                    holder: other.as_ref().map(|l| l.owner),
                })
            }
        }
        if g.epoch != expected {
            let actual = g.epoch;
            g.stale_rejects += 1;
            return Err(CommitError::StaleEpoch { expected, actual });
        }
        g.epoch += 1;
        g.seq += 1;
        let epoch = g.epoch;
        g.snapshot = Arc::new(Snapshot {
            channel: self.channel.clone(),
            epoch,
            seq: g.seq,
            data: Arc::from(data),
            writer: Some(owner),
        });
        g.writes += 1;
        g.lease = None;
        Ok(epoch)
    }

    fn force(&self, data: &str) -> Epoch {
        let mut g = self.state.lock().unwrap();
        g.epoch += 1;
        g.seq += 1;
        let epoch = g.epoch;
        g.snapshot = Arc::new(Snapshot {
            channel: self.channel.clone(),
            epoch,
            seq: g.seq,
            data: Arc::from(data),
            writer: None,
        });
        g.writes += 1;
        g.lease = None;
        epoch
    }

    fn stats(&self) -> (Epoch, u64, u64, u64, u64, Option<ExecId>) {
        let g = self.state.lock().unwrap();
        let holder = match &g.lease {
            Some(l) if !l.expired(Instant::now()) => Some(l.owner),
            _ => None,
        };
        (
            g.epoch,
            g.reads,
            g.writes,
            g.stale_rejects,
            g.conflicts,
            holder,
        )
    }
}

static SLOTS: OnceLock<Mutex<BTreeMap<String, Arc<Slot>>>> = OnceLock::new();

fn slots() -> &'static Mutex<BTreeMap<String, Arc<Slot>>> {
    SLOTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// 取得（或惰性创建）通道的并发控制单元。
pub fn slot(channel: &str) -> Arc<Slot> {
    let mut m = slots().lock().unwrap();
    m.entry(channel.to_string())
        .or_insert_with(|| Arc::new(Slot::new(channel)))
        .clone()
}

/// 读快照。
pub fn snapshot(channel: &str) -> Arc<Snapshot> {
    slot(channel).snapshot()
}

/// 当前世界版本。
pub fn epoch(channel: &str) -> Epoch {
    slot(channel).epoch()
}

/// 申请写租约。`ttl=None` 表示无限（由 release/commit 释放）。
pub fn acquire(channel: &str, owner: ExecId, ttl: Option<Duration>) -> Result<Epoch, LeaseConflict> {
    slot(channel).acquire(owner, ttl)
}

/// 申请写租约；冲突时对持有者做**优先级继承**。
///
/// 这是调度反转的解法：高优先级的意识主体想要写某条通道，
/// 而低优先级主体正占着它 —— 那就把占着的人的优先级抬上来，
/// 让它尽快写完释放，而不是让高优先级一直等。
pub fn acquire_with_priority(
    channel: &str,
    owner: ExecId,
    priority: CpuPriority,
    ttl: Option<Duration>,
) -> Result<Epoch, LeaseConflict> {
    match acquire(channel, owner, ttl) {
        Ok(ep) => Ok(ep),
        Err(conflict) => {
            exec::boost_priority(conflict.holder, priority);
            Err(conflict)
        }
    }
}

/// 释放租约。
pub fn release(channel: &str, owner: ExecId) -> bool {
    slot(channel).release(owner)
}

/// 乐观提交：world 版本必须是 `expected`。
///
/// 成功后自增 epoch、写入快照，并 `wake(channel)` 唤醒所有等待者。
pub fn commit(channel: &str, owner: ExecId, expected: Epoch, data: &str) -> Result<Epoch, CommitError> {
    let slot = slot(channel);
    let epoch = slot.commit(owner, expected, data)?;
    // 提交即事件：唤醒所有在这条通道上等待的意识主体。
    exec::wake(channel);
    exec::wake(&format!("lease://wait/{}", channel));
    Ok(epoch)
}

/// 强制写入（治理域 / root 路径，绕过租约但会推进 epoch）。
pub fn force(channel: &str, data: &str) -> Epoch {
    let slot = slot(channel);
    let epoch = slot.force(data);
    exec::wake(channel);
    exec::wake(&format!("lease://wait/{}", channel));
    epoch
}

/// 在这条通道的下一版上等待（事件驱动，不轮询）。
pub fn wait_next(channel: &str, timeout: Option<Duration>) -> Option<Arc<Snapshot>> {
    let before = epoch(channel);
    let reason = exec::block_on(&format!("lease://wait/{}", channel), timeout);
    if reason == exec::WakeReason::Timeout {
        return None;
    }
    let snap = snapshot(channel);
    if snap.epoch > before {
        Some(snap)
    } else {
        None
    }
}

/// 汇总状态（供 `sense lease://channels`）。
pub fn status() -> String {
    let snapshot_map: Vec<Arc<Slot>> = {
        let m = slots().lock().unwrap();
        m.values().cloned().collect()
    };
    if snapshot_map.is_empty() {
        return "（无租约通道）".to_string();
    }
    let mut out = String::new();
    for s in snapshot_map {
        let (ep, reads, writes, stale, conflicts, holder) = s.stats();
        out.push_str(&format!(
            "{:<32} epoch={:<4} reads={:<4} writes={:<4} stale={:<3} conflicts={:<3} holder={:?}\n",
            s.channel(),
            ep,
            reads,
            writes,
            stale,
            conflicts,
            holder
        ));
    }
    out.trim_end().to_string()
}

/// 单通道详情。
pub fn channel_status(channel: &str) -> String {
    let s = slot(channel);
    let (ep, reads, writes, stale, conflicts, holder) = s.stats();
    let snap = s.snapshot();
    format!(
        "channel={} epoch={} seq={} reads={} writes={} stale={} conflicts={} holder={:?} data={:?}",
        channel, ep, snap.seq, reads, writes, stale, conflicts, holder, &*snap.data
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_writer_enforced() {
        let ch = "unittest://lease/single-writer";
        let a = 101u64;
        let b = 102u64;
        assert!(acquire(ch, a, None).is_ok());
        let err = acquire(ch, b, None).unwrap_err();
        assert_eq!(err.holder, a);
        assert!(release(ch, a));
        assert!(acquire(ch, b, None).is_ok());
        assert!(release(ch, b));
    }

    #[test]
    fn stale_commit_is_rejected() {
        let ch = "unittest://lease/stale";
        let owner = 201u64;
        let ep = acquire(ch, owner, None).unwrap();
        assert_eq!(ep, 0);
        let new_ep = commit(ch, owner, ep, "v1").unwrap();
        assert_eq!(new_ep, 1);

        // 再拿一次租约，但用旧 epoch 提交
        let ep2 = acquire(ch, owner, None).unwrap();
        assert_eq!(ep2, 1);
        let err = commit(ch, owner, 0, "v2").unwrap_err();
        assert_eq!(
            err,
            CommitError::StaleEpoch {
                expected: 0,
                actual: 1
            }
        );
        let _ = release(ch, owner);
        assert!(status().contains(ch));
    }

    #[test]
    fn commit_without_lease_is_rejected() {
        let ch = "unittest://lease/no-lease";
        let err = commit(ch, 301, 0, "x").unwrap_err();
        assert_eq!(err, CommitError::NotLeaseOwner { holder: None });
    }

    #[test]
    fn snapshot_carries_writer_and_epoch() {
        let ch = "unittest://lease/snapshot";
        let owner = 401u64;
        let ep = acquire(ch, owner, None).unwrap();
        commit(ch, owner, ep, "hello").unwrap();
        let snap = snapshot(ch);
        assert_eq!(snap.epoch, 1);
        assert_eq!(&*snap.data, "hello");
        assert_eq!(snap.writer, Some(owner));
        assert_eq!(epoch(ch), 1);
    }

    #[test]
    fn ttl_expiry_frees_the_channel() {
        let ch = "unittest://lease/ttl";
        assert!(acquire(ch, 501, Some(Duration::from_millis(10))).is_ok());
        assert_eq!(slot(ch).holder(), Some(501));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(slot(ch).holder(), None);
        // 过期后别人可以拿
        assert!(acquire(ch, 502, None).is_ok());
        let _ = release(ch, 502);
    }

    #[test]
    fn force_bumps_epoch_without_lease() {
        let ch = "unittest://lease/force";
        let ep = force(ch, "root-write");
        assert_eq!(ep, 1);
        assert_eq!(&*snapshot(ch).data, "root-write");
    }

    #[test]
    fn commit_wakes_waiter() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let ch = "unittest://lease/wake";
        // 先把 epoch 推到 1，确保 wait_next 的 before 稳定。
        force(ch, "init");

        let hit = Arc::new(AtomicBool::new(false));
        let hit2 = hit.clone();
        exec::spawn("lease-waiter", CpuPriority::High, move || {
            if let Some(s) = wait_next(ch, Some(Duration::from_secs(2))) {
                if s.epoch >= 2 {
                    hit2.store(true, Ordering::SeqCst);
                }
            }
        });
        std::thread::sleep(Duration::from_millis(30));
        force(ch, "second");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !hit.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(hit.load(Ordering::SeqCst), "提交应唤醒等待者");
    }
}
