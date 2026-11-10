//! 进程调度接口 —— P0-2 子模块。

use crate::process::{ProcessId, ProcessState, Pcb};
use crate::process::process_mgr::{pm_state, ProcessManager};

/// 将进程设为就绪状态。
pub fn make_ready(pid: ProcessId) -> bool {
    crate::process::process_mgr::ProcessManager::set_state(pid, ProcessState::Ready)
}

/// 将进程设为阻塞状态。
pub fn make_blocked(pid: ProcessId) -> bool {
    crate::process::process_mgr::ProcessManager::set_state(pid, ProcessState::Blocked)
}

/// 将进程设为运行状态。
pub fn make_running(pid: ProcessId) -> bool {
    crate::process::process_mgr::ProcessManager::set_state(pid, ProcessState::Running)
}

/// 查询进程状态。
pub fn query_state(pid: ProcessId) -> Option<ProcessState> {
    crate::process::process_mgr::ProcessManager::get(pid).map(|p| p.state)
}

/// 进程是否可调度。
pub fn is_runnable(pid: ProcessId) -> bool {
    crate::process::process_mgr::ProcessManager::get(pid)
        .map(|p| p.is_runnable())
        .unwrap_or(false)
}

/// 获取进程的调度域。
pub fn query_domain(pid: ProcessId) -> Option<String> {
    crate::process::process_mgr::ProcessManager::get(pid).map(|p| p.domain.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::process_mgr::spawn_process;

    #[test]
    fn state_transitions_via_sched() {
        let pid = spawn_process("sched-test", 5, "reflex");
        assert!(make_ready(pid));
        assert_eq!(query_state(pid), Some(ProcessState::Ready));
        assert!(make_running(pid));
        assert_eq!(query_state(pid), Some(ProcessState::Running));
        assert!(make_blocked(pid));
        assert_eq!(query_state(pid), Some(ProcessState::Blocked));
        assert!(!is_runnable(pid));
    }

    #[test]
    fn query_domain_works() {
        let pid = spawn_process("domain-test", 8, "deliberation");
        assert_eq!(query_domain(pid), Some("deliberation".to_string()));
    }
}
