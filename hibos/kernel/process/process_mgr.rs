//! 进程管理器 —— P0-2 子模块。

use crate::process::{ProcessId, ProcessState, Pcb};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 全局进程管理器。
struct ProcessManagerState {
    processes: BTreeMap<ProcessId, Pcb>,
    next_id: u64,
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

    /// 设置进程状态。
    pub fn set_state(pid: ProcessId, state: ProcessState) -> bool {
        let mut g = pm_state().lock().unwrap();
        match g.processes.get_mut(&pid) {
            Some(pcb) => {
                pcb.state = state;
                true
            }
            None => false,
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
        assert_eq!(ProcessManager::count(), 1);
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
}
