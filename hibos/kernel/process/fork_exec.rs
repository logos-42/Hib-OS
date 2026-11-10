//! fork/exec 镜像加载 —— P0-2 子模块。

use crate::process::{ProcessId, Pcb, ProcessState};
use crate::process::process_mgr::{pm_state, ProcessManager};
use std::sync::Mutex;

/// fork：克隆当前进程的 PCB（通道集合克隆），返回子进程 pid。
pub fn fork(parent_pid: ProcessId) -> Result<ProcessId, String> {
    let mut g = pm_state().lock().unwrap();
    let parent = g
        .processes
        .get(&parent_pid)
        .ok_or("父进程不存在")?;
    let child_id = ProcessId::new(g.next_id);
    g.next_id += 1;
    let mut child = parent.clone();
    child.pid = child_id;
    child.parent = Some(parent_pid);
    child.state = ProcessState::Ready;
    child.token_used = 0;
    g.processes.insert(child_id, child);
    if let Some(p) = g.processes.get_mut(&parent_pid) {
        p.children.push(child_id);
    }
    Ok(child_id)
}

/// exec：加载镜像到进程地址空间（AI 原生：模型实例绑定）。
pub fn exec(pid: ProcessId, model_instance: u64) -> Result<(), String> {
    let mut g = pm_state().lock().unwrap();
    let pcb = g
        .processes
        .get_mut(&pid)
        .ok_or("进程不存在")?;
    pcb.model_instance = Some(model_instance);
    pcb.state = ProcessState::Ready;
    Ok(())
}

/// waitpid：等待子进程退出。
pub fn waitpid(pid: ProcessId) -> Result<i32, String> {
    let g = pm_state().lock().unwrap();
    let pcb = g
        .processes
        .get(&pid)
        .ok_or("进程不存在")?;
    if pcb.is_terminal() {
        Ok(0)
    } else {
        Err("进程未退出".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::process_mgr::ProcessManager;

    #[test]
    fn fork_exec_waitpid() {
        let parent = crate::process::process_mgr::spawn_process("parent", 10, "governance");
        let child = fork(parent).unwrap();
        assert!(exec(child, 42).is_ok());
        assert!(waitpid(child).is_err()); // 未退出
        crate::process::process_mgr::exit_process(child);
        assert!(waitpid(child).is_ok());
    }

    #[test]
    fn fork_nonexistent_parent() {
        assert!(fork(ProcessId::new(999)).is_err());
    }

    #[test]
    fn exec_nonexistent_process() {
        assert!(exec(ProcessId::new(999), 42).is_err());
    }
}
