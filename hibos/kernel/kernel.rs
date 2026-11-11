//! Hib OS —— 裸机内核入口 (no_std, x86_64)
//!
//! 这是从 "在 Windows/Linux 上跑的用户态原型" 过渡到 "真实可启动内核" 的第一阶段。
//! GPU Primacy 设计（调度器与内存管理器天生具备 GPU 感知）在基座搭完后逐步集成。

#![no_std]
#![no_main]

use core::panic::PanicInfo;

pub mod arch;
pub mod drivers;

/// 内核主入口：由 bootloader 跳转到这里。
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // 初始化最早可用的输出：COM1 串口（QEMU -serial stdio 可见）。
    drivers::serial::init();

    // 初始化 VGA 文本模式（屏幕可见）。
    drivers::vga_text::init();
    vga_println!("Hib OS 0.1.0");
    vga_println!("Booting on x86_64 (GPU-primed macro-kernel prototype)");

    // 初始化 GDT + IDT（最小中断基座，防止三重错误重启）。
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();

    serial_println!("[OK] GDT+IDT initialized");

    // 至此基座就绪，进入空闲循环（后续接入调度器）。
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("PANIC: {}", info);
    vga_println!("PANIC: {}", info);
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
    }
}

/// 向 COM1 串口打印。
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let mut w = crate::drivers::serial::Com1Writer;
            let _ = w.write_fmt(format_args!($($arg)*));
        }
    };
}

/// 向 COM1 串口打印一行（换行）。
#[macro_export]
macro_rules! serial_println {
    () => (serial_print!(""));
    ($($arg:tt)*) => (serial_print!("{}\n", $($arg)*));
}

/// 向 VGA 文本缓冲区打印。
#[macro_export]
macro_rules! vga_print {
    ($($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = crate::drivers::vga_text::writer().write_fmt(format_args!($($arg)*));
        }
    };
}

/// 向 VGA 文本缓冲区打印一行（换行）。
#[macro_export]
macro_rules! vga_println {
    () => (vga_print!(""));
    ($($arg:tt)*) => (vga_print!("{}\n", $($arg)*));
}
