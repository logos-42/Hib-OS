//! IPI 进程间中断 —— P1-3 子模块。

use std::sync::{Mutex, OnceLock};

use crate::smp::IpiMessage;
use crate::drivers::IrqLine;

/// IPI 消息队列。
struct IpiQueue {
    queue: Vec<(IrqLine, IpiMessage)>,
}

static IPI_Q: OnceLock<Mutex<IpiQueue>> = OnceLock::new();

fn ipi_q() -> &'static Mutex<IpiQueue> {
    IPI_Q.get_or_init(|| Mutex::new(IpiQueue { queue: Vec::new() }))
}

/// 发送 IPI 到指定 CPU。
pub fn ipi_send(target: IrqLine, msg: IpiMessage) -> bool {
    let mut g = ipi_q().lock().unwrap();
    g.queue.push((target, msg));
    true
}

/// 广播 IPI 到所有 CPU。
pub fn ipi_broadcast(msg: IpiMessage, targets: &[IrqLine]) -> usize {
    let mut g = ipi_q().lock().unwrap();
    for &t in targets {
        g.queue.push((t, msg.clone()));
    }
    targets.len()
}

/// 获取并清空队列。
pub fn ipi_drain() -> Vec<(IrqLine, IpiMessage)> {
    let mut g = ipi_q().lock().unwrap();
    std::mem::take(&mut g.queue)
}

/// 队列长度。
pub fn ipi_queue_len() -> usize {
    let g = ipi_q().lock().unwrap();
    g.queue.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::IrqLine;

    #[test]
    fn send_and_drain() {
        assert!(ipi_send(IrqLine(1), IpiMessage::Reschedule));
        assert_eq!(ipi_queue_len(), 1);
        let drained = ipi_drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(ipi_queue_len(), 0);
    }

    #[test]
    fn broadcast() {
        let targets = [IrqLine(0), IrqLine(1), IrqLine(2)];
        let n = ipi_broadcast(IpiMessage::Shutdown(0), &targets);
        assert_eq!(n, 3);
        assert_eq!(ipi_queue_len(), 3);
    }
}
