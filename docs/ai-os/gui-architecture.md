# HibOS GUI 架构文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P2 GUI 基础)

---

## 1. GUI 定位

GUI 是 **Human Control Plane + System Observatory + Agent Workspace**。

GUI 不应只是传统桌面，也不应只是聊天界面。它必须让用户理解系统正在发生什么，并能够随时控制系统。

---

## 2. 信息架构

### 2.1 一级导航 (建议)

1. Home — 系统概览，当前状态，活跃 Agent，正在运行的任务
2. Agents — Agent 管理与观察
3. Tasks — 任务管理与时间线
4. Workspace — 工作空间，文件与 Agent 工作区
5. Files — 文件浏览器
6. Applications — 应用启动与管理
7. Terminal — 终端入口
8. System — 系统监控，资源，日志
9. Settings — 系统设置

**不机械地全部做成左侧菜单**。根据产品定位调整，Home 作为仪表盘，其他作为顶部或侧边导航。

### 2.2 主界面必须展示

- 当前系统状态
- 活跃 Agent
- 正在运行的任务
- 最近活动
- 待处理授权
- 系统资源
- 最近文件
- 工作空间状态

**不能让用户进入系统后只看到一个聊天输入框。**

---

## 3. 核心界面

### 3.1 Agent Workspace (AI OS 核心界面)

每个 Agent 展示：

| 字段 | 说明 |
|------|------|
| Name | Agent 名称 |
| Agent ID | 唯一标识 |
| Role | 角色 |
| Identity | 身份 / Owner / Principal |
| Model | 使用的模型 |
| Runtime Status | 运行时状态 |
| Current Task | 当前任务 |
| Current Context | 当前上下文 |
| Available Tools | 可用工具 |
| Permissions | 权限列表 |
| Resource Usage | 资源占用 |
| Recent Events | 最近事件 |
| Execution History | 执行历史 |

**Agent 状态必须清晰区分：**

Created → Initializing → Idle → Running → Waiting → Blocked → Paused → Completed / Failed → Stopped

用户能够：创建/启动/暂停/恢复/停止 Agent，查看运行记录、工具调用，修改授权，查看资源占用。

**不能通过前端假状态模拟 Agent 生命周期。**

### 3.2 Task Workspace

任务是人类目标和 Agent 执行之间的桥梁。

展示：

| 字段 | 说明 |
|------|------|
| Task ID | 任务标识 |
| Task Description | 任务描述 |
| Status | 状态 |
| Assigned Agents | 分配的 Agent |
| Subtasks | 子任务 |
| Dependencies | 依赖 |
| Progress | 进度 (来自实际执行事件) |
| Execution Timeline | 执行时间线 |
| Logs | 日志 |
| Results | 结果 |
| Errors | 错误 |
| Resource Consumption | 资源消耗 |

支持：Task Creation / Cancellation / Pause / Resume / Retry / Inspection / Result Review。

**任务进度必须来自实际执行事件。不能通过任意递增百分比制造虚假的进度动画。**

### 3.3 Agent Collaboration Visualization

展示多 Agent 协作：

- Agent Relationships（Agent 关系）
- Delegation（委托）
- Communication（通信）
- Task Dependencies（任务依赖）
- Shared Resources（共享资源）
- Execution Flow（执行流）

**图形关系必须来自真实数据。不能为了视觉效果展示虚构的 Agent 连接。**

### 3.4 Context & Memory Visualization

让用户检查 Agent 使用的上下文：

- Context Usage（上下文使用量）
- Context Sources（上下文来源）
- Memory Records（记忆记录）
- Retrieved Information（检索信息）
- Tool Results（工具结果）
- Context Compression（上下文压缩）
- Context Updates（上下文更新）

支持：查看 / 搜索 / 检查来源 / 查看时间 / 权限管理 / 必要时编辑或删除。

敏感上下文应受到权限保护。

### 3.5 Filesystem GUI

文件管理器必须与 Agent 工作空间结合：

- File Browser / Folder Navigation / Breadcrumbs / Address Bar
- Search / Sort / Filter
- Grid View / List View / Preview
- Multi-select / Copy / Move / Rename / Delete / Trash / Restore
- File Properties / File Permissions
- Drag & Drop / Tabs / Recent Files / Favorites

额外：

- Agent Workspace Directories（Agent 工作区目录）
- Task Output Directories（任务输出目录）
- Shared Workspace（共享工作区）
- Agent File Access Visibility（Agent 文件访问可见性）

用户应能明确知道某个文件属于哪个工作空间，以及哪个 Agent 有权访问。

### 3.6 Window Management

实现真正的窗口系统：

- Window Lifecycle / Focus Management / Move / Resize / Minimize / Maximize / Restore / Close
- Snap / Split View / Multiple Windows / Multiple Workspaces
- Keyboard Shortcuts / Window Persistence

支持 Alt+Tab 和系统级窗口切换。

**如果底层 GUI Framework 已有窗口管理能力，优先使用现有能力，不要重复造轮子。** (现有 `kernel/gui/wm.rs` 已实现完整窗口管理器)

### 3.7 System Monitor

展示：

- CPU / Memory / Storage / Network
- Processes / Running Applications
- Active Agents / Active Tasks
- Runtime Health / API Usage / Token Consumption / Model Latency / Tool Execution Latency

**系统资源和 AI 资源必须区分。不要把模型 API Token 消耗和 CPU 内存占用混为一个资源指标。**

### 3.8 Permission Center

统一权限中心：

- Agent Identity / Granted Capabilities / Pending Requests
- Permission Scope / Expiration / Execution History / Risk Level

高风险操作（文件删除、系统配置修改、外部网络访问、安装软件、执行高风险命令）提供适当授权机制。

授权支持：Once / Session / Persistent。

**不能默认所有 Agent 拥有全部权限。**

### 3.9 Settings

设置至少包含：

Appearance / Display / Keyboard / Mouse / Terminal / Network / Storage / Applications / Agents / Models / Providers / API Configuration / Permissions / Privacy / Security / Notifications / System Updates / Developer Options

**所有设置必须与真实系统配置关联。**

---

## 4. 与现有代码的映射

| 组件 | 现有代码 | 状态 | 计划 |
|------|---------|------|------|
| Compositor | `kernel/gui/compositor.rs` | COMPLETE | 保留 |
| Window Manager | `kernel/gui/wm.rs` | COMPLETE | 保留，增加 Snap/Split View |
| Window | `kernel/gui/window.rs` | COMPLETE | 保留 |
| Terminal | `kernel/gui/terminal.rs` | COMPLETE | 保留，增加 PTY 支持 |
| File Manager | `kernel/gui/file_manager.rs` | PARTIAL | 增加 Agent Workspace 目录、权限可见性 |
| Taskbar | `kernel/gui/taskbar.rs` | PARTIAL | 扩展 |
| Apps | `kernel/gui/apps.rs` | PARTIAL | 扩展应用运行时 |
| Surface | `kernel/gui/surface.rs` | COMPLETE | 保留 |
| Input | `kernel/gui/input.rs` | COMPLETE | 保留 |
| Navigation | — | MISSING | 新建一级导航 |
| Agent Workspace | — | MISSING | 新建 |
| Task Workspace | — | MISSING | 新建 |
| Permission Center | — | MISSING | 新建 |
| System Monitor | — | MISSING | 新建 |
| Settings | — | MISSING | 新建 |
| Notification Center | — | MISSING | 新建 |
| Event Subscription | — | MISSING | 替代轮询，订阅事件总线 |

---

## 5. 设计语言 (UX)

视觉方向：Minimal / Functional / Information Dense but Readable / Calm / Precise / Responsive / Professional

重点：

1. 清晰的层级
2. 优秀的字体与排版
3. 一致的图标
4. 合理的信息密度
5. 明确的交互反馈
6. 快速的键盘操作
7. 稳定的窗口行为
8. 清晰的任务状态
9. 可观察的 Agent 执行过程
10. 低延迟

必须覆盖状态：Hover / Focus / Pressed / Disabled / Loading / Empty / Error / Success / Warning / Permission Required

动画只用于表达真实状态变化。

禁止：过度装饰 / 无意义的渐变 / 虚假进度条 / 假 Agent 状态 / 不工作的按钮 / 没有数据的图表 / 仅为截图设计的静态界面

完整支持键盘导航、屏幕阅读器语义、字体缩放和减少动态效果。

---
