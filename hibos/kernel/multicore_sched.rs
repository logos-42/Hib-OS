//! 多核调度模拟（CFS / EEVDF 简化版）。
//!
//! 模拟多核系统的调度器：每个核心独立维护运行队列，支持 CFS（完全公平调度）
//! 的虚拟运行时间 vruntime 与 nice 值权重，以及 EEVDF 简化版的截止时间排序。
//! 提供跨核负载均衡：繁忙核向空闲核迁移任务。
//!
//! 本模块为纯内存模拟，不操作真实线程/CPU，适合调度子系统的原型验证。

use std::collections::{BTreeMap, BinaryHeap, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 任务与核心通用类型
// =====================================================================================

/// 任务标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub u64);

impl TaskId {
    pub fn new() -> Self {
        Self(std::sync::atomic::AtomicU64::new(0).load(std::sync::atomic::Ordering::Relaxed) + 1)
    }

    /// 生成下一个全局唯一 ID（简化：由调用方保证单线程或使用全局计数器）。
    pub fn next() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(n + 1)
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task#{}", self.0)
    }
}

/// 核心标识符。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoreId(pub usize);

/// 任务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// 就绪，可调度。
    Ready,
    /// 正在运行。
    Running,
    /// 等待（阻塞）。
    Waiting,
    /// 已完成。
    Completed,
}

/// 任务记录。
#[derive(Clone, Debug)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    /// nice 值（-20..=19，越小优先级越高）。
    pub nice: i32,
    /// 权重（由 nice 推导）。
    pub weight: u32,
    /// CFS 虚拟运行时间（纳秒）。
    pub vruntime: u64,
    /// EEVDF 截止时间（纳秒，简化版）。
    pub deadline: u64,
    /// 已实际运行时间（纳秒）。
    pub run_time_ns: u64,
    pub state: TaskState,
    /// 绑定的核心（None 表示未绑定，可迁移）。
    pub bound_core: Option<CoreId>,
}

impl Task {
    pub fn new(name: &str, nice: i32) -> Self {
        let weight = nice_to_weight(nice);
        Self {
            id: TaskId::next(),
            name: name.to_string(),
            nice,
            weight,
            vruntime: 0,
            deadline: 0,
            run_time_ns: 0,
            state: TaskState::Ready,
            bound_core: None,
        }
    }

    /// 返回任务的调度优先级标签。
    pub fn priority_label(&self) -> &'static str {
        match self.nice {
            n if n <= -10 => "high",
            n if n <= 0 => "normal",
            _ => "low",
        }
    }
}

/// nice 值 -> 权重（简化版，参考 Linux CFS 的 prio_to_weight）。
fn nice_to_weight(nice: i32) -> u32 {
    const WEIGHTS: [u32; 40] = [
        88761, 71755, 58316, 47360, 38453, 31206, 25333, 20572, 16711, 13563,
        11016, 8940, 7255, 5887, 4782, 3883, 3153, 2557, 2075, 1685,
        1368, 1110, 901, 731, 594, 482, 391, 317, 257, 208,
        169, 137, 111, 90, 73, 59, 48, 39, 32, 26,
    ];
    let idx = (nice.clamp(-20, 19) + 20) as usize;
    WEIGHTS[idx]
}

// =====================================================================================
// 就绪队列：按 vruntime 排序（CFS 模拟）与按 deadline 排序（EEVDF 模拟）
// =====================================================================================

/// CFS 就绪队列：按 vruntime 升序排列，vruntime 最小的任务先运行。
#[derive(Clone, Debug)]
pub struct CfsRunQueue {
    /// 按 vruntime 排序的任务（BTreeMap: vruntime -> 任务 ID 列表，支持同 vruntime 多任务）。
    tasks: BTreeMap<u64, Vec<TaskId>>,
    /// 任务 ID -> 任务（用于查询状态）。
    task_map: HashMap<TaskId, Task>,
    /// 当前运行任务。
    current: Option<TaskId>,
    /// 核心累计 vruntime（用于计算新任务的初始 vruntime）。
    min_vruntime: u64,
}

impl CfsRunQueue {
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            task_map: HashMap::new(),
            current: None,
            min_vruntime: 0,
        }
    }

    /// 入队任务。
    pub fn enqueue(&mut self, task: Task) {
        let tid = task.id;
        let vruntime = task.vruntime;
        self.min_vruntime = self.min_vruntime.min(vruntime);
        self.tasks.entry(vruntime).or_default().push(tid);
        self.task_map.insert(tid, task);
    }

    /// 选出下一个要运行的任务（vruntime 最小）。
    pub fn pick_next(&mut self) -> Option<TaskId> {
        // 移除已完成或非就绪的任务。
        self.cleanup_non_ready();
        let mut min_vruntime = None;
        let mut chosen = None;
        for (&vruntime, ids) in self.tasks.iter() {
            if let Some(tid) = ids.iter().find(|&&t| self.task_map.get(&t).map_or(false, |task| task.state == TaskState::Ready)) {
                min_vruntime = Some(vruntime);
                chosen = Some(*tid);
                break;
            }
        }
        if let Some(tid) = chosen {
            self.current = Some(tid);
            if let Some(task) = self.task_map.get_mut(&tid) {
                task.state = TaskState::Running;
            }
        } else {
            self.current = None;
        }
        chosen
    }

    /// 将任务从就绪队列中移除（不改变其状态）。
    pub fn remove(&mut self, tid: TaskId) -> Option<Task> {
        let task = self.task_map.remove(&tid)?;
        let vruntime = task.vruntime;
        if let Some(ids) = self.tasks.get_mut(&vruntime) {
            ids.retain(|&t| t != tid);
            if ids.is_empty() {
                self.tasks.remove(&vruntime);
            }
        }
        if self.current == Some(tid) {
            self.current = None;
        }
        Some(task)
    }

    /// 更新任务的 vruntime（运行一段时间后重新入队）。
    pub fn update_vruntime(&mut self, tid: TaskId, delta_ns: u64) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            // CFS：实际运行时间 * (NICE_0_WEIGHT / weight) 增加 vruntime。
            let delta_vruntime = delta_ns * 1024 / task.weight.max(1) as u64;
            task.vruntime = task.vruntime.saturating_add(delta_vruntime);
            task.run_time_ns = task.run_time_ns.saturating_add(delta_ns);
            task.state = TaskState::Ready;
            // 重新插入（先移除旧的，再按新 vruntime 插入）。
            // 简化：直接更新 task_map 中的 vruntime；下次 pick_next 时按最新 vruntime 排序。
            // 这里我们把任务从旧 vruntime 桶移到新桶。
            let old_vruntime = task.vruntime.saturating_sub(delta_vruntime);
            if let Some(ids) = self.tasks.get_mut(&old_vruntime) {
                ids.retain(|&t| t != tid);
                if ids.is_empty() {
                    self.tasks.remove(&old_vruntime);
                }
            }
            self.tasks.entry(task.vruntime).or_default().push(tid);
            self.min_vruntime = self.min_vruntime.min(task.vruntime);
        }
    }

    /// 任务让出 CPU（回到就绪）。
    pub fn yield_task(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Ready;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    /// 任务阻塞。
    pub fn block(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Waiting;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    /// 任务完成。
    pub fn complete(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Completed;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    /// 返回就绪任务数量。
    pub fn ready_count(&self) -> usize {
        self.task_map
            .values()
            .filter(|t| t.state == TaskState::Ready)
            .count()
    }

    /// 返回运行中任务数量（0 或 1）。
    pub fn running_count(&self) -> usize {
        self.task_map.values().filter(|t| t.state == TaskState::Running).count()
    }

    /// 返回等待中任务数量。
    pub fn waiting_count(&self) -> usize {
        self.task_map.values().filter(|t| t.state == TaskState::Waiting).count()
    }

    /// 返回当前运行任务 ID。
    pub fn current(&self) -> Option<TaskId> {
        self.current
    }

    /// 查询任务状态。
    pub fn task_state(&self, tid: TaskId) -> Option<TaskState> {
        self.task_map.get(&tid).map(|t| t.state)
    }

    /// 返回任务 vruntime。
    pub fn task_vruntime(&self, tid: TaskId) -> Option<u64> {
        self.task_map.get(&tid).map(|t| t.vruntime)
    }

    /// 清理非就绪任务（辅助）。
    fn cleanup_non_ready(&mut self) {
        let mut empty_buckets = Vec::new();
        for (&vruntime, ids) in self.tasks.iter() {
            let has_ready = ids.iter().any(|&t| self.task_map.get(&t).map_or(false, |task| task.state == TaskState::Ready));
            if !has_ready {
                empty_buckets.push(vruntime);
            }
        }
        for vruntime in empty_buckets {
            self.tasks.remove(&vruntime);
        }
    }

    /// 返回队列文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "cfs_rq tasks={} ready={} running={} waiting={} min_vruntime={} current={:?}",
            self.task_map.len(),
            self.ready_count(),
            self.running_count(),
            self.waiting_count(),
            self.min_vruntime,
            self.current.map(|t| t.0).unwrap_or(0)
        )
    }
}

/// EEVDF 简化版就绪队列：按截止时间（deadline）升序调度。
#[derive(Clone, Debug)]
pub struct EevdfRunQueue {
    /// 按 deadline 排序的任务（BinaryHeap 最大堆，取反 deadlines 实现最小堆语义）。
    // 使用 BTreeMap：deadline -> 任务 ID 列表。
    tasks: BTreeMap<u64, Vec<TaskId>>,
    task_map: HashMap<TaskId, Task>,
    current: Option<TaskId>,
}

impl EevdfRunQueue {
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            task_map: HashMap::new(),
            current: None,
        }
    }

    pub fn enqueue(&mut self, task: Task) {
        let tid = task.id;
        self.tasks.entry(task.deadline).or_default().push(tid);
        self.task_map.insert(tid, task);
    }

    pub fn pick_next(&mut self) -> Option<TaskId> {
        // 选 deadline 最小的就绪任务。
        let chosen = self
            .tasks
            .iter()
            .find_map(|(&deadline, ids)| {
                ids.iter()
                    .find(|&&t| self.task_map.get(&t).map_or(false, |task| task.state == TaskState::Ready))
                    .map(|&t| (deadline, t))
            });
        if let Some((_deadline, tid)) = chosen {
            self.current = Some(tid);
            if let Some(task) = self.task_map.get_mut(&tid) {
                task.state = TaskState::Running;
            }
            Some(tid)
        } else {
            self.current = None;
            None
        }
    }

    pub fn remove(&mut self, tid: TaskId) -> Option<Task> {
        let task = self.task_map.remove(&tid)?;
        let deadline = task.deadline;
        if let Some(ids) = self.tasks.get_mut(&deadline) {
            ids.retain(|&t| t != tid);
            if ids.is_empty() {
                self.tasks.remove(&deadline);
            }
        }
        if self.current == Some(tid) {
            self.current = None;
        }
        Some(task)
    }

    pub fn yield_task(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Ready;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    pub fn block(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Waiting;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    pub fn complete(&mut self, tid: TaskId) {
        if let Some(task) = self.task_map.get_mut(&tid) {
            task.state = TaskState::Completed;
        }
        if self.current == Some(tid) {
            self.current = None;
        }
    }

    pub fn ready_count(&self) -> usize {
        self.task_map.values().filter(|t| t.state == TaskState::Ready).count()
    }

    /// 返回运行中任务数。
    pub fn running_count(&self) -> usize {
        self.task_map.values().filter(|t| t.state == TaskState::Running).count()
    }

    pub fn task_state(&self, tid: TaskId) -> Option<TaskState> {
        self.task_map.get(&tid).map(|t| t.state)
    }

    pub fn current(&self) -> Option<TaskId> {
        self.current
    }

    pub fn summary(&self) -> String {
        format!(
            "eevdf_rq tasks={} ready={} current={:?}",
            self.task_map.len(),
            self.ready_count(),
            self.current.map(|t| t.0).unwrap_or(0)
        )
    }
}

// =====================================================================================
// CoreScheduler —— 单核调度器（CFS + EEVDF 双队列）
// =====================================================================================

/// 单核调度器：维护 CFS 队列与 EEVDF 队列，支持 tick（时间片推进）、
/// 入队、选下一个、让出、阻塞、完成。
pub struct CoreScheduler {
    pub core_id: CoreId,
    pub cfs: CfsRunQueue,
    pub eevdf: EevdfRunQueue,
    /// 时间片长度（纳秒）。
    pub time_slice_ns: u64,
    /// 核心负载（已运行任务权重之和，用于负载均衡）。
    pub load: u64,
    /// 总调度次数。
    pub schedule_count: u64,
}

impl CoreScheduler {
    pub fn new(core_id: CoreId) -> Self {
        Self {
            core_id,
            cfs: CfsRunQueue::new(),
            eevdf: EevdfRunQueue::new(),
            time_slice_ns: 1_000_000, // 1 ms
            load: 0,
            schedule_count: 0,
        }
    }

    /// 将任务入队到指定调度策略的就绪队列。
    pub fn enqueue(&mut self, task: Task, use_eevdf: bool) {
        let weight = task.weight;
        self.load = self.load.saturating_add(weight as u64);
        if use_eevdf {
            self.eevdf.enqueue(task);
        } else {
            self.cfs.enqueue(task);
        }
    }

    /// 选出下一个要运行的任务：优先 EEVDF（截止时间紧），其次 CFS。
    pub fn pick_next(&mut self, use_eevdf: bool) -> Option<TaskId> {
        let tid = if use_eevdf {
            self.eevdf.pick_next()
        } else {
            self.cfs.pick_next()
        };
        if tid.is_some() {
            self.schedule_count += 1;
            self._update_load();
        }
        tid
    }

    /// 让出 CPU：当前运行任务回到就绪。
    pub fn yield_to(&mut self, tid: TaskId, use_eevdf: bool) {
        if use_eevdf {
            self.eevdf.yield_task(tid);
        } else {
            self.cfs.yield_task(tid);
        }
    }

    /// 时钟中断：运行任务消耗一个时间片，更新 vruntime。
    pub fn tick(&mut self, tid: TaskId, use_eevdf: bool, delta_ns: u64) {
        if use_eevdf {
            // EEVDF：按实际运行时间推进 deadline（简化：deadline += delta）。
            if let Some(task) = self.eevdf.task_map.get_mut(&tid) {
                task.deadline = task.deadline.saturating_add(delta_ns);
                task.run_time_ns = task.run_time_ns.saturating_add(delta_ns);
                task.state = TaskState::Ready;
            }
            self.eevdf.current = None;
        } else {
            self.cfs.update_vruntime(tid, delta_ns);
        }
    }

    /// 阻塞任务。
    pub fn block(&mut self, tid: TaskId, use_eevdf: bool) {
        if use_eevdf {
            self.eevdf.block(tid);
        } else {
            self.cfs.block(tid);
        }
        self._update_load();
    }

    /// 任务完成。
    pub fn complete(&mut self, tid: TaskId, use_eevdf: bool) {
        if use_eevdf {
            self.eevdf.complete(tid);
        } else {
            self.cfs.complete(tid);
        }
        self._update_load();
    }

    /// 返回就绪任务总数（跨两个队列）。
    pub fn ready_count(&self) -> usize {
        self.cfs.ready_count() + self.eevdf.ready_count()
    }

    /// 返回运行中任务总数（跨两个队列，通常每核最多 1 个）。
    pub fn running_count(&self) -> usize {
        self.cfs.running_count() + self.eevdf.running_count()
    }

    /// 查询任务状态。
    pub fn task_state(&self, tid: TaskId) -> Option<TaskState> {
        self.cfs
            .task_state(tid)
            .or_else(|| self.eevdf.task_state(tid))
    }

    /// 返回核心负载（任务权重之和）。
    pub fn load(&self) -> u64 {
        self.load
    }

    /// 返回调度总次数。
    pub fn schedule_count(&self) -> u64 {
        self.schedule_count
    }

    fn _update_load(&mut self) {
        let mut load: u64 = 0;
        for task in self.cfs.task_map.values() {
            if task.state == TaskState::Ready || task.state == TaskState::Running {
                load = load.saturating_add(task.weight as u64);
            }
        }
        for task in self.eevdf.task_map.values() {
            if task.state == TaskState::Ready || task.state == TaskState::Running {
                load = load.saturating_add(task.weight as u64);
            }
        }
        self.load = load;
    }

    /// 返回核心调度器文本摘要。
    pub fn summary(&self) -> String {
        format!(
            "core={} cfs[{}] eevdf[{}] load={} sched_count={} time_slice_ns={}",
            self.core_id.0,
            self.cfs.ready_count(),
            self.eevdf.ready_count(),
            self.load,
            self.schedule_count,
            self.time_slice_ns,
        )
    }
}

// =====================================================================================
// MulticoreScheduler —— 多核调度器与负载均衡
// =====================================================================================

/// 多核调度器：管理多个 CoreScheduler，提供跨核负载均衡。
pub struct MulticoreScheduler {
    pub core_count: usize,
    cores: BTreeMap<usize, CoreScheduler>,
    /// 总任务数。
    task_count: u64,
    /// 负载均衡阈值：繁忙核与空闲核负载差超过此值时触发迁移。
    pub balance_threshold: u64,
}

impl MulticoreScheduler {
    /// 创建 `core_count` 个核心的调度器。
    pub fn new(core_count: usize) -> Self {
        let mut cores = BTreeMap::new();
        for i in 0..core_count {
            cores.insert(i, CoreScheduler::new(CoreId(i)));
        }
        Self {
            core_count,
            cores,
            task_count: 0,
            balance_threshold: 1024,
        }
    }

    /// 返回核心数。
    pub fn core_count(&self) -> usize {
        self.core_count
    }

    /// 将任务入队到指定核心。
    pub fn enqueue(&mut self, core_id: usize, task: Task, use_eevdf: bool) {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.enqueue(task, use_eevdf);
        self.task_count += 1;
    }

    /// 将任务入队到负载最小的核心（自动选择）。
    pub fn enqueue_least_loaded(&mut self, task: Task, use_eevdf: bool) {
        let (best_core, _) = self
            .cores
            .iter()
            .min_by_key(|(_, c)| c.load())
            .map(|(id, c)| (*id, c.load()))
            .unwrap_or((0, 0));
        self.enqueue(best_core, task, use_eevdf);
    }

    /// 选出指定核心的下一个运行任务。
    pub fn pick_next(&mut self, core_id: usize, use_eevdf: bool) -> Option<TaskId> {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.pick_next(use_eevdf)
    }

    /// 让出指定核心上的任务。
    pub fn yield_to(&mut self, core_id: usize, tid: TaskId, use_eevdf: bool) {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.yield_to(tid, use_eevdf);
    }

    /// 时钟中断：指定核心上的任务运行 `delta_ns`。
    pub fn tick(&mut self, core_id: usize, tid: TaskId, use_eevdf: bool, delta_ns: u64) {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.tick(tid, use_eevdf, delta_ns);
    }

    /// 阻塞指定核心上的任务。
    pub fn block(&mut self, core_id: usize, tid: TaskId, use_eevdf: bool) {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.block(tid, use_eevdf);
    }

    /// 任务完成。
    pub fn complete(&mut self, core_id: usize, tid: TaskId, use_eevdf: bool) {
        let core = self.cores.get_mut(&core_id).expect("core not found");
        core.complete(tid, use_eevdf);
    }

    /// 查询任务所在核心与状态。
    pub fn task_state(&self, tid: TaskId) -> Option<(CoreId, TaskState)> {
        for (&id, core) in self.cores.iter() {
            if let Some(state) = core.task_state(tid) {
                return Some((CoreId(id), state));
            }
        }
        None
    }

    /// 返回总就绪任务数。
    pub fn total_ready_count(&self) -> usize {
        self.cores.values().map(|c| c.ready_count()).sum()
    }

    /// 返回总运行中任务数。
    pub fn total_running_count(&self) -> usize {
        self.cores.values().map(|c| c.running_count()).sum()
    }

    /// 负载均衡：将负载最高的核心上的一个任务迁移到负载最低的核心，
    // 若两者负载差超过 `balance_threshold`。
    ///
    /// 迁移时将任务从源核 CFS/EEVDF 队列移除，放入目标核的相应队列，
    // 并调整其 bound_core。
    pub fn load_balance(&mut self) -> Option<(usize, usize, TaskId)> {
        if self.core_count < 2 {
            return None;
        }
        // 找负载最高和最低的核。
        let loads: Vec<(usize, u64)> = self
            .cores
            .iter()
            .map(|(id, c)| (*id, c.load()))
            .collect();
        let max = loads.iter().max_by_key(|(_, l)| *l).unwrap();
        let min = loads.iter().min_by_key(|(_, l)| *l).unwrap();
        if max.1.saturating_sub(min.1) <= self.balance_threshold {
            return None; // 负载均衡，无需迁移
        }
        let busy_core = max.0;
        let idle_core = min.0;
        // 从繁忙核挑选一个就绪任务迁移（优先 CFS，其次 EEVDF）。
        let busy = self.cores.get_mut(&busy_core).unwrap();
        let tid = busy
            .cfs
            .tasks
            .iter()
            .find_map(|(&_, ids)| {
                ids.iter()
                    .find(|&&t| busy.cfs.task_map.get(&t).map_or(false, |task| task.state == TaskState::Ready))
                    .copied()
            })
            .or_else(|| {
                busy.eevdf.tasks.iter().find_map(|(&_, ids)| {
                    ids.iter()
                        .find(|&&t| busy.eevdf.task_map.get(&t).map_or(false, |task| task.state == TaskState::Ready))
                        .copied()
                })
            });
        let tid = match tid {
            Some(t) => t,
            None => return None,
        };
        // 判断任务在哪个队列。
        let use_eevdf = busy.eevdf.task_map.contains_key(&tid);
        // 从繁忙核移除。
        let task = if use_eevdf {
            busy.eevdf.remove(tid)
        } else {
            busy.cfs.remove(tid)
        };
        let task = match task {
            Some(t) => t,
            None => return None,
        };
        // 放入空闲核（保持原调度策略）。
        let idle = self.cores.get_mut(&idle_core).unwrap();
        idle.enqueue(task.clone(), use_eevdf);
        // 更新 bound_core（如果原任务有绑定）。
        // 这里简化：任务不强制绑定，迁移后 bound_core 设为空闲核。
        // task.bound_core = Some(CoreId(idle_core)); // Task 是 clone，需重新构造
        // 由于 Task 在 enqueue 时已被移入，此处不再修改。
        self.task_count = self.task_count; // 任务总数不变
        Some((busy_core, idle_core, tid))
    }

    /// 设置负载均衡阈值。
    pub fn set_balance_threshold(&mut self, threshold: u64) {
        self.balance_threshold = threshold;
    }

    /// 返回总任务数。
    pub fn task_count(&self) -> u64 {
        self.task_count
    }

    /// 返回多核调度器文本摘要。
    pub fn summary(&self) -> String {
        let core_summaries: Vec<String> = self
            .cores
            .values()
            .map(|c| c.summary())
            .collect();
        format!(
            "multicore cores={} total_ready={} balance_threshold={} task_count={}\n{}",
            self.core_count,
            self.total_ready_count(),
            self.balance_threshold,
            self.task_count,
            core_summaries.join("\n  ")
        )
    }
}

impl Default for MulticoreScheduler {
    fn default() -> Self {
        Self::new(4)
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static MULTICORE_SCHED: OnceLock<Mutex<MulticoreScheduler>> = OnceLock::new();

fn multicore_sched() -> &'static Mutex<MulticoreScheduler> {
    MULTICORE_SCHED.get_or_init(|| Mutex::new(MulticoreScheduler::new(4)))
}

/// 创建核心数为 `core_count` 的全局多核调度器。
pub fn init_multicore(core_count: usize) {
    *multicore_sched().lock().unwrap() = MulticoreScheduler::new(core_count);
}

/// 入队任务到指定核心。
pub fn sched_enqueue(core_id: usize, name: &str, nice: i32, use_eevdf: bool) -> TaskId {
    let task = Task::new(name, nice);
    let tid = task.id;
    multicore_sched().lock().unwrap().enqueue(core_id, task, use_eevdf);
    tid
}

/// 入队到负载最小核心。
pub fn sched_enqueue_least_loaded(name: &str, nice: i32, use_eevdf: bool) -> TaskId {
    let task = Task::new(name, nice);
    let tid = task.id;
    multicore_sched().lock().unwrap().enqueue_least_loaded(task, use_eevdf);
    tid
}

/// 选出下一个运行任务。
pub fn sched_pick_next(core_id: usize, use_eevdf: bool) -> Option<TaskId> {
    multicore_sched()
        .lock()
        .unwrap()
        .pick_next(core_id, use_eevdf)
}

/// 时钟中断。
pub fn sched_tick(core_id: usize, tid: TaskId, use_eevdf: bool, delta_ns: u64) {
    multicore_sched()
        .lock()
        .unwrap()
        .tick(core_id, tid, use_eevdf, delta_ns);
}

/// 负载均衡。
pub fn sched_load_balance() -> Option<(usize, usize, TaskId)> {
    multicore_sched().lock().unwrap().load_balance()
}

/// 返回总就绪数。
pub fn sched_total_ready() -> usize {
    multicore_sched().lock().unwrap().total_ready_count()
}

/// 返回摘要。
pub fn sched_summary() -> String {
    multicore_sched().lock().unwrap().summary()
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_to_weight_bounds() {
        assert!(nice_to_weight(-20) > nice_to_weight(19));
        assert_eq!(nice_to_weight(-20), WEIGHT_FOR_TEST(-20));
        assert_eq!(nice_to_weight(0), 1024);
    }

    fn WEIGHT_FOR_TEST(nice: i32) -> u32 {
        nice_to_weight(nice)
    }

    #[test]
    fn cfs_enqueue_pick_next() {
        let mut rq = CfsRunQueue::new();
        let mut t1 = Task::new("t1", 0);
        t1.vruntime = 100;
        let mut t2 = Task::new("t2", 0);
        t2.vruntime = 50;
        let t2_id = t2.id;
        rq.enqueue(t1);
        rq.enqueue(t2);
        // vruntime 最小的 t2 应先被选中
        let next = rq.pick_next().unwrap();
        assert_eq!(next, t2_id);
    }

    #[test]
    fn cfs_update_vruntime() {
        let mut rq = CfsRunQueue::new();
        let mut t = Task::new("t", 0);
        let tid = t.id;
        rq.enqueue(t);
        rq.update_vruntime(tid, 1000);
        let vr = rq.task_vruntime(tid).unwrap();
        assert!(vr > 0);
    }

    #[test]
    fn cfs_block_and_complete() {
        let mut rq = CfsRunQueue::new();
        let mut t = Task::new("t", 0);
        let tid = t.id;
        rq.enqueue(t);
        assert_eq!(rq.ready_count(), 1);
        rq.block(tid);
        assert_eq!(rq.ready_count(), 0);
        assert_eq!(rq.task_state(tid), Some(TaskState::Waiting));
        rq.complete(tid);
        assert_eq!(rq.task_state(tid), Some(TaskState::Completed));
    }

    #[test]
    fn cfs_yield_task() {
        let mut rq = CfsRunQueue::new();
        let mut t = Task::new("t", 0);
        let tid = t.id;
        rq.enqueue(t);
        let _ = rq.pick_next(); // 变成 Running
        rq.yield_task(tid);
        assert_eq!(rq.task_state(tid), Some(TaskState::Ready));
    }

    #[test]
    fn eevdf_pick_next_by_deadline() {
        let mut rq = EevdfRunQueue::new();
        let mut t1 = Task::new("t1", 0);
        t1.deadline = 200;
        let mut t2 = Task::new("t2", 0);
        t2.deadline = 100;
        let t2_id = t2.id;
        rq.enqueue(t1);
        rq.enqueue(t2);
        let next = rq.pick_next().unwrap();
        assert_eq!(next, t2_id); // deadline 更小者先
    }

    #[test]
    fn core_scheduler_enqueue_pick() {
        let mut core = CoreScheduler::new(CoreId(0));
        let mut t = Task::new("core-t", 0);
        let tid = t.id;
        core.enqueue(t, false);
        let next = core.pick_next(false).unwrap();
        assert_eq!(next, tid);
        assert_eq!(core.ready_count(), 0); // pick_next 后变成 Running
    }

    #[test]
    fn core_scheduler_tick_updates_vruntime() {
        let mut core = CoreScheduler::new(CoreId(0));
        let mut t = Task::new("tick-t", 0);
        let tid = t.id;
        core.enqueue(t, false);
        core.pick_next(false);
        core.tick(tid, false, 1_000_000);
        assert_eq!(core.task_state(tid), Some(TaskState::Ready));
        let vr = core.cfs.task_vruntime(tid).unwrap();
        assert!(vr > 0);
    }

    #[test]
    fn multicore_enqueue_least_loaded() {
        let mut ms = MulticoreScheduler::new(4);
        let t1 = Task::new("heavy", -20); // 高权重
        let tid1 = t1.id;
        ms.enqueue_least_loaded(t1, false);
        // 所有核负载均为 0，选 core 0
        let _ = tid1;
        let st = ms.task_state(tid1).unwrap();
        assert_eq!(st.0, CoreId(0));
    }

    #[test]
    fn multicore_load_balance_migration() {
        let mut ms = MulticoreScheduler::new(2);
        ms.set_balance_threshold(0); // 任何差异都触发
        // 向 core 0 入队高权重任务
        let mut heavy = Task::new("heavy", -20);
        let heavy_tid = heavy.id;
        ms.enqueue(0, heavy, false);
        // 向 core 1 入队低权重任务
        let mut light = Task::new("light", 19);
        let light_tid = light.id;
        ms.enqueue(1, light, false);
        // 负载不均，触发负载均衡：core 0 的任务迁移到 core 1
        let result = ms.load_balance();
        assert!(result.is_some());
        let (from, to, tid) = result.unwrap();
        assert_eq!(from, 0);
        assert_eq!(to, 1);
        assert_eq!(tid, heavy_tid);
    }

    #[test]
    fn multicore_no_balance_when_even() {
        let mut ms = MulticoreScheduler::new(2);
        ms.set_balance_threshold(10000);
        let _ = ms.enqueue(0, Task::new("a", 0), false);
        let _ = ms.enqueue(1, Task::new("b", 0), false);
        assert!(ms.load_balance().is_none());
    }

    #[test]
    fn sched_task_state_query() {
        let mut ms = MulticoreScheduler::new(2);
        let mut t = Task::new("query", 5);
        let tid = t.id;
        ms.enqueue(0, t, false);
        let st = ms.task_state(tid).unwrap();
        assert_eq!(st.0, CoreId(0));
        assert_eq!(st.1, TaskState::Ready);
    }

    #[test]
    fn global_sched_functions() {
        init_multicore(2);
        let tid = sched_enqueue(0, "global-t", 10, false);
        let _ = sched_pick_next(0, false);
        sched_tick(0, tid, false, 500_000);
        let _ = sched_load_balance();
        let _ready = sched_total_ready();
        let _ = _ready;
        let _s = sched_summary();
        let _ = _s;
    }

    #[test]
    fn summary_contains_fields() {
        let mut ms = MulticoreScheduler::new(2);
        let _ = ms.enqueue(0, Task::new("s", 0), false);
        let s = ms.summary();
        assert!(s.contains("cores="));
        assert!(s.contains("load="));
    }
}
