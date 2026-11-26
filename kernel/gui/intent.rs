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
    ReturnDesktop,
    NextPage,
    PrevPage,
    ToggleDock,
    RequestPermission { what: String, level: AuthLevel },
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
            _ => None,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_language_maps_to_open_page() {
        assert_eq!(IntentRouter::resolve("打开任务"), Some(Intent::OpenPage(PageKind::Tasks)));
        assert_eq!(IntentRouter::resolve("看看有什么要我确认的"), Some(Intent::OpenPage(PageKind::Permission)));
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
