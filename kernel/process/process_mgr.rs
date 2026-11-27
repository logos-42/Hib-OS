//! 进程管理器 —— P0-2 子模块。

use crate::process::{ProcessId, ProcessState, Pcb};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 全局进程管理器。
pub struct ProcessManagerState {
    pub(crate) processes: BTreeMap<ProcessId, Pcb>,
    pub(crate) next_id: u64,
}

static PM: OnceLock<Mutex<ProcessManagerState>> = OnceLock::new();

pub(crate) fn pm_state() -> &'static Mutex<ProcessManagerState> {
    PM.get_or_init(|| {
        Mutex::new(ProcessManagerState {
            processes: BTreeMap::new(),
            next_id: 1,
        })
    })
}

/// 进程管理器句柄。
pub struct ProcessManager;

impl ProcessManager {
    /// 创建新进程，返回 pid。
    pub fn create(name: &str, priority: u8, domain: &str) -> ProcessId {
        let mut g = pm_state().lock().unwrap();
        let id = ProcessId::new(g.next_id);
        g.next_id += 1;
        let pcb = Pcb::new(id, name, priority, domain);
        g.processes.insert(id, pcb);
        id
    }

    /// 获取进程 PCB。
    pub fn get(pid: ProcessId) -> Option<Pcb> {
        let g = pm_state().lock().unwrap();
        g.processes.get(&pid).cloned()
    }

    /// 设置进程状态（非法迁移返回 false，状态保持不变）。
    pub fn set_state(pid: ProcessId, state: ProcessState) -> bool {
        let mut g = pm_state().lock().unwrap();
        match g.processes.get_mut(&pid) {
            Some(pcb) => crate::process::transition(pcb, state).is_ok(),
            None => false,
        }
    }

    /// 回收僵尸进程：Zombie → Terminated，对应 TLA+ 中的 Reap(p) 公平动作。
    pub fn reap(pid: ProcessId) -> bool {
        Self::set_state_if(pid, ProcessState::Zombie, ProcessState::Terminated)
    }

    fn set_state_if(pid: ProcessId, from: ProcessState, to: ProcessState) -> bool {
        let mut g = pm_state().lock().unwrap();
        match g.processes.get_mut(&pid) {
            Some(pcb) if pcb.state == from => crate::process::transition(pcb, to).is_ok(),
            _ => false,
        }
    }

    /// 退出进程。
    pub fn exit(pid: ProcessId) -> bool {
        Self::set_state(pid, ProcessState::Zombie)
    }

    /// 进程总数。
    pub fn count() -> usize {
        let g = pm_state().lock().unwrap();
        g.processes.len()
    }
}

/// 全局进程管理器单例。
pub fn process_manager() -> &'static Mutex<ProcessManagerState> {
    pm_state()
}
/// 创建进程。
pub fn spawn_process(name: &str, priority: u8, domain: &str) -> ProcessId {
    ProcessManager::create(name, priority, domain)
}

/// 退出进程。
pub fn exit_process(pid: ProcessId) -> bool {
    ProcessManager::exit(pid)
}

/// 等待进程退出（简化版：立即返回）。
pub fn wait_process(pid: ProcessId) -> bool {
    let g = pm_state().lock().unwrap();
    matches!(
        g.processes.get(&pid),
        Some(pcb) if pcb.is_terminal()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_exit() {
        let pid = spawn_process("test", 10, "perception");
        // 全局注册表被同进程内其它并行测试共享，count 只增不减，
        // 这里只断言可见性，不断言精确数量。
        assert!(ProcessManager::count() >= 1);
        assert!(ProcessManager::get(pid).is_some());
        assert!(exit_process(pid));
        assert!(wait_process(pid));
    }

    #[test]
    fn set_state() {
        let pid = spawn_process("s", 5, "memory");
        assert!(ProcessManager::set_state(pid, ProcessState::Running));
        let pcb = ProcessManager::get(pid).unwrap();
        assert_eq!(pcb.state, ProcessState::Running);
    }

    #[test]
    fn terminal_state_cannot_be_resurrected() {
        // 对应 model/ProcLifeCurrent.tla 的 Zombie -> Created 反例。
        let pid = spawn_process("z", 5, "memory");
        assert!(exit_process(pid));
        assert_eq!(ProcessManager::get(pid).unwrap().state, ProcessState::Zombie);
        assert!(!ProcessManager::set_state(pid, ProcessState::Created));
        assert!(!ProcessManager::set_state(pid, ProcessState::Running));
        assert!(!crate::process::process_sched::make_ready(pid));
        assert_eq!(ProcessManager::get(pid).unwrap().state, ProcessState::Zombie);
    }

    #[test]
    fn zombie_is_reaped_then_absorbing() {
        let pid = spawn_process("r", 5, "memory");
        assert!(exit_process(pid));
        assert!(ProcessManager::reap(pid));
        assert_eq!(ProcessManager::get(pid).unwrap().state, ProcessState::Terminated);
        assert!(!ProcessManager::set_state(pid, ProcessState::Ready));
        assert!(!ProcessManager::reap(pid));
    }
}
