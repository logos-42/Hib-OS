//! 自然语言对话入口 —— Dock 的对话图标展开输入面板，用户用日常语言说出意图。
//!
//! 反馈使用自然语言 + 语义页面，不回显命令、路径或错误码。高风险操作返回
//! 权限确认请求，说明「为什么需要」与「允许后会发生什么」。

use super::intent::{AuthLevel, Intent, IntentRouter, PageKind};

#[derive(Clone, Debug)]
pub struct Reply {
    pub feedback: String,
    pub page: Option<PageKind>,
    pub permission: Option<(String, AuthLevel)>,
}

impl Reply {
    fn plain(feedback: impl Into<String>) -> Self {
        Self { feedback: feedback.into(), page: None, permission: None }
    }
}

#[derive(Debug, Default)]
pub struct Conversation;

impl Conversation {
    pub fn new() -> Self {
        Self
    }

    /// 提交一句自然语言，返回面向零基础用户的反馈。
    pub fn submit(&mut self, text: &str) -> Reply {
        match IntentRouter::resolve(text) {
            Some(Intent::OpenPage(page)) => Reply {
                feedback: format!("已为你打开{}", page.label()),
                page: Some(page),
                permission: None,
            },
            Some(Intent::ReturnDesktop) => Reply::plain("好的，回到桌面"),
            Some(Intent::NextPage) => Reply::plain("已切换到下一个页面"),
            Some(Intent::PrevPage) => Reply::plain("已切换到上一个页面"),
            Some(Intent::ToggleDock) => Reply::plain("已展开底部图标"),
            Some(Intent::OpenTerminal) => Reply::plain("已为你打开终端"),
            Some(Intent::GenerateApp(_)) | Some(Intent::GeneratePlugin(_))
            | Some(Intent::GenerateIme(_)) | Some(Intent::GenerateAnimation(_)) => {
                Reply::plain("可以，请在桌面右键的 AI 浮层里说出需求，我会为你生成。")
            }
            Some(Intent::RequestPermission { what, level }) => Reply {
                feedback: format!("“{}”需要你的确认", what),
                page: None,
                permission: Some((what, level)),
            },
            Some(Intent::Unknown(s)) => {
                Reply::plain(format!("我还没学会“{}”，你可以说“打开任务”或“返回桌面”", s))
            }
            None => Reply::plain("我没太听懂，可以说“打开任务”“打开文件”或“返回桌面”"),
        }
    }

    /// 权限确认面板文案（为什么需要 + 允许后会发生什么）。
    pub fn confirm_message(what: &str, level: AuthLevel) -> String {
        let scope = match level {
            AuthLevel::Once => "仅这一次",
            AuthLevel::Session => "本次使用期间",
            AuthLevel::Persistent => "始终允许",
        };
        format!("你准备允许：{}（{}）。允许后系统会立即执行该操作，你可以随时在权限页面收回。", what, scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_resolves_natural_language() {
        let mut c = Conversation::new();
        let out = c.submit("看看有什么要我确认的");
        assert!(out.feedback.contains("权限"));
        assert_eq!(out.page, Some(PageKind::Permission));
    }

    #[test]
    fn conversation_reports_pending_permission() {
        let mut c = Conversation::new();
        let out = c.submit("待授权");
        assert!(out.feedback.contains("权限"));
    }

    #[test]
    fn high_risk_requires_confirmation() {
        let mut c = Conversation::new();
        let out = c.submit("删除所有文件");
        assert!(out.permission.is_some());
        assert!(out.feedback.contains("确认"));
    }

    #[test]
    fn unknown_input_is_friendly() {
        let mut c = Conversation::new();
        let out = c.submit("随便说点什么");
        assert!(out.feedback.contains("没太听懂"));
    }

    #[test]
    fn confirm_message_explains_scope() {
        let msg = Conversation::confirm_message("删除所有文件", AuthLevel::Once);
        assert!(msg.contains("仅这一次"));
        assert!(msg.contains("收回"));
    }
}
