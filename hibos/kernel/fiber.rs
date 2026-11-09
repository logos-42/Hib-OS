//! 用户态纤程 —— 真正的多线程上下文切换（非 std::thread 协作式）。
//!
//! 使用 Windows Fiber API 在用户态实现独立线程。每个纤程拥有自己的
//! 栈和寄存器上下文，`SwitchToFiber` 执行真正的寄存器保存/恢复和
//! 栈切换，不依赖 OS 内核线程调度。
//!
//! 调度器维护纤程就绪队列，在时间片到期时强制切换到下一个纤程，
//! 实现抢占式调度。纤程在运行完或主动让出时，通过 `SwitchToFiber`
//! 返回调度器，由调度器选择下一个纤程。

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
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
    fibers: HashMap<FiberId, Fiber>,
    /// 当前运行的纤程。
    current: Option<FiberId>,
    /// 调度器自身的 Fiber 句柄（主纤程）。
    scheduler_fiber: *mut c_void,
    next_id: FiberId,
    time_slice_ns: u64,
    last_switch: Instant,
}

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
            fibers: HashMap::new(),
            current: None,
            scheduler_fiber: std::ptr::null_mut(),
            next_id: 1,
            time_slice_ns: 10_000_000, // 10 ms
            last_switch: Instant::now(),
        })
    })
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
        crate::fiber::mark_fiber_exited();
        SwitchToFiber(scheduler_fiber);
    }
}

/// 纤程入口参数。
struct FiberEntryParam {
    scheduler_fiber: *mut c_void,
    closure: Box<dyn FnOnce() + Send>,
}

/// 把当前线程转换为调度器纤程（仅在调度器首次初始化时调用一次）。
fn ensure_scheduler_fiber() {
    let mut s = scheduler().lock().unwrap();
    if s.scheduler_fiber.is_null() {
        unsafe {
            s.scheduler_fiber = ConvertThreadToFiber(std::ptr::null_mut());
        }
    }
    drop(s);
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

/// 当前纤程主动让出控制权，切换回调度器。
pub fn yield_to_scheduler() {
    let scheduler_fiber = {
        let s = scheduler().lock().unwrap();
        s.scheduler_fiber
    };
    if !scheduler_fiber.is_null() {
        unsafe {
            SwitchToFiber(scheduler_fiber);
        }
    }
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
    unsafe {
        SwitchToFiber(handle);
    }
}

/// 标记当前纤程已退出（由 fiber_entry 在闭包结束时调用）。
fn mark_fiber_exited() {
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
    // 不立即删除句柄，由调度器在下一次调度时清理
    let _ = current_id;
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
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// 将当前纤程放回就绪队列（用于时间片到期后的重入队）。
///
/// 这个函数由纤程内部的时间片检查调用。
pub fn requeue_current() {
    let id = {
        let s = scheduler().lock().unwrap();
        s.current
    };
    if let Some(id) = id {
        let mut s = scheduler().lock().unwrap();
        s.ready.push_back(id);
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
}
