//! 输入子系统 —— 键盘 / 鼠标事件到窗口的路由。
//!
//! 事件不是「全局回调」，而是先进入队列，再由 [`dispatch`] 按 z-order
//! 路由到具体窗口：这对应真实 OS 的「输入通道 → 焦点窗口」模型。
//!
//! 支持的行为：
//! * 点击：置顶 + 聚焦；点关闭/最小化按钮立即生效
//! * 标题栏拖动：按住左键移动窗口（真实坐标跟随）
//! * 右下角缩放：按住左键改变窗口尺寸，客户端表面同步重建
//! * 键盘：只有焦点窗口收到字符 / 退格 / 回车
//! * 滚轮：滚动焦点窗口的终端回滚

use std::collections::VecDeque;

use super::compositor::{Compositor, DragMode};
use super::surface::Rect;
use super::window::Hit;

/// 键盘按键（已做平台无关抽象）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Enter,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    /// 未映射的按键，保留原始名字。
    Other(String),
}

/// 输入事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    MouseMove { x: i32, y: i32 },
    MouseDown { x: i32, y: i32, button: MouseButton },
    MouseUp { x: i32, y: i32, button: MouseButton },
    Wheel { x: i32, y: i32, delta: i32 },
    KeyDown(Key),
    /// 触摸按下 / 移动 / 抬起（触摸屏 / 触控板）。
    TouchDown { id: u64, x: i32, y: i32 },
    TouchMove { id: u64, x: i32, y: i32 },
    TouchUp { id: u64, x: i32, y: i32 },
    /// 窗口关闭请求（真实窗口后端用）。
    CloseRequested,
    /// 需要重绘（后端在 Resized / 定时器里发）。
    Redraw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// 一次 dispatch 的副作用，交给宿主（shell / 后端）处理。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// 窗口被关闭。
    Closed(u64),
    /// 焦点变化。
    Focused(Option<u64>),
    /// 需要把命令交给内核执行（终端里输入的命令）。
    Command { window: u64, line: String },
    /// 画面需要重绘。
    Redraw,
}

/// 输入队列（可观测，便于测试与真实后端共用）。
#[derive(Default)]
pub struct InputQueue {
    queue: VecDeque<Event>,
    pub accepted: u64,
    pub dropped: u64,
    capacity: usize,
}

impl InputQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            accepted: 0,
            dropped: 0,
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, ev: Event) {
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(ev);
        self.accepted += 1;
    }

    pub fn pop(&mut self) -> Option<Event> {
        self.queue.pop_front()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn status(&self) -> String {
        format!(
            "input queue={} accepted={} dropped={} capacity={}",
            self.queue.len(),
            self.accepted,
            self.dropped,
            self.capacity
        )
    }
}

/// 把事件应用到合成器，返回副作用列表。
pub fn dispatch(comp: &mut Compositor, ev: &Event) -> Vec<Effect> {
    // 命令板已打开时，它优先消费所有按键（含 Esc 关闭），避免被工作区快捷键抢走。
    if super::command_palette::is_open() {
        if let Some(effects) = super::command_palette::intercept(comp, ev) {
            return effects;
        }
    }
    // 优先级 1：工作区 / 意图路由（Esc 回桌面、⌘K 高级工具、⌘Tab 切页、快捷键）。
    if let Some(effects) = dispatch_intent_first(comp, ev) {
        return effects;
    }
    // 优先级 2：命令板隐藏入口（⌘K 打开）。
    if let Some(effects) = super::command_palette::intercept(comp, ev) {
        return effects;
    }
    match ev {
        Event::MouseMove { x, y } => {
            comp.set_cursor(*x, *y);
            match comp.drag() {
                DragMode::Move { id, dx, dy } => {
                    if let Some(w) = comp.window_mut(id) {
                        w.move_to(x - dx, y - dy);
                    }
                }
                DragMode::Resize { id, ox, oy } => {
                    if let Some(w) = comp.window_mut(id) {
                        let ww = (x - ox).max(120) as u32;
                        let hh = (y - oy).max(60) as u32;
                        w.resize(ww, hh);
                    }
                }
                DragMode::None => {}
            }
            vec![Effect::Redraw]
        }
        Event::MouseDown { x, y, button } => {
            comp.set_cursor(*x, *y);
            if *button != MouseButton::Left {
                return vec![Effect::Redraw];
            }
            // 底部 Dock 的点击已在 dispatch_intent_first 中处理。
            let Some((id, hit)) = comp.hit_test(*x, *y) else {
                // 点空白：取消焦点（真实桌面行为）
                return vec![Effect::Redraw];
            };
            comp.focus(id);
            match hit {
                Hit::CloseButton => {
                    comp.close(id);
                    return vec![Effect::Closed(id), Effect::Focused(comp.focused()), Effect::Redraw];
                }
                Hit::MinimizeButton => {
                    if let Some(w) = comp.window_mut(id) {
                        w.minimized = !w.minimized;
                    }
                }
                Hit::MaximizeButton => {
                    if let Some(w) = comp.window_mut(id) {
                        w.maximized = !w.maximized;
                    }
                }
                Hit::TitleBar => {
                    let (wx, wy) = comp
                        .window(id)
                        .map(|w| (w.x, w.y))
                        .unwrap_or((0, 0));
                    comp.set_drag(DragMode::Move {
                        id,
                        dx: *x - wx,
                        dy: *y - wy,
                    });
                }
                Hit::ResizeCorner => {
                    let (wx, wy) = comp
                        .window(id)
                        .map(|w| (w.x, w.y))
                        .unwrap_or((0, 0));
                    // 记录窗口原点，移动时用「鼠标 - 原点」算新尺寸。
                    comp.set_drag(DragMode::Resize {
                        id,
                        ox: wx,
                        oy: wy,
                    });
                }
                Hit::Client | Hit::None => {}
            }
            vec![Effect::Focused(comp.focused()), Effect::Redraw]
        }
        Event::MouseUp { x, y, button } => {
            comp.set_cursor(*x, *y);
            if *button == MouseButton::Left {
                comp.set_drag(DragMode::None);
            }
            vec![Effect::Redraw]
        }
        Event::Wheel { x, y, delta } => {
            comp.set_cursor(*x, *y);
            if let Some((id, _)) = comp.hit_test(*x, *y) {
                if let Some(w) = comp.window_mut(id) {
                    w.scroll_by(-delta.signum() * 2);
                }
            }
            vec![Effect::Redraw]
        }
        Event::KeyDown(key) => {
            let Some(id) = comp.focused() else {
                return vec![];
            };
            let Some(w) = comp.window_mut(id) else {
                return vec![];
            };
            match key {
                Key::Char(c) => {
                    w.type_char(*c);
                    vec![Effect::Redraw]
                }
                Key::Backspace => {
                    w.backspace();
                    vec![Effect::Redraw]
                }
                Key::Enter => {
                    if let Some(line) = w.enter() {
                        vec![Effect::Command { window: id, line }, Effect::Redraw]
                    } else {
                        vec![Effect::Redraw]
                    }
                }
                Key::Up => {
                    w.scroll_by(-1);
                    vec![Effect::Redraw]
                }
                Key::Down => {
                    w.scroll_by(1);
                    vec![Effect::Redraw]
                }
                Key::Tab => {
                    w.type_char('\t');
                    vec![Effect::Redraw]
                }
                Key::Left | Key::Right | Key::Escape | Key::Other(_) => vec![],
            }
        }
        Event::TouchDown { id, x, y } => {
            let now = now_ms();
            super::gesture::gesture_recognizer()
                .lock()
                .unwrap()
                .touch_down(*id, *x, *y, now);
            vec![Effect::Redraw]
        }
        Event::TouchMove { id, x, y } => {
            let now = now_ms();
            let mut gr = super::gesture::gesture_recognizer().lock().unwrap();
            gr.touch_move(*id, *x, *y, now);
            let gesture = gr.recognize();
            drop(gr);
            apply_gesture_intent(gesture);
            vec![Effect::Redraw]
        }
        Event::TouchUp { id, x, y } => {
            let now = now_ms();
            let down = touch_down_time(*id).unwrap_or(now);
            let mut gr = super::gesture::gesture_recognizer().lock().unwrap();
            gr.touch_up(*id, *x, *y, now);
            let gesture = gr.recognize_after_up(*id, *x, *y, down, now);
            drop(gr);
            clear_touch_down_time(*id);
            apply_gesture_intent(gesture);
            vec![Effect::Redraw]
        }
        Event::CloseRequested => vec![],
        Event::Redraw => vec![Effect::Redraw],
    }
}

/// 意图优先分发：只处理明确的工作区级意图，返回 `Some` 表示事件已被消费。
fn dispatch_intent_first(comp: &mut Compositor, ev: &Event) -> Option<Vec<Effect>> {
    let key_name = match ev {
        Event::KeyDown(Key::Escape) => Some("Escape"),
        Event::KeyDown(Key::Tab) => None,
        Event::KeyDown(Key::Other(s)) if s.eq_ignore_ascii_case("cmdk") => Some("CmdK"),
        Event::KeyDown(Key::Other(s)) if s.eq_ignore_ascii_case("cmdtab") => Some("CmdTab"),
        Event::KeyDown(Key::Other(s)) if s.eq_ignore_ascii_case("cmdshift tab") => Some("CmdShiftTab"),
        _ => None,
    };
    if let Some(name) = key_name {
        if let Some(intent) = super::intent::IntentRouter::resolve_key(name) {
            let mut ws = super::workspace::workspaces().lock().unwrap();
            if ws.handle_intent(intent.clone()) {
                // ⌘K 进「高级工具」全屏页，同时打开命令板（保留 CHANNEL / Effect::Command 能力）。
                if name == "CmdK" {
                    if let Ok(mut p) = super::command_palette::palette().lock() {
                        p.open();
                    }
                }
                return Some(vec![Effect::Redraw]);
            }
        }
    }
    if let Event::MouseDown { x, y, button: MouseButton::Left } = ev {
        // 顶栏左图标（x<24 且 y<TOPBAR_H）点击 → 回桌面；顶栏其余区域不再打开命令板。
        if *y < super::desktop_shell::TOPBAR_H && *x < 24 {
            super::workspace::workspaces()
                .lock()
                .unwrap()
                .handle_intent(super::intent::Intent::ReturnDesktop);
            return Some(vec![Effect::Redraw]);
        }
        // Dock 图标命中 → 打开对应全屏工作区页面；优先于旧任务栏命中。
        if let Some(page) = super::desktop_shell::dock::hit_page(*x, *y, comp.width(), comp.height()) {
            super::workspace::workspaces()
                .lock()
                .unwrap()
                .handle_intent(super::intent::Intent::OpenPage(page));
            return Some(vec![Effect::Redraw]);
        }
    }
    let _ = comp;
    None
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn touch_times() -> &'static std::sync::Mutex<std::collections::BTreeMap<u64, u64>> {
    static TIMES: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<u64, u64>>> =
        std::sync::OnceLock::new();
    TIMES.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

fn touch_down_time(id: u64) -> Option<u64> {
    touch_times().lock().unwrap().get(&id).copied()
}

fn clear_touch_down_time(id: u64) {
    touch_times().lock().unwrap().remove(&id);
}

fn apply_gesture_intent(gesture: Option<super::gesture::Gesture>) {
    if let Some(g) = gesture {
        if let Some(intent) = super::intent::IntentRouter::resolve_gesture(g) {
            super::workspace::workspaces().lock().unwrap().handle_intent(intent);
        }
    }
}

/// 把一串触摸事件路由成意图列表（供测试与无 GUI 环境验证）。
pub fn route_touch_events(events: &[Event]) -> Vec<super::intent::Intent> {
    let mut recognizer = super::gesture::GestureRecognizer::new();
    let mut intents = Vec::new();
    let mut downs: std::collections::BTreeMap<u64, u64> = std::collections::BTreeMap::new();
    for ev in events {
        match ev {
            Event::TouchDown { id, x, y } => {
                let t = recognizer.active_count() as u64 * 10;
                recognizer.touch_down(*id, *x, *y, t);
                downs.insert(*id, t);
            }
            Event::TouchMove { id, x, y } => {
                let t = downs.get(id).copied().unwrap_or(0) + 50;
                recognizer.touch_move(*id, *x, *y, t);
                if let Some(g) = recognizer.recognize() {
                    if let Some(i) = super::intent::IntentRouter::resolve_gesture(g) {
                        intents.push(i);
                    }
                }
            }
            Event::TouchUp { id, x, y } => {
                let down = downs.remove(id).unwrap_or(0);
                let t = down + 100;
                recognizer.touch_up(*id, *x, *y, t);
                if let Some(g) = recognizer.recognize_after_up(*id, *x, *y, down, t) {
                    if let Some(i) = super::intent::IntentRouter::resolve_gesture(g) {
                        intents.push(i);
                    }
                }
            }
            _ => {}
        }
    }
    intents
}

#[cfg(test)]
mod touch_tests {
    use super::*;

    #[test]
    fn touch_event_dispatches_to_gesture() {
        let events = vec![
            Event::TouchDown { id: 1, x: 300, y: 100 },
            Event::TouchMove { id: 1, x: 100, y: 100 },
            Event::TouchUp { id: 1, x: 100, y: 100 },
        ];
        let intents = route_touch_events(&events);
        assert!(intents.contains(&super::super::intent::Intent::NextPage));
    }
}

/// 便捷函数：鼠标点击窗口标题栏并拖动到新位置。
pub fn drag_window(comp: &mut Compositor, id: u64, x: i32, y: i32) {
    let Some(w) = comp.window(id) else { return };
    let (wx, wy) = (w.x, w.y);
    dispatch(comp, &Event::MouseDown {
        x: wx + 10,
        y: wy + 10,
        button: MouseButton::Left,
    });
    dispatch(comp, &Event::MouseMove { x: x + 10, y: y + 10 });
    dispatch(comp, &Event::MouseUp { x: x + 10, y: y + 10, button: MouseButton::Left });
}

/// 便捷函数：向焦点窗口输入一段文本。
pub fn type_text(comp: &mut Compositor, text: &str) -> Vec<Effect> {
    let mut effects = Vec::new();
    for ch in text.chars() {
        let key = if ch == '\n' {
            Key::Enter
        } else if ch == '\u{8}' {
            Key::Backspace
        } else {
            Key::Char(ch)
        };
        effects.extend(dispatch(comp, &Event::KeyDown(key)));
    }
    effects
}

/// 某窗口的屏幕矩形（供点击测试）。
pub fn window_rect(comp: &Compositor, id: u64) -> Option<Rect> {
    comp.window(id).map(|w| w.rect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Compositor, u64, u64) {
        let mut c = Compositor::new(800, 600);
        let a = c.open_terminal("A", 300, 220, "> ");
        c.window_mut(a).unwrap().move_to(40, 40);
        let b = c.open_terminal("B", 300, 220, "$ ");
        c.window_mut(b).unwrap().move_to(400, 300);
        (c, a, b)
    }

    #[test]
    fn click_focuses_and_raises() {
        let (mut c, a, _b) = setup();
        let (wx, wy) = {
            let w = c.window(a).unwrap();
            (w.x, w.y)
        };
        dispatch(&mut c, &Event::MouseDown {
            x: wx + 20,
            y: wy + 10,
            button: MouseButton::Left,
        });
        assert_eq!(c.focused(), Some(a));
        assert_eq!(c.windows().last().unwrap().id, a);
    }

    #[test]
    fn close_button_closes_window() {
        let (mut c, a, b) = setup();
        let r = c.window(a).unwrap().close_rect();
        let effects = dispatch(&mut c, &Event::MouseDown {
            x: r.x + 3,
            y: r.y + 3,
            button: MouseButton::Left,
        });
        assert!(c.window(a).is_none());
        assert!(effects.contains(&Effect::Closed(a)));
        assert_eq!(c.focused(), Some(b));
    }

    #[test]
    fn dragging_titlebar_moves_window() {
        let (mut c, a, _) = setup();
        let before = (c.window(a).unwrap().x, c.window(a).unwrap().y);
        drag_window(&mut c, a, 200, 180);
        let after = (c.window(a).unwrap().x, c.window(a).unwrap().y);
        assert_eq!(after, (200, 180));
        assert_ne!(before, after);
        assert_eq!(c.drag(), DragMode::None);
    }

    #[test]
    fn typing_goes_to_focused_window_only() {
        let (mut c, a, b) = setup();
        c.focus(a);
        type_text(&mut c, "hi");
        assert_eq!(c.window(a).unwrap().terminal.as_ref().unwrap().input, "hi");
        assert_eq!(c.window(b).unwrap().terminal.as_ref().unwrap().input, "");
    }

    #[test]
    fn enter_emits_command_effect() {
        let (mut c, a, _) = setup();
        c.focus(a);
        let effects = type_text(&mut c, "heap\n");
        assert!(effects.iter().any(|e| matches!(e, Effect::Command { line, .. } if line == "heap")));
        assert!(c.window(a).unwrap().terminal.as_ref().unwrap().lines.iter().any(|l| l == "> heap"));
    }

    #[test]
    fn resize_corner_resizes() {
        let (mut c, a, _) = setup();
        let w = c.window(a).unwrap();
        let (cx, cy) = (w.x + w.w as i32 - 5, w.y + w.h as i32 - 5);
        dispatch(&mut c, &Event::MouseDown { x: cx, y: cy, button: MouseButton::Left });
        dispatch(&mut c, &Event::MouseMove { x: cx + 80, y: cy + 40 });
        dispatch(&mut c, &Event::MouseUp { x: cx + 80, y: cy + 40, button: MouseButton::Left });
        let w = c.window(a).unwrap();
        assert!(w.w > 300);
        assert!(w.h > 220);
        assert_eq!(w.client.width(), w.w - 2);
    }

    #[test]
    fn wheel_scrolls_terminal() {
        let (mut c, a, _) = setup();
        for i in 0..40 {
            if let Some(w) = c.window_mut(a) {
                if let Some(t) = w.terminal.as_mut() {
                    t.push_line(format!("line {}", i));
                }
            }
        }
        c.focus(a);
        let w = c.window(a).unwrap();
        let (x, y) = (w.x + 20, w.y + 60);
        dispatch(&mut c, &Event::Wheel { x, y, delta: -1 });
        assert!(c.window(a).unwrap().terminal.as_ref().unwrap().scroll > 0);
    }

    #[test]
    fn queue_drops_oldest_when_full() {
        let mut q = InputQueue::new(2);
        q.push(Event::Redraw);
        q.push(Event::Redraw);
        q.push(Event::KeyDown(Key::Char('a')));
        assert_eq!(q.len(), 2);
        assert_eq!(q.dropped, 1);
        assert!(q.status().contains("queue=2"));
    }

    #[test]
    fn minimize_toggles_and_hides() {
        let (mut c, a, _) = setup();
        let r = c.window(a).unwrap().minimize_rect();
        dispatch(&mut c, &Event::MouseDown { x: r.x + 2, y: r.y + 2, button: MouseButton::Left });
        assert!(c.window(a).unwrap().minimized);
        // 最小化后不能命中
        let (x, y) = (c.window(a).unwrap().x + 10, c.window(a).unwrap().y + 10);
        assert!(c.hit_test(x, y).is_none());
    }
}
