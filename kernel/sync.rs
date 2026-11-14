//! 同步原语 —— AI 原生「协作通道」的真实实现。
//!
//! 与 `ipc.rs` 的区别：`ipc.rs` 是**可观测的记账**（信号量到 0 就返回错误），
//! 本模块是**真会阻塞的同步对象**，跑在真 OS 线程上。
//!
//! 三种原语对应三种 AI 原生语义：
//!
//! * [`Semaphore`]   —— 配额：多少个子意识主体可以同时用某条昂贵通道。
//! * [`Barrier`]     —— **认知屏障**：N 个子主体在行动前对齐到同一个世界版本(epoch)。
//! * [`PriorityMutex`] —— 通道租约的本地互斥，带**优先级继承**，
//!   避免「高优先级意识主体等低优先级持有者」造成的调度反转。

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::exec::{self, ExecId};
use crate::scheduler::CpuPriority;

/// 给 `exec` 打一个「这个线程正阻塞在同步对象上」的观察标记。
fn note_block(key: &str) {
    // 观察用：不参与真实的等待/唤醒（唤醒由本模块的 Condvar 负责）。
    exec::note_blocked(key);
}

fn note_run() {
    exec::note_running();
}

/// 计数信号量（真阻塞，FIFO 唤醒）。
pub struct Semaphore {
    id: u64,
    state: Mutex<SemState>,
    cv: Condvar,
}

struct SemState {
    count: usize,
    waiters: VecDeque<u64>,
    next_ticket: u64,
    acquires: u64,
    releases: u64,
    timeouts: u64,
}

static SEM_SEQ: AtomicU64 = AtomicU64::new(1);
static SEM_CREATED: AtomicU64 = AtomicU64::new(0);

impl Semaphore {
    pub fn new(count: usize) -> Self {
        SEM_CREATED.fetch_add(1, Ordering::Relaxed);
        Self {
            id: SEM_SEQ.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(SemState {
                count,
                waiters: VecDeque::new(),
                next_ticket: 1,
                acquires: 0,
                releases: 0,
                timeouts: 0,
            }),
            cv: Condvar::new(),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn value(&self) -> usize {
        self.state.lock().unwrap().count
    }

    /// 尝试不阻塞地拿一个配额。
    pub fn try_acquire(&self) -> bool {
        let mut g = self.state.lock().unwrap();
        if g.count > 0 && g.waiters.is_empty() {
            g.count -= 1;
            g.acquires += 1;
            true
        } else {
            false
        }
    }

    /// 拿一个配额；`None` 表示无限等待。返回是否成功。
    pub fn acquire(&self, timeout: Option<Duration>) -> bool {
        if self.try_acquire() {
            return true;
        }
        let key = format!("sem/{}", self.id);
        note_block(&key);
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self.state.lock().unwrap();
        let ticket = g.next_ticket;
        g.next_ticket += 1;
        g.waiters.push_back(ticket);

        let mut ok = false;
        loop {
            // 只剩自己排队 + 有配额 => 轮到我了
            if g.count > 0 && g.waiters.front() == Some(&ticket) {
                g.count -= 1;
                g.acquires += 1;
                g.waiters.pop_front();
                ok = true;
                break;
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        g.timeouts += 1;
                        g.waiters.retain(|t| *t != ticket);
                        break;
                    }
                    let (ng, res) = self.cv.wait_timeout(g, d - now).unwrap();
                    g = ng;
                    if res.timed_out() {
                        g.timeouts += 1;
                        g.waiters.retain(|t| *t != ticket);
                        break;
                    }
                }
                None => {
                    g = self.cv.wait(g).unwrap();
                }
            }
        }
        drop(g);
        note_run();
        ok
    }

    /// 归还一个配额，并向等待该信号量的调度域令牌发 wake_key。
    pub fn release(&self) {
        let key = format!("sem/{}", self.id);
        let mut g = self.state.lock().unwrap();
        g.count += 1;
        g.releases += 1;
        drop(g);
        self.cv.notify_all();
        // 通道事件即调度事件：信号量释放唤醒等待该键的令牌。
        exec::signal(&key);
    }

    pub fn status(&self) -> String {
        let g = self.state.lock().unwrap();
        format!(
            "sem#{} value={} waiters={} acquires={} releases={} timeouts={}",
            self.id,
            g.count,
            g.waiters.len(),
            g.acquires,
            g.releases,
            g.timeouts
        )
    }
}

/// 认知屏障：N 个执行体对齐到同一个 epoch 后才能一起继续。
///
/// 与 `pthread_barrier` 的差别：返回的 epoch 就是「世界版本号」。
/// 子意识主体拿到不同的 epoch，就说明有人在自己思考期间改了世界，
/// 必须重新感知 —— 这是 AI 原生的一致性，而不是执行进度对齐。
pub struct Barrier {
    id: u64,
    n: usize,
    state: Mutex<BarrierState>,
    cv: Condvar,
}

struct BarrierState {
    arrived: usize,
    epoch: u64,
    generation: u64,
    trips: u64,
    timeouts: u64,
}

static BARRIER_SEQ: AtomicU64 = AtomicU64::new(1);
static BARRIER_CREATED: AtomicU64 = AtomicU64::new(0);

impl Barrier {
    pub fn new(n: usize) -> Self {
        BARRIER_CREATED.fetch_add(1, Ordering::Relaxed);
        Self {
            id: BARRIER_SEQ.fetch_add(1, Ordering::Relaxed),
            n: n.max(1),
            state: Mutex::new(BarrierState {
                arrived: 0,
                epoch: 0,
                generation: 0,
                trips: 0,
                timeouts: 0,
            }),
            cv: Condvar::new(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.state.lock().unwrap().epoch
    }

    pub fn arrived(&self) -> usize {
        self.state.lock().unwrap().arrived
    }

    /// 到达并等待其余同伴。返回对齐后的 epoch。
    ///
    /// `timeout` 到期仍不对齐时返回 `None`（调用方应视为"世界观不一致"）。
    pub fn arrive_and_wait(&self, timeout: Option<Duration>) -> Option<u64> {
        let key = format!("barrier/{}", self.id);
        note_block(&key);
        let deadline = timeout.map(|d| Instant::now() + d);
        let mut g = self.state.lock().unwrap();
        let my_gen = g.generation;
        g.arrived += 1;
        let last = g.arrived == self.n;

        if last {
            g.arrived = 0;
            g.generation += 1;
            g.epoch += 1;
            g.trips += 1;
            let ep = g.epoch;
            let key = format!("barrier/{}", self.id);
            drop(g);
            self.cv.notify_all();
            // 屏障到达即通道事件：唤醒等待该屏障的调度域令牌。
            exec::signal(&key);
            note_run();
            return Some(ep);
        }

        let mut result = None;
        loop {
            if g.generation != my_gen {
                result = Some(g.epoch);
                break;
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        g.timeouts += 1;
                        g.arrived = g.arrived.saturating_sub(1);
                        break;
                    }
                    let (ng, res) = self.cv.wait_timeout(g, d - now).unwrap();
                    g = ng;
                    if res.timed_out() && g.generation == my_gen {
                        g.timeouts += 1;
                        g.arrived = g.arrived.saturating_sub(1);
                        break;
                    }
                }
                None => {
                    g = self.cv.wait(g).unwrap();
                }
            }
        }
        drop(g);
        note_run();
        result
    }

    pub fn status(&self) -> String {
        let g = self.state.lock().unwrap();
        format!(
            "barrier#{} n={} arrived={} epoch={} trips={} timeouts={}",
            self.id, self.n, g.arrived, g.epoch, g.trips, g.timeouts
        )
    }
}

/// 带优先级继承的互斥锁。
pub struct PriorityMutex<T> {
    inner: Mutex<PmState<T>>,
    cv: Condvar,
}

struct PmState<T> {
    data: T,
    /// 持有者身份用 OS 线程 id：裸线程（非执行体）也能正确区分。
    owner: Option<std::thread::ThreadId>,
    owner_exec: Option<ExecId>,
    owner_prio: CpuPriority,
    contention: u64,
    boosts: u64,
}

/// 持锁守卫。drop 时清 owner + 恢复优先级 + 唤醒下一个。
pub struct PmGuard<'a, T> {
    mutex: &'a PriorityMutex<T>,
    thread: std::thread::ThreadId,
    exec: Option<ExecId>,
    boosted: bool,
}

impl<T> PriorityMutex<T> {
    pub fn new(data: T) -> Self {
        Self {
            inner: Mutex::new(PmState {
                data,
                owner: None,
                owner_exec: None,
                owner_prio: CpuPriority::Medium,
                contention: 0,
                boosts: 0,
            }),
            cv: Condvar::new(),
        }
    }

    /// 加锁（阻塞 + 优先级继承）。
    pub fn lock(&self, priority: CpuPriority) -> PmGuard<'_, T> {
        let me_thread = std::thread::current().id();
        let me_exec = exec::current();
        let mut g = self.inner.lock().unwrap();
        let mut boosted = false;

        loop {
            match g.owner {
                None => break,
                Some(o) if o == me_thread => break, // 同线程重入
                Some(_) => {
                    g.contention += 1;
                    if priority > g.owner_prio {
                        g.owner_prio = priority;
                        g.boosts += 1;
                        if let Some(o) = g.owner_exec {
                            exec::boost_priority(o, priority);
                            boosted = true;
                        }
                    }
                    g = self.cv.wait(g).unwrap();
                }
            }
        }

        g.owner = Some(me_thread);
        g.owner_exec = me_exec;
        g.owner_prio = priority;
        drop(g);
        PmGuard {
            mutex: self,
            thread: me_thread,
            exec: me_exec,
            boosted,
        }
    }

    /// 不阻塞尝试。
    pub fn try_lock(&self, priority: CpuPriority) -> Option<PmGuard<'_, T>> {
        let me_thread = std::thread::current().id();
        let me_exec = exec::current();
        let mut g = self.inner.lock().unwrap();
        match g.owner {
            None => {
                g.owner = Some(me_thread);
                g.owner_exec = me_exec;
                g.owner_prio = priority;
                drop(g);
                Some(PmGuard {
                    mutex: self,
                    thread: me_thread,
                    exec: me_exec,
                    boosted: false,
                })
            }
            Some(o) if o == me_thread => {
                drop(g);
                Some(PmGuard {
                    mutex: self,
                    thread: me_thread,
                    exec: me_exec,
                    boosted: false,
                })
            }
            Some(_) => None,
        }
    }

    pub fn status(&self) -> String {
        let g = self.inner.lock().unwrap();
        format!(
            "pmutex held={} prio={} contention={} boosts={}",
            g.owner.is_some(),
            g.owner_prio.name(),
            g.contention,
            g.boosts
        )
    }
}

impl<T> PmGuard<'_, T> {
    /// 在持锁状态下访问受保护数据。
    ///
    /// 用闭包而不是返回 `MutexGuard`：避免「守卫析构要拿内部锁、而返回值还占着内部锁」
    /// 造成的自死锁。
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut g = self.mutex.inner.lock().unwrap();
        f(&mut g.data)
    }
}

impl<T> Drop for PmGuard<'_, T> {
    fn drop(&mut self) {
        let mut g = self.mutex.inner.lock().unwrap();
        if g.owner == Some(self.thread) {
            g.owner = None;
            g.owner_exec = None;
            g.owner_prio = CpuPriority::Medium;
            drop(g);
            if self.boosted {
                if let Some(o) = self.exec {
                    exec::restore_priority(o);
                }
            }
            // 互斥锁释放即通道事件：唤醒等待该锁的调度域令牌。
            let key = format!("pmutex/{:p}", self.mutex);
            exec::signal(&key);
            self.mutex.cv.notify_one();
        }
    }
}

/// 读写锁（std 实现 + 统计包装）。
pub struct RwSync<T> {
    lock: RwLock<T>,
    reads: AtomicU64,
    writes: AtomicU64,
}

impl<T> RwSync<T> {
    pub fn new(data: T) -> Self {
        Self {
            lock: RwLock::new(data),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
        }
    }

    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, T> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.lock.read().unwrap()
    }

    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, T> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        self.lock.write().unwrap()
    }

    pub fn status(&self) -> String {
        format!(
            "rwlock reads={} writes={}",
            self.reads.load(Ordering::Relaxed),
            self.writes.load(Ordering::Relaxed)
        )
    }
}

static NAMED_SEMS: OnceLock<Mutex<BTreeMap<String, Arc<Semaphore>>>> = OnceLock::new();
static NAMED_BARRIERS: OnceLock<Mutex<BTreeMap<String, Arc<Barrier>>>> = OnceLock::new();

/// 按名字复用信号量（跨模块共享同一个配额对象）。
pub fn named_semaphore(name: &str, count: usize) -> Arc<Semaphore> {
    let map = NAMED_SEMS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut m = map.lock().unwrap();
    m.entry(name.to_string())
        .or_insert_with(|| Arc::new(Semaphore::new(count)))
        .clone()
}

/// 按名字复用屏障（同一个名字 = 同一个对齐点）。
pub fn named_barrier(name: &str, n: usize) -> Arc<Barrier> {
    let map = NAMED_BARRIERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut m = map.lock().unwrap();
    m.entry(name.to_string())
        .or_insert_with(|| Arc::new(Barrier::new(n)))
        .clone()
}

/// 同步原语总览（供 `sense sync/primitives`）。
pub fn status() -> String {
    let sems = {
        let map = NAMED_SEMS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let m = map.lock().unwrap();
        m.iter()
            .map(|(k, v)| format!("sem {} => {}", k, v.status()))
            .collect::<Vec<_>>()
    };
    let bars = {
        let map = NAMED_BARRIERS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let m = map.lock().unwrap();
        m.iter()
            .map(|(k, v)| format!("barrier {} => {}", k, v.status()))
            .collect::<Vec<_>>()
    };
    let mut out = format!(
        "semaphores_created={} barriers_created={} named_semaphores={} named_barriers={}",
        SEM_CREATED.load(Ordering::Relaxed),
        BARRIER_CREATED.load(Ordering::Relaxed),
        sems.len(),
        bars.len()
    );
    for s in sems.iter().chain(bars.iter()) {
        out.push('\n');
        out.push_str(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as O};
    use std::sync::Arc;

    #[test]
    fn semaphore_blocks_and_wakes() {
        let sem = Arc::new(Semaphore::new(1));
        assert!(sem.acquire(None));
        let s2 = sem.clone();
        let got = Arc::new(AtomicUsize::new(0));
        let g2 = got.clone();
        let t = std::thread::spawn(move || {
            if s2.acquire(Some(Duration::from_secs(2))) {
                g2.fetch_add(1, O::SeqCst);
                s2.release();
            }
        });
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(got.load(O::SeqCst), 0, "信号量应立即阻塞第二个获取者");
        sem.release();
        t.join().unwrap();
        assert_eq!(got.load(O::SeqCst), 1);
    }

    #[test]
    fn semaphore_timeout() {
        let sem = Semaphore::new(0);
        let began = Instant::now();
        assert!(!sem.acquire(Some(Duration::from_millis(30))));
        assert!(began.elapsed() >= Duration::from_millis(20));
        assert!(sem.status().contains("timeouts=1"));
    }

    #[test]
    fn barrier_aligns_all_and_bumps_epoch() {
        let b = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..3 {
            let b2 = b.clone();
            let s2 = seen.clone();
            handles.push(std::thread::spawn(move || {
                let ep = b2.arrive_and_wait(Some(Duration::from_secs(2))).unwrap();
                s2.lock().unwrap().push(ep);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let v = seen.lock().unwrap().clone();
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|e| *e == v[0]), "所有主体必须看到同一个 epoch");
        assert_eq!(b.epoch(), 1);
    }

    #[test]
    fn barrier_timeout_reports_inconsistency() {
        let b = Barrier::new(2);
        assert_eq!(b.arrive_and_wait(Some(Duration::from_millis(20))), None);
        assert!(b.status().contains("timeouts=1"));
    }

    #[test]
    fn priority_mutex_is_mutually_exclusive() {
        let m = Arc::new(PriorityMutex::new(0usize));
        let m2 = m.clone();
        let t = std::thread::spawn(move || {
            let guard = m2.lock(CpuPriority::High);
            guard.with(|d| *d += 1);
        });
        // 两个线程都做 `+= 1`；互斥保证不会丢失更新，最终必须是 2。
        // （不再假设谁先拿到锁，避免调度顺序导致的偶发失败。）
        {
            let guard = m.lock(CpuPriority::Low);
            guard.with(|d| *d += 1);
        }
        t.join().unwrap();
        let total = m.lock(CpuPriority::Low).with(|d| *d);
        assert_eq!(total, 2);
    }

    #[test]
    fn rwlock_tracks_access() {
        let rw = RwSync::new(5);
        assert_eq!(*rw.read(), 5);
        *rw.write() = 7;
        assert_eq!(*rw.read(), 7);
        assert!(rw.status().contains("reads=2"));
    }
}
