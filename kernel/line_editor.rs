//! 行编辑器 —— 真 REPL 的前端。
//!
//! * 在 Windows 控制台下启用 VT 输入模式，支持 `↑`/`↓` 历史、`Tab` 补全、
//!   `←`/`→` 光标移动、`Backspace`/`Delete` 编辑、`Ctrl+C` 取消、`Ctrl+D` 退出。
//! * 非控制台 (管道 / 重定向) 下自动降级为逐行读取，保证脚本化可用。

use std::io::{self, BufRead, Read, Write};

/// 历史记录上限。
pub const MAX_HISTORY: usize = 200;

/// 补全结果：候选列表 + 可选参数提示。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    pub items: Vec<String>,
    /// `--help` 式参数提示，Tab 无候选时展示。
    pub hint: Option<String>,
}

impl Completion {
    pub fn new(items: Vec<String>) -> Self {
        Self { items, hint: None }
    }

    pub fn with_hint(items: Vec<String>, hint: impl Into<String>) -> Self {
        Self {
            items,
            hint: Some(hint.into()),
        }
    }

    pub fn hint_only(hint: impl Into<String>) -> Self {
        Self {
            items: Vec::new(),
            hint: Some(hint.into()),
        }
    }
}

/// 读取结果。
pub enum ReadOutcome {
    Line(String),
    /// Ctrl+C：本行作废，但继续 REPL。
    Cancelled,
    /// EOF / Ctrl+D：退出。
    Eof,
}

/// 行编辑器。
pub struct LineEditor {
    history: Vec<String>,
}

impl LineEditor {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// 追加历史（跳过空行与连续重复）。
    pub fn add_history(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.history.last().map(|s| s.as_str()) == Some(line) {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > MAX_HISTORY {
            let overflow = self.history.len() - MAX_HISTORY;
            self.history.drain(0..overflow);
        }
    }

    /// 读取一行。`completer` 接收当前缓冲区，返回候选补全项。
    pub fn read_line<F>(&mut self, prompt: &str, completer: F) -> io::Result<ReadOutcome>
    where
        F: Fn(&str) -> Completion,
    {
        #[cfg(windows)]
        {
            if let Some(guard) = console::RawMode::enable() {
                let out = self.read_line_console(prompt, &completer);
                drop(guard);
                return out;
            }
        }
        let _ = prompt;
        let _ = completer;
        self.read_line_plain()
    }

    fn read_line_plain(&mut self) -> io::Result<ReadOutcome> {
        let stdin = io::stdin();
        let mut buf = String::new();
        let n = stdin.lock().read_line(&mut buf)?;
        if n == 0 {
            return Ok(ReadOutcome::Eof);
        }
        // 去掉行尾换行与可能存在的 UTF-8 BOM（管道/重定向输入常带 BOM）。
        let line = buf
            .trim_end_matches(['\r', '\n'])
            .trim_start_matches('\u{feff}')
            .to_string();
        self.add_history(&line);
        Ok(ReadOutcome::Line(line))
    }

    /// 控制台原始模式读取。
    #[cfg(windows)]
    fn read_line_console<F>(&mut self, prompt: &str, completer: &F) -> io::Result<ReadOutcome>
    where
        F: Fn(&str) -> Completion,
    {
        let mut buf: Vec<char> = Vec::new();
        let mut cursor = 0usize;
        let mut hist_idx = self.history.len();
        let mut draft: Vec<char> = Vec::new();

        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut out = io::stdout();

        render(&mut out, prompt, &buf, cursor)?;

        loop {
            let Some(first) = read_byte(&mut input)? else {
                return Ok(ReadOutcome::Eof);
            };

            match first {
                // Enter
                b'\r' | b'\n' => {
                    writeln!(out)?;
                    out.flush()?;
                    let line: String = buf.iter().collect();
                    self.add_history(&line);
                    return Ok(ReadOutcome::Line(line));
                }
                // Ctrl+C
                0x03 => {
                    write!(out, "^C\r\n")?;
                    out.flush()?;
                    return Ok(ReadOutcome::Cancelled);
                }
                // Ctrl+D
                0x04 => {
                    if buf.is_empty() {
                        writeln!(out)?;
                        out.flush()?;
                        return Ok(ReadOutcome::Eof);
                    }
                    if cursor < buf.len() {
                        buf.remove(cursor);
                        render(&mut out, prompt, &buf, cursor)?;
                    }
                }
                // Backspace (0x7f / 0x08)
                0x7f | 0x08 => {
                    if cursor > 0 {
                        cursor -= 1;
                        buf.remove(cursor);
                        render(&mut out, prompt, &buf, cursor)?;
                    }
                }
                // Tab
                b'\t' => {
                    let current: String = buf.iter().collect();
                    let completion = completer(&current);
                    let candidates = completion.items;
                    if candidates.len() == 1 {
                        apply_completion(&mut buf, &mut cursor, &candidates[0]);
                        render(&mut out, prompt, &buf, cursor)?;
                    } else if candidates.len() > 1 {
                        let prefix = longest_common_prefix(&candidates);
                        let token = last_token(&current);
                        if prefix.chars().count() > token.chars().count() {
                            apply_completion(&mut buf, &mut cursor, &prefix);
                        }
                        write!(out, "\r\n")?;
                        print_candidates(&mut out, &candidates, &token)?;
                        if let Some(hint) = &completion.hint {
                            write!(out, "  \x1b[2;36m{}\x1b[0m\r\n", hint)?;
                        }
                        render(&mut out, prompt, &buf, cursor)?;
                    } else if let Some(hint) = &completion.hint {
                        // 无候选但有参数提示：展示 --help 式说明
                        write!(out, "\r\n  \x1b[2;36m{}\x1b[0m\r\n", hint)?;
                        render(&mut out, prompt, &buf, cursor)?;
                    }
                }
                // ESC 序列
                0x1b => {
                    match read_byte(&mut input)? {
                        Some(b'[') => match read_byte(&mut input)? {
                            // ↑
                            Some(b'A') => {
                                if hist_idx > 0 {
                                    if hist_idx == self.history.len() {
                                        draft = buf.clone();
                                    }
                                    hist_idx -= 1;
                                    buf = self.history[hist_idx].chars().collect();
                                    cursor = buf.len();
                                    render(&mut out, prompt, &buf, cursor)?;
                                }
                            }
                            // ↓
                            Some(b'B') => {
                                if hist_idx < self.history.len() {
                                    hist_idx += 1;
                                    buf = if hist_idx == self.history.len() {
                                        draft.clone()
                                    } else {
                                        self.history[hist_idx].chars().collect()
                                    };
                                    cursor = buf.len();
                                    render(&mut out, prompt, &buf, cursor)?;
                                }
                            }
                            // →
                            Some(b'C') => {
                                if cursor < buf.len() {
                                    cursor += 1;
                                    render(&mut out, prompt, &buf, cursor)?;
                                }
                            }
                            // ←
                            Some(b'D') => {
                                if cursor > 0 {
                                    cursor -= 1;
                                    render(&mut out, prompt, &buf, cursor)?;
                                }
                            }
                            // Home / End
                            Some(b'H') => {
                                cursor = 0;
                                render(&mut out, prompt, &buf, cursor)?;
                            }
                            Some(b'F') => {
                                cursor = buf.len();
                                render(&mut out, prompt, &buf, cursor)?;
                            }
                            // Delete: ESC [ 3 ~
                            Some(b'3') => {
                                let _ = read_byte(&mut input)?;
                                if cursor < buf.len() {
                                    buf.remove(cursor);
                                    render(&mut out, prompt, &buf, cursor)?;
                                }
                            }
                            _ => {}
                        },
                        // Alt 组合等，忽略
                        _ => {}
                    }
                }
                // 普通字符 (含 UTF-8 多字节)
                0x20..=0x7e => {
                    let ch = first as char;
                    buf.insert(cursor, ch);
                    cursor += 1;
                    render(&mut out, prompt, &buf, cursor)?;
                }
                _ => {
                    if let Some(ch) = read_utf8_char(&mut input, first)? {
                        buf.insert(cursor, ch);
                        cursor += 1;
                        render(&mut out, prompt, &buf, cursor)?;
                    }
                }
            }
        }
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

fn read_byte<R: Read>(input: &mut R) -> io::Result<Option<u8>> {
    let mut b = [0u8; 1];
    match input.read(&mut b)? {
        0 => Ok(None),
        _ => Ok(Some(b[0])),
    }
}

fn read_utf8_char<R: Read>(input: &mut R, first: u8) -> io::Result<Option<char>> {
    let need = if first >> 5 == 0b110 {
        1
    } else if first >> 4 == 0b1110 {
        2
    } else if first >> 3 == 0b11110 {
        3
    } else {
        return Ok(None);
    };
    let mut bytes = vec![first];
    for _ in 0..need {
        match read_byte(input)? {
            Some(b) => bytes.push(b),
            None => break,
        }
    }
    Ok(String::from_utf8_lossy(&bytes).chars().next())
}

/// 重绘：回到行首、清除到行尾、输出提示符与缓冲、再把光标左移回去。
fn render(out: &mut io::Stdout, prompt: &str, buf: &[char], cursor: usize) -> io::Result<()> {
    let text: String = buf.iter().collect();
    write!(out, "\r\x1b[K{}{}", prompt, text)?;
    let back = buf.len().saturating_sub(cursor);
    if back > 0 {
        write!(out, "\x1b[{}D", back)?;
    }
    out.flush()
}

/// 高亮匹配前缀并按 3 列打印候选。
pub fn print_candidates(
    out: &mut io::Stdout,
    candidates: &[String],
    match_prefix: &str,
) -> io::Result<()> {
    const COL_W: usize = 24;
    const PER_ROW: usize = 3;
    for (i, cand) in candidates.iter().enumerate() {
        if i % PER_ROW == 0 {
            if i > 0 {
                write!(out, "\r\n")?;
            }
            write!(out, "  ")?;
        }
        let (head, tail) = split_prefix(cand, match_prefix);
        write!(out, "\x1b[1;36m{}\x1b[0m{}", head, tail)?;
        let is_last = i + 1 == candidates.len();
        if i % PER_ROW != PER_ROW - 1 && !is_last {
            let pad = COL_W.saturating_sub(cand.chars().count());
            write!(out, "{:width$}", "", width = pad)?;
        }
    }
    write!(out, "\r\n")
}

/// 按字符数切出匹配前缀与剩余部分。
pub fn split_prefix<'a>(candidate: &'a str, prefix: &str) -> (&'a str, &'a str) {
    if candidate.starts_with(prefix) {
        let n = prefix.len();
        (&candidate[..n], &candidate[n..])
    } else {
        ("", candidate)
    }
}

/// 取最后一个空白分隔的 token。
pub fn last_token(input: &str) -> String {    input
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_string()
}

/// 用 `replacement` 替换缓冲区中最后一个 token。
pub fn apply_completion(buf: &mut Vec<char>, cursor: &mut usize, replacement: &str) {
    let text: String = buf.iter().collect();
    let start_chars = match text.rfind(|c: char| c.is_whitespace()) {
        Some(byte_idx) => text[..=byte_idx].chars().count(),
        None => 0,
    };
    let repl: Vec<char> = replacement.chars().collect();
    buf.splice(start_chars..*cursor, repl.iter().cloned());
    *cursor = start_chars + repl.len();
}

/// 多个候选的最长公共前缀。
pub fn longest_common_prefix(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut prefix: Vec<char> = items[0].chars().collect();
    for item in &items[1..] {
        let chars: Vec<char> = item.chars().collect();
        let mut i = 0;
        while i < prefix.len() && i < chars.len() && prefix[i] == chars[i] {
            i += 1;
        }
        prefix.truncate(i);
        if prefix.is_empty() {
            break;
        }
    }
    prefix.into_iter().collect()
}

#[cfg(windows)]
mod console {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> Handle;
        fn GetConsoleMode(h: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(h: Handle, mode: u32) -> i32;
        fn GetConsoleCP() -> u32;
        fn SetConsoleCP(cp: u32) -> i32;
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(cp: u32) -> i32;
    }

    const STDIN: u32 = -10i32 as u32;
    const STDOUT: u32 = -11i32 as u32;
    const CP_UTF8: u32 = 65001;

    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

    /// 原始模式守卫：Drop 时恢复原控制台模式与代码页。
    pub struct RawMode {
        in_mode: u32,
        out_mode: u32,
        in_cp: u32,
        out_cp: u32,
    }

    impl RawMode {
        pub fn enable() -> Option<Self> {
            unsafe {
                let hin = GetStdHandle(STDIN);
                let hout = GetStdHandle(STDOUT);
                let mut in_mode = 0u32;
                let mut out_mode = 0u32;
                if GetConsoleMode(hin, &mut in_mode) == 0 {
                    return None;
                }
                if GetConsoleMode(hout, &mut out_mode) == 0 {
                    return None;
                }
                // 统一为 UTF-8，保证中文输入/输出不乱码。
                let in_cp = GetConsoleCP();
                let out_cp = GetConsoleOutputCP();
                let _ = SetConsoleCP(CP_UTF8);
                let _ = SetConsoleOutputCP(CP_UTF8);

                let new_in = (in_mode
                    & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT;
                if SetConsoleMode(hin, new_in) == 0 {
                    let _ = SetConsoleCP(in_cp);
                    let _ = SetConsoleOutputCP(out_cp);
                    return None;
                }
                let _ = SetConsoleMode(hout, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                Some(RawMode {
                    in_mode,
                    out_mode,
                    in_cp,
                    out_cp,
                })
            }
        }
    }

    impl Drop for RawMode {
        fn drop(&mut self) {
            unsafe {
                let _ = SetConsoleMode(GetStdHandle(STDIN), self.in_mode);
                let _ = SetConsoleMode(GetStdHandle(STDOUT), self.out_mode);
                let _ = SetConsoleCP(self.in_cp);
                let _ = SetConsoleOutputCP(self.out_cp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_dedup_and_cap() {
        let mut ed = LineEditor::new();
        ed.add_history("ls");
        ed.add_history("ls");
        ed.add_history("   ");
        ed.add_history("pwd");
        assert_eq!(ed.history(), &["ls".to_string(), "pwd".to_string()]);

        for i in 0..MAX_HISTORY + 20 {
            ed.add_history(&format!("cmd{}", i));
        }
        assert_eq!(ed.history().len(), MAX_HISTORY);
        assert_eq!(ed.history().last().unwrap(), &format!("cmd{}", MAX_HISTORY + 19));
    }

    #[test]
    fn common_prefix_works() {
        let items = vec![
            "help".to_string(),
            "hello".to_string(),
            "helium".to_string(),
        ];
        assert_eq!(longest_common_prefix(&items), "hel");
        assert_eq!(longest_common_prefix(&["abc".to_string()]), "abc");
        assert_eq!(longest_common_prefix(&[]), "");
        assert_eq!(
            longest_common_prefix(&["a".to_string(), "b".to_string()]),
            ""
        );
    }

    #[test]
    fn last_token_extracts() {
        assert_eq!(last_token("ls -l /et"), "/et");
        assert_eq!(last_token("help"), "help");
        assert_eq!(last_token(""), "");
    }

    #[test]
    fn completion_replaces_token() {
        let mut buf: Vec<char> = "cat /et".chars().collect();
        let mut cursor = buf.len();
        apply_completion(&mut buf, &mut cursor, "/etc/");
        let s: String = buf.iter().collect();
        assert_eq!(s, "cat /etc/");
        assert_eq!(cursor, s.chars().count());
    }

    #[test]
    fn completion_on_empty_buffer() {
        let mut buf: Vec<char> = Vec::new();
        let mut cursor = 0;
        apply_completion(&mut buf, &mut cursor, "help");
        assert_eq!(buf.iter().collect::<String>(), "help");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn utf8_chars_are_char_indexed() {
        let mut buf: Vec<char> = "写".chars().collect();
        let mut cursor = buf.len();
        apply_completion(&mut buf, &mut cursor, "写文件");
        assert_eq!(buf.iter().collect::<String>(), "写文件");
        assert_eq!(cursor, 3);
    }
}
