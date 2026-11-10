//! CPU 拓扑 —— P1-3 子模块。

use std::sync::{Mutex, OnceLock};

use crate::smp::{CpuInfo, CpuTopology};

static CPU_TOPO: OnceLock<Mutex<Option<CpuTopology>>> = OnceLock::new();

fn cpu_topo() -> &'static Mutex<Option<CpuTopology>> {
    CPU_TOPO.get_or_init(|| Mutex::new(None))
}

/// 获取 CPU 拓扑（单例）。
pub fn cpu_topology() -> &'static Mutex<Option<CpuTopology>> {
    cpu_topo()
}

/// 使 CPU 上线。
pub fn cpu_online(id: usize) -> bool {
    let mut g = cpu_topo().lock().unwrap();
    match g.as_mut() {
        Some(topo) => {
            if id < topo.cpus.len() {
                topo.cpus[id].online = true;
                topo.online_cores += 1;
                true
            } else {
                false
            }
        }
        None => false,
    }
}

/// 使 CPU 下线。
pub fn cpu_offline(id: usize) -> bool {
    let mut g = cpu_topo().lock().unwrap();
    match g.as_mut() {
        Some(topo) => {
            if id < topo.cpus.len() && topo.cpus[id].online {
                topo.cpus[id].online = false;
                topo.online_cores -= 1;
                true
            } else {
                false
            }
        }
        None => false,
    }
}

/// 初始化拓扑。
pub fn init_topo(total_cores: usize) {
    let mut g = cpu_topo().lock().unwrap();
    if g.is_none() {
        *g = Some(CpuTopology::new(total_cores));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_offline() {
        init_topo(4);
        assert!(cpu_offline(2));
        assert_eq!(cpu_topo().lock().unwrap().as_ref().unwrap().online_cores, 3);
        assert!(cpu_online(2));
        assert_eq!(cpu_topo().lock().unwrap().as_ref().unwrap().online_cores, 4);
    }
}
