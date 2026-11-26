//! 窗口内运行的应用 —— 每个窗口有自己的应用状态与像素内容。
//!
//! 之前窗口只是"一张画好的面板"；这里让窗口真正**跑起来**：
//! 应用每帧重绘自己的客户端表面、接收键盘、把命令交回内核执行再回显结果。
//!
//! 五种应用：终端 / 文件管理器 / 系统监视器 / 壁纸设置 / 记事本。

use super::surface::{palette, Surface};
use super::text_render;
use super::wallpaper;

/// 应用种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    Terminal,
    Files,
    Monitor,
    Wallpaper,
    Notes,
    /// 运行时由需求生成的应用：不出现在内置菜单，靠 `app generate` 产生。
    Generated,
}

pub const ALL_APPS: [AppKind; 5] = [
    AppKind::Terminal,
    AppKind::Files,
    AppKind::Monitor,
    AppKind::Wallpaper,
    AppKind::Notes,
];

impl AppKind {
    pub fn name(self) -> &'static str {
        match self {
            AppKind::Terminal => "terminal",
            AppKind::Files => "files",
            AppKind::Monitor => "monitor",
            AppKind::Wallpaper => "wallpaper",
            AppKind::Notes => "notes",
            AppKind::Generated => "generated",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            AppKind::Terminal => "TERMINAL",
            AppKind::Files => "FILES",
            AppKind::Monitor => "MONITOR",
            AppKind::Wallpaper => "WALLPAPER",
            AppKind::Notes => "NOTES",
            AppKind::Generated => "GENERATED APP",
        }
    }

    pub fn accent(self) -> u32 {
        match self {
            AppKind::Terminal => palette::BLUE,
            AppKind::Files => palette::GREEN,
            AppKind::Monitor => palette::MAUVE,
            AppKind::Wallpaper => palette::PEACH,
            AppKind::Notes => palette::YELLOW,
            AppKind::Generated => palette::MAUVE,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        ALL_APPS.iter().copied().find(|a| a.name() == s)
    }

    pub fn list() -> String {
        ALL_APPS
            .iter()
            .map(|a| a.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// 一个应用实例的运行状态。
pub struct AppState {
    pub kind: AppKind,
    pub lines: Vec<String>,
    pub input: String,
    pub scroll: usize,
    pub ticks: u64,
    /// 实时生成应用对应的说明书编号；内置应用为 `None`。
    pub spec_id: Option<String>,
}

impl AppState {
    pub fn new(kind: AppKind) -> Self {
        let mut s = Self {
            kind,
            lines: Vec::new(),
            input: String::new(),
            scroll: 0,
            ticks: 0,
            spec_id: None,
        };
        s.lines.push(format!("HIB OS / {}", kind.title()));
        match kind {
            AppKind::Terminal => {
                s.lines.push("Type a command and press Enter.".into());
                s.lines.push("Try: sense gpu/vram".into());
            }
            AppKind::Notes => s.lines.push("Just type; Enter adds a line.".into()),
            AppKind::Wallpaper => s.lines.push("Press 1-8 to switch wallpaper.".into()),
            _ => s.lines.push("Use Up/Down to scroll.".into()),
        }
        s
    }

    /// 用一份实时生成的说明书创建应用状态：窗口内容来自说明书，而不是编译期枚举。
    pub fn from_spec(spec: &crate::appgen::AppSpec) -> Self {
        Self {
            kind: AppKind::Generated,
            lines: spec.summary_lines(),
            input: String::new(),
            scroll: 0,
            ticks: 0,
            spec_id: Some(spec.id.clone()),
        }
    }

    /// 每帧重绘客户端表面（实时数据在这里刷新）。
    pub fn paint(&mut self, s: &mut Surface) {
        self.ticks += 1;
        s.fill(palette::BASE);
        let mut rows = self.lines.clone();
        match self.kind {
            AppKind::Monitor => {
                let st = crate::monitor::get_stats();
                rows = vec![
                    "HIB OS / MONITOR".into(),
                    format!("uptime = {} ms", st.system_uptime_ns / 1_000_000),
                    format!("tokens = {} active = {}", st.total_tokens, st.active_tokens),
                    format!("tasks ok = {} failed = {}", st.total_tasks_completed, st.total_tasks_failed),
                    format!("vram used = {} B", st.total_vram_used),
                    crate::exec::status_line(),
                    format!("frame = {}", self.ticks),
                ];
            }
            AppKind::Files => {
                let v = crate::vfs::Vfs::new();
                let mut r = vec!["HIB OS / FILES".into()];
                if let Ok(items) = v.ls(Some("/")) {
                    for (name, kind, mode, size) in items {
                        r.push(format!("{} {:04o} {:>6}B  {}", kind.type_char(), mode, size, name));
                    }
                }
                rows = r;
            }
            AppKind::Wallpaper => {
                let cur = wallpaper::bank().lock().unwrap().name.clone();
                let mut r = vec![
                    "HIB OS / WALLPAPER".into(),
                    format!("CURRENT: {}", cur),
                    "PRESS 1-9 TO SWITCH".into(),
                ];
                for (i, name) in wallpaper::STYLES.iter().enumerate() {
                    r.push(format!("  {} {}", i + 1, name));
                }
                r.push(self.input.clone());
                rows = r;
            }
            _ => {}
        }

        let pad = text_render::PAD;
        let line_h = text_render::BODY_LINE_H;
        let max = ((s.height() as i32 - pad * 2) / line_h).max(1) as usize;
        let start = rows.len().saturating_sub(max);
        for (i, line) in rows[start..].iter().enumerate() {
            let color = if line.starts_with("HIB OS") {
                text_render::ACCENT
            } else {
                text_render::FG
            };
            text_render::draw_body(s, pad, pad + i as i32 * line_h, line, color);
        }
        // 输入行（终端 / 记事本）
        if matches!(self.kind, AppKind::Terminal | AppKind::Notes | AppKind::Generated) {
            let y = pad + (rows.len() - start).min(max) as i32 * line_h;
            if y + line_h < s.height() as i32 {
                let prompt = if self.kind == AppKind::Terminal { "> " } else { "" };
                text_render::draw_body(
                    s,
                    pad,
                    y,
                    &format!("{}{}_", prompt, self.input),
                    text_render::PROMPT,
                );
            }
        }
    }

    /// 键盘字符。
    pub fn on_char(&mut self, c: char) {
        if self.kind == AppKind::Wallpaper {
            if let Some(i) = c.to_digit(10) {
                let idx = i as usize;
                if idx >= 1 && idx <= wallpaper::STYLES.len() {
                    let name = wallpaper::STYLES[idx - 1];
                    if let Ok(msg) = wallpaper::bank().lock().unwrap().set_style(name) {
                        self.input = msg;
                    }
                }
            }
            return;
        }
        if self.input.chars().count() < 180 {
            self.input.push(c);
        }
    }

    pub fn on_backspace(&mut self) {
        self.input.pop();
    }

    /// 回车：终端返回命令交给内核；记事本把输入变成一行。
    pub fn on_enter(&mut self) -> Option<String> {
        let line = std::mem::take(&mut self.input);
        match self.kind {
            AppKind::Terminal => {
                self.lines.push(format!("> {}", line));
                if line.trim().is_empty() {
                    None
                } else {
                    Some(line)
                }
            }
            AppKind::Notes => {
                self.lines.push(line);
                None
            }
            AppKind::Generated => {
                // 生成的应用也能交互：输入一行就执行它的 echo 动作，结果回显到窗口。
                self.lines.push(format!("> {}", line));
                if line.trim().is_empty() {
                    return None;
                }
                // 先复制编号再改 lines，避免同时可变/不可变借用 self。
                if let Some(id) = self.spec_id.clone() {
                    match crate::appgen::invoke_by_id(&id, "echo", &line) {
                        Ok(text) => self.lines.push(text),
                        Err(e) => self.lines.push(format!("ERROR: {}", e)),
                    }
                }
                None
            }
            _ => {
                self.input = line;
                None
            }
        }
    }

    /// 内核执行结果回显。
    pub fn output(&mut self, text: &str) {
        for line in text.lines() {
            self.lines.push(line.to_string());
        }
        while self.lines.len() > 200 {
            self.lines.remove(0);
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).max(0) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_list() {
        assert_eq!(AppKind::parse("files"), Some(AppKind::Files));
        assert!(AppKind::parse("nope").is_none());
        assert!(AppKind::list().contains("terminal"));
    }

    #[test]
    fn terminal_returns_command_and_echoes_output() {
        let mut a = AppState::new(AppKind::Terminal);
        a.on_char('l');
        a.on_char('s');
        assert_eq!(a.on_enter(), Some("ls".to_string()));
        assert!(a.lines.iter().any(|l| l == "> ls"));
        a.output("ok\nfile1");
        assert!(a.lines.iter().any(|l| l == "file1"));
    }

    #[test]
    fn notes_enter_makes_a_line() {
        let mut a = AppState::new(AppKind::Notes);
        a.on_char('h');
        a.on_char('i');
        assert_eq!(a.on_enter(), None);
        assert!(a.lines.iter().any(|l| l == "hi"));
    }

    #[test]
    fn paint_fills_pixels() {
        let mut a = AppState::new(AppKind::Monitor);
        let mut s = Surface::new(200, 100);
        a.paint(&mut s);
        assert!(s.pixels().iter().any(|p| *p != palette::BASE));
        assert_eq!(a.ticks, 1);
    }
}
