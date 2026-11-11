//! GUI 控件库 —— 声明式控件 + 像素级渲染 + 焦点/事件分发。
//!
//! 提供 Button / Label / TextEntry / Checkbox / RadioButton / ComboBox /
//! Slider / ProgressBar / ScrollBar / Panel / Tab 十一种控件，以及
//! `WidgetManager` 负责布局（绝对 / 流式 / 网格）、焦点遍历（Tab 键）
//! 和输入事件分发（鼠标 / 键盘 → 控件）。
//!
//! 控件状态：正常 / 悬停 / 按下 / 禁用 / 聚焦，每种状态对应一套颜色方案。
//! 所有坐标为像素，渲染直接写入 32bpp `Surface`，与合成器共享同一份像素。

use super::surface::{argb, palette, Rect, Surface};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 控件状态
// ============================================================================

/// 控件交互状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WidgetState {
    /// 默认正常状态。
    #[default]
    Normal,
    /// 鼠标悬停。
    Hover,
    /// 鼠标按下。
    Pressed,
    /// 禁用（不可交互）。
    Disabled,
    /// 获得焦点。
    Focused,
}

impl WidgetState {
    pub fn label(self) -> &'static str {
        match self {
            WidgetState::Normal => "normal",
            WidgetState::Hover => "hover",
            WidgetState::Pressed => "pressed",
            WidgetState::Disabled => "disabled",
            WidgetState::Focused => "focused",
        }
    }
}

/// 控件种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    /// 可点击的按钮，带 `on_click` 回调。
    Button,
    /// 静态文本标签，不可编辑。
    Label,
    /// 可编辑单行文本框。
    TextEntry,
    /// 复选框，带 on/off 状态。
    Checkbox,
    /// 单选按钮，同组内互斥。
    RadioButton,
    /// 下拉选择框。
    ComboBox,
    /// 数值滑块。
    Slider,
    /// 进度条（0-100）。
    ProgressBar,
    /// 滚动条。
    ScrollBar,
    /// 容器面板，可包含子控件。
    Panel,
    /// 选项卡，每个 tab 关联一个内容面板。
    Tab,
}

impl WidgetKind {
    pub fn name(self) -> &'static str {
        match self {
            WidgetKind::Button => "button",
            WidgetKind::Label => "label",
            WidgetKind::TextEntry => "text_entry",
            WidgetKind::Checkbox => "checkbox",
            WidgetKind::RadioButton => "radio_button",
            WidgetKind::ComboBox => "combo_box",
            WidgetKind::Slider => "slider",
            WidgetKind::ProgressBar => "progress_bar",
            WidgetKind::ScrollBar => "scroll_bar",
            WidgetKind::Panel => "panel",
            WidgetKind::Tab => "tab",
        }
    }
}

// ============================================================================
// 颜色方案
// ============================================================================

/// 单种控件在各状态下的颜色。
#[derive(Clone, Debug, Default)]
pub struct WidgetColors {
    pub bg_normal: u32,
    pub bg_hover: u32,
    pub bg_pressed: u32,
    pub bg_disabled: u32,
    pub bg_focused: u32,
    pub fg_normal: u32,
    pub fg_hover: u32,
    pub fg_pressed: u32,
    pub fg_disabled: u32,
    pub fg_focused: u32,
    pub border_normal: u32,
    pub border_hover: u32,
    pub border_pressed: u32,
    pub border_disabled: u32,
    pub border_focused: u32,
}

impl WidgetColors {
    /// 默认 Catppuccin Mocha 风格配色。
    pub fn default_theme() -> Self {
        Self {
            bg_normal: palette::SURFACE0,
            bg_hover: palette::SURFACE1,
            bg_pressed: palette::OVERLAY0,
            bg_disabled: palette::BASE,
            bg_focused: palette::BLUE,
            fg_normal: palette::TEXT,
            fg_hover: palette::TEXT,
            fg_pressed: palette::TEXT,
            fg_disabled: palette::SURFACE1,
            fg_focused: palette::BASE,
            border_normal: palette::OVERLAY0,
            border_hover: palette::BLUE,
            border_pressed: palette::MAUVE,
            border_disabled: palette::SURFACE0,
            border_focused: palette::BLUE,
        }
    }

    /// 高对比度配色。
    pub fn high_contrast() -> Self {
        Self {
            bg_normal: palette::CRUST,
            bg_hover: palette::SURFACE0,
            bg_pressed: palette::OVERLAY0,
            bg_disabled: palette::BASE,
            bg_focused: palette::YELLOW,
            fg_normal: palette::TEXT,
            fg_hover: palette::TEXT,
            fg_pressed: palette::TEXT,
            fg_disabled: palette::SURFACE1,
            fg_focused: palette::BASE,
            border_normal: palette::TEXT,
            border_hover: palette::TEXT,
            border_pressed: palette::TEXT,
            border_disabled: palette::SURFACE1,
            border_focused: palette::YELLOW,
        }
    }

    /// 赛博朋克配色。
    pub fn cyberpunk() -> Self {
        Self {
            bg_normal: argb(255, 0x0a, 0x0a, 0x12),
            bg_hover: argb(255, 0x1a, 0x1a, 0x2a),
            bg_pressed: argb(255, 0x2a, 0x2a, 0x4a),
            bg_disabled: argb(255, 0x05, 0x05, 0x0a),
            bg_focused: argb(255, 0x00, 0xff, 0x41),
            fg_normal: argb(255, 0x00, 0xff, 0x41),
            fg_hover: argb(255, 0xcc, 0xff, 0xcc),
            fg_pressed: argb(255, 0xcc, 0xff, 0xcc),
            fg_disabled: argb(255, 0x44, 0x44, 0x44),
            fg_focused: argb(255, 0x05, 0x05, 0x0a),
            border_normal: argb(255, 0x00, 0xff, 0x41),
            border_hover: argb(255, 0x33, 0xff, 0x66),
            border_pressed: argb(255, 0x66, 0xff, 0x99),
            border_disabled: argb(255, 0x22, 0x22, 0x33),
            border_focused: argb(255, 0xff, 0x00, 0xff),
        }
    }
}

/// 根据状态取当前生效颜色。
fn state_color(colors: &WidgetColors, state: WidgetState) -> (u32, u32, u32) {
    match state {
        WidgetState::Normal => (colors.bg_normal, colors.fg_normal, colors.border_normal),
        WidgetState::Hover => (colors.bg_hover, colors.fg_hover, colors.border_hover),
        WidgetState::Pressed => (colors.bg_pressed, colors.fg_pressed, colors.border_pressed),
        WidgetState::Disabled => (
            colors.bg_disabled,
            colors.fg_disabled,
            colors.border_disabled,
        ),
        WidgetState::Focused => (colors.bg_focused, colors.fg_focused, colors.border_focused),
    }
}

// ============================================================================
// 控件核心数据结构
// ============================================================================

/// 控件唯一标识。
pub type WidgetId = u64;

/// 单个控件。
pub struct Widget {
    pub id: WidgetId,
    pub kind: WidgetKind,
    pub parent_id: Option<WidgetId>,
    pub rect: Rect,
    /// 显示文本（按钮标签、输入内容等）。
    pub text: String,
    pub state: WidgetState,
    pub enabled: bool,
    pub visible: bool,
    /// 是否可获得焦点。
    pub focusable: bool,
    /// 当前是否有焦点。
    pub focused: bool,
    /// 复选框/单选按钮是否选中。
    pub checked: bool,
    /// 数值控件的当前值（0.0-1.0）。
    pub value: f64,
    /// 数值控件的最小值。
    pub min: f64,
    /// 数值控件的最大值。
    pub max: f64,
    /// 单选按钮分组 ID。
    pub radio_group: Option<u64>,
    /// 下拉框当前选中索引。
    pub combo_selected: usize,
    /// 下拉框选项列表。
    pub combo_items: Vec<String>,
    /// 进度值（0-100）。
    pub progress: u32,
    /// 颜色方案（None 使用全局默认）。
    pub colors: Option<WidgetColors>,
    /// 点击回调（Box<dyn Fn()>，不捕获上下文以保持 Send + Sync）。
    pub on_click: Option<Box<dyn Fn() + Send + Sync>>,
    /// 文本变化回调。
    pub on_change: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// 光标位置（用于文本编辑）。
    pub cursor_pos: usize,
    /// 滚动条滚动偏移。
    pub scroll_offset: f64,
}

impl Widget {
    /// 创建一个新控件。
    pub fn new(kind: WidgetKind, parent_id: Option<WidgetId>, rect: Rect) -> Self {
        let (min, max, value) = match kind {
            WidgetKind::Slider | WidgetKind::ProgressBar | WidgetKind::ScrollBar => (0.0, 100.0, 0.0),
            _ => (0.0, 1.0, 0.0),
        };
        Self {
            id: 0, // 由 WidgetManager 分配
            kind,
            parent_id,
            rect,
            text: String::new(),
            state: WidgetState::Normal,
            enabled: true,
            visible: true,
            focusable: matches!(
                kind,
                WidgetKind::Button | WidgetKind::TextEntry | WidgetKind::Checkbox
                    | WidgetKind::RadioButton | WidgetKind::ComboBox | WidgetKind::Slider
            ),
            focused: false,
            checked: false,
            value,
            min,
            max,
            radio_group: None,
            combo_selected: 0,
            combo_items: Vec::new(),
            progress: 0,
            colors: None,
            on_click: None,
            on_change: None,
            cursor_pos: 0,
            scroll_offset: 0.0,
        }
    }

    /// 设置控件颜色方案。
    pub fn set_colors(&mut self, colors: WidgetColors) {
        self.colors = Some(colors);
    }

    /// 返回当前生效的颜色方案（优先使用控件自带，否则全局默认）。
    pub fn effective_colors<'a>(&'a self, global: &'a WidgetColors) -> &'a WidgetColors {
        self.colors.as_ref().unwrap_or(global)
    }

    /// 根据状态返回当前（背景、前景、边框）颜色。
    pub fn current_colors(&self, global: &WidgetColors) -> (u32, u32, u32) {
        let state = if !self.enabled {
            WidgetState::Disabled
        } else if self.focused {
            WidgetState::Focused
        } else {
            self.state
        };
        state_color(self.effective_colors(global), state)
    }

    /// 命中测试：坐标是否在控件矩形内。
    pub fn hit_test(&self, x: i32, y: i32) -> bool {
        self.visible && self.enabled && self.rect.contains(x, y)
    }

    /// 设置显示文本。
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor_pos = self.text.chars().count();
    }

    /// 追加文本（用于 TextEntry 输入）。
    pub fn insert_char(&mut self, ch: char) {
        let pos = self.cursor_pos.min(self.text.chars().count());
        let mut chars: Vec<char> = self.text.chars().collect();
        chars.insert(pos, ch);
        self.text = chars.into_iter().collect();
        self.cursor_pos += 1;
    }

    /// 退格删除。
    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let mut chars: Vec<char> = self.text.chars().collect();
        chars.remove(self.cursor_pos - 1);
        self.text = chars.into_iter().collect();
        self.cursor_pos -= 1;
    }

    /// 设置光标位置。
    pub fn set_cursor_pos(&mut self, pos: usize) {
        self.cursor_pos = pos.min(self.text.chars().count());
    }

    /// 光标左移。
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    /// 光标右移。
    pub fn cursor_right(&mut self) {
        if self.cursor_pos < self.text.chars().count() {
            self.cursor_pos += 1;
        }
    }

    /// 渲染控件到表面。
    pub fn render(&self, surface: &mut Surface, global: &WidgetColors) {
        if !self.visible {
            return;
        }
        let (bg, fg, border) = self.current_colors(global);
        let r = self.rect;
        let x = r.x;
        let y = r.y;
        let w = r.w;
        let h = r.h;

        // 绘制背景。
        surface.fill_rect(x, y, w, h, bg);
        // 绘制边框。
        surface.draw_rect(x, y, w, h, border, 1);

        match self.kind {
            WidgetKind::Button => {
                // 按钮文本居中。
                let tx = x + 8;
                let ty = y + (h as i32 / 2) - 4;
                surface.draw_text(tx, ty, &self.text, fg, 2);
            }
            WidgetKind::Label => {
                surface.draw_text(x + 4, y + 2, &self.text, fg, 2);
            }
            WidgetKind::TextEntry => {
                // 绘制文本，带光标。
                let tx = x + 4;
                let ty = y + (h as i32 / 2) - 4;
                surface.draw_text(tx, ty, &self.text, fg, 2);
                if self.focused {
                    // 光标：在光标位置画一条竖线。
                    let char_w = 8 * 2; // 近似字符宽度
                    let cx = tx + self.cursor_pos as i32 * char_w;
                    surface.draw_line(cx, y + 4, cx, y + h as i32 - 4, fg);
                }
            }
            WidgetKind::Checkbox => {
                // 复选框：左侧方框，右侧文本。
                let box_size = 16;
                let bx = x + 4;
                let by = y + (h as i32 / 2) - (box_size as i32 / 2);
                surface.draw_rect(bx, by, box_size, box_size, border, 2);
                if self.checked {
                    surface.fill_rect(bx + 2, by + 2, box_size - 4, box_size - 4, fg);
                }
                surface.draw_text(bx + box_size as i32 + 6, by - 2, &self.text, fg, 2);
            }
            WidgetKind::RadioButton => {
                // 单选按钮：圆形。
                let radius = 8;
                let cx = x + 4 + radius;
                let cy = y + (h as i32 / 2);
                surface.draw_circle(cx, cy, radius, border, 1);
                if self.checked {
                    surface.fill_circle(cx, cy, radius - 2, fg);
                }
                surface.draw_text(x + 4 + radius * 2 + 4, cy - 6, &self.text, fg, 2);
            }
            WidgetKind::ComboBox => {
                // 下拉框：矩形 + 下拉箭头。
                surface.draw_text(x + 4, y + (h as i32 / 2) - 4, &self.text, fg, 2);
                // 箭头（简单画两条线）。
                let ax = x + w as i32 - 12;
                let ay = y + (h as i32 / 2) - 2;
                surface.draw_line(ax, ay, ax + 6, ay + 6, border);
                surface.draw_line(ax + 6, ay + 6, ax + 12, ay, border);
            }
            WidgetKind::Slider => {
                // 滑块：轨道 + 拇指。
                let track_y = y + (h as i32 / 2);
                let track_w = w.saturating_sub(20);
                surface.draw_rect(x + 10, track_y - 2, track_w, 4, border, 1);
                let t = if self.max > self.min {
                    (self.value - self.min) / (self.max - self.min)
                } else {
                    0.0
                };
                let thumb_x = x + 10 + (track_w as f64 * t) as i32;
                surface.fill_rect(thumb_x - 4, track_y - 6, 8, 12, fg);
            }
            WidgetKind::ProgressBar => {
                // 进度条：底轨 + 填充。
                let track_y = y + (h as i32 / 2) - 4;
                surface.draw_rect(x, track_y, w, 8, border, 1);
                let fill_w = (w as f64 * self.progress as f64 / 100.0) as u32;
                if fill_w > 0 {
                    surface.fill_rect(x, track_y, fill_w, 8, fg);
                }
            }
            WidgetKind::ScrollBar => {
                // 滚动条：轨道 + 滑块。
                let track_y = y;
                surface.draw_rect(x, track_y, w, h, border, 1);
                let thumb_h = (h as f64 * 0.3) as u32;
                let thumb_y = y + (self.scroll_offset as i32).min(h.saturating_sub(thumb_h) as i32);
                surface.fill_rect(x, thumb_y, w, thumb_h, fg);
            }
            WidgetKind::Panel => {
                // 面板不绘制额外内容，子控件自行渲染。
            }
            WidgetKind::Tab => {
                // 选项卡标签。
                surface.draw_text(x + 8, y + 4, &self.text, fg, 2);
                if self.focused {
                    // 选中标签下划线。
                    surface.draw_line(x, y + h as i32 - 2, x + w as i32, y + h as i32 - 2, fg);
                }
            }
        }
    }
}

// ============================================================================
// 布局模式
// ============================================================================

/// 布局策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutKind {
    /// 绝对定位，使用控件各自的 rect。
    #[default]
    Absolute,
    /// 流式布局，从左到右自动换行。
    Flow,
    /// 网格布局，固定列数。
    Grid,
}

/// 布局配置。
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub kind: LayoutKind,
    /// 网格列数（Grid 模式有效）。
    pub columns: usize,
    /// 控件间距。
    pub spacing: u32,
    /// 外边距。
    pub margin: u32,
    /// 起始位置。
    pub origin: (i32, i32),
}

impl Layout {
    pub fn absolute() -> Self {
        Self {
            kind: LayoutKind::Absolute,
            ..Default::default()
        }
    }

    pub fn flow(spacing: u32) -> Self {
        Self {
            kind: LayoutKind::Flow,
            spacing,
            ..Default::default()
        }
    }

    pub fn grid(columns: usize, spacing: u32) -> Self {
        Self {
            kind: LayoutKind::Grid,
            columns,
            spacing,
            ..Default::default()
        }
    }
}

// ============================================================================
// 控件管理器
// ============================================================================

/// 全局控件 ID 分配器。
static WIDGET_ID_COUNTER: OnceLock<Mutex<u64>> = OnceLock::new();

fn next_widget_id() -> WidgetId {
    let mut counter = WIDGET_ID_COUNTER.get_or_init(|| Mutex::new(0)).lock().unwrap();
    *counter += 1;
    *counter
}

/// 控件管理器：持有全部控件、执行布局、路由输入事件、管理焦点。
pub struct WidgetManager {
    /// 全局默认颜色方案。
    pub colors: WidgetColors,
    /// 控件注册表，按 id 排序便于确定性遍历。
    widgets: BTreeMap<WidgetId, Widget>,
    /// 布局配置。
    pub layout: Layout,
    /// 当前焦点控件 id。
    focused: Option<WidgetId>,
    /// 鼠标悬停控件 id。
    hover: Option<WidgetId>,
    /// 最近一次点击时间戳（用于双击检测）。
    last_click_ns: u64,
    /// 上一次点击的控件 id。
    last_click_id: Option<WidgetId>,
}

impl Default for WidgetManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetManager {
    /// 创建空的控件管理器。
    pub fn new() -> Self {
        Self {
            colors: WidgetColors::default_theme(),
            widgets: BTreeMap::new(),
            layout: Layout::absolute(),
            focused: None,
            hover: None,
            last_click_ns: 0,
            last_click_id: None,
        }
    }

    /// 设置全局颜色方案。
    pub fn set_colors(&mut self, colors: WidgetColors) {
        self.colors = colors;
    }

    /// 设置布局模式。
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
    }

    /// 注册一个控件，自动分配 id。
    pub fn add(&mut self, mut widget: Widget) -> WidgetId {
        let id = next_widget_id();
        widget.id = id;
        self.widgets.insert(id, widget);
        id
    }

    /// 移除控件。
    pub fn remove(&mut self, id: WidgetId) -> bool {
        if self.focused == Some(id) {
            self.focused = None;
        }
        if self.hover == Some(id) {
            self.hover = None;
        }
        self.widgets.remove(&id).is_some()
    }

    /// 获取控件引用。
    pub fn get(&self, id: WidgetId) -> Option<&Widget> {
        self.widgets.get(&id)
    }

    /// 获取可变控件引用。
    pub fn get_mut(&mut self, id: WidgetId) -> Option<&mut Widget> {
        self.widgets.get_mut(&id)
    }

    /// 设置控件文本。
    pub fn set_text(&mut self, id: WidgetId, text: &str) -> bool {
        if let Some(w) = self.widgets.get_mut(&id) {
            w.set_text(text);
            if let Some(cb) = w.on_change.as_ref() {
                cb(text);
            }
            true
        } else {
            false
        }
    }

    /// 设置启用/禁用。
    pub fn set_enabled(&mut self, id: WidgetId, enabled: bool) -> bool {
        self.widgets.get_mut(&id).map_or(false, |w| {
            w.enabled = enabled;
            true
        })
    }

    /// 设置显示/隐藏。
    pub fn set_visible(&mut self, id: WidgetId, visible: bool) -> bool {
        self.widgets.get_mut(&id).map_or(false, |w| {
            w.visible = visible;
            true
        })
    }

    /// 注册点击回调。
    pub fn on_click<F>(&mut self, id: WidgetId, handler: F) -> bool
    where
        F: Fn() + Send + Sync + 'static,
    {
        if let Some(w) = self.widgets.get_mut(&id) {
            w.on_click = Some(Box::new(handler));
            true
        } else {
            false
        }
    }

    /// 注册文本变化回调。
    pub fn on_change<F>(&mut self, id: WidgetId, handler: F) -> bool
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        if let Some(w) = self.widgets.get_mut(&id) {
            w.on_change = Some(Box::new(handler));
            return true;
        }
        false
    }

    /// 为控件聚焦。
    pub fn focus(&mut self, id: WidgetId) -> bool {
        if self.widgets.get(&id).map_or(false, |w| w.focusable && w.enabled) {
            // 取消旧焦点。
            if let Some(old) = self.focused {
                if let Some(w) = self.widgets.get_mut(&old) {
                    w.focused = false;
                }
            }
            self.focused = Some(id);
            if let Some(w) = self.widgets.get_mut(&id) {
                w.focused = true;
                w.state = WidgetState::Focused;
            }
            true
        } else {
            false
        }
    }

    /// 取消焦点。
    pub fn blur(&mut self, id: WidgetId) -> bool {
        if self.focused == Some(id) {
            if let Some(w) = self.widgets.get_mut(&id) {
                w.focused = false;
                w.state = WidgetState::Normal;
            }
            self.focused = None;
            true
        } else {
            false
        }
    }

    /// 返回当前焦点控件 id。
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    /// Tab 键焦点遍历（向前）。
    pub fn focus_next(&mut self) -> Option<WidgetId> {
        let focusable: Vec<WidgetId> = self
            .widgets
            .iter()
            .filter(|(_, w)| w.focusable && w.enabled && w.visible)
            .map(|(id, _)| *id)
            .collect();
        if focusable.is_empty() {
            return None;
        }
        let idx = self
            .focused
            .and_then(|id| focusable.iter().position(|&x| x == id))
            .unwrap_or(usize::MAX);
        let next = (idx + 1) % focusable.len();
        let id = focusable[next];
        self.focus(id);
        Some(id)
    }

    /// Shift+Tab 焦点遍历（向后）。
    pub fn focus_prev(&mut self) -> Option<WidgetId> {
        let focusable: Vec<WidgetId> = self
            .widgets
            .iter()
            .filter(|(_, w)| w.focusable && w.enabled && w.visible)
            .map(|(id, _)| *id)
            .collect();
        if focusable.is_empty() {
            return None;
        }
        let idx = self
            .focused
            .and_then(|id| focusable.iter().position(|&x| x == id))
            .unwrap_or(0);
        let prev = if idx == 0 {
            focusable.len() - 1
        } else {
            idx - 1
        };
        let id = focusable[prev];
        self.focus(id);
        Some(id)
    }

    /// 鼠标移动命中测试，更新 hover 状态。
    pub fn mouse_move(&mut self, x: i32, y: i32) -> Option<WidgetId> {
        // 从上层（插入顺序逆向）查找第一个可见且命中的控件。
        let hit: Option<WidgetId> = self
            .widgets
            .iter()
            .rev()
            .find(|(_, w)| w.hit_test(x, y))
            .map(|(id, _)| *id);
        if hit != self.hover {
            // 取消旧 hover。
            if let Some(old) = self.hover {
                if let Some(w) = self.widgets.get_mut(&old) {
                    if w.state == WidgetState::Hover {
                        w.state = WidgetState::Normal;
                    }
                }
            }
            self.hover = hit;
            if let Some(id) = hit {
                if let Some(w) = self.widgets.get_mut(&id) {
                    if w.state != WidgetState::Pressed && w.state != WidgetState::Focused {
                        w.state = WidgetState::Hover;
                    }
                }
            }
        }
        hit
    }

    /// 鼠标按下。
    pub fn mouse_down(&mut self, x: i32, y: i32) -> Option<WidgetId> {
        let hit: Option<WidgetId> = self
            .widgets
            .iter()
            .rev()
            .find(|(_, w)| w.hit_test(x, y))
            .map(|(id, _)| *id);
        if let Some(id) = hit {
            if let Some(w) = self.widgets.get_mut(&id) {
                w.state = WidgetState::Pressed;
                if w.focusable {
                    let _ = self.focus(id);
                }
            }
            // 复选框翻转。
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.kind == WidgetKind::Checkbox {
                    w.checked = !w.checked;
                    if let Some(cb) = w.on_change.as_ref() {
                        cb(if w.checked { "1" } else { "0" });
                    }
                }
                if w.kind == WidgetKind::RadioButton {
                    // 同组单选按钮互斥：先取出 group，释放对 w 的借用，再遍历全部控件。
                    let group = w.radio_group;
                    w.checked = true;
                    if let Some(g) = group {
                        for (_, other) in self.widgets.iter_mut() {
                            if other.kind == WidgetKind::RadioButton
                                && other.radio_group == Some(g)
                                && other.id != id
                            {
                                other.checked = false;
                            }
                        }
                    }
                }
            }
        }
        hit
    }

    /// 鼠标抬起，触发 on_click。
    pub fn mouse_up(&mut self, x: i32, y: i32) -> Option<WidgetId> {
        let hit: Option<WidgetId> = self
            .widgets
            .iter()
            .rev()
            .find(|(_, w)| w.hit_test(x, y))
            .map(|(id, _)| *id);
        if let Some(id) = hit {
            if let Some(w) = self.widgets.get(&id) {
                if w.state == WidgetState::Pressed {
                    if let Some(cb) = w.on_click.as_ref() {
                        cb();
                    }
                }
            }
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.state == WidgetState::Pressed {
                    w.state = WidgetState::Normal;
                }
            }
        }
        hit
    }

    /// 键盘输入：向焦点控件发送字符。
    pub fn key_char(&mut self, ch: char) -> bool {
        if let Some(id) = self.focused {
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.kind == WidgetKind::TextEntry {
                    w.insert_char(ch);
                    if let Some(cb) = w.on_change.as_ref() {
                        cb(&w.text);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// 键盘退格。
    pub fn key_backspace(&mut self) -> bool {
        if let Some(id) = self.focused {
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.kind == WidgetKind::TextEntry {
                    w.backspace();
                    if let Some(cb) = w.on_change.as_ref() {
                        cb(&w.text);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// 键盘光标左移。
    pub fn key_cursor_left(&mut self) -> bool {
        if let Some(id) = self.focused {
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.kind == WidgetKind::TextEntry {
                    w.cursor_left();
                    return true;
                }
            }
        }
        false
    }

    /// 键盘光标右移。
    pub fn key_cursor_right(&mut self) -> bool {
        if let Some(id) = self.focused {
            if let Some(w) = self.widgets.get_mut(&id) {
                if w.kind == WidgetKind::TextEntry {
                    w.cursor_right();
                    return true;
                }
            }
        }
        false
    }

    /// 渲染全部控件到表面。
    pub fn render(&self, surface: &mut Surface) {
        for (_, widget) in self.widgets.iter() {
            widget.render(surface, &self.colors);
        }
    }

    /// 按当前布局模式重新排列全部控件。
    pub fn layout_widgets(&mut self, container: Rect) {
        match self.layout.kind {
            LayoutKind::Absolute => {}
            LayoutKind::Flow => {
                let mut cx = container.x + self.layout.margin as i32;
                let mut cy = container.y + self.layout.margin as i32;
                let mut row_h = 0u32;
                for (_, w) in self.widgets.iter_mut() {
                    if !w.visible {
                        continue;
                    }
                    let tw = w.rect.w;
                    let th = w.rect.h;
                    if cx + tw as i32 > container.right() && cx > container.x {
                        cx = container.x + self.layout.margin as i32;
                        cy += (row_h + self.layout.spacing) as i32;
                        row_h = 0;
                    }
                    w.rect.x = cx;
                    w.rect.y = cy;
                    cx += tw as i32 + self.layout.spacing as i32;
                    row_h = row_h.max(th);
                }
            }
            LayoutKind::Grid => {
                let cols = self.layout.columns.max(1);
                let mut cx = container.x + self.layout.margin as i32;
                let mut cy = container.y + self.layout.margin as i32;
                let mut col = 0usize;
                let mut row_h = 0u32;
                for (_, w) in self.widgets.iter_mut() {
                    if !w.visible {
                        continue;
                    }
                    let tw = w.rect.w;
                    let th = w.rect.h;
                    w.rect.x = cx;
                    w.rect.y = cy;
                    cx += tw as i32 + self.layout.spacing as i32;
                    row_h = row_h.max(th);
                    col += 1;
                    if col >= cols {
                        cx = container.x + self.layout.margin as i32;
                        cy += (row_h + self.layout.spacing) as i32;
                        row_h = 0;
                        col = 0;
                    }
                }
            }
        }
    }

    /// 返回控件总数。
    pub fn len(&self) -> usize {
        self.widgets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.widgets.is_empty()
    }

    /// 迭代全部控件（id 升序）。
    pub fn iter(&self) -> impl Iterator<Item = (WidgetId, &Widget)> {
        self.widgets.iter().map(|(id, w)| (*id, w))
    }
}

// ============================================================================
// 内置控件工厂
// ============================================================================

impl WidgetManager {
    /// 添加按钮。
    pub fn add_button(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        text: &str,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Button, parent_id, rect);
        w.set_text(text);
        self.add(w)
    }

    /// 添加标签。
    pub fn add_label(&mut self, parent_id: Option<WidgetId>, rect: Rect, text: &str) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Label, parent_id, rect);
        w.set_text(text);
        w.focusable = false;
        self.add(w)
    }

    /// 添加文本输入框。
    pub fn add_text_entry(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        text: &str,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::TextEntry, parent_id, rect);
        w.set_text(text);
        self.add(w)
    }

    /// 添加复选框。
    pub fn add_checkbox(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        text: &str,
        checked: bool,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Checkbox, parent_id, rect);
        w.set_text(text);
        w.checked = checked;
        self.add(w)
    }

    /// 添加单选按钮。
    pub fn add_radio_button(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        text: &str,
        group: u64,
        checked: bool,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::RadioButton, parent_id, rect);
        w.set_text(text);
        w.radio_group = Some(group);
        w.checked = checked;
        self.add(w)
    }

    /// 添加下拉框。
    pub fn add_combo_box(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        items: &[String],
        selected: usize,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::ComboBox, parent_id, rect);
        w.combo_items = items.to_vec();
        w.combo_selected = selected.min(items.len().saturating_sub(1));
        if let Some(label) = items.get(w.combo_selected) {
            w.set_text(label);
        }
        self.add(w)
    }

    /// 添加滑块。
    pub fn add_slider(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        min: f64,
        max: f64,
        value: f64,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Slider, parent_id, rect);
        w.min = min;
        w.max = max;
        w.value = value.clamp(min, max);
        self.add(w)
    }

    /// 添加进度条。
    pub fn add_progress_bar(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
        progress: u32,
    ) -> WidgetId {
        let mut w = Widget::new(WidgetKind::ProgressBar, parent_id, rect);
        w.progress = progress.min(100);
        w.focusable = false;
        self.add(w)
    }

    /// 添加滚动条。
    pub fn add_scroll_bar(
        &mut self,
        parent_id: Option<WidgetId>,
        rect: Rect,
    ) -> WidgetId {
        let w = Widget::new(WidgetKind::ScrollBar, parent_id, rect);
        self.add(w)
    }

    /// 添加面板。
    pub fn add_panel(&mut self, parent_id: Option<WidgetId>, rect: Rect) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Panel, parent_id, rect);
        w.focusable = false;
        self.add(w)
    }

    /// 添加选项卡。
    pub fn add_tab(&mut self, parent_id: Option<WidgetId>, rect: Rect, text: &str) -> WidgetId {
        let mut w = Widget::new(WidgetKind::Tab, parent_id, rect);
        w.set_text(text);
        w.focusable = true;
        self.add(w)
    }
}

// ============================================================================
// sense/act 通道接口（供 kernel/sense.rs 调用）
// ============================================================================

/// 全局 WidgetManager 单例（用于 `gui://widget` 通道）。
static WIDGET_MANAGER: OnceLock<Mutex<WidgetManager>> = OnceLock::new();

/// 获取全局控件管理器。
pub fn widget_manager() -> &'static Mutex<WidgetManager> {
    WIDGET_MANAGER.get_or_init(|| Mutex::new(WidgetManager::new()))
}

/// `sense gui://widget`：返回控件管理器状态摘要。
pub fn widget_sense() -> String {
    let mgr = widget_manager().lock().unwrap();
    let mut lines = vec![format!("widgets={} focused={:?}", mgr.len(), mgr.focused)];
    for (id, w) in mgr.iter() {
        lines.push(format!(
            "  #{}: kind={} text={:?} state={} enabled={} visible={} focused={}",
            id,
            w.kind.name(),
            w.text,
            w.state.label(),
            w.enabled,
            w.visible,
            w.focused
        ));
    }
    lines.join("\n")
}

/// `act gui://widget <op> ...`：对控件执行行动。
pub fn widget_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mgr = widget_manager();
    let mut m = mgr.lock().unwrap();
    match it.next().unwrap_or("") {
        "add" => {
            let kind = it.next().ok_or("用法: widget add <种类> ...")?;
            let x: i32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺少 x")?;
            let y: i32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺少 y")?;
            let w: u32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺少 w")?;
            let h: u32 = it.next().and_then(|v| v.parse().ok()).ok_or("缺少 h")?;
            let text = it.next().unwrap_or("");
            let rect = Rect::new(x, y, w, h);
            let id = match kind {
                "button" => m.add_button(None, rect, text),
                "label" => m.add_label(None, rect, text),
                "text_entry" => m.add_text_entry(None, rect, text),
                "checkbox" => m.add_checkbox(None, rect, text, false),
                "radio_button" => m.add_radio_button(None, rect, text, 0, false),
                "combo_box" => m.add_combo_box(None, rect, &[text.to_string()], 0),
                "slider" => m.add_slider(None, rect, 0.0, 100.0, 0.0),
                "progress_bar" => m.add_progress_bar(None, rect, 0),
                "scroll_bar" => m.add_scroll_bar(None, rect),
                "panel" => m.add_panel(None, rect),
                "tab" => m.add_tab(None, rect, text),
                _ => return Err(format!("未知控件种类: {}", kind)),
            };
            Ok(format!("已添加控件 #{} kind={}", id, kind))
        }
        "set_text" => {
            let id: u64 = it
                .next()
                .ok_or("缺少 id")?
                .parse::<u64>()
                .map_err(|e| e.to_string())?;
            let text = it.next().unwrap_or("");
            if m.set_text(id, text) {
                Ok(format!("控件 #{} 文本已设置", id))
            } else {
                Err(format!("控件 #{} 不存在", id))
            }
        }
        "set_enabled" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let enabled = it.next().unwrap_or("1") == "1";
            if m.set_enabled(id, enabled) {
                Ok(format!("控件 #{} enabled={}", id, enabled))
            } else {
                Err(format!("控件 #{} 不存在", id))
            }
        }
        "set_visible" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let visible = it.next().unwrap_or("1") == "1";
            if m.set_visible(id, visible) {
                Ok(format!("控件 #{} visible={}", id, visible))
            } else {
                Err(format!("控件 #{} 不存在", id))
            }
        }
        "focus" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            if m.focus(id) {
                Ok(format!("控件 #{} 已获焦点", id))
            } else {
                Err(format!("无法聚焦控件 #{}", id))
            }
        }
        "blur" => {
            let id: u64 = it.next().ok_or("缺少 id")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            if m.blur(id) {
                Ok(format!("控件 #{} 已失焦点", id))
            } else {
                Err(format!("控件 #{} 没有焦点", id))
            }
        }
        "focus_next" => {
            if let Some(id) = m.focus_next() {
                Ok(format!("焦点移至 #{}", id))
            } else {
                Ok("无可聚焦控件".to_string())
            }
        }
        "focus_prev" => {
            if let Some(id) = m.focus_prev() {
                Ok(format!("焦点移至 #{}", id))
            } else {
                Ok("无可聚焦控件".to_string())
            }
        }
        "list" => Ok(widget_sense()),
        _ => Err("用法: widget add|set_text|set_enabled|set_visible|focus|blur|focus_next|focus_prev|list".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::surface::palette;

    #[test]
    fn widget_add_and_get() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(10, 10, 100, 30), "OK");
        assert!(id > 0);
        let w = mgr.get(id).unwrap();
        assert_eq!(w.kind, WidgetKind::Button);
        assert_eq!(w.text, "OK");
        assert!(w.enabled);
        assert!(w.visible);
    }

    #[test]
    fn widget_set_text_and_enabled() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(0, 0, 50, 20), "hi");
        assert!(mgr.set_text(id, "hello"));
        assert_eq!(mgr.get(id).unwrap().text, "hello");
        assert!(mgr.set_enabled(id, false));
        assert!(!mgr.get(id).unwrap().enabled);
        assert!(!mgr.set_text(9999, "nope"));
    }

    #[test]
    fn widget_set_visible() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_label(None, Rect::new(0, 0, 50, 20), "lab");
        assert!(mgr.set_visible(id, false));
        assert!(!mgr.get(id).unwrap().visible);
        assert!(mgr.set_visible(id, true));
        assert!(mgr.get(id).unwrap().visible);
    }

    #[test]
    fn widget_hit_test() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(10, 10, 100, 30), "btn");
        assert!(mgr.get(id).unwrap().hit_test(50, 25));
        assert!(!mgr.get(id).unwrap().hit_test(200, 200));
    }

    #[test]
    fn widget_focus_blur() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(0, 0, 50, 20), "btn");
        assert!(mgr.focus(id));
        assert_eq!(mgr.focused(), Some(id));
        assert!(mgr.get(id).unwrap().focused);
        assert!(mgr.blur(id));
        assert_eq!(mgr.focused(), None);
        assert!(!mgr.get(id).unwrap().focused);
    }

    #[test]
    fn widget_focus_next_prev() {
        let mut mgr = WidgetManager::new();
        let a = mgr.add_button(None, Rect::new(0, 0, 50, 20), "a");
        let b = mgr.add_text_entry(None, Rect::new(0, 30, 50, 20), "b");
        let c = mgr.add_checkbox(None, Rect::new(0, 60, 50, 20), "c", false);
        // 标签不可聚焦。
        let _ = mgr.add_label(None, Rect::new(0, 90, 50, 20), "label");

        assert_eq!(mgr.focus_next(), Some(a));
        assert_eq!(mgr.focus_next(), Some(b));
        assert_eq!(mgr.focus_next(), Some(c));
        // 循环回到 a。
        assert_eq!(mgr.focus_next(), Some(a));

        assert_eq!(mgr.focus_prev(), Some(c));
        assert_eq!(mgr.focus_prev(), Some(b));
        assert_eq!(mgr.focus_prev(), Some(a));
        // 循环回到 c。
        assert_eq!(mgr.focus_prev(), Some(c));
    }

    #[test]
    fn widget_mouse_click_triggers_callback() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(0, 0, 50, 20), "btn");
        let mut clicked = false;
        mgr.on_click(id, || {
            clicked = true;
        });
        mgr.mouse_down(25, 10);
        mgr.mouse_up(25, 10);
        assert!(clicked);
    }

    #[test]
    fn widget_text_entry_input() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_text_entry(None, Rect::new(0, 0, 200, 30), "");
        mgr.focus(id);
        mgr.key_char('H');
        mgr.key_char('i');
        assert_eq!(mgr.get(id).unwrap().text, "Hi");
        mgr.key_backspace();
        assert_eq!(mgr.get(id).unwrap().text, "H");
    }

    #[test]
    fn widget_checkbox_toggle() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_checkbox(None, Rect::new(0, 0, 50, 20), "chk", false);
        assert!(!mgr.get(id).unwrap().checked);
        mgr.mouse_down(10, 10);
        mgr.mouse_up(10, 10);
        assert!(mgr.get(id).unwrap().checked);
    }

    #[test]
    fn widget_radio_button_mutual_exclusion() {
        let mut mgr = WidgetManager::new();
        let a = mgr.add_radio_button(None, Rect::new(0, 0, 50, 20), "a", 1, true);
        let b = mgr.add_radio_button(None, Rect::new(0, 30, 50, 20), "b", 1, false);
        assert!(mgr.get(a).unwrap().checked);
        assert!(!mgr.get(b).unwrap().checked);
        mgr.mouse_down(10, 40);
        mgr.mouse_up(10, 40);
        assert!(!mgr.get(a).unwrap().checked);
        assert!(mgr.get(b).unwrap().checked);
    }

    #[test]
    fn widget_slider_value() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_slider(None, Rect::new(0, 0, 100, 20), 0.0, 100.0, 50.0);
        let w = mgr.get(id).unwrap();
        assert!((w.value - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn widget_progress_bar() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_progress_bar(None, Rect::new(0, 0, 100, 20), 75);
        assert_eq!(mgr.get(id).unwrap().progress, 75);
    }

    #[test]
    fn widget_combo_box() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_combo_box(
            None,
            Rect::new(0, 0, 100, 25),
            &["one".to_string(), "two".to_string(), "three".to_string()],
            1,
        );
        let w = mgr.get(id).unwrap();
        assert_eq!(w.combo_selected, 1);
        assert_eq!(w.text, "two");
    }

    #[test]
    fn widget_scroll_bar() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_scroll_bar(None, Rect::new(0, 0, 20, 200));
        let w = mgr.get(id).unwrap();
        assert_eq!(w.kind, WidgetKind::ScrollBar);
    }

    #[test]
    fn widget_layout_flow() {
        let mut mgr = WidgetManager::new();
        mgr.set_layout(Layout::flow(5));
        let _ = mgr.add_button(None, Rect::new(0, 0, 50, 30), "a");
        let _ = mgr.add_button(None, Rect::new(0, 0, 50, 30), "b");
        mgr.layout_widgets(Rect::new(0, 0, 200, 200));
        // 流式布局后第二个控件应紧邻第一个右侧。
        let a = mgr.widgets.values().next().unwrap();
        let b = mgr.widgets.values().nth(1).unwrap();
        assert!(b.rect.x > a.rect.x);
    }

    #[test]
    fn widget_layout_grid() {
        let mut mgr = WidgetManager::new();
        mgr.set_layout(Layout::grid(2, 5));
        let _ = mgr.add_button(None, Rect::new(0, 0, 40, 30), "a");
        let _ = mgr.add_button(None, Rect::new(0, 0, 40, 30), "b");
        let _ = mgr.add_button(None, Rect::new(0, 0, 40, 30), "c");
        mgr.layout_widgets(Rect::new(0, 0, 200, 200));
        let widgets: Vec<&Widget> = mgr.widgets.values().collect();
        // 第三个控件应换到第二行。
        assert!(widgets[2].rect.y > widgets[0].rect.y);
    }

    #[test]
    fn widget_state_colors() {
        let colors = WidgetColors::default_theme();
        let (bg, fg, border) = state_color(&colors, WidgetState::Normal);
        assert_eq!(bg, palette::SURFACE0);
        let hc = WidgetColors::high_contrast();
        let (bg2, _, _) = state_color(&hc, WidgetState::Focused);
        assert_eq!(bg2, palette::YELLOW);
        let cp = WidgetColors::cyberpunk();
        let (_, fg2, _) = state_color(&cp, WidgetState::Normal);
        assert_eq!(fg2, argb(255, 0x00, 0xff, 0x41));
    }

    #[test]
    fn widget_current_colors_disabled() {
        let mut mgr = WidgetManager::new();
        let id = mgr.add_button(None, Rect::new(0, 0, 50, 20), "btn");
        mgr.set_enabled(id, false);
        let w = mgr.get(id).unwrap();
        let (bg, _, _) = w.current_colors(&mgr.colors);
        assert_eq!(bg, mgr.colors.bg_disabled);
    }

    #[test]
    fn widget_render_no_panic() {
        let mut mgr = WidgetManager::new();
        let _ = mgr.add_button(None, Rect::new(0, 0, 100, 40), "btn");
        let _ = mgr.add_label(None, Rect::new(0, 50, 100, 20), "label");
        let _ = mgr.add_checkbox(None, Rect::new(0, 80, 100, 20), "chk", true);
        let _ = mgr.add_progress_bar(None, Rect::new(0, 110, 100, 20), 50);
        let mut surface = Surface::filled(200, 200, palette::BASE);
        mgr.render(&mut surface);
        // 只是确认不 panic。
        assert_eq!(surface.width(), 200);
    }

    #[test]
    fn widget_sense_and_act() {
        let mut mgr = widget_manager().lock().unwrap();
        let id = mgr.add_button(None, Rect::new(0, 0, 50, 20), "hi");
        drop(mgr);
        let sense = widget_sense();
        assert!(sense.contains(&format!("#{}", id)));
        let res = widget_act(&format!("set_text {} hello", id)).unwrap();
        assert!(res.contains("已设置"));
        let res = widget_act("list").unwrap();
        assert!(res.contains("widgets="));
        let err = widget_act("set_text 99999 x").unwrap_err();
        assert!(err.contains("不存在"));
    }

    #[test]
    fn widget_kind_names() {
        assert_eq!(WidgetKind::Button.name(), "button");
        assert_eq!(WidgetKind::Label.name(), "label");
        assert_eq!(WidgetKind::Tab.name(), "tab");
    }

    #[test]
    fn widget_state_label() {
        assert_eq!(WidgetState::Normal.label(), "normal");
        assert_eq!(WidgetState::Pressed.label(), "pressed");
    }
}
