//! AI 原生 GUI 层 —— AI 原生 OS 的「智能图形界面」 (AI-Native GUI)
//!
//! 本模块实现 AI 原生 OS 的图形用户界面，对应传统 OS 中的
//! 「窗口系统 / 合成器 / 事件循环 / 主题」概念，但重新映射到 AI 域：
//!
//! * SemanticWindow (语义窗口) —— 带语义标签的窗口，等价于传统窗口；
//! * AIGWidget (AI 小部件) —— 智能 UI 小部件，等价于传统控件；
//! * AILayer (AI 图层) —— 合成图层，等价于图层 / 表面；
//! * AIEvent (AI 事件) —— 带语义的输入事件，等价于传统输入事件；
//! * AICompositor (AI 合成器) —— 多图层合成引擎，等价于窗口合成器；
//! * AITheme (AI 主题) —— 自适应视觉主题，等价于主题引擎；
//! * AILayout (AI 布局) —— 基于意图的自动布局，等价于布局管理器；
//! * AIMultiInput (多模态输入) —— 触摸/笔/语音/视线统一输入，等价于输入子系统；
//! * AIWindowManager (AI 窗口管理器) —— 智能窗口管理，等价于 WM；
//! * GUAI (AI 原生 GUI 引擎) —— 统一入口，通过 gui_ai://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无显示硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 语义窗口 (SemanticWindow)
// =====================================================================================

/// 窗口状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Hidden,
    Visible,
    Minimized,
    Maximized,
    Closed,
}

/// 语义窗口：带语义标签的窗口。
#[derive(Clone, Debug)]
pub struct SemanticWindow {
    pub id: u64,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub state: WindowState,
    pub semantic_tag: Option<String>,
    pub z_order: u64,
    pub focus: bool,
    pub created_at: u128,
}

impl SemanticWindow {
    pub fn new(id: u64, title: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            id,
            title: title.to_string(),
            x,
            y,
            w,
            h,
            state: WindowState::Visible,
            semantic_tag: None,
            z_order: 0,
            focus: false,
            created_at: crate::ai_os::native::now_ns(),
        }
    }

    pub fn set_tag(&mut self, tag: &str) {
        self.semantic_tag = Some(tag.to_string());
    }

    pub fn set_focus(&mut self, focused: bool) {
        self.focus = focused;
    }

    pub fn minimize(&mut self) {
        self.state = WindowState::Minimized;
    }

    pub fn restore(&mut self) {
        self.state = WindowState::Visible;
    }

    pub fn close(&mut self) {
        self.state = WindowState::Closed;
    }

    pub fn area(&self) -> u64 {
        (self.w as u64) * (self.h as u64)
    }
}

// =====================================================================================
// AI 小部件 (AIGWidget)
// =====================================================================================

/// 小部件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetType {
    Button,       // 按钮
    Label,        // 标签
    TextInput,    // 文本输入
    Checkbox,     // 复选框
    Radio,        // 单选框
    Slider,       // 滑块
    Progress,     // 进度条
    Image,        // 图像
    Canvas,       // 画布
    Menu,         // 菜单
}

/// AI 小部件：智能 UI 小部件。
#[derive(Clone, Debug)]
pub struct AIGWidget {
    pub id: u64,
    pub r#type: WidgetType,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub enabled: bool,
    pub visible: bool,
    pub value: String,
    pub semantic_role: Option<String>,
    pub window_id: Option<u64>,
}

impl AIGWidget {
    pub fn new(id: u64, r#type: WidgetType, label: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            id,
            r#type,
            label: label.to_string(),
            x,
            y,
            w,
            h,
            enabled: true,
            visible: true,
            value: String::new(),
            semantic_role: None,
            window_id: None,
        }
    }

    pub fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
    }

    pub fn set_role(&mut self, role: &str) {
        self.semantic_role = Some(role.to_string());
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn show(&mut self) {
        self.visible = true;
    }
}

// =====================================================================================
// AI 图层 (AILayer)
// =====================================================================================

/// 图层：合成图层。
#[derive(Clone, Debug)]
pub struct AILayer {
    pub id: u64,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub opacity: f32,
    pub visible: bool,
    pub window_id: Option<u64>,
}

impl AILayer {
    pub fn new(id: u64, name: &str, x: i32, y: i32, w: u32, h: u32) -> Self {
        Self {
            id,
            name: name.to_string(),
            x,
            y,
            w,
            h,
            opacity: 1.0,
            visible: true,
            window_id: None,
        }
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.0, 1.0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn show(&mut self) {
        self.visible = true;
    }
}

// =====================================================================================
// AI 事件 (AIEvent)
// =====================================================================================

/// 输入事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEventType {
    KeyPress,
    KeyRelease,
    MouseMove,
    MouseButton,
    Touch,
    Pen,
    Gesture,
    Voice,
    Gaze,
}

/// AI 事件：带语义的输入事件。
#[derive(Clone, Debug)]
pub struct AIEvent {
    pub id: u64,
    pub r#type: InputEventType,
    pub timestamp_ns: u128,
    pub x: i32,
    pub y: i32,
    pub data: String,
    pub semantic_intent: Option<String>,
    pub handled: bool,
}

impl AIEvent {
    pub fn new(id: u64, r#type: InputEventType, x: i32, y: i32, data: &str) -> Self {
        Self {
            id,
            r#type,
            timestamp_ns: crate::ai_os::native::now_ns(),
            x,
            y,
            data: data.to_string(),
            semantic_intent: None,
            handled: false,
        }
    }

    pub fn set_intent(&mut self, intent: &str) {
        self.semantic_intent = Some(intent.to_string());
    }

    pub fn mark_handled(&mut self) {
        self.handled = true;
    }
}

// =====================================================================================
// AI 合成器 (AICompositor)
// =====================================================================================

/// AI 合成器：多图层合成引擎。
pub struct AICompositor {
    pub layers: BTreeMap<u64, AILayer>,
    pub next_layer_id: u64,
    pub frame_count: u64,
    pub last_frame_ns: u128,
    pub fps: f32,
}

impl AICompositor {
    pub fn new() -> Self {
        Self {
            layers: BTreeMap::new(),
            next_layer_id: 1,
            frame_count: 0,
            last_frame_ns: 0,
            fps: 0.0,
        }
    }

    pub fn add_layer(&mut self, name: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        let id = self.next_layer_id;
        self.next_layer_id += 1;
        self.layers.insert(id, AILayer::new(id, name, x, y, w, h));
        id
    }

    pub fn remove_layer(&mut self, id: u64) -> bool {
        self.layers.remove(&id).is_some()
    }

    pub fn compose(&mut self, now_ns: u128) {
        self.frame_count += 1;
        if self.last_frame_ns > 0 {
            let dt = now_ns - self.last_frame_ns;
            if dt > 0 {
                self.fps = 1_000_000_000.0 / dt as f32;
            }
        }
        self.last_frame_ns = now_ns;
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub fn visible_layer_count(&self) -> usize {
        self.layers.values().filter(|l| l.visible).count()
    }
}

// =====================================================================================
// AI 主题 (AITheme)
// =====================================================================================

/// 颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AIColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl AIColor {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn black() -> Self { Self { r: 0, g: 0, b: 0, a: 255 } }
    pub fn white() -> Self { Self { r: 255, g: 255, b: 255, a: 255 } }
    pub fn blue() -> Self { Self { r: 0, g: 122, b: 255, a: 255 } }
}

/// AI 主题：自适应视觉主题。
pub struct AITheme {
    pub name: String,
    pub bg_color: AIColor,
    pub fg_color: AIColor,
    pub accent_color: AIColor,
    pub font_size: u32,
    pub border_radius: u32,
    pub animation_enabled: bool,
}

impl AITheme {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            bg_color: AIColor::black(),
            fg_color: AIColor::white(),
            accent_color: AIColor::blue(),
            font_size: 14,
            border_radius: 4,
            animation_enabled: true,
        }
    }

    pub fn set_colors(&mut self, bg: AIColor, fg: AIColor, accent: AIColor) {
        self.bg_color = bg;
        self.fg_color = fg;
        self.accent_color = accent;
    }

    pub fn set_animation(&mut self, enabled: bool) {
        self.animation_enabled = enabled;
    }
}

// =====================================================================================
// AI 布局 (AILayout)
// =====================================================================================

/// 布局类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutType {
    Vertical,   // 垂直
    Horizontal, // 水平
    Grid,       // 网格
    Free,       // 自由
}

/// AI 布局：基于意图的自动布局。
pub struct AILayout {
    pub r#type: LayoutType,
    pub spacing: u32,
    pub margin: u32,
    pub auto_wrap: bool,
}

impl AILayout {
    pub fn new(r#type: LayoutType) -> Self {
        Self {
            r#type,
            spacing: 8,
            margin: 16,
            auto_wrap: false,
        }
    }

    pub fn set_spacing(&mut self, spacing: u32) {
        self.spacing = spacing;
    }

    pub fn set_margin(&mut self, margin: u32) {
        self.margin = margin;
    }
}

// =====================================================================================
// 多模态输入 (AIMultiInput)
// =====================================================================================

/// 输入模态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputModality {
    Touch,    // 触摸
    Pen,      // 手写笔
    Voice,    // 语音
    Gaze,     // 视线
    Keyboard, // 键盘
    Mouse,    // 鼠标
}

/// 多模态输入：统一输入子系统。
pub struct AIMultiInput {
    pub active_modalities: BTreeSet<InputModality>,
    pub event_count: u64,
}

impl AIMultiInput {
    pub fn new() -> Self {
        Self {
            active_modalities: BTreeSet::new(),
            event_count: 0,
        }
    }

    pub fn enable_modality(&mut self, modality: InputModality) {
        self.active_modalities.insert(modality);
    }

    pub fn disable_modality(&mut self, modality: InputModality) {
        self.active_modalities.remove(&modality);
    }

    pub fn record_event(&mut self) {
        self.event_count += 1;
    }

    pub fn modality_count(&self) -> usize {
        self.active_modalities.len()
    }
}

// =====================================================================================
// AI 窗口管理器 (AIWindowManager)
// =====================================================================================

/// AI 窗口管理器：智能窗口管理。
pub struct AIWindowManager {
    pub windows: BTreeMap<u64, SemanticWindow>,
    pub next_window_id: u64,
    pub focused_window: Option<u64>,
    pub total_windows_created: u64,
    pub total_windows_closed: u64,
}

impl AIWindowManager {
    pub fn new() -> Self {
        Self {
            windows: BTreeMap::new(),
            next_window_id: 1,
            focused_window: None,
            total_windows_created: 0,
            total_windows_closed: 0,
        }
    }

    pub fn create_window(&mut self, title: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        let window = SemanticWindow::new(id, title, x, y, w, h);
        self.total_windows_created += 1;
        self.windows.insert(id, window);
        self.focused_window = Some(id);
        id
    }

    pub fn close_window(&mut self, id: u64) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.close();
            self.total_windows_closed += 1;
            if self.focused_window == Some(id) {
                self.focused_window = self.windows.keys().next().copied();
            }
            true
        } else {
            false
        }
    }

    pub fn focus_window(&mut self, id: u64) -> bool {
        if self.windows.contains_key(&id) {
            // 取消之前的焦点
            if let Some(focused) = self.focused_window {
                if let Some(w) = self.windows.get_mut(&focused) {
                    w.set_focus(false);
                }
            }
            if let Some(w) = self.windows.get_mut(&id) {
                w.set_focus(true);
            }
            self.focused_window = Some(id);
            true
        } else {
            false
        }
    }

    pub fn minimize_window(&mut self, id: u64) -> bool {
        if let Some(window) = self.windows.get_mut(&id) {
            window.minimize();
            true
        } else {
            false
        }
    }

    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    pub fn visible_window_count(&self) -> usize {
        self.windows.values().filter(|w| w.state == WindowState::Visible).count()
    }
}

// =====================================================================================
// AI 原生 GUI 引擎 (GUAI) —— 统一入口
// =====================================================================================

/// AI 原生 GUI 引擎：整合语义窗口、AI 小部件、AI 图层、AI 事件、AI 合成器、AI 主题、
/// AI 布局、多模态输入、AI 窗口管理器。
pub struct GUAI {
    pub window_manager: AIWindowManager,
    pub widgets: BTreeMap<u64, AIGWidget>,
    pub compositor: AICompositor,
    pub theme: AITheme,
    pub layout: AILayout,
    pub multi_input: AIMultiInput,
    pub events: VecDeque<AIEvent>,
    pub next_event_id: u64,
    pub next_widget_id: u64,
    pub enabled: bool,
    pub total_operations: u64,
}

impl GUAI {
    pub fn new() -> Self {
        Self {
            window_manager: AIWindowManager::new(),
            widgets: BTreeMap::new(),
            compositor: AICompositor::new(),
            theme: AITheme::new("default"),
            layout: AILayout::new(LayoutType::Vertical),
            multi_input: AIMultiInput::new(),
            events: VecDeque::new(),
            next_event_id: 1,
            next_widget_id: 1,
            enabled: true,
            total_operations: 0,
        }
    }

    pub fn create_window(&mut self, title: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        self.window_manager.create_window(title, x, y, w, h)
    }

    pub fn close_window(&mut self, id: u64) -> bool {
        self.window_manager.close_window(id)
    }

    pub fn focus_window(&mut self, id: u64) -> bool {
        self.window_manager.focus_window(id)
    }

    pub fn add_widget(&mut self, r#type: WidgetType, label: &str, x: i32, y: i32, w: u32, h: u32, window_id: Option<u64>) -> u64 {
        let id = self.next_widget_id;
        self.next_widget_id += 1;
        let mut widget = AIGWidget::new(id, r#type, label, x, y, w, h);
        widget.window_id = window_id;
        self.widgets.insert(id, widget);
        self.total_operations += 1;
        id
    }

    pub fn add_layer(&mut self, name: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        self.compositor.add_layer(name, x, y, w, h)
    }

    pub fn compose_frame(&mut self, now_ns: u128) {
        self.compositor.compose(now_ns);
        self.total_operations += 1;
    }

    pub fn inject_event(&mut self, r#type: InputEventType, x: i32, y: i32, data: &str) -> u64 {
        let id = self.next_event_id;
        self.next_event_id += 1;
        let event = AIEvent::new(id, r#type, x, y, data);
        self.events.push_back(event);
        self.multi_input.record_event();
        self.total_operations += 1;
        id
    }

    pub fn poll_event(&mut self) -> Option<AIEvent> {
        self.events.pop_front()
    }

    pub fn set_theme_name(&mut self, name: &str) {
        self.theme.name = name.to_string();
    }

    pub fn enable_input_modality(&mut self, modality: InputModality) {
        self.multi_input.enable_modality(modality);
    }

    pub fn window_count(&self) -> usize {
        self.window_manager.window_count()
    }

    pub fn visible_window_count(&self) -> usize {
        self.window_manager.visible_window_count()
    }

    pub fn widget_count(&self) -> usize {
        self.widgets.len()
    }

    pub fn layer_count(&self) -> usize {
        self.compositor.layer_count()
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    pub fn fps(&self) -> f32 {
        self.compositor.fps
    }
}

impl Default for GUAI {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static GUI_AI: OnceLock<Mutex<GUAI>> = OnceLock::new();

pub fn gui_ai() -> &'static Mutex<GUAI> {
    GUI_AI.get_or_init(|| Mutex::new(GUAI::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = gui_ai();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = gui_ai().lock().unwrap();
    format!(
        "gui_ai: windows={} visible_windows={} widgets={} layers={} events={} fps={:.1} input_modalities={} theme={} operations={}",
        eng.window_count(),
        eng.visible_window_count(),
        eng.widget_count(),
        eng.layer_count(),
        eng.event_count(),
        eng.fps(),
        eng.multi_input.modality_count(),
        eng.theme.name,
        eng.total_operations
    )
}

pub fn gui_ai_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn gui_ai_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create-window") => {
            let title = it.next().ok_or("用法: gui_ai:// create-window <标题> <x> <y> <w> <h>")?;
            let x = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(100);
            let y = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(100);
            let w = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(800);
            let h = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(600);
            let mut eng = gui_ai().lock().unwrap();
            let id = eng.create_window(title, x, y, w, h);
            Ok(format!("已创建窗口 id={} 标题={}", id, title))
        }
        Some("close-window") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少窗口id")?;
            let mut eng = gui_ai().lock().unwrap();
            if eng.close_window(id) {
                Ok(format!("已关闭窗口 id={}", id))
            } else {
                Err(format!("窗口 id={} 不存在", id))
            }
        }
        Some("focus-window") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少窗口id")?;
            let mut eng = gui_ai().lock().unwrap();
            if eng.focus_window(id) {
                Ok(format!("已聚焦窗口 id={}", id))
            } else {
                Err(format!("窗口 id={} 不存在", id))
            }
        }
        Some("add-widget") => {
            let type_str = it.next().unwrap_or("button");
            let r#type = match type_str {
                "button" => WidgetType::Button,
                "label" => WidgetType::Label,
                "text" => WidgetType::TextInput,
                "checkbox" => WidgetType::Checkbox,
                "slider" => WidgetType::Slider,
                "progress" => WidgetType::Progress,
                "image" => WidgetType::Image,
                "canvas" => WidgetType::Canvas,
                "menu" => WidgetType::Menu,
                _ => WidgetType::Button,
            };
            let label = it.next().unwrap_or("widget");
            let x = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let y = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let w = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(100);
            let h = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(30);
            let window_id = it.next().and_then(|s| s.parse::<u64>().ok());
            let mut eng = gui_ai().lock().unwrap();
            let id = eng.add_widget(r#type, label, x, y, w, h, window_id);
            Ok(format!("已添加小部件 id={} 类型={:?} 标签={}", id, r#type, label))
        }
        Some("add-layer") => {
            let name = it.next().ok_or("缺少图层名")?;
            let x = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let y = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let w = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(800);
            let h = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(600);
            let mut eng = gui_ai().lock().unwrap();
            let id = eng.add_layer(name, x, y, w, h);
            Ok(format!("已添加图层 id={} 名称={}", id, name))
        }
        Some("compose") => {
            let now = it.next().and_then(|s| s.parse::<u128>().ok()).unwrap_or(0);
            let mut eng = gui_ai().lock().unwrap();
            eng.compose_frame(now);
            Ok(format!("已合成帧 now={} fps={:.1}", now, eng.fps()))
        }
        Some("inject-event") => {
            let type_str = it.next().unwrap_or("click");
            let r#type = match type_str {
                "key" => InputEventType::KeyPress,
                "mouse" => InputEventType::MouseButton,
                "touch" => InputEventType::Touch,
                "pen" => InputEventType::Pen,
                "gesture" => InputEventType::Gesture,
                "voice" => InputEventType::Voice,
                "gaze" => InputEventType::Gaze,
                _ => InputEventType::MouseMove,
            };
            let x = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let y = it.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            let data: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = gui_ai().lock().unwrap();
            let id = eng.inject_event(r#type, x, y, &data);
            Ok(format!("已注入事件 id={} 类型={:?} 坐标=({},{})", id, r#type, x, y))
        }
        Some("poll-event") => {
            let mut eng = gui_ai().lock().unwrap();
            match eng.poll_event() {
                Some(event) => Ok(format!("事件 id={} 类型={:?} 坐标=({},{}) 数据={}", event.id, event.r#type, event.x, event.y, event.data)),
                None => Ok("无事件".into()),
            }
        }
        Some("set-theme") => {
            let name = it.next().ok_or("缺少主题名")?;
            let mut eng = gui_ai().lock().unwrap();
            eng.set_theme_name(name);
            Ok(format!("已设置主题: {}", name))
        }
        Some("enable-modality") => {
            let modality_str = it.next().unwrap_or("touch");
            let modality = match modality_str {
                "touch" => InputModality::Touch,
                "pen" => InputModality::Pen,
                "voice" => InputModality::Voice,
                "gaze" => InputModality::Gaze,
                "keyboard" => InputModality::Keyboard,
                "mouse" => InputModality::Mouse,
                _ => InputModality::Touch,
            };
            let mut eng = gui_ai().lock().unwrap();
            eng.enable_input_modality(modality);
            Ok(format!("已启用输入模态: {:?}", modality))
        }
        Some("set-layout") => {
            let type_str = it.next().unwrap_or("vertical");
            let layout_type = match type_str {
                "vertical" => LayoutType::Vertical,
                "horizontal" => LayoutType::Horizontal,
                "grid" => LayoutType::Grid,
                "free" => LayoutType::Free,
                _ => LayoutType::Vertical,
            };
            let mut eng = gui_ai().lock().unwrap();
            eng.layout.r#type = layout_type;
            Ok(format!("已设置布局: {:?}", layout_type))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 gui_ai 动作: {}", other)),
    }
}

pub fn gui_ai_act_dispatch(payload: &str) -> Result<String, String> {
    gui_ai_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_creation() {
        let mut wm = AIWindowManager::new();
        let id = wm.create_window("main", 100, 100, 800, 600);
        assert_eq!(id, 1);
        assert_eq!(wm.window_count(), 1);
        assert_eq!(wm.visible_window_count(), 1);
        let window = wm.windows.get(&id).unwrap();
        assert_eq!(window.title, "main");
        assert_eq!(window.state, WindowState::Visible);
    }

    #[test]
    fn window_close() {
        let mut wm = AIWindowManager::new();
        let id = wm.create_window("main", 0, 0, 100, 100);
        assert!(wm.close_window(id));
        assert_eq!(wm.total_windows_closed, 1);
    }

    #[test]
    fn window_focus() {
        let mut wm = AIWindowManager::new();
        let id1 = wm.create_window("win1", 0, 0, 100, 100);
        let id2 = wm.create_window("win2", 0, 0, 100, 100);
        assert!(wm.focus_window(id2));
        assert_eq!(wm.focused_window, Some(id2));
    }

    #[test]
    fn window_minimize() {
        let mut wm = AIWindowManager::new();
        let id = wm.create_window("main", 0, 0, 100, 100);
        assert!(wm.minimize_window(id));
        assert_eq!(wm.windows.get(&id).unwrap().state, WindowState::Minimized);
        assert_eq!(wm.visible_window_count(), 0);
    }

    #[test]
    fn widget_creation() {
        let mut widget = AIGWidget::new(1, WidgetType::Button, "确定", 10, 10, 100, 30);
        assert_eq!(widget.label, "确定");
        assert!(widget.enabled);
        assert!(widget.visible);
    }

    #[test]
    fn widget_value_role() {
        let mut widget = AIGWidget::new(1, WidgetType::TextInput, "输入框", 0, 0, 200, 30);
        widget.set_value("hello");
        widget.set_role("input");
        assert_eq!(widget.value, "hello");
        assert_eq!(widget.semantic_role, Some("input".to_string()));
    }

    #[test]
    fn layer_opacity() {
        let mut layer = AILayer::new(1, "bg", 0, 0, 800, 600);
        layer.set_opacity(0.5);
        assert!((layer.opacity - 0.5).abs() < 1e-6);
        layer.hide();
        assert!(!layer.visible);
        layer.show();
        assert!(layer.visible);
    }

    #[test]
    fn compositor() {
        let mut comp = AICompositor::new();
        let id1 = comp.add_layer("bg", 0, 0, 800, 600);
        let id2 = comp.add_layer("fg", 0, 0, 400, 300);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(comp.layer_count(), 2);
        comp.compose(1_000_000_000);
        assert_eq!(comp.frame_count, 1);
    }

    #[test]
    fn theme() {
        let mut theme = AITheme::new("dark");
        theme.set_colors(AIColor::black(), AIColor::white(), AIColor::blue());
        theme.set_animation(false);
        assert!(!theme.animation_enabled);
        assert_eq!(theme.name, "dark");
    }

    #[test]
    fn layout() {
        let mut layout = AILayout::new(LayoutType::Horizontal);
        layout.set_spacing(16);
        layout.set_margin(32);
        assert_eq!(layout.spacing, 16);
        assert_eq!(layout.margin, 32);
    }

    #[test]
    fn multi_input() {
        let mut input = AIMultiInput::new();
        input.enable_modality(InputModality::Touch);
        input.enable_modality(InputModality::Voice);
        assert_eq!(input.modality_count(), 2);
        input.record_event();
        assert_eq!(input.event_count, 1);
    }

    #[test]
    fn event_creation() {
        let event = AIEvent::new(1, InputEventType::Touch, 100, 200, "tap");
        assert_eq!(event.r#type, InputEventType::Touch);
        assert_eq!(event.x, 100);
        assert_eq!(event.y, 200);
        assert_eq!(event.data, "tap");
        assert!(!event.handled);
    }

    #[test]
    fn gui_engine_full() {
        let mut eng = GUAI::new();
        let win_id = eng.create_window("main", 0, 0, 800, 600);
        assert_eq!(win_id, 1);
        let widget_id = eng.add_widget(WidgetType::Button, "btn", 10, 10, 80, 30, Some(win_id));
        assert_eq!(widget_id, 1);
        let layer_id = eng.add_layer("bg", 0, 0, 800, 600);
        assert_eq!(layer_id, 1);
        eng.inject_event(InputEventType::MouseButton, 100, 100, "click");
        let event = eng.poll_event().unwrap();
        assert_eq!(event.data, "click");
        eng.compose_frame(1_000_000_000);
        assert!(eng.fps() >= 0.0);
    }

    #[test]
    fn gui_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("gui_ai:"));
        assert!(s.contains("windows="));
    }

    #[test]
    fn gui_ai_act_create_window() {
        init();
        let res = gui_ai_act("create-window 主窗口 100 100 800 600").unwrap();
        assert!(res.contains("已创建窗口"));
    }

    #[test]
    fn gui_ai_act_close_window() {
        init();
        gui_ai_act("create-window 主窗口 100 100 800 600").unwrap();
        let res = gui_ai_act("close-window 1").unwrap();
        assert!(res.contains("已关闭窗口"));
    }

    #[test]
    fn gui_ai_act_focus_window() {
        init();
        gui_ai_act("create-window 主窗口 100 100 800 600").unwrap();
        let res = gui_ai_act("focus-window 1").unwrap();
        assert!(res.contains("已聚焦窗口"));
    }

    #[test]
    fn gui_ai_act_add_widget() {
        init();
        let res = gui_ai_act("add-widget button 确定 10 10 80 30").unwrap();
        assert!(res.contains("已添加小部件"));
    }

    #[test]
    fn gui_ai_act_add_layer() {
        init();
        let res = gui_ai_act("add-layer bg 0 0 800 600").unwrap();
        assert!(res.contains("已添加图层"));
    }

    #[test]
    fn gui_ai_act_compose() {
        init();
        let res = gui_ai_act("compose 1000000000").unwrap();
        assert!(res.contains("已合成帧"));
    }

    #[test]
    fn gui_ai_act_inject_event() {
        init();
        let res = gui_ai_act("inject-event touch 100 200 点击").unwrap();
        assert!(res.contains("已注入事件"));
    }

    #[test]
    fn gui_ai_act_poll_event() {
        init();
        gui_ai_act("inject-event touch 100 200 点击").unwrap();
        let res = gui_ai_act("poll-event").unwrap();
        assert!(res.contains("事件"));
    }

    #[test]
    fn gui_ai_act_theme() {
        init();
        let res = gui_ai_act("set-theme dark").unwrap();
        assert!(res.contains("已设置主题"));
    }

    #[test]
    fn gui_ai_act_modality() {
        init();
        let res = gui_ai_act("enable-modality voice").unwrap();
        assert!(res.contains("已启用输入模态"));
    }

    #[test]
    fn gui_ai_act_layout() {
        init();
        let res = gui_ai_act("set-layout horizontal").unwrap();
        assert!(res.contains("已设置布局"));
    }

    #[test]
    fn gui_ai_act_status() {
        init();
        let res = gui_ai_act("status").unwrap();
        assert!(res.contains("gui_ai:"));
    }

    #[test]
    fn window_states() {
        assert_eq!(WindowState::Hidden as u32, 0);
        assert_eq!(WindowState::Closed as u32, 4);
    }

    #[test]
    fn widget_types() {
        assert_eq!(WidgetType::Button as u32, 0);
        assert_eq!(WidgetType::Menu as u32, 9);
    }

    #[test]
    fn input_event_types() {
        assert_eq!(InputEventType::KeyPress as u32, 0);
        assert_eq!(InputEventType::Gaze as u32, 8);
    }

    #[test]
    fn input_modalities() {
        assert_eq!(InputModality::Touch as u32, 0);
        assert_eq!(InputModality::Mouse as u32, 5);
    }

    #[test]
    fn layout_types() {
        assert_eq!(LayoutType::Vertical as u32, 0);
        assert_eq!(LayoutType::Free as u32, 3);
    }
}
