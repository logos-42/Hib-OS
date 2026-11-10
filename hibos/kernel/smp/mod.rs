//! 多核 SMP 支持 —— P1-3 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - 多处理机 / 调度 / 亲和性 → 多智能体并行、多调度域、模型实例亲和与负载均衡
//! - SMP vs Master-Slave → 五调度域独立调度
//! - 亲和性 (Affinity) → 模型实例亲和，复用 KV 缓存
//! 
//! 通道：`smp://*`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod cpu_topology;
pub mod spinlock;
pub mod ipi;
pub mod load_balancer;
pub mod core_scheduler;

// 公共类型
pub use cpu_topology::{CpuTopology, CpuInfo, cpu_topology, cpu_online, cpu_offline};
pub use spinlock::{SpinLock, spin_lock, spin_unlock, spin_trylock};
pub use ipi::{IpiMessage, ipi_send, ipi_broadcast};
pub use load_balancer::{LoadBalancer, balance_load};
pub use core_scheduler::{CoreScheduler, core_schedule, core_migrate};

// CPU 拓扑信息。
#[derive(Clone, Debug)]
pub struct CpuInfo {
    pub id: usize,
    pub online: bool,
    pub current_load: f64,
    pub affinity_mask: u64,
    pub domain: String, // 调度域
}

/// CPU 拓扑。
#[derive(Clone, Debug)]
pub struct CpuTopology {
    pub cpus: Vec<CpuInfo>,
    pub total_cores: usize,
    pub online_cores: usize,
}

impl CpuTopology {
    pub fn new(total_cores: usize) -> Self {
        let cpus = (0..total_cores)
            .map(|i| CpuInfo {
                id: i,
                online: true,
                current_load: 0.0,
                affinity_mask: 1u64 << i,
                domain: "deliberation".to_string(),
            })
            .collect();
        Self {
            cpus,
            total_cores,
            online_cores: total_cores,
        }
    }

    pub fn least_loaded_cpu(&self) -> Option<usize> {
        self.cpus
            .iter()
            .filter(|c| c.online)
            .min_by(|a, b| a.current_load.partial_cmp(&b.current_load).unwrap())
            .map(|c| c.id)
    }
}

/// 自旋锁。
#[derive(Debug)]
pub struct SpinLock<T> {
    data: std::sync::Mutex<T>,
    locked: std::sync::atomic::AtomicBool,
}

impl<T> SpinLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            data: std::sync::Mutex::new(data),
            locked: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.data.lock().unwrap()
    }

    pub fn try_lock(&self) -> Option<std::sync::MutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, std::sync::atomic::Ordering::Acquire, std::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            Some(self.data.lock().unwrap())
        } else {
            None
        }
    }

    pub fn unlock(&self) {
        self.locked.store(false, std::sync::atomic::Ordering::Release);
    }
}

impl<T> Default for SpinLock<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// IPI 消息。
#[derive(Clone, Debug)]
pub enum IpiMessage {
    Reschedule,
    Shutdown(u64),
    Interrupt(u32),
    Custom(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_topology_creation() {
        let topo = CpuTopology::new(4);
        assert_eq!(topo.total_cores, 4);
        assert_eq!(topo.online_cores, 4);
        assert_eq!(topo.cpus.len(), 4);
    }

    #[test]
    fn least_loaded_cpu() {
        let mut topo = CpuTopology::new(4);
        topo.cpus[2].current_load = 0.9;
        let cpu = topo.least_loaded_cpu();
        assert!(cpu.is_some());
        assert_ne!(cpu.unwrap(), 2);
    }

    #[test]
    fn spinlock_basic() {
        let lock = SpinLock::new(0u32);
        {
            let mut guard = lock.lock();
            *guard = 42;
        }
        assert_eq!(*lock.lock(), 42);
    }

    #[test]
    fn spinlock_try_lock() {
        let lock = SpinLock::new(0u32);
        let _g1 = lock.lock();
        assert!(lock.try_lock().is_none());
        drop(_g1);
        assert!(lock.try_lock().is_some());
    }

    #[test]
    fn ipi_message_variants() {
        let msgs = [
            IpiMessage::Reschedule,
            IpiMessage::Shutdown(0),
            IpiMessage::Interrupt(5),
            IpiMessage::Custom(123),
        ];
        assert_eq!(msgs.len(), 4);
    }
}
