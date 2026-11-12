//! PCB 与进程状态机 —— P0-2 子模块。

use crate::process::{ProcessId, ProcessState};

/// 进程控制块（已在 process/mod.rs 定义，此处为扩展点）。
pub struct PcbExt {
    pub pid: ProcessId,
    pub state: ProcessState,
    pub exec_time_ns: u64,
    pub wait_count: u64,
}

impl PcbExt {
    pub fn new(pid: ProcessId) -> Self {
        Self {
            pid,
            state: ProcessState::Created,
            exec_time_ns: 0,
            wait_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessId;

    #[test]
    fn pcb_ext_creation() {
        let ext = PcbExt::new(ProcessId::new(5));
        assert_eq!(ext.pid, ProcessId::new(5));
        assert_eq!(ext.state, ProcessState::Created);
        assert_eq!(ext.exec_time_ns, 0);
    }
}
