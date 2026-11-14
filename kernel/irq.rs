//! 中断通道 —— `irq/<设备>` 的后端。
//!
//! Linux 思维：IRQ 编号、中断描述符表、中断上下文、软中断/tasklet。
//! AI 通道思维：中断 = 外部世界向某个通道投递的「感知事件」；
//! `irq/<设备>` 是一个通道，设备事件通过 `act irq/<设备> fire` 投递，
//! 等待在该通道上的执行体被 `signal` 唤醒。
//!
//! 中断处理器 = 高优先级执行体，绑定到 `irq/<设备>` 通道。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use crate::ksync::{SpinLock, OnceLock};

/// 中断处理器绑定：设备名 -> 处理该中断的 exec-id。
struct IrqBinding {
    #[allow(dead_code)]
    device: String,
    handler_exec_id: Option<u64>,
    fire_count: u64,
}

/// 中断控制器状态。
struct IrqState {
    bindings: BTreeMap<String, IrqBinding>,
}

static IRQ: OnceLock<SpinLock<IrqState>> = OnceLock::new();

fn irq_state() -> &'static SpinLock<IrqState> {
    IRQ.get_or_init(|| {
        SpinLock::new(IrqState {
            bindings: BTreeMap::new(),
        })
    })
}

/// 绑定一个中断处理器到设备。
pub fn bind(device: &str, handler_exec_id: u64) {
    let mut g = irq_state().lock();
    g.bindings.insert(
        device.to_string(),
        IrqBinding {
            device: device.to_string(),
            handler_exec_id: Some(handler_exec_id),
            fire_count: 0,
        },
    );
}

/// 解绑设备的中断处理器。
pub fn unbind(device: &str) {
    let mut g = irq_state().lock();
    g.bindings.remove(device);
}

/// 投递一个中断到设备：如果有绑定的处理器，向其等待的通道发 signal。
pub fn fire(device: &str) -> u64 {
    let mut g = irq_state().lock();
    if let Some(b) = g.bindings.get_mut(device) {
        b.fire_count += 1;
        let handler = b.handler_exec_id;
        drop(g);
        if let Some(_eid) = handler {
            // no_std 环境下无 exec 调度器，signal/boost_priority 暂为空操作
            let n = 0u64;
            n
        } else {
            0
        }
    } else {
        0
    }
}

/// 列出所有中断绑定。
pub fn list() -> String {
    let g = irq_state().lock();
    if g.bindings.is_empty() {
        return "（无中断绑定）".to_string();
    }
    let mut out = String::new();
    for (dev, b) in g.bindings.iter() {
        out.push_str(&format!(
            "irq={} handler={:?} fires={}\n",
            dev, b.handler_exec_id, b.fire_count
        ));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_fire() {
        bind("timer", 42);
        let n = fire("timer");
        assert_eq!(n, 0); // 没有 exec 在等待 irq/timer，signal 返回 0
        let s = list();
        assert!(s.contains("timer"));
        assert!(s.contains("fires=1"));
        unbind("timer");
        let s = list();
        assert!(!s.contains("timer"));
    }
}
