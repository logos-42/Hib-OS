//! 全局描述符表 (GDT) —— 最小基座。
//! x86_64 长模式下 GDT 仍需存在，但大部份段描述符为 flat 模型。
//! 注意：bootloader 已完成保护模式->长模式切换，此处仅加载 GDT 表。

use core::arch::asm;

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_middle: u8,
    access: u8,
    granularity: u8,
    base_high: u8,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct GdtPtr {
    limit: u16,
    base: u64,
}

const NULL_ENTRY: u64 = 0;
const CODE_ENTRY: u64 = 0x00AF_9A00_0000_FFFF; // 64-bit code, present, ring 0, executable
const DATA_ENTRY: u64 = 0x00CF_9200_0000_FFFF; // 64-bit data, present, ring 0, writable

static mut GDT: [GdtEntry; 3] = [GdtEntry { limit_low: 0, base_low: 0, base_middle: 0, access: 0, granularity: 0, base_high: 0}; 3];

fn entry_from_u64(val: u64) -> GdtEntry {
    unsafe { core::mem::transmute::<u64, GdtEntry>(val) }
}

/// 初始化 GDT 并加载段寄存器。
pub fn init() {
    unsafe {
        GDT[0] = entry_from_u64(NULL_ENTRY);
        GDT[1] = entry_from_u64(CODE_ENTRY);
        GDT[2] = entry_from_u64(DATA_ENTRY);

        let ptr = GdtPtr {
            limit: (core::mem::size_of::<[GdtEntry; 3]>() - 1) as u16,
            base: GDT.as_ptr() as u64,
        };

        asm!("lgdt [{}]", in(reg) &ptr as *const GdtPtr, options(readonly, nostack));
    }
}
