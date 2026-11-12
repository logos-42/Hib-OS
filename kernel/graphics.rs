//! 图形输出模块（no_std，GPU Primacy 基础）。
//!
//! 提供基本的像素操作和文本渲染，封装已有的 VGA 文本模式驱动。
//! 后续可扩展为真实帧缓冲/GPU 输出。

use crate::drivers::vga_text::{self, Color as VgaColor, VgaChar, VGA_ADDR, WIDTH, HEIGHT};
use crate::ksync::{SpinLock, OnceLock};

// =======================================================================
// 颜色映射（从 vga_text::Color 重新包装，避免泄露内部实现）
// =======================================================================

/// 屏幕分辨率（字符）。
pub const SCREEN_COLS: usize = WIDTH;
pub const SCREEN_ROWS: usize = HEIGHT;

/// 颜色定义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Black,
    Blue,
    Green,
    Cyan,
    Red,
    Magenta,
    Brown,
    LightGray,
    DarkGray,
    LightBlue,
    LightGreen,
    LightCyan,
    LightRed,
    Pink,
    Yellow,
    White,
}

impl From<Color> for VgaColor {
    fn from(c: Color) -> Self {
        match c {
            Color::Black => VgaColor::Black,
            Color::Blue => VgaColor::Blue,
            Color::Green => VgaColor::Green,
            Color::Cyan => VgaColor::Cyan,
            Color::Red => VgaColor::Red,
            Color::Magenta => VgaColor::Magenta,
            Color::Brown => VgaColor::Brown,
            Color::LightGray => VgaColor::LightGray,
            Color::DarkGray => VgaColor::DarkGray,
            Color::LightBlue => VgaColor::LightBlue,
            Color::LightGreen => VgaColor::LightGreen,
            Color::LightCyan => VgaColor::LightCyan,
            Color::LightRed => VgaColor::LightRed,
            Color::Pink => VgaColor::Pink,
            Color::Yellow => VgaColor::Yellow,
            Color::White => VgaColor::White,
        }
    }
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
        unsafe {
            for y in 0..SCREEN_ROWS {
                for x in 0..SCREEN_COLS {
                    let cell = self.cells[y][x];
                    let idx = y * WIDTH + x;
                    VGA_ADDR.add(idx).write_volatile(VgaChar::new(
                        cell.ch,
                        cell.fg.into(),
                        cell.bg.into(),
                    ));
                }
            }
        }
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
    s.print_str(0, 0, "Hib OS 0.2.0 - GPU Primacy Graphics", Color::LightGreen, Color::Black);
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
