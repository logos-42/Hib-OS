//! 图形输出模块（no_std，GPU Primacy 基础）。
//!
//! 提供基本的像素操作和文本渲染，封装已有的 VGA 文本模式驱动。
//! 后续可扩展为真实帧缓冲/GPU 输出。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::drivers::vga_text::{self, VgaWriter, WRITER};

/// 屏幕分辨率（字符）。
pub const SCREEN_COLS: usize = 80;
pub const SCREEN_ROWS: usize = 25;

/// 颜色定义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
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

/// 屏幕上的一个字符单元。
#[derive(Clone, Copy, Debug)]
pub struct CharCell {
    pub ch: u8,
    pub fg: Color,
    pub bg: Color,
}

/// 屏幕缓冲区。
pub struct Screen {
    cells: [[CharCell; SCREEN_COLS]; SCREEN_ROWS],
    cursor_x: usize,
    cursor_y: usize,
}

impl Screen {
    pub fn new() -> Self {
        let blank = CharCell {
            ch: b' ',
            fg: Color::LightGray,
            bg: Color::Black,
        };
        Self {
            cells: [[blank; SCREEN_COLS]; SCREEN_ROWS],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    pub fn clear(&mut self) {
        let blank = CharCell {
            ch: b' ',
            fg: Color::LightGray,
            bg: Color::Black,
        };
        for row in self.cells.iter_mut() {
            for cell in row.iter_mut() {
                *cell = blank;
            }
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    pub fn put_char(&mut self, x: usize, y: usize, ch: u8, fg: Color, bg: Color) {
        if x < SCREEN_COLS && y < SCREEN_ROWS {
            self.cells[y][x] = CharCell { ch, fg, bg };
        }
    }

    pub fn print_str(&mut self, x: usize, y: usize, s: &str, fg: Color, bg: Color) {
        for (i, b) in s.bytes().enumerate() {
            if x + i < SCREEN_COLS {
                self.put_char(x + i, y, b, fg, bg);
            }
        }
    }

    pub fn render(&self) {
        let mut writer = WRITER.lock();
        for y in 0..SCREEN_ROWS {
            for x in 0..SCREEN_COLS {
                let cell = self.cells[y][x];
                // 直接写入 VGA 缓冲区
                unsafe {
                    let idx = y * vga_text::WIDTH + x;
                    vga_text::VGA_ADDR.add(idx).write_volatile(vga_text::VgaChar::new(
                        cell.ch,
                        cell.fg,
                        cell.bg,
                    ));
                }
            }
        }
        // 移动光标到最后一行
        writer.write_str("\r\n").ok();
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局屏幕实例。
static SCREEN: OnceLock<SpinLock<Screen>> = OnceLock::new();

pub fn screen() -> &'static SpinLock<Screen> {
    SCREEN.get_or_init(|| SpinLock::new(Screen::new()))
}

/// 初始化图形输出。
pub fn init() {
    vga_text::init();
    let mut s = screen().lock();
    s.clear();
    s.print_str(0, 0, "Hib OS 0.1.0 - GPU Primacy Graphics", Color::LightGreen, Color::Black);
    s.print_str(0, 1, "Text-mode graphics buffer active", Color::LightCyan, Color::Black);
    s.render();
}

/// 绘制一行文本到屏幕。
pub fn draw_text(y: usize, text: &str, fg: Color, bg: Color) {
    let mut s = screen().lock();
    s.print_str(0, y, text, fg, bg);
    s.render();
}

/// 绘制一个像素点（字符单元）。
pub fn draw_pixel(x: usize, y: usize, color: Color) {
    if x < SCREEN_COLS && y < SCREEN_ROWS {
        let mut s = screen().lock();
        s.put_char(x, y, b' ', color, color);
        s.render();
    }
}

use crate::ksync::{SpinLock, OnceLock};
