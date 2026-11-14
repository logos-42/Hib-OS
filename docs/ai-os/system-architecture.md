# HibOS 系统架构文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**版本**: 0.2.0 → 1.0 (Agent-Native)  
**类别**: A (Rust 自研内核，裸机 x86_64 no_std + 宿主 OS std 原型栈)

---

## 1. 架构定位

HibOS 不是传统 Linux Desktop 的复刻，也不是给现有 OS 添加的 AI 聊天插件。

它是一个 **以 Agent 为核心运行主体、以人类意图为输入、以系统能力为执行基础的 Agent-Native Operating System**。

产品范式：

```
Human Intent → Agent → System Capabilities → Execution → Feedback
```

三种交互入口共享同一套 **System Capability Layer**：

- **GUI** — Human Control Plane（人类控制平面）
- **Shell** — Programmable System Interface（可编程系统接口）
- **Agent API** — Native Intelligence Interface（原生智能接口）

---

## 2. 责任边界 (Host OS vs AI OS)

HibOS 运行于宿主操作系统之上（裸机内核 + 宿主 std 原型双栈）。责任边界如下：

| 责任域 | 宿主 OS (Linux/Windows) | HibOS (AI OS 层) |
|--------|------------------------|------------------|
| 硬件中断/驱动 | 由宿主 OS 管理 | 通过宿主 OS 系统调用访问硬件 |
| 内存页表/MMU | 由宿主 OS 管理 | 逻辑地址空间管理（在宿主进程内）|
| 进程调度 | 由宿主 OS 管理 | Agent/Task 调度（用户级调度器）|
| 文件系统 | 宿主文件系统 | VFS 抽象层，映射到宿主路径或自有 SimpleFS |
| 网络栈 | 宿主网络栈 | 通过宿主 socket 访问网络 |
| 图形输出 | 宿主窗口系统 (winit/softbuffer) | 自有合成器 + 像素管道 |
| 终端/PTY | 宿主终端模拟器 | 自有 PTY 层 + 线编辑器 |
| Agent Runtime | — | 独立运行，不依赖宿主进程模型 |

**关键原则**：即使 LLM API 不可用，GUI、Shell、文件管理、进程管理必须继续工作。AI 不能成为基础系统功能的单点故障。

---

## 3. 逻辑架构

```
┌─────────────────────────────────────────────────────────────┐
│                    User / Human / Agent                      │
└──────────────────────────────┬──────────────────────────────┘
                               │
        ┌──────────────────────┼──────────────────────┐
        │                      │                      │
    ┌───▼───┐            ┌────▼────┐            ┌───▼───┐
    │ GUI   │            │ Shell   │            │ Agent │
    │入口   │            │入口     │            │入口   │
    └───┬───┘            └────┬────┘            └───┬───┘
        │                      │                      │
        └──────────────────────┴──────────────────────┘
                               │
                    ┌──────────▼──────────┐
                    │ Interaction Gateway │
                    │ (路由 + 权限 + 审计)   │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │ System Capability   │
                    │ Registry            │
                    │                     │
                    │ ┌───────────────┐   │
                    │ │ Filesystem    │   │
                    │ │ Process       │   │
                    │ │ Application   │   │
                    │ │ Network       │   │
                    │ │ Device        │   │
                    │ │ Window        │   │
                    │ │ Storage       │   │
                    │ │ Settings      │   │
                    │ │ Notification  │   │
                    │ │ Agent         │   │
                    │ │ Task          │   │
                    │ └───────────────┘   │
                    └──────────┬──────────┘
                               │
        ┌──────────────────────┴──────────────────────┐
        │  System Services / Host OS / Kernel (Rust)   │
        └─────────────────────────────────────────────┘
```

注意：这是**逻辑架构**，不要求所有功能通过单一中心进程。合理使用 Rust 模块、Actor、异步消息和 IPC。

---

## 4. Crate 模块划分 (Rust 工程)

| Crate/模块 | 路径 | 职责 | 依赖 |
|-----------|------|------|------|
| `hibos` (根) | `lib.rs` / `kernel/mod.rs` | crate 根，重导出所有内核模块 | alloc |
| `capability` | `kernel/capability/` (新建) | 统一系统能力层：CapabilityRequest/Response/Error/Event | serde, serde_json |
| `event` | `kernel/event/` (新建) | 统一事件总线：订阅、过滤、重放 | capability |
| `permission` | `kernel/permission/` (新建) | 权限模型：Agent 身份、能力集、作用域、配额 | capability |
| `shell` | `kernel/shell.rs` | Shell 解析器、执行器、内置命令 | line_editor, vfs, capability |
| `gui` | `kernel/gui/` | 合成器、窗口管理器、终端、文件管理器 | surface, wm, window |
| `agent` | `kernel/agent.rs` | Agent 编排器、生命周期、邮箱、屏障 | exec, scheduler |
| `channels` | `kernel/channels.rs` | 统一感知通道 (sense/act) | vfs, gpu, net |
| `syscall` | `kernel/syscall/` | AiSystemCall 枚举与结果 | ai_token, gpu, scheduler |
| `vfs` | `kernel/vfs.rs` | 虚拟文件系统 | alloc |
| `exec` | `kernel/exec.rs` | 执行体 spwan/join/signal | scheduler |
| `monitor` | `kernel/monitor.rs` | 系统统计与运行时间 | time |

**依赖规则**：避免循环依赖。`capability`/`event`/`permission` 是底层，不依赖 `shell`/`gui`/`agent`；上层依赖下层。

---

## 5. 异步与并发模型

- 使用 **Tokio** 作为异步运行时 (宿主 std 模式)。
- 裸机内核模式使用自定义 `async_runtime` (kernel/async_runtime.rs)。
- 使用 **Bounded Channels** 实现背压 (backpressure)。
- 使用 **CancellationToken** 实现任务取消。
- 避免将 CPU 密集型任务直接放入异步执行线程（使用 `spawn_blocking`）。
- GUI 与 Runtime 的通信通过稳定边界（channel + IPC 协议）。

---

## 6. 数据流

### 6.1 人类通过 GUI 删除文件

```
GUI (PermissionDialog) → Interaction Gateway → CapabilityRegistry
  → Filesystem Capability → VFS → Host OS / Disk
  → Event: FileDeleted → Event Bus → GUI (刷新) + Shell (查询) + Agent (订阅)
```

### 6.2 Agent 调用文件读取

```
Agent → Agent API → CapabilityRegistry → Filesystem Capability
  → Permission Check (Agent 的 Capability Set) → VFS → 返回内容
  → Event: ToolInvoked / ToolCompleted → Event Bus
```

### 6.3 Shell 执行命令

```
Shell → Parser → Executor → CapabilityRegistry → 对应 Capability
  → 返回 CapabilityResponse → Shell 格式化输出 (文本或 JSON)
```

---

## 7. 错误模型

所有错误使用统一的 `CapabilityError`：

```rust
pub struct CapabilityError {
    pub code: ErrorCode,
    pub message: String,
    pub source: Option<String>,
    pub retryable: bool,
    pub audit_info: AuditInfo,
}
```

---

## 8. 与现有代码的映射

| 架构组件 | 现有代码位置 | 状态 |
|---------|------------|------|
| Interaction Gateway | — (新建) | MISSING |
| Capability Registry | — (新建) | MISSING |
| Filesystem Capability | `kernel/vfs.rs` | PARTIAL |
| Process Capability | `kernel/syscall/mod.rs` | PARTIAL |
| Window Capability | `kernel/gui/wm.rs` | PARTIAL |
| Agent Capability | `kernel/agent.rs` | PARTIAL |
| Event Bus | — (新建) | MISSING |
| Permission Model | — (新建) | MISSING |
| Shell Parser/Executor | `kernel/shell.rs` | PARTIAL |
| GUI Compositor | `kernel/gui/compositor.rs` | COMPLETE |
| Window Manager | `kernel/gui/wm.rs` | COMPLETE |

---
