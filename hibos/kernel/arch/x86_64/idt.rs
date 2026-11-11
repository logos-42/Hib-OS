//! 中断描述符表 (IDT) —— 最小基座。
//! 只设置双重错误处理，防止三重错误导致 CPU 重启。

use core::arch::asm;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}

const IDT_ENTRIES: usize = 256;

static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, _reserved: 0}; IDT_ENTRIES];

/// 初始化 IDT：只安装双重错误处理器，其余留空。
pub fn init() {
    unsafe {
        let handler = double_fault_handler as u64;
        let entry = IdtEntry {
            offset_low: handler as u16,
            selector: 0x08, // 内核代码段
            ist: 0,
            type_attr: 0x8E, // 存在, 环0, 中断门, 32位
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            _reserved: 0,
        };
        IDT[8] = entry; // 向量 8 = 双重错误

        let ptr = IdtPtr {
            limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
            base: IDT.as_ptr() as u64,
        };

        asm!(
            "lidt [{}]",
            in(reg) &ptr as *const IdtPtr,
            options(readonly, nostack)
        );
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn double_fault_handler() -> ! {
    core::arch::naked_asm!("hlt");
}
