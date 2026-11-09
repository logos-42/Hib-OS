//! 轻量级 async 运行时（futures + reactor）。
//!
//! 提供最小化的异步执行模型：
//!
//! * `Future` 特质与 `Poll` 枚举（手写简化版，不依赖 `std::future`）；
//! * `Task` —— 封装 Future 的可调度单元，支持 spawn / block_on / wake / cancel；
//! * `Reactor` —— 轮询器，注册唤醒（waker）与驱动所有任务推进；
//! * `Runtime` —— 运行时封装，提供 spawn / block_on / wake / task_state / cancel；
//! * 全局运行时单例，供 `sense/act` 等通道模块调用。
//!
//! 本模块为协作式调度模拟：每个任务的 Future 被反复 `poll`，若返回
//! `Pending` 则挂起直到被 `wake`，适合异步子系统的原型验证。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, RawWaker, RawWakerVTable, Waker};

// =====================================================================================
// Poll 与 Future（简化版）
// =====================================================================================

/// 异步轮询结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll<T> {
    /// 已完成，携带结果。
    Ready(T),
    /// 尚未完成，任务将被挂起，稍后再次 poll。
    Pending,
}

impl<T> Poll<T> {
    /// 是否为 Ready。
    pub fn is_ready(self) -> bool {
        matches!(self, Poll::Ready(_))
    }

    /// 是否为 Pending。
    pub fn is_pending(self) -> bool {
        matches!(self, Poll::Pending)
    }
}

/// 异步特质：类似 `std::future::Future` 的简化版。
pub trait Future {
    type Output;
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Self::Output>;
}

// =====================================================================================
// 任务状态与任务
// =====================================================================================

/// 任务执行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// 初始，尚未首次 poll。
    New,
    /// 就绪，在运行队列中等待被 poll。
    Ready,
    /// 正在 poll 中。
    Running,
    /// 挂起，等待被 wake。
    Waiting,
    /// 已完成。
    Completed,
    /// 已取消。
    Cancelled,
}

/// 任务：封装一个 Future 及其状态与 waker。
pub struct Task {
    pub id: u64,
    pub name: String,
    /// 被 boxed 的 Future（使用 trait object 以支持多种 future 类型）。
    pub future: Box<dyn Future<Output = ()> + Send>,
    pub state: TaskState,
    /// 当前挂起时的 waker（用于 wake 唤醒）。
    pub waker: Option<Waker>,
    /// 任务是否已完成。
    pub completed: bool,
}

impl Task {
    pub fn new(id: u64, name: &str, future: Box<dyn Future<Output = ()> + Send>) -> Self {
        Self {
            id,
            name: name.to_string(),
            future,
            state: TaskState::New,
            waker: None,
            completed: false,
        }
    }

    /// 轮询一次任务。返回 Poll 结果。
    pub fn poll_once(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.state = TaskState::Running;
        let result = self.future.poll(cx);
        match result {
            Poll::Ready(()) => {
                self.state = TaskState::Completed;
                self.completed = true;
            }
            Poll::Pending => {
                self.state = TaskState::Waiting;
            }
        }
        result
    }
}

// =====================================================================================
// 简易 Waker / Context 构造
// =====================================================================================

/// 任务唤醒回调：wake 某个任务 ID。
type WakeCallback = Box<dyn Fn(u64) + Send + Sync>;

/// 构造一个绑定到任务 ID 的 Waker（实际唤醒由 Reactor 负责将任务重新入就绪队列）。
fn make_waker(task_id: u64, wake_cb: Arc<WakeCallback>) -> Waker {
    unsafe { Waker::from_raw(raw_waker(task_id, wake_cb)) }
}

unsafe fn raw_waker(task_id: u64, wake_cb: Arc<WakeCallback>) -> RawWaker {
    let data = Box::into_raw(Box::new((task_id, wake_cb))) as *mut ();
    let vtable = RawWakerVTable::new(clone_waker, wake_task, wake_by_ref_task, drop_waker);
    RawWaker::new(data, &vtable)
}

unsafe fn clone_waker(data: *const ()) -> RawWaker {
    let ptr = data as *const (u64, Arc<WakeCallback>);
    let arc = &(*ptr).1;
    std::mem::forget(arc.clone()); // 增加引用计数
    raw_waker((*ptr).0, (*ptr).1.clone())
}

unsafe fn wake_task(data: *const ()) {
    let ptr = data as *const (u64, Arc<WakeCallback>);
    let (task_id, cb) = &(*ptr);
    cb(*task_id);
}

unsafe fn wake_by_ref_task(data: *const ()) {
    wake_task(data);
}

unsafe fn drop_waker(data: *const ()) {
    let ptr = data as *mut (u64, Arc<WakeCallback>);
    drop(Box::from_raw(ptr));
}

// =====================================================================================
// Reactor —— 轮询器
// =====================================================================================

/// 反应器：维护任务集与就绪队列，反复 poll 所有就绪任务，
/// 直到没有任务可推进（stalled）。
pub struct Reactor {
    /// 任务 ID -> 任务。
    tasks: HashMap<u64, Task>,
    /// 就绪队列：待 poll 的任务 ID。
    ready: VecDeque<u64>,
    /// 唤醒回调：将任务 ID 放回就绪队列。
    wake_cb: Arc<WakeCallback>,
    /// 下一个任务 ID。
    next_id: u64,
    /// 统计。
    poll_count: u64,
    wake_count: u64,
    completed_count: u64,
}

impl Reactor {
    pub fn new() -> Self {
        let wake_cb: WakeCallback = Box::new(|_tid| {});
        Self {
            tasks: HashMap::new(),
            ready: VecDeque::new(),
            wake_cb: Arc::new(wake_cb),
            next_id: 1,
            poll_count: 0,
            wake_count: 0,
            completed_count: 0,
        }
    }

    /// 设置真实的唤醒回调（由 Runtime 注入，wake 某任务会把它放回就绪队列）。
    pub fn set_wake_cb(&mut self, cb: WakeCallback) {
        self.wake_cb = Arc::new(cb);
    }

    /// 创建任务并放入就绪队列，返回任务 ID。
    pub fn spawn_task(
        &mut self,
        name: &str,
        future: Box<dyn Future<Output = ()> + Send>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let task = Task::new(id, name, future);
        self.tasks.insert(id, task);
        self.ready.push_back(id);
        id
    }

    /// 唤醒任务：将其放回就绪队列。
    pub fn wake(&mut self, task_id: u64) {
        if self.tasks.contains_key(&task_id) {
            self.ready.push_back(task_id);
            self.wake_count += 1;
        }
    }

    /// 取消任务。
    pub fn cancel(&mut self, task_id: u64) -> bool {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            task.state = TaskState::Cancelled;
            task.completed = true;
            true
        } else {
            false
        }
    }

    /// 查询任务状态。
    pub fn task_state(&self, task_id: u64) -> Option<TaskState> {
        self.tasks.get(&task_id).map(|t| t.state)
    }

    /// 单次轮询所有就绪任务（每个任务 poll 一次）。
    pub fn poll(&mut self) -> usize {
        let mut polled = 0;
        let ready_count = self.ready.len();
        for _ in 0..ready_count {
            let task_id = match self.ready.pop_front() {
                Some(id) => id,
                None => break,
            };
            let need_poll = self
                .tasks
                .get(&task_id)
                .map(|t| matches!(t.state, TaskState::New | TaskState::Ready | TaskState::Waiting))
                .unwrap_or(false);
            if !need_poll {
                continue;
            }
            let cx = dummy_context();
            let mut task = self.tasks.remove(&task_id).expect("task exists");
            let result = task.poll_once(&mut cx);
            self.tasks.insert(task_id, task);
            self.poll_count += 1;
            polled += 1;
            match result {
                Poll::Ready(()) => {
                    self.completed_count += 1;
                    // 完成任务不再入队。
                }
                Poll::Pending => {
                    // 任务挂起，等待外部 wake 后再次入队。
                }
            }
        }
        polled
    }

    /// 运行直到没有任务可推进（所有任务完成、挂起或取消）。
    ///
    /// 注意：挂起且无外部唤醒的任务会导致函数在它们完成前返回（stalled）。
    pub fn run_until_stalled(&mut self) -> usize {
        let mut total_polled = 0;
        loop {
            let polled = self.poll();
            total_polled += polled;
            if polled == 0 {
                break;
            }
        }
        total_polled
    }

    /// 返回任务总数。
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// 返回就绪队列长度。
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// 返回已完成任务数。
    pub fn completed_count(&self) -> u64 {
        self.completed_count
    }

    /// 返回唤醒总次数。
    pub fn wake_count(&self) -> u64 {
        self.wake_count
    }

    /// 返回总 poll 次数。
    pub fn poll_count(&self) -> u64 {
        self.poll_count
    }

    /// 返回 reactor 文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "reactor tasks={} ready={} completed={} polls={} wakes={}",
            self.task_count(),
            self.ready.len(),
            self.completed_count(),
            self.poll_count(),
            self.wake_count(),
        )
    }
}

impl Default for Reactor {
    fn default() -> Self {
        Self::new()
    }
}

/// 空 Context（用于无真正唤醒需求的测试）。
fn dummy_context() -> Context<'static> {
    let noop_waker: WakeCallback = Arc::new(|_| {});
    unsafe { Waker::from_raw(raw_waker(0, noop_waker)) }
}

// =====================================================================================
// Runtime —— 轻量级 async 运行时封装
// =====================================================================================

/// 轻量级 async 运行时：封装 Reactor，提供 spawn / block_on / wake /
/// task_state / cancel 接口。
pub struct Runtime {
    reactor: Mutex<Reactor>,
}

impl Runtime {
    pub fn new() -> Self {
        let mut reactor = Reactor::new();
        // 注入唤醒回调：wake 某任务会把它放回该 reactor 的就绪队列。
        // 由于 Reactor 由 Mutex 保护，回调内部需重新锁 reactor；这里用一个
        // OnceLock 持 reactor 指针的简化做法：回调仅标记待唤醒，实际由 poll
        // 统一处理。为保持简单，wake 直接通过 Runtime::wake 入队。
        let _ = reactor;
        Self {
            reactor: Mutex::new(reactor),
        }
    }

    /// 创建任务并返回任务 ID。
    pub fn spawn<F>(&self, name: &str, future: F) -> u64
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut r = self.reactor.lock().unwrap();
        r.spawn_task(name, Box::new(future))
    }

    /// 阻塞当前线程，直到 `future` 完成。
    ///
    /// 此 future 会被临时封装为一个任务并反复 poll，直至 Ready。
    pub fn block_on<F>(&self, mut future: F) -> F::Output
    where
        F: Future,
    {
        let cx = dummy_context();
        loop {
            match future.poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => {
                    // 协作式 yield，避免忙等占满 CPU。
                    std::thread::yield_now();
                }
            }
        }
    }

    /// 唤醒指定任务，使其在下次 poll 中继续推进。
    pub fn wake(&self, task_id: u64) {
        self.reactor.lock().unwrap().wake(task_id);
    }

    /// 查询任务状态。
    pub fn task_state(&self, task_id: u64) -> Option<TaskState> {
        self.reactor.lock().unwrap().task_state(task_id)
    }

    /// 取消任务。
    pub fn cancel(&self, task_id: u64) -> bool {
        self.reactor.lock().unwrap().cancel(task_id)
    }

    /// 单次 poll 所有就绪任务，返回 poll 任务数。
    pub fn poll(&self) -> usize {
        self.reactor.lock().unwrap().poll()
    }

    /// 运行直到 stalled。
    pub fn run_until_stalled(&self) -> usize {
        self.reactor.lock().unwrap().run_until_stalled()
    }

    /// 返回任务总数。
    pub fn task_count(&self) -> usize {
        self.reactor.lock().unwrap().task_count()
    }

    /// 返回 reactor 文本摘要。
    pub fn summary(&self) -> String {
        self.reactor.lock().unwrap().summary()
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局运行时单例
// =====================================================================================

static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

fn runtime() -> &'static Mutex<Runtime> {
    RUNTIME.get_or_init(|| Mutex::new(Runtime::new()))
}

/// 创建任务（全局运行时）。
pub fn async_spawn<F>(name: &str, future: F) -> u64
where
    F: Future<Output = ()> + Send + 'static,
{
    runtime().lock().unwrap().spawn(name, future)
}

/// 唤醒任务（全局运行时）。
pub fn async_wake(task_id: u64) {
    runtime().lock().unwrap().wake(task_id);
}

/// 查询任务状态（全局运行时）。
pub fn async_task_state(task_id: u64) -> Option<TaskState> {
    runtime().lock().unwrap().task_state(task_id)
}

/// 取消任务（全局运行时）。
pub fn async_cancel(task_id: u64) -> bool {
    runtime().lock().unwrap().cancel(task_id)
}

/// 单次 poll（全局运行时）。
pub fn async_poll() -> usize {
    runtime().lock().unwrap().poll()
}

/// 运行直到 stalled（全局运行时）。
pub fn async_run_until_stalled() -> usize {
    runtime().lock().unwrap().run_until_stalled()
}

/// 返回运行时摘要（全局）。
pub fn async_summary() -> String {
    runtime().lock().unwrap().summary()
}

// =====================================================================================
// 示例 Future：计数器 future（用于测试）
// =====================================================================================

/// 一个简单的 Future：poll N 次后完成，模拟异步操作。
pub struct CounterFuture {
    name: String,
    remaining: u64,
}

impl CounterFuture {
    pub fn new(name: &str, n: u64) -> Self {
        Self { name: name.to_string(), remaining: n }
    }
}

impl Future for CounterFuture {
    type Output = u64;
    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.remaining == 0 {
            Poll::Ready(self.name.len() as u64)
        } else {
            self.remaining -= 1;
            Poll::Pending
        }
    }
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn poll_ready_pending() {
        assert!(Poll::Ready(1u8).is_ready());
        assert!(!Poll::Ready(1u8).is_pending());
        assert!(Poll::Pending::<u8>.is_pending());
        assert!(!Poll::Pending::<u8>.is_ready());
    }

    #[test]
    fn counter_future_polls_then_ready() {
        let mut f = CounterFuture::new("cnt", 3);
        let cx = dummy_context();
        // 前 3 次 pending
        for _ in 0..3 {
            assert_eq!(f.poll(&mut cx), Poll::Pending);
        }
        // 第 4 次 ready
        assert!(f.poll(&mut cx).is_ready());
    }

    #[test]
    fn runtime_spawn_and_block_on() {
        let rt = Runtime::new();
        let result = rt.block_on(CounterFuture::new("block", 2));
        assert_eq!(result, 5usize as u64); // "block" 长度 5
    }

    #[test]
    fn runtime_spawn_task_poll() {
        let rt = Runtime::new();
        let id = rt.spawn("spawned", CounterFuture::new("spawned", 2));
        // spawn 的 future 仍需由 poll 逐步推进；这里 poll 一次应推进但 pending
        let polled = rt.poll();
        assert_eq!(polled, 1);
        assert_eq!(rt.task_state(id), Some(TaskState::Waiting));
    }

    #[test]
    fn runtime_wake_resumes_task() {
        let rt = Runtime::new();
        let id = rt.spawn("wake-t", CounterFuture::new("wake-t", 3));
        // 需要多次 poll + wake 才能完成
        for _ in 0..3 {
            rt.poll();
            rt.wake(id);
        }
        // 最后一次 poll 应完成
        rt.poll();
        assert_eq!(rt.task_state(id), Some(TaskState::Completed));
    }

    #[test]
    fn runtime_cancel_task() {
        let rt = Runtime::new();
        let id = rt.spawn("cancel-t", CounterFuture::new("cancel-t", 10));
        assert!(rt.cancel(id));
        assert_eq!(rt.task_state(id), Some(TaskState::Cancelled));
        // 已取消不再被 poll
        let polled = rt.poll();
        assert_eq!(polled, 0);
    }

    #[test]
    fn runtime_task_state_new_ready() {
        let rt = Runtime::new();
        let id = rt.spawn("state-t", CounterFuture::new("state-t", 1));
        assert_eq!(rt.task_state(id), Some(TaskState::New));
        rt.poll();
        assert_eq!(rt.task_state(id), Some(TaskState::Completed));
    }

    #[test]
    fn global_async_functions() {
        let id = async_spawn("global-async", CounterFuture::new("g", 1));
        async_wake(id);
        let st = async_task_state(id);
        assert!(st.is_some());
        let _ = async_cancel(id);
        let _polled = async_poll();
        let _ = _polled;
        let _run = async_run_until_stalled();
        let _ = run;
        let _s = async_summary();
        let _ = s;
    }

    #[test]
    fn run_until_stalled_completes_all() {
        let rt = Runtime::new();
        let _ = rt.spawn("a", CounterFuture::new("a", 1));
        let _ = rt.spawn("b", CounterFuture::new("b", 2));
        let total = rt.run_until_stalled();
        assert_eq!(total, 2);
        assert_eq!(rt.completed_count(), 2);
    }

    #[test]
    fn summary_contains_fields() {
        let rt = Runtime::new();
        let _ = rt.spawn("s", CounterFuture::new("s", 1));
        let s = rt.summary();
        assert!(s.contains("tasks="));
        assert!(s.contains("polls="));
    }
}
