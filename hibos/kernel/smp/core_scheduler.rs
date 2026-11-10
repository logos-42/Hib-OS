//! 核心调度器 —— P1-3 子模块。

use crate::smp::{CpuInfo, CpuTopology};

/// 核心调度器：将任务分配到特定 CPU。
pub struct CoreScheduler {
    affinity: Vec<Option<u64>>, // cpu -> 当前任务 ID
}

impl CoreScheduler {
    pub fn new(num_cpus: usize) -> Self {
        Self {
            affinity: vec![None; num_cpus],
        }
    }

    /// 调度任务到 CPU（亲和性优先）。
    pub fn core_schedule(&mut self, cpu: usize, task_id: u64) -> Result<(), String> {
        if cpu >= self.affinity.len() {
            return Err(format!("CPU {} 不存在", cpu));
        }
        self.affinity[cpu] = Some(task_id);
        Ok(())
    }

    /// 迁移任务从源 CPU 到目标 CPU。
    pub fn core_migrate(&mut self, from: usize, to: usize) -> Result<(), String> {
        if from >= self.affinity.len() || to >= self.affinity.len() {
            return Err("CPU 索引越界".to_string());
        }
        let task = self.affinity[from].take();
        match task {
            Some(tid) => {
                self.affinity[to] = Some(tid);
                Ok(())
            }
            None => Err("源 CPU 上无任务".to_string()),
        }
    }

    /// 查询 CPU 上的任务。
    pub fn task_on_cpu(&self, cpu: usize) -> Option<u64> {
        self.affinity.get(cpu).and_then(|t| *t)
    }
}

/// 全局调度。
pub fn core_schedule(topo: &mut CpuTopology, cpu: usize, task_id: u64) -> Result<(), String> {
    if cpu >= topo.cpus.len() {
        return Err("CPU 不存在".to_string());
    }
    if !topo.cpus[cpu].online {
        return Err("CPU 未上线".to_string());
    }
    Ok(())
}

/// 任务迁移。
pub fn core_migrate(topo: &mut CpuTopology, from: usize, to: usize) -> Result<(), String> {
    if from >= topo.cpus.len() || to >= topo.cpus.len() {
        return Err("CPU 索引越界".to_string());
    }
    if !topo.cpus[to].online {
        return Err("目标 CPU 未上线".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_migrate() {
        let mut sched = CoreScheduler::new(4);
        assert!(sched.core_schedule(0, 100).is_ok());
        assert_eq!(sched.task_on_cpu(0), Some(100));
        assert!(sched.core_migrate(0, 2).is_ok());
        assert_eq!(sched.task_on_cpu(2), Some(100));
        assert_eq!(sched.task_on_cpu(0), None);
    }

    #[test]
    fn migrate_invalid() {
        let mut sched = CoreScheduler::new(2);
        assert!(sched.core_migrate(0, 5).is_err());
        assert!(sched.core_schedule(5, 1).is_err());
    }
}
