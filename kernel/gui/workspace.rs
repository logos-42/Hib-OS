//! 全屏工作区 —— 每打开一个新工作区，从桌面展开一扇占据全屏的页面。
//!
//! 工作区是「语义页面」而不是桌面里的小窗口：它覆盖整个画布，顶栏仍保留；
//! 通过 Esc / 顶部下滑 / 左上图标返回桌面；左右滑动或 ⌘Tab 在页面间切换。

use std::sync::{Mutex, OnceLock};

use super::intent::{Intent, PageKind};
use super::surface::Surface;

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
        if let Some(page) = self.current() {
            frame.fill_rect(0, 0, frame.width(), frame.height(), 0xFF141518);
            frame.draw_text(40, 40, page.kind.label(), 0xFFE6E8EC, 2);
            frame.fill_rect(40, 70, 120, 2, 0xFF7FB3A0);
        }
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
}
