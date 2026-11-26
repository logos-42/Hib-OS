//! 命令板（Omni Palette）—— Hib Shell 的核心交互面。
//!
//! 设计意图：AI 原生 OS 不该要求用户「找到图标再点」，而应该让用户
//! **用一句话/几个字母表达意图**，由系统把它翻译成一次可审计的通道动作。
//! 命令板就是这个入口：
//!
//! ```text
//!   ⌘K / Ctrl+K / 点击顶栏  ──►  输入  ──►  模糊匹配  ──►  Enter  ──►  act
//! ```
//!
//! 三条约束：
//! 1. 键盘完全可达：↑↓ 选择、Enter 执行、Esc 关闭、输入即过滤。
//! 2. 不绕过内核：每个条目最终落到 `Compositor` 的真实操作或一条
//!    `Effect::Command`（交给 shell / 通道执行），没有"界面私有捷径"。
//! 3. 与其它渲染共享同一份像素：终端预览、BMP 导出、真实窗口都包含命令板。

use std::sync::{Mutex, OnceLock};

use super::compositor::Compositor;
use super::input::{Effect, Event, Key, MouseButton};
use super::surface::{argb, palette as pal, Rect, Surface};
use super::window::Window;

/// 面板最大宽度（像素）。
pub const PANEL_MAX_W: u32 = 620;
/// 单行高度。
pub const ROW_H: i32 = 24;
/// 最多同时显示的结果行数。
pub const MAX_ROWS: usize = 8;
/// 顶栏高度（统一指向新外壳常量；不再用「点顶栏打开命令板」）。
pub const TOPBAR_H: i32 = super::desktop_shell::TOPBAR_H;

const HEADER_H: i32 = 38;
const FOOTER_H: i32 = 20;
const PAD: i32 = 10;

/// 一个命令板条目最终要执行的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// 打开内置应用。
    OpenApp(&'static str),
    /// 聚焦某个窗口。
    FocusWindow(u64),
    /// 关闭当前焦点窗口。
    CloseFocused,
    /// 最小化 / 还原当前焦点窗口。
    MinimizeFocused,
    /// 最大化 / 还原当前焦点窗口。
    MaximizeFocused,
    /// 把一条命令交给内核执行（走 `Effect::Command`）。
    Run(&'static str),
}

/// 命令板条目（动态生成：窗口列表会变）。
#[derive(Clone, Debug)]
pub struct Item {
    pub group: &'static str,
    pub title: String,
    pub hint: String,
    pub action: Action,
}

/// 命令板状态。
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// 最近一次执行结果（显示在顶栏，便于确认"确实发生了"）。
    pub last_result: String,
    /// 累计打开次数（诊断用）。
    pub opened: u64,
    /// 累计执行次数（诊断用）。
    pub executed: u64,
}

impl Palette {
    fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            last_result: String::new(),
            opened: 0,
            executed: 0,
        }
    }

    /// 打开并清空查询（Spotlight 行为：每次打开都是新的意图）。
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.opened += 1;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn status(&self) -> String {
        format!(
            "open={} query={:?} selected={} opened={} executed={} last={}",
            self.open, self.query, self.selected, self.opened, self.executed, self.last_result
        )
    }
}

static PALETTE: OnceLock<Mutex<Palette>> = OnceLock::new();

/// 全局命令板。
pub fn palette() -> &'static Mutex<Palette> {
    PALETTE.get_or_init(|| Mutex::new(Palette::new()))
}

/// 是否处于打开状态（供合成器决定要不要画遮罩）。
pub fn is_open() -> bool {
    palette().lock().map(|p| p.open).unwrap_or(false)
}

// ============================================================================
// 目录：所有可被"一句话"触达的能力
// ============================================================================

/// 构建当前可执行条目。窗口相关条目会随桌面状态变化。
pub fn catalog(windows: &[Window]) -> Vec<Item> {
    let mut items = Vec::new();

    // 应用（与任务栏启动器同源：AppKind::parse）。
    for app in super::apps::ALL_APPS {
        items.push(Item {
            group: "APP",
            title: format!("Open {}", app.title()),
            hint: app.name().to_string(),
            action: Action::OpenApp(app.name()),
        });
    }

    // 窗口动作。
    items.push(Item {
        group: "WINDOW",
        title: "Minimize focused window".into(),
        hint: "⌘M".into(),
        action: Action::MinimizeFocused,
    });
    items.push(Item {
        group: "WINDOW",
        title: "Maximize / restore window".into(),
        hint: "⌘↑".into(),
        action: Action::MaximizeFocused,
    });
    items.push(Item {
        group: "WINDOW",
        title: "Close focused window".into(),
        hint: "⌘W".into(),
        action: Action::CloseFocused,
    });

    // 跳到某个窗口。
    for w in windows.iter().take(12) {
        items.push(Item {
            group: "JUMP",
            title: format!("Focus #{} {}", w.id, w.title),
            hint: "window".into(),
            action: Action::FocusWindow(w.id),
        });
    }

    // 通道 / 系统命令：与 shell 完全同一条路径。
    for (title, hint, cmd) in [
        ("Desktop status", "gui status", "gui status"),
        ("Screenshot desktop", "gui shot", "gui shot gui-output.bmp"),
        ("Agent list", "agent list", "agent list"),
        ("Task list", "task list", "task list"),
        ("Permission audit", "system logs", "system logs"),
        ("Sense channels", "sense", "sense"),
    ] {
        items.push(Item {
            group: "CHANNEL",
            title: title.into(),
            hint: hint.into(),
            action: Action::Run(cmd),
        });
    }

    items
}

// ============================================================================
// 模糊匹配
// ============================================================================

/// 子序列打分：命中越靠前、越连续，分数越高。
///
/// 空查询返回 0（全部保留，按目录顺序展示）。
pub fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut qi = 0usize;
    let mut total = 0i32;
    let mut last_hit: Option<usize> = None;
    for (ti, ch) in t.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if *ch == q[qi] {
            // 连续命中给高分，词首命中次之。
            let bonus = match last_hit {
                Some(prev) if prev + 1 == ti => 8,
                _ => 0,
            };
            let start = if ti == 0 || !t[ti - 1].is_alphanumeric() { 6 } else { 0 };
            total += 10 + bonus + start;
            last_hit = Some(ti);
            qi += 1;
        }
    }
    if qi == q.len() {
        // 短标题略微优先，避免"Open TERMINAL"永远压过精确命令。
        Some(total - (t.len() as i32 / 4))
    } else {
        None
    }
}

/// 过滤 + 排序。
pub fn filtered(query: &str, windows: &[Window]) -> Vec<Item> {
    let mut scored: Vec<(i32, Item)> = catalog(windows)
        .into_iter()
        .filter_map(|item| {
            let s = score(query, &item.title)
                .or_else(|| score(query, &item.hint))
                .or_else(|| score(query, item.group))?;
            Some((s, item))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, item)| item).collect()
}

// ============================================================================
// 几何
// ============================================================================

fn panel_width(dst_w: u32) -> u32 {
    PANEL_MAX_W.min(dst_w.saturating_sub(64)).max(240)
}

fn visible_rows(total: usize) -> i32 {
    total.clamp(1, MAX_ROWS) as i32
}

/// 面板矩形（打开时居中偏上，像 Spotlight / Raycast）。
pub fn panel_rect(dst_w: u32, dst_h: u32, rows: usize) -> Rect {
    let w = panel_width(dst_w);
    let h = (HEADER_H + visible_rows(rows) * ROW_H + FOOTER_H + 8) as u32;
    let x = (dst_w as i32 - w as i32) / 2;
    let y = (dst_h as i32 / 6).max(TOPBAR_H + 12);
    Rect::new(x, y, w, h)
}

fn row_rect(panel: Rect, index: usize) -> Rect {
    Rect::new(
        panel.x + 6,
        panel.y + HEADER_H + index as i32 * ROW_H,
        (panel.w as i32 - 12).max(0) as u32,
        (ROW_H - 2) as u32,
    )
}

/// 点击落在第几行（0 基），不在结果区返回 None。
fn row_at(panel: Rect, rows: usize, x: i32, y: i32) -> Option<usize> {
    for i in 0..visible_rows(rows) as usize {
        if row_rect(panel, i).contains(x, y) {
            return Some(i);
        }
    }
    None
}

// ============================================================================
// 渲染
// ============================================================================

/// 把命令板画到已合成的帧上（最上层，压过窗口/任务栏/顶栏）。
pub fn render(dst: &mut Surface, windows: &[Window]) {
    let (pw, ph) = (dst.width(), dst.height());
    let (open, query, selected) = {
        let p = palette().lock().unwrap();
        if !p.open {
            return;
        }
        (p.open, p.query.clone(), p.selected)
    };
    if !open {
        return;
    }

    let items = filtered(&query, windows);
    let rows = items.len();
    let panel = panel_rect(pw, ph, rows);

    // 1) 遮罩：让底下的窗口退后一层（modal 语义）。
    let scrim = argb(120, 0, 0, 0);
    for y in 0..ph as i32 {
        for x in 0..pw as i32 {
            dst.blend(x, y, scrim);
        }
    }

    // 2) 阴影 + 面板。
    let shadow = Rect::new(panel.x + 3, panel.y + 4, panel.w, panel.h);
    dst.fill_round_rect(shadow, 12, argb(110, 0, 0, 0));
    dst.fill_round_rect(panel, 10, pal::MANTLE);
    dst.draw_rect(panel.x, panel.y, panel.w, panel.h, pal::SURFACE1, 1);
    // 顶部强调线：告诉用户"这是命令模式，不是普通窗口"。
    dst.fill_rect(panel.x + 10, panel.y, panel.w.saturating_sub(20), 2, pal::MAUVE);

    // 3) 输入行。
    let qy = panel.y + 12;
    dst.draw_text(panel.x + PAD, qy, ">", pal::MAUVE, 2);
    let text_x = panel.x + PAD + 16;
    let shown = if query.is_empty() { "type a command…" } else { &query };
    let color = if query.is_empty() { pal::OVERLAY0 } else { pal::TEXT };
    let end = dst.draw_text_clipped(
        text_x,
        qy,
        shown,
        color,
        2,
        panel.x + panel.w as i32 - PAD,
    );
    if !query.is_empty() {
        // 光标：2px 竖条，静态（点阵字体没有真实 blink，避免每帧抖动）。
        dst.fill_rect(end + 1, qy, 2, 14, pal::MAUVE);
    }

    // 4) 结果行。
    let sel = selected.min(items.len().saturating_sub(1));
    for i in 0..visible_rows(items.len()) as usize {
        let r = row_rect(panel, i);
        let Some(item) = items.get(i) else { break };
        if i == sel {
            dst.fill_round_rect(r, 5, pal::SURFACE0);
            dst.fill_rect(r.x, r.y + 4, 2, r.h.saturating_sub(8), pal::MAUVE);
        }
        let ty = r.y + (ROW_H - 2 - 7) / 2;
        dst.draw_text_clipped(
            r.x + 10,
            ty,
            &item.title,
            if i == sel { pal::TEXT } else { pal::SUBTEXT0 },
            1,
            r.x + r.w as i32 - 90,
        );
        // 右侧提示（快捷键 / 通道名）。
        let hint_w = Surface::text_width(&item.hint, 1) as i32;
        dst.draw_text(
            r.x + r.w as i32 - hint_w - 8,
            ty,
            &item.hint,
            if i == sel { pal::MAUVE } else { pal::OVERLAY0 },
            1,
        );
    }
    if items.is_empty() {
        let r = row_rect(panel, 0);
        dst.draw_text(r.x + 10, r.y + 8, "no matching command", pal::OVERLAY0, 1);
    }

    // 5) 底部提示。
    let fy = panel.y + panel.h as i32 - FOOTER_H + 4;
    dst.fill_rect(panel.x + 1, fy - 4, panel.w - 2, 1, pal::SURFACE0);
    dst.draw_text(
        panel.x + PAD,
        fy,
        "↑↓ select   ENTER run   ESC close",
        pal::OVERLAY0,
        1,
    );
    let brand = "HIB COMMAND";
    let bw = Surface::text_width(brand, 1) as i32;
    dst.draw_text(
        panel.x + panel.w as i32 - bw - PAD,
        fy,
        brand,
        pal::MAUVE,
        1,
    );
}

// ============================================================================
// 输入
// ============================================================================

/// 尝试由命令板消费该事件。
///
/// 返回 `Some(effects)` 表示事件已被命令板吃掉；`None` 表示继续走窗口路由。
pub fn intercept(comp: &mut Compositor, ev: &Event) -> Option<Vec<Effect>> {
    let mut p = palette().lock().unwrap();
    if !p.open {
        // 隐藏入口：只认 ⌘K / Ctrl+K；不再有「点顶栏打开」（顶栏左图标是回桌面）。
        let open_it = match ev {
            Event::KeyDown(Key::Other(s)) => matches!(s.as_str(), "cmd+k" | "cmd+space" | "ctrl+k"),
            _ => false,
        };
        if open_it {
            p.open();
            return Some(vec![Effect::Redraw]);
        }
        return None;
    }

    // 打开态：键盘全部归命令板，鼠标只认面板与遮罩。
    match ev {
        Event::KeyDown(key) => {
            match key {
                Key::Char(c) if !c.is_control() => {
                    p.query.push(*c);
                    p.selected = 0;
                }
                Key::Backspace => {
                    p.query.pop();
                    p.selected = 0;
                }
                Key::Escape => p.close(),
                Key::Enter => {
                    let items = filtered(&p.query, comp.windows());
                    if let Some(item) = items.get(p.selected.min(items.len().saturating_sub(1))).cloned() {
                        let eff = execute(comp, &mut p, item);
                        return Some(eff);
                    }
                }
                Key::Up => p.selected = p.selected.saturating_sub(1),
                Key::Down => {
                    let items = filtered(&p.query, comp.windows());
                    if !items.is_empty() {
                        p.selected = (p.selected + 1).min(items.len() - 1);
                    }
                }
                Key::Tab => {
                    // Tab 补全第一条结果到查询框，不执行（防误触）。
                    let items = filtered(&p.query, comp.windows());
                    if let Some(item) = items.get(p.selected) {
                        p.query = item.title.clone();
                        p.selected = 0;
                    }
                }
                Key::Other(s) if matches!(s.as_str(), "cmd+k" | "cmd+space" | "ctrl+k") => p.close(),
                _ => {}
            }
            Some(vec![Effect::Redraw])
        }
        Event::MouseDown { x, y, button } => {
            if *button != MouseButton::Left {
                return Some(vec![Effect::Redraw]);
            }
            let items = filtered(&p.query, comp.windows());
            let panel = panel_rect(comp.width(), comp.height(), items.len());
            if !panel.contains(*x, *y) {
                p.close();
                return Some(vec![Effect::Redraw]);
            }
            if let Some(idx) = row_at(panel, items.len(), *x, *y) {
                if let Some(item) = items.get(idx).cloned() {
                    p.selected = idx;
                    let eff = execute(comp, &mut p, item);
                    return Some(eff);
                }
            }
            Some(vec![Effect::Redraw])
        }
        Event::MouseMove { .. } | Event::MouseUp { .. } | Event::Wheel { .. } => {
            Some(vec![Effect::Redraw])
        }
        _ => None,
    }
}

/// 执行条目：只走真实内核操作或 `Effect::Command`。
fn execute(comp: &mut Compositor, p: &mut Palette, item: Item) -> Vec<Effect> {
    let mut effects = Vec::new();
    let result = match item.action {
        Action::OpenApp(name) => match super::apps::AppKind::parse(name) {
            Some(kind) => {
                let id = comp.open_app(kind);
                effects.push(Effect::Focused(Some(id)));
                format!("opened {} -> win#{}", name, id)
            }
            None => format!("unknown app {}", name),
        },
        Action::FocusWindow(id) => {
            if comp.window(id).is_some() {
                comp.focus(id);
                effects.push(Effect::Focused(Some(id)));
                format!("focused win#{}", id)
            } else {
                format!("win#{} gone", id)
            }
        }
        Action::CloseFocused => match comp.focused() {
            Some(id) => {
                comp.close(id);
                effects.push(Effect::Closed(id));
                format!("closed win#{}", id)
            }
            None => "no focused window".to_string(),
        },
        Action::MinimizeFocused => match comp.focused() {
            Some(id) => {
                if let Some(w) = comp.window_mut(id) {
                    w.minimized = !w.minimized;
                    format!("minimized win#{} = {}", id, w.minimized)
                } else {
                    "no focused window".to_string()
                }
            }
            None => "no focused window".to_string(),
        },
        Action::MaximizeFocused => match comp.focused() {
            Some(id) => {
                if let Some(w) = comp.window_mut(id) {
                    w.maximized = !w.maximized;
                    format!("maximized win#{} = {}", id, w.maximized)
                } else {
                    "no focused window".to_string()
                }
            }
            None => "no focused window".to_string(),
        },
        Action::Run(cmd) => {
            effects.push(Effect::Command {
                window: 0,
                line: cmd.to_string(),
            });
            format!("run: {}", cmd)
        }
    };
    p.executed += 1;
    p.last_result = result;
    p.close();
    effects.push(Effect::Redraw);
    effects
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::compositor::Compositor;

    fn fresh() -> Compositor {
        let mut c = Compositor::new(960, 600);
        let id = c.open("TERMINAL", 320, 220, pal::BLUE);
        c.focus(id);
        // 重置后立即释放锁：intercept 内部会再次 lock，持有 guard 会死锁。
        let mut p = palette().lock().unwrap();
        p.open = false;
        p.query.clear();
        p.selected = 0;
        drop(p);
        c
    }

    #[test]
    fn fuzzy_scores_prefix_and_consecutive() {
        assert!(score("ot", "Open TERMINAL").is_some());
        assert!(score("xyz", "Open TERMINAL").is_none());
        assert!(score("open", "Open TERMINAL").unwrap() > score("ot", "Open TERMINAL").unwrap());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn catalog_contains_apps_windows_and_channels() {
        let c = Compositor::new(960, 600);
        let items = catalog(c.windows());
        assert!(items.iter().any(|i| i.title.contains("Open TERMINAL")));
        assert!(items.iter().any(|i| i.group == "CHANNEL"));
    }

    #[test]
    fn query_filters_results() {
        let c = Compositor::new(960, 600);
        let r = filtered("term", c.windows());
        assert!(!r.is_empty());
        assert!(r[0].title.to_lowercase().contains("term"));
    }

    #[test]
    fn keyboard_opens_filters_and_runs() {
        let mut c = fresh();
        palette().lock().unwrap().open();
        assert!(palette().lock().unwrap().open);
        // 模拟键入 "screenshot"
        for ch in "screenshot".chars() {
            let eff = intercept(&mut c, &Event::KeyDown(Key::Char(ch))).unwrap();
            assert_eq!(eff, vec![Effect::Redraw]);
        }
        assert_eq!(palette().lock().unwrap().query, "screenshot");
        let eff = intercept(&mut c, &Event::KeyDown(Key::Enter)).unwrap();
        assert!(eff.iter().any(|e| matches!(e, Effect::Command { line, .. } if line == "gui shot gui-output.bmp")));
        assert!(!palette().lock().unwrap().open, "执行后命令板应关闭");
        assert!(palette().lock().unwrap().last_result.contains("gui shot"));
    }

    #[test]
    fn topbar_click_does_not_open_palette_shortcut_does() {
        let mut c = fresh();
        // 顶栏点击不再打开命令板（左图标由 IntentRouter 处理为回桌面）。
        assert_eq!(intercept(&mut c, &Event::MouseDown { x: 400, y: 8, button: MouseButton::Left }), None);
        assert!(!palette().lock().unwrap().open);
        // 隐藏入口 ⌘K 仍可打开。
        let eff = intercept(&mut c, &Event::KeyDown(Key::Other("cmd+k".into()))).unwrap();
        assert_eq!(eff, vec![Effect::Redraw]);
        assert!(palette().lock().unwrap().open);
        intercept(&mut c, &Event::KeyDown(Key::Escape)).unwrap();
        assert!(!palette().lock().unwrap().open);
    }

    #[test]
    fn opening_app_action_really_opens_window() {
        let mut c = fresh();
        let before = c.windows().len();
        {
            let mut p = palette().lock().unwrap();
            p.open();
            p.query = "files".into();
        }
        intercept(&mut c, &Event::KeyDown(Key::Enter)).unwrap();
        assert!(c.windows().len() > before, "FILES 应用应真的开窗");
        assert!(palette().lock().unwrap().last_result.contains("opened files"));
    }

    #[test]
    fn render_paints_scrim_and_panel() {
        let mut c = Compositor::new(960, 600);
        let mut s = Surface::filled(960, 600, pal::BASE);
        {
            let mut p = palette().lock().unwrap();
            p.open();
            p.query = "open".into();
        }
        render(&mut s, c.windows());
        let panel = panel_rect(960, 600, filtered("open", c.windows()).len());
        // 面板区域是 MANTLE，遮罩外的角落更暗。
        assert_eq!(s.get(panel.x + 4, panel.y + 30), Some(pal::MANTLE));
        assert!(s.get(4, 590).unwrap() != pal::BASE);
        let _ = c.compose();
    }
}
