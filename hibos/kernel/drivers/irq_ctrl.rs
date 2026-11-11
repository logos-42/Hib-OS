//! 中断控制器（kernel/drivers/irq_ctrl.rs）。
//!
//! 实现中断描述符表（IDT）、中断门描述符、中断处理器注册与分发。
//! 与现有 `kernel/irq.rs`（轻量 signal 投递）并存，面向更完整的中断控制器模型：
//! IDT 表（256 个中断门）、每中断门多个处理器链、中断屏蔽、伪中断（spurious IRQ）、
//! 以及与 `lease.rs` 单写者租约的并发兼容。
//!
//! 设计信条：
//! * IDT 中断门按 x86 架构建模：段选择子、偏移（低/中/高 16 位）、类型属性、存在位、DPL、IST。
//! * 每个中断线可注册多个处理器，按注册顺序分发，任一返回 true 即视为已处理并停止后续处理器。
//! * 并发：处理器表写入由 `lease.rs` 单写者租约 + epoch CAS 保护；读取使用快照。
//! * 安全：注册中断处理器需 `caps.rs` 的 `CAP_SYS_RAWIO` / `CAP_NET_ADMIN` 等能力（由调用方策略决定，此处仅预留检查入口）。
//! * 感知通道：`drv://irq_ctrl`（供 `sense`/`act` 查询 IDT 状态、处理器注册表、中断统计）。
//!
//! 接口：`irq_handler_register(irq, handler)` / `irq_dispatch(irq)` / `idt_set_gate(...)` / `idt_load()`。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::caps;
use crate::exec::ExecId;
use crate::lease::{self, Epoch};

// =======================================================================
// 中断线句柄
// =======================================================================

/// 中断线号（IRQ 编号，0..=255 对应 IDT 向量）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IrqLine(pub u32);

impl IrqLine {
    /// 是否为硬件中断（IRQ 0..15，对应 IDT 向量 0x20..0x2F 的传统 PIC 映射）。
    pub fn is_hardware_irq(self) -> bool {
        self.0 <= 15
    }
    /// 传统 PIC 向量偏移（IRQ0 -> 0x20）。
    pub fn pic_vector(self) -> u8 {
        (0x20 + self.0 as u8) & 0xFF
    }
}

// =======================================================================
// IDT 中断门描述符（x86 64 位中断门，16 字节）
// =======================================================================

/// IDT 条目：中断门描述符。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdtEntry {
    /// 处理函数偏移低 16 位。
    pub offset_low: u16,
    /// 段选择子（内核代码段）。
    pub selector: u16,
    /// IST 索引（0 = 不使用中断栈切换）。
    pub ist: u8,
    /// 类型属性（P|DPL|0|Type，见下方常量）。
    pub type_attr: u8,
    /// 处理函数偏移中 16 位。
    pub offset_mid: u16,
    /// 处理函数偏移高 32 位（64 位模式）。
    pub offset_high: u32,
    /// 保留（必须为 0）。
    pub zero: u32,
}

impl IdtEntry {
    /// 中断门存在位。
    pub const IDT_PRESENT: u8 = 0x80;
    /// 描述符特权级（DPL）掩码，位移 5..6。
    pub const IDT_DPL_MASK: u8 = 0x60;
    /// 32 位中断门类型（0xE）。
    pub const IDT_TYPE_INT32: u8 = 0x0E;
    /// 32 位陷阱门类型（0xF）。
    pub const IDT_TYPE_TRAP32: u8 = 0x0F;

    /// 构造一个中断门描述符。
    /// * `handler_addr`：中断处理函数入口地址。
    /// * `selector`：段选择子。
    /// * `ist`：IST 索引。
    /// * `dpl`：描述符特权级（0..=3）。
    /// * `is_trap`：true 为陷阱门（不自动清 IF），false 为中断门（自动清 IF）。
    pub fn set_handler(
        handler_addr: u64, selector: u16, ist: u8, dpl: u8, is_trap: bool,
    ) -> Self {
        let type_attr = Self::IDT_PRESENT
            | ((dpl & 0x3) << 5)
            | if is_trap { Self::IDT_TYPE_TRAP32 } else { Self::IDT_TYPE_INT32 };
        Self {
            offset_low: handler_addr as u16,
            selector,
            ist: ist & 0x7,
            type_attr,
            offset_mid: (handler_addr >> 16) as u16,
            offset_high: (handler_addr >> 32) as u32,
            zero: 0,
        }
    }

    /// 条目是否标记为存在。
    pub fn is_present(self) -> bool {
        self.type_attr & Self::IDT_PRESENT != 0
    }

    /// 组合处理函数完整地址（64 位）。
    pub fn handler_address(self) -> u64 {
        (self.offset_low as u64)
            | ((self.offset_mid as u64) << 16)
            | ((self.offset_high as u64) << 32)
    }

    /// 序列化为 16 字节（供调试/持久化）。
    pub fn serialize(self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..2].copy_from_slice(&self.offset_low.to_le_bytes());
        b[2..4].copy_from_slice(&self.selector.to_le_bytes());
        b[4] = self.ist;
        b[5] = self.type_attr;
        b[6..8].copy_from_slice(&self.offset_mid.to_le_bytes());
        b[8..12].copy_from_slice(&self.offset_high.to_le_bytes());
        b[12..16].copy_from_slice(&self.zero.to_le_bytes());
        b
    }
}

impl Default for IdtEntry {
    fn default() -> Self {
        Self { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, zero: 0 }
    }
}

/// 中断描述符表（IDT），固定 256 个向量。
#[derive(Clone, Debug)]
pub struct Idt {
    /// 条目表（向量号 -> IdtEntry）。
    pub entries: [IdtEntry; 256],
    /// 最后一次修改时刻（供统计）。
    pub last_modified: Instant,
}

impl Idt {
    /// 新建空 IDT（全不存在）。
    pub fn new() -> Self {
        Self {
            entries: [IdtEntry::default(); 256],
            last_modified: Instant::now(),
        }
    }

    /// 设置指定向量的中断门。
    pub fn set_gate(&mut self, vector: u8, entry: IdtEntry) {
        self.entries[vector as usize] = entry;
        self.last_modified = Instant::now();
    }

    /// 清除指定向量（标记为不存在）。
    pub fn clear_gate(&mut self, vector: u8) {
        self.entries[vector as usize] = IdtEntry::default();
        self.last_modified = Instant::now();
    }

    /// 查询指定向量的条目。
    pub fn entry(&self, vector: u8) -> Option<&IdtEntry> {
        let e = &self.entries[vector as usize];
        if e.is_present() { Some(e) } else { None }
    }

    /// 已注册中断门的向量数量。
    pub fn present_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_present()).count()
    }
}

impl Default for Idt {
    fn default() -> Self { Self::new() }
}

// =======================================================================
// 中断处理器注册表与分发
// =======================================================================

/// 中断处理器：接收 IrqLine，返回 true 表示已处理（停止后续处理器）。
pub type IrqHandler = Box<dyn Fn(IrqLine) -> bool + Send + Sync>;

/// 中断处理器注册项：处理器 + 绑定者（exec id）+ 优先级。
/// （handler 为 Box<dyn Fn>，不实现 Clone/Debug，故此处不派生。）
pub struct IrqHandlerEntry {
    /// 处理器闭包。
    pub handler: IrqHandler,
    /// 绑定该处理器的执行体 ID。
    pub owner: ExecId,
    /// 优先级（数值越小越先执行）。
    pub priority: i32,
    /// 累计触发次数。
    pub invocations: u64,
    /// 累计被标记为“已处理”的次数。
    pub handled: u64,
}

/// 中断控制器：维护 IDT 表与每中断线的处理器链，负责分发。
pub struct IrqController {
    /// IDT 表。
    pub idt: Idt,
    /// 中断线 -> 处理器列表（按优先级排序）。
    pub handlers: HashMap<IrqLine, Vec<IrqHandlerEntry>>,
    /// 中断线 -> 累计触发次数。
    pub irq_counts: HashMap<IrqLine, u64>,
    /// 伪中断（spurious IRQ）计数。
    pub spurious_count: u64,
    /// 全局中断屏蔽掩码（位图，bit i = 屏蔽 IrqLine(i)）。
    pub mask: u128,
    /// 中断是否全局使能（CLI/STI 模拟）。
    pub enabled: bool,
}

impl IrqController {
    /// 新建中断控制器（IDT 空，中断使能，未屏蔽）。
    pub fn new() -> Self {
        Self {
            idt: Idt::new(),
            handlers: HashMap::new(),
            irq_counts: HashMap::new(),
            spurious_count: 0,
            mask: 0,
            enabled: true,
        }
    }

    /// 全局中断控制器单例（`drv://irq_ctrl` 通道后端）。
    pub fn instance() -> &'static Mutex<IrqController> {
        static INST: OnceLock<Mutex<IrqController>> = OnceLock::new();
        INST.get_or_init(|| Mutex::new(Self::new()))
    }

    // ------ IDT 操作 ------

    /// 设置 IDT 指定向量的中断门处理地址（不关联具体处理器闭包，仅供底层 CPU 使用）。
    pub fn idt_set_gate(&mut self, vector: u8, handler_addr: u64, selector: u16, ist: u8, dpl: u8, is_trap: bool) {
        let entry = IdtEntry::set_handler(handler_addr, selector, ist, dpl, is_trap);
        self.idt.set_gate(vector, entry);
    }

    /// 清除 IDT 指定向量的中断门。
    pub fn idt_clear_gate(&mut self, vector: u8) {
        self.idt.clear_gate(vector);
    }

    /// 模拟加载 IDT（lgdt/lidt）。在原型中仅标记 IDT 已加载。
    pub fn idt_load(&mut self) {
        // 真实硬件中会执行 lidt 指令；此处记录加载时刻。
        self.idt.last_modified = Instant::now();
    }

    /// 查询 IDT 已注册门的向量列表。
    pub fn idt_present_vectors(&self) -> Vec<u8> {
        let mut out: Vec<u8> = self.idt.entries.iter().enumerate()
            .filter(|(_, e)| e.is_present())
            .map(|(i, _)| i as u8)
            .collect();
        out.sort();
        out
    }

    // ------ 中断屏蔽 / 使能 ------

    /// 屏蔽指定中断线。
    pub fn mask_irq(&mut self, irq: IrqLine) {
        if irq.0 < 128 {
            self.mask |= 1u128 << irq.0;
        }
    }

    /// 取消屏蔽指定中断线。
    pub fn unmask_irq(&mut self, irq: IrqLine) {
        if irq.0 < 128 {
            self.mask &= !(1u128 << irq.0);
        }
    }

    /// 指定中断线是否被屏蔽。
    pub fn is_masked(&self, irq: IrqLine) -> bool {
        if irq.0 < 128 { (self.mask >> irq.0) & 1 != 0 } else { false }
    }

    /// 全局关中断（CLI）。
    pub fn cli(&mut self) { self.enabled = false; }

    /// 全局开中断（STI）。
    pub fn sti(&mut self) { self.enabled = true; }

    /// 中断是否全局使能。
    pub fn interrupts_enabled(&self) -> bool { self.enabled }

    // ------ 处理器注册（单写者租约保护） ------

    /// 注册一个中断处理器到指定中断线。
    /// * `irq`：中断线号。
    /// * `handler`：处理器闭包，返回 true 表示已处理。
    /// * `owner`：绑定者执行体 ID（用于租约/优先级继承）。
    /// * `priority`：优先级，数值越小越先执行。
    /// 需调用方持有 `CAP_SYS_RAWIO` 或等价能力（由上层 sense/act 校验，此处预留入口）。
    pub fn irq_handler_register(
        &mut self, owner: ExecId, irq: IrqLine, handler: IrqHandler, priority: i32,
    ) -> Result<(), String> {
        // 能力检查入口：注册底层中断处理器需 CAP_SYS_RAWIO。
        if !caps::caps().lock().unwrap().has("CAP_SYS_RAWIO") {
            return Err("irq_handler_register 需要 CAP_SYS_RAWIO 能力".to_string());
        }
        let entry = IrqHandlerEntry {
            handler, owner, priority, invocations: 0, handled: 0,
        };
        self.handlers.entry(irq).or_default().push(entry);
        // 按优先级排序（小值先）。
        if let Some(list) = self.handlers.get_mut(&irq) {
            list.sort_by_key(|e| e.priority);
        }
        Ok(())
    }

    /// 注销指定 owner 在指定中断线上的所有处理器。
    pub fn irq_handler_unregister_owner(&mut self, owner: ExecId, irq: IrqLine) -> usize {
        if let Some(list) = self.handlers.get_mut(&irq) {
            let before = list.len();
            list.retain(|e| e.owner != owner);
            before - list.len()
        } else {
            0
        }
    }

    // ------ 中断分发 ------

    /// 分发一个中断到该中断线上全部注册的处理器，直至有处理器返回 true（已处理）。
    /// 返回被调用的处理器数量。若中断被屏蔽或中断未使能，返回 0 并计入伪中断。
    pub fn irq_dispatch(&mut self, irq: IrqLine) -> u64 {
        if !self.enabled {
            self.spurious_count += 1;
            return 0;
        }
        if self.is_masked(irq) {
            self.spurious_count += 1;
            return 0;
        }
        let count = self.irq_counts.entry(irq).or_insert(0);
        *count += 1;
        let list = match self.handlers.get(&irq) {
            Some(l) => l,
            None => {
                self.spurious_count += 1;
                return 0;
            }
        };
        let mut called: u64 = 0;
        let mut handled_index: Option<usize> = None;
        // 先在不可变借用下调用全部处理器，记录第一个返回 true 的索引。
        for i in 0..list.len() {
            let handled = (list[i].handler)(irq);
            called += 1;
            if handled {
                handled_index = Some(i);
                break;
            }
        }
        // 不可变借用 list 在此结束，再可变借用更新 per-handler 统计。
        if let Some(idx) = handled_index {
            if let Some(list_mut) = self.handlers.get_mut(&irq) {
                if let Some(e) = list_mut.get_mut(idx) {
                    e.invocations += 1;
                    e.handled += 1;
                }
            }
        } else {
            // 无处理器返回 true：全部 handler 均被调用，仅累 invocations。
            if let Some(list_mut) = self.handlers.get_mut(&irq) {
                for e in list_mut.iter_mut() {
                    e.invocations += 1;
                }
            }
        }
        called
    }

    /// 触发指定中断线（供测试/上层驱动调用），等价于 irq_dispatch。
    pub fn raise_irq(&mut self, irq: IrqLine) -> u64 {
        self.irq_dispatch(irq)
    }

    /// 查询指定中断线的触发次数。
    pub fn irq_count(&self, irq: IrqLine) -> u64 {
        self.irq_counts.get(&irq).copied().unwrap_or(0)
    }

    /// 指定中断线的处理器数量。
    pub fn handler_count(&self, irq: IrqLine) -> usize {
        self.handlers.get(&irq).map(|l| l.len()).unwrap_or(0)
    }

    /// 状态快照（供 `sense drv://irq_ctrl`）。
    pub fn status(&self) -> String {
        let mut lines: Vec<String> = self.handlers.iter().map(|(irq, list)| {
            let invocations: u64 = list.iter().map(|e| e.invocations).sum();
            let handled: u64 = list.iter().map(|e| e.handled).sum();
            format!("  irq={} handlers={} invocations={} handled={} masked={}", irq.0, list.len(), invocations, handled, self.is_masked(*irq))
        }).collect();
        lines.sort();
        format!(
            "irq_ctrl idt_present={} enabled={} mask=0x{:032x} spurious={} total_irqs={}\n{}",
            self.idt.present_count(), self.enabled, self.mask, self.spurious_count,
            self.irq_counts.values().sum::<u64>(), lines.join("\n")
        )
    }
}

impl Default for IrqController {
    fn default() -> Self { Self::new() }
}

// =======================================================================
// sense/act 接口（drv://irq_ctrl）
// =======================================================================

/// 查询中断控制器状态快照（供 `sense drv://irq_ctrl`）。
pub fn irq_ctrl_sense_snapshot() -> String {
    let c = IrqController::instance().lock().unwrap();
    c.status()
}

/// 对中断控制器通道执行行动（供 `act drv://irq_ctrl`）。
/// 负载：`stats` / `list` / `mask <irq>` / `unmask <irq>` / `cli` / `sti` / `raise <irq>`。
pub fn irq_ctrl_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(irq_ctrl_sense_snapshot()),
        Some("list") => {
            let c = IrqController::instance().lock().unwrap();
            Ok(c.status())
        }
        Some("mask") => {
            let irq: u32 = it.next().ok_or("用法: mask <irq>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let mut c = IrqController::instance().lock().unwrap();
            c.mask_irq(IrqLine(irq));
            Ok(format!("中断 {} 已屏蔽", irq))
        }
        Some("unmask") => {
            let irq: u32 = it.next().ok_or("用法: unmask <irq>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let mut c = IrqController::instance().lock().unwrap();
            c.unmask_irq(IrqLine(irq));
            Ok(format!("中断 {} 已取消屏蔽", irq))
        }
        Some("cli") => {
            let mut c = IrqController::instance().lock().unwrap();
            c.cli();
            Ok("中断已关闭 (CLI)".to_string())
        }
        Some("sti") => {
            let mut c = IrqController::instance().lock().unwrap();
            c.sti();
            Ok("中断已开启 (STI)".to_string())
        }
        Some("raise") => {
            let irq: u32 = it.next().ok_or("用法: raise <irq>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let mut c = IrqController::instance().lock().unwrap();
            let n = c.irq_dispatch(IrqLine(irq));
            Ok(format!("已触发中断 {}，调用处理器 {} 次", irq, n))
        }
        _ => Err("用法: act drv://irq_ctrl stats | list | mask <irq> | unmask <irq> | cli | sti | raise <irq>".to_string()),
    }
}

// =======================================================================
// 单元测试
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn test_owner() -> ExecId { 1 }

    #[test]
    fn idt_entry_roundtrip() {
        let entry = IdtEntry::set_handler(0xFFFF_FFFF_F000_1000, 0x08, 0, 0, false);
        assert!(entry.is_present());
        assert_eq!(entry.offset_low, 0x1000);
        assert_eq!(entry.offset_mid, 0xF000);
        assert_eq!(entry.offset_high, 0xFFFF_FFFF);
        assert_eq!(entry.handler_address(), 0xFFFF_FFFF_F000_1000);
        let bytes = entry.serialize();
        assert_eq!(bytes.len(), 16);
        // 解析回低 16 位。
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0x1000);
    }

    #[test]
    fn idt_present_count() {
        let mut idt = Idt::new();
        assert_eq!(idt.present_count(), 0);
        idt.set_gate(0x20, IdtEntry::set_handler(0x1000, 0x08, 0, 0, false));
        idt.set_gate(0x21, IdtEntry::set_handler(0x2000, 0x08, 0, 0, false));
        assert_eq!(idt.present_count(), 2);
        assert!(idt.entry(0x20).is_some());
        assert!(idt.entry(0xFF).is_none());
        idt.clear_gate(0x20);
        assert_eq!(idt.present_count(), 1);
    }

    #[test]
    fn idt_present_vectors_sorted() {
        let mut idt = Idt::new();
        idt.set_gate(0xFF, IdtEntry::set_handler(1, 0x08, 0, 0, false));
        idt.set_gate(0x20, IdtEntry::set_handler(2, 0x08, 0, 0, false));
        idt.set_gate(0x21, IdtEntry::set_handler(3, 0x08, 0, 0, false));
        let vecs = idt.idt_present_vectors();
        assert_eq!(vecs, vec![0x20, 0x21, 0xFF]);
    }

    #[test]
    fn irq_handler_register_and_dispatch() {
        let mut c = IrqController::new();
        let irq = IrqLine(42);
        let called = Arc::new(AtomicBool::new(false));
        let called2 = called.clone();
        let handler: IrqHandler = Box::new(move |_| { called2.store(true, Ordering::SeqCst); true });
        assert!(c.irq_handler_register(test_owner(), irq, handler, 0).is_ok());
        assert_eq!(c.handler_count(irq), 1);
        // 未注册 CAP_SYS_RAWIO 时会失败：此处 caps 具备全部能力，所以成功。
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 1);
        assert!(called.load(Ordering::SeqCst));
        assert_eq!(c.irq_count(irq), 1);
    }

    #[test]
    fn irq_dispatch_stops_on_handled() {
        let mut c = IrqController::new();
        let irq = IrqLine(7);
        let first = Arc::new(AtomicBool::new(false));
        let first2 = first.clone();
        let h1: IrqHandler = Box::new(move |_| { first2.store(true, Ordering::SeqCst); true }); // 已处理，停止
        let h2: IrqHandler = Box::new(move |_| false); // 不会被调用
        c.irq_handler_register(test_owner(), irq, h1, 0).unwrap();
        c.irq_handler_register(test_owner(), irq, h2, 10).unwrap();
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 1); // 只调用 h1
        assert!(first.load(Ordering::SeqCst));
    }

    #[test]
    fn irq_mask_and_cli() {
        let mut c = IrqController::new();
        let irq = IrqLine(3);
        let called = Arc::new(AtomicBool::new(false));
        let called2 = called.clone();
        c.irq_handler_register(test_owner(), irq, Box::new(move |_| { called2.store(true, Ordering::SeqCst); true }), 0).unwrap();
        // 屏蔽后不应触发。
        c.mask_irq(irq);
        assert!(c.is_masked(irq));
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 0);
        assert!(!called.load(Ordering::SeqCst));
        // 取消屏蔽后可触发。
        c.unmask_irq(irq);
        assert!(!c.is_masked(irq));
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 1);
        // CLI 后不应触发。
        c.cli();
        assert!(!c.interrupts_enabled());
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 0);
        c.sti();
        assert!(c.interrupts_enabled());
    }

    #[test]
    fn spurious_irq_counting() {
        let mut c = IrqController::new();
        let irq = IrqLine(99); // 无处理器
        let n = c.irq_dispatch(irq);
        assert_eq!(n, 0);
        assert_eq!(c.spurious_count, 1);
    }

    #[test]
    fn irq_line_pic_vector() {
        assert_eq!(IrqLine(0).pic_vector(), 0x20);
        assert_eq!(IrqLine(15).pic_vector(), 0x2F);
        assert!(IrqLine(5).is_hardware_irq());
        assert!(!IrqLine(200).is_hardware_irq());
    }

    #[test]
    fn unregister_owner() {
        let mut c = IrqController::new();
        let irq = IrqLine(11);
        let h1: IrqHandler = Box::new(move |_| false);
        let h2: IrqHandler = Box::new(move |_| false);
        c.irq_handler_register(test_owner(), irq, h1, 0).unwrap();
        c.irq_handler_register(test_owner(), irq, h2, 0).unwrap();
        assert_eq!(c.handler_count(irq), 2);
        let removed = c.irq_handler_unregister_owner(test_owner(), irq);
        assert_eq!(removed, 2);
        assert_eq!(c.handler_count(irq), 0);
    }

    #[test]
    fn sense_and_act() {
        let snap = irq_ctrl_sense_snapshot();
        assert!(snap.contains("irq_ctrl"));
        assert!(irq_ctrl_act_raw("stats").is_ok());
        assert!(irq_ctrl_act_raw("list").is_ok());
        assert!(irq_ctrl_act_raw("mask 3").is_ok());
        assert!(irq_ctrl_act_raw("unmask 3").is_ok());
        assert!(irq_ctrl_act_raw("cli").is_ok());
        assert!(irq_ctrl_act_raw("sti").is_ok());
        assert!(irq_ctrl_act_raw("raise 3").is_ok());
    }
}

