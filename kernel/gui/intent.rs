//! 意图路由 —— 把键鼠、手势、自然语言三种输入归一为 `Intent`。
//!
//! 这是「界面零命令」的核心：用户表达的是意图，不是命令；系统把意图翻译成
//! GUAI 动作，并在高风险操作时强制走权限确认。

use super::gesture::{Gesture, SwipeDirection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    Conversation,
    Tasks,
    Permission,
    Files,
    Observation,
    Settings,
    Help,
    Advanced,
}

impl PageKind {
    pub fn label(self) -> &'static str {
        match self {
            PageKind::Conversation => "对话",
            PageKind::Tasks => "任务",
            PageKind::Permission => "权限",
            PageKind::Files => "文件",
            PageKind::Observation => "观察",
            PageKind::Settings => "设置",
            PageKind::Help => "帮助",
            PageKind::Advanced => "高级工具",
        }
    }

    /// 屏幕短标签（ASCII）：位图字体只覆盖 ASCII，界面统一用它，避免豆腐块。
    pub fn label_en(self) -> &'static str {
        match self {
            PageKind::Conversation => "CHAT",
            PageKind::Tasks => "TASKS",
            PageKind::Permission => "PERMISSION",
            PageKind::Files => "FILES",
            PageKind::Observation => "OBSERVE",
            PageKind::Settings => "SETTINGS",
            PageKind::Help => "HELP",
            PageKind::Advanced => "ADVANCED",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthLevel {
    Once,
    Session,
    Persistent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    OpenPage(PageKind),
    /// 打开一个浮动终端窗口（默认桌面无终端）。
    OpenTerminal,
    ReturnDesktop,
    NextPage,
    PrevPage,
    ToggleDock,
    RequestPermission { what: String, level: AuthLevel },
    /// AI 生成创造物：需求文本。
    GenerateApp(String),
    GeneratePlugin(String),
    GenerateIme(String),
    GenerateAnimation(String),
    Unknown(String),
}

pub struct IntentRouter;

impl IntentRouter {
    /// 自然语言 → 意图。高频意图本地匹配，未知意图交给上层 Agent 解析。
    pub fn resolve(text: &str) -> Option<Intent> {
        let t = text.trim();
        match t {
            "打开对话" | "对话" | "chat" => Some(Intent::OpenPage(PageKind::Conversation)),
            "打开任务" | "任务" | "tasks" => Some(Intent::OpenPage(PageKind::Tasks)),
            "看看有什么要我确认的" | "待授权" | "权限" => Some(Intent::OpenPage(PageKind::Permission)),
            "打开文件" | "文件" | "files" => Some(Intent::OpenPage(PageKind::Files)),
            "系统状态" | "观察" | "monitor" => Some(Intent::OpenPage(PageKind::Observation)),
            "设置" | "settings" => Some(Intent::OpenPage(PageKind::Settings)),
            "帮助" | "help" => Some(Intent::OpenPage(PageKind::Help)),
            "高级工具" | "advanced" => Some(Intent::OpenPage(PageKind::Advanced)),
            "返回桌面" | "桌面" | "desktop" => Some(Intent::ReturnDesktop),
            "下一个" | "下一页" => Some(Intent::NextPage),
            "上一个" | "上一页" => Some(Intent::PrevPage),
            "删除所有文件" | "清空所有" => Some(Intent::RequestPermission {
                what: "删除所有文件".into(),
                level: AuthLevel::Once,
            }),
            // AI 创造意图：前缀式表达（需求自由文本）。
            _ if t.starts_with("做一个") || t.starts_with("生成app") || t.starts_with("create app") => {
                Some(Intent::GenerateApp(t.to_string()))
            }
            _ if t.starts_with("写个插件") || t.starts_with("生成插件") => {
                Some(Intent::GeneratePlugin(t.to_string()))
            }
            _ if t.starts_with("自定义输入法") || t.starts_with("生成输入法") => {
                Some(Intent::GenerateIme(t.to_string()))
            }
            _ if t.starts_with("做个动画") || t.starts_with("生成动画") => {
                Some(Intent::GenerateAnimation(t.to_string()))
            }
            // 精确匹配失败后走语义兜底：同义短语 + 最长匹配 + 高风险优先。
            _ => Self::resolve_fuzzy(t),
        }
    }

    /// 语义兜底：覆盖用户不必记住精确说法的同义表达。
    ///
    /// 打分规则：命中关键词的最长长度（越长越具体）；高分者胜。
    /// 高风险操作放在表首，避免「删除所有文件」被「文件」抢走。
    fn resolve_fuzzy(text: &str) -> Option<Intent> {
        let t = text.to_lowercase();
        let mut best: Option<(usize, Intent)> = None;
        for &(keys, intent) in fuzzy_table() {
            for k in keys {
                if t.contains(*k) {
                    let score = k.chars().count();
                    if best.as_ref().map_or(true, |(s, _)| score > *s) {
                        best = Some((score, intent()));
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// 快捷键 → 意图。快捷键是可选加速，不是必需知识。
    pub fn resolve_key(key: &str) -> Option<Intent> {
        match key {
            "Escape" => Some(Intent::ReturnDesktop),
            "CmdTab" => Some(Intent::NextPage),
            "CmdShiftTab" => Some(Intent::PrevPage),
            "CmdK" => Some(Intent::OpenPage(PageKind::Advanced)),
            "Cmd1" => Some(Intent::OpenPage(PageKind::Conversation)),
            "Cmd2" => Some(Intent::OpenPage(PageKind::Tasks)),
            "Cmd3" => Some(Intent::OpenPage(PageKind::Permission)),
            "Cmd4" => Some(Intent::OpenPage(PageKind::Files)),
            "ctrl+alt+t" => Some(Intent::OpenTerminal),
            _ => None,
        }
    }

    /// 手势 → 意图。与键鼠走同一条路由。
    pub fn resolve_gesture(g: Gesture) -> Option<Intent> {
        match g {
            Gesture::Swipe { direction: SwipeDirection::Left, .. } => Some(Intent::NextPage),
            Gesture::Swipe { direction: SwipeDirection::Right, .. } => Some(Intent::PrevPage),
            Gesture::Swipe { direction: SwipeDirection::Down, .. } => Some(Intent::ReturnDesktop),
            Gesture::Swipe { direction: SwipeDirection::Up, .. } => Some(Intent::ToggleDock),
            Gesture::DoubleTap { .. } => Some(Intent::NextPage),
            _ => None,
        }
    }
}

/// 语义兜底短语表：高风险操作在前，其次按页面；关键词越具体越优先。
fn fuzzy_table() -> &'static [(&'static [&'static str], fn() -> Intent)] {
    &[
        (
            &["删除所有", "清空所有", "格式化", "rm -rf", "delete all", "wipe"],
            || Intent::RequestPermission {
                what: "删除所有文件".into(),
                level: AuthLevel::Once,
            },
        ),
        (&["对话", "聊天", "chat", "conversation"], || {
            Intent::OpenPage(PageKind::Conversation)
        }),
        (&["任务", "待办", "task", "todo"], || Intent::OpenPage(PageKind::Tasks)),
        (
            &["权限", "授权", "确认", "permission", "approve", "allow"],
            || Intent::OpenPage(PageKind::Permission),
        ),
        (&["文件", "目录", "文件夹", "file", "folder", "dir"], || {
            Intent::OpenPage(PageKind::Files)
        }),
        (
            &["观察", "监控", "状态", "observe", "monitor", "status"],
            || Intent::OpenPage(PageKind::Observation),
        ),
        (
            &["设置", "配置", "setting", "config", "preference"],
            || Intent::OpenPage(PageKind::Settings),
        ),
        (&["帮助", "怎么用", "guide", "help"], || Intent::OpenPage(PageKind::Help)),
        (
            &["高级", "终端", "命令", "advanced", "terminal", "shell"],
            || Intent::OpenPage(PageKind::Advanced),
        ),
        (
            &["返回桌面", "回到桌面", "桌面", "desktop", "home", "back"],
            || Intent::ReturnDesktop,
        ),
        (&["下一个", "下一页", "next"], || Intent::NextPage),
        (&["上一个", "上一页", "previous", "prev"], || Intent::PrevPage),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_language_maps_to_open_page() {
        assert_eq!(IntentRouter::resolve("打开任务"), Some(Intent::OpenPage(PageKind::Tasks)));
        assert_eq!(IntentRouter::resolve("看看有什么要我确认的"), Some(Intent::OpenPage(PageKind::Permission)));
    }

    #[test]
    fn fuzzy_understands_synonyms_and_phrases() {
        // 非精确说法也要能命中。
        assert_eq!(IntentRouter::resolve("帮我打开任务吧"), Some(Intent::OpenPage(PageKind::Tasks)));
        assert_eq!(IntentRouter::resolve("我想看看文件"), Some(Intent::OpenPage(PageKind::Files)));
        assert_eq!(IntentRouter::resolve("show me permissions"), Some(Intent::OpenPage(PageKind::Permission)));
        assert_eq!(IntentRouter::resolve("回到桌面"), Some(Intent::ReturnDesktop));
        assert_eq!(IntentRouter::resolve("go home"), Some(Intent::ReturnDesktop));
    }

    #[test]
    fn high_risk_phrase_beats_page_keyword() {
        // 「删除所有…」必须优先于「文件」这一页面关键词。
        let i = IntentRouter::resolve("删除所有文件").unwrap();
        assert!(matches!(
            i,
            Intent::RequestPermission { level: AuthLevel::Once, .. }
        ));
    }

    #[test]
    fn gibberish_has_no_intent() {
        assert_eq!(IntentRouter::resolve("随便说点什么"), None);
    }

    #[test]
    fn shortcut_maps_to_return_desktop() {
        assert_eq!(IntentRouter::resolve_key("Escape"), Some(Intent::ReturnDesktop));
        assert_eq!(IntentRouter::resolve_key("CmdTab"), Some(Intent::NextPage));
    }

    #[test]
    fn gesture_maps_to_next_page() {
        let g = Gesture::Swipe {
            start_x: 300,
            start_y: 100,
            end_x: 100,
            end_y: 100,
            direction: SwipeDirection::Left,
            velocity: 1.0,
        };
        assert_eq!(IntentRouter::resolve_gesture(g), Some(Intent::NextPage));
    }

    #[test]
    fn high_risk_requires_permission() {
        let i = IntentRouter::resolve("删除所有文件").unwrap();
        assert!(matches!(i, Intent::RequestPermission { level: AuthLevel::Once, .. }));
    }

    #[test]
    fn unknown_text_returns_none() {
        assert_eq!(IntentRouter::resolve("随便说点什么"), None);
    }
}
