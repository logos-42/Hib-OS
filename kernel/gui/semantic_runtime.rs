//! 语义 GUI 运行时 —— 把 `gui_ai.rs` 的语义窗口/控件桥接到真实像素栈。
//!
//! `GUAI` 持有语义真相，现有 `Compositor` 只负责光栅输出；本模块维护
//! `SemanticWindow.id ↔ gui::Window.id` 双轨映射，并把 AIGWidget 绘制到窗口客户端表面。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::ai_os::gui_ai::{self, WidgetType};

#[derive(Clone, Debug)]
pub struct WidgetView {
    pub id: u64,
    pub kind: WidgetType,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub enabled: bool,
    pub visible: bool,
    pub window_id: Option<u64>,
}

pub struct SemanticRuntime {
    pub id_map: BTreeMap<u64, u64>,
    pub reverse: BTreeMap<u64, u64>,
    dirty: bool,
}

impl SemanticRuntime {
    pub fn new() -> Self {
        Self { id_map: BTreeMap::new(), reverse: BTreeMap::new(), dirty: false }
    }

    /// 创建语义窗口并同步创建真实光栅窗口。
    pub fn create_semantic_window(&mut self, title: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        let sem_id = gui_ai::gui_ai().lock().unwrap().create_window(title, x, y, w, h);
        let raster_id = super::open_window(title, w, h);
        if let Some(win) = super::desktop().lock().unwrap().window_mut(raster_id) {
            win.move_to(x, y);
            win.repaint();
        }
        self.id_map.insert(sem_id, raster_id);
        self.reverse.insert(raster_id, sem_id);
        self.dirty = true;
        sem_id
    }

    pub fn raster_id(&self, sem_id: u64) -> Option<u64> {
        self.id_map.get(&sem_id).copied()
    }

    pub fn semantic_id(&self, raster_id: u64) -> Option<u64> {
        self.reverse.get(&raster_id).copied()
    }

    pub fn raster_window_exists(&self, id: u64) -> bool {
        super::desktop().lock().unwrap().window(id).is_some()
    }

    /// 添加语义控件（不直接绘制）。
    pub fn add_widget(
        &mut self,
        kind: WidgetType,
        label: &str,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        window_id: Option<u64>,
    ) -> u64 {
        let id = gui_ai::gui_ai().lock().unwrap().add_widget(kind, label, x, y, w, h, window_id);
        self.dirty = true;
        id
    }

    /// 读取控件快照（避免跨锁持有引用）。
    pub fn widget_view(&self, id: u64) -> Option<WidgetView> {
        let eng = gui_ai::gui_ai().lock().unwrap();
        eng.widgets.get(&id).map(|w| WidgetView {
            id: w.id,
            kind: w.r#type,
            label: w.label.clone(),
            x: w.x,
            y: w.y,
            w: w.w,
            h: w.h,
            enabled: w.enabled,
            visible: w.visible,
            window_id: w.window_id,
        })
    }

    /// 把控件画到其所属光栅窗口客户端表面。
    pub fn paint_widget(&mut self, id: u64) -> bool {
        let view = match self.widget_view(id) {
            Some(v) => v,
            None => return false,
        };
        if !view.visible {
            return false;
        }
        let raster = view.window_id.and_then(|sem| self.raster_id(sem));
        if let Some(rid) = raster {
            let mut d = super::desktop().lock().unwrap();
            if let Some(win) = d.window_mut(rid) {
                let bg = if view.enabled { 0xFF2B2B2B } else { 0xFF1C1C1C };
                let fg = if view.enabled { 0xFFEDEDED } else { 0xFF777777 };
                win.client.fill_rect(view.x, view.y, view.w, view.h, bg);
                win.client.draw_text(view.x + 6, view.y + 6, &view.label, fg, 1);
            }
        }
        self.dirty = true;
        true
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }
}

impl Default for SemanticRuntime {
    fn default() -> Self {
        Self::new()
    }
}

static RUNTIME: OnceLock<Mutex<SemanticRuntime>> = OnceLock::new();

pub fn runtime() -> &'static Mutex<SemanticRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(SemanticRuntime::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_window_maps_to_raster_window() {
        let mut rt = SemanticRuntime::new();
        let sem = rt.create_semantic_window("TASKS", 100, 100, 400, 300);
        let raster = rt.raster_id(sem).expect("mapping");
        assert!(rt.raster_window_exists(raster));
        assert_eq!(rt.semantic_id(raster), Some(sem));
    }

    #[test]
    fn widget_render_marks_dirty() {
        let mut rt = SemanticRuntime::new();
        let wid = rt.add_widget(WidgetType::Button, "OK", 10, 10, 80, 30, None);
        assert!(rt.paint_widget(wid));
        assert!(rt.dirty());
    }

    #[test]
    fn missing_widget_is_noop() {
        let mut rt = SemanticRuntime::new();
        assert!(!rt.paint_widget(u64::MAX));
    }
}
