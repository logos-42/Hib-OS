//! 命名互斥锁 —— `sync://mutex/<名>` 的后端。
//!
//! 与 `sync.rs` 的 `PriorityMutex`（线程本地、`ThreadId` owner）不同：
//! * 本模块的锁对象是**通道级**的，跨执行体、跨进程、跨调度域都可见；
//! * 加锁 = `act sync://mutex/<名> lock`，解锁 = `act sync://mutex/<名> unlock`；
//! * 没拿到锁的请求进入 FIFO 等待队列，锁释放时 `signal` 唤醒下一个等待者；
//! * 持有者可通过 `owner` 字段知道是谁占着，做优先级继承的依据。
//!
//! 并发模型与 `lease.rs` 保持一致：单写者、无全局锁、只有「谁在持哪条通道锁」。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::exec::{self, ExecId};

/// 命名互斥锁的状态。
struct MutexState {
    /// 当前持有者（None 表示空闲）。
    holder: Option<ExecId>,
    /// FIFO 等待队列。
    waiters: VecDeque<ExecId>,
    lock_count: u64,
    unlock_count: u64,
    contention_count: u64,
}

/// 命名互斥锁对象。
pub struct NamedMutex {
    name: String,
    state: Mutex<MutexState>,
}

impl NamedMutex {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: Mutex::new(MutexState {
                holder: None,
                waiters: VecDeque::new(),
                lock_count: 0,
                unlock_count: 0,
                contention_count: 0,
            }),
        }
    }

    /// 尝试加锁（不阻塞）。返回 true 表示成功。
    pub fn try_lock(&self, owner: ExecId) -> bool {
        let mut g = self.state.lock().unwrap();
        if g.holder.is_none() {
            g.holder = Some(owner);
            g.lock_count += 1;
            true
        } else if g.holder == Some(owner) {
            // 同一 owner 可重入。
            true
        } else {
            g.contention_count += 1;
            false
        }
    }

    /// 阻塞加锁。`timeout=None` 表示无限等待。
    pub fn lock(&self, owner: ExecId, timeout: Option<Duration>) -> bool {
        if self.try_lock(owner) {
            return true;
        }

        let key = format!("sync://mutex/{}", self.name);
        exec::note_blocked(&key);

        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self.state.lock().unwrap();
        g.waiters.push_back(owner);
        drop(g);

        // 轮询等待：每 20ms 检查一次是否轮到自己，配合 signal 唤醒。
        loop {
            {
                let g = self.state.lock().unwrap();
                if g.holder == Some(owner) {
                    // 已被前任释放并指派给自己。
                    return true;
                }
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        // 超时：从队列移除自己。
                        let mut g = self.state.lock().unwrap();
                        g.waiters.retain(|w| *w != owner);
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                None => {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    /// 解锁。`owner` 必须是当前持有者。
    pub fn unlock(&self, owner: ExecId) -> bool {
        let mut g = self.state.lock().unwrap();
        if g.holder != Some(owner) {
            return false;
        }
        g.holder = None;
        g.unlock_count += 1;

        // 指派给下一个等待者（FIFO）。
        if let Some(next) = g.waiters.pop_front() {
            g.holder = Some(next);
            g.lock_count += 1;
            drop(g);
            // 边沿触发唤醒：下一个持有者可能正在 block_on。
            exec::signal(&format!("sync://mutex/{}", self.name));
            return true;
        }
        drop(g);
        true
    }

    /// 查询当前持有者。
    pub fn holder(&self) -> Option<ExecId> {
        self.state.lock().unwrap().holder
    }

    /// 可观测状态。
    pub fn status(&self) -> String {
        let g = self.state.lock().unwrap();
        format!(
            "mutex={} holder={:?} waiters={} locks={} unlocks={} contention={}",
            self.name,
            g.holder,
            g.waiters.len(),
            g.lock_count,
            g.unlock_count,
            g.contention_count,
        )
    }
}

/// 全局命名互斥锁注册表。
struct Registry {
    map: BTreeMap<String, Arc<NamedMutex>>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            map: BTreeMap::new(),
        })
    })
}

/// 获取或创建一个命名互斥锁。
pub fn get_mutex(name: &str) -> Arc<NamedMutex> {
    let mut reg = registry().lock().unwrap();
    if let Some(m) = reg.map.get(name) {
        return m.clone();
    }
    let m = Arc::new(NamedMutex::new(name));
    reg.map.insert(name.to_string(), m.clone());
    m
}

/// 列出所有命名互斥锁的状态。
pub fn list_all() -> String {
    let reg = registry().lock().unwrap();
    if reg.map.is_empty() {
        return "（无命名互斥锁）".to_string();
    }
    let mut out = String::new();
    for (_, m) in reg.map.iter() {
        out.push_str(&format!("{}\n", m.status()));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn lock_unlock_roundtrip() {
        let m = NamedMutex::new("test-mutex");
        assert!(m.try_lock(1));
        assert!(m.unlock(1));
        assert_eq!(m.holder(), None);
    }

    #[test]
    fn contention_serializes() {
        let m = Arc::new(NamedMutex::new("contention"));
        let flag = Arc::new(AtomicBool::new(false));
        let m2 = m.clone();
        let f2 = flag.clone();
        let t = thread::spawn(move || {
            m2.lock(2, None);
            f2.store(true, Ordering::SeqCst);
            m2.unlock(2);
        });
        // 主线程先拿锁，子线程必须等待。
        m.lock(1, None);
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(!flag.load(Ordering::SeqCst));
        m.unlock(1);
        t.join().unwrap();
        assert!(flag.load(Ordering::SeqCst));
    }

    #[test]
    fn wrong_owner_cannot_unlock() {
        let m = NamedMutex::new("wrong-owner");
        m.try_lock(1);
        assert!(!m.unlock(2));
        assert_eq!(m.holder(), Some(1));
    }

    #[test]
    fn registry_get_and_list() {
        let a = get_mutex("reg-mutex");
        let b = get_mutex("reg-mutex");
        assert!(Arc::ptr_eq(&a, &b));
        let s = list_all();
        assert!(s.contains("reg-mutex"));
    }

    #[test]
    fn timeout_rejected() {
        let m = NamedMutex::new("timeout-mutex");
        m.try_lock(1);
        let ok = m.lock(2, Some(std::time::Duration::from_millis(30)));
        assert!(!ok);
        m.unlock(1);
    }
}
