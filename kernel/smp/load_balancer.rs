//! 负载均衡器 —— P1-3 子模块。

use std::sync::{Mutex, OnceLock};

use crate::smp::CpuTopology;

/// 负载均衡器。
pub struct LoadBalancer {
    threshold: f64,
}

static LB: OnceLock<Mutex<LoadBalancer>> = OnceLock::new();

fn lb() -> &'static Mutex<LoadBalancer> {
    LB.get_or_init(|| Mutex::new(LoadBalancer { threshold: 0.7 }))
}

impl LoadBalancer {
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// 检查是否需要负载均衡。
    pub fn needs_balance(&self, topo: &CpuTopology) -> bool {
        let loads: Vec<f64> = topo.cpus.iter().filter(|c| c.online).map(|c| c.current_load).collect();
        if loads.is_empty() {
            return false;
        }
        let max = loads.iter().copied().fold(0.0f64, f64::max);
        let min = loads.iter().copied().fold(f64::INFINITY, f64::min);
        (max - min) > self.threshold
    }

    /// 执行负载均衡，返回 (源 CPU, 目标 CPU) 或 None。
    pub fn balance(&self, topo: &mut CpuTopology) -> Option<(usize, usize)> {
        let max_idx = topo
            .cpus
            .iter()
            .enumerate()
            .filter(|(_, c)| c.online)
            .max_by(|a, b| a.1.current_load.partial_cmp(&b.1.current_load).unwrap())
            .map(|(i, _)| i)?;
        let min_idx = topo
            .cpus
            .iter()
            .enumerate()
            .filter(|(_, c)| c.online)
            .min_by(|a, b| a.1.current_load.partial_cmp(&b.1.current_load).unwrap())
            .map(|(i, _)| i)?;
        if max_idx == min_idx {
            return None;
        }
        // 转移一部分负载。
        let max_load = topo.cpus[max_idx].current_load;
        let min_load = topo.cpus[min_idx].current_load;
        let transfer = (max_load - min_load) / 2.0;
        topo.cpus[max_idx].current_load -= transfer;
        topo.cpus[min_idx].current_load += transfer;
        Some((max_idx, min_idx))
    }
}

/// 全局负载均衡。
pub fn balance_load(topo: &mut CpuTopology) -> Option<(usize, usize)> {
    let g = lb().lock().unwrap();
    g.balance(topo)
}

/// 检查是否需要均衡。
pub fn needs_balance(topo: &CpuTopology) -> bool {
    let g = lb().lock().unwrap();
    g.needs_balance(topo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smp::CpuTopology;

    #[test]
    fn balance_detects_imbalance() {
        let mut topo = CpuTopology::new(4);
        topo.cpus[0].current_load = 0.95;
        topo.cpus[1].current_load = 0.1;
        topo.cpus[2].current_load = 0.1;
        topo.cpus[3].current_load = 0.1;
        let lb = LoadBalancer::new(0.5);
        assert!(lb.needs_balance(&topo));
        let result = lb.balance(&mut topo);
        assert!(result.is_some());
    }

    #[test]
    fn no_balance_when_balanced() {
        let topo = CpuTopology::new(4);
        let lb = LoadBalancer::new(0.5);
        assert!(!lb.needs_balance(&topo));
    }
}
