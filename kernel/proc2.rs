//! 进程 / 子意识主体容器 —— `proc://spawn` / `proc://kill` 的后端。
//!
//! Linux 把进程看成「地址空间 + 文件描述符表 + PID」的资源容器。
//! Hib OS 把**进程 = 子意识主体**：
//! * 每个进程拥有**独立的 `wait_table` 副本**（它关心的通道事件断言）；
//! * 每个进程拥有**独立的租约集**（它当前持有哪些通道的写租约）；
//! * 进程 ID（proc-id）不是 PID，是意识主体标识；
//! * `proc://spawn <镜像>` 加载一段可调用 `sense`/`act` 的意识镜像；
//! * `proc://kill <id>` 优雅弹出：释放该进程全部租约、推进相关通道 epoch、
//!   清空其 wait_table 条目、通知等待者。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::exec::{self, ExecId};
use crate::lease;

/// 进程标识。
pub type ProcId = u64;

/// 进程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    Created,
    Running,
    Blocked,
    Suspended,
    Exited,
}

impl ProcState {
    pub fn name(self) -> &'static str {
        match self {
            ProcState::Created => "created",
            ProcState::Running => "running",
            ProcState::Blocked => "blocked",
            ProcState::Suspended => "suspended",
            ProcState::Exited => "exited",
        }
    }
}

/// 一个进程（子意识主体）。
pub struct Proc {
    pub id: ProcId,
    pub name: String,
    pub state: ProcState,
    /// 该进程派生的主执行体（真 OS 线程）。
    pub exec_id: Option<ExecId>,
    /// 独立的等待表副本：通道键 → 该进程内等待者。
    /// 与全局 `exec::wait_table` 解耦，只记录该进程关心的断言。
    pub local_waits: BTreeMap<String, VecDeque<ExecId>>,
    /// 该进程当前持有的写租约集合（通道名）。
    pub leases: BTreeSet<String>,
    /// 创建时间。
    pub created_ns: u128,
    pub exit_ns: Option<u128>,
    pub exit_code: Option<i32>,
}

/// 进程注册表。
struct ProcTable {
    procs: BTreeMap<ProcId, Proc>,
    next_id: ProcId,
}

static PROC_TABLE: OnceLock<Mutex<ProcTable>> = OnceLock::new();

fn table() -> &'static Mutex<ProcTable> {
    PROC_TABLE.get_or_init(|| {
        Mutex::new(ProcTable {
            procs: BTreeMap::new(),
            next_id: 1,
        })
    })
}

/// 派生一个新进程（子意识主体）。
///
/// `image` 是意识镜像名称，决定加载什么样的 `sense`/`act` 行为；
/// 目前内置镜像：`echo`, `sense-loop`, `act-loop`, `idle`。
pub fn spawn(image: &str) -> Result<ProcId, String> {
    let image = image.trim();
    if image.is_empty() {
        return Err("用法: proc://spawn <镜像名>".to_string());
    }

    let id = {
        let mut t = table().lock().unwrap();
        let id = t.next_id;
        t.next_id += 1;
        t.procs.insert(
            id,
            Proc {
                id,
                name: image.to_string(),
                state: ProcState::Created,
                exec_id: None,
                local_waits: BTreeMap::new(),
                leases: BTreeSet::new(),
                created_ns: crate::monitor::get_time_ns() as u128,
                exit_ns: None,
                exit_code: None,
            },
        );
        id
    };

    // 为该进程派生一个真执行体，运行镜像行为。
    let image2 = image.to_string();
    let exec_id = exec::spawn(
        &format!("proc-{}-{}", image, id),
        crate::scheduler::CpuPriority::Medium,
        move || {
            run_proc_image(&image2, id);
        },
    );

    {
        let mut t = table().lock().unwrap();
        if let Some(p) = t.procs.get_mut(&id) {
            p.state = ProcState::Running;
            p.exec_id = Some(exec_id);
        }
    }

    Ok(id)
}

/// 优雅弹出进程：释放全部租约、推进通道 epoch、清空本地等待表。
pub fn kill(id: ProcId) -> Result<String, String> {
    let (exec_id_opt, leases) = {
        let mut t = table().lock().unwrap();
        let p = match t.procs.get_mut(&id) {
            Some(p) => p,
            None => return Err(format!("进程 {} 不存在", id)),
        };
        if p.state == ProcState::Exited {
            return Err(format!("进程 {} 已退出", id));
        }
        let leases: Vec<String> = p.leases.iter().cloned().collect();
        let exec_id_opt = p.exec_id;
        (exec_id_opt, leases)
    };

    // 1. 释放该进程持有的全部写租约，并推进相关通道 epoch。
    for ch in &leases {
        let _ = lease::release(ch, exec_id_opt.unwrap_or(0));
    }

    // 2. 再次锁表：标记退出、清空本地等待表、清空租约记录。
    {
        let mut t = table().lock().unwrap();
        if let Some(p) = t.procs.get_mut(&id) {
            p.state = ProcState::Exited;
            p.exit_ns = Some(crate::monitor::get_time_ns() as u128);
            p.exit_code = Some(0);
            p.local_waits.clear();
            p.leases.clear();
        }
    }

    // 3. 结束该进程的执行体（让它从 block_on 中返回并退出）。
    if let Some(eid) = exec_id_opt {
        let _ = exec::resume(eid);
    }

    Ok(format!(
        "进程 {} 已弹出，释放租约 {} 条",
        id,
        leases.len()
    ))
}

/// 列出所有进程。
pub fn list() -> String {
    let t = table().lock().unwrap();
    if t.procs.is_empty() {
        return "（无进程）".to_string();
    }
    let mut out = String::new();
    for p in t.procs.values() {
        out.push_str(&format!(
            "proc={:<4} {:<20} {:<10} exec={:<4} leases={} waits={}\n",
            p.id,
            p.name,
            p.state.name(),
            p.exec_id.map(|e| e.to_string()).unwrap_or_else(|| "-".into()),
            p.leases.len(),
            p.local_waits.values().map(|v| v.len()).sum::<usize>()
        ));
    }
    out.trim_end().to_string()
}

/// 查询指定进程的本地等待表。
pub fn wait_table(id: ProcId) -> Result<String, String> {
    let t = table().lock().unwrap();
    let p = match t.procs.get(&id) {
        Some(p) => p,
        None => return Err(format!("进程 {} 不存在", id)),
    };
    if p.local_waits.is_empty() {
        return Ok(format!("进程 {} 本地等待表为空", id));
    }
    let mut out = String::new();
    for (k, v) in &p.local_waits {
        out.push_str(&format!("  {} <- {:?}\n", k, v));
    }
    Ok(out.trim_end().to_string())
}

/// 给进程注册一个本地等待断言（供 `block_on` 通道事件使用）。
pub fn note_wait(id: ProcId, key: &str, exec_id: ExecId) {
    let mut t = table().lock().unwrap();
    if let Some(p) = t.procs.get_mut(&id) {
        p.local_waits.entry(key.to_string()).or_default().push_back(exec_id);
    }
}

/// 从进程本地等待表移除一个断言。
pub fn note_unwait(id: ProcId, key: &str, exec_id: ExecId) {
    let mut t = table().lock().unwrap();
    if let Some(p) = t.procs.get_mut(&id) {
        if let Some(v) = p.local_waits.get_mut(key) {
            v.retain(|x| *x != exec_id);
            if v.is_empty() {
                p.local_waits.remove(key);
            }
        }
    }
}

/// 给进程授予一个通道的写租约记录。
pub fn grant_lease(id: ProcId, channel: &str) {
    let mut t = table().lock().unwrap();
    if let Some(p) = t.procs.get_mut(&id) {
        p.leases.insert(channel.to_string());
    }
}

/// 从进程撤销一个写租约记录。
pub fn revoke_lease(id: ProcId, channel: &str) {
    let mut t = table().lock().unwrap();
    if let Some(p) = t.procs.get_mut(&id) {
        p.leases.remove(channel);
    }
}

/// 查询进程状态。
pub fn status(id: ProcId) -> Result<String, String> {
    let t = table().lock().unwrap();
    match t.procs.get(&id) {
        Some(p) => Ok(format!(
            "proc={} {:<20} state={} exec={:?} leases={} waits={}",
            p.id,
            p.name,
            p.state.name(),
            p.exec_id,
            p.leases.len(),
            p.local_waits.values().map(|v| v.len()).sum::<usize>()
        )),
        None => Err(format!("进程 {} 不存在", id)),
    }
}

/// 全局统计。
pub fn stats() -> String {
    let t = table().lock().unwrap();
    let mut running = 0;
    let mut blocked = 0;
    let mut suspended = 0;
    let mut exited = 0;
    for p in t.procs.values() {
        match p.state {
            ProcState::Running => running += 1,
            ProcState::Blocked => blocked += 1,
            ProcState::Suspended => suspended += 1,
            ProcState::Exited => exited += 1,
            _ => {}
        }
    }
    format!(
        "procs={} running={} blocked={} suspended={} exited={}",
        t.procs.len(),
        running,
        blocked,
        suspended,
        exited
    )
}

fn run_proc_image(image: &str, proc_id: ProcId) {
    // 意识镜像行为：不同镜像执行不同的 sense/act 循环。
    match image {
        "echo" => {
            // 简单回声：不断 checkpoint，等待外部 act 唤醒。
            loop {
                exec::checkpoint();
                let key = format!("proc://echo/{}", proc_id);
                let _ = exec::block_on(&key, Some(Duration::from_millis(100)));
            }
        }
        "sense-loop" => {
            loop {
                let _ = crate::sense::probe("gpu://vram");
                exec::checkpoint();
                let key = format!("proc://sense/{}", proc_id);
                let _ = exec::block_on(&key, Some(Duration::from_millis(200)));
            }
        }
        "act-loop" => {
            let mut i = 0u64;
            loop {
                let _ = crate::sense::act(
                    "lease://channels",
                    &format!("commit proc://act/{} {} hello-from-proc", proc_id, i),
                );
                i += 1;
                exec::checkpoint();
                let key = format!("proc://act/{}", proc_id);
                let _ = exec::block_on(&key, Some(Duration::from_millis(500)));
            }
        }
        "idle" | _ => {
            // 空闲镜像：只 checkpoint，不占 CPU。
            loop {
                exec::checkpoint();
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_list() {
        let id = spawn("echo").unwrap();
        let s = list();
        assert!(s.contains(&format!("proc={} ", id)));
        assert!(s.contains("echo"));
        let _ = kill(id);
        let s = list();
        assert!(s.contains("exited"));
    }

    #[test]
    fn kill_releases_leases() {
        let id = spawn("echo").unwrap();
        grant_lease(id, "ctx://test");
        grant_lease(id, "mem://test");
        {
            let t = table().lock().unwrap();
            assert_eq!(t.procs[&id].leases.len(), 2);
        }
        let _ = kill(id);
        {
            let t = table().lock().unwrap();
            assert_eq!(t.procs[&id].leases.len(), 0);
            assert_eq!(t.procs[&id].state, ProcState::Exited);
        }
    }

    #[test]
    fn wait_table_note() {
        let id = spawn("echo").unwrap();
        note_wait(id, "proc://test/key", 42);
        let wt = wait_table(id).unwrap();
        assert!(wt.contains("proc://test/key"));
        note_unwait(id, "proc://test/key", 42);
        let wt = wait_table(id).unwrap();
        assert!(!wt.contains("proc://test/key"));
    }

    #[test]
    fn stats_non_empty() {
        let _ = spawn("idle").unwrap();
        let s = stats();
        assert!(s.contains("procs="));
    }
}
