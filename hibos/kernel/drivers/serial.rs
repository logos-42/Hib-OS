//! COM1 串口驱动 —— 最早可用的调试输出（QEMU -serial stdio 可见）。

use core::sync::atomic::{AtomicU8, Ordering};

const COM1: u16 = 0x3F8;

static INIT: AtomicU8 = AtomicU8::new(0);

fn port_out(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nostack));
    }
}

fn port_in(port: u16) -> u8 {
    let v: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nostack));
    }
    v
}

/// 初始化串口：115200 baud, 8N1。
pub fn init() {
    if INIT.swap(1, Ordering::SeqCst) == 1 {
        return;
    }
    unsafe {
        port_out(COM1 + 1, 0x00); // 禁用所有中断
        port_out(COM1 + 3, 0x80); // 启用 DLAB
        port_out(COM1 + 0, 0x01); // 除数低字节 (115200 baud)
        port_out(COM1 + 1, 0x00); // 除数高字节
        port_out(COM1 + 3, 0x03); // 8 位，无校验，1 位停止
        port_out(COM1 + 2, 0xC7); // 启用 FIFO，清空
        port_out(COM1 + 4, 0x0B); // IRQ 启用，RTS/DSR
    }
}

/// 向 COM1 发送一个字节。
fn write_byte(byte: u8) {
    unsafe {
        // 等待发送寄存器空
        loop {
            let status = port_in(COM1 + 5);
            if status & 0x20 != 0 {
                break;
            }
        }
        port_out(COM1, byte);
    }
}

/// 向 COM1 发送字符串。
pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
            write_byte(b'\n');
        } else {
            write_byte(b);
        }
    }
}

/// 全局串口写入器（供 writelock 宏使用）。
pub struct Com1Writer;

impl core::fmt::Write for Com1Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write_str(s);
        Ok(())
    }
}

pub static COM1: Com1Writer = Com1Writer;

#[macro_export]
macro_rules! writelock {
    ($target:expr, $($arg:tt)*) => {
        {
            use core::fmt::Write;
            let _ = $target.write_fmt(core::fmt::format_args!($($arg)*));
        }
    };
}
