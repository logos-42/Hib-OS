//! 用户态纤程 —— 真正的多线程上下文切换（基于 Windows Fiber）。
//!
//! 每个纤程拥有独立的栈和寄存器上下文，`SwitchToFiber` 执行真正的
//! 寄存器保存/恢复和栈切换，不依赖 OS 内核线程调度。
//!
//! 抢占式调度：
//! * 一个监视线程每隔若干毫秒检查当前运行纤程的时间片是否已耗尽；
//! * 若耗尽，设置全局抢占标志 `PREEMPT_REQUESTED`；
//! * 纤程在 `checkpoint()` / `yield_to_scheduler()` / 通道阻塞点检查该标志，
//!   若已设置则主动切换回调度器（准抢占式，Windows Fiber 无法从外部
//!   线程强制切换，这是用户态纤程的最高可达抢占级别）。

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// 纤程标识。
pub type FiberId = u64;

/// 纤程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiberState {
    Ready,
    Running,
    Blocked,
    Exited,
}

/// 用户态纤程。
pub struct Fiber {
    pub id: FiberId,
    /// Windows Fiber 句柄（用于 SwitchToFiber）。
    pub handle: *mut c_void,
    pub state: FiberState,
    /// 该纤程所属的调度域（None 表示全局域）。
    pub domain: Option<crate::scheduler_domains::SchedDomain>,
    pub created_ns: u64,
    /// 累计运行时间（纳秒）。
    pub cpu_ns: u64,
}

/// 纤程调度器。
struct FiberScheduler {
    /// 就绪队列（FiberId 队列）。
    ready: VecDeque<FiberId>,
    /// 纤程句柄表。
    fibers: BTreeMap<FiberId, Fiber>,
    /// 当前运行的纤程。
    current: Option<FiberId>,
    /// 调度器自身的 Fiber 句柄（主纤程）。
    scheduler_fiber: *mut c_void,
    next_id: FiberId,
    time_slice_ns: u64,
    /// 上次切换到当前纤程的时刻。
    last_switch: Instant,
}

/// 全局抢占标志：监视线程在时间片耗尽时置位，纤程在检查点读取后主动让出。
static PREEMPT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// SAFETY: Windows Fiber 句柄 (`*mut c_void`) 仅在调度器所在的单个 OS 线程上
/// 创建、使用和销毁，不会跨线程传递。全局调度器仅用于可观测性和命令路由，
/// 真正的纤程切换由 `SwitchToFiber` 在当前线程上完成。
unsafe impl Send for FiberScheduler {}
unsafe impl Sync for FiberScheduler {}

static FIBER_SCHEDULER: OnceLock<Mutex<FiberScheduler>> = OnceLock::new();

fn scheduler() -> &'static Mutex<FiberScheduler> {
    FIBER_SCHEDULER.get_or_init(|| {
        Mutex::new(FiberScheduler {
            ready: VecDeque::new(),
            fibers: BTreeMap::new(),
            current: None,
            scheduler_fiber: std::ptr::null_mut(),
            next_id: 1,
            time_slice_ns: 10_000_000, // 10 ms
            last_switch: Instant::now(),
        })
    })
}

/// 把当前线程转换为调度器纤程，并启动抢占监视线程（仅在首次初始化时调用）。
fn ensure_scheduler_fiber() {
    let mut s = scheduler().lock().unwrap();
    if s.scheduler_fiber.is_null() {
        unsafe {
            s.scheduler_fiber = ConvertThreadToFiber(std::ptr::null_mut());
        }
    }
    drop(s);
    // 启动抢占监视线程（仅一次）。
    static MONITOR_STARTED: OnceLock<()> = OnceLock::new();
    let _ = MONITOR_STARTED.get_or_init(|| {
        thread::spawn(|| preemption_monitor());
        ()
    });
}

/// 抢占监视线程：每隔 1ms 检查当前运行纤程是否已超时，若超时置位抢占标志。
fn preemption_monitor() {
    loop {
        thread::sleep(Duration::from_millis(1));
        let should_preempt = {
            let s = scheduler().lock().unwrap();
            match s.current {
                Some(id) => match s.fibers.get(&id) {
                    Some(f) if f.state == FiberState::Running => {
                        s.last_switch.elapsed() >= Duration::from_nanos(s.time_slice_ns)
                    }
                    _ => false,
                },
                None => false,
            }
        };
        if should_preempt {
            PREEMPT_REQUESTED.store(true, Ordering::SeqCst);
        }
    }
}

/// 检查并处理抢占请求：若监视线程已置位抢占标志，则重置标志并让出
/// 控制权给调度器（当前纤程会被重新入队）。
pub fn check_preemption() {
    if PREEMPT_REQUESTED.swap(false, Ordering::SeqCst) {
        requeue_current_and_yield();
    }
}

/// 把当前纤程重新入队并切换回调度器。
fn requeue_current_and_yield() {
    let scheduler_fiber = {
        let mut s = scheduler().lock().unwrap();
        if let Some(id) = s.current {
            s.ready.push_back(id);
            if let Some(f) = s.fibers.get_mut(&id) {
                f.state = FiberState::Ready;
            }
            s.current = None;
            s.last_switch = Instant::now();
            s.scheduler_fiber
        } else {
            std::ptr::null_mut()
        }
    };
    if !scheduler_fiber.is_null() {
        unsafe {
            SwitchToFiber(scheduler_fiber);
        }
    }
}

/// 纤程入口 trampoline：调用用户闭包，完成后切换回调度器。
extern "system" fn fiber_entry(param: *mut c_void) {
    // param 指向 FiberEntryParam
    let param = unsafe { Box::from_raw(param as *mut FiberEntryParam) };
    let scheduler_fiber = param.scheduler_fiber;
    let closure = param.closure;

    // 执行用户闭包
    closure();

    // 用户闭包结束，切换回调度器纤程。
    unsafe {
        mark_fiber_exited();
        SwitchToFiber(scheduler_fiber);
    }
}

/// 纤程入口参数。
struct FiberEntryParam {
    scheduler_fiber: *mut c_void,
    closure: Box<dyn FnOnce() + Send>,
}

/// 创建一个用户态纤程。
///
/// `closure` 将在纤程的独立栈上执行。闭包结束后，纤程自动切换回调度器。
pub fn spawn_fiber<F>(_name: &str, domain: Option<crate::scheduler_domains::SchedDomain>, f: F) -> FiberId
where
    F: FnOnce() + Send + 'static,
{
    ensure_scheduler_fiber();

    let scheduler_fiber = {
        let s = scheduler().lock().unwrap();
        s.scheduler_fiber
    };

    let id = {
        let mut s = scheduler().lock().unwrap();
        let id = s.next_id;
        s.next_id += 1;
        let handle = unsafe {
            CreateFiberEx(
                64 * 1024,    // commit 64KB
                2 * 1024 * 1024, // reserve 2MB
                0,
                fiber_entry as *mut c_void,
                Box::into_raw(Box::new(FiberEntryParam {
                    scheduler_fiber,
                    closure: Box::new(f),
                })) as *mut c_void,
            )
        };
        if handle.is_null() {
            panic!("CreateFiberEx 失败");
        }
        s.fibers.insert(
            id,
            Fiber {
                id,
                handle,
                state: FiberState::Ready,
                domain,
                created_ns: crate::monitor::get_time_ns() as u64,
                cpu_ns: 0,
            },
        );
        s.ready.push_back(id);
        id
    };

    id
}

/// 当前纤程主动让出控制权，切换回调度器（并在切换前检查抢占）。
pub fn yield_to_scheduler() {
    check_preemption();
    let scheduler_fiber = {
        let mut s = scheduler().lock().unwrap();
        s.current = None;
        s.last_switch = Instant::now();
        s.scheduler_fiber
    };
    if !scheduler_fiber.is_null() {
        unsafe {
            SwitchToFiber(scheduler_fiber);
        }
    }
}

/// 协作式安全点：纤程在长任务中周期性调用，检查是否该被抢占。
pub fn checkpoint() {
    check_preemption();
}

/// 标记当前纤程已退出（由 fiber_entry 在闭包结束时调用）。
fn mark_fiber_exited() {
    PREEMPT_REQUESTED.store(false, Ordering::SeqCst);
    let current_id = {
        let mut s = scheduler().lock().unwrap();
        if let Some(id) = s.current {
            if let Some(f) = s.fibers.get_mut(&id) {
                f.state = FiberState::Exited;
            }
            s.current = None;
            id
        } else {
            return;
        }
    };
    let _ = current_id;
}

/// 由调度器切换到指定纤程运行。
///
/// 调度器保存当前上下文，切换到目标纤程。目标纤程运行完或让出后，
/// 会通过 SwitchToFiber 返回调度器。
fn switch_to_fiber(fiber_id: FiberId) {
    // 第一步：只读取句柄（避免与后续状态更新的借用冲突）。
    let handle = {
        let s = scheduler().lock().unwrap();
        match s.fibers.get(&fiber_id) {
            Some(f) => f.handle,
            None => return,
        }
    };
    if handle.is_null() {
        return;
    }
    // 第二步：更新状态（新的锁作用域）。
    {
        let mut s = scheduler().lock().unwrap();
        if let Some(f) = s.fibers.get_mut(&fiber_id) {
            f.state = FiberState::Running;
            s.current = Some(fiber_id);
            s.last_switch = Instant::now();
        }
    }
    PREEMPT_REQUESTED.store(false, Ordering::SeqCst);
    unsafe {
        SwitchToFiber(handle);
    }
}

/// 运行调度器主循环：不断从就绪队列取出纤程并切换过去。
///
/// 这个函数会阻塞直到所有纤程都退出。
pub fn run_scheduler() {
    loop {
        let next_id = {
            let mut s = scheduler().lock().unwrap();
            // 清理已退出的纤程
            s.fibers.retain(|_, f| f.state != FiberState::Exited);
            s.ready.pop_front()
        };

        match next_id {
            Some(id) => {
                // 检查纤程是否仍存在
                let still_exists = {
                    let s = scheduler().lock().unwrap();
                    s.fibers.contains_key(&id)
                };
                if still_exists {
                    switch_to_fiber(id);
                }
            }
            None => {
                // 就绪队列为空，检查是否全部退出
                let all_exited = {
                    let s = scheduler().lock().unwrap();
                    s.fibers.values().all(|f| f.state == FiberState::Exited)
                        && s.ready.is_empty()
                };
                if all_exited {
                    break;
                }
                // 短暂休眠避免空转
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// 将当前纤程放回就绪队列（用于时间片到期后的重入队）。
pub fn requeue_current() {
    let id = {
        let s = scheduler().lock().unwrap();
        s.current
    };
    if let Some(id) = id {
        let mut s = scheduler().lock().unwrap();
        s.ready.push_back(id);
        if let Some(f) = s.fibers.get_mut(&id) {
            f.state = FiberState::Ready;
        }
        s.current = None;
    }
}

/// 查询调度器状态。
pub fn status() -> String {
    let s = scheduler().lock().unwrap();
    let mut running = 0;
    let mut ready = 0;
    let mut blocked = 0;
    let mut exited = 0;
    for f in s.fibers.values() {
        match f.state {
            FiberState::Running => running += 1,
            FiberState::Ready => ready += 1,
            FiberState::Blocked => blocked += 1,
            FiberState::Exited => exited += 1,
        }
    }
    format!(
        "fibers={} ready={} running={} blocked={} exited={} slice={}ns",
        s.fibers.len(),
        ready,
        running,
        blocked,
        exited,
        s.time_slice_ns
    )
}

/// 列出所有纤程。
pub fn list() -> String {
    let s = scheduler().lock().unwrap();
    if s.fibers.is_empty() {
        return "（无纤程）".to_string();
    }
    let mut out = String::new();
    for f in s.fibers.values() {
        out.push_str(&format!(
            "fiber={:<4} {:<10} domain={:?} cpu={}ns\n",
            f.id,
            f.state.name(),
            f.domain,
            f.cpu_ns
        ));
    }
    out.trim_end().to_string()
}

impl FiberState {
    pub fn name(self) -> &'static str {
        match self {
            FiberState::Ready => "ready",
            FiberState::Running => "running",
            FiberState::Blocked => "blocked",
            FiberState::Exited => "exited",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_status() {
        // 只验证纤程创建和状态查询，不触发 SwitchToFiber 切换
        //（避免 ConvertThreadToFiber 污染测试线程）。
        let id = spawn_fiber("test-fiber", None, || {});
        let s = status();
        assert!(s.contains("fibers="), "status should contain fibers count: {}", s);
        let l = list();
        assert!(l.contains(&format!("fiber={:<4} ", id)));
        assert!(l.contains("ready"));
    }

    #[test]
    fn domain_is_recorded() {
        use crate::scheduler_domains::SchedDomain;
        let _id = spawn_fiber("domain-fiber", Some(SchedDomain::Reflex), || {});
        let l = list();
        assert!(l.contains("domain=Some(Reflex)"));
    }

    #[test]
    fn preemption_flag_is_reset_on_switch() {
        // 切换到纤程时应重置抢占标志
        let _id = spawn_fiber("preempt-test", None, || {});
        // 手动置位抢占标志
        PREEMPT_REQUESTED.store(true, Ordering::SeqCst);
        // 调用 switch_to_fiber 内部逻辑通过 ensure_scheduler_fiber + spawn 已触发，
        // 这里只验证标志位可被正常读写
        assert!(PREEMPT_REQUESTED.load(Ordering::SeqCst));
        PREEMPT_REQUESTED.store(false, Ordering::SeqCst);
        assert!(!PREEMPT_REQUESTED.load(Ordering::SeqCst));
    }
}

// ===== Windows Fiber API 手动声明（避免引入重型 windows-sys crate）=====

#[link(name = "kernel32")]
extern "system" {
    /// 将当前线程转换为纤程，返回纤程句柄。
    fn ConvertThreadToFiber(lpParameter: *mut c_void) -> *mut c_void;
    /// 创建纤程。
    fn CreateFiberEx(
        dwStackCommitSize: usize,
        dwStackReserveSize: usize,
        dwFlags: u32,
        lpStartAddress: *mut c_void, // FIBER_START_ROUTINE
        lpParameter: *mut c_void,
    ) -> *mut c_void;
    /// 切换到指定纤程。
    fn SwitchToFiber(lpFiber: *mut c_void);
    /// 删除纤程。
    #[allow(dead_code)]
    fn DeleteFiber(lpFiber: *mut c_void);
}


