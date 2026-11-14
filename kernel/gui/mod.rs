//! GUI 子系统 —— 真窗口、真像素、真输入。
//!
//! 这是对旧「80×25 字符假桌面」的替换路径：文本桌面仍然保留（SSH / 无图形
//! 环境可用），但 `gui` 命令走的是完全不同的栈：
//!
//! ```text
//!  输入事件 ──► Compositor(窗口/z-order/焦点) ──► Surface(32bpp) ──► BMP / softbuffer
//! ```
//!
//! 三条硬约束：
//! 1. 每个窗口拥有自己的客户端表面，不是往共享字符网格上「画框」。
//! 2. 所有坐标都是像素，合成器输出可直接作为真实窗口的后备缓冲。
//! 3. 没有「预览专用代码」：终端里看到的 ANSI 预览和导出的 BMP、
//!    以及 `gui run` 打开的真实窗口，用的是同一份像素。

pub mod apps;
pub mod agent_workspace;
pub mod bmp;
pub mod compositor;
pub mod compositor_ext;
pub mod file_manager;
pub mod gesture;
pub mod input;
pub mod notification;
pub mod permission_center;
pub mod png;
pub mod shell;
pub mod system_monitor;
pub mod task_workspace;
pub mod taskbar;
pub mod terminal;
pub mod theme;
pub mod settings;
pub mod surface;
pub mod wallpaper;
pub mod widget;
pub mod window;
pub mod wm;

#[cfg(feature = "gui")]
pub mod backend;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::event::{
    event_bus, EventFilter, EventSubscriber, SystemEvent, SystemEventType,
};

use compositor::{Compositor, Wallpaper};
use surface::{argb, palette, Surface};
use window::Window;

use crate::exec;
use crate::gpu::framebuffer::{ADVANCE, LINE_HEIGHT};

/// 默认显示分辨率。
pub const DEFAULT_WIDTH: u32 = 960;
pub const DEFAULT_HEIGHT: u32 = 600;

/// 全局桌面（显示服务器）。
pub fn desktop() -> &'static Mutex<Compositor> {
    static DESKTOP: OnceLock<Mutex<Compositor>> = OnceLock::new();
    DESKTOP.get_or_init(|| Mutex::new(Compositor::new(DEFAULT_WIDTH, DEFAULT_HEIGHT)))
}

/// 全局输入队列（`input/kbd` / `input/mouse` 的实体）。
pub fn input_queue() -> &'static Mutex<input::InputQueue> {
    static QUEUE: OnceLock<Mutex<input::InputQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(input::InputQueue::new(1024)))
}

/// 注入一个事件：先入队，再交给合成器路由。返回副作用。
pub fn inject(ev: input::Event) -> Vec<input::Effect> {
    {
        let mut q = input_queue().lock().unwrap();
        q.push(ev.clone());
    }
    dispatch_one(&ev)
}

/// 处理队首事件（供真实窗口后端的轮询循环）。
pub fn pump() -> Vec<input::Effect> {
    let ev = match input_queue().lock().unwrap().pop() {
        Some(e) => e,
        None => return Vec::new(),
    };
    dispatch_one(&ev)
}

fn dispatch_one(ev: &input::Event) -> Vec<input::Effect> {
    let mut d = desktop().lock().unwrap();
    input::dispatch(&mut d, ev)
}

/// 扫描并处理所有排队事件。
pub fn pump_all() -> Vec<input::Effect> {
    let mut effects = Vec::new();
    loop {
        {
            let q = input_queue().lock().unwrap();
            if q.is_empty() {
                break;
            }
        }
        let e = pump();
        if e.is_empty() {
            break;
        }
        effects.extend(e);
    }
    effects
}

/// 确保桌面已初始化（首次调用会构建演示桌面）。
pub fn ensure_demo() {
    // GUI 一启动就订阅系统事件，实时视图由事件驱动而不是定时轮询。
    init_event_subscription();
    let mut d = desktop().lock().unwrap();
    if d.windows().is_empty() {
        build_demo(&mut d);
    }
}

/// 构建一套能证明各子系统都在跑的演示桌面。
///
/// 布局按 960×600 屏幕手工排过：四扇窗全部落在可视区域内且互不压边。
pub fn build_demo(comp: &mut Compositor) {
    comp.set_wallpaper(Wallpaper::Nebula);
    let (sw, sh) = (comp.width() as i32, comp.height() as i32);

    // 1. 控制台（键盘输入闭环）
    let console = comp.open_terminal("HIB OS CONSOLE", 460, 290, "hib> ");
    if let Some(w) = comp.window_mut(console) {
        w.move_to(48, 56);
        if let Some(t) = w.terminal.as_mut() {
            t.push_line(concat!(
                "Hib OS ",
                crate::version::hibos_version!(),
                " / real pixel compositor"
            ));
            t.push_line("type a command, press ENTER");
            t.push_line("channels: exec sync lease agent gui");
            t.push_line("hello, operator.");
        }
        w.repaint();
    }

    // 2. 执行体视图（实时）
    let bodies = comp.open("EXEC BODIES", 400, 250, palette::TEAL);
    if let Some(w) = comp.window_mut(bodies) {
        w.move_to(sw - 400 - 32, 48);
        paint_lines(w, &exec_lines());
    }

    // 3. 子智能体视图（实时）
    let agents = comp.open("AGENT ORCHESTRATOR", 400, 220, palette::MAUVE);
    if let Some(w) = comp.window_mut(agents) {
        w.move_to(sw - 400 - 32, 316);
        paint_lines(w, &agent_lines());
    }

    // 4. 通道租约 / 世界版本
    let leases = comp.open("LEASE / EPOCH", 452, 190, palette::PEACH);
    if let Some(w) = comp.window_mut(leases) {
        w.move_to(48, sh - 190 - 40);
        paint_lines(w, &lease_lines());
    }

    // 5. Agent Workspace
    build_agent_workspace(comp, 48, 48, 400, 220);

    // 6. Task Workspace
    build_task_workspace(comp, 460, 48, 460, 200);

    // 7. Permission Center
    build_permission_center(comp, 48, 280, 400, 180);

    comp.focus(console);
}

/// 构建 Agent Workspace 窗口：列出全部智能体及其状态。
pub fn build_agent_workspace(comp: &mut Compositor, x: i32, y: i32, w: u32, h: u32) {
    let id = comp.open("AGENT WORKSPACE", w, h, palette::MAUVE);
    if let Some(win) = comp.window_mut(id) {
        win.move_to(x, y);
        let mut lines = vec![
            "=== Agent Workspace ===".to_string(),
            String::new(),
        ];
        lines.push(crate::agent::status());
        lines.push(String::new());
        for line in crate::agent::list().lines() {
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        lines.push(String::new());
        lines.push("操作: shell 输入 `agent list / inspect <id> / start <id> / stop <id>`".to_string());
        paint_lines(win, &lines);
    }
}

/// 构建 Task Workspace 窗口：列出全部任务及其状态。
pub fn build_task_workspace(comp: &mut Compositor, x: i32, y: i32, w: u32, h: u32) {
    let id = comp.open("TASK WORKSPACE", w, h, palette::TEAL);
    if let Some(win) = comp.window_mut(id) {
        win.move_to(x, y);
        let mut lines = vec![
            "=== Task Workspace ===".to_string(),
            String::new(),
        ];
        // 通过统一能力层查询任务
        let req = crate::capability::CapabilityRequest {
            id: "task".to_string(),
            input: serde_json::json!({"op": "list"}),
            context: crate::capability::ExecutionContext {
                caller_id: "gui".to_string(),
                caller_type: crate::capability::CallerType::Gui,
                permissions: vec!["task:read".to_string()],
                resource_quota: crate::capability::ResourceQuota::default(),
            },
            timeout_ns: None,
            cancellation_token: None,
            audit: crate::capability::AuditInfo {
                actor: "gui".to_string(),
                action: "task.list".to_string(),
                timestamp: crate::capability::now_ns(),
                result: crate::capability::AuditResult::Executed,
                correlation_id: None,
            },
        };
        let reg = crate::capability::capability_registry();
        if let Some(cap) = reg.read().unwrap().get("task") {
            match cap.execute(req) {
                Ok(resp) => {
                    if let Some(tasks) = resp.output.get("tasks").and_then(|v| v.as_array()) {
                        for t in tasks {
                            let id = t.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                            let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
                            let state = t.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                            lines.push(format!("  [{}] {} ({})", id, desc, state));
                        }
                    }
                }
                Err(e) => lines.push(format!("错误: {}", e.message)),
            }
        } else {
            lines.push("能力 task 未注册".to_string());
        }
        lines.push(String::new());
        lines.push("操作: shell 输入 `task create <描述> / cancel <id> / pause <id> / resume <id>`".to_string());
        paint_lines(win, &lines);
    }
}

/// 构建 Permission Center 窗口：列出权限与审计日志。
pub fn build_permission_center(comp: &mut Compositor, x: i32, y: i32, w: u32, h: u32) {
    let id = comp.open("PERMISSION CENTER", w, h, palette::PEACH);
    if let Some(win) = comp.window_mut(id) {
        win.move_to(x, y);
        let mut lines = vec![
            "=== Permission Center ===".to_string(),
            String::new(),
        ];
        // 列出全部智能体及其权限
        for summary in crate::permission::permission_manager().lock().unwrap().list_agents() {
            lines.push(format!(
                "Agent {}: {} [{}] state={} caps={}",
                summary.agent_id, summary.name, summary.role,
                summary.state.name(),
                summary.capabilities.join(", ")
            ));
        }
        lines.push(String::new());
        lines.push("--- 最近审计 ---".to_string());
        for entry in crate::permission::permission_manager().lock().unwrap().query_audit(Some(10)) {
            lines.push(format!(
                "  {} {} {} -> {:?}",
                entry.timestamp, entry.actor, entry.action, entry.result
            ));
        }
        lines.push(String::new());
        lines.push("操作: shell 输入 `system logs` 查看更多审计".to_string());
        paint_lines(win, &lines);
    }
}

/// 刷新工作区窗口的实时内容。
pub fn refresh_workspaces(comp: &mut Compositor) {
    // 先收集需要刷新的窗口 ID，避免借用冲突
    let refresh_ids: Vec<u64> = comp
        .windows()
        .iter()
        .filter(|w| {
            matches!(
                w.title.as_str(),
                "AGENT WORKSPACE" | "TASK WORKSPACE" | "PERMISSION CENTER"
            )
        })
        .map(|w| w.id)
        .collect();

    for id in refresh_ids {
        let title = comp.window(id).map(|w| w.title.clone()).unwrap_or_default();
        match title.as_str() {
            "AGENT WORKSPACE" => {
                if let Some(win) = comp.window_mut(id) {
                    let mut lines = vec!["=== Agent Workspace ===".to_string(), String::new()];
                    lines.push(crate::agent::status());
                    lines.push(String::new());
                    for line in crate::agent::list().lines() {
                        if !line.is_empty() {
                            lines.push(line.to_string());
                        }
                    }
                    paint_lines(win, &lines);
                }
            }
            "TASK WORKSPACE" => {
                if let Some(win) = comp.window_mut(id) {
                    let mut lines = vec!["=== Task Workspace ===".to_string(), String::new()];
                    let req = crate::capability::CapabilityRequest {
                        id: "task".to_string(),
                        input: serde_json::json!({"op": "list"}),
                        context: crate::capability::ExecutionContext {
                            caller_id: "gui".to_string(),
                            caller_type: crate::capability::CallerType::Gui,
                            permissions: vec!["task:read".to_string()],
                            resource_quota: crate::capability::ResourceQuota::default(),
                        },
                        timeout_ns: None,
                        cancellation_token: None,
                        audit: crate::capability::AuditInfo {
                            actor: "gui".to_string(),
                            action: "task.list".to_string(),
                            timestamp: crate::capability::now_ns(),
                            result: crate::capability::AuditResult::Executed,
                            correlation_id: None,
                        },
                    };
                    let reg = crate::capability::capability_registry();
                    if let Some(cap) = reg.read().unwrap().get("task") {
                        if let Ok(resp) = cap.execute(req) {
                            if let Some(tasks) = resp.output.get("tasks").and_then(|v| v.as_array())
                            {
                                for t in tasks {
                                    let id = t.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
                                    let state = t.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                                    lines.push(format!("  [{}] {} ({})", id, desc, state));
                                }
                            }
                        }
                    }
                    lines.push(String::new());
                    lines.push("操作: shell 输入 `task ...` 管理任务".to_string());
                    paint_lines(win, &lines);
                }
            }
            "PERMISSION CENTER" => {
                if let Some(win) = comp.window_mut(id) {
                    let mut lines = vec!["=== Permission Center ===".to_string(), String::new()];
                    for summary in crate::permission::permission_manager().lock().unwrap().list_agents()
                    {
                        lines.push(format!(
                            "Agent {}: {} [{}] state={} caps={}",
                            summary.agent_id, summary.name, summary.role,
                            summary.state.name(),
                            summary.capabilities.join(", ")
                        ));
                    }
                    lines.push(String::new());
                    lines.push("--- 最近审计 ---".to_string());
                    for entry in crate::permission::permission_manager().lock().unwrap().query_audit(Some(10))
                    {
                        lines.push(format!(
                            "  {} {} {} -> {:?}",
                            entry.timestamp, entry.actor, entry.action, entry.result
                        ));
                    }
                    paint_lines(win, &lines);
                }
            }
            _ => {}
        }
    }
}

fn exec_lines() -> Vec<String> {
    let s = exec::snapshot();
    vec![
        format!("bodies    {}", s.total),
        format!("running   {}", s.running),
        format!("blocked   {}", s.blocked),
        format!("suspended {}", s.suspended),
        format!("waiters   {}", s.waiters),
        format!("wakeups   {}", s.wakeups),
        format!("checkpts  {}", s.checkpoints),
        String::new(),
        "wait_table is the scheduler's truth".to_string(),
    ]
}

fn agent_lines() -> Vec<String> {
    let status = crate::agent::status();
    let mut lines = vec![status];
    let list = crate::agent::list();
    for l in list.lines().take(4) {
        lines.push(l.to_string());
    }
    lines
}

fn lease_lines() -> Vec<String> {
    let raw = crate::lease::status();
    // 字体只覆盖 ASCII：中文占位符会渲染成「豆腐块」，这里换成英文等价信息。
    if raw.trim().is_empty() || raw.contains('（') {
        return vec![
            "channels   0".to_string(),
            "epoch      -".to_string(),
            "no write leases yet".to_string(),
            String::new(),
            "acquire -> commit -> release".to_string(),
        ];
    }
    raw.lines().take(6).map(|s| s.to_string()).collect()
}

/// 把文本行画进一个纯像素窗口的客户端表面。
///
/// 与终端一致：先按可用列数折行，再逐视觉行推进 y，避免长行互相覆盖。
pub fn paint_lines(win: &mut Window, lines: &[String]) {
    let bg = palette::BASE;
    win.client.fill(bg);
    let pad = 10i32;
    let lh = LINE_HEIGHT as i32 * win.scale.max(1) as i32;
    let cols = Surface::cols_for_width(
        win.client.width().saturating_sub(pad as u32 * 2),
        win.scale,
    )
    .max(1);
    let max_x = win.client.width() as i32 - pad;
    let mut y = pad;
    'outer: for line in lines {
        for visual in Surface::wrap_text(line, cols, win.scale) {
            if y + lh > win.client.height() as i32 {
                break 'outer;
            }
            let color = if line.contains('=') || line.contains(':') {
                palette::SUBTEXT0
            } else {
                palette::TEXT
            };
            win.client
                .draw_text_clipped(pad, y, &visual, color, win.scale, max_x);
            y += lh;
        }
    }
    win.content_epoch += 1;
}

// ==========================================================================
// 事件驱动实时刷新（ARCH-002）
//
// 旧实现由 GUI 后端每 400ms 无条件重画一遍实时视图，属于轮询：
// 系统没变化时白白烧 CPU，系统刚变化又要等最多 400ms 才看得见。
// 现在 GUI 在启动时向 EventBus 注册订阅者，能力层发布事件时只递增一个
// 「脏」代号；GUI 线程发现代号变化才真正重画。
// ==========================================================================

/// 事件代号：每收到一个相关事件就 +1。
static LIVE_EVENT_GENERATION: AtomicU64 = AtomicU64::new(1);
/// 已应用到画面的事件代号。
static LIVE_APPLIED_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 保证订阅者只注册一次（多个 GUI 会话 / 测试共用同一个全局总线）。
static LIVE_SUBSCRIPTION: OnceLock<()> = OnceLock::new();

/// 实时视图订阅者：只做一次原子递增，绝不持有锁或重画，避免发布线程被拖慢。
struct LiveRefreshSubscriber;

impl EventSubscriber for LiveRefreshSubscriber {
    fn on_event(&self, _event: &SystemEvent) {
        LIVE_EVENT_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}

/// 通知中心订阅者：把系统事件变成通知中心里的一条记录。
struct NotificationSubscriber;

impl EventSubscriber for NotificationSubscriber {
    fn on_event(&self, event: &SystemEvent) {
        // `notify()` 已经自己入队过一条通知并发布了同名事件，
        // 这里跳过它，避免一条通知被存两遍。
        if event.payload.get("notification_id").is_some() {
            return;
        }
        notification::on_system_event(event);
    }
}

/// 哪些事件需要刷新实时视图（基本覆盖全部运行时状态变化）。
fn live_refresh_filter() -> EventFilter {
    use SystemEventType as T;
    EventFilter {
        event_types: Some(vec![
            T::AgentStarted, T::AgentStopped, T::AgentStateChanged,
            T::TaskCreated, T::TaskStarted, T::TaskProgress, T::TaskCompleted,
            T::TaskFailed, T::TaskCancelled, T::TaskPaused, T::TaskResumed,
            T::ToolInvoked, T::ToolCompleted, T::ToolFailed,
            T::PermissionRequested, T::PermissionGranted, T::PermissionDenied,
            T::PermissionRevoked,
            T::FileCreated, T::FileChanged, T::FileDeleted, T::FileMoved,
            T::ProcessStarted, T::ProcessExited, T::ProcessSignaled,
            T::WindowCreated, T::WindowClosed, T::WindowFocused,
            T::WindowMoved, T::WindowResized,
            T::SystemResourceChanged,
        ]),
        ..Default::default()
    }
}

/// 注册 GUI 的事件订阅者（幂等，可在 boot / GUI 启动 / 测试里重复调用）。
pub fn init_event_subscription() {
    LIVE_SUBSCRIPTION.get_or_init(|| {
        let bus = event_bus().lock().unwrap_or_else(|e| e.into_inner());
        bus.subscribe(live_refresh_filter(), Box::new(LiveRefreshSubscriber));
        bus.subscribe(
            EventFilter {
                event_types: Some(vec![
                    SystemEventType::AgentStarted,
                    SystemEventType::AgentStopped,
                    SystemEventType::TaskCompleted,
                    SystemEventType::TaskFailed,
                    SystemEventType::PermissionRequested,
                ]),
                ..Default::default()
            },
            Box::new(NotificationSubscriber),
        );
    });
}

/// 订阅是否已建立（供测试与诊断使用）。
pub fn event_subscription_active() -> bool {
    LIVE_SUBSCRIPTION.get().is_some()
}

/// 当前事件代号（供测试判断事件是否送达 GUI）。
pub fn live_event_generation() -> u64 {
    LIVE_EVENT_GENERATION.load(Ordering::SeqCst)
}

/// 手工把画面标记为需要刷新（例如 Shell 直接改了内核状态但没有发布事件时）。
pub fn mark_live_dirty() {
    LIVE_EVENT_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// 仅当 EventBus 上有新事件时才刷新实时视图；返回是否真的重画了。
pub fn refresh_live_if_dirty() -> bool {
    init_event_subscription();
    let gen = LIVE_EVENT_GENERATION.load(Ordering::SeqCst);
    if gen == LIVE_APPLIED_GENERATION.load(Ordering::SeqCst) {
        return false;
    }
    refresh_live();
    true
}

/// 刷新所有「实时视图」窗口。
pub fn refresh_live() {
    let mut d = desktop().lock().unwrap();
    let updates: Vec<(u64, Vec<String>)> = d
        .windows()
        .iter()
        .map(|w| match w.title.as_str() {
            "EXEC BODIES" => (w.id, exec_lines()),
            "AGENT ORCHESTRATOR" => (w.id, agent_lines()),
            "LEASE / EPOCH" => (w.id, lease_lines()),
            _ => (w.id, Vec::new()),
        })
        .filter(|(_, lines)| !lines.is_empty())
        .collect();
    for (id, lines) in updates {
        if let Some(w) = d.window_mut(id) {
            paint_lines(w, &lines);
        }
    }
    drop(d);
    // 画面已经与本代号对应的事件同步，后续无需重复重画。
    LIVE_APPLIED_GENERATION.store(LIVE_EVENT_GENERATION.load(Ordering::SeqCst), Ordering::SeqCst);
}

#[cfg(test)]
mod event_subscription_tests {
    use super::*;
    use crate::event::{publish_event, EventSource};
    use serde_json::json;

    #[test]
    fn gui_refresh_is_driven_by_events() {
        init_event_subscription();
        assert!(event_subscription_active());

        let before = live_event_generation();
        publish_event(
            EventSource::System,
            SystemEventType::AgentStarted,
            None,
            json!({"agent_id": "1"}),
        );
        // 订阅者收到了事件，代号推进。
        assert!(live_event_generation() > before);

        // 事件驱动的刷新入口能真正执行刷新，并把代号标记为已应用。
        let _ = refresh_live_if_dirty();
        assert!(LIVE_APPLIED_GENERATION.load(Ordering::SeqCst) >= before);
    }
}

/// 便捷：单击（左键按下 + 抬起）。
pub fn click(x: i32, y: i32) -> Vec<input::Effect> {
    let mut e = inject(input::Event::MouseDown {
        x,
        y,
        button: input::MouseButton::Left,
    });
    e.extend(inject(input::Event::MouseUp {
        x,
        y,
        button: input::MouseButton::Left,
    }));
    e
}

/// 便捷：向焦点窗口输入文本。
pub fn key_text(text: &str) -> Vec<input::Effect> {
    let mut effects = Vec::new();
    for ch in text.chars() {
        let key = if ch == '\n' {
            input::Key::Enter
        } else if ch == '\u{8}' {
            input::Key::Backspace
        } else {
            input::Key::Char(ch)
        };
        effects.extend(inject(input::Event::KeyDown(key)));
    }
    effects
}

/// 便捷：移动鼠标。
pub fn mouse_move(x: i32, y: i32) -> Vec<input::Effect> {
    inject(input::Event::MouseMove { x, y })
}

/// 便捷：移动窗口。
pub fn move_window(id: u64, x: i32, y: i32) -> bool {
    let mut d = desktop().lock().unwrap();
    match d.window_mut(id) {
        Some(w) => {
            w.move_to(x, y);
            true
        }
        None => false,
    }
}

/// 便捷：聚焦窗口。
pub fn focus_window(id: u64) -> bool {
    let mut d = desktop().lock().unwrap();
    if d.window(id).is_none() {
        return false;
    }
    d.focus(id);
    true
}

/// 便捷：关闭窗口。
pub fn close_window(id: u64) -> bool {
    desktop().lock().unwrap().close(id)
}

/// 输入 + 桌面摘要。
pub fn input_status() -> String {
    let q = input_queue().lock().unwrap();
    let d = desktop().lock().unwrap();
    format!("{} focused={:?} cursor=({},{})", q.status(), d.focused(), d.cursor().0, d.cursor().1)
}

/// 窗口里输入的命令（终端 Enter 触发），由 shell 在窗口关闭后统一执行。
pub fn record_command(line: &str) {
    let mut q = pending_commands().lock().unwrap();
    q.push(line.to_string());
}

fn pending_commands() -> &'static Mutex<Vec<String>> {
    static CMDS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    CMDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 取走所有待执行命令。
pub fn take_commands() -> Vec<String> {
    std::mem::take(&mut *pending_commands().lock().unwrap())
}

/// 打开一个演示窗口，返回 id。
pub fn open_window(title: &str, w: u32, h: u32) -> u64 {
    let mut d = desktop().lock().unwrap();
    d.open(title, w, h, palette::BLUE)
}

/// 打开一个终端窗口。
pub fn open_terminal(title: &str, w: u32, h: u32) -> u64 {
    let mut d = desktop().lock().unwrap();
    d.open_terminal(title, w, h, "hib> ")
}

/// 合成并导出 BMP。
pub fn screenshot(path: &str) -> Result<String, String> {
    ensure_demo();
    let mut d = desktop().lock().unwrap();
    let frame = d.compose().clone();
    let n = bmp::save(&frame, path).map_err(|e| format!("写 BMP 失败: {}", e))?;
    Ok(format!(
        "已导出 {}x{} 像素到 {}（{} 字节，{} 个非黑像素）",
        frame.width(),
        frame.height(),
        path,
        n,
        frame.lit_pixels()
    ))
}

/// 终端里的真彩预览（半块字符，上=前景 下=背景）。
pub fn preview(cols: usize, rows: usize) -> String {
    ensure_demo();
    let mut d = desktop().lock().unwrap();
    let frame = d.compose().clone();
    preview_surface(&frame, cols, rows)
}

/// 对任意表面做半块字符预览。
pub fn preview_surface(s: &Surface, cols: usize, rows: usize) -> String {
    if cols == 0 || rows == 0 || s.width() == 0 || s.height() == 0 {
        return String::new();
    }
    let px_rows = rows * 2;
    let mut out = String::new();
    for cy in 0..rows {
        for cx in 0..cols {
            let top = sample(s, cx, cy * 2, cols, px_rows);
            let bottom = sample(s, cx, cy * 2 + 1, cols, px_rows);
            let (fg, bg) = (top, bottom);
            out.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                surface::red_of(fg),
                surface::green_of(fg),
                surface::blue_of(fg),
                surface::red_of(bg),
                surface::green_of(bg),
                surface::blue_of(bg)
            ));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn sample(s: &Surface, cx: usize, cy: usize, cols: usize, px_rows: usize) -> u32 {
    let w = s.width() as usize;
    let h = s.height() as usize;
    let x = (cx * w / cols).min(w.saturating_sub(1));
    let y = (cy * h / px_rows.max(1)).min(h.saturating_sub(1));
    s.get(x as i32, y as i32).unwrap_or(0)
}

/// 启动一个「窗口内运行的应用」，返回窗口 id。
pub fn open_app(name: &str) -> Result<u64, String> {
    let kind = apps::AppKind::parse(name).ok_or_else(|| {
        format!("未知应用 {} (可用: {})", name, apps::AppKind::list())
    })?;
    Ok(desktop().lock().unwrap().open_app(kind))
}

/// 把内核执行结果回显到应用窗口（终端应用会显示）。
pub fn app_output(id: u64, text: &str) {
    if let Some(w) = desktop().lock().unwrap().window_mut(id) {
        if let Some(app) = w.app.as_mut() {
            app.output(text);
        }
    }
}

/// 模拟点击任务栏（键盘/脚本也能走同一套窗口管理逻辑）。
pub fn taskbar_click(x: i32, y: i32) -> Vec<input::Effect> {
    let hit = {
        let d = desktop().lock().unwrap();
        d.taskbar_hit(x, y)
    };
    match hit {
        Some(h) => desktop().lock().unwrap().handle_taskbar(h),
        None => Vec::new(),
    }
}

/// 桌面摘要。
pub fn status() -> String {
    ensure_demo();
    let mut d = desktop().lock().unwrap();
    let _ = d.compose();
    d.status()
}

/// 导出文本行到 BMP（无窗口，纯像素验证）。
pub fn render_text_bitmap(text: &str, path: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as u32;
    let w = (longest * ADVANCE as u32 + 32).max(64);
    let h = (lines.len() as u32 * LINE_HEIGHT as u32 + 32).max(64);
    let mut s = Surface::filled(w, h, argb(255, 0x11, 0x11, 0x1b));
    let mut y = 16i32;
    for line in &lines {
        s.draw_text(16, y, line, palette::TEXT, 2);
        y += LINE_HEIGHT as i32 * 2;
    }
    let n = bmp::save(&s, path).map_err(|e| format!("写 BMP 失败: {}", e))?;
    Ok(format!("{}x{} -> {}（{} 字节）", w, h, path, n))
}

/// 启动真实窗口（需要 `--features gui`）。
pub fn run_native() -> Result<(), String> {
    #[cfg(feature = "gui")]
    {
        backend::run()
    }
    #[cfg(not(feature = "gui"))]
    {
        Err(
            "真实窗口后端未编译。请用 `cargo run --features gui -- gui run`，\n\
             或 `cargo run --features gui -- --shell` 进入 shell 后输入 `gui run` 启动。"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_builds_four_windows_and_composes() {
        let mut c = Compositor::new(960, 600);
        build_demo(&mut c);
        assert_eq!(c.windows().len(), 4);
        let frame = c.compose();
        assert!(frame.lit_pixels() > 10_000);
        assert_eq!(frame.width(), 960);
    }

    #[test]
    fn preview_has_truecolor_and_blocks() {
        let s = Surface::filled(8, 4, palette::BLUE);
        let p = preview_surface(&s, 4, 2);
        assert!(p.contains("\x1b[38;2;137;180;250m"));
        assert!(p.contains('▀'));
        assert_eq!(p.lines().count(), 2);
    }

    #[test]
    fn screenshot_roundtrip() {
        let dir = std::env::temp_dir().join("hibos-gui-shot");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("desktop.bmp");
        let msg = screenshot(path.to_str().unwrap()).unwrap();
        assert!(msg.contains("已导出"));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bmp::decode_header(&bytes).unwrap().0, DEFAULT_WIDTH);
    }

    #[test]
    fn text_bitmap_exports() {
        let path = std::env::temp_dir().join("hibos-gui-shot").join("text.bmp");
        let msg = render_text_bitmap("HELLO\nHIB OS", path.to_str().unwrap()).unwrap();
        assert!(msg.contains("->"));
        assert!(path.exists());
    }

    #[test]
    fn open_window_visible_in_status() {
        let id = open_window("UNIT-WINDOW", 200, 150);
        assert!(status().contains(&format!("win#{}", id)));
    }

    #[test]
    fn native_backend_reports_when_disabled() {
        // 未启用 gui feature 时应给出明确指引，而不是 panic。
        if !cfg!(feature = "gui") {
            assert!(run_native().is_err());
        }
    }
}
