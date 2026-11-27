//! 全屏工作区 —— 每打开一个新工作区，从桌面展开一扇占据全屏的页面。
//!
//! 工作区是「语义页面」而不是桌面里的小窗口：它覆盖整个画布，顶栏仍保留；
//! 通过 Esc / 顶部下滑 / 左上图标返回桌面；左右滑动或 ⌘Tab 在页面间切换。
//!
//! 页面内容来自真实内核数据（Agent 状态 / 权限 / 文件系统 / 系统监控），
//! 且全部用 ASCII 短标签渲染 —— 位图字体只覆盖 ASCII，中文标签会显示成方块。

use std::sync::{Mutex, OnceLock};

use super::intent::{Intent, PageKind};
use super::surface::Surface;
use super::text_render;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub kind: PageKind,
    pub fullscreen: bool,
    pub bounds: (i32, i32, u32, u32),
}

#[derive(Debug)]
pub struct WorkspaceManager {
    pub pages: Vec<Workspace>,
    pub current: Option<usize>,
}

impl WorkspaceManager {
    pub fn new() -> Self {
        Self { pages: Vec::new(), current: None }
    }

    pub fn open(&mut self, kind: PageKind, width: u32, height: u32) -> usize {
        let page = Workspace { kind, fullscreen: true, bounds: (0, 0, width, height) };
        self.pages.push(page);
        self.current = Some(self.pages.len() - 1);
        self.current.unwrap()
    }

    pub fn current(&self) -> Option<&Workspace> {
        self.current.and_then(|i| self.pages.get(i))
    }

    pub fn current_kind(&self) -> Option<PageKind> {
        self.current().map(|w| w.kind)
    }

    pub fn handle_intent(&mut self, intent: Intent) -> bool {
        match intent {
            Intent::ReturnDesktop => {
                self.current = None;
                true
            }
            Intent::NextPage if self.current.is_some() && !self.pages.is_empty() => {
                self.current = Some((self.current.unwrap() + 1) % self.pages.len());
                true
            }
            Intent::PrevPage if self.current.is_some() && !self.pages.is_empty() => {
                let len = self.pages.len();
                self.current = Some((self.current.unwrap() + len - 1) % len);
                true
            }
            Intent::OpenPage(kind) => {
                self.open(kind, 960, 600);
                true
            }
            _ => false,
        }
    }

    /// 把当前全屏页面绘制到帧缓冲（在窗口层之上、外壳之下）。
    pub fn draw(&self, frame: &mut Surface) {
        let Some(page) = self.current() else {
            return;
        };
        // 页面底色：近黑，避免抢走内容。
        frame.fill_rect(0, 0, frame.width(), frame.height(), 0xFF161922);

        // 顶部：页面标题 + 强调色细线。
        let title = page.kind.label_en();
        text_render::draw_text(
            frame,
            40,
            30,
            title,
            20.0,
            text_render::FG,
            frame.width() as i32 - 40,
        );
        frame.fill_rect(40, 58, 140, 2, text_render::ACCENT);

        // 内容区：真实数据行，逐行绘制（行距 16px，适配 5x7 字形）。
        let lines = page_lines(page.kind);
        let mut y = 76;
        for line in lines {
            text_render::draw_text(
                frame,
                40,
                y,
                &line,
                12.0,
                0xFFB9BCC4,
                frame.width() as i32 - 40,
            );
            y += 19;
        }
    }
}

/// 按页面类型收集真实内核数据，返回可直接渲染的 ASCII 行。
fn page_lines(kind: PageKind) -> Vec<String> {
    match kind {
        PageKind::Conversation => vec![
            "Say what you want to do.".to_string(),
            "Try: \"open tasks\" / \"show permissions\" / \"back to desktop\".".to_string(),
            String::new(),
            "This page never shows terminal commands.".to_string(),
        ],
        PageKind::Tasks => {
            use crate::ai_os::agent_proc::{agents, AgentPcbState};
            let t = agents().lock().unwrap_or_else(|e| e.into_inner());
            let total = t.iter().count();
            let ready = t.state_count(AgentPcbState::Ready);
            let running = t.state_count(AgentPcbState::Running);
            let blocked = t.state_count(AgentPcbState::Blocked);
            vec![
                format!("agents total  {}", total),
                format!("ready         {}", ready),
                format!("running       {}", running),
                format!("waiting       {}", blocked),
            ]
        }
        PageKind::Permission => {
            let pm = crate::permission::permission_manager()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let agents = pm.list_agents();
            let pending = super::notification::unread_notifications();
            vec![
                format!("agents with permissions  {}", agents.len()),
                format!("notifications waiting   {}", pending),
                String::new(),
                "High-risk actions always ask for your approval.".to_string(),
            ]
        }
        PageKind::Files => {
            let stats = crate::fs::vfs::vfs_stats();
            let mut lines: Vec<String> = stats
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().to_string())
                .take(8)
                .collect();
            if lines.is_empty() {
                lines.push("(no files yet)".to_string());
            }
            lines
        }
        PageKind::Observation => {
            let s = crate::monitor::get_stats();
            let up_s = s.system_uptime_ns / 1_000_000_000;
            vec![
                format!("uptime        {}s", up_s),
                format!("tokens live   {}", s.active_tokens),
                format!("tasks done    {}", s.total_tasks_completed),
                format!("tasks failed  {}", s.total_tasks_failed),
            ]
        }
        PageKind::Settings => vec![
            "Appearance: minimal, dark.".to_string(),
            "Dock: always visible (auto-hide optional).".to_string(),
            "More settings coming soon.".to_string(),
        ],
        PageKind::Help => vec![
            "Click a Dock icon to open a full-screen page.".to_string(),
            "Esc or the top-left icon returns to the desktop.".to_string(),
            "Swipe left/right or Cmd+Tab to switch pages.".to_string(),
        ],
        PageKind::Advanced => vec![
            "Advanced tools live here, not on the main screen.".to_string(),
            "Press Cmd+K for the command palette.".to_string(),
        ],
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

static WORKSPACES: OnceLock<Mutex<WorkspaceManager>> = OnceLock::new();

pub fn workspaces() -> &'static Mutex<WorkspaceManager> {
    WORKSPACES.get_or_init(|| Mutex::new(WorkspaceManager::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_page_goes_fullscreen() {
        let mut mgr = WorkspaceManager::new();
        mgr.open(PageKind::Tasks, 960, 600);
        let page = mgr.current().unwrap();
        assert!(page.fullscreen);
        assert_eq!(page.bounds, (0, 0, 960, 600));
    }

    #[test]
    fn escape_returns_desktop() {
        let mut mgr = WorkspaceManager::new();
        mgr.open(PageKind::Tasks, 960, 600);
        assert!(mgr.handle_intent(Intent::ReturnDesktop));
        assert!(mgr.current().is_none());
    }

    #[test]
    fn next_prev_cycles_pages() {
        let mut mgr = WorkspaceManager::new();
        mgr.open(PageKind::Tasks, 960, 600);
        mgr.open(PageKind::Files, 960, 600);
        assert_eq!(mgr.current_kind(), Some(PageKind::Files));
        mgr.handle_intent(Intent::NextPage);
        assert_eq!(mgr.current_kind(), Some(PageKind::Tasks));
        mgr.handle_intent(Intent::PrevPage);
        assert_eq!(mgr.current_kind(), Some(PageKind::Files));
    }

    #[test]
    fn every_page_has_real_lines() {
        for kind in [
            PageKind::Conversation,
            PageKind::Tasks,
            PageKind::Permission,
            PageKind::Files,
            PageKind::Observation,
            PageKind::Settings,
            PageKind::Help,
            PageKind::Advanced,
        ] {
            let lines = page_lines(kind);
            assert!(!lines.is_empty(), "{:?} 应有内容", kind);
        }
    }

    #[test]
    fn draw_marks_page_background() {
        let mut mgr = WorkspaceManager::new();
        mgr.open(PageKind::Help, 400, 300);
        let mut s = Surface::new(400, 300);
        mgr.draw(&mut s);
        assert_eq!(s.get(5, 5), Some(0xFF161922));
    }
}
