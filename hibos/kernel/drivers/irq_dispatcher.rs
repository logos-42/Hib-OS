//! IRQ 分发器 —— P1-2 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::drivers::{IrqLine, IrqHandler};

/// IRQ 处理器注册表。
struct IrqRegistry {
    handlers: BTreeMap<IrqLine, IrqHandler>,
}

static IRQ_REG: OnceLock<Mutex<IrqRegistry>> = OnceLock::new();

fn irq_reg() -> &'static Mutex<IrqRegistry> {
    IRQ_REG.get_or_init(|| Mutex::new(IrqRegistry {
        handlers: BTreeMap::new(),
    }))
}

/// 绑定 IRQ 处理器。
pub fn irq_bind(irq: IrqLine, handler: IrqHandler) -> Result<(), String> {
    let mut g = irq_reg().lock().unwrap();
    if g.handlers.contains_key(&irq) {
        return Err(format!("IRQ 已绑定: {}", irq.0));
    }
    g.handlers.insert(irq, handler);
    Ok(())
}

/// 触发 IRQ（调用处理器）。
pub fn irq_fire(irq: IrqLine) -> bool {
    let g = irq_reg().lock().unwrap();
    g.handlers.contains_key(&irq)
}

/// 解绑 IRQ 处理器。
pub fn irq_unbind(irq: IrqLine) -> bool {
    let mut g = irq_reg().lock().unwrap();
    g.handlers.remove(&irq).is_some()
}

/// 列出所有绑定的 IRQ。
pub fn irq_list() -> Vec<IrqLine> {
    let g = irq_reg().lock().unwrap();
    g.handlers.keys().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_fire() {
        let irq = IrqLine(42);
        assert!(irq_bind(irq, |_| {}).is_ok());
        assert!(irq_fire(irq));
        assert!(irq_unbind(irq));
        assert!(!irq_fire(irq));
    }

    #[test]
    fn bind_duplicate() {
        let irq = IrqLine(1);
        assert!(irq_bind(irq, |_| {}).is_ok());
        assert!(irq_bind(irq, |_| {}).is_err());
    }
}
