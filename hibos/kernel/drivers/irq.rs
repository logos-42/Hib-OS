//! IRQ 投递 —— `irq://<设备>` 通道后端（复用 kernel/irq.rs 的 bind/fire）。
//!
//! 设备中断通过 `irq://<设备>` 通道投递为 `signal` 事件，
//! 唤醒等待该通道的执行体/纤程。

use alloc::string::String;

/// 绑定一个中断处理器到设备。
pub fn bind_irq(device: &str, handler_exec_id: u64) {
    crate::irq::bind(device, handler_exec_id);
}

/// 解绑设备的中断处理器。
pub fn unbind_irq(device: &str) {
    crate::irq::unbind(device);
}

/// 投递一个中断到设备：向绑定的处理器所在通道发 signal，
/// 返回被唤醒的执行体数。
pub fn fire_irq(device: &str) -> u64 {
    crate::irq::fire(device) as u64
}

/// 列出所有中断绑定。
pub fn list_irqs() -> String {
    crate::irq::list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_fire_irq() {
        bind_irq("nvme0", 7);
        let n = fire_irq("nvme0");
        // 没有 exec 在等待 irq://nvme0，signal 返回 0，但 fire_count 已递增
        assert_eq!(n, 0);
        let s = list_irqs();
        assert!(s.contains("nvme0"));
        assert!(s.contains("fires=1"));
        unbind_irq("nvme0");
        let s = list_irqs();
        assert!(!s.contains("nvme0"));
    }

    #[test]
    fn fire_unbound_irq_returns_zero() {
        let n = fire_irq("nonexistent");
        assert_eq!(n, 0);
    }

    #[test]
    fn fire_after_unbind_returns_zero() {
        bind_irq("timer", 1);
        unbind_irq("timer");
        let n = fire_irq("timer");
        assert_eq!(n, 0);
        let s = list_irqs();
        assert!(!s.contains("timer"));
    }
}
