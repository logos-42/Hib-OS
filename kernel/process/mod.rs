//! 进程隔离与 PCB 子系统 —— P0-2 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - 传统进程 (PCB、状态机、虚拟 CPU) → 智能体 (Agent)，独立 PCB 与状态机
//! - fork/exec/waitpid → 通道集合克隆 / 镜像加载 / 等待
//! - 进程组、会话、控制终端 → 智能体父子关系与调度域归属
//! 
//! 通道：`proc://*`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod pcb;
pub mod process_mgr;
pub mod fork_exec;
pub mod process_sched;

// 公共类型（已在本模块定义）
pub use process_mgr::{ProcessManager, process_manager, spawn_process, exit_process, wait_process};

// 进程 ID
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcessId(pub u64);

impl ProcessId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

/// 进程状态机。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Ready,
    Running,
    Blocked,
    Zombie,
    Terminated,
}

/// PCB：进程控制块，独立上下文与可调度单元。
#[derive(Clone, Debug)]
pub struct Pcb {
    pub pid: ProcessId,
    pub name: String,
    pub state: ProcessState,
    pub priority: u8,
    pub domain: String, // 调度域：reflex/perception/deliberation/memory/governance
    pub parent: Option<ProcessId>,
    pub children: Vec<ProcessId>,
    pub token_budget: u64,
    pub token_used: u64,
    pub context_window_pages: u64,
    pub model_instance: Option<u64>, // 绑定的模型实例 ID
    pub caps: Vec<String>,           // 能力位子集
    pub created_ns: u64,
}

impl Pcb {
    pub fn new(pid: ProcessId, name: &str, priority: u8, domain: &str) -> Self {
        Self {
            pid,
            name: name.to_string(),
            state: ProcessState::Created,
            priority,
            domain: domain.to_string(),
            parent: None,
            children: Vec::new(),
            token_budget: 100_000_000,
            token_used: 0,
            context_window_pages: 0,
            model_instance: None,
            caps: Vec::new(),
            created_ns: crate::monitor::get_time_ns(),
        }
    }

    pub fn is_runnable(&self) -> bool {
        matches!(self.state, ProcessState::Ready | ProcessState::Running)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self.state, ProcessState::Zombie | ProcessState::Terminated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcb_creation_and_state() {
        let pid = ProcessId::new(1);
        let pcb = Pcb::new(pid, "init", 10, "governance");
        assert_eq!(pcb.pid, pid);
        assert_eq!(pcb.name, "init");
        assert_eq!(pcb.state, ProcessState::Created);
        assert!(pcb.is_runnable() == false);
        assert!(pcb.is_terminal() == false);
    }

    #[test]
    fn pcb_state_transitions() {
        let mut pcb = Pcb::new(ProcessId::new(2), "worker", 5, "perception");
        pcb.state = ProcessState::Ready;
        assert!(pcb.is_runnable());
        pcb.state = ProcessState::Running;
        assert!(pcb.is_runnable());
        pcb.state = ProcessState::Blocked;
        assert!(!pcb.is_runnable());
        pcb.state = ProcessState::Zombie;
        assert!(pcb.is_terminal());
    }

    #[test]
    fn pcb_parent_child_relationship() {
        let parent = ProcessId::new(10);
        let child = ProcessId::new(11);
        let mut pcb = Pcb::new(parent, "parent", 10, "governance");
        pcb.children.push(child);
        assert_eq!(pcb.children.len(), 1);
        assert_eq!(pcb.children[0], child);
    }
}
