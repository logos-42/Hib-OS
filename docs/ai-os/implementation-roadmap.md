# HibOS 实施路线图

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 规划文档 (P0-P5)

---

## 总体策略

不要一次性重写整个项目。先审计，再确定实施顺序。

每个阶段必须完成：**代码、测试、文档、验收**。

不得为了快速完成而降低现有系统能力。

---

## P0 — Architecture Foundation (架构基础)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| Repository Audit | 完整审计现有代码库 | `docs/ai-os/interaction-audit.md` | ✅ 完成 |
| Runtime Boundary | 明确 Host OS 和 AI OS 责任边界 | `docs/ai-os/system-architecture.md` | ✅ 完成 |
| System Capability Model | 统一系统能力层模型 | `docs/ai-os/system-capabilities.md` | ✅ 完成 |
| Permission Model | 权限模型 (Agent 身份、能力集、作用域、配额) | `docs/ai-os/permission-model.md` | ✅ 完成 |
| Event Model | 统一事件系统模型 | `docs/ai-os/event-model.md` | ✅ 完成 |
| Error Model | 统一错误模型 (CapabilityError) | 纳入 system-capabilities.md | ✅ 完成 |
| 能力层代码实现 | `kernel/capability/` 模块，CapabilityRequest/Response/Error/Event | Rust 代码 | 进行中 |
| 事件总线代码实现 | `kernel/event/` 模块，EventBus 订阅/过滤/查询 | Rust 代码 | 待开始 |
| 权限模型代码实现 | `kernel/permission/` 模块，AgentIdentity/PermissionChecker/AuthorizationManager | Rust 代码 | 待开始 |
| 能力层单元测试 | 能力注册表、权限检查、事件总线测试 | `tests/` | 待开始 |

**P0 验收标准**：架构文档全部完成，能力层/事件系统/权限模型代码通过单元测试。

---

## P1 — Shell Foundation (Shell 基础)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| Parser 重写 | 替代 `split_whitespace`，支持引号/转义/管道/重定向/变量替换 | `kernel/shell_parser.rs` | 待开始 |
| Executor 增强 | 支持管道、重定向、后台作业、退出码 | `kernel/shell_executor.rs` | 待开始 |
| PTY 层 | 支持真 PTY、交互式程序、终端尺寸变化 | `kernel/shell_pty.rs` | 待开始 |
| Process Integration | ps/top/kill/jobs/fg/bg/wait | Shell 内置命令 | 待开始 |
| History 增强 | Ctrl+R 历史搜索 | `kernel/line_editor.rs` | 待开始 |
| Completion 增强 | 参数补全、双 Tab 建议 | `kernel/shell.rs` | 待开始 |
| Built-in Commands | agent/task/capability/sysinfo/uptime/hostname/whoami/date/status/logs | `kernel/shell.rs` | 待开始 |
| JSON Output | `--json` 机器可读输出 | Shell 命令 | 待开始 |
| Shell 单元测试 | Parser/Executor/Completion/History/PTY/Signal/Process/JSON | `tests/shell_*` | 待开始 |

**P1 验收标准**：Shell 支持 agent/task/capability 命令，`--json` 输出，Parser 通过单元测试。

---

## P2 — GUI Foundation (GUI 基础)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| Navigation | 一级导航 (Home/Agents/Tasks/Workspace/Files/Applications/Terminal/System/Settings) | `kernel/gui/navigation.rs` | 待开始 |
| Window Management 增强 | Snap/Split View/Multiple Workspaces/Window Persistence | `kernel/gui/wm.rs` | 待开始 |
| Desktop Workspace | 主工作区，系统状态概览 | `kernel/gui/desktop.rs` | 待开始 |
| Application Launcher | 应用启动器 | `kernel/gui/launcher.rs` | 待开始 |
| Terminal 窗口 | 真终端窗口 (已有 terminal.rs，保留) | `kernel/gui/terminal.rs` | ✅ 已有 |
| System Status | 系统状态面板 | `kernel/gui/status.rs` | 待开始 |
| 事件订阅 | 替代轮询，订阅事件总线 | `kernel/event/` 集成 | 待开始 |
| GUI 单元测试 | Window/Navigation/Keyboard/FileManager | `tests/gui_*` | 待开始 |

**P2 验收标准**：GUI 有导航、窗口管理、桌面工作区、应用启动器、终端、系统状态，支持事件订阅。

---

## P3 — Agent Interaction (智能体交互)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| Agent Workspace | Agent 管理和观察界面 | `kernel/gui/agent_workspace.rs` | 待开始 |
| Task Workspace | 任务管理和时间线界面 | `kernel/gui/task_workspace.rs` | 待开始 |
| Agent Lifecycle Control | 完整生命周期状态机 (Created→…→Stopped) | `kernel/agent.rs` | 待开始 |
| Permission Center | 统一权限中心 (Once/Session/Persistent) | `kernel/gui/permission_center.rs` | 待开始 |
| Event Visualization | 事件流可视化 | `kernel/gui/event_view.rs` | 待开始 |
| Execution Logs | 执行日志查看 | `kernel/gui/execution_logs.rs` | 待开始 |
| Task 取消/恢复 | 任务 CancellationToken, pause/resume | `kernel/task/` | 待开始 |
| MCP Integration | Model Context Protocol | `kernel/mcp/` | 待开始 |
| Agent 单元测试 | 生命周期/任务/委托/权限/取消/事件/恢复 | `tests/agent_*` | 待开始 |

**P3 验收标准**：Agent 支持完整生命周期、权限配额、任务取消和恢复、MCP。

---

## P4 — System Applications (系统应用)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| File Manager | 文件浏览器 + Agent Workspace 目录 + 权限可见性 | `kernel/gui/file_manager.rs` | 待开始 |
| Settings | 系统设置 (Appearance/Display/Keyboard/…/Developer Options) | `kernel/gui/settings.rs` | 待开始 |
| System Monitor | CPU/Memory/Storage/Network/Processes/Agents/Tasks/Token/Latency | `kernel/gui/system_monitor.rs` | 待开始 |
| Application Manager | 应用列表/启动/停止/信息 | `kernel/gui/app_manager.rs` | 待开始 |
| Notification Center | 通知中心 | `kernel/gui/notification.rs` | 待开始 |

**P4 验收标准**：所有系统应用与真实系统配置关联，功能完整。

---

## P5 — Integration (集成)

| 任务 | 说明 | 交付物 | 状态 |
|------|------|--------|------|
| Shell/GUI/Agent 一致性 | 跨接口一致性测试 (6 种操作 × 3 入口) | `tests/consistency_*` | 待开始 |
| E2E 场景测试 | 场景一~六 | `tests/e2e_*` | 待开始 |
| Recovery | GUI 关闭后 Agent 继续，Agent 崩溃隔离 | `tests/recovery_*` | 待开始 |
| Performance | 性能基准测试 (GUI/Shell/Agent/System) | `benches/` | 待开始 |
| Accessibility | 键盘导航、屏幕阅读器、字体缩放、减少动画 | GUI 代码 | 待开始 |

**P5 验收标准**：跨接口一致性测试通过，6 个 E2E 场景全部通过，性能基准建立，无障碍支持。

---

## 里程碑总览

| 里程碑 | 阶段 | 预计交付 | 关键验收 |
|--------|------|---------|---------|
| M1 | P0 | 架构文档 + 能力层/事件/权限代码 | 单元测试通过 |
| M2 | P1 | Shell 重构 (Parser/Executor/PTY/命令) | Shell 测试通过 |
| M3 | P2 | GUI 基础 (导航/窗口/桌面/终端/状态) | GUI 测试通过 |
| M4 | P3 | Agent 交互 (工作空间/任务/权限/MCP) | Agent 测试通过 |
| M5 | P4 | 系统应用 (文件/设置/监控/应用/通知) | 应用测试通过 |
| M6 | P5 | 集成验收 (一致性/E2E/恢复/性能/无障碍) | 全部 E2E 通过 |

---

## 当前代码可复用资产

| 资产 | 路径 | 可直接使用 |
|------|------|-----------|
| 窗口管理器 | `kernel/gui/wm.rs` | ✅ 完整 (创建/聚焦/移动/缩放/最小化/最大化/关闭/Alt-Tab) |
| 窗口对象 | `kernel/gui/window.rs` | ✅ 完整 (像素窗口、终端模型、渲染) |
| 合成器 | `kernel/gui/compositor.rs` | ✅ 完整 (像素合成) |
| 终端 | `kernel/gui/terminal.rs` | ✅ 完整 (真终端窗口) |
| 文件管理器 | `kernel/gui/file_manager.rs` | ⚠️ 部分 (需扩展) |
| 任务栏 | `kernel/gui/taskbar.rs` | ⚠️ 部分 |
| 应用运行时 | `kernel/gui/apps.rs` | ⚠️ 部分 |
| Agent 编排器 | `kernel/agent.rs` | ✅ 完整 (spawn/mailbox/barrier/submit/broadcast/stop) |
| 感知通道 | `kernel/channels.rs` | ✅ 完整 (sense/act 抽象) |
| VFS | `kernel/vfs.rs` | ✅ 完整 (虚拟文件系统) |
| 系统调用 | `kernel/syscall/mod.rs` | ✅ 完整 (AiSystemCall) |
| 线编辑器 | `kernel/line_editor.rs` | ✅ 完整 (历史/补全/光标编辑) |
| 监控统计 | `kernel/monitor.rs` | ✅ 完整 (系统统计) |
| 集成测试 | `tests/ai_os_integration.rs` | ✅ 完整 (AI OS 管道测试) |
| 版本一致性 | `tests/version_consistency.rs` | ✅ 完整 |

---

## 已知技术限制

1. **no_std 内核与 std 原型混同**：`kernel/mod.rs` 中 `main()` 用于宿主环境，`kernel_main()` 用于裸机，部分模块依赖 `std`。
2. **版本二元性**：根 crate 和 kernel crate 有独立的 Cargo.toml，版本可能漂移 (已有测试守护)。
3. **无真 PTY**：Shell 目前无 PTY 支持，需新建。
4. **无事件系统**：GUI 目前通过轮询刷新 (`refresh_live`)，需迁移为事件订阅。
5. **无 JSON 输出**：Shell 所有输出为纯文本。

---
