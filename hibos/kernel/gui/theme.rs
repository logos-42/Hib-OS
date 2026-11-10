//! 主题系统 —— 颜色角色、字体、图标、CSS 简化样式表、主题切换与持久化。
//!
//! `Theme` 描述单一主题；`ThemeManager` 管理当前主题、内置主题（浅色/深色/高对比/赛博朋克）
//! 与 CSS 简化版（选择器 + 属性）。`apply(widget_manager)` 把主题应用到全部控件。
//! `save(path)` / `load(path)` 支持主题文件的读写（JSON 简化格式，手写解析/序列化）。
//! 与 `kernel/gui/widget.rs` 的 `WidgetColors` 互补：Theme 是用户可见层，WidgetColors 是控件渲染层。

use super::surface::{argb, palette};
use super::widget::WidgetColors;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ============================================================================
// 颜色角色
// ============================================================================

/// UI 颜色角色：每个角色对应一组界面元素的颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorRole {
    /// 背景（窗口底色、桌面）。
    Background,
    /// 前景（默认文本）。
    Foreground,
    /// 主色（品牌色、主按钮）。
    Primary,
    /// 辅助色（次要按钮、强调）。
    Secondary,
    /// 成功（提示、确认）。
    Success,
    /// 警告（提醒、等待）。
    Warning,
    /// 错误（失败、删除）。
    Error,
    /// 悬停（鼠标悬停时）。
    Hover,
    /// 按下（点击时）。
    Pressed,
    /// 禁用（不可交互）。
    Disabled,
    /// 边框（窗口/控件边框）。
    Border,
    /// 阴影（浮层下方）。
    Shadow,
}

impl ColorRole {
    pub fn label(self) -> &'static str {
        match self {
            ColorRole::Background => "背景",
            ColorRole::Foreground => "前景",
            ColorRole::Primary => "主色",
            ColorRole::Secondary => "辅助色",
            ColorRole::Success => "成功",
            ColorRole::Warning => "警告",
            ColorRole::Error => "错误",
            ColorRole::Hover => "悬停",
            ColorRole::Pressed => "按下",
            ColorRole::Disabled => "禁用",
            ColorRole::Border => "边框",
            ColorRole::Shadow => "阴影",
        }
    }

    /// 该角色在深色默认主题下的默认值。
    pub fn default_value(self) -> u32 {
        match self {
            ColorRole::Background => palette::BASE,
            ColorRole::Foreground => palette::TEXT,
            ColorRole::Primary => palette::BLUE,
            ColorRole::Secondary => palette::MAUVE,
            ColorRole::Success => palette::GREEN,
            ColorRole::Warning => palette::YELLOW,
            ColorRole::Error => palette::RED,
            ColorRole::Hover => palette::SURFACE1,
            ColorRole::Pressed => palette::OVERLAY0,
            ColorRole::Disabled => palette::SURFACE0,
            ColorRole::Border => palette::OVERLAY0,
            ColorRole::Shadow => argb(128, 0, 0, 0),
        }
    }
}

// ============================================================================
// 主题
// ============================================================================

/// 单一主题：名称、颜色角色表、字体、图标表。
#[derive(Clone, Debug, Default)]
pub struct Theme {
    pub name: String,
    /// 颜色角色 -> ARGB。
    pub colors: BTreeMap<ColorRole, u32>,
    /// 字体名称 -> 尺寸。
    pub fonts: BTreeMap<String, u32>,
    /// 图标名称 -> 数据（base64 或路径，简化为字符串）。
    pub icons: BTreeMap<String, String>,
}

impl Theme {
    /// 创建主题。
    pub fn new(name: &str) -> Self {
        let mut colors = BTreeMap::new();
        for role in [
            ColorRole::Background,
            ColorRole::Foreground,
            ColorRole::Primary,
            ColorRole::Secondary,
            ColorRole::Success,
            ColorRole::Warning,
            ColorRole::Error,
            ColorRole::Hover,
            ColorRole::Pressed,
            ColorRole::Disabled,
            ColorRole::Border,
            ColorRole::Shadow,
        ] {
            colors.insert(role, role.default_value());
        }
        Self {
            name: name.to_string(),
            colors,
            fonts: BTreeMap::new(),
            icons: BTreeMap::new(),
        }
    }

    /// 设置角色颜色。
    pub fn set_color(&mut self, role: ColorRole, color: u32) {
        self.colors.insert(role, color);
    }

    /// 设置字体（名称 -> 尺寸）。
    pub fn set_font(&mut self, name: &str, size: u32) {
        self.fonts.insert(name.to_string(), size);
    }

    /// 设置图标（名称 -> 数据）。
    pub fn set_icon(&mut self, icon_name: &str, data: &str) {
        self.icons.insert(icon_name.to_string(), data.to_string());
    }

    /// 返回角色颜色（未设置则返回默认值）。
    pub fn color(&self, role: ColorRole) -> u32 {
        self.colors.get(&role).copied().unwrap_or(role.default_value())
    }

    /// 返回字体尺寸（未设置则 12）。
    pub fn font_size(&self, name: &str) -> u32 {
        self.fonts.get(name).copied().unwrap_or(12)
    }

    /// 返回图标数据（未设置则空）。
    pub fn icon(&self, icon_name: &str) -> Option<&str> {
        self.icons.get(icon_name).map(|s| s.as_str())
    }

    /// 把主题转换为控件颜色方案。
    pub fn to_widget_colors(&self) -> WidgetColors {
        WidgetColors {
            bg_normal: self.color(ColorRole::Background),
            bg_hover: self.color(ColorRole::Hover),
            bg_pressed: self.color(ColorRole::Pressed),
            bg_disabled: self.color(ColorRole::Disabled),
            bg_focused: self.color(ColorRole::Primary),
            fg_normal: self.color(ColorRole::Foreground),
            fg_hover: self.color(ColorRole::Foreground),
            fg_pressed: self.color(ColorRole::Foreground),
            fg_disabled: self.color(ColorRole::Disabled),
            fg_focused: self.color(ColorRole::Background),
            border_normal: self.color(ColorRole::Border),
            border_hover: self.color(ColorRole::Primary),
            border_pressed: self.color(ColorRole::Secondary),
            border_disabled: self.color(ColorRole::Disabled),
            border_focused: self.color(ColorRole::Primary),
        }
    }
}

// ============================================================================
// CSS 简化版：选择器 + 属性
// ============================================================================

/// CSS 属性键。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CssProperty {
    Color,
    BackgroundColor,
    BorderColor,
    FontSize,
    FontName,
}

impl CssProperty {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "color" => Some(CssProperty::Color),
            "background-color" => Some(CssProperty::BackgroundColor),
            "border-color" => Some(CssProperty::BorderColor),
            "font-size" => Some(CssProperty::FontSize),
            "font-name" => Some(CssProperty::FontName),
            _ => None,
        }
    }
}

/// CSS 规则：选择器 + 属性列表。
#[derive(Clone, Debug, Default)]
pub struct CssRule {
    pub selector: String,
    pub properties: BTreeMap<CssProperty, String>,
}

impl CssRule {
    pub fn new(selector: &str) -> Self {
        Self {
            selector: selector.to_string(),
            properties: BTreeMap::new(),
        }
    }

    pub fn set(&mut self, prop: CssProperty, value: &str) {
        self.properties.insert(prop, value.to_string());
    }
}

/// CSS 样式表（简化版）。
#[derive(Clone, Debug, Default)]
pub struct CssStylesheet {
    pub rules: Vec<CssRule>,
}

impl CssStylesheet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加规则。
    pub fn add_rule(&mut self, rule: CssRule) {
        self.rules.push(rule);
    }

    /// 从 CSS 文本解析（极简：`selector { prop: value; }`）。
    pub fn parse(css: &str) -> Self {
        let mut sheet = Self::new();
        let mut depth = 0usize;
        let mut current_selector = String::new();
        let mut current_props: BTreeMap<CssProperty, String> = BTreeMap::new();
        let mut buf = String::new();

        for ch in css.chars() {
            match ch {
                '{' => {
                    if depth == 0 {
                        current_selector = buf.trim().to_string();
                        buf.clear();
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        // 解析 buf 中的属性。
                        for decl in buf.split(';') {
                            let decl = decl.trim();
                            if decl.is_empty() {
                                continue;
                            }
                            if let Some((prop, val)) = decl.split_once(':') {
                                let prop = prop.trim();
                                let val = val.trim();
                                if let Some(cp) = CssProperty::from_str(prop) {
                                    current_props.insert(cp, val.to_string());
                                }
                            }
                        }
                        if !current_selector.is_empty() {
                            let mut rule = CssRule::new(&current_selector);
                            // BTreeMap 没有 drain()，用 take 取走本轮属性再遍历。
                            let props = std::mem::take(&mut current_props);
                            for (prop, val) in props {
                                rule.set(prop, &val);
                            }
                            sheet.add_rule(rule);
                        }
                        current_selector.clear();
                        buf.clear();
                    } else {
                        buf.push(ch);
                    }
                }
                _ => {
                    if depth > 0 {
                        buf.push(ch);
                    } else {
                        buf.push(ch);
                    }
                }
            }
        }
        sheet
    }
}

// ============================================================================
// 内置主题
// ============================================================================

/// 浅色主题。
pub fn light_theme() -> Theme {
    let mut t = Theme::new("浅色");
    t.set_color(ColorRole::Background, argb(255, 0xff, 0xff, 0xff));
    t.set_color(ColorRole::Foreground, argb(255, 0x11, 0x11, 0x11));
    t.set_color(ColorRole::Primary, argb(255, 0x00, 0x66, 0xcc));
    t.set_color(ColorRole::Secondary, argb(255, 0x66, 0x66, 0x66));
    t.set_color(ColorRole::Success, argb(255, 0x00, 0xaa, 0x00));
    t.set_color(ColorRole::Warning, argb(255, 0xcc, 0x66, 0x00));
    t.set_color(ColorRole::Error, argb(255, 0xcc, 0x00, 0x00));
    t.set_color(ColorRole::Hover, argb(255, 0xee, 0xee, 0xee));
    t.set_color(ColorRole::Pressed, argb(255, 0xdd, 0xdd, 0xdd));
    t.set_color(ColorRole::Disabled, argb(255, 0xee, 0xee, 0xee));
    t.set_color(ColorRole::Border, argb(255, 0xcc, 0xcc, 0xcc));
    t.set_color(ColorRole::Shadow, argb(100, 0, 0, 0));
    t.set_font("default", 14);
    t
}

/// 深色主题（Catppuccin Mocha）。
pub fn dark_theme() -> Theme {
    let mut t = Theme::new("深色");
    t.set_color(ColorRole::Background, palette::BASE);
    t.set_color(ColorRole::Foreground, palette::TEXT);
    t.set_color(ColorRole::Primary, palette::BLUE);
    t.set_color(ColorRole::Secondary, palette::MAUVE);
    t.set_color(ColorRole::Success, palette::GREEN);
    t.set_color(ColorRole::Warning, palette::YELLOW);
    t.set_color(ColorRole::Error, palette::RED);
    t.set_color(ColorRole::Hover, palette::SURFACE1);
    t.set_color(ColorRole::Pressed, palette::OVERLAY0);
    t.set_color(ColorRole::Disabled, palette::SURFACE0);
    t.set_color(ColorRole::Border, palette::OVERLAY0);
    t.set_color(ColorRole::Shadow, argb(150, 0, 0, 0));
    t.set_font("default", 13);
    t
}

/// 高对比度主题。
pub fn high_contrast_theme() -> Theme {
    let mut t = Theme::new("高对比度");
    t.set_color(ColorRole::Background, argb(255, 0x00, 0x00, 0x00));
    t.set_color(ColorRole::Foreground, argb(255, 0xff, 0xff, 0xff));
    t.set_color(ColorRole::Primary, argb(255, 0xff, 0xff, 0x00));
    t.set_color(ColorRole::Secondary, argb(255, 0x00, 0xff, 0xff));
    t.set_color(ColorRole::Success, argb(255, 0x00, 0xff, 0x00));
    t.set_color(ColorRole::Warning, argb(255, 0xff, 0xff, 0x00));
    t.set_color(ColorRole::Error, argb(255, 0xff, 0x00, 0x00));
    t.set_color(ColorRole::Hover, argb(255, 0x33, 0x33, 0x33));
    t.set_color(ColorRole::Pressed, argb(255, 0x55, 0x55, 0x55));
    t.set_color(ColorRole::Disabled, argb(255, 0x11, 0x11, 0x11));
    t.set_color(ColorRole::Border, argb(255, 0xff, 0xff, 0xff));
    t.set_color(ColorRole::Shadow, argb(255, 0xff, 0xff, 0xff));
    t.set_font("default", 14);
    t
}

/// 赛博朋克主题。
pub fn cyberpunk_theme() -> Theme {
    let mut t = Theme::new("赛博朋克");
    t.set_color(ColorRole::Background, argb(255, 0x05, 0x05, 0x0a));
    t.set_color(ColorRole::Foreground, argb(255, 0x00, 0xff, 0x41));
    t.set_color(ColorRole::Primary, argb(255, 0x00, 0xff, 0x41));
    t.set_color(ColorRole::Secondary, argb(255, 0xff, 0x00, 0xff));
    t.set_color(ColorRole::Success, argb(255, 0x00, 0xff, 0x41));
    t.set_color(ColorRole::Warning, argb(255, 0xff, 0xcc, 0x00));
    t.set_color(ColorRole::Error, argb(255, 0xff, 0x00, 0x41));
    t.set_color(ColorRole::Hover, argb(255, 0x1a, 0x1a, 0x2a));
    t.set_color(ColorRole::Pressed, argb(255, 0x2a, 0x2a, 0x4a));
    t.set_color(ColorRole::Disabled, argb(255, 0x22, 0x22, 0x33));
    t.set_color(ColorRole::Border, argb(255, 0x00, 0xff, 0x41));
    t.set_color(ColorRole::Shadow, argb(150, 0x00, 0xff, 0x41));
    t.set_font("default", 13);
    t
}

// ============================================================================
// 主题管理器
// ============================================================================

/// 主题管理器：当前主题、内置主题注册表、主题切换、CSS 应用。
pub struct ThemeManager {
    /// 内置主题名 -> 主题。
    themes: BTreeMap<String, Theme>,
    /// 当前主题名。
    current: String,
    /// 是否启用 CSS 覆盖。
    pub css_enabled: bool,
    /// CSS 样式表。
    pub css: CssStylesheet,
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ThemeManager {
    /// 创建管理器并注册内置主题。
    pub fn new() -> Self {
        let mut themes = BTreeMap::new();
        themes.insert("浅色".to_string(), light_theme());
        themes.insert("深色".to_string(), dark_theme());
        themes.insert("高对比度".to_string(), high_contrast_theme());
        themes.insert("赛博朋克".to_string(), cyberpunk_theme());
        let current = "深色".to_string();
        Self {
            themes,
            current: current.clone(),
            css_enabled: false,
            css: CssStylesheet::new(),
        }
    }

    /// 返回当前主题引用。
    pub fn current_theme(&self) -> &Theme {
        self.themes.get(&self.current).unwrap()
    }

    /// 返回当前主题名。
    pub fn current_name(&self) -> &str {
        &self.current
    }

    /// 切换主题。
    pub fn set_theme(&mut self, name: &str) -> bool {
        if self.themes.contains_key(name) {
            self.current = name.to_string();
            true
        } else {
            false
        }
    }

    /// 列出全部主题名。
    pub fn theme_names(&self) -> Vec<String> {
        self.themes.keys().cloned().collect()
    }

    /// 添加/覆盖一个主题。
    pub fn add_theme(&mut self, theme: Theme) {
        let name = theme.name.clone();
        self.themes.insert(name, theme);
    }

    /// 移除主题（不能移除当前主题）。
    pub fn remove_theme(&mut self, name: &str) -> bool {
        if name == self.current {
            return false;
        }
        self.themes.remove(name).is_some()
    }

    /// 设置角色颜色（应用到当前主题）。
    pub fn set_color(&mut self, role: ColorRole, color: u32) {
        if let Some(t) = self.themes.get_mut(&self.current) {
            t.set_color(role, color);
        }
    }

    /// 设置字体。
    pub fn set_font(&mut self, name: &str, size: u32) {
        if let Some(t) = self.themes.get_mut(&self.current) {
            t.set_font(name, size);
        }
    }

    /// 设置图标。
    pub fn set_icon(&mut self, icon_name: &str, data: &str) {
        if let Some(t) = self.themes.get_mut(&self.current) {
            t.set_icon(icon_name, data);
        }
    }

    /// 把当前主题应用到 WidgetManager（转换为 WidgetColors 并设置）。
    pub fn apply(&self, widget_manager: &mut super::widget::WidgetManager) {
        let colors = self.current_theme().to_widget_colors();
        widget_manager.set_colors(colors);
    }

    /// 应用 CSS 样式表到主题（简化：解析 color/background-color 等映射到角色）。
    pub fn apply_css(&mut self, css: &str) {
        let sheet = CssStylesheet::parse(css);
        self.css = sheet;
        self.css_enabled = true;
        // 简化映射：通配符选择器 * 的 color/background-color 应用到前景/背景。
        // 先克隆规则，避免在对 self 的可变调用时与 self.css.rules 的不可变借用冲突。
        let rules: Vec<CssRule> = self.css.rules.clone();
        for rule in &rules {
            if rule.selector == "*" || rule.selector == "root" {
                for (prop, val) in &rule.properties {
                    match prop {
                        CssProperty::Color => {
                            if let Some(c) = parse_color(val) {
                                self.set_color(ColorRole::Foreground, c);
                            }
                        }
                        CssProperty::BackgroundColor => {
                            if let Some(c) = parse_color(val) {
                                self.set_color(ColorRole::Background, c);
                            }
                        }
                        CssProperty::BorderColor => {
                            if let Some(c) = parse_color(val) {
                                self.set_color(ColorRole::Border, c);
                            }
                        }
                        CssProperty::FontSize => {
                            if let Ok(sz) = val.trim_end_matches("px").parse::<u32>() {
                                self.set_font("default", sz);
                            }
                        }
                        CssProperty::FontName => {
                            self.set_font(val, 14);
                        }
                    }
                }
            }
        }
    }

    /// 保存主题到文件（JSON 简化格式）。
    pub fn save(&self, path: &str) -> Result<(), String> {
        let theme = self.current_theme();
        let mut json = String::new();
        json.push_str("{\n");
        json.push_str(&format!("  \"name\": \"{}\",\n", theme.name));
        json.push_str("  \"colors\": {\n");
        let roles = [
            ColorRole::Background,
            ColorRole::Foreground,
            ColorRole::Primary,
            ColorRole::Secondary,
            ColorRole::Success,
            ColorRole::Warning,
            ColorRole::Error,
            ColorRole::Hover,
            ColorRole::Pressed,
            ColorRole::Disabled,
            ColorRole::Border,
            ColorRole::Shadow,
        ];
        for (i, role) in roles.iter().enumerate() {
            let c = theme.color(*role);
            json.push_str(&format!("    \"{}\": #{:08X}", role.label(), c));
            if i < roles.len() - 1 {
                json.push(',');
            }
            json.push('\n');
        }
        json.push_str("  },\n");
        json.push_str("  \"fonts\": {\n");
        let fonts: Vec<_> = theme.fonts.iter().collect();
        for (i, (name, size)) in fonts.iter().enumerate() {
            json.push_str(&format!("    \"{}\": {}", name, size));
            if i < fonts.len() - 1 {
                json.push(',');
            }
            json.push('\n');
        }
        json.push_str("  }\n");
        json.push_str("}\n");
        std::fs::write(path, json).map_err(|e| format!("保存失败：{}", e))
    }

    /// 从文件加载主题（JSON 简化格式）。
    pub fn load(&mut self, path: &str) -> Result<Theme, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{}", e))?;
        let mut theme = Theme::new("已加载");
        // 非常简化的 JSON 解析：提取 #RRGGBBAA 颜色。
        for line in content.lines() {
            let line = line.trim();
            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim().trim_matches('"');
                let val = val.trim().trim_matches('"').trim_matches(',');
                if let Some(c) = parse_color(val) {
                    let role = match key {
                        "背景" => ColorRole::Background,
                        "前景" => ColorRole::Foreground,
                        "主色" => ColorRole::Primary,
                        "辅助色" => ColorRole::Secondary,
                        "成功" => ColorRole::Success,
                        "警告" => ColorRole::Warning,
                        "错误" => ColorRole::Error,
                        "悬停" => ColorRole::Hover,
                        "按下" => ColorRole::Pressed,
                        "禁用" => ColorRole::Disabled,
                        "边框" => ColorRole::Border,
                        "阴影" => ColorRole::Shadow,
                        _ => continue,
                    };
                    theme.set_color(role, c);
                }
            }
        }
        Ok(theme)
    }

    /// 返回主题数量。
    pub fn len(&self) -> usize {
        self.themes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.themes.is_empty()
    }
}

/// 解析颜色字符串（支持 #RRGGBB / #RRGGBBAA / "红" 等简化）。
fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.starts_with('#') {
        let hex = s.trim_start_matches('#');
        if hex.len() == 8 {
            u32::from_str_radix(hex, 16).ok()
        } else if hex.len() == 6 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            Some(argb(255, (v >> 16) as u8, (v >> 8) as u8, v as u8))
        } else {
            None
        }
    } else {
        match s {
            "红" => Some(palette::RED),
            "绿" => Some(palette::GREEN),
            "蓝" => Some(palette::BLUE),
            "黑" => Some(palette::BASE),
            "白" => Some(palette::TEXT),
            _ => None,
        }
    }
}

// ============================================================================
// 全局 ThemeManager 单例（供 kernel/sense.rs 调用）
// ============================================================================

static THEME_MANAGER: OnceLock<Mutex<ThemeManager>> = OnceLock::new();

/// 获取全局主题管理器。
pub fn theme_manager() -> &'static Mutex<ThemeManager> {
    THEME_MANAGER.get_or_init(|| Mutex::new(ThemeManager::new()))
}

/// `sense gui://theme`：返回主题管理器状态摘要。
pub fn theme_sense() -> String {
    let tm = theme_manager().lock().unwrap();
    let t = tm.current_theme();
    format!(
        "current={} themes={} css={} bg=#{:08X} fg=#{:08X} primary=#{:08X}",
        tm.current_name(),
        tm.len(),
        tm.css_enabled,
        t.color(ColorRole::Background),
        t.color(ColorRole::Foreground),
        t.color(ColorRole::Primary)
    )
}

/// `act gui://theme <op> ...`：对主题管理器执行行动。
pub fn theme_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let tm = theme_manager();
    let mut tm = tm.lock().unwrap();
    match it.next().unwrap_or("") {
        "set" => {
            let name = it.next().ok_or("缺少主题名")?;
            if tm.set_theme(name) {
                Ok(format!("主题已切换到 {}", name))
            } else {
                Err(format!("未知主题 {}", name))
            }
        }
        "list" => {
            let names = tm.theme_names().join(", ");
            Ok(format!("主题列表：{}", names))
        }
        "color" => {
            let role = it.next().ok_or("缺少角色")?;
            let val = it.next().ok_or("缺少颜色")?;
            let c = parse_color(val).ok_or("无效颜色")?;
            let role = match role {
                "背景" => ColorRole::Background,
                "前景" => ColorRole::Foreground,
                "主色" => ColorRole::Primary,
                "辅助色" => ColorRole::Secondary,
                "成功" => ColorRole::Success,
                "警告" => ColorRole::Warning,
                "错误" => ColorRole::Error,
                "悬停" => ColorRole::Hover,
                "按下" => ColorRole::Pressed,
                "禁用" => ColorRole::Disabled,
                "边框" => ColorRole::Border,
                "阴影" => ColorRole::Shadow,
                _ => return Err("未知颜色角色".to_string()),
            };
            tm.set_color(role, c);
            Ok(format!("{} 已设置为 {}", role.label(), val))
        }
        "font" => {
            let name = it.next().ok_or("缺少字体名")?;
            let size: u32 = it
                .next()
                .ok_or("缺少尺寸")?
                .parse::<u32>()
                .map_err(|e| e.to_string())?;
            tm.set_font(name, size);
            Ok(format!("字体 {} 尺寸 {}px", name, size))
        }
        "icon" => {
            let icon_name = it.next().ok_or("缺少图标名")?;
            let data = it.next().ok_or("缺少图标数据")?;
            tm.set_icon(icon_name, data);
            Ok(format!("图标 {} 已设置", icon_name))
        }
        "apply" => {
            // 应用到全局 WidgetManager。
            let widget_mgr = super::widget::widget_manager();
            let mut wm = widget_mgr.lock().unwrap();
            tm.apply(&mut wm);
            Ok("主题已应用到控件管理器".to_string())
        }
        "css" => {
            let css = it.collect::<Vec<_>>().join(" ");
            tm.apply_css(&css);
            Ok(format!("CSS 已应用（{} 条规则）", tm.css.rules.len()))
        }
        "save" => {
            let path = it.next().ok_or("缺少路径")?;
            tm.save(path)?;
            Ok(format!("主题已保存到 {}", path))
        }
        "load" => {
            let path = it.next().ok_or("缺少路径")?;
            let _theme = tm.load(path)?;
            Ok(format!("主题已从 {} 加载", path))
        }
        "add" => {
            let name = it.next().ok_or("缺少主题名")?;
            let mut t = Theme::new(name);
            t.set_color(ColorRole::Primary, palette::BLUE);
            tm.add_theme(t);
            Ok(format!("主题 {} 已添加", name))
        }
        "remove" => {
            let name = it.next().ok_or("缺少主题名")?;
            if tm.remove_theme(name) {
                Ok(format!("主题 {} 已移除", name))
            } else {
                Err(format!("无法移除主题 {}（可能是当前主题或不存在）", name))
            }
        }
        _ => Err(
            "用法: theme set|list|color|font|icon|apply|css|save|load|add|remove".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_new_defaults() {
        let t = Theme::new("单元测试");
        assert_eq!(t.name, "单元测试");
        assert_eq!(t.color(ColorRole::Background), palette::BASE);
        assert_eq!(t.color(ColorRole::Foreground), palette::TEXT);
    }

    #[test]
    fn theme_set_color_font_icon() {
        let mut t = Theme::new("t");
        t.set_color(ColorRole::Primary, palette::RED);
        assert_eq!(t.color(ColorRole::Primary), palette::RED);
        t.set_font("mono", 16);
        assert_eq!(t.font_size("mono"), 16);
        t.set_icon("save", "data:save");
        assert_eq!(t.icon("save"), Some("data:save"));
    }

    #[test]
    fn theme_to_widget_colors() {
        let mut t = Theme::new("t");
        t.set_color(ColorRole::Background, palette::RED);
        t.set_color(ColorRole::Foreground, palette::GREEN);
        let wc = t.to_widget_colors();
        assert_eq!(wc.bg_normal, palette::RED);
        assert_eq!(wc.fg_normal, palette::GREEN);
    }

    #[test]
    fn light_dark_theme() {
        let light = light_theme();
        assert_eq!(light.color(ColorRole::Background), argb(255, 0xff, 0xff, 0xff));
        let dark = dark_theme();
        assert_eq!(dark.color(ColorRole::Background), palette::BASE);
    }

    #[test]
    fn high_contrast_cyberpunk() {
        let hc = high_contrast_theme();
        assert_eq!(hc.color(ColorRole::Background), argb(255, 0, 0, 0));
        assert_eq!(hc.color(ColorRole::Foreground), argb(255, 0xff, 0xff, 0xff));
        let cp = cyberpunk_theme();
        assert_eq!(cp.color(ColorRole::Foreground), argb(255, 0x00, 0xff, 0x41));
    }

    #[test]
    fn theme_manager_switch() {
        let mut tm = ThemeManager::new();
        assert_eq!(tm.current_name(), "深色");
        assert!(tm.set_theme("浅色"));
        assert_eq!(tm.current_name(), "浅色");
        assert!(!tm.set_theme("不存在"));
        assert!(tm.remove_theme("高对比度"));
        assert!(!tm.remove_theme("浅色")); // 当前主题不能移除。
    }

    #[test]
    fn theme_manager_apply_css() {
        let mut tm = ThemeManager::new();
        tm.apply_css("* { color: 红; background-color: #000000ff; }");
        assert!(tm.css_enabled);
        assert_eq!(tm.current_theme().color(ColorRole::Foreground), palette::RED);
        assert_eq!(tm.current_theme().color(ColorRole::Background), argb(255, 0, 0, 0));
    }

    #[test]
    fn theme_save_load() {
        let mut tm = ThemeManager::new();
        tm.set_color(ColorRole::Primary, palette::MAUVE);
        let path = std::env::temp_dir().join("hibos-theme-test.json");
        tm.save(path.to_str().unwrap()).unwrap();
        let mut tm2 = ThemeManager::new();
        let loaded = tm2.load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.color(ColorRole::Primary), palette::MAUVE);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_color_helper() {
        assert_eq!(parse_color("#ff0000ff"), Some(argb(255, 0xff, 0, 0)));
        assert_eq!(parse_color("#00ff00"), Some(argb(255, 0, 0xff, 0)));
        assert_eq!(parse_color("红"), Some(palette::RED));
        assert_eq!(parse_color("invalid"), None);
    }

    #[test]
    fn css_rule_set() {
        let mut rule = CssRule::new("button");
        rule.set(CssProperty::Color, "#ff0000");
        assert!(rule.properties.contains_key(&CssProperty::Color));
    }

    #[test]
    fn css_parse_simple() {
        let sheet = CssStylesheet::parse("button { color: 红; } label { background-color: #00ff00; }");
        assert_eq!(sheet.rules.len(), 2);
    }

    #[test]
    fn theme_manager_sense_and_act() {
        let mut tm = theme_manager().lock().unwrap();
        tm.set_theme("浅色");
        drop(tm);
        let sense = theme_sense();
        assert!(sense.contains("current="));
        let res = theme_act("set 深色").unwrap();
        assert!(res.contains("深色"));
        let res = theme_act("list").unwrap();
        assert!(res.contains("浅色"));
        let res = theme_act("color 主色 #00ff00").unwrap();
        assert!(res.contains("主色"));
        let err = theme_act("set 不存在").unwrap_err();
        assert!(err.contains("未知"));
    }
}
