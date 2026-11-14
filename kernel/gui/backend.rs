//! 真实窗口后端 —— winit 事件循环 + softbuffer 软件光栅输出。
//!
//! 只有启用 `gui` feature 时才编译。它把 [`Compositor`] 合成出的 32bpp
//! 表面直接拷进窗口后备缓冲，因此「终端预览 / BMP 导出 / 真实窗口」
//! 三者共享同一份像素，不存在两套渲染代码。
//!
//! 输入方向相反：winit 事件被翻译成 [`crate::gui::input::Event`]，
//! 走和 shell 命令完全相同的 `dispatch` 路径。

use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window as WinitWindow, WindowId};

use super::input::{Effect, Event, Key, MouseButton};
use super::{desktop, ensure_demo, inject, record_command};
use super::surface::Surface;

type SoftContext = softbuffer::Context<Arc<WinitWindow>>;
type SoftSurface = softbuffer::Surface<Arc<WinitWindow>, Arc<WinitWindow>>;

struct App {
    window: Option<Arc<WinitWindow>>,
    context: Option<SoftContext>,
    surface: Option<SoftSurface>,
    /// 最近一次光标的物理像素坐标（winit 的 MouseInput 不带坐标）。
    cursor_px: (f64, f64),
    size_px: (u32, u32),
    last_error: Option<String>,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            context: None,
            surface: None,
            cursor_px: (0.0, 0.0),
            size_px: (super::DEFAULT_WIDTH, super::DEFAULT_HEIGHT),
            last_error: None,
        }
    }

    /// 物理像素 → 合成器坐标。
    fn to_comp(&self, x: f64, y: f64) -> (i32, i32) {
        let d = desktop().lock().unwrap();
        let (cw, ch) = (d.width() as f64, d.height() as f64);
        drop(d);
        let sx = cw / self.size_px.0.max(1) as f64;
        let sy = ch / self.size_px.1.max(1) as f64;
        ((x * sx) as i32, (y * sy) as i32)
    }

    fn handle_effects(effects: Vec<Effect>) -> bool {
        let mut needs_redraw = false;
        for e in effects {
            match e {
                Effect::Command { line, window } => {
                    println!("[gui win#{}] 命令: {}", window, line);
                    record_command(&line);
                }
                Effect::Closed(id) => println!("[gui] 窗口 #{} 已关闭", id),
                Effect::Focused(id) => {
                    if let Some(id) = id {
                        println!("[gui] 焦点 -> #{}", id);
                    }
                }
                Effect::Redraw => needs_redraw = true,
            }
        }
        needs_redraw
    }

    fn redraw(&mut self) {
        let (Some(window), Some(surface)) = (self.window.clone(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        self.size_px = (w, h);
        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            if let Err(e) = surface.resize(nw, nh) {
                self.last_error = Some(format!("resize: {}", e));
                return;
            }
        }
        let frame = {
            let mut d = desktop().lock().unwrap();
            d.compose().clone()
        };
        let scaled = scale_surface(&frame, w, h);
        match surface.buffer_mut() {
            Ok(mut buf) => {
                for (dst, src) in buf.iter_mut().zip(scaled.pixels()) {
                    // softbuffer 要 0x00RRGGBB
                    *dst = *src & 0x00ff_ffff;
                }
                if let Err(e) = buf.present() {
                    self.last_error = Some(format!("present: {}", e));
                }
            }
            Err(e) => self.last_error = Some(format!("buffer: {}", e)),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WinitWindow::default_attributes()
            .with_title("Hib OS — real pixel desktop")
            .with_inner_size(winit::dpi::LogicalSize::new(
                super::DEFAULT_WIDTH,
                super::DEFAULT_HEIGHT,
            ))
            .with_min_inner_size(winit::dpi::LogicalSize::new(320, 240));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.last_error = Some(format!("create_window: {}", e));
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.last_error = Some(format!("softbuffer context: {}", e));
                event_loop.exit();
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                self.last_error = Some(format!("softbuffer surface: {}", e));
                event_loop.exit();
                return;
            }
        };
        self.size_px = (window.inner_size().width, window.inner_size().height);
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        ensure_demo();
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.size_px = (size.width.max(1), size.height.max(1));
                self.redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ScaleFactorChanged { .. } => self.redraw(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_px = (position.x, position.y);
                let (x, y) = self.to_comp(position.x, position.y);
                let eff = inject(Event::MouseMove { x, y });
                let redraw = Self::handle_effects(eff);
                if redraw {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    WinitMouseButton::Left => MouseButton::Left,
                    WinitMouseButton::Right => MouseButton::Right,
                    _ => MouseButton::Middle,
                };
                let (x, y) = self.to_comp(self.cursor_px.0, self.cursor_px.1);
                let ev = if state == ElementState::Pressed {
                    Event::MouseDown { x, y, button: b }
                } else {
                    Event::MouseUp { x, y, button: b }
                };
                let eff = inject(ev);
                let redraw = Self::handle_effects(eff);
                if redraw {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as i32,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as i32,
                };
                let (x, y) = self.to_comp(self.cursor_px.0, self.cursor_px.1);
                let eff = inject(Event::Wheel { x, y, delta: d });
                let redraw = Self::handle_effects(eff);
                if redraw {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let mut keys: Vec<Key> = Vec::new();
                match &event.logical_key {
                    WinitKey::Character(s) => {
                        for ch in s.chars() {
                            keys.push(Key::Char(ch));
                        }
                    }
                    WinitKey::Named(NamedKey::Enter) => keys.push(Key::Enter),
                    WinitKey::Named(NamedKey::Backspace) => keys.push(Key::Backspace),
                    WinitKey::Named(NamedKey::Tab) => keys.push(Key::Tab),
                    WinitKey::Named(NamedKey::Escape) => keys.push(Key::Escape),
                    WinitKey::Named(NamedKey::ArrowUp) => keys.push(Key::Up),
                    WinitKey::Named(NamedKey::ArrowDown) => keys.push(Key::Down),
                    WinitKey::Named(NamedKey::ArrowLeft) => keys.push(Key::Left),
                    WinitKey::Named(NamedKey::ArrowRight) => keys.push(Key::Right),
                    WinitKey::Named(NamedKey::Space) => keys.push(Key::Char(' ')),
                    _ => {
                        if let Some(text) = &event.text {
                            for ch in text.chars() {
                                keys.push(Key::Char(ch));
                            }
                        }
                    }
                }
                for key in keys {
                    let eff = inject(Event::KeyDown(key));
                    Self::handle_effects(eff);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // 事件驱动刷新：只有 EventBus 上出现新事件（由能力层 / Agent 发布）
        // 时才重画实时视图，不再每 400ms 无条件全量刷新。
        if super::refresh_live_if_dirty() {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        // 心跳仅用于把事件循环从 Wait 中唤醒，好让其它线程发布的事件能被
        // GUI 线程看到；它本身不读取状态、不重画，开销可忽略。
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(120),
        ));
    }
}

/// 最近邻缩放（合成器分辨率 → 窗口物理像素）。
pub fn scale_surface(src: &Surface, w: u32, h: u32) -> Surface {
    if src.width() == w && src.height() == h {
        return src.clone();
    }
    let mut out = Surface::new(w.max(1), h.max(1));
    let sw = src.width().max(1) as u64;
    let sh = src.height().max(1) as u64;
    for y in 0..out.height() {
        let sy = (y as u64 * sh / out.height().max(1) as u64) as i32;
        for x in 0..out.width() {
            let sx = (x as u64 * sw / out.width().max(1) as u64) as i32;
            if let Some(p) = src.get(sx, sy) {
                out.put(x as i32, y as i32, p);
            }
        }
    }
    out
}

/// 打开真实窗口，阻塞到用户关闭。
pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("创建事件循环失败: {}", e))?;
    let mut app = App::new();
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("事件循环失败: {}", e))?;
    if let Some(e) = app.last_error.take() {
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::surface::palette;

    #[test]
    fn scaling_keeps_geometry() {
        let s = Surface::filled(4, 4, palette::BLUE);
        let big = scale_surface(&s, 8, 8);
        assert_eq!(big.width(), 8);
        assert_eq!(big.height(), 8);
        assert_eq!(big.get(0, 0), Some(palette::BLUE));
        let same = scale_surface(&s, 4, 4);
        assert_eq!(same.get(3, 3), Some(palette::BLUE));
    }

    #[test]
    fn scaling_down_keeps_content() {
        let s = Surface::filled(100, 50, palette::GREEN);
        let small = scale_surface(&s, 10, 5);
        assert_eq!(small.pixels().len(), 50);
        assert!(small.pixels().iter().all(|p| *p == palette::GREEN));
    }
}
