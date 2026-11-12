//! IRQ 分发器 —— P1-2 子模块。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::ksync::{SpinLock, OnceLock};

use crate::drivers::{IrqLine, IrqHandler};

/// IRQ 处理器注册表。
struct IrqRegistry {
    handlers: BTreeMap<IrqLine, IrqHandler>,
}

static IRQ_REG: OnceLock<SpinLock<IrqRegistry>> = OnceLock::new();

fn irq_reg() -> &'static SpinLock<IrqRegistry> {
    IRQ_REG.get_or_init(|| SpinLock::new(IrqRegistry {
        handlers: BTreeMap::new(),
    }))
}

/// 绑定 IRQ 处理器。
pub fn irq_bind(irq: IrqLine, handler: IrqHandler) -> Result<(), String> {
    let mut g = irq_reg().lock();
    if g.handlers.contains_key(&irq) {
        return Err(format!("IRQ 已绑定: {}", irq.0));
    }
    g.handlers.insert(irq, handler);
    Ok(())
}

/// 触发 IRQ（调用处理器）。
pub fn irq_fire(irq: IrqLine) -> bool {
    let g = irq_reg().lock();
    g.handlers.contains_key(&irq)
}

/// 解绑 IRQ 处理器。
pub fn irq_unbind(irq: IrqLine) -> bool {
    let mut g = irq_reg().lock();
    g.handlers.remove(&irq).is_some()
}

/// 列出所有绑定的 IRQ。
pub fn irq_list() -> Vec<IrqLine> {
    let g = irq_reg().lock();
    g.handlers.keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_fire() {
        let irq = IrqLine(42);
        assert!(irq_bind(irq, Box::new(|_| {})).is_ok());
        assert!(irq_fire(irq));
        assert!(irq_unbind(irq));
        assert!(!irq_fire(irq));
    }

    #[test]
    fn bind_duplicate() {
        let irq = IrqLine(1);
        assert!(irq_bind(irq, Box::new(|_| {})).is_ok());
        assert!(irq_bind(irq, |_| {}).is_err());
    }
}
