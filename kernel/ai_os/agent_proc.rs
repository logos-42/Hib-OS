//! 智能体进程子系统 —— 经典「进程」的 AI 原生对应物。
//!
//! 对应《现代操作系统》第 2 章「进程与线程」：
//! * `AgentPcb` 等价于 PCB：标识、状态、优先级、模型实例、上下文窗口、能力位；
//! * 状态机：Created → Ready → Running → (Blocked) → Completed / Failed；
//! * `AgentTable` 提供派生 (spawn)、终止 (kill)、退出 (exit)、状态迁移与追踪；
//! * 通过 `agent/*` 通道暴露给 `sense`/`act`。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::scheduler::CpuPriority;

/// 智能体 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pid(pub u64);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 智能体状态（与经典进程状态一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentPcbState {
    /// 已创建，尚未就绪。
    Created,
    /// 就绪，等待 CPU。
    Ready,
    /// 运行中。
    Running,
    /// 阻塞：等待工具租约、模型实例或上下文空间。
    Blocked,
    /// 正常完成。
    Completed,
    /// 失败。
    Failed,
}

/// 智能体进程控制块。
#[derive(Clone, Debug)]
pub struct AgentPcb {
    pub pid: Pid,
    pub name: String,
    pub role: String,
    pub state: AgentPcbState,
    pub priority: CpuPriority,
    /// 绑定的模型实例。
    pub model_instance: Option<u64>,
    /// 上下文窗口 id。
    pub ctx_window: Option<u64>,
    /// 能力位。
    pub caps: Vec<String>,
    /// 阻塞原因。
    pub blocked_on: Option<String>,
    /// 已消耗 token。
    pub tokens_used: u64,
    /// 父智能体（进程层次结构）。
    pub parent: Option<Pid>,
}

/// 派生错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    DuplicateName { name: String },
    NotFound { pid: Pid },
    BadState { from: AgentPcbState, to: AgentPcbState },
    NoSuchModel,
}

/// 智能体表。
pub struct AgentTable {
    next_pid: u64,
    pcb: BTreeMap<Pid, AgentPcb>,
    name_to_pid: BTreeMap<String, Pid>,
    pub spawn_count: u64,
    pub exit_count: u64,
    pub kill_count: u64,
}

impl AgentTable {
    pub fn new() -> Self {
        Self {
            next_pid: 1,
            pcb: BTreeMap::new(),
            name_to_pid: BTreeMap::new(),
            spawn_count: 0,
            exit_count: 0,
            kill_count: 0,
        }
    }

    /// 派生一个智能体。
    pub fn spawn(
        &mut self,
        name: &str,
        role: &str,
        priority: CpuPriority,
        model_instance: Option<u64>,
    ) -> Result<Pid, AgentError> {
        if self.name_to_pid.contains_key(name) {
            return Err(AgentError::DuplicateName {
                name: name.to_string(),
            });
        }
        let pid = Pid(self.next_pid);
        self.next_pid += 1;
        self.pcb.insert(
            pid,
            AgentPcb {
                pid,
                name: name.to_string(),
                role: role.to_string(),
                state: AgentPcbState::Created,
                priority,
                model_instance,
                ctx_window: None,
                caps: vec!["fs.read".to_string(), "memory.search".to_string()],
                blocked_on: None,
                tokens_used: 0,
                parent: None,
            },
        );
        self.name_to_pid.insert(name.to_string(), pid);
        self.spawn_count += 1;
        Ok(pid)
    }

    /// 状态迁移（校验合法性）。
    pub fn set_state(&mut self, pid: Pid, to: AgentPcbState) -> Result<(), AgentError> {
        let p = self.pcb.get_mut(&pid).ok_or(AgentError::NotFound { pid })?;
        let ok = matches!(
            (p.state, to),
            (AgentPcbState::Created, AgentPcbState::Ready)
                | (AgentPcbState::Ready, AgentPcbState::Running)
                | (AgentPcbState::Running, AgentPcbState::Ready)
                | (AgentPcbState::Running, AgentPcbState::Blocked)
                | (AgentPcbState::Running, AgentPcbState::Completed)
                | (AgentPcbState::Running, AgentPcbState::Failed)
                | (AgentPcbState::Blocked, AgentPcbState::Ready)
        );
        if !ok {
            return Err(AgentError::BadState { from: p.state, to });
        }
        p.state = to;
        Ok(())
    }

    /// 阻塞。
    pub fn set_blocked(&mut self, pid: Pid, reason: &str) -> Result<(), AgentError> {
        self.set_state(pid, AgentPcbState::Blocked)?;
        if let Some(p) = self.pcb.get_mut(&pid) {
            p.blocked_on = Some(reason.to_string());
        }
        Ok(())
    }

    /// 解除阻塞。
    pub fn unblock(&mut self, pid: Pid) -> Result<(), AgentError> {
        self.set_state(pid, AgentPcbState::Ready)?;
        if let Some(p) = self.pcb.get_mut(&pid) {
            p.blocked_on = None;
        }
        Ok(())
    }

    /// 正常退出。
    pub fn exit(&mut self, pid: Pid, success: bool) -> Result<(), AgentError> {
        let p = self.pcb.get_mut(&pid).ok_or(AgentError::NotFound { pid })?;
        p.state = if success {
            AgentPcbState::Completed
        } else {
            AgentPcbState::Failed
        };
        self.exit_count += 1;
        Ok(())
    }

    /// 强制终止（等价于 SIGKILL，不校验状态）。
    pub fn kill(&mut self, pid: Pid) -> Result<(), AgentError> {
        let p = self.pcb.get_mut(&pid).ok_or(AgentError::NotFound { pid })?;
        p.state = AgentPcbState::Failed;
        self.kill_count += 1;
        Ok(())
    }

    pub fn pcb(&self, pid: Pid) -> Option<&AgentPcb> {
        self.pcb.get(&pid)
    }

    /// 按名字或 id 解析。
    pub fn resolve(&self, key: &str) -> Result<Pid, AgentError> {
        if let Some(pid) = self.name_to_pid.get(key) {
            return Ok(*pid);
        }
        key.parse::<u64>()
            .ok()
            .map(Pid)
            .filter(|p| self.pcb.contains_key(p))
            .ok_or(AgentError::NotFound { pid: Pid(0) })
    }

    /// 某状态的智能体数量。
    pub fn state_count(&self, state: AgentPcbState) -> usize {
        self.pcb.values().filter(|p| p.state == state).count()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentPcb> {
        self.pcb.values()
    }
}

impl Default for AgentTable {
    fn default() -> Self {
        Self::new()
    }
}

static AGENTS: OnceLock<Mutex<AgentTable>> = OnceLock::new();

/// 全局智能体表。
pub fn agents() -> &'static Mutex<AgentTable> {
    AGENTS.get_or_init(|| Mutex::new(AgentTable::new()))
}

fn parse_priority(s: &str) -> CpuPriority {
    match s.to_ascii_lowercase().as_str() {
        "high" => CpuPriority::High,
        "low" => CpuPriority::Low,
        _ => CpuPriority::Medium,
    }
}

/// `act agent/*` 的后端：spawn / kill / list / status / trace。
pub fn agent_proc_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mut table = agents().lock().unwrap();
    match it.next() {
        Some("spawn") => {
            let name = it.next().ok_or("用法: act agent/spawn <名字> <角色> [优先级] [模型实例]")?;
            let role = it.next().unwrap_or("general");
            let prio = parse_priority(it.next().unwrap_or("medium"));
            let model = it.next().and_then(|s| s.parse::<u64>().ok());
            let pid = table
                .spawn(name, role, prio, model)
                .map_err(|e| format!("派生失败: {:?}", e))?;
            Ok(format!("已派生智能体 pid={} name={}", pid, name))
        }
        Some("kill") => {
            let key = it.next().ok_or("用法: act agent/kill <pid|名字>")?;
            let pid = table.resolve(key).map_err(|e| format!("未知智能体: {:?}", e))?;
            table.kill(pid).map_err(|e| format!("终止失败: {:?}", e))?;
            Ok(format!("已终止 pid={}", pid))
        }
        Some("list") | None => {
            let mut out = String::from("agent 列表:\n");
            for p in table.iter() {
                out.push_str(&format!(
                    "  pid={} name={} role={} state={:?} prio={:?} tok={}\n",
                    p.pid, p.name, p.role, p.state, p.priority, p.tokens_used
                ));
            }
            Ok(out)
        }
        Some("trace") => {
            let key = it.next().ok_or("用法: act agent/trace <pid|名字>")?;
            let pid = table.resolve(key).map_err(|e| format!("未知智能体: {:?}", e))?;
            let p = table.pcb(pid).ok_or("智能体不存在")?;
            Ok(format!(
                "pid={} state={:?} tok={} model={:?} blocked_on={:?}",
                p.pid, p.state, p.tokens_used, p.model_instance, p.blocked_on
            ))
        }
        Some("status") => Ok(status()),
        Some(other) => Err(format!("未知 agent 动作: {}", other)),
    }
}

/// 初始化：派生若干系统智能体。
pub fn init() {
    let mut t = agents().lock().unwrap();
    if t.iter().next().is_some() {
        return;
    }
    let _ = t.spawn("init", "system", CpuPriority::High, None);
    let _ = t.spawn("perception", "sensor", CpuPriority::Medium, None);
    let _ = t.spawn("reasoner", "planner", CpuPriority::Medium, None);
}

/// 状态摘要。
pub fn status() -> String {
    let t = agents().lock().unwrap();
    format!(
        "agents={} ready={} running={} exit={} kill={}",
        t.iter().count(),
        t.state_count(AgentPcbState::Ready),
        t.state_count(AgentPcbState::Running),
        t.exit_count,
        t.kill_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_state_machine() {
        let mut t = AgentTable::new();
        let pid = t
            .spawn("worker-a", "coder", CpuPriority::Medium, Some(1))
            .unwrap();
        assert_eq!(t.pcb(pid).unwrap().state, AgentPcbState::Created);
        t.set_state(pid, AgentPcbState::Ready).unwrap();
        t.set_state(pid, AgentPcbState::Running).unwrap();
        t.set_blocked(pid, "等待工具租约").unwrap();
        assert_eq!(t.pcb(pid).unwrap().state, AgentPcbState::Blocked);
        t.unblock(pid).unwrap();
        assert_eq!(t.state_count(AgentPcbState::Ready), 1);
        t.exit(pid, true).unwrap();
        assert_eq!(t.pcb(pid).unwrap().state, AgentPcbState::Completed);
        assert_eq!(t.exit_count, 1);
    }

    #[test]
    fn kill_and_duplicate() {
        let mut t = AgentTable::new();
        let pid = t.spawn("worker-b", "reviewer", CpuPriority::Low, None).unwrap();
        t.kill(pid).unwrap();
        assert_eq!(t.pcb(pid).unwrap().state, AgentPcbState::Failed);
        assert_eq!(t.kill_count, 1);
        assert!(t.spawn("worker-b", "x", CpuPriority::Low, None).is_err());
    }

    #[test]
    fn resolve_by_name_and_id() {
        let mut t = AgentTable::new();
        let pid = t.spawn("resolve-me", "x", CpuPriority::High, None).unwrap();
        assert_eq!(t.resolve(&pid.to_string()).unwrap(), pid);
        assert_eq!(t.resolve("resolve-me").unwrap(), pid);
        assert!(t.resolve("nope").is_err());
    }

    #[test]
    fn act_roundtrip() {
        let mut t = agents().lock().unwrap();
        *t = AgentTable::new();
        drop(t);
        init();
        let res = agent_proc_act("spawn hello coder medium 1").unwrap();
        assert!(res.contains("已派生智能体"));
        let res = agent_proc_act("list").unwrap();
        assert!(res.contains("state="));
        let pid = res
            .lines()
            .find(|l| l.contains("name=hello"))
            .and_then(|l| l.split("pid=").nth(1))
            .and_then(|s| s.split(' ').next())
            .unwrap()
            .to_string();
        let res = agent_proc_act(&format!("trace {}", pid)).unwrap();
        assert!(res.contains("tok="));
        let res = agent_proc_act(&format!("kill {}", pid)).unwrap();
        assert!(res.contains("已终止"));
    }

    #[test]
    fn bad_transition_rejected() {
        let mut t = AgentTable::new();
        let pid = t.spawn("a", "x", CpuPriority::Low, None).unwrap();
        assert!(matches!(
            t.set_state(pid, AgentPcbState::Completed),
            Err(AgentError::BadState { .. })
        ));
    }
}
