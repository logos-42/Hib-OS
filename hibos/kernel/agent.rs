//! 多子意识编排 —— `agent://orchestrator` 的真实后端。
//!
//! 之前这两个通道只是注册了名字，`sense`/`act` 里没有任何实现。
//! 现在每个子智能体是一个**真执行体**（`exec::spawn` 的真 OS 线程），
//! 有独立邮箱、独立状态、独立结果，父体可以 spawn / 派任务 / 广播 / 同步 / 回收。
//!
//! AI 原生点：
//! * 子体的等待是**事件驱动**的（邮箱键），不是轮询。
//! * 子体的任务只能通过**通道**（sense/act）作用于世界，不允许直接改内核状态 ——
//!   也就是说「能力边界 = 通道边界」。
//! * `barrier` 用来做**认知对齐**：多个子体在动手前对齐到同一个世界版本(epoch)。
//! * 子体禁止再操作 `agent://` 自身，避免递归编排导致的死锁。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::exec::{self, ExecId};
use crate::monitor::get_time_ns;
use crate::scheduler::CpuPriority;
use crate::scheduler_domains::SchedDomain;
use crate::sense;
use crate::sync::Barrier;

/// 子智能体标识。
pub type AgentId = u64;

/// 子智能体状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    /// 空闲，等邮箱。
    Idle,
    /// 正在执行任务。
    Working,
    /// 已停止（线程退出）。
    Stopped,
}

impl AgentState {
    pub fn name(self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Stopped => "stopped",
        }
    }
}

/// 一条已完成的任务记录。
#[derive(Clone, Debug)]
pub struct TaskRecord {
    pub agent: String,
    pub agent_id: AgentId,
    pub payload: String,
    pub ok: bool,
    pub result: String,
    pub epoch_seen: u64,
    pub elapsed_ns: u64,
}

struct AgentEntry {
    id: AgentId,
    name: String,
    role: String,
    state: AgentState,
    exec_id: ExecId,
    priority: CpuPriority,
    /// 所属调度域（None 表示全局域）。
    #[allow(dead_code)]
    domain: Option<SchedDomain>,
    mailbox: VecDeque<String>,
    done: u64,
    failed: u64,
    stop: bool,
    born_ns: u128,
    parent: Option<AgentId>,
    last_result: Option<String>,
    epoch_seen: u64,
}

struct Orchestrator {
    agents: BTreeMap<AgentId, AgentEntry>,
    by_name: BTreeMap<String, AgentId>,
    next_id: AgentId,
    barriers: BTreeMap<String, Arc<Barrier>>,
    results: VecDeque<TaskRecord>,
    spawned: u64,
    completed: u64,
    failed: u64,
    max_results: usize,
}

impl Orchestrator {
    fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            by_name: BTreeMap::new(),
            next_id: 1,
            barriers: BTreeMap::new(),
            results: VecDeque::new(),
            spawned: 0,
            completed: 0,
            failed: 0,
            max_results: 64,
        }
    }

    fn push_result(&mut self, rec: TaskRecord) {
        if rec.ok {
            self.completed += 1;
        } else {
            self.failed += 1;
        }
        self.results.push_back(rec);
        while self.results.len() > self.max_results {
            self.results.pop_front();
        }
    }
}

static ORCH: OnceLock<Mutex<Orchestrator>> = OnceLock::new();

fn orch() -> &'static Mutex<Orchestrator> {
    ORCH.get_or_init(|| Mutex::new(Orchestrator::new()))
}

fn mailbox_key(id: AgentId) -> String {
    format!("agent://mailbox/{}", id)
}

fn done_key(id: AgentId) -> String {
    format!("agent://done/{}", id)
}

/// 派生一个子智能体（真线程 + 真邮箱）。
pub fn spawn(name: &str, role: &str, priority: CpuPriority) -> Result<AgentId, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("用法: act agent://orchestrator spawn <名称> [角色]".to_string());
    }
    let parent_agent = {
        // 若当前执行体本身是一个子智能体，则记录编排血缘。
        let me = exec::current();
        let o = orch().lock().unwrap();
        o.agents
            .values()
            .find(|a| a.exec_id != 0 && Some(a.exec_id) == me)
            .map(|a| a.id)
    };
    let id = {
        let mut o = orch().lock().unwrap();
        if o.by_name.contains_key(name) {
            return Err(format!("子智能体 {} 已存在", name));
        }
        let id = o.next_id;
        o.next_id += 1;
        o.spawned += 1;
        o.by_name.insert(name.to_string(), id);
        o.agents.insert(
            id,
            AgentEntry {
                id,
                name: name.to_string(),
                role: if role.is_empty() {
                    "worker".to_string()
                } else {
                    role.to_string()
                },
                state: AgentState::Idle,
                exec_id: 0,
                priority,
                domain: None,
                mailbox: VecDeque::new(),
                done: 0,
                failed: 0,
                stop: false,
                born_ns: get_time_ns() as u128,
                parent: parent_agent,
                last_result: None,
                epoch_seen: 0,
            },
        );
        id
    };

    let mb = mailbox_key(id);
    let exec_id = exec::spawn(&format!("agent:{}", name), priority, move || {
        agent_loop(id, &mb);
    });

    let mut o = orch().lock().unwrap();
    if let Some(a) = o.agents.get_mut(&id) {
        a.exec_id = exec_id;
    }
    Ok(id)
}

/// 派生一个归属指定调度域的子智能体（真线程 + 真邮箱）。
pub fn spawn_in_domain(
    name: &str,
    role: &str,
    priority: CpuPriority,
    domain: SchedDomain,
) -> Result<AgentId, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("用法: act agent://orchestrator spawn <名称> [角色]".to_string());
    }
    let parent_agent = {
        let me = exec::current();
        let o = orch().lock().unwrap();
        o.agents
            .values()
            .find(|a| a.exec_id != 0 && Some(a.exec_id) == me)
            .map(|a| a.id)
    };
    let id = {
        let mut o = orch().lock().unwrap();
        if o.by_name.contains_key(name) {
            return Err(format!("子智能体 {} 已存在", name));
        }
        let id = o.next_id;
        o.next_id += 1;
        o.spawned += 1;
        o.by_name.insert(name.to_string(), id);
        o.agents.insert(
            id,
            AgentEntry {
                id,
                name: name.to_string(),
                role: if role.is_empty() {
                    "worker".to_string()
                } else {
                    role.to_string()
                },
                state: AgentState::Idle,
                exec_id: 0,
                priority,
                domain: Some(domain),
                mailbox: VecDeque::new(),
                done: 0,
                failed: 0,
                stop: false,
                born_ns: get_time_ns() as u128,
                parent: parent_agent,
                last_result: None,
                epoch_seen: 0,
            },
        );
        id
    };

    let mb = mailbox_key(id);
    let exec_id = exec::spawn_in_domain(&format!("agent:{}", name), priority, domain, move || {
        agent_loop(id, &mb);
    });

    let mut o = orch().lock().unwrap();
    if let Some(a) = o.agents.get_mut(&id) {
        a.exec_id = exec_id;
    }
    Ok(id)
}

/// 子智能体主循环：等邮箱 → 执行 → 记录 → 再等。没有轮询主循环。
fn agent_loop(id: AgentId, mailbox: &str) {
    loop {
        exec::checkpoint();
        let (task, stopped) = {
            let mut o = orch().lock().unwrap();
            let Some(a) = o.agents.get_mut(&id) else {
                return;
            };
            if a.stop {
                a.state = AgentState::Stopped;
                (None, true)
            } else {
                match a.mailbox.pop_front() {
                    Some(t) => {
                        a.state = AgentState::Working;
                        (Some(t), false)
                    }
                    None => {
                        a.state = AgentState::Idle;
                        (None, false)
                    }
                }
            }
        };
        if stopped {
            break;
        }
        match task {
            Some(payload) => {
                let began = get_time_ns();
                let (ok, result, epoch_seen) = run_task(&payload);
                let elapsed = get_time_ns().saturating_sub(began);
                let (name, aid) = {
                    let mut o = orch().lock().unwrap();
                    let rec = TaskRecord {
                        agent: o
                            .agents
                            .get(&id)
                            .map(|a| a.name.clone())
                            .unwrap_or_default(),
                        agent_id: id,
                        payload: payload.clone(),
                        ok,
                        result: result.clone(),
                        epoch_seen,
                        elapsed_ns: elapsed,
                    };
                    o.push_result(rec);
                    if let Some(a) = o.agents.get_mut(&id) {
                        a.state = AgentState::Idle;
                        if ok {
                            a.done += 1;
                        } else {
                            a.failed += 1;
                        }
                        a.last_result = Some(result.clone());
                        a.epoch_seen = epoch_seen;
                        (a.name.clone(), a.id)
                    } else {
                        return;
                    }
                };
                let _ = name;
                exec::signal(&done_key(aid));
            }
            None => {
                exec::block_on(mailbox, Some(Duration::from_millis(200)));
            }
        }
    }
    exec::signal(&done_key(id));
}

/// 执行一条任务。任务只能通过通道作用于世界。
///
/// 支持的指令：
/// * `sense <通道>`
/// * `act <通道> <负载...>`
/// * `barrier <名字> <人数> [超时ms]` —— 认知对齐，返回对齐后的 epoch
/// * `sleep <ms>`
/// * `echo <文本>`
fn run_task(payload: &str) -> (bool, String, u64) {
    let mut it = payload.splitn(2, ' ');
    let verb = it.next().unwrap_or("echo");
    let rest = it.next().unwrap_or("").trim();

    match verb {
        "sense" => {
            if rest.is_empty() {
                return (false, "用法: sense <通道>".to_string(), 0);
            }
            if rest.starts_with("agent://") {
                return (false, "子体不允许递归操作 agent://".to_string(), 0);
            }
            let ep = crate::lease::epoch(rest);
            match sense::probe(rest) {
                Some(t) => (true, t, ep),
                None => (false, format!("通道 {} 没有 probe 后端", rest), ep),
            }
        }
        "act" => {
            if rest.is_empty() {
                return (false, "用法: act <通道> <负载>".to_string(), 0);
            }
            if rest.starts_with("agent://") {
                return (false, "子体不允许递归操作 agent://".to_string(), 0);
            }
            let (ch, body) = match rest.split_once(' ') {
                Some((c, b)) => (c, b),
                None => (rest, ""),
            };
            let ep = crate::lease::epoch(ch);
            match sense::act(ch, body) {
                Some(Ok(t)) => (true, t, ep),
                Some(Err(e)) => (false, e, ep),
                None => (false, format!("通道 {} 没有 act 后端", ch), ep),
            }
        }
        "barrier" => {
            let mut parts = rest.split_whitespace();
            let name = parts.next().unwrap_or("default").to_string();
            let n = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let timeout = parts
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_millis)
                .or(Some(Duration::from_secs(5)));
            let b = {
                let mut o = orch().lock().unwrap();
                o.barriers
                    .entry(name.clone())
                    .or_insert_with(|| Arc::new(Barrier::new(n)))
                    .clone()
            };
            match b.arrive_and_wait(timeout) {
                Some(epoch) => (true, format!("barrier {} aligned epoch={}", name, epoch), epoch),
                None => (
                    false,
                    format!("barrier {} 超时：世界观不一致", name),
                    b.epoch(),
                ),
            }
        }
        "sleep" => {
            let ms = rest.parse::<u64>().unwrap_or(10).min(60_000);
            let began = get_time_ns();
            exec::block_on("agent://sleep", Some(Duration::from_millis(ms)));
            (
                true,
                format!("slept {}ms", (get_time_ns() - began) / 1_000_000),
                0,
            )
        }
        _ => (true, payload.to_string(), 0),
    }
}

/// 向某个子体投递任务。
pub fn submit(name_or_id: &str, payload: &str) -> Result<String, String> {
    let id = resolve(name_or_id)?;
    let (name, depth) = {
        let mut o = orch().lock().unwrap();
        let Some(a) = o.agents.get_mut(&id) else {
            return Err(format!("子智能体 {} 不存在", name_or_id));
        };
        if a.state == AgentState::Stopped {
            return Err(format!("子智能体 {} 已停止", a.name));
        }
        a.mailbox.push_back(payload.to_string());
        (a.name.clone(), a.mailbox.len())
    };
    exec::signal(&mailbox_key(id));
    Ok(format!("已投递任务给 {} (队列 {}): {}", name, depth, payload))
}

/// 广播任务给所有未停止的子体。
pub fn broadcast(payload: &str) -> Result<String, String> {
    let targets: Vec<(AgentId, String)> = {
        let mut o = orch().lock().unwrap();
        let mut t = Vec::new();
        for a in o.agents.values_mut() {
            if a.state != AgentState::Stopped {
                a.mailbox.push_back(payload.to_string());
                t.push((a.id, a.name.clone()));
            }
        }
        t
    };
    for (id, _) in &targets {
        exec::signal(&mailbox_key(*id));
    }
    Ok(format!(
        "已广播给 {} 个子体: {}",
        targets.len(),
        targets
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>()
            .join(",")
    ))
}

/// 等待某个子体空闲（当前任务完成）。
///
/// 「空闲」的定义是 **邮箱为空且不在 Working**：只判 `state != Working` 会在线程
/// 还没来得及取走任务时提前返回（提交与执行之间的竞态）。
pub fn wait_idle(name_or_id: &str, timeout: Option<Duration>) -> Result<String, String> {
    let id = resolve(name_or_id)?;
    let began = std::time::Instant::now();
    loop {
        {
            let o = orch().lock().unwrap();
            match o.agents.get(&id) {
                Some(a) if a.state != AgentState::Working && a.mailbox.is_empty() => {
                    return Ok(format!(
                        "{} 已空闲 state={} done={} failed={} last={:?}",
                        a.name,
                        a.state.name(),
                        a.done,
                        a.failed,
                        a.last_result
                    ))
                }
                Some(_) => {}
                None => return Err(format!("子智能体 {} 不存在", name_or_id)),
            }
        }
        if let Some(t) = timeout {
            if began.elapsed() >= t {
                return Err(format!("等待 {} 空闲超时", name_or_id));
            }
        }
        exec::block_on(&done_key(id), Some(Duration::from_millis(50)));
    }
}

/// 停止并回收一个子体。
pub fn stop(name_or_id: &str) -> Result<String, String> {
    let id = resolve(name_or_id)?;
    let (exec_id, name) = {
        let mut o = orch().lock().unwrap();
        let Some(a) = o.agents.get_mut(&id) else {
            return Err(format!("子智能体 {} 不存在", name_or_id));
        };
        a.stop = true;
        a.mailbox.clear();
        (a.exec_id, a.name.clone())
    };
    exec::signal(&mailbox_key(id));
    let code = exec::join(exec_id, Some(Duration::from_secs(2)));
    let mut o = orch().lock().unwrap();
    if let Some(a) = o.agents.get_mut(&id) {
        a.state = AgentState::Stopped;
    }
    Ok(format!("{} 已停止 (exit={:?})", name, code))
}

fn resolve(name_or_id: &str) -> Result<AgentId, String> {
    let o = orch().lock().unwrap();
    if let Ok(id) = name_or_id.parse::<AgentId>() {
        if o.agents.contains_key(&id) {
            return Ok(id);
        }
    }
    o.by_name
        .get(name_or_id)
        .copied()
        .ok_or_else(|| format!("子智能体 {} 不存在", name_or_id))
}

/// `act agent://orchestrator <...>` 的入口。
pub fn dispatch(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(2, ' ');
    let verb = it.next().unwrap_or("list");
    let rest = it.next().unwrap_or("").trim();
    match verb {
        "spawn" => {
            let mut p = rest.splitn(2, ' ');
            let name = p.next().unwrap_or("");
            let role = p.next().unwrap_or("worker");
            spawn(name, role, CpuPriority::Medium).map(|id| {
                format!("已派生子智能体 {} (agent={}) exec 已启动", name, id)
            })
        }
        "task" => {
            let (name, body) = rest
                .split_once(' ')
                .ok_or("用法: task <名称> <sense|act|barrier|sleep ...>")?;
            submit(name, body)
        }
        "broadcast" => broadcast(rest),
        "wait" => wait_idle(rest, Some(Duration::from_secs(5))),
        "stop" => stop(rest),
        "results" => Ok(results()),
        "barriers" => Ok(barrier_list()),
        "list" | "" => Ok(list()),
        _ => Err(format!(
            "未知指令 {}。可用: spawn/task/broadcast/wait/stop/results/barriers/list",
            verb
        )),
    }
}

/// 汇总状态。
pub fn status() -> String {
    let o = orch().lock().unwrap();
    let mut idle = 0;
    let mut working = 0;
    let mut stopped = 0;
    for a in o.agents.values() {
        match a.state {
            AgentState::Idle => idle += 1,
            AgentState::Working => working += 1,
            AgentState::Stopped => stopped += 1,
        }
    }
    format!(
        "agents={} idle={} working={} stopped={} spawned={} completed={} failed={} barriers={} results={}",
        o.agents.len(),
        idle,
        working,
        stopped,
        o.spawned,
        o.completed,
        o.failed,
        o.barriers.len(),
        o.results.len()
    )
}

/// 子体列表。
pub fn list() -> String {
    let o = orch().lock().unwrap();
    if o.agents.is_empty() {
        return "（无子智能体）".to_string();
    }
    let now = get_time_ns() as u128;
    let mut out = String::new();
    for a in o.agents.values() {
        // 存活时长（出生时间来自 born_ns；父体用于展示编排血缘）。
        let age_ms = now.saturating_sub(a.born_ns) / 1_000_000;
        let parent = a
            .parent
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "agent={:<4} {:<16} role={:<10} state={:<8} exec={:<4} prio={:<8} done={:<3} failed={:<3} epoch={:<3} age={}ms parent={}\n",
            a.id,
            a.name,
            a.role,
            a.state.name(),
            a.exec_id,
            a.priority.name(),
            a.done,
            a.failed,
            a.epoch_seen,
            age_ms,
            parent
        ));
    }
    out.trim_end().to_string()
}

/// 最近的任务结果。
pub fn results() -> String {
    let o = orch().lock().unwrap();
    if o.results.is_empty() {
        return "（无任务结果）".to_string();
    }
    let mut out = String::new();
    for r in o.results.iter().rev().take(12) {
        out.push_str(&format!(
            "[{}] agent={} ok={} epoch={} {}us :: {}\n",
            r.agent,
            r.agent_id,
            r.ok,
            r.epoch_seen,
            r.elapsed_ns / 1000,
            truncate(&r.result, 80)
        ));
    }
    out.trim_end().to_string()
}

fn truncate(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn barrier_list() -> String {
    let o = orch().lock().unwrap();
    if o.barriers.is_empty() {
        return "（无屏障）".to_string();
    }
    o.barriers
        .iter()
        .map(|(k, v)| format!("{}: {}", k, v.status()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 子智能体是**全局**编排器，测试必须串行，否则会互相污染（广播计数、结果表）。
    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static G: OnceLock<Mutex<()>> = OnceLock::new();
        G.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn spawn_and_run_a_sense_task() {
        let _serial = test_guard();
        let id = spawn("ut-sense", "worker", CpuPriority::Medium).unwrap();
        submit("ut-sense", "sense gpu://vram").unwrap();
        let r = wait_idle("ut-sense", Some(Duration::from_secs(3))).unwrap();
        assert!(r.contains("idle"));
        let o = orch().lock().unwrap();
        let a = &o.agents[&id];
        assert!(a.done >= 1, "任务应成功执行: {:?}", a.last_result);
        assert!(a.last_result.as_deref().unwrap_or("").contains("total="));
        drop(o);
        stop("ut-sense").unwrap();
    }

    #[test]
    fn agent_cannot_recurse_into_agent_channels() {
        let _serial = test_guard();
        spawn("ut-recursion", "worker", CpuPriority::Low).unwrap();
        submit("ut-recursion", "act agent://orchestrator list").unwrap();
        wait_idle("ut-recursion", Some(Duration::from_secs(3))).unwrap();
        let o = orch().lock().unwrap();
        let rec = o.results.iter().rev().find(|r| r.agent == "ut-recursion").unwrap();
        assert!(!rec.ok);
        assert!(rec.result.contains("递归"));
        drop(o);
        stop("ut-recursion").unwrap();
    }

    #[test]
    fn barrier_aligns_two_agents() {
        let _serial = test_guard();
        spawn("ut-bar-a", "worker", CpuPriority::Medium).unwrap();
        spawn("ut-bar-b", "worker", CpuPriority::Medium).unwrap();
        submit("ut-bar-a", "barrier ut-align 2 2000").unwrap();
        submit("ut-bar-b", "barrier ut-align 2 2000").unwrap();
        wait_idle("ut-bar-a", Some(Duration::from_secs(3))).unwrap();
        wait_idle("ut-bar-b", Some(Duration::from_secs(3))).unwrap();
        let o = orch().lock().unwrap();
        let eps: Vec<u64> = o
            .results
            .iter()
            .filter(|r| r.agent == "ut-bar-a" || r.agent == "ut-bar-b")
            .filter(|r| r.result.contains("aligned"))
            .map(|r| r.epoch_seen)
            .collect();
        assert_eq!(eps.len(), 2, "两个子体都应完成对齐");
        assert_eq!(eps[0], eps[1], "对齐后必须看到同一个 epoch");
        drop(o);
        stop("ut-bar-a").unwrap();
        stop("ut-bar-b").unwrap();
    }

    #[test]
    fn broadcast_reaches_all_live_agents() {
        let _serial = test_guard();
        spawn("ut-bc-a", "worker", CpuPriority::Low).unwrap();
        spawn("ut-bc-b", "worker", CpuPriority::Low).unwrap();
        let msg = broadcast("echo hello").unwrap();
        assert!(msg.contains("2 个子体"));
        wait_idle("ut-bc-a", Some(Duration::from_secs(3))).unwrap();
        wait_idle("ut-bc-b", Some(Duration::from_secs(3))).unwrap();
        stop("ut-bc-a").unwrap();
        stop("ut-bc-b").unwrap();
    }

    #[test]
    fn status_and_list_are_non_empty() {
        let _serial = test_guard();
        assert!(status().contains("agents="));
        let _ = list();
        let _ = results();
        let _ = barrier_list();
    }

    #[test]
    fn duplicate_spawn_rejected() {
        let _serial = test_guard();
        spawn("ut-dup", "worker", CpuPriority::Low).unwrap();
        assert!(spawn("ut-dup", "worker", CpuPriority::Low).is_err());
        stop("ut-dup").unwrap();
    }
}
