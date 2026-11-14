# HibOS Agent-Native OS 交互能力审计报告

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**类别**: A (Rust 自研内核，裸机 x86_64 no_std + 宿主 OS std 原型栈)  
**审计日期**: 2026-11-14  
**审计人**: Principal OS Architect / Kilo  
**状态**: 初稿

---

## 1. 审计总览

| 维度 | 总体评级 | 说明 |
|------|---------|------|
| 内核架构 | PARTIAL | 自研宏内核，GPU Primacy / AI Consciousness / Action Processes / Modularity / Unified Perception 五大支柱，但缺少统一系统能力层、事件系统、权限模型 |
| Shell | PARTIAL | 交互式 syscall Shell，有 Parser/Executor/PTY/History/Completion，但缺少 Agent/Task/Capability 命令、JSON 输出、管道/重定向 |
| GUI | PARTIAL | 真像素合成器、窗口管理器、终端窗口、文件管理器，但缺少 Agent Workspace、Task Workspace、Permission Center、系统监控 |
| Agent Runtime | PARTIAL | 子智能体编排器 (agent.rs)，真线程+邮箱+屏障，但缺少 Agent Identity/Lifecycle/权限配额/审计轨迹/MCP |
| 统一系统能力层 | MISSING | 无 CapabilityRequest/Response/Error/Event 统一抽象，Shell/GUI/Agent 各自直连内核能力 |
| 事件系统 | MISSING | 无统一事件总线，GUI 轮询而非订阅，无事件重放/关联 ID |
| 权限模型 | PARTIAL | 通道 Read/Write/ReadWrite 权限，但无 Agent 身份/能力集/配额/过期/撤销机制 |
| 测试 | PARTIAL | 有单元测试 (wm/window/mod/agent) 和集成测试 (ai_os_integration)，但无跨接口一致性测试、E2E 场景测试 |

### 代码位置索引

| 模块 | 路径 |
|------|------|
| 内核入口 | `kernel/mod.rs` |
| 库入口 | `lib.rs` |
| 用户态库 | `user/lib.rs` |
| Shell | `kernel/shell.rs` |
| GUI 子系统 | `kernel/gui/` |
| 窗口管理器 | `kernel/gui/wm.rs` |
| 窗口对象 | `kernel/gui/window.rs` |
| 合成器 | `kernel/gui/compositor.rs` |
| 文件管理器 | `kernel/gui/file_manager.rs` |
| 终端 | `kernel/gui/terminal.rs` |
| 输入 | `kernel/gui/input.rs` |
| 表面/像素 | `kernel/gui/surface.rs` |
| Agent 编排器 | `kernel/agent.rs` |
| 感知通道 | `kernel/channels.rs` |
| 系统调用 | `kernel/syscall/mod.rs` |
| VFS | `kernel/vfs.rs` |
| 执行体 | `kernel/exec.rs` |
| 线编辑器 | `kernel/line_editor.rs` |
| 监控/统计 | `kernel/monitor.rs` |
| 集成测试 | `tests/ai_os_integration.rs` |
| 版本一致性测试 | `tests/version_consistency.rs` |

---

## 2. Shell 审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Command Parsing | PARTIAL | `kernel/shell.rs:310` (`dispatch`) | 使用 `split_whitespace()`，不支持引号/转义/变量替换/管道/重定向 | 实现真正的 Shell Parser，支持引号、转义、管道、重定向 |
| Arguments | PARTIAL | `kernel/shell.rs:316` | 仅 `split_whitespace`，无参数解析规范 | 支持位置参数和选项参数 |
| Quoting | MISSING | — | 不存在引号处理 | 支持单引号/双引号 |
| Escaping | MISSING | — | 不存在转义处理 | 支持反斜杠转义 |
| Variables | MISSING | — | 不存在变量替换 | 支持 `$VAR` 和 `${VAR}` |
| Environment Variables | PARTIAL | `kernel/shell.rs:149-162` | 仅读取 `DEEPSEEK_API_KEY`/`HIBOS_LLM_*_KEY`，无 `export`/`env` 命令 | 实现 `env`/`export`/`unset` |
| Command Substitution | MISSING | — | 不存在 `$(cmd)` 或反引号 | 支持命令替换 |
| Pipes | MISSING | — | 不存在管道 | 实现 `|` 管道 |
| Redirection | MISSING | — | 不存在 `>`/`<`/`>>` | 实现重定向 |
| Append Redirection | MISSING | — | 不存在 `>>` | 实现追加重定向 |
| Command Chaining | MISSING | — | 不存在 `&&`/`||`/`;` | 实现命令链 |
| Conditional Execution | MISSING | — | 不存在条件执行 | 实现 `if/then/else` 或 `&&`/`||` |
| Glob/Wildcards | MISSING | — | 不存在路径展开 | 实现 `*`/`?`/`[...]` |
| Exit Codes | PARTIAL | `kernel/shell.rs` | 命令成功/失败未返回统一退出码 | 每条命令返回明确退出码 |
| Background Jobs | MISSING | — | 不存在 `&` 后台作业 | 实现后台作业和 `jobs` |
| Signals | MISSING | — | 不存在 Ctrl+C/SIGINT 处理 | 实现信号处理 |
| Script Execution | MISSING | — | 不存在脚本文件执行 | 实现 `source`/脚本执行 |
| Command History | COMPLETE | `kernel/line_editor.rs` | 有历史，支持 ↑↓ | 现有即可，需测试 |
| History Search | PARTIAL | `kernel/line_editor.rs` | 有历史，未确认搜索 | 支持 `Ctrl+R` 历史搜索 |
| Tab Completion | COMPLETE | `kernel/shell.rs:283` (`complete`) | 有 Tab 补全（命令名+路径） | 现有即可，需参数补全 |
| Double Tab Suggestions | MISSING | — | 无双 Tab 建议 | 实现双 Tab 显示候选 |
| Command Suggestions | PARTIAL | `kernel/shell.rs:269` (`hint_for`) | 有 `--help` 式提示 | 扩展为参数补全 |
| Argument Completion | PARTIAL | `kernel/shell.rs:299-306` | 仅路径补全 | 扩展为命令参数补全 |
| Path Completion | COMPLETE | `kernel/shell.rs:292` (`vfs.complete_path`) | 有路径补全 | 现有即可 |
| Syntax Highlighting | MISSING | — | 无语法高亮 | 实现 ANSI 语法高亮 |
| Error Highlighting | MISSING | — | 无错误高亮 | 实现错误高亮 |
| Inline Hints | PARTIAL | `kernel/shell.rs:269` | 有提示行 | 现有即可 |
| Command Help | COMPLETE | `kernel/shell.rs:367` (`cmd_help`) | 有 `help <命令>` | 现有即可 |
| Prompt Customization | PARTIAL | `kernel/shell.rs:176` (`prompt`) | 有固定 prompt | 支持自定义 prompt |
| Alias | MISSING | — | 不存在别名 | 实现 `alias`/`unalias` |
| Keyboard Shortcuts | PARTIAL | `kernel/line_editor.rs` | 有 ↑↓/←→/Tab | 支持 Ctrl+C/D/R/L |
| Copy / Paste | MISSING | — | 无剪贴板 | 实现复制/粘贴 |
| Search Output | MISSING | — | 无输出搜索 | 实现输出搜索 |
| Multiple Tabs | MISSING | — | 无多标签 | 实现多标签 |
| Multiple Sessions | MISSING | — | 无多会话 | 实现多会话 |
| Long Running Task Status | MISSING | — | 无长任务状态 | 实现任务状态查询 |
| Process Interruption | MISSING | — | 无进程中断 | 实现中断正在运行的任务 |
| Terminal Resize | PARTIAL | `kernel/gui/terminal.rs` | GUI 终端有尺寸，Shell 无 PTY 尺寸变化 | 支持终端尺寸变化 |
| PTY | MISSING | — | 无真 PTY | 实现 PTY 支持 |
| 交互式程序 | MISSING | — | 不支持交互式程序 | 支持 vim/less 等交互式程序 |
| JSON Output | MISSING | — | 所有输出为纯文本 | `agent list --json` 等支持 JSON |
| Machine-readable Errors | MISSING | — | 错误为纯文本 | 结构化机器可读错误 |
| Structured Progress | MISSING | — | 无进度事件 | 实现结构化进度 |
| Task IDs / Execution IDs | MISSING | — | 无任务/执行 ID 体系 | 实现任务 ID 体系 |
| Cancellation | MISSING | — | 无取消机制 | 实现任务取消 |
| Permission Prompts | MISSING | — | 无授权提示 | 实现权限确认提示 |
| Dry Run | MISSING | — | 无试运行 | 实现 `--dry-run` |

### Shell 现有命令 (kernel/shell.rs:87-127)

实现的命令: help, channels, sense, act, concepts, policy, token, query, prio, proc, run, queue, stats, desktop, gui, pwd, ls, cd, cat, write, append, mkdir, touch, rm, tree, df, fbpreview, save, load, resetfs, wallpaper, deepseek, llm, modules, modload, modunload, history, exit, quit

缺失的目标命令 (PART 3.1): ps, top, kill, jobs, fg, bg, wait, pgrep, pkill, env, export, time, sysinfo, uptime, hostname, whoami, date, status, logs, services, devices, memory, storage, network, app list/launch/stop/info/install/uninstall, agent list/inspect/create/start/stop/pause/resume/logs/permissions/tools, task list/inspect/create/run/cancel/pause/resume/logs, capability list/inspect/permissions/test, find, stat, file, du, less, grep, diff

---

## 3. GUI 审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Window Lifecycle | COMPLETE | `kernel/gui/wm.rs` | 创建/显示/隐藏/关闭/聚焦/最小化/最大化/还原 | 现有即可，需更多测试 |
| Focus Management | COMPLETE | `kernel/gui/wm.rs:392` | 点击聚焦、Alt-Tab、置顶 | 现有即可 |
| Move | COMPLETE | `kernel/gui/wm.rs:476` | 移动窗口 | 现有即可 |
| Resize | COMPLETE | `kernel/gui/wm.rs:496` | 缩放窗口 | 现有即可 |
| Minimize | COMPLETE | `kernel/gui/wm.rs:418` | 最小化窗口 | 现有即可 |
| Maximize | COMPLETE | `kernel/gui/wm.rs:436` | 最大化窗口 | 现有即可 |
| Restore | COMPLETE | `kernel/gui/wm.rs:460` | 还原窗口 | 现有即可 |
| Close | COMPLETE | `kernel/gui/wm.rs:381` | 关闭窗口 | 现有即可 |
| Snap | MISSING | — | 无窗口对齐 | 实现 Snap |
| Split View | MISSING | — | 无分屏 | 实现 Split View |
| Multiple Windows | COMPLETE | `kernel/gui/compositor.rs` | 支持多窗口 | 现有即可 |
| Multiple Workspaces | MISSING | — | 无多工作区 | 实现多工作区 |
| Keyboard Shortcuts | PARTIAL | `kernel/gui/wm.rs:589` | Alt-Tab 循环 | 扩展更多快捷键 |
| Window Persistence | MISSING | — | 无窗口状态持久化 | 实现窗口位置/大小持久化 |
| Alt+Tab | COMPLETE | `kernel/gui/wm.rs:589` | Alt-Tab/ Alt-Shift-Tab | 现有即可 |
| Desktop Environment | PARTIAL | `kernel/gui/mod.rs:115` (`build_demo`) | 有演示桌面，4 个窗口 | 需正式桌面环境 |
| Window Manager | COMPLETE | `kernel/gui/wm.rs` | 真窗口管理器 | 现有即可 |
| Application Runtime | PARTIAL | `kernel/gui/apps.rs` | 有 AppState，应用有限 | 扩展应用运行时 |
| Navigation | MISSING | — | 无一级导航 (Home/Agents/Tasks/…) | 实现导航栏 |
| Launcher | MISSING | — | 无应用启动器 | 实现启动器 |
| Dock / Taskbar | PARTIAL | `kernel/gui/taskbar.rs` | 有任务栏 | 扩展任务栏 |
| File Manager | PARTIAL | `kernel/gui/file_manager.rs` | 有文件管理器 | 扩展为完整文件管理器 |
| Settings | MISSING | — | 无设置界面 | 实现设置 |
| Terminal UI | COMPLETE | `kernel/gui/terminal.rs` | 有真终端窗口 | 现有即可 |
| Agent Workspace | MISSING | — | 无 Agent 管理界面 | 实现 Agent Workspace |
| Task Workspace | MISSING | — | 无任务管理界面 | 实现 Task Workspace |
| Notification Center | MISSING | — | 无通知中心 | 实现通知中心 |
| System Monitor | MISSING | — | 无系统监控界面 | 实现系统监控 |
| Permission Dialog | MISSING | — | 无权限对话框 | 实现权限对话 |
| Context Visualization | MISSING | — | 无上下文可视化 | 实现上下文可视化 |

### GUI 现有窗口 (kernel/gui/mod.rs:115-158)

- HIB OS CONSOLE (终端)
- EXEC BODIES (执行体视图)
- AGENT ORCHESTRATOR (子智能体视图)
- LEASE / EPOCH (通道租约/世界版本)

---

## 4. Agent Runtime 审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Agent Registry | COMPLETE | `kernel/agent.rs:81` (`Orchestrator`) | 有 BTreeMap 注册表 | 现有即可 |
| Identity | PARTIAL | `kernel/agent.rs:61` (`AgentEntry`) | 有 id/name/role，无 owner/principal | 增加 owner、principal 字段 |
| Lifecycle | PARTIAL | `kernel/agent.rs:30` (`AgentState`) | 仅 Idle/Working/Stopped，缺少 Created/Initializing/Waiting/Blocked/Paused/Completed/Failed | 扩展为完整状态机 |
| Scheduling | PARTIAL | `kernel/agent.rs:187` (`exec::spawn`) | 真线程，基于 CpuPriority | 扩展为任务调度器 |
| Context | MISSING | — | 无上下文管理 | 实现 Agent Context |
| Memory | MISSING | — | 无记忆系统 | 实现 Agent Memory |
| Tool Calling | PARTIAL | `kernel/agent.rs:347` (`run_task`) | 支持 sense/act/barrier/sleep/echo | 扩展为结构化工具调用 |
| Tool Registry | MISSING | — | 无工具注册表 | 实现工具注册表 |
| MCP | MISSING | — | 无 Model Context Protocol | 实现 MCP 集成 |
| Multi-Agent Communication | PARTIAL | `kernel/agent.rs:446` (`broadcast`) | 有广播，无点对点消息 | 实现点对点通信 |
| Delegation | MISSING | — | 无委托机制 | 实现 agent.delegate |
| Task Execution | PARTIAL | `kernel/agent.rs:266` (`agent_loop`) | 邮箱驱动任务执行 | 实现任务对象和生命周期 |
| Cancellation | MISSING | — | 无任务取消 | 实现任务取消 |
| Recovery | MISSING | — | 无崩溃恢复 | 实现 Agent 崩溃恢复 |
| Event Streaming | MISSING | — | 无事件流 | 实现事件流 |
| Observability | PARTIAL | `kernel/agent.rs:574` (`status`) | 有状态摘要 | 扩展为可观测性 |
| Agent ID | COMPLETE | `kernel/agent.rs:26` | `AgentId = u64` | 现有即可 |
| Owner / Principal | MISSING | — | 无所有者概念 | 实现 Agent 身份 |
| Capability Set | MISSING | — | 无能力集 | 实现 Agent 能力集 |
| Permission Scope | MISSING | — | 无权限范围 | 实现权限范围 |
| Execution Context | MISSING | — | 无执行上下文 | 实现执行上下文 |
| Resource Quota | MISSING | — | 无资源配额 | 实现资源配额 |
| Lifecycle State | PARTIAL | `kernel/agent.rs:30` | 有基础状态 | 扩展为完整状态机 |
| Audit Trail | MISSING | — | 无审计轨迹 | 实现审计日志 |

---

## 5. 统一系统能力层审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Capability Registry | MISSING | — | 无能力注册表 | 实现 SystemCapabilityRegistry |
| Filesystem Capability | PARTIAL | `kernel/vfs.rs` | VFS 存在，但无统一能力接口 | 封装为 capability.list/read/write/… |
| Process Capability | PARTIAL | `kernel/syscall/mod.rs` | 有 CreateProcess/QueryProcess，无统一接口 | 封装为 process.list/inspect/start/stop |
| Application Capability | PARTIAL | `kernel/gui/apps.rs` | 有应用，无统一接口 | 封装为 app.list/launch/close |
| Network Capability | PARTIAL | `kernel/net.rs` | 有网络，无统一接口 | 封装为 network.* |
| Device Capability | PARTIAL | `kernel/channels.rs` (Device) | 有设备通道，无统一接口 | 封装为 device.* |
| Window Capability | PARTIAL | `kernel/gui/wm.rs` | 有窗口管理，无统一接口 | 封装为 window.list/inspect/focus/move/resize |
| Storage Capability | PARTIAL | `kernel/vfs.rs` | 有 VFS，无统一接口 | 封装为 storage.* |
| Settings Capability | MISSING | — | 无设置能力 | 实现 settings.read/update |
| Notification Capability | MISSING | — | 无通知能力 | 实现 notification.* |
| Agent Capability | PARTIAL | `kernel/agent.rs` | 有 Agent 编排，无统一接口 | 封装为 agent.list/inspect/start/stop |
| Task Capability | MISSING | — | 无任务能力 | 实现 task.create/inspect/cancel/pause/resume |
| Capability ID | MISSING | — | 无能力 ID | 定义能力 ID 体系 |
| Input/Output Schema | MISSING | — | 无模式定义 | 定义输入/输出 Schema |
| Error Schema | MISSING | — | 错误格式不统一 | 定义统一 Error Schema |
| Permission Requirements | MISSING | — | 无能力权限声明 | 为每个能力定义权限要求 |
| Execution Context | MISSING | — | 无执行上下文 | 定义执行上下文 |
| Timeout | MISSING | — | 无超时机制 | 实现超时 |
| Cancellation | MISSING | — | 无取消机制 | 实现取消 |
| Progress Events | MISSING | — | 无进度事件 | 实现进度事件 |
| Audit Information | MISSING | — | 无审计信息 | 实现审计信息 |

---

## 6. 事件系统审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Event ID | MISSING | — | 无事件 ID | 定义事件 ID |
| Timestamp | MISSING | — | 无事件时间戳 | 定义时间戳 |
| Source | MISSING | — | 无事件源 | 定义事件源 |
| Event Type | MISSING | — | 无事件类型枚举 | 定义所有事件类型 |
| Correlation ID | MISSING | — | 无关联 ID | 实现关联 ID |
| Payload | MISSING | — | 无事件载荷 | 定义载荷 |
| Schema Version | MISSING | — | 无 Schema 版本 | 定义版本 |
| Subscription | MISSING | — | 无订阅机制 | 实现订阅/过滤 |
| Replay | MISSING | — | 无事件重放 | 实现重放 (持久化时) |
| Real-time Push | MISSING | — | 无实时推送 | GUI 应订阅事件而非轮询 |
| AgentStarted/Stopped | MISSING | — | 无 Agent 事件 | 实现 Agent 生命周期事件 |
| TaskCreated/Started/… | MISSING | — | 无任务事件 | 实现任务生命周期事件 |
| ToolInvoked/Completed | MISSING | — | 无工具事件 | 实现工具调用事件 |
| PermissionRequested/Granted/Denied | MISSING | — | 无权限事件 | 实现权限事件 |
| FileChanged | MISSING | — | 无文件事件 | 实现文件变更事件 |
| ProcessStarted/Exited | MISSING | — | 无进程事件 | 实现进程事件 |
| WindowCreated/Closed | MISSING | — | 无窗口事件 | 实现窗口事件 |
| SystemResourceChanged | MISSING | — | 无资源事件 | 实现资源变更事件 |

---

## 7. 权限模型审计

| 功能 | 评级 | 代码位置 | 问题 | 验收条件 |
|------|------|---------|------|---------|
| Agent Identity | PARTIAL | `kernel/agent.rs:26` | 有 AgentId，无 Owner/Principal | 实现完整身份 |
| Capability Set | MISSING | — | 无能力集 | 实现能力集 |
| Permission Scope | MISSING | — | 无权限范围 | 实现权限范围 |
| Resource Quota | MISSING | — | 无配额 | 实现资源配额 |
| Least Privilege | MISSING | — | 默认无权限限制 | 实现最小权限 |
| Authorization (Once/Session/Persistent) | MISSING | — | 无授权模式 | 实现三种授权模式 |
| Permission Revocation | MISSING | — | 无撤销机制 | 实现权限撤销 |
| Audit Trail | MISSING | — | 无审计 | 实现权限审计 |
| High-risk Operation Confirmation | MISSING | — | 无高风险确认 | 实现删除/配置/网络/安装的确认 |
| Shell Injection Protection | MISSING | — | `split_whitespace` 易被注入 | 实现真正的解析器 |
| Path Traversal Protection | MISSING | — | VFS 无路径穿越保护 | 实现路径安全 |
| IPC Authentication | MISSING | — | 无 IPC 身份验证 | 实现 IPC 认证 |

---

## 8. 跨接口一致性审计

| 操作 | GUI | Shell | Agent API | 一致性 |
|------|-----|-------|-----------|--------|
| 文件删除 | MISSING | `rm` (VFS) | MISSING | 不一致 |
| 文件读取 | PARTIAL | `cat` (VFS) | MISSING | 不一致 |
| 进程列表 | MISSING | `proc` | MISSING | 不一致 |
| 窗口管理 | COMPLETE (wm://) | `gui` | MISSING | 不一致 |
| Agent 管理 | MISSING | MISSING | `agent://orchestrator` | 不一致 |
| 任务管理 | MISSING | MISSING | MISSING | 不一致 |
| 权限管理 | MISSING | MISSING | MISSING | 不一致 |

---

## 9. 测试覆盖审计

| 测试类型 | 评级 | 代码位置 | 说明 |
|---------|------|---------|------|
| Parser Tests | MISSING | — | 无 Shell Parser 测试 |
| Executor Tests | MISSING | — | 无 Shell Executor 测试 |
| Pipeline Tests | MISSING | — | 无管道测试 |
| Redirection Tests | MISSING | — | 无重定向测试 |
| Completion Tests | PARTIAL | `kernel/shell.rs` | 有补全逻辑，无专门测试 |
| History Tests | PARTIAL | `kernel/line_editor.rs` | 有历史，无专门测试 |
| PTY Tests | MISSING | — | 无 PTY 测试 |
| Signal Tests | MISSING | — | 无信号测试 |
| Process Tests | MISSING | — | 无进程测试 |
| JSON Output Tests | MISSING | — | 无 JSON 输出测试 |
| Window Tests | COMPLETE | `kernel/gui/wm.rs` | 有窗口管理器单元测试 |
| Navigation Tests | MISSING | — | 无导航测试 |
| Keyboard Tests | MISSING | — | 无键盘测试 |
| File Manager Tests | PARTIAL | `kernel/gui/file_manager.rs` | 有文件管理器，测试有限 |
| Agent Workspace Tests | MISSING | — | 无 Agent Workspace 测试 |
| Task Workspace Tests | MISSING | — | 无 Task Workspace 测试 |
| Permission Dialog Tests | MISSING | — | 无权限对话测试 |
| Notification Tests | MISSING | — | 无通知测试 |
| Recovery Tests | MISSING | — | 无恢复测试 |
| Agent Lifecycle Tests | PARTIAL | `kernel/agent.rs` | 有 Agent 单元测试 |
| Task Lifecycle Tests | MISSING | — | 无任务生命周期测试 |
| Delegation Tests | MISSING | — | 无委托测试 |
| Capability Invocation Tests | MISSING | — | 无能力调用测试 |
| Permission Tests | MISSING | — | 无权限测试 |
| Cancellation Tests | MISSING | — | 无取消测试 |
| Event Streaming Tests | MISSING | — | 无事件流测试 |
| Cross-Layer Consistency Tests | MISSING | — | 无跨接口一致性测试 |
| E2E Scenario Tests | MISSING | — | 无端到端场景测试 |

---

## 10. 技术限制和已知问题

1. **无统一系统能力层**：Shell、GUI、Agent 各自直接调用内核能力，导致接口不一致。
2. **无事件系统**：GUI 通过轮询获取状态，而非订阅事件。
3. **Shell Parser 脆弱**：使用 `split_whitespace()`，不支持引号、转义、管道、重定向。
4. **无 JSON 输出**：所有命令输出为纯文本，不适合机器读取。
5. **无 Agent 权限模型**：Agent 缺少身份、能力集、配额、审计轨迹。
6. **无任务体系**：缺少 Task 对象、任务生命周期、任务取消。
7. **无 MCP 集成**：缺少 Model Context Protocol 支持。
8. **版本二元性**：根 crate 和 kernel crate 有独立的 Cargo.toml，版本可能漂移 (已有测试守护)。
9. **no_std 内核与 std 原型混同**：`kernel/mod.rs` 中 `main()` 用于宿主环境，`kernel_main()` 用于裸机，部分模块依赖 `std`。

---

## 11. 验收条件总结 (Acceptance Criteria)

P0 阶段完成标志：
- [ ] `docs/ai-os/interaction-audit.md` 完成并提交
- [ ] 统一系统能力层代码实现 (CapabilityRequest/Response/Error/Event)
- [ ] 统一事件系统代码实现
- [ ] 权限模型代码实现 (Agent Identity, Capability Set, Permission Scope)
- [ ] 错误模型统一

P1 阶段完成标志：
- [ ] Shell Parser 支持引号、转义、管道、重定向
- [ ] Shell 支持 agent/task/capability 命令
- [ ] Shell 支持 JSON 输出 (`--json`)
- [ ] Shell 有 Parser/Executor/Completion 单元测试

P2 阶段完成标志：
- [ ] GUI 有 Agent Workspace 界面
- [ ] GUI 有 Task Workspace 界面
- [ ] GUI 有 Permission Center 界面
- [ ] GUI 支持事件订阅而非轮询

P3 阶段完成标志：
- [ ] Agent 支持完整生命周期状态
- [ ] Agent 支持权限配额
- [ ] Agent 支持任务取消和恢复
- [ ] Agent 支持 MCP

P5 阶段完成标志：
- [ ] 跨接口一致性测试 (GUI/Shell/Agent 相同操作结果一致)
- [ ] E2E 场景测试 (6 个场景全部通过)
- [ ] 性能基准测试
- [ ] 无障碍支持

---
