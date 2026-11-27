//! AI 全宽浮层 —— 桌面右键打开的自然语言入口。
//!
//! 形态：横向铺满桌面的轻量浮层（不是窗口系统里的普通窗口），分三段：
//! * 顶部：输入框（用户用日常语言说出需求）
//! * 中部：AI 回应区，按帧流式呈现 + 加载动画
//! * 底部：仪表盘（token 速率 / KV 缓存命中 / 上下文占用 / 模型名 / 主题切换）
//!
//! 所有指标来自真实内核状态；流式呈现是本地分帧动画（仓库内无真实流式 LLM API）。

use std::sync::{Mutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

use super::intent::{Intent, IntentRouter};
use super::surface::Surface;
use super::{text_render, theme};

/// 浮层状态。
#[derive(Debug)]
pub struct Overlay {
    pub open: bool,
    pub x: i32,
    pub y: i32,
    pub input: String,
    /// 当前显示的回复（逐帧增长）。
    pub reply: String,
    /// 完整回复目标。
    pub target: String,
    /// 是否处于“思考中”（显示加载动画）。
    pub thinking: bool,
    pub started: Option<Instant>,
    /// 上一次推进流式的时刻。
    pub last_tick: Option<Instant>,
}

impl Overlay {
    fn new() -> Self {
        Self {
            open: false,
            x: 0,
            y: 0,
            input: String::new(),
            reply: String::new(),
            target: String::new(),
            thinking: false,
            started: None,
            last_tick: None,
        }
    }

    /// 在光标位置打开浮层（横向铺满屏幕）。
    pub fn open_at(&mut self, x: i32, y: i32, frame_w: u32, frame_h: u32) {
        self.open = true;
        self.x = x.clamp(16, (frame_w as i32 - 16).max(16));
        self.y = y.clamp(super::desktop_shell::TOPBAR_H + 8, frame_h as i32 - 8);
        self.input.clear();
        self.reply.clear();
        self.target.clear();
        self.thinking = false;
        self.started = None;
        self.last_tick = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.thinking = false;
    }

    /// 提交输入：经 IntentRouter 解析，生成自然语言回复并启动流式呈现。
    pub fn submit(&mut self) -> Option<Intent> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        let intent = IntentRouter::resolve(&text);
        self.target = match &intent {
            Some(Intent::OpenPage(page)) => format!("好的，正在打开{}页面。", page.label()),
            Some(Intent::ReturnDesktop) => "好的，回到桌面。".to_string(),
            Some(Intent::NextPage) => "切换到下一个页面。".to_string(),
            Some(Intent::PrevPage) => "切换到上一个页面。".to_string(),
            Some(Intent::ToggleDock) => "已展开底部图标。".to_string(),
            Some(Intent::OpenTerminal) => "正在打开终端。".to_string(),
            Some(Intent::RequestPermission { what, .. }) => {
                format!("“{}”需要你的确认。", what)
            }
            Some(Intent::Unknown(s)) => format!("我还没学会“{}”，可以试试“打开任务”。", s),
            None => "我没太听懂，可以说“打开任务”“打开文件”或“回到桌面”。".to_string(),
        };
        self.reply.clear();
        self.thinking = true;
        self.started = Some(Instant::now());
        self.last_tick = None;
        self.input.clear();
        intent
    }

    /// 推进流式呈现：每 60ms 多显示 2 个字符；完成后退出思考态。
    pub fn tick(&mut self) {
        if !self.open || !self.thinking {
            return;
        }
        let now = Instant::now();
        let period = std::time::Duration::from_millis(60);
        let first = self.last_tick.is_none();
        if first || now.duration_since(self.last_tick.unwrap()) >= period {
            self.last_tick = Some(now);
            let chars: Vec<char> = self.target.chars().collect();
            let next = (self.reply.chars().count() + 2).min(chars.len());
            self.reply = chars[..next].iter().collect();
            if next >= chars.len() {
                self.thinking = false;
            }
        }
    }

    /// 加载动画帧（4 相位）。
    pub fn spinner(&self) -> &'static str {
        if !self.thinking {
            return "";
        }
        let ms = self
            .started
            .map(|s| s.elapsed().as_millis())
            .unwrap_or(0);
        match (ms / 200) % 4 {
            0 => "|",
            1 => "/",
            2 => "-",
            _ => "\\",
        }
    }
}

static OVERLAY: OnceLock<Mutex<Overlay>> = OnceLock::new();

pub fn overlay() -> &'static Mutex<Overlay> {
    OVERLAY.get_or_init(|| Mutex::new(Overlay::new()))
}

pub fn is_open() -> bool {
    overlay().lock().unwrap().open
}

/// 推进流式动画（由事件循环调用）。
pub fn tick() {
    overlay().lock().unwrap().tick();
}

// ── 绘制 ──────────────────────────────────────────────────────────────────

const PANEL_BG: u32 = 0xF214161A;
const BORDER: u32 = 0xFF3A4048;

/// 当前主题主色（随主题切换变化）。
fn accent() -> u32 {
    theme::theme_manager()
        .lock()
        .unwrap()
        .current_theme()
        .color(theme::ColorRole::Primary)
}

/// 画浮层到帧缓冲最上层。
pub fn draw(frame: &mut Surface) {
    let o = overlay().lock().unwrap();
    if !o.open {
        return;
    }
    let fw = frame.width() as i32;
    let fh = frame.height() as i32;
    let h: u32 = 260;
    let x = 16;
    let y = (o.y - h as i32 / 2).clamp(super::desktop_shell::TOPBAR_H + 8, fh - h as i32 - 8);
    let w = fw - 32;

    // 面板底色 + 边框 + 主色顶边。
    frame.fill_rect(x, y, w as u32, h, PANEL_BG);
    frame.draw_rect(x, y, w as u32, h, BORDER, 1);
    frame.fill_rect(x, y, w as u32, 2, accent());

    let acc = accent();
    // 顶部：输入框（含光标闪烁）。
    frame.draw_ui_text(x + 16, y + 14, "ASK", acc, x + w - 16);
    let caret = if super::terminal_cursor_visible() { "|" } else { " " };
    let prompt = format!("{}{}", o.input, caret);
    frame.draw_ui_text(x + 58, y + 14, &prompt, 0xFFE6E8EC, x + w - 16);
    frame.fill_rect(x + 16, y + 32, (w - 32) as u32, 1, BORDER);

    // 中部：流式回复 + 加载动画。
    let spinner = o.spinner();
    let reply = if spinner.is_empty() {
        o.reply.clone()
    } else {
        format!("{} {}", o.reply, spinner)
    };
    frame.draw_ui_text(x + 16, y + 48, &reply, 0xFFC8CCD4, x + w - 16);

    // 底部仪表盘（高 44px）。
    let dash_y = y + h as i32 - 46;
    frame.fill_rect(x + 1, dash_y, (w - 2) as u32, 45, 0xFF101216);
    frame.fill_rect(x + 1, dash_y, (w - 2) as u32, 1, BORDER);

    let metrics = crate::ai_os::model::ai_metrics_snapshot();
    let rate = super::desktop_shell::topbar::token_rate();
    let pct = if metrics.context_limit == 0 {
        0.0
    } else {
        (metrics.context_chars as f32 / metrics.context_limit as f32).min(1.0)
    };

    let col1 = x + 16;
    let col2 = x + w / 4;
    let col3 = x + w / 2;
    let col4 = x + w * 3 / 4;

    frame.draw_ui_text(col1, dash_y + 8, "TOKEN RATE", 0xFF7F8896, x + w - 16);
    frame.draw_ui_text(col1, dash_y + 24, &format!("{} t/s", rate), acc, x + w - 16);

    frame.draw_ui_text(col2, dash_y + 8, "CACHE HIT", 0xFF7F8896, x + w - 16);
    frame.draw_ui_text(
        col2,
        dash_y + 24,
        &format!("{:.0}%", metrics.cache_hit_rate * 100.0),
        acc,
        x + w - 16,
    );

    frame.draw_ui_text(col3, dash_y + 8, "CONTEXT", 0xFF7F8896, x + w - 16);
    frame.draw_ui_text(
        col3,
        dash_y + 24,
        &format!("{}/{}", metrics.context_chars, metrics.context_limit),
        acc,
        x + w - 16,
    );
    // 上下文占用进度条。
    let bar_w = ((w / 4 - 24) as f32 * pct) as u32;
    frame.fill_rect(col3, dash_y + 40, (w / 4 - 24) as u32, 2, BORDER);
    frame.fill_rect(col3, dash_y + 40, bar_w.max(1), 2, acc);

    // 模型名 + 主题切换提示。
    let model: String = metrics.model_name.chars().take(20).collect();
    frame.draw_ui_text(col4, dash_y + 8, "MODEL", 0xFF7F8896, x + w - 16);
    frame.draw_ui_text(col4, dash_y + 24, &model, acc, x + w - 16);
    let theme_name = theme::theme_manager()
        .lock()
        .unwrap()
        .current_theme()
        .name
        .clone();
    frame.draw_ui_text(
        col4,
        dash_y + 40,
        &format!("THEME {}", theme_name.to_ascii_uppercase()),
        0xFF7F8896,
        x + w - 16,
    );

    let _ = text_render::BODY_PX;
    let _ = std::time::SystemTime::now().duration_since(UNIX_EPOCH);
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::intent::PageKind;

    #[test]
    fn open_at_clamps_position() {
        let mut o = Overlay::new();
        o.open_at(-50, -50, 960, 600);
        assert!(o.open);
        assert!(o.x >= 16);
        assert!(o.y > 0);
    }

    #[test]
    fn submit_generates_reply_and_starts_streaming() {
        let mut o = Overlay::new();
        o.input = "打开任务".to_string();
        let intent = o.submit();
        assert!(matches!(intent, Some(Intent::OpenPage(PageKind::Tasks))));
        assert!(o.thinking);
        assert!(o.target.contains("任务"));
        assert!(o.input.is_empty());
    }

    #[test]
    fn draw_overlay_with_dashboard() {
        let mut s = Surface::filled(960, 600, 0xFF000000);
        let before = s.lit_pixels();
        {
            let mut o = overlay().lock().unwrap();
            o.open_at(480, 300, 960, 600);
            o.input = "hello".to_string();
            o.target = "opening tasks".to_string();
            o.reply = "opening".to_string();
            o.thinking = true;
            o.started = Some(Instant::now());
        }
        draw(&mut s);
        overlay().lock().unwrap().close();
        // 浮层改变画面（面板底色/顶边强调色）。
        assert!(s.lit_pixels() > before);
    }

    #[test]
    fn spinner_advances() {
        let mut o = Overlay::new();
        o.thinking = true;
        o.started = Some(Instant::now());
        let a = o.spinner().to_string();
        std::thread::sleep(std::time::Duration::from_millis(220));
        let b = o.spinner().to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn tick_reveals_reply_progressively() {
        let mut o = Overlay::new();
        o.open = true;
        o.target = "ABCDEFGH".to_string();
        o.thinking = true;
        o.started = Some(Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(70));
        o.tick();
        assert!(o.reply.len() >= 2);
        assert!(o.reply.len() <= o.target.len());
    }
}
