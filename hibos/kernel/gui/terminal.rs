//! 终端仿真器 —— ANSI 转义码解析、字符单元格缓冲、滚动历史、选中与剪贴板。
//!
//! `Terminal` 提供：
//! * `write(data)` —— 写入原始字节，交给 `AnsiParser` 解析
//! * `resize(rows, cols)` / `scroll(n)` / `select(start, end)` / `copy()` / `paste()` / `clear()` / `reset()`
//! * `AnsiParser`：光标移动、颜色设置、屏幕清除、显示/隐藏光标、字符集、键盘键定义
//! * `TerminalBuffer`：字符单元格（字符、前景色、背景色、属性）、光标位置、滚动区、scrollback
//!
//! 颜色支持 16 色 / 256 色 / 真彩色；光标样式：块 / 下划线 / 条。
//! 与 `kernel/gui/window.rs` 的 `TerminalModel` 互补：这里是底层仿真，window.rs 是上层 UI 包装。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 颜色
// ============================================================================

/// 终端颜色：16 色索引、256 色调色板、或真彩色 RGB。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    /// 标准 16 色索引 (0-15)。
    Index(u8),
    /// 256 色调色板索引 (0-255)。
    Palette256(u8),
    /// 真彩色 RGB。
    Rgb(u8, u8, u8),
    /// 默认前景色。
    #[default]
    DefaultFg,
    /// 默认背景色。
    DefaultBg,
}

impl Color {
    pub fn name(self) -> &'static str {
        match self {
            Color::Index(i) => match i {
                0 => "黑",
                1 => "红",
                2 => "绿",
                3 => "黄",
                4 => "蓝",
                5 => "品红",
                6 => "青",
                7 => "白",
                8 => "亮黑",
                9 => "亮红",
                10 => "亮绿",
                11 => "亮黄",
                12 => "亮蓝",
                13 => "亮品红",
                14 => "亮青",
                15 => "亮白",
                _ => "未知索引",
            },
            Color::Palette256(i) => match i {
                0 => "256色#0",
                231 => "256色#231",
                _ => "256色",
            },
            Color::Rgb(r, g, b) => "真彩色",
            Color::DefaultFg => "默认前景",
            Color::DefaultBg => "默认背景",
        }
    }
}

/// 字符单元格属性（粗体、下划线、反转、闪烁）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellAttrs {
    pub bold: bool,
    pub underline: bool,
    pub reverse: bool,
    pub blink: bool,
    pub italic: bool,
    pub hidden: bool,
}

/// 字符单元格。
#[derive(Clone, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

impl Cell {
    pub fn blank() -> Self {
        Self {
            ch: ' ',
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            attrs: CellAttrs::default(),
        }
    }

    pub fn with_char(ch: char) -> Self {
        Self {
            ch,
            ..Cell::blank()
        }
    }
}

// ============================================================================
// 光标
// ============================================================================

/// 光标样式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

/// 光标状态。
#[derive(Clone, Debug)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub style: CursorStyle,
    pub blink: bool,
}

impl Cursor {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            style: CursorStyle::Block,
            blink: true,
        }
    }

    pub fn clamp(&mut self, rows: usize, cols: usize) {
        self.row = self.row.min(rows.saturating_sub(1));
        self.col = self.col.min(cols.saturating_sub(1));
    }
}

// ============================================================================
// 终端缓冲区
// ============================================================================

/// 终端缓冲区：可见屏幕 + scrollback 历史行。
pub struct TerminalBuffer {
    pub rows: usize,
    pub cols: usize,
    /// 可见屏幕单元格 (rows × cols)。
    pub grid: Vec<Vec<Cell>>,
    /// scrollback 历史（最早在前）。
    pub scrollback: VecDeque<Vec<Cell>>,
    /// 最大 scrollback 行数。
    pub max_scrollback: usize,
    pub cursor: Cursor,
    /// 滚动区顶部（0 = 整屏）。
    pub scroll_region_top: usize,
    /// 滚动区底部。
    pub scroll_region_bottom: usize,
    /// 自动换行。
    pub wrap: bool,
    /// 插入模式。
    pub insert_mode: bool,
    /// 原始模式（不解析 ANSI）。
    pub raw_mode: bool,
}

impl TerminalBuffer {
    pub fn new(rows: usize, cols: usize) -> Self {
        let grid = vec![vec![Cell::blank(); cols]; rows];
        Self {
            rows,
            cols,
            grid,
            scrollback: VecDeque::new(),
            max_scrollback: 1000,
            cursor: Cursor::new(rows, cols),
            scroll_region_top: 0,
            scroll_region_bottom: rows.saturating_sub(1),
            wrap: true,
            insert_mode: false,
            raw_mode: false,
        }
    }

    /// 重设大小：保留内容，新增行/列填空白，超出部分截断。
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let old_rows = self.rows;
        let old_cols = self.cols;
        self.rows = rows;
        self.cols = cols;

        // 调整每行列数。
        for row in self.grid.iter_mut() {
            if row.len() < cols {
                row.resize(cols, Cell::blank());
            } else {
                row.truncate(cols);
            }
        }
        // 调整行数。
        if self.grid.len() < rows {
            for _ in self.grid.len()..rows {
                self.grid.push(vec![Cell::blank(); cols]);
            }
        } else {
            self.grid.truncate(rows);
        }

        // 光标 clamp。
        self.cursor.clamp(rows, cols);
        // 滚动区 clamp。
        self.scroll_region_top = self.scroll_region_top.min(rows.saturating_sub(1));
        self.scroll_region_bottom = self.scroll_region_bottom.min(rows.saturating_sub(1));
        if self.scroll_region_top > self.scroll_region_bottom {
            self.scroll_region_bottom = self.scroll_region_top;
        }

        // 如果屏幕变小，把溢出的行推入 scrollback。
        while self.grid.len() > rows && old_rows > rows {
            if let Some(row) = self.grid.pop() {
                if self.scrollback.len() >= self.max_scrollback {
                    self.scrollback.pop_front();
                }
                self.scrollback.push_back(row);
            }
        }
    }

    /// 返回指定单元格引用。
    pub fn get(&self, row: usize, col: usize) -> Option<&Cell> {
        self.grid.get(row).and_then(|r| r.get(col))
    }

    /// 返回可变单元格引用。
    pub fn get_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.grid.get_mut(row).and_then(|r| r.get_mut(col))
    }

    /// 在当前光标处写入一个字符（按当前属性）。
    pub fn put_char(&mut self, ch: char) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        if let Some(cell) = self.get_mut(row, col) {
            cell.ch = ch;
        }
        self.cursor.col += 1;
        if self.cursor.col >= self.cols {
            self.cursor.col = 0;
            self.cursor.row += 1;
        }
        if self.cursor.row >= self.rows {
            self.scroll(1);
            self.cursor.row = self.rows.saturating_sub(1);
        }
    }

    /// 写入字符串。
    pub fn put_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.put_char(ch);
        }
    }

    /// 滚动 n 行（正数向上滚，旧行进入 scrollback）。
    pub fn scroll(&mut self, n: usize) {
        for _ in 0..n {
            if self.scrollback.len() >= self.max_scrollback {
                self.scrollback.pop_front();
            }
            let mut new_row = vec![Cell::blank(); self.cols];
            // 保留当前滚动区内的属性默认。
            self.scrollback.push_back(self.grid.remove(0));
            self.grid.push(new_row);
        }
        self.cursor.row = self.cursor.row.saturating_sub(n.min(self.cursor.row));
    }

    /// 清屏：按模式 (0=全部,1=光标到结尾,2=开头到光标)。
    pub fn clear_screen(&mut self, mode: u8) {
        match mode {
            0 => {
                for row in self.grid.iter_mut() {
                    for cell in row.iter_mut() {
                        *cell = Cell::blank();
                    }
                }
            }
            1 => {
                for r in self.cursor.row..self.rows {
                    for c in self.cursor.col..self.cols {
                        if let Some(cell) = self.get_mut(r, c) {
                            *cell = Cell::blank();
                        }
                    }
                }
                for r in self.cursor.row + 1..self.rows {
                    for c in 0..self.cols {
                        if let Some(cell) = self.get_mut(r, c) {
                            *cell = Cell::blank();
                        }
                    }
                }
            }
            2 => {
                for r in 0..=self.cursor.row {
                    for c in 0..self.cols {
                        if let Some(cell) = self.get_mut(r, c) {
                            *cell = Cell::blank();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// 清行：按模式。
    pub fn clear_line(&mut self, mode: u8) {
        if self.cursor.row >= self.rows {
            return;
        }
        match mode {
            0 => {
                for c in self.cursor.col..self.cols {
                    if let Some(cell) = self.get_mut(self.cursor.row, c) {
                        *cell = Cell::blank();
                    }
                }
            }
            1 => {
                for c in 0..=self.cursor.col {
                    if let Some(cell) = self.get_mut(self.cursor.row, c) {
                        *cell = Cell::blank();
                    }
                }
            }
            2 => {
                for c in 0..self.cols {
                    if let Some(cell) = self.get_mut(self.cursor.row, c) {
                        *cell = Cell::blank();
                    }
                }
            }
            _ => {}
        }
    }

    /// 设置当前光标位置单元格的属性（用于 SGR 后继字符）。
    pub fn set_current_attrs(&mut self, fg: Color, bg: Color, attrs: CellAttrs) {
        if let Some(cell) = self.get_mut(self.cursor.row, self.cursor.col) {
            cell.fg = fg;
            cell.bg = bg;
            cell.attrs = attrs;
        }
    }

    /// 把可见屏幕渲染为字符串（带 ANSI 颜色，供调试/预览）。
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let mut cur_fg = Color::DefaultFg;
        let mut cur_bg = Color::DefaultBg;
        for r in 0..self.rows {
            for c in 0..self.cols {
                let cell = self.grid[r].get(c).unwrap();
                if cell.fg != cur_fg || cell.bg != cur_bg {
                    out.push_str(&color_ansi(cell.fg, cell.bg));
                    cur_fg = cell.fg;
                    cur_bg = cell.bg;
                }
                out.push(cell.ch);
            }
            out.push('\n');
        }
        out.push_str("\x1b[0m");
        out
    }

    /// 获取选中区域文本。
    pub fn selection_text(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (sr, sc) = start;
        let (er, ec) = end;
        let mut out = String::new();
        let r_min = sr.min(er);
        let r_max = sr.max(er).min(self.rows.saturating_sub(1));
        for r in r_min..=r_max {
            let c_start = if r == r_min { sc.min(ec) } else { 0 };
            let c_end = if r == r_max { sc.max(ec).min(self.cols) } else { self.cols };
            for c in c_start..c_end {
                if let Some(cell) = self.get(r, c) {
                    out.push(cell.ch);
                }
            }
            out.push('\n');
        }
        out
    }
}

// ============================================================================
// ANSI 解析器
// ============================================================================

/// ANSI 参数类型。
#[derive(Clone, Debug)]
enum CsiParam {
    Numeric(Option<u16>),
    String(String),
}

/// ANSI 解析器状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnsiState {
    Ground,
    Escape,
    CsiEntry,
    CsiParam,
    CsiIgnore,
    String,
}

/// ANSI 转义码解析器：把字节流转译为对 `TerminalBuffer` 的操作。
pub struct AnsiParser {
    state: AnsiState,
    params: Vec<CsiParam>,
}

impl AnsiParser {
    pub fn new() -> Self {
        Self {
            state: AnsiState::Ground,
            params: Vec::new(),
        }
    }

    /// 处理一个字节，返回产生的操作指令。
    pub fn feed(&mut self, byte: u8) -> Vec<AnsiAction> {
        let mut actions = Vec::new();
        match self.state {
            AnsiState::Ground => {
                if byte == 0x1b {
                    self.state = AnsiState::Escape;
                } else if byte == 0x7f || byte >= 0x20 {
                    actions.push(AnsiAction::PutChar(byte as char));
                }
                // 其他控制字符忽略（CR/LF 由 Terminal 处理）。
            }
            AnsiState::Escape => {
                self.state = AnsiState::Ground;
                match byte {
                    b'[' => self.state = AnsiState::CsiEntry,
                    b']' => self.state = AnsiState::String,
                    b'c' => actions.push(AnsiAction::Da1),
                    b'M' => actions.push(AnsiAction::DeleteLine),
                    b'H' => actions.push(AnsiAction::TabSet),
                    _ => {}
                }
            }
            AnsiState::CsiEntry => {
                self.state = AnsiState::CsiParam;
                self.params.clear();
                match byte {
                    b'?' | b'>' | b'=' => {
                        // 私有前缀，简化为忽略并进入参数。
                        self.state = AnsiState::CsiParam;
                    }
                    b'0'..=b'9' => {
                        self.params.push(CsiParam::Numeric(Some((byte - b'0') as u16)));
                        self.state = AnsiState::CsiParam;
                    }
                    b';' => self.params.push(CsiParam::Numeric(None)),
                    _ => {
                        self.state = AnsiState::Ground;
                        self.dispatch_csi(byte, &mut actions);
                    }
                }
            }
            AnsiState::CsiParam => {
                match byte {
                    b'0'..=b'9' => {
                        if let Some(last) = self.params.last_mut() {
                            match last {
                                CsiParam::Numeric(Some(v)) => *v = *v * 10 + (byte - b'0') as u16,
                                _ => {}
                            }
                        }
                    }
                    b';' => self.params.push(CsiParam::Numeric(None)),
                    b'@' | b'A'..=b'Z' | b'a'..=b'z' => {
                        self.state = AnsiState::Ground;
                        self.dispatch_csi(byte, &mut actions);
                    }
                    _ => {
                        self.state = AnsiState::CsiIgnore;
                    }
                }
            }
            AnsiState::CsiIgnore => {
                if byte >= 0x30 && byte <= 0x7e {
                    self.state = AnsiState::Ground;
                    self.dispatch_csi(byte, &mut actions);
                }
            }
            AnsiState::String => {
                if byte == 0x1b || byte == 0x7f || byte == 0x9c {
                    self.state = AnsiState::Ground;
                }
            }
        }
        actions
    }

    fn numeric_param(&self, idx: usize, default: u16) -> u16 {
        self.params.get(idx).and_then(|p| match p {
            CsiParam::Numeric(Some(v)) => Some(*v),
            _ => None,
        }).unwrap_or(default)
    }

    fn dispatch_csi(&self, final_byte: u8, actions: &mut Vec<AnsiAction>) {
        match final_byte {
            // 光标移动
            b'A' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::CursorUp(n));
            }
            b'B' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::CursorDown(n));
            }
            b'C' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::CursorForward(n));
            }
            b'D' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::CursorBackward(n));
            }
            b'H' | b'f' => {
                let r = self.numeric_param(0, 1).saturating_sub(1) as usize;
                let c = self.numeric_param(1, 1).saturating_sub(1) as usize;
                actions.push(AnsiAction::CursorSet(r, c));
            }
            b'J' => {
                let n = self.numeric_param(0, 0);
                actions.push(AnsiAction::EraseScreen(n as u8));
            }
            b'K' => {
                let n = self.numeric_param(0, 0);
                actions.push(AnsiAction::EraseLine(n as u8));
            }
            b'm' => {
                let mut sgr = Vec::new();
                for p in &self.params {
                    match p {
                        CsiParam::Numeric(Some(v)) => sgr.push(*v),
                        CsiParam::Numeric(None) => sgr.push(0),
                        CsiParam::String(_) => {}
                    }
                }
                if sgr.is_empty() {
                    sgr.push(0);
                }
                actions.push(AnsiAction::Sgr(sgr));
            }
            b'h' => {
                // 私有模式设置，简化。
                actions.push(AnsiAction::SetMode(self.params.clone()));
            }
            b'l' => {
                actions.push(AnsiAction::ResetMode(self.params.clone()));
            }
            b'c' => {
                actions.push(AnsiAction::Da1);
            }
            b's' => actions.push(AnsiAction::SaveCursor),
            b'u' => actions.push(AnsiAction::RestoreCursor),
            b'G' => {
                let c = self.numeric_param(0, 1).saturating_sub(1) as usize;
                actions.push(AnsiAction::CursorSetColumn(c));
            }
            b'L' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::InsertLine(n));
            }
            b'M' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::DeleteLineN(n));
            }
            b'P' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::DeleteChar(n));
            }
            b'@' => {
                let n = self.numeric_param(0, 1).max(1) as usize;
                actions.push(AnsiAction::InsertChar(n));
            }
            _ => {}
        }
    }
}

/// ANSI 解析产生的动作指令。
#[derive(Clone, Debug)]
pub enum AnsiAction {
    PutChar(char),
    CursorUp(usize),
    CursorDown(usize),
    CursorForward(usize),
    CursorBackward(usize),
    CursorSet(usize, usize),
    CursorSetColumn(usize),
    EraseScreen(u8),
    EraseLine(u8),
    Sgr(Vec<u16>),
    SetMode(Vec<CsiParam>),
    ResetMode(Vec<CsiParam>),
    Da1,
    SaveCursor,
    RestoreCursor,
    InsertLine(usize),
    DeleteLine,
    DeleteLineN(usize),
    DeleteChar(usize),
    InsertChar(usize),
    TabSet,
}

// ============================================================================
// SGR 图形渲染
// ============================================================================

/// 当前 SGR 状态。
#[derive(Clone, Debug)]
pub struct SgrState {
    pub fg: Color,
    pub bg: Color,
    pub attrs: CellAttrs,
}

impl Default for SgrState {
    fn default() -> Self {
        // 默认前景/背景各自对应，不能简单地都取 `Color::default()`。
        Self {
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            attrs: CellAttrs::default(),
        }
    }
}

impl SgrState {
    pub fn apply(&mut self, code: u16) {
        match code {
            0 => {
                self.fg = Color::DefaultFg;
                self.bg = Color::DefaultBg;
                self.attrs = CellAttrs::default();
            }
            1 => self.attrs.bold = true,
            3 => self.attrs.italic = true,
            4 => self.attrs.underline = true,
            7 => self.attrs.reverse = true,
            22 => {
                self.attrs.bold = false;
            }
            24 => self.attrs.underline = false,
            27 => self.attrs.reverse = false,
            30..=37 => self.fg = Color::Index((code - 30) as u8),
            38 => self.fg = Color::Palette256(0), // 占位，实际需读取后续参数
            39 => self.fg = Color::DefaultFg,
            40..=47 => self.bg = Color::Index((code - 40) as u8),
            48 => self.bg = Color::Palette256(0),
            49 => self.bg = Color::DefaultBg,
            90..=97 => self.fg = Color::Index((code - 90 + 8) as u8),
            100..=107 => self.bg = Color::Index((code - 100 + 8) as u8),
            _ => {}
        }
    }
}

/// 将 Color 转为 ANSI 前景/背景转义码。
fn color_ansi(fg: Color, bg: Color) -> String {
    let mut s = String::new();
    match fg {
        Color::DefaultFg => s.push_str("\x1b[39m"),
        Color::DefaultBg => s.push_str("\x1b[39m"), // 语义上 fg 不应为背景色，但保持穷尽性
        Color::Index(i) => {
            if i < 8 {
                s.push_str(&format!("\x1b[{}m", 30 + i));
            } else {
                s.push_str(&format!("\x1b[{}m", 82 + i)); // 近似
            }
        }
        Color::Palette256(i) => s.push_str(&format!("\x1b[38;5;{}m", i)),
        Color::Rgb(r, g, b) => s.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b)),
    }
    match bg {
        Color::DefaultBg => s.push_str("\x1b[49m"),
        Color::DefaultFg => s.push_str("\x1b[49m"), // 语义上 bg 不应为前景色，但保持穷尽性
        Color::Index(i) => {
            if i < 8 {
                s.push_str(&format!("\x1b[{}m", 40 + i));
            } else {
                s.push_str(&format!("\x1b[{}m", 92 + i));
            }
        }
        Color::Palette256(i) => s.push_str(&format!("\x1b[48;5;{}m", i)),
        Color::Rgb(r, g, b) => s.push_str(&format!("\x1b[48;2;{};{};{}m", r, g, b)),
    }
    s
}

// ============================================================================
// 终端
// ============================================================================

/// 终端仿真器。
pub struct Terminal {
    pub buffer: TerminalBuffer,
    pub parser: AnsiParser,
    pub sgr: SgrState,
    pub saved_cursor: Option<Cursor>,
    /// 剪贴板文本。
    pub clipboard: String,
    /// 是否启用鼠标报告。
    pub mouse_reporting: bool,
    /// 是否启用备用屏幕。
    pub alt_screen: bool,
    /// 备用屏幕缓冲。
    pub alt_buffer: Option<TerminalBuffer>,
}

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            buffer: TerminalBuffer::new(rows, cols),
            parser: AnsiParser::new(),
            sgr: SgrState::default(),
            saved_cursor: None,
            clipboard: String::new(),
            mouse_reporting: false,
            alt_screen: false,
            alt_buffer: None,
        }
    }

    /// 返回行数。
    pub fn rows(&self) -> usize {
        self.buffer.rows
    }

    /// 返回列数。
    pub fn cols(&self) -> usize {
        self.buffer.cols
    }

    /// 写入原始字节（解析 ANSI）。
    pub fn write(&mut self, data: &[u8]) {
        for &byte in data {
            match byte {
                b'\r' => {
                    self.buffer.cursor.col = 0;
                }
                b'\n' => {
                    self.buffer.cursor.row += 1;
                    if self.buffer.cursor.row >= self.buffer.rows {
                        self.buffer.scroll(1);
                        self.buffer.cursor.row = self.buffer.rows.saturating_sub(1);
                    }
                }
                b'\t' => {
                    self.buffer.cursor.col = (self.buffer.cursor.col / 8 + 1) * 8;
                    if self.buffer.cursor.col >= self.buffer.cols {
                        self.buffer.cursor.col = 0;
                        self.buffer.cursor.row += 1;
                        if self.buffer.cursor.row >= self.buffer.rows {
                            self.buffer.scroll(1);
                            self.buffer.cursor.row = self.buffer.rows.saturating_sub(1);
                        }
                    }
                }
                b'\x08' => {
                    // 退格。
                    if self.buffer.cursor.col > 0 {
                        self.buffer.cursor.col -= 1;
                    }
                }
                b'\x07' => {}
                _ => {
                    let actions = self.parser.feed(byte);
                    for action in actions {
                        self.apply_action(action);
                    }
                }
            }
        }
    }

    /// 写入字符串。
    pub fn write_str(&mut self, s: &str) {
        self.write(s.as_bytes());
    }

    /// 应用一个 ANSI 动作。
    fn apply_action(&mut self, action: AnsiAction) {
        match action {
            AnsiAction::PutChar(ch) => {
                let cell = Cell {
                    ch,
                    fg: self.sgr.fg,
                    bg: self.sgr.bg,
                    attrs: self.sgr.attrs,
                };
                if let Some(cur) = self.buffer.get_mut(self.buffer.cursor.row, self.buffer.cursor.col) {
                    *cur = cell;
                }
                self.buffer.cursor.col += 1;
                if self.buffer.cursor.col >= self.buffer.cols {
                    self.buffer.cursor.col = 0;
                    self.buffer.cursor.row += 1;
                    if self.buffer.cursor.row >= self.buffer.rows {
                        self.buffer.scroll(1);
                        self.buffer.cursor.row = self.buffer.rows.saturating_sub(1);
                    }
                }
            }
            AnsiAction::CursorUp(n) => {
                self.buffer.cursor.row = self.buffer.cursor.row.saturating_sub(n);
            }
            AnsiAction::CursorDown(n) => {
                self.buffer.cursor.row = (self.buffer.cursor.row + n).min(self.buffer.rows.saturating_sub(1));
            }
            AnsiAction::CursorForward(n) => {
                self.buffer.cursor.col = (self.buffer.cursor.col + n).min(self.buffer.cols.saturating_sub(1));
            }
            AnsiAction::CursorBackward(n) => {
                self.buffer.cursor.col = self.buffer.cursor.col.saturating_sub(n);
            }
            AnsiAction::CursorSet(r, c) => {
                self.buffer.cursor.row = r.min(self.buffer.rows.saturating_sub(1));
                self.buffer.cursor.col = c.min(self.buffer.cols.saturating_sub(1));
            }
            AnsiAction::CursorSetColumn(c) => {
                self.buffer.cursor.col = c.min(self.buffer.cols.saturating_sub(1));
            }
            AnsiAction::EraseScreen(mode) => self.buffer.clear_screen(mode),
            AnsiAction::EraseLine(mode) => self.buffer.clear_line(mode),
            AnsiAction::Sgr(codes) => {
                for code in codes {
                    self.sgr.apply(code);
                }
            }
            AnsiAction::SaveCursor => {
                self.saved_cursor = Some(self.buffer.cursor.clone());
            }
            AnsiAction::RestoreCursor => {
                if let Some(cur) = self.saved_cursor.take() {
                    self.buffer.cursor = cur;
                }
            }
            AnsiAction::DeleteLine => {
                let row = self.buffer.cursor.row;
                if row < self.buffer.rows {
                    self.buffer.grid.remove(row);
                    self.buffer.grid.push(vec![Cell::blank(); self.buffer.cols]);
                }
            }
            AnsiAction::DeleteLineN(n) => {
                for _ in 0..n {
                    let row = self.buffer.cursor.row;
                    if row < self.buffer.rows {
                        self.buffer.grid.remove(row);
                        self.buffer.grid.push(vec![Cell::blank(); self.buffer.cols]);
                    }
                }
            }
            AnsiAction::InsertLine(n) => {
                for _ in 0..n {
                    let row = self.buffer.cursor.row;
                    if row < self.buffer.rows {
                        self.buffer.grid.insert(row, vec![Cell::blank(); self.buffer.cols]);
                        self.buffer.grid.pop();
                    }
                }
            }
            AnsiAction::DeleteChar(n) => {
                let row = self.buffer.cursor.row;
                let col = self.buffer.cursor.col;
                if row < self.buffer.rows {
                    if let Some(r) = self.buffer.grid.get_mut(row) {
                        for _ in 0..n {
                            if col < r.len() {
                                r.remove(col);
                            }
                            r.push(Cell::blank());
                        }
                    }
                }
            }
            AnsiAction::InsertChar(n) => {
                let row = self.buffer.cursor.row;
                let col = self.buffer.cursor.col;
                if row < self.buffer.rows {
                    if let Some(r) = self.buffer.grid.get_mut(row) {
                        for _ in 0..n {
                            if col < r.len() {
                                r.insert(col, Cell::blank());
                            }
                            r.pop();
                        }
                    }
                }
            }
            AnsiAction::SetMode(_) | AnsiAction::ResetMode(_) | AnsiAction::TabSet | AnsiAction::Da1 => {}
        }
    }

    /// 重设大小。
    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.buffer.resize(rows, cols);
        if let Some(alt) = self.alt_buffer.as_mut() {
            alt.resize(rows, cols);
        }
    }

    /// 滚动 n 行。
    pub fn scroll(&mut self, n: usize) {
        self.buffer.scroll(n);
    }

    /// 选中区域。
    pub fn select(&mut self, start: (usize, usize), end: (usize, usize)) {
        let _ = start;
        let _ = end;
    }

    /// 把选中区域复制到剪贴板。
    pub fn copy(&mut self, start: (usize, usize), end: (usize, usize)) {
        self.clipboard = self.buffer.selection_text(start, end);
    }

    /// 从剪贴板粘贴（返回要写入的文本）。
    pub fn paste(&self) -> &str {
        &self.clipboard
    }

    /// 清屏。
    pub fn clear(&mut self) {
        self.buffer.clear_screen(0);
        self.buffer.cursor.row = 0;
        self.buffer.cursor.col = 0;
    }

    /// 重置终端状态。
    pub fn reset(&mut self) {
        self.buffer = TerminalBuffer::new(self.buffer.rows, self.buffer.cols);
        self.parser = AnsiParser::new();
        self.sgr = SgrState::default();
        self.saved_cursor = None;
        self.clipboard.clear();
    }

    /// 返回可见屏幕文本。
    pub fn render(&self) -> String {
        self.buffer.render_text()
    }

    /// 返回光标位置。
    pub fn cursor_pos(&self) -> (usize, usize) {
        (self.buffer.cursor.row, self.buffer.cursor.col)
    }

    /// 设置光标可见性。
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.buffer.cursor.visible = visible;
    }

    /// 设置光标样式。
    pub fn set_cursor_style(&mut self, style: CursorStyle) {
        self.buffer.cursor.style = style;
    }

    /// 返回剪贴板内容。
    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }
}

// ============================================================================
// 全局 Terminal 单例（供 kernel/sense.rs 调用）
// ============================================================================

static TERMINAL: OnceLock<Mutex<Terminal>> = OnceLock::new();

/// 获取全局终端。
pub fn terminal() -> &'static Mutex<Terminal> {
    TERMINAL.get_or_init(|| Mutex::new(Terminal::new(24, 80)))
}

/// `sense gui://terminal`：返回终端状态摘要。
pub fn terminal_sense() -> String {
    let t = terminal().lock().unwrap();
    let (r, c) = t.cursor_pos();
    format!(
        "rows={} cols={} cursor=({},{}) visible={} clipboard={}B alt_screen={}",
        t.rows(),
        t.cols(),
        r,
        c,
        t.buffer.cursor.visible,
        t.clipboard.len(),
        t.alt_screen
    )
}

/// `act gui://terminal <op> ...`：对终端执行行动。
pub fn terminal_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let t = terminal();
    let mut t = t.lock().unwrap();
    match it.next().unwrap_or("") {
        "write" => {
            let text = it.next().ok_or("缺少文本")?;
            t.write_str(text);
            Ok(format!("已写入 {} 字节", text.len()))
        }
        "resize" => {
            let rows: usize = it
                .next()
                .ok_or("缺少 rows")?
                .parse::<usize>()
                .map_err(|e| e.to_string())?;
            let cols: usize = it.next().ok_or("缺少 cols")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            t.resize(rows, cols);
            Ok(format!("终端已调整为 {}x{}", rows, cols))
        }
        "scroll" => {
            let n: usize = it.next().ok_or("缺少 n")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            t.scroll(n);
            Ok(format!("已滚动 {} 行", n))
        }
        "select" => {
            let r1: usize = it.next().ok_or("缺少 r1")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let c1: usize = it.next().ok_or("缺少 c1")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let r2: usize = it.next().ok_or("缺少 r2")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let c2: usize = it.next().ok_or("缺少 c2")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            t.copy((r1, c1), (r2, c2));
            Ok(format!("已选中并复制 ({},{})->({},{})", r1, c1, r2, c2))
        }
        "copy" => {
            let text = it.next().unwrap_or("");
            t.clipboard = text.to_string();
            Ok(format!("剪贴板已写入 {} 字节", text.len()))
        }
        "paste" => Ok(format!("剪贴板: {}", t.clipboard())),
        "clear" => {
            t.clear();
            Ok("终端已清屏".to_string())
        }
        "reset" => {
            t.reset();
            Ok("终端已重置".to_string())
        }
        "cursor_visible" => {
            let v = it.next().unwrap_or("1") == "1";
            t.set_cursor_visible(v);
            Ok(format!("光标可见={}", v))
        }
        "cursor_style" => {
            let style = match it.next().unwrap_or("block") {
                "underline" => CursorStyle::Underline,
                "bar" => CursorStyle::Bar,
                _ => CursorStyle::Block,
            };
            t.set_cursor_style(style);
            Ok(format!("光标样式={:?}", style))
        }
        "render" => Ok(t.render()),
        "list" => Ok(terminal_sense()),
        _ => Err(
            "用法: terminal write|resize|scroll|select|copy|paste|clear|reset|cursor_visible|cursor_style|render|list"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_blank() {
        let c = Cell::blank();
        assert_eq!(c.ch, ' ');
        assert!(!c.attrs.bold);
    }

    #[test]
    fn buffer_put_and_scroll() {
        let mut buf = TerminalBuffer::new(3, 5);
        buf.put_str("hello");
        buf.put_char('\n');
        buf.put_str("world");
        assert_eq!(buf.cursor.row, 1);
        assert_eq!(buf.cursor.col, 5);
        // 滚动。
        buf.scroll(1);
        assert_eq!(buf.scrollback.len(), 1);
    }

    #[test]
    fn buffer_clear_screen() {
        let mut buf = TerminalBuffer::new(3, 5);
        buf.put_str("abcde\nfghij\nklmno");
        buf.clear_screen(0);
        for row in buf.grid.iter() {
            for cell in row.iter() {
                assert_eq!(cell.ch, ' ');
            }
        }
    }

    #[test]
    fn buffer_resize() {
        let mut buf = TerminalBuffer::new(5, 5);
        buf.put_str("hello\n");
        buf.resize(3, 3);
        assert_eq!(buf.rows, 3);
        assert_eq!(buf.cols, 3);
    }

    #[test]
    fn ansi_cursor_movement() {
        let mut t = Terminal::new(10, 20);
        t.write_str("\x1b[5;10H");
        let (r, c) = t.cursor_pos();
        assert_eq!(r, 5);
        assert_eq!(c, 10);
        t.write_str("\x1b[3A");
        let (r, _) = t.cursor_pos();
        assert_eq!(r, 2);
    }

    #[test]
    fn ansi_sgr_colors() {
        let mut t = Terminal::new(5, 10);
        t.write_str("\x1b[31m\x1b[42mX");
        // 红前景绿背景。
        assert!(matches!(t.sgr.fg, Color::Index(1)));
        assert!(matches!(t.sgr.bg, Color::Index(2)));
    }

    #[test]
    fn ansi_reset() {
        let mut t = Terminal::new(5, 10);
        t.write_str("\x1b[1;31m");
        assert!(t.sgr.attrs.bold);
        t.write_str("\x1b[0m");
        assert!(!t.sgr.attrs.bold);
    }

    #[test]
    fn ansi_erase_screen() {
        let mut t = Terminal::new(3, 5);
        t.write_str("abcde\nfghij\n");
        t.write_str("\x1b[2J");
        // 全部空白。
        for row in t.buffer.grid.iter() {
            for cell in row.iter() {
                assert_eq!(cell.ch, ' ');
            }
        }
    }

    #[test]
    fn terminal_write_and_render() {
        let mut t = Terminal::new(5, 20);
        t.write_str("hello, terminal\r\n");
        let out = t.render();
        assert!(out.contains("hello"));
    }

    #[test]
    fn terminal_copy_paste() {
        let mut t = Terminal::new(5, 20);
        t.write_str("line1\nline2\n");
        t.copy((0, 0), (0, 5));
        assert_eq!(t.clipboard(), "line1\n");
        t.clipboard = "paste me".to_string();
        assert_eq!(t.paste(), "paste me");
    }

    #[test]
    fn terminal_clear_reset() {
        let mut t = Terminal::new(5, 20);
        t.write_str("some text\r\n");
        t.clear();
        t.reset();
        assert_eq!(t.cursor_pos(), (0, 0));
    }

    #[test]
    fn cursor_style_toggle() {
        let mut t = Terminal::new(5, 20);
        t.set_cursor_style(CursorStyle::Underline);
        assert_eq!(t.buffer.cursor.style, CursorStyle::Underline);
        t.set_cursor_visible(false);
        assert!(!t.buffer.cursor.visible);
    }

    #[test]
    fn terminal_sense_and_act() {
        let mut t = terminal().lock().unwrap();
        t.write_str("hello\r\n");
        drop(t);
        let sense = terminal_sense();
        assert!(sense.contains("rows="));
        let res = terminal_act("resize 30 100").unwrap();
        assert!(res.contains("30"));
        let res = terminal_act("clear").unwrap();
        assert!(res.contains("清屏"));
        let res = terminal_act("list").unwrap();
        assert!(sense.contains("cols="));
    }

    #[test]
    fn color_name() {
        assert_eq!(Color::Index(1).name(), "红");
        assert_eq!(Color::Rgb(255, 0, 0).name(), "真彩色");
    }

    #[test]
    fn color_ansi_output() {
        let s = color_ansi(Color::Index(1), Color::Index(2));
        assert!(s.contains("\x1b[31m"));
        assert!(s.contains("\x1b[42m"));
    }
}
