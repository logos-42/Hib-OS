//! 桌面外壳 —— `gui://shell`。
//!
//! GUI 内核（窗口 / 输入 / 合成）已经具备，这里补上**桌面环境层**：
//! 启动器、剪贴板、DPI、多显示器、焦点策略，以及两处闭环：
//!
//! * **输入 → 通道**：把一次点击翻译成一条可执行的 `act` 建议（命中窗口/启动器）；
//! * **合成 → GPU**：把合成器输出的窗口信息光栅到 `gpu://framebuffer`，
//!   让软件合成结果与 GPU 帧缓冲走同一份可感知像素。
//!
//! * `sense gui://shell`
//! * `act gui://shell launch <应用>`
//! * `act gui://shell clipboard set <文本>` / `clipboard get`
//! * `act gui://shell dpi <n>` / `monitor add <名> <宽>x<高>` / `focus <策略>`
//! * `act gui://shell route <x> <y>` / `compose`

use std::sync::{Mutex, OnceLock};

use crate::desktop;
use crate::gui;

/// 一台显示器。
#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

pub struct DesktopShell {
    pub clipboard: String,
    pub dpi: u32,
    pub monitors: Vec<Monitor>,
    pub launcher: Vec<String>,
    /// 焦点策略：click / follow-mouse
    pub focus_policy: String,
    pub routed: u64,
    pub composed: u64,
}

impl DesktopShell {
    fn new() -> Self {
        Self {
            clipboard: String::new(),
            dpi: 96,
            monitors: vec![Monitor {
                name: "main".to_string(),
                width: gui::DEFAULT_WIDTH,
                height: gui::DEFAULT_HEIGHT,
                scale: 100,
            }],
            launcher: vec!["terminal".into(), "files".into(), "monitor".into(), "settings".into()],
            focus_policy: "click".to_string(),
            routed: 0,
            composed: 0,
        }
    }

    pub fn status(&self) -> String {
        let mon: Vec<String> = self
            .monitors
            .iter()
            .map(|m| format!("{} {}x{}@{}%", m.name, m.width, m.height, m.scale))
            .collect();
        format!(
            "dpi={} focus={} clipboard={}B routed={} composed={}\nmonitors: {}\nlauncher: {}\nwindows: {}",
            self.dpi,
            self.focus_policy,
            self.clipboard.len(),
            self.routed,
            self.composed,
            mon.join(", "),
            self.launcher.join(", "),
            gui::desktop().lock().unwrap().windows().len()
        )
    }

    /// 启动一个应用：真的开一个窗口，并返回窗口 id。
    pub fn launch(&mut self, app: &str) -> Result<String, String> {
        if app.trim().is_empty() {
            return Err("用法: launch <应用名>".to_string());
        }
        if !self.launcher.iter().any(|a| a == app) {
            self.launcher.push(app.to_string());
        }
        // 优先启动「窗口内运行的应用」；未知名字退回普通像素窗口。
        let id = match gui::open_app(app) {
            Ok(id) => id,
            Err(_) if app == "terminal" => gui::open_terminal(app, 480, 260),
            Err(_) => gui::open_window(app, 360, 220),
        };
        Ok(format!("已启动 {} -> win#{}", app, id))
    }

    pub fn set_dpi(&mut self, n: u32) -> Result<String, String> {
        if !(48..=384).contains(&n) {
            return Err("DPI 必须在 48-384".to_string());
        }
        self.dpi = n;
        for m in &mut self.monitors {
            m.scale = (n * 100) / 96;
        }
        Ok(format!("DPI -> {} (缩放 {}%)", n, (n * 100) / 96))
    }

    pub fn add_monitor(&mut self, name: &str, wh: &str) -> Result<String, String> {
        let (w, h) = wh
            .split_once('x')
            .ok_or_else(|| "尺寸格式应为 <宽>x<高>".to_string())?;
        let w: u32 = w.parse().map_err(|_| "宽度无效".to_string())?;
        let h: u32 = h.parse().map_err(|_| "高度无效".to_string())?;
        if w == 0 || h == 0 {
            return Err("显示器尺寸必须 > 0".to_string());
        }
        if self.monitors.iter().any(|m| m.name == name) {
            return Err(format!("显示器 {} 已存在", name));
        }
        self.monitors.push(Monitor {
            name: name.to_string(),
            width: w,
            height: h,
            scale: (self.dpi * 100) / 96,
        });
        Ok(format!("已接入显示器 {} {}x{}", name, w, h))
    }

    /// 输入闭环：把点击坐标翻译成一条可执行的通道动作。
    pub fn route(&mut self, x: i32, y: i32) -> String {
        self.routed += 1;
        let hit = {
            let d = gui::desktop().lock().unwrap();
            d.hit_test(x, y)
        };
        match hit {
            Some((id, _)) => format!("act gui://windows focus {}  (命中 win#{})", id, id),
            None if y < 28 => {
                let app = self.launcher.first().cloned().unwrap_or_default();
                format!("act gui://shell launch {}  (命中启动器)", app)
            }
            None => format!("act gui://shell clipboard set <{}:{}>  (桌面空白处)", x, y),
        }
    }

    /// GPU 合成路径：把合成器的窗口标题光栅到 GPU 帧缓冲。
    pub fn compose_to_gpu(&mut self) -> String {
        // 首次合成时先构建演示桌面，保证有真实窗口参与合成。
        gui::ensure_demo();
        let (titles, lit) = {
            let mut d = gui::desktop().lock().unwrap();
            let frame = d.compose();
            let lit = frame.lit_pixels();
            let titles: Vec<String> = d
                .windows()
                .iter()
                .map(|w| w.title.clone())
                .collect();
            drop(d);
            (titles, lit)
        };
        let mut fb = desktop::framebuffer().lock().unwrap();
        fb.clear();
        let mut y = 1usize;
        for t in &titles {
            fb.draw_text(1, y, t);
            y += 2;
        }
        self.composed += 1;
        format!(
            "GPU 合成完成: {} 个窗口, 合成面点亮 {} 像素, 帧缓冲点亮 {} 像素",
            titles.len(),
            lit,
            fb.lit_pixels()
        )
    }
}

static SHELL: OnceLock<Mutex<DesktopShell>> = OnceLock::new();

pub fn desktop_shell() -> &'static Mutex<DesktopShell> {
    SHELL.get_or_init(|| Mutex::new(DesktopShell::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_status() {
        let s = DesktopShell::new();
        let out = s.status();
        assert!(out.contains("dpi=96"), "{}", out);
        assert!(out.contains("terminal"), "{}", out);
    }

    #[test]
    fn clipboard_and_dpi() {
        let mut s = DesktopShell::new();
        s.clipboard = "hello".to_string();
        assert_eq!(s.clipboard, "hello");
        assert!(s.set_dpi(192).is_ok());
        assert_eq!(s.monitors[0].scale, 200);
        assert!(s.set_dpi(10).is_err());
    }

    #[test]
    fn monitors_and_launcher() {
        let mut s = DesktopShell::new();
        assert!(s.add_monitor("side", "1280x720").is_ok());
        assert!(s.add_monitor("side", "800x600").is_err());
        assert!(s.add_monitor("bad", "oops").is_err());
        assert!(s.launch("files").unwrap().contains("win#"));
        assert!(s.launcher.contains(&"files".to_string()));
    }

    #[test]
    fn input_routing_produces_actions() {
        let mut s = DesktopShell::new();
        let a = s.route(5, 5);
        assert!(a.starts_with("act "), "{}", a);
        let b = s.route(400, 300);
        assert!(b.starts_with("act "), "{}", b);
        assert_eq!(s.routed, 2);
    }

    #[test]
    fn gpu_compose_writes_framebuffer() {
        let mut s = DesktopShell::new();
        gui::ensure_demo();
        let r = s.compose_to_gpu();
        assert!(r.contains("GPU 合成完成"), "{}", r);
        assert_eq!(s.composed, 1);
    }
}
