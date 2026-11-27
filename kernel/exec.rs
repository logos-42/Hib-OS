//! 执行体子系统 (Execution Bodies) —— AI 原生的「真线程 + 事件驱动唤醒」。
//!
//! 与 Linux 的根本差异：**调度的单位不是时间片，而是通道事件**。
//!
//! * Linux: 每个线程被时间片打断，调度器决定谁上 CPU。
//! * Hib OS: 每个执行体在「等某个通道的某个断言」时主动挂起，通道变更时被唤醒。
//!
//! 因此本模块的核心数据结构是 `wait_table`（通道 → 等待者），
//! 而不是 `ready_queue`。就绪队列只是它的派生物。
//!
//! 说明（诚实边界）：
//! * 这里用的是**真 OS 线程**（`std::thread`），不是模拟线程。
//! * Rust 标准库无法做抢占式上下文切换，所以「抢占」是**协作式**的：
//!   长任务必须在安全点调用 [`checkpoint`]，这也是未来接入用户态
//!   上下文切换（协程/纤程）的挂载点。
//! * 阻塞的解除只有一个来源：`wake(key)`。没有轮询主循环。

use std::cell::Cell;
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::monitor::get_time_ns;
use crate::scheduler::{self, CpuPriority};
use crate::scheduler_domains::SchedDomain;
use crate::fiber;

/// 执行体标识。
pub type ExecId = u64;

thread_local! {
    /// 当前线程绑定的执行体。
    static CURRENT: Cell<Option<ExecId>> = const { Cell::new(None) };
}

/// 执行体状态机 —— **单一真相源**。
///
/// `Running → Blocked{key} ⇄ Running`；任意活跃态 `→ Suspended{resume_to}`；
/// 任意态 `→ Exited`。阻塞键与「挂起后回到哪里」都编码在状态内部，
/// 恢复时直接读 `resume_to`，不再从散落字段反推。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecState {
    Ready,
    Running,
    /// 阻塞在某个通道键上。
    Blocked { key: String },
    /// 被挂起（睡眠）；`resume_to` 记录恢复后回到的状态。
    Suspended { resume_to: ResumeTo },
    Exited,
}

/// 挂起前记住的「恢复目标」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeTo {
    /// 回到 Running。
    Idle,
    /// 回到 Blocked{key}。
    Blocked(String),
}

impl ExecState {
    pub fn name(&self) -> &'static str {
        match self {
            ExecState::Ready => "ready",
            ExecState::Running => "running",
            ExecState::Blocked { .. } => "blocked",
            ExecState::Suspended { .. } => "suspended",
            ExecState::Exited => "exited",
        }
    }
}

/// 受控迁移事件。所有对 `Body::state` 的写入必须经 [`transition`]。
#[derive(Debug)]
enum ExecEvent {
    /// Running/Ready → Blocked{key}（对同一键重复阻塞幂等）。
    Block { key: String },
    /// Blocked → Running（已 Running 时幂等）。
    Unblock,
    /// 活跃态 → Suspended{resume_to}（已挂起时幂等，保留最初的 resume_to）。
    Suspend,
    /// Suspended → 恢复到 `resume_to` 记忆的原状态。
    Resume,
    /// 任意态 → Exited。
    Exit,
}

/// 集中状态迁移：非法迁移被拒绝（返回 `Err` 且**不修改状态**）。
fn transition(b: &mut Body, event: ExecEvent) -> Result<(), String> {
    match (&b.state, event) {
        (ExecState::Ready | ExecState::Running, ExecEvent::Block { key }) => {
            b.state = ExecState::Blocked { key };
        }
        (ExecState::Blocked { key: old }, ExecEvent::Block { key }) if *old == key => {}
        (ExecState::Blocked { .. } | ExecState::Running, ExecEvent::Unblock) => {
            b.state = ExecState::Running;
        }
        (ExecState::Ready | ExecState::Running, ExecEvent::Suspend) => {
            b.state = ExecState::Suspended {
                resume_to: ResumeTo::Idle,
            };
        }
        (ExecState::Blocked { key }, ExecEvent::Suspend) => {
            b.state = ExecState::Suspended {
                resume_to: ResumeTo::Blocked(key.clone()),
            };
        }
        (ExecState::Suspended { .. }, ExecEvent::Suspend) => {}
        (ExecState::Suspended { resume_to }, ExecEvent::Resume) => {
            b.state = match resume_to {
                ResumeTo::Idle => ExecState::Running,
                ResumeTo::Blocked(key) => ExecState::Blocked { key: key.clone() },
            };
        }
        (_, ExecEvent::Exit) => b.state = ExecState::Exited,
        (from, event) => {
            return Err(format!(
                "exec: 非法迁移 {} --{:?}-->（已拒绝）",
                from.name(),
                event
            ));
        }
    }
    Ok(())
}

/// 一次阻塞的结束原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    /// 被 `wake(key)` 命中。
    Signaled,
    /// 超时。
    Timeout,
    /// 等待期间被整体挂起（睡眠）。
    Suspended,
    /// 当前线程不是执行体（裸线程调用）。
    NotCurrent,
}

impl WakeReason {
    pub fn name(self) -> &'static str {
        match self {
            WakeReason::Signaled => "signaled",
            WakeReason::Timeout => "timeout",
            WakeReason::Suspended => "suspended",
            WakeReason::NotCurrent => "not-current",
        }
    }
}

/// 一个执行体。
#[derive(Clone, Debug)]
pub struct Body {
    pub id: ExecId,
    pub name: String,
    pub state: ExecState,
    /// 当前有效优先级（可能被优先级继承临时抬升）。
    pub priority: CpuPriority,
    /// 基础优先级（继承结束后恢复到这个）。
    pub base_priority: CpuPriority,
    /// 父执行体（子意识主体的来源）。
    pub parent: Option<ExecId>,
    /// 所属调度域（None 表示全局/默认域，不归属任何专用 MLFQ）。
    pub domain: Option<SchedDomain>,
    pub created_ns: u128,
    pub exit_ns: Option<u128>,
    pub exit_code: Option<i32>,
    /// 累计执行时间（实际是挂钟存活时间，见文档说明）。
    pub cpu_ns: u64,
    pub wakeups: u64,
    pub checkpoints: u64,
    /// 被 `wake`/`signal` 置位、但尚未被等待者消费的边沿闩锁。
    /// （这是事件闩锁机制，不是状态机的一部分——状态只看 `state`。）
    pub signaled: bool,
    /// 优先级被继承抬升过。
    pub inherited: bool,
}

impl Body {
    /// 当前正在等待（或挂起前等待）的通道键。
    pub fn wait_key(&self) -> Option<&str> {
        match &self.state {
            ExecState::Blocked { key } => Some(key),
            ExecState::Suspended {
                resume_to: ResumeTo::Blocked(key),
            } => Some(key),
            _ => None,
        }
    }
}

/// 执行体快照（供 `sense exec/threads`）。
#[derive(Clone, Debug, Default)]
pub struct ExecSnapshot {
    pub total: usize,
    pub running: usize,
    pub ready: usize,
    pub blocked: usize,
    pub suspended: usize,
    pub exited: usize,
    pub waiters: usize,
    pub wakeups: u64,
    pub checkpoints: u64,
}

struct Inner {
    bodies: BTreeMap<ExecId, Body>,
    wait_table: BTreeMap<String, Vec<ExecId>>,
    /// 边沿触发闩锁：`signal()` 在无人等待时把令牌存在这里，
    /// 下一次 `block_on` 立刻消费 —— 消除「检查条件 → 注册等待」之间的丢唤醒窗口。
    pending: BTreeMap<String, u64>,
    next_id: ExecId,
    spawned: u64,
    failed_spawns: u64,
}

struct Exec {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl Exec {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                bodies: BTreeMap::new(),
                wait_table: BTreeMap::new(),
                pending: BTreeMap::new(),
                next_id: 1,
                spawned: 0,
                failed_spawns: 0,
            }),
            cv: Condvar::new(),
        }
    }
}

static EXEC: OnceLock<Exec> = OnceLock::new();

fn ex() -> &'static Exec {
    EXEC.get_or_init(Exec::new)
}

/// 当前执行体 id。
pub fn current() -> Option<ExecId> {
    CURRENT.with(|c| c.get())
}

/// 当前执行体名字。
pub fn current_name() -> Option<String> {
    let id = current()?;
    let g = ex().inner.lock().unwrap();
    g.bodies.get(&id).map(|b| b.name.clone())
}

/// 派生一个真执行体（真 OS 线程）。
///
/// 返回的 id 立刻可被 `sense exec/threads` 看到；`f` 结束时状态转 `Exited`，
/// 并唤醒所有等待 `exec/exit/<id>` 的父执行体。
pub fn spawn<F>(name: &str, priority: CpuPriority, f: F) -> ExecId
where
    F: FnOnce() + Send + 'static,
{
    let parent = current();
    let id = {
        let mut g = ex().inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.spawned += 1;
        g.bodies.insert(
            id,
            Body {
                id,
                name: name.to_string(),
                state: ExecState::Running,
                priority,
                base_priority: priority,
                parent,
                domain: None,
                created_ns: get_time_ns() as u128,
                exit_ns: None,
                exit_code: None,
                cpu_ns: 0,
                wakeups: 0,
                checkpoints: 0,
                signaled: false,
                inherited: false,
            },
        );
        id
    };

    let thread_name = format!("hibos-{}-{}", sanitize(name), id);
    let started = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            CURRENT.with(|c| c.set(Some(id)));
            let began = Instant::now();
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(f));
            let code = if outcome.is_ok() { 0 } else { 1 };
            finish(id, began.elapsed(), code);
        });

    if started.is_err() {
        // 线程创建失败：不要留下一个永远 Running 的僵尸。
        let mut g = ex().inner.lock().unwrap();
        g.failed_spawns += 1;
        if let Some(b) = g.bodies.get_mut(&id) {
            let _ = transition(b, ExecEvent::Exit);
            b.exit_ns = Some(get_time_ns() as u128);
            b.exit_code = Some(-1);
        }
        drop(g);
        ex().cv.notify_all();
    }
    id
}

/// 派生一个归属指定调度域的真执行体（真 OS 线程）。
///
/// 与 [`spawn`] 的区别：新执行体会被记入指定调度域的 MLFQ 队列，
/// 该域独立调度，不与全局/其它域共享就绪队列。
pub fn spawn_in_domain<F>(name: &str, priority: CpuPriority, domain: SchedDomain, f: F) -> ExecId
where
    F: FnOnce() + Send + 'static,
{
    let parent = current();
    let id = {
        let mut g = ex().inner.lock().unwrap();
        let id = g.next_id;
        g.next_id += 1;
        g.spawned += 1;
        g.bodies.insert(
            id,
            Body {
                id,
                name: name.to_string(),
                state: ExecState::Running,
                priority,
                base_priority: priority,
                parent,
                domain: Some(domain),
                created_ns: get_time_ns() as u128,
                exit_ns: None,
                exit_code: None,
                cpu_ns: 0,
                wakeups: 0,
                checkpoints: 0,
                signaled: false,
                inherited: false,
            },
        );
        id
    };

    let thread_name = format!("hibos-{}-{}", sanitize(name), id);
    let started = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            CURRENT.with(|c| c.set(Some(id)));
            let began = Instant::now();
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(f));
            let code = if outcome.is_ok() { 0 } else { 1 };
            finish(id, began.elapsed(), code);
        });

    if started.is_err() {
        let mut g = ex().inner.lock().unwrap();
        g.failed_spawns += 1;
        if let Some(b) = g.bodies.get_mut(&id) {
            let _ = transition(b, ExecEvent::Exit);
            b.exit_ns = Some(get_time_ns() as u128);
            b.exit_code = Some(-1);
        }
        drop(g);
        ex().cv.notify_all();
    }
    id
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(24)
        .collect()
}

fn finish(id: ExecId, elapsed: Duration, code: i32) {
    let mut g = ex().inner.lock().unwrap();
    if let Some(b) = g.bodies.get_mut(&id) {
        let _ = transition(b, ExecEvent::Exit);
        b.exit_ns = Some(get_time_ns() as u128);
        b.exit_code = Some(code);
        b.cpu_ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
        b.signaled = false;
    }
    drop(g);
    // 唤醒所有 join 者（它们注册在 exec/exit/<id> 上）。
    ex().cv.notify_all();
    wake(&exit_key(id));
}

/// `join` 使用的等待键。
pub fn exit_key(id: ExecId) -> String {
    format!("exec/exit/{}", id)
}

/// 以通道事件为断言挂起当前执行体。
///
/// `timeout=None` 表示无限等待。返回结束原因。
pub fn block_on(key: &str, timeout: Option<Duration>) -> WakeReason {
    let Some(id) = current() else {
        return WakeReason::NotCurrent;
    };
    if id == 0 {
        return WakeReason::NotCurrent;
    }
    let ex = ex();
    let deadline = timeout.map(|d| Instant::now() + d);
    let mut g = ex.inner.lock().unwrap();

    if !g.bodies.contains_key(&id) {
        return WakeReason::NotCurrent;
    }

    // 边沿触发闩锁：如果在我们注册等待之前已经有人 `signal` 过，立刻消费并返回。
    // 这是「睡眠不丢事件」在竞态窗口上的补丁。
    if let Some(n) = g.pending.get_mut(key) {
        if *n > 0 {
            *n -= 1;
            if *n == 0 {
                g.pending.remove(key);
            }
            if let Some(b) = g.bodies.get_mut(&id) {
                b.wakeups += 1;
                let _ = transition(b, ExecEvent::Unblock);
            }
            return WakeReason::Signaled;
        }
    }

    {
        let b = g.bodies.get_mut(&id).unwrap();
        if transition(b, ExecEvent::Block { key: key.to_string() }).is_err() {
            return WakeReason::NotCurrent;
        }
        b.signaled = false;
    }
    g.wait_table.entry(key.to_string()).or_default().push(id);

    let mut timed_out = false;
    loop {
        let (signaled, suspended, gone) = {
            match g.bodies.get(&id) {
                Some(b) => (
                    b.signaled,
                    matches!(b.state, ExecState::Suspended { .. }),
                    false,
                ),
                None => (false, false, true),
            }
        };
        if gone {
            break;
        }
        // 挂起期间既不吃掉信号，也不退出等待：保持等待者身份，
        // 唤醒后（resume）再消费信号 —— 这就是「睡眠不丢事件」。
        if suspended {
            let (ng, _) = ex
                .cv
                .wait_timeout(g, Duration::from_millis(50))
                .unwrap();
            g = ng;
            continue;
        }
        if signaled {
            break;
        }
        match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    timed_out = true;
                    break;
                }
                let (ng, res) = ex.cv.wait_timeout(g, d - now).unwrap();
                g = ng;
                if res.timed_out() {
                    timed_out = true;
                    break;
                }
            }
            None => {
                g = ex.cv.wait(g).unwrap();
            }
        }
    }

    // 注销等待者。
    let mut emptied = false;
    if let Some(v) = g.wait_table.get_mut(key) {
        v.retain(|x| *x != id);
        emptied = v.is_empty();
    }
    if emptied {
        g.wait_table.remove(key);
    }

    let (suspended, signaled) = match g.bodies.get_mut(&id) {
        Some(b) => {
            let sig = std::mem::take(&mut b.signaled);
            if !matches!(b.state, ExecState::Suspended { .. }) {
                let _ = transition(b, ExecEvent::Unblock);
                if timed_out {
                    b.wakeups += 1;
                }
            }
            (matches!(b.state, ExecState::Suspended { .. }), sig)
        }
        None => (false, false),
    };
    drop(g);

    if suspended {
        WakeReason::Suspended
    } else if signaled {
        WakeReason::Signaled
    } else if timed_out {
        WakeReason::Timeout
    } else {
        WakeReason::Signaled
    }
}

/// 唤醒某个通道键上的全部等待者。返回被唤醒的数量。
///
/// 与 [`signal`] 的区别：`wake` 是电平触发（只影响当前等待者），不闩锁令牌。
/// 但同样会向各调度域发 `wake_key`，保证「通道事件即调度事件」。
pub fn wake(key: &str) -> usize {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    let ids = g.wait_table.get(key).cloned().unwrap_or_default();
    let mut woke = 0;
    let mut involved_domains: Vec<SchedDomain> = Vec::new();
    for id in &ids {
        if let Some(b) = g.bodies.get_mut(id) {
            // 已退出/已挂起的等待者只登记信号，不改变其状态。
            b.signaled = true;
            if matches!(b.state, ExecState::Blocked { .. }) {
                let _ = transition(b, ExecEvent::Unblock);
                b.wakeups += 1;
            }
            woke += 1;
            if let Some(domain) = b.domain {
                if !involved_domains.contains(&domain) {
                    involved_domains.push(domain);
                }
            }
        }
    }
    drop(g);
    if woke > 0 {
        ex.cv.notify_all();
    }
    // 感知驱动抢占：向各涉及调度域发 wake_key，让等待该通道的令牌回到 MLFQ 就绪队列。
    let _ = scheduler::wake_key_global(key);
    for domain in involved_domains {
        let _ = crate::scheduler_domains::wake_key(domain, key);
    }
    // 通道事件即调度事件：若当前运行在纤程上下文中，检查是否该被抢占。
    fiber::check_preemption();
    woke
}

/// 观察用：标记当前执行体阻塞在某个同步对象上。
///
/// 同步原语（`sync.rs`）内部用 Condvar 完成真实等待，这里只负责让
/// `sense exec/threads` 能看到「谁阻塞在哪」——可观测性与机制解耦。
pub fn note_blocked(key: &str) {
    let Some(id) = current() else { return };
    let mut g = ex().inner.lock().unwrap();
    if let Some(b) = g.bodies.get_mut(&id) {
        if matches!(b.state, ExecState::Running | ExecState::Ready) {
            let _ = transition(b, ExecEvent::Block { key: key.to_string() });
        }
    }
}

/// 观察用：标记当前执行体恢复运行。
pub fn note_running() {
    let Some(id) = current() else { return };
    let mut g = ex().inner.lock().unwrap();
    if let Some(b) = g.bodies.get_mut(&id) {
        if matches!(b.state, ExecState::Blocked { .. }) {
            let _ = transition(b, ExecEvent::Unblock);
        }
    }
}

/// 边沿触发信号：有人等待就唤醒；没人等待就把令牌闩锁起来，交给下一个 `block_on`。
///
/// 与 [`wake`] 的区别：`wake` 是电平触发（只影响当前等待者），`signal` 是边沿
/// 触发（事件本身被记住）。邮箱/完成通知这类「一次性事件」必须用 `signal`，
/// 否则会丢在「检查 → 阻塞」的竞态窗口里。
pub fn signal(key: &str) -> usize {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    let ids = g.wait_table.get(key).cloned().unwrap_or_default();
    if ids.is_empty() {
        let e = g.pending.entry(key.to_string()).or_insert(0);
        *e = (*e).saturating_add(1);
        return 0;
    }
    let mut woke = 0;
    let mut involved_domains: Vec<SchedDomain> = Vec::new();
    for id in &ids {
        if let Some(b) = g.bodies.get_mut(id) {
            b.signaled = true;
            if matches!(b.state, ExecState::Blocked { .. }) {
                let _ = transition(b, ExecEvent::Unblock);
            }
            woke += 1;
            if let Some(domain) = b.domain {
                if !involved_domains.contains(&domain) {
                    involved_domains.push(domain);
                }
            }
        }
    }
    drop(g);
    ex.cv.notify_all();
    // 感知驱动抢占：向全局调度器及各涉及调度域发 wake_key，
    // 让等待该通道的令牌回到对应 MLFQ 的就绪队列。
    let _ = scheduler::wake_key_global(key);
    for domain in involved_domains {
        let _ = crate::scheduler_domains::wake_key(domain, key);
    }
    woke
}

/// 协作式安全点：如果整个系统被挂起，就在这里停下来。
///
/// 长任务（例如 LLM 推理循环）必须周期性调用它，否则「睡眠」对它是无效的。
pub fn checkpoint() {
    let Some(id) = current() else { return };
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    loop {
        let suspended = g
            .bodies
            .get(&id)
            .map(|b| matches!(b.state, ExecState::Suspended { .. }))
            .unwrap_or(false);
        if !suspended {
            break;
        }
        let (ng, _) = ex
            .cv
            .wait_timeout(g, Duration::from_millis(50))
            .unwrap();
        g = ng;
    }
    if let Some(b) = g.bodies.get_mut(&id) {
        b.checkpoints += 1;
    }
    // 通道安全点即纤程抢占点：检查是否该被抢占。
    fiber::check_preemption();
}

/// 让出一次执行权（协作式）。
///
/// 目前等价于一次安全点检查 + 一次极短让出；真正的抢占式切换
/// 由纤程调度器的监视线程通过时间片实现。
pub fn yield_now() {
    checkpoint();
    std::thread::yield_now();
}

/// 挂起单个执行体（电源/调试用）。
pub fn suspend(id: ExecId) -> bool {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    match g.bodies.get_mut(&id) {
        Some(b) if !matches!(b.state, ExecState::Exited) => {
            let _ = transition(b, ExecEvent::Suspend);
            drop(g);
            ex.cv.notify_all();
            true
        }
        _ => false,
    }
}

/// 唤醒单个执行体。恢复到挂起时记忆的原状态（`resume_to`），不再反推。
pub fn resume(id: ExecId) -> bool {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    match g.bodies.get_mut(&id) {
        Some(b) if matches!(b.state, ExecState::Suspended { .. }) => {
            let _ = transition(b, ExecEvent::Resume);
            drop(g);
            ex.cv.notify_all();
            true
        }
        _ => false,
    }
}

/// 挂起整个系统（真正的 suspend，而不是改字符串）。
///
/// 返回被挂起的执行体数量。`wait_table` 原样保留 —— 这是「睡眠不丢事件」的关键。
pub fn suspend_all() -> usize {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    let mut n = 0;
    for b in g.bodies.values_mut() {
        if !matches!(b.state, ExecState::Exited) {
            let _ = transition(b, ExecEvent::Suspend);
            n += 1;
        }
    }
    drop(g);
    ex.cv.notify_all();
    n
}

/// 恢复整个系统。
pub fn resume_all() -> usize {
    let ex = ex();
    let mut g = ex.inner.lock().unwrap();
    let mut n = 0;
    for b in g.bodies.values_mut() {
        if matches!(b.state, ExecState::Suspended { .. }) {
            let _ = transition(b, ExecEvent::Resume);
            n += 1;
        }
    }
    drop(g);
    ex.cv.notify_all();
    n
}

/// 等待某个执行体结束，返回退出码。
pub fn join(id: ExecId, timeout: Option<Duration>) -> Option<i32> {
    let began = Instant::now();
    let key = exit_key(id);
    loop {
        {
            let g = ex().inner.lock().unwrap();
            match g.bodies.get(&id) {
                Some(b) if b.state == ExecState::Exited => return b.exit_code,
                Some(_) => {}
                None => return None,
            }
        }
        if let Some(t) = timeout {
            if began.elapsed() >= t {
                return None;
            }
        }
        // 50ms 兜底，防止「检查-注册」之间漏掉一次唤醒。
        block_on(&key, Some(Duration::from_millis(50)));
    }
}

/// 优先级继承：把 `id` 的有效优先级临时抬到 `to`。
pub fn boost_priority(id: ExecId, to: CpuPriority) {
    let mut g = ex().inner.lock().unwrap();
    if let Some(b) = g.bodies.get_mut(&id) {
        if to > b.priority {
            b.priority = to;
            b.inherited = true;
        }
    }
}

/// 继承结束，恢复基础优先级。
pub fn restore_priority(id: ExecId) {
    let mut g = ex().inner.lock().unwrap();
    if let Some(b) = g.bodies.get_mut(&id) {
        if b.inherited {
            b.priority = b.base_priority;
            b.inherited = false;
        }
    }
}

/// 获得执行体快照。
pub fn snapshot() -> ExecSnapshot {
    let g = ex().inner.lock().unwrap();
    let mut s = ExecSnapshot {
        total: g.bodies.len(),
        ..Default::default()
    };
    for b in g.bodies.values() {
        match b.state {
            ExecState::Ready => s.ready += 1,
            ExecState::Running => s.running += 1,
            ExecState::Blocked { .. } => s.blocked += 1,
            ExecState::Suspended { .. } => s.suspended += 1,
            ExecState::Exited => s.exited += 1,
        }
        s.wakeups += b.wakeups;
        s.checkpoints += b.checkpoints;
    }
    s.waiters = g.wait_table.values().map(|v| v.len()).sum();
    s
}

/// 全部执行体（含已退出）。
pub fn bodies() -> Vec<Body> {
    let g = ex().inner.lock().unwrap();
    g.bodies.values().cloned().collect()
}

/// 等待表快照：(键, 等待者 id 列表)。
pub fn wait_table() -> Vec<(String, Vec<ExecId>)> {
    let g = ex().inner.lock().unwrap();
    g.wait_table
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// 回收已退出的执行体记录。
pub fn reap() -> usize {
    let mut g = ex().inner.lock().unwrap();
    let dead: Vec<ExecId> = g
        .bodies
        .iter()
        .filter(|(_, b)| b.state == ExecState::Exited)
        .map(|(id, _)| *id)
        .collect();
    for id in &dead {
        g.bodies.remove(id);
    }
    dead.len()
}

/// 人读状态行（供通道输出）。
pub fn status_line() -> String {
    let s = snapshot();
    format!(
        "bodies={} running={} ready={} blocked={} suspended={} exited={} waiters={} wakeups={} checkpoints={}",
        s.total, s.running, s.ready, s.blocked, s.suspended, s.exited, s.waiters, s.wakeups, s.checkpoints
    )
}

/// 人读执行体表。
pub fn list() -> String {
    let bodies = bodies();
    if bodies.is_empty() {
        return "（无执行体）".to_string();
    }
    let mut out = String::new();
    for b in bodies {
        out.push_str(&format!(
            "exec={:<4} {:<20} {:<10} prio={:<8} wait={:<24} wake={:<4} ckpt={:<4} parent={:?}\n",
            b.id,
            b.name,
            b.state.name(),
            b.priority.name(),
            b.wait_key().unwrap_or("-"),
            b.wakeups,
            b.checkpoints,
            b.parent
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn spawn_runs_and_exits() {
        let flag = Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        let id = spawn("unit-spawn", CpuPriority::Medium, move || {
            f2.store(true, Ordering::SeqCst);
        });
        let code = join(id, Some(Duration::from_secs(2)));
        assert_eq!(code, Some(0));
        assert!(flag.load(Ordering::SeqCst));
        let g = ex().inner.lock().unwrap();
        assert_eq!(g.bodies[&id].state, ExecState::Exited);
    }

    #[test]
    fn block_then_wake_roundtrip() {
        let key = "unittest/exec/wake";
        let hit = Arc::new(AtomicBool::new(false));
        let hit2 = hit.clone();
        let id = spawn("unit-block", CpuPriority::Medium, move || {
            let r = block_on(key, Some(Duration::from_secs(2)));
            if r == WakeReason::Signaled {
                hit2.store(true, Ordering::SeqCst);
            }
        });
        // 等它进入 Blocked 再唤醒，避免竞态。
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let blocked = {
                let g = ex().inner.lock().unwrap();
                g.bodies
                    .get(&id)
                    .map(|b| matches!(b.state, ExecState::Blocked { .. }))
                    .unwrap_or(false)
            };
            if blocked {
                break;
            }
            assert!(Instant::now() < deadline, "执行体未进入 Blocked");
            std::thread::yield_now();
        }
        assert_eq!(wake(key), 1);
        assert_eq!(join(id, Some(Duration::from_secs(2))), Some(0));
        assert!(hit.load(Ordering::SeqCst));
    }

    #[test]
    fn timeout_is_reported() {
        let got = Arc::new(Mutex::new(None));
        let g2 = got.clone();
        let id = spawn("unit-timeout", CpuPriority::Low, move || {
            let r = block_on("unittest/exec/never", Some(Duration::from_millis(20)));
            *g2.lock().unwrap() = Some(r);
        });
        assert_eq!(join(id, Some(Duration::from_secs(2))), Some(0));
        assert_eq!(*got.lock().unwrap(), Some(WakeReason::Timeout));
    }

    #[test]
    fn suspend_keeps_waiter_registered() {
        let key = "unittest/exec/suspend";
        let id = spawn("unit-suspend", CpuPriority::Medium, move || {
            block_on(key, Some(Duration::from_secs(5)));
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            {
                let g = ex().inner.lock().unwrap();
                if g.wait_table.get(key).map(|v| v.contains(&id)).unwrap_or(false) {
                    break;
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(suspend(id));
        {
            let g = ex().inner.lock().unwrap();
            assert!(matches!(
                g.bodies[&id].state,
                ExecState::Suspended { .. }
            ));
            // 挂起不应把等待者从表里删掉
            assert!(g.wait_table.get(key).map(|v| v.contains(&id)).unwrap_or(false));
        }
        assert!(resume(id));
        assert_eq!(wake(key), 1);
        assert_eq!(join(id, Some(Duration::from_secs(2))), Some(0));
    }

    #[test]
    fn priority_inheritance_boosts_and_restores() {
        let id = spawn("unit-prio", CpuPriority::Low, || {
            block_on("unittest/exec/prio", Some(Duration::from_millis(30)));
        });
        boost_priority(id, CpuPriority::Critical);
        {
            let g = ex().inner.lock().unwrap();
            assert_eq!(g.bodies[&id].priority, CpuPriority::Critical);
            assert!(g.bodies[&id].inherited);
        }
        restore_priority(id);
        {
            let g = ex().inner.lock().unwrap();
            assert_eq!(g.bodies[&id].priority, CpuPriority::Low);
            assert!(!g.bodies[&id].inherited);
        }
        let _ = join(id, Some(Duration::from_secs(2)));
    }

    #[test]
    fn snapshot_counts_are_monotonic() {
        let before = snapshot().total;
        let id = spawn("unit-snap", CpuPriority::Low, || {});
        let after = snapshot().total;
        assert!(after > before);
        let _ = join(id, Some(Duration::from_secs(2)));
        assert!(snapshot().exited >= 1);
    }

    #[test]
    fn bare_thread_reports_not_current() {
        let r = std::thread::spawn(|| block_on("unittest/exec/bare", Some(Duration::from_millis(5))))
            .join()
            .unwrap();
        assert_eq!(r, WakeReason::NotCurrent);
    }

    fn fake_body() -> Body {
        Body {
            id: 9999,
            name: "unit-sm".into(),
            state: ExecState::Running,
            priority: CpuPriority::Medium,
            base_priority: CpuPriority::Medium,
            parent: None,
            domain: None,
            created_ns: 0,
            exit_ns: None,
            exit_code: None,
            cpu_ns: 0,
            wakeups: 0,
            checkpoints: 0,
            signaled: false,
            inherited: false,
        }
    }

    #[test]
    fn illegal_transitions_are_rejected_without_mutation() {
        let mut b = fake_body();
        // Running 上 Resume / Exit 之外的非法事件必须被拒绝且状态不变。
        assert!(transition(&mut b, ExecEvent::Resume).is_err());
        assert_eq!(b.state, ExecState::Running);
        assert!(transition(&mut b, ExecEvent::Unblock).is_ok()); // 幂等
        assert_eq!(b.state, ExecState::Running);
        // Exited 之后一切活跃迁移都被拒绝。
        let _ = transition(&mut b, ExecEvent::Exit);
        assert!(transition(&mut b, ExecEvent::Block { key: "k".into() }).is_err());
        assert!(transition(&mut b, ExecEvent::Resume).is_err());
        assert_eq!(b.state, ExecState::Exited);
    }

    #[test]
    fn suspend_resume_restores_state_not_inferred() {
        let mut b = fake_body();
        // Running → Suspend → Resume 应回到 Running（不靠 wait_key 反推）。
        assert!(transition(&mut b, ExecEvent::Suspend).is_ok());
        assert_eq!(
            b.state,
            ExecState::Suspended {
                resume_to: ResumeTo::Idle
            }
        );
        assert!(transition(&mut b, ExecEvent::Resume).is_ok());
        assert_eq!(b.state, ExecState::Running);

        // Blocked → Suspend → Resume 应回到 Blocked{同一个 key}。
        assert!(transition(&mut b, ExecEvent::Block { key: "chan/x".into() }).is_ok());
        assert!(transition(&mut b, ExecEvent::Suspend).is_ok());
        assert_eq!(
            b.state,
            ExecState::Suspended {
                resume_to: ResumeTo::Blocked("chan/x".into())
            }
        );
        assert_eq!(b.wait_key(), Some("chan/x"));
        assert!(transition(&mut b, ExecEvent::Resume).is_ok());
        assert_eq!(
            b.state,
            ExecState::Blocked {
                key: "chan/x".into()
            }
        );

        // 已挂起时再次 Suspend 幂等：不覆盖最初的 resume_to。
        assert!(transition(&mut b, ExecEvent::Suspend).is_ok());
        assert_eq!(
            b.state,
            ExecState::Suspended {
                resume_to: ResumeTo::Blocked("chan/x".into())
            }
        );
    }

    #[test]
    fn suspend_resume_roundtrip_keeps_running_body_running() {
        let id = spawn(
            "unit-sres",
            CpuPriority::Medium,
            || {
                std::thread::sleep(Duration::from_millis(300));
            },
        );
        assert!(suspend(id));
        {
            let g = ex().inner.lock().unwrap();
            assert!(matches!(
                g.bodies[&id].state,
                ExecState::Suspended { resume_to: ResumeTo::Idle }
            ));
        }
        assert!(resume(id));
        {
            let g = ex().inner.lock().unwrap();
            assert_eq!(g.bodies[&id].state, ExecState::Running);
        }
        assert_eq!(join(id, Some(Duration::from_secs(2))), Some(0));
    }
}
