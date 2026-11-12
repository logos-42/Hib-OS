//! 文本模式桌面 —— 80×25 单元格「假显存」+ 窗口合成。
//!
//! 参考 Linux 桌面环境（GNOME / KDE / Xfce）的布局：
//! * 顶部面板：应用菜单 + 系统状态（时钟、网络、电源）
//! * 窗口：带标题栏与窗口控制按钮（关闭 × / 最小化 _ / 最大化 □）
//! * 底部任务栏：打开的窗口标签 + 系统托盘
//! * 「假显示器」窗口：实时光栅化帧缓冲，支持滚动
//!
//! `to_framebuffer()` 会把这套界面真正光栅化成 1bpp 像素帧缓冲，
//! 用于验证未来接显卡输出的通路。

use std::sync::{Mutex, OnceLock};

use crate::gpu::framebuffer::{self, Framebuffer, ADVANCE, FONT_HEIGHT, FONT_WIDTH, LINE_HEIGHT};

/// 屏幕列数。
pub const COLS: usize = 80;
/// 屏幕行数。
pub const ROWS: usize = 25;

/// Catppuccin Mocha 风格配色（Linux 桌面常用深色主题）。
pub mod color {
    pub const BASE: u8 = 0;   // #11111b
    pub const MANTLE: u8 = 8; // #181825
    pub const CRUST: u8 = 1;  // #11111b (variant)
    pub const SURFACE0: u8 = 8; // #313244
    pub const SURFACE1: u8 = 15; // #cdd6f4 (text)
    pub const OVERLAY0: u8 = 244; // #6c7086
    pub const TEXT: u8 = 15; // #cdd6f4
    pub const SUBTEXT0: u8 = 244; // #6c7086
    pub const RED: u8 = 9;    // #f38ba8
    pub const GREEN: u8 = 10; // #a6e3a1
    pub const YELLOW: u8 = 11; // #f9e2af
    pub const BLUE: u8 = 12;  // #89b4fa
    pub const MAUVE: u8 = 13; // #cba6f7
    pub const PINK: u8 = 213; // #f5c2e7
    pub const TEAL: u8 = 14;  // #94e2d5
    pub const CYAN: u8 = 14;  // #94e2d5 (alias teal)
    pub const MAGENTA: u8 = 13; // #cba6f7 (alias mauve)
    pub const WHITE: u8 = 15; // #cdd6f4
    pub const GRAY: u8 = 244; // #6c7086
    pub const DARK_GRAY: u8 = 8; // #313244
    pub const BLUE_BG: u8 = 17; // dark blue bg
    pub const GRAY_BG: u8 = 236; // dark gray bg

    /// 24-bit RGB 常量。
    pub const BASE_RGB: (u8, u8, u8) = (0x11, 0x11, 0x1b);
    pub const SURFACE0_RGB: (u8, u8, u8) = (0x31, 0x32, 0x44);
    pub const TEXT_RGB: (u8, u8, u8) = (0xcd, 0xd6, 0xf4);
    pub const SUBTEXT_RGB: (u8, u8, u8) = (0x6c, 0x70, 0x86);
    pub const BLUE_RGB: (u8, u8, u8) = (0x89, 0xb4, 0xfa);
    pub const GREEN_RGB: (u8, u8, u8) = (0xa6, 0xe3, 0xa1);
    pub const RED_RGB: (u8, u8, u8) = (0xf3, 0x8b, 0xa8);
    pub const YELLOW_RGB: (u8, u8, u8) = (0xf9, 0xe2, 0xaf);
    pub const CYAN_RGB: (u8, u8, u8) = (0x94, 0xe2, 0xd5);
    pub const MAUVE_RGB: (u8, u8, u8) = (0xcb, 0xa6, 0xf7);
    pub const PINK_RGB: (u8, u8, u8) = (0xf5, 0xc2, 0xe7);
}

/// 一个字符单元：支持 256 色索引与可选 24-bit 真彩。
#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
    pub fg_rgb: Option<(u8, u8, u8)>,
    pub bg_rgb: Option<(u8, u8, u8)>,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: ' ',
            fg: color::TEXT,
            bg: color::BASE,
            fg_rgb: None,
            bg_rgb: None,
        }
    }

    #[allow(dead_code)]
    fn with_rgb(mut self, fg: (u8, u8, u8), bg: (u8, u8, u8)) -> Self {
        self.fg_rgb = Some(fg);
        self.bg_rgb = Some(bg);
        self
    }
}

/// 80×25 字符屏幕。
pub struct Screen {
    cells: Vec<Cell>,
}

impl Screen {
    pub fn new() -> Self {
        Self {
            cells: vec![Cell::blank(); COLS * ROWS],
        }
    }

    fn idx(x: usize, y: usize) -> Option<usize> {
        if x < COLS && y < ROWS {
            Some(y * COLS + x)
        } else {
            None
        }
    }

    /// 放置单元（256 色）。
    pub fn put(&mut self, x: usize, y: usize, ch: char, fg: u8, bg: u8) {
        if let Some(i) = Self::idx(x, y) {
            self.cells[i] = Cell { ch, fg, bg, ..Cell::blank() };
        }
    }

    /// 放置单元（24-bit 真彩）。
    pub fn put_rgb(
        &mut self,
        x: usize,
        y: usize,
        ch: char,
        fg: (u8, u8, u8),
        bg: (u8, u8, u8),
    ) {
        if let Some(i) = Self::idx(x, y) {
            self.cells[i] = Cell {
                ch,
                fg: color::TEXT,
                bg: color::BASE,
                fg_rgb: Some(fg),
                bg_rgb: Some(bg),
            };
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Option<Cell> {
        Self::idx(x, y).map(|i| self.cells[i])
    }

    pub fn write(&mut self, x: usize, y: usize, text: &str, fg: u8, bg: u8) -> usize {
        let mut cx = x;
        for ch in text.chars() {
            if cx >= COLS {
                break;
            }
            self.put(cx, y, ch, fg, bg);
            cx += 1;
        }
        cx
    }

    /// 放置带真彩的文本。
    pub fn write_rgb(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        fg: (u8, u8, u8),
        bg: (u8, u8, u8),
    ) -> usize {
        let mut cx = x;
        for ch in text.chars() {
            if cx >= COLS {
                break;
            }
            self.put_rgb(cx, y, ch, fg, bg);
            cx += 1;
        }
        cx
    }

    pub fn fill_row(&mut self, y: usize, ch: char, fg: u8, bg: u8) {
        for x in 0..COLS {
            self.put(x, y, ch, fg, bg);
        }
    }

    /// 填充矩形区域（256 色）。
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, fg: u8, bg: u8) {
        for yy in y..y.saturating_add(h).min(ROWS) {
            for xx in x..x.saturating_add(w).min(COLS) {
                self.put(xx, yy, ' ', fg, bg);
            }
        }
    }

    /// 填充矩形区域（24-bit 真彩）。
    pub fn fill_rect_rgb(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        fg: (u8, u8, u8),
        bg: (u8, u8, u8),
    ) {
        for yy in y..y.saturating_add(h).min(ROWS) {
            for xx in x..x.saturating_add(w).min(COLS) {
                self.put_rgb(xx, yy, ' ', fg, bg);
            }
        }
    }

    /// 画窗口边框与标题（Linux 风格：深色标题栏 + 窗口控制按钮）。
    pub fn window(&mut self, x: usize, y: usize, w: usize, h: usize, title: &str) {
        if w < 6 || h < 4 {
            return;
        }
        let border = color::SURFACE0;
        let title_bg = color::BASE;
        let title_fg = color::TEXT;

        // 内部清空
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, ' ', color::TEXT, color::BASE);
            }
        }

        // 边框
        for xx in x..x + w {
            self.put(xx, y, '─', border, color::BASE);
            self.put(xx, y + h - 1, '─', border, color::BASE);
        }
        for yy in y..y + h {
            self.put(x, yy, '│', border, color::BASE);
            self.put(x + w - 1, yy, '│', border, color::BASE);
        }
        self.put(x, y, '┌', border, color::BASE);
        self.put(x + w - 1, y, '┐', border, color::BASE);
        self.put(x, y + h - 1, '└', border, color::BASE);
        self.put(x + w - 1, y + h - 1, '┘', border, color::BASE);

        // 标题栏：左侧标题 + 右侧控制按钮
        let label = format!(" {} ", title);
        for (i, ch) in label.chars().enumerate() {
            if x + 2 + i < x + w - 6 {
                self.put(x + 2 + i, y, ch, title_fg, title_bg);
            }
        }
        // 控制按钮：[×][_][□]
        let btn_x = x + w - 6;
        self.put(btn_x, y, '×', color::RED, title_bg);
        self.put(btn_x + 2, y, '_', color::YELLOW, title_bg);
        self.put(btn_x + 4, y, '□', color::GREEN, title_bg);
    }

    /// 画面板（无标题，纯色填充）。
    pub fn panel(&mut self, x: usize, y: usize, w: usize, h: usize, bg: u8) {
        for yy in y..y.saturating_add(h).min(ROWS) {
            for xx in x..x.saturating_add(w).min(COLS) {
                self.put(xx, yy, ' ', color::SUBTEXT0, bg);
            }
        }
    }

    /// 渲染为可打印的 ANSI 字符串（优先 24-bit 真彩，降级 256 色）。
    pub fn render(&self) -> String {
        let mut out = String::from("\x1b[H");
        let mut cur_fg = 255u8;
        let mut cur_bg = 255u8;
        let mut cur_fg_rgb: Option<(u8, u8, u8)> = None;
        let mut cur_bg_rgb: Option<(u8, u8, u8)> = None;

        fn emit_rgb(
            out: &mut String,
            cur: &mut Option<(u8, u8, u8)>,
            is_fg: bool,
            rgb: (u8, u8, u8),
        ) {
            if *cur != Some(rgb) {
                let prefix = if is_fg { "\x1b[38;2;" } else { "\x1b[48;2;" };
                out.push_str(&format!(
                    "{}{};{};{}m",
                    prefix, rgb.0, rgb.1, rgb.2
                ));
                *cur = Some(rgb);
            }
        }

        fn emit_256(out: &mut String, cur: &mut u8, is_fg: bool, code: u8) {
            if *cur != code {
                let prefix = if is_fg { "\x1b[38;5;" } else { "\x1b[48;5;" };
                out.push_str(&format!("{}{}", prefix, code));
                *cur = code;
            }
        }

        for y in 0..ROWS {
            for x in 0..COLS {
                let cell = self.cells[y * COLS + x];
                match (cell.fg_rgb, cell.bg_rgb) {
                    (Some(fg), Some(bg)) => {
                        emit_rgb(&mut out, &mut cur_fg_rgb, true, fg);
                        emit_rgb(&mut out, &mut cur_bg_rgb, false, bg);
                        cur_fg = 255;
                        cur_bg = 255;
                    }
                    _ => {
                        cur_fg_rgb = None;
                        cur_bg_rgb = None;
                        out.push_str("\x1b[39m\x1b[49m");
                        emit_256(&mut out, &mut cur_fg, true, cell.fg);
                        emit_256(&mut out, &mut cur_bg, false, cell.bg);
                    }
                }
                out.push(cell.ch);
            }
            out.push_str("\x1b[0m");
            cur_fg = 255;
            cur_bg = 255;
            cur_fg_rgb = None;
            cur_bg_rgb = None;
            if y + 1 < ROWS {
                out.push_str("\r\n");
            }
        }
        out.push_str("\x1b[0m");
        out
    }

    /// 光栅化成像素帧缓冲（含制表符连线与字模）。
    pub fn to_framebuffer(&self, scale: usize) -> Framebuffer {
        let scale = scale.max(1);
        let cw = ADVANCE * scale;
        let ch = LINE_HEIGHT * scale;
        let mut fb = Framebuffer::new(COLS * cw, ROWS * ch);
        for y in 0..ROWS {
            for x in 0..COLS {
                let cell = self.cells[y * COLS + x];
                blit(&mut fb, x * cw, y * ch, cell.ch, scale, cw, ch);
            }
        }
        fb
    }
}

fn blit(fb: &mut Framebuffer, x0: usize, y0: usize, ch: char, scale: usize, cw: usize, chh: usize) {
    let t = scale.max(1);
    let mid_x = x0 + cw / 2;
    let mid_y = y0 + chh / 2;
    match ch {
        ' ' => {}
        '█' => fb.fill_rect(x0, y0, cw, chh),
        '▀' => fb.fill_rect(x0, y0, cw, chh / 2),
        '▄' => fb.fill_rect(x0, y0 + chh / 2, cw, chh - chh / 2),
        '─' => {
            for k in 0..t {
                fb.hline(x0, x0 + cw, mid_y + k);
            }
        }
        '│' => {
            for k in 0..t {
                fb.vline(mid_x + k, y0, y0 + chh);
            }
        }
        '┌' => {
            for k in 0..t {
                fb.hline(mid_x, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, mid_y, y0 + chh);
            }
        }
        '┐' => {
            for k in 0..t {
                fb.hline(x0, mid_x + t, mid_y + k);
                fb.vline(mid_x + k, mid_y, y0 + chh);
            }
        }
        '└' => {
            for k in 0..t {
                fb.hline(mid_x, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, y0, mid_y + t);
            }
        }
        '┘' => {
            for k in 0..t {
                fb.hline(x0, mid_x + t, mid_y + k);
                fb.vline(mid_x + k, y0, mid_y + t);
            }
        }
        '├' => {
            for k in 0..t {
                fb.hline(mid_x, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, y0, y0 + chh);
            }
        }
        '┤' => {
            for k in 0..t {
                fb.hline(x0, mid_x + t, mid_y + k);
                fb.vline(mid_x + k, y0, y0 + chh);
            }
        }
        '┬' => {
            for k in 0..t {
                fb.hline(x0, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, mid_y, y0 + chh);
            }
        }
        '┴' => {
            for k in 0..t {
                fb.hline(x0, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, y0, mid_y + t);
            }
        }
        '┼' => {
            for k in 0..t {
                fb.hline(x0, x0 + cw, mid_y + k);
                fb.vline(mid_x + k, y0, y0 + chh);
            }
        }
        _ => {
            let rows = framebuffer::glyph(ch);
            for dy in 0..FONT_HEIGHT {
                for dx in 0..FONT_WIDTH {
                    if rows[dy] & (1 << (FONT_WIDTH - 1 - dx)) != 0 {
                        fb.fill_rect(x0 + dx * scale, y0 + dy * scale, scale, scale);
                    }
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

/// 桌面需要展示的数据 (由 shell 填充，避免循环依赖)。
#[derive(Clone, Debug, Default)]
pub struct DesktopInfo {
    pub cwd: String,
    pub concepts: usize,
    pub tokens: u64,
    pub active_tokens: u64,
    pub tasks_ok: u64,
    pub tasks_fail: u64,
    pub ready: usize,
    pub vram_used_gib: f64,
    pub vram_total_gib: f64,
    pub partitions: usize,
    pub dirs: usize,
    pub files: usize,
    pub modules: usize,
    pub uptime_ms: f64,
    /// (通道名, 类别, 感知+行动次数)
    pub channels: Vec<(String, String, u64)>,
    /// 顶部面板：系统时间
    pub panel_time: String,
    /// 顶部面板：网络状态
    pub panel_net: String,
    /// 帧缓冲预览滚动偏移 Y（像素行）
    pub fb_scroll_y: usize,
    /// 帧缓冲预览滚动偏移 X（像素列）
    pub fb_scroll_x: usize,
    /// 当前壁纸名称
    pub wallpaper: String,
    /// 桌面上的 App 窗口 (名称列表)
    pub apps: Vec<String>,
}

impl DesktopInfo {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 绘制整个桌面并返回屏幕。
pub fn compose(info: &DesktopInfo) -> Screen {
    let mut s = Screen::new();

    // ==================== 桌面壁纸 ====================
    draw_wallpaper(&mut s, &info.wallpaper);

    // ==================== 顶部面板 (Top Bar) ====================
    s.fill_row(0, ' ', color::TEXT, color::BASE);
    // 应用菜单 / 活动
    s.write_rgb(1, 0, "Activities", color::BLUE_RGB, color::BASE_RGB);
    s.write(30, 0, "HIB OS  v0.2.0", color::SUBTEXT0, color::BASE);
    // 右侧系统状态
    let net = format!("{}", info.panel_net);
    s.write(COLS - net.chars().count() - 2, 0, &net, color::TEAL, color::BASE);
    let time = format!("{}", info.panel_time);
    s.write(COLS - time.chars().count() - 1, 0, &time, color::TEXT, color::BASE);

    // 顶部面板下方分隔线
    for x in 0..COLS {
        s.put(x, 1, '─', color::SURFACE0, color::BASE);
    }

    // ==================== 左窗口：系统监视器 ====================
    s.window(0, 2, 39, 9, "SYSTEM MONITOR");
    let rows = [
        ("TOKENS", format!("{} (ACTIVE {})", info.tokens, info.active_tokens)),
        ("TASKS", format!("{} OK / {} FAIL", info.tasks_ok, info.tasks_fail)),
        ("READY QUEUE", format!("{}", info.ready)),
        ("MODULES", format!("{}", info.modules)),
        ("UPTIME", format!("{:.2} MS", info.uptime_ms)),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let y = 4 + i;
        s.write(2, y, k, color::CYAN, color::BASE);
        let vx = 18;
        s.write(vx, y, v, color::TEXT, color::BASE);
    }
    // 显存进度条
    let ratio = if info.vram_total_gib > 0.0 {
        (info.vram_used_gib / info.vram_total_gib).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let bar_w = 26usize;
    let filled = (ratio * bar_w as f64).round() as usize;
    let y = 9;
    s.write(2, y, "VRAM", color::CYAN, color::BASE);
    s.write(3, y, "[", color::SUBTEXT0, color::BASE);
    for i in 0..bar_w {
        let ch = if i < filled { '█' } else { '░' };
        let fg = if i < filled { color::GREEN } else { color::SURFACE0 };
        s.put(4 + i, y, ch, fg, color::BASE);
    }
    s.write(4 + bar_w, y, "]", color::SUBTEXT0, color::BASE);

    // ==================== 右窗口：感知通道 ====================
    s.window(40, 2, 40, 9, "PERCEPTION CHANNELS");
    s.write(42, 4, "CHANNEL", color::CYAN, color::BASE);
    s.write(64, 4, "KIND", color::CYAN, color::BASE);
    s.write(74, 4, "IOPS", color::CYAN, color::BASE);
    if info.channels.is_empty() {
        s.write(42, 5, "(EMPTY)", color::OVERLAY0, color::BASE);
    } else {
        for (i, (name, kind, hits)) in info.channels.iter().take(5).enumerate() {
            let y = 5 + i;
            let short: String = name.chars().take(21).collect();
            s.write(42, y, &short, color::TEXT, color::BASE);
            let k: String = kind.chars().take(9).collect();
            s.write(64, y, &k, color::MAUVE, color::BASE);
            s.write(74, y, &format!("{}", hits), color::YELLOW, color::BASE);
        }
    }

    // ==================== 底部：文件系统 + 显示器 ====================
    // 左侧：文件系统 (39x7)
    s.window(0, 12, 39, 7, "FILESYSTEM");
    s.write(2, 13, "CWD", color::CYAN, color::BASE);
    let cwd: String = info.cwd.chars().take(30).collect();
    s.write(8, 13, &cwd, color::TEXT, color::BASE);
    s.write(2, 14, "TREE", color::CYAN, color::BASE);
    s.write(
        8,
        14,
        &format!("{} DIRS / {} FILES", info.dirs, info.files),
        color::TEXT,
        color::BASE,
    );
    s.write(2, 15, "MOUNT", color::CYAN, color::BASE);
    s.write(
        8,
        15,
        "/ -> RAMFS | /DEV/*",
        color::TEXT,
        color::BASE,
    );
    s.write(2, 16, "DEV", color::CYAN, color::BASE);
    s.write(
        8,
        16,
        "GPU0 IOMMU0 NULL DISK",
        color::TEXT,
        color::BASE,
    );
    s.write(2, 17, "NET", color::CYAN, color::BASE);
    s.write(8, 17, "eth0 10.0.0.2", color::TEXT, color::BASE);

    // 右侧：显示器窗口 (40x7) —— 可滚动帧缓冲预览
    s.window(40, 12, 40, 7, "DISPLAY :0");
    draw_monitor(&mut s, 40, 12, 40, 7, info);

    // ==================== 底部任务栏 ====================
    s.fill_row(23, ' ', color::TEXT, color::MANTLE);
    // 任务栏按钮
    s.write_rgb(1, 23, " [HIB OS] ", color::BASE_RGB, color::BLUE_RGB);
    s.write(12, 23, " [SHELL] ", color::SUBTEXT0, color::MANTLE);
    s.write(22, 23, " [VFS] ", color::SUBTEXT0, color::MANTLE);
    s.write(30, 23, " [GPU] ", color::SUBTEXT0, color::MANTLE);
    s.write(38, 23, " [NET] ", color::SUBTEXT0, color::MANTLE);
    // 右侧系统托盘
    let tray = "  ⌘  ▲  ⏻";
    s.write(COLS - tray.chars().count() - 1, 23, tray, color::TEXT, color::MANTLE);

    // ==================== 状态行 ====================
    s.fill_row(24, ' ', color::SUBTEXT0, color::BASE);
    s.write(
        1,
        24,
        &format!(
            "{} CONCEPTS  |  {} TOKENS  |  VRAM {:.2}/{:.2} GIB  |  UPTIME {:.1} MS  |  FB scroll ({},{})",
            info.concepts,
            info.tokens,
            info.vram_used_gib,
            info.vram_total_gib,
            info.uptime_ms,
            info.fb_scroll_x,
            info.fb_scroll_y
        ),
        color::SUBTEXT0,
        color::BASE,
    );

    s
}

/// 绘制桌面壁纸（在窗口之前绘制，作为背景）。
/// 支持两个内置壁纸："default"（Hib OS GPU 核心）和 "deepseek"（AI 意识）。
/// 未知名称回退到纯色背景。
fn draw_wallpaper(s: &mut Screen, name: &str) {
    match name {
        "default" | "hibos" | "circuit" | "sunset" | "grid" | "solid" => {
            draw_wallpaper_hibos(s)
        }
        "deepseek" | "ai" | "consciousness" | "aurora" | "nebula" => {
            draw_wallpaper_deepseek(s)
        }
        _ => {
            // 纯色渐变背景
            for y in 2..ROWS.saturating_sub(1) {
                for x in 0..COLS {
                    let bg = if (x + y) % 3 == 0 {
                        color::SURFACE0_RGB
                    } else {
                        color::BASE_RGB
                    };
                    s.put_rgb(x, y, ' ', bg, bg);
                }
            }
        }
    }
}

/// 壁纸 1：Hib OS GPU 核心 —— 抽象的 GPU 核心与数据流。
fn draw_wallpaper_hibos(s: &mut Screen) {
    // 底色：深蓝紫
    for y in 2..ROWS.saturating_sub(1) {
        for x in 0..COLS {
            s.put_rgb(x, y, ' ', color::BASE_RGB, color::BASE_RGB);
        }
    }

    // 中心 GPU 核心 (用方块字符渲染)
    let core = [
        "  ██████  ",
        " █████████ ",
        "████  ██  ███",
        "██  ████  ███",
        "████  ██  ███",
        " █████████ ",
        "  ██████  ",
    ];
    let cx = COLS / 2 - 5;
    let cy = ROWS / 2 - 3;
    for (dy, row) in core.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch != ' ' {
                let x = cx + dx;
                let y = cy + dy;
                if x < COLS && y < ROWS.saturating_sub(1) {
                    s.put_rgb(x, y, ch, color::BLUE_RGB, color::BASE_RGB);
                }
            }
        }
    }

    // 数据流线
    let flow = "▬▬▬ data ▬▬▬";
    let fy = cy + 4;
    if fy < ROWS.saturating_sub(1) {
        for (dx, ch) in flow.chars().enumerate() {
            let x = cx + 1 + dx;
            if x < COLS {
                s.put_rgb(x, fy, ch, color::CYAN_RGB, color::BASE_RGB);
            }
        }
    }

    // 角落装饰
    for (x, y) in [(0, 2), (COLS - 1, 2), (0, ROWS - 2), (COLS - 1, ROWS - 2)] {
        if y < ROWS {
            s.put(x, y, '◆', color::SURFACE0, color::BASE);
        }
    }
}

/// 壁纸 2：DeepSeek AI 意识 —— 神经元连接与意识网络。
fn draw_wallpaper_deepseek(s: &mut Screen) {
    // 底色：极深青黑
    for y in 2..ROWS.saturating_sub(1) {
        for x in 0..COLS {
            s.put_rgb(x, y, ' ', color::BASE_RGB, color::BASE_RGB);
        }
    }

    // 中心意识节点
    let node = [
        "   🧠   ",
        "  ╱╲╱╲  ",
        " ╱  ●  ╲ ",
        "╱   ●   ╲",
        " ╲  ●  ╱ ",
        "  ╲╱╲╱  ",
        "   🧠   ",
    ];
    let cx = COLS / 2 - 4;
    let cy = ROWS / 2 - 3;
    for (dy, row) in node.iter().enumerate() {
        for (dx, ch) in row.chars().enumerate() {
            if ch != ' ' {
                let x = cx + dx;
                let y = cy + dy;
                if x < COLS && y < ROWS.saturating_sub(1) {
                    let fg = if ch == '●' { color::PINK_RGB } else { color::MAUVE_RGB };
                    s.put_rgb(x, y, ch, fg, color::BASE_RGB);
                }
            }
        }
    }

    // 神经元连接线
    let connections = [
        (cx, cy, 2, 2),
        (cx + 7, cy, 2, 2),
        (cx, cy + 6, 2, 2),
        (cx + 7, cy + 6, 2, 2),
    ];
    for (sx, sy, ex, ey) in connections {
        let steps = ((ex as isize - sx as isize).abs().max((ey as isize - sy as isize).abs()) as usize).max(1);
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let x = (sx as f64 + (ex as f64 - sx as f64) * t) as usize;
            let y = (sy as f64 + (ey as f64 - sy as f64) * t) as usize;
            if x < COLS && y < ROWS.saturating_sub(1) && y >= 2 {
                s.put_rgb(x, y, '·', color::SUBTEXT_RGB, color::BASE_RGB);
            }
        }
    }

    // 标语
    let slogan = " DeepSeek AI · Unified Perception ";
    let sy = cy + 8;
    if sy < ROWS.saturating_sub(1) {
        let sx = cx_or_center(COLS, slogan.len());
        for (dx, ch) in slogan.chars().enumerate() {
            let x = sx + dx;
            if x < COLS {
                s.put_rgb(x, sy, ch, color::TEXT_RGB, color::BASE_RGB);
            }
        }
    }
}

/// 计算居中起始位置。
fn cx_or_center(cols: usize, len: usize) -> usize {
    cols.saturating_sub(len) / 2
}
fn draw_monitor(
    s: &mut Screen,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    info: &DesktopInfo,
) {
    let inner_w = w.saturating_sub(2);
    let inner_h = h.saturating_sub(2);
    if inner_w == 0 || inner_h == 0 {
        return;
    }

    // 标题栏下方：显示器状态行
    let (fw, fh, _bytes, lit) = framebuffer_stats();
    let stat = format!("{}x{} lit={}", fw, fh, lit);
    s.write(x + 2, y + 1, &stat, color::SUBTEXT0, color::BASE);

    let fb = framebuffer().lock().unwrap();
    // 每两行像素对应一行字符（半块字符）
    let char_rows = inner_h.saturating_sub(1);
    for dy in 0..char_rows {
        let src_y = (info.fb_scroll_y + dy * 2).min(fh.saturating_sub(1));
        let src_y_next = (src_y + 1).min(fh.saturating_sub(1));
        for dx in 0..inner_w {
            let src_x = (info.fb_scroll_x + dx).min(fw.saturating_sub(1));
            let top = fb.get(src_x, src_y);
            let bottom = fb.get(src_x, src_y_next);
            let (ch, fg_rgb, bg_rgb) = match (top, bottom) {
                (true, true) => ('█', color::BLUE_RGB, color::BASE_RGB),
                (true, false) => ('▀', color::BLUE_RGB, color::BASE_RGB),
                (false, true) => ('▄', color::GREEN_RGB, color::BASE_RGB),
                (false, false) => (' ', color::SURFACE0_RGB, color::BASE_RGB),
            };
            s.put_rgb(
                x + 1 + dx,
                y + 2 + dy,
                ch,
                fg_rgb,
                bg_rgb,
            );
        }
    }
    drop(fb);

    if lit == 0 {
        s.write(
            x + 2,
            y + 3,
            "(空帧 — 先用 fbpreview / act driver://framebuffer 填充)",
            color::OVERLAY0,
            color::BASE,
        );
    }
}

/// 渲染为 ANSI 字符串（含清屏与光标隐藏）。
pub fn render(info: &DesktopInfo) -> String {
    let screen = compose(info);
    format!("\x1b[2J\x1b[?25l{}{}", screen.render(), "\x1b[?25h")
}

/// 全局帧缓冲 (感知通道 `gpu://framebuffer` 的实体)。
pub fn framebuffer() -> &'static Mutex<Framebuffer> {
    static FB: OnceLock<Mutex<Framebuffer>> = OnceLock::new();
    FB.get_or_init(|| Mutex::new(Framebuffer::new(COLS * ADVANCE, ROWS * LINE_HEIGHT)))
}

/// 帧缓冲统计: (宽, 高, 字节数, 点亮像素)。
    pub fn framebuffer_stats() -> (usize, usize, usize, usize) {
        let fb = framebuffer().lock().unwrap();
        (fb.width(), fb.height(), fb.byte_len(), fb.lit_pixels())
    }

/// 向全局帧缓冲绘制文本 (行动)，返回结束 x 与点亮像素数。
pub fn framebuffer_draw(x: usize, y: usize, text: &str) -> (usize, usize) {
    let mut fb = framebuffer().lock().unwrap();
    let end = fb.draw_text(x, y, text);
    (end, fb.lit_pixels())
}

/// 全局帧缓冲的缩略图 (ANSI 真彩)。
pub fn framebuffer_preview(cols: usize, rows: usize) -> String {
    let fb = framebuffer().lock().unwrap();
    fb.preview_ansi(cols, rows)
}

/// 用当前桌面内容刷新全局帧缓冲。
pub fn refresh_framebuffer(info: &DesktopInfo) {
    let screen = compose(info);
    let rendered = screen.to_framebuffer(1);
    let mut fb = framebuffer().lock().unwrap();
    *fb = rendered;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_cell_addressing() {
        let mut s = Screen::new();
        s.put_rgb(10, 5, 'X', color::RED_RGB, color::BASE_RGB);
        let c = s.get(10, 5).unwrap();
        assert_eq!(c.ch, 'X');
        assert_eq!(c.fg_rgb, Some(color::RED_RGB));
        assert_eq!(c.bg_rgb, Some(color::BASE_RGB));
        assert!(s.get(COLS, 0).is_none());
        assert!(s.get(0, ROWS).is_none());
        s.put_rgb(1000, 1000, 'Y', color::TEXT_RGB, color::BASE_RGB);
    }

    #[test]
    fn write_clips_at_edge() {
        let mut s = Screen::new();
        let end = s.write(COLS - 2, 0, "ABCDE", color::TEXT, color::BASE);
        assert_eq!(end, COLS);
        assert_eq!(s.get(COLS - 1, 0).unwrap().ch, 'B');
    }

    #[test]
    fn window_draws_frame_and_title() {
        let mut s = Screen::new();
        s.window(2, 2, 20, 6, "TEST");
        assert_eq!(s.get(2, 2).unwrap().ch, '┌');
        assert_eq!(s.get(21, 2).unwrap().ch, '┐');
        assert_eq!(s.get(2, 7).unwrap().ch, '└');
        assert_eq!(s.get(21, 7).unwrap().ch, '┘');
        // 控制按钮：x + w - 6, x + w - 4, x + w - 2
        assert_eq!(s.get(16, 2).unwrap().ch, '×');
        assert_eq!(s.get(18, 2).unwrap().ch, '_');
        assert_eq!(s.get(20, 2).unwrap().ch, '□');
        let title: String = (4..10).map(|x| s.get(x, 2).unwrap().ch).collect();
        assert!(title.contains('T'));
    }

    #[test]
    fn compose_fills_screen() {
        let info = DesktopInfo {
            channels: vec![
                ("context://ontology".into(), "context".into(), 3),
                ("gpu://vram".into(), "gpu".into(), 1),
            ],
            tokens: 3,
            panel_time: "15:30".into(),
            panel_net: "eth0 up".into(),
            ..Default::default()
        };
        let s = compose(&info);
        let text: String = (0..COLS).map(|x| s.get(x, 0).unwrap().ch).collect();
        assert!(text.contains("Activities"));
        assert!(text.contains("15:30"));
        let found = (0..ROWS).any(|y| {
            (0..COLS)
                .map(|x| s.get(x, y).unwrap().ch)
                .collect::<String>()
                .contains("CHANNEL")
        });
        assert!(found);
        // 显示器窗口存在
        let mon_found = (0..ROWS).any(|y| {
            (0..COLS)
                .map(|x| s.get(x, y).unwrap().ch)
                .collect::<String>()
                .contains("DISPLAY")
        });
        assert!(mon_found);
    }

    #[test]
    fn render_contains_ansi_and_all_rows() {
        let out = render(&DesktopInfo::default());
        assert!(out.contains("\x1b[2J"));
        assert!(out.contains("\x1b[38;2;"));
        assert!(out.matches("\r\n").count() >= ROWS - 1);
    }

    #[test]
    fn framebuffer_rasterizes_screen() {
        let s = compose(&DesktopInfo::default());
        let fb = s.to_framebuffer(2);
        assert_eq!(fb.width(), COLS * ADVANCE * 2);
        assert_eq!(fb.height(), ROWS * LINE_HEIGHT * 2);
        assert!(fb.lit_pixels() > 5000, "点亮像素 {}", fb.lit_pixels());
    }

    #[test]
    fn framebuffer_preview_is_smaller() {
        let s = compose(&DesktopInfo::default());
        let fb = s.to_framebuffer(1);
        let p = fb.preview_ansi(40, 10);
        assert_eq!(p.lines().count(), 10);
        assert!(p.contains("\x1b[38;2;"));
        assert!(p.chars().any(|c| c == '█' || c == '▀' || c == '▄'));
    }

    #[test]
    fn monitor_scroll_respects_bounds() {
        let mut s = Screen::new();
        let info = DesktopInfo {
            fb_scroll_y: 1000,
            fb_scroll_x: 1000,
            ..Default::default()
        };
        draw_monitor(&mut s, 40, 12, 40, 7, &info);
        // 不应 panicking，越界被 clamp
        let _ = s.get(41, 13).unwrap();
    }
}
