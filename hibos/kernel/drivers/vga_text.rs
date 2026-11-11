//! VGA 文本模式驱动 —— 屏幕可见输出（0xB8000）。

use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

const VGA_ADDR: *mut u16 = 0xB8000 as *mut u16;
const WIDTH: usize = 80;
const HEIGHT: usize = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
struct VgaChar {
    ascii: u8,
    color: u8,
}

impl VgaChar {
    fn new(ascii: u8, fg: Color, bg: Color) -> Self {
        VgaChar {
            ascii,
            color: (bg as u8) << 4 | (fg as u8),
        }
    }
}

static ROW: AtomicUsize = AtomicUsize::new(0);
static COL: AtomicUsize = AtomicUsize::new(0);

fn vga_write_byte(byte: u8) {
    match byte {
        b'\n' => newline(),
        byte => {
            let row = ROW.load(Ordering::Relaxed);
            let col = COL.load(Ordering::Relaxed);
            if col >= WIDTH {
                newline();
            }
            let row = ROW.load(Ordering::Relaxed);
            let col = COL.load(Ordering::Relaxed);
            let idx = row * WIDTH + col;
            unsafe {
                VGA_ADDR.add(idx).write_volatile(VgaChar::new(byte, Color::LightGreen, Color::Black));
            }
            COL.store(col + 1, Ordering::Relaxed);
        }
    }
}

fn newline() {
    let row = ROW.load(Ordering::Relaxed);
    ROW.store(row + 1, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
    if row + 1 >= HEIGHT {
        // 简单滚屏：把每行向上移一行
        for r in 1..HEIGHT {
            for c in 0..WIDTH {
                let src = (r * WIDTH + c) as isize;
                let dst = ((r - 1) * WIDTH + c) as isize;
                unsafe {
                    VGA_ADDR.add(dst as usize).write_volatile(VGA_ADDR.add(src as usize).read_volatile());
                }
            }
        }
        for c in 0..WIDTH {
            let idx = (HEIGHT - 1) * WIDTH + c;
            unsafe {
                VGA_ADDR.add(idx).write_volatile(VgaChar::new(b' ', Color::LightGray, Color::Black));
            }
        }
        ROW.store(HEIGHT - 1, Ordering::Relaxed);
        COL.store(0, Ordering::Relaxed);
    }
}

/// 清屏。
pub fn clear() {
    for i in 0..(WIDTH * HEIGHT) {
        unsafe {
            VGA_ADDR.add(i).write_volatile(VgaChar::new(b' ', Color::LightGray, Color::Black));
        }
    }
    ROW.store(0, Ordering::Relaxed);
    COL.store(0, Ordering::Relaxed);
}

/// 初始化 VGA 文本模式。
pub fn init() {
    clear();
}

/// 返回全局 VGA 写入器（供 writelock 宏使用）。
pub struct VgaWriter;

impl Write for VgaWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            vga_write_byte(b);
        }
        Ok(())
    }
}

pub static VGA: VgaWriter = VgaWriter;

pub fn writer() -> &'static VgaWriter {
    &VGA
}
