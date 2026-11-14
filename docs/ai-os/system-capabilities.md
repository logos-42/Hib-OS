# HibOS 统一系统能力层 (System Capability Layer)

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P0 架构基础)

---

## 1. 设计目标

Shell、GUI、Agent API 不应各自直接实现系统操作。建立统一的系统能力层，三种入口通过同一套能力注册表调用底层系统服务。

逻辑架构：

```
User / Agent
    |
    +-- GUI Interface
    |
    +-- Shell Interface
    |
    +-- Agent Interface
    |
    v
Interaction Gateway
    |
    v
System Capability Registry
    |
    +-- Filesystem Capability
    +-- Process Capability
    +-- Application Capability
    +-- Network Capability
    +-- Device Capability
    +-- Window Capability
    +-- Storage Capability
    +-- Settings Capability
    +-- Notification Capability
    +-- Agent Capability
    +-- Task Capability
    |
    v
System Services / Host OS / Kernel
```

注意：这是逻辑架构，不要求所有功能通过单一中心进程。合理使用 Rust 模块、Actor、异步消息和 IPC。

---

## 2. 统一执行抽象

每项能力必须定义以下要素：

- Capability ID
- Description
- Input Schema
- Output Schema
- Error Schema
- Permission Requirements
- Execution Context
- Timeout
- Cancellation
- Progress Events
- Audit Information

### 2.1 核心类型

```rust
/// 能力 ID，全局唯一，格式为 `domain.action`
pub type CapabilityId = String;

/// 能力请求
pub struct CapabilityRequest {
    pub id: CapabilityId,
    pub input: serde_json::Value,
    pub context: ExecutionContext,
    pub timeout: Option<Duration>,
    pub cancellation_token: CancellationToken,
    pub audit: AuditInfo,
}

/// 能力响应
pub struct CapabilityResponse {
    pub request_id: Uuid,
    pub output: serde_json::Value,
    pub duration_ms: u64,
    pub audit: AuditInfo,
}

/// 能力错误
pub struct CapabilityError {
    pub code: ErrorCode,
    pub message: String,
    pub source: Option<String>,
    pub retryable: bool,
    pub audit: AuditInfo,
}

/// 进度事件
pub struct CapabilityEvent {
    pub event_id: Uuid,
    pub timestamp: u64,
    pub capability_id: CapabilityId,
    pub event_type: EventType,
    pub correlation_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

/// 执行上下文
pub struct ExecutionContext {
    pub caller_id: String,        // Agent ID / Shell session / GUI session
    pub caller_type: CallerType,
    pub permissions: Vec<Permission>,
    pub resource_quota: ResourceQuota,
}

/// 审计信息
pub struct AuditInfo {
    pub actor: String,
    pub action: String,
    pub timestamp: u64,
    pub result: AuditResult,
}
```

### 2.2 能力注册表

```rust
pub trait SystemCapability: Send + Sync {
    fn id(&self) -> CapabilityId;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;   // JSON Schema
    fn output_schema(&self) -> serde_json::Value;  // JSON Schema
    fn error_schema(&self) -> serde_json::Value;
    fn required_permissions(&self) -> Vec<Permission>;
    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError>;
}

pub struct CapabilityRegistry {
    capabilities: HashMap<CapabilityId, Box<dyn SystemCapability>>,
}
```

---

## 3. 能力定义清单

| Capability ID | 说明 | 输入 | 输出 | 权限要求 |
|--------------|------|------|------|----------|
| `filesystem.list` | 列出目录 | { path } | { entries: [...] } | fs:read:<path> |
| `filesystem.read` | 读取文件 | { path } | { content: bytes } | fs:read:<path> |
| `filesystem.write` | 写入文件 | { path, content } | { bytes_written } | fs:write:<path> |
| `filesystem.search` | 搜索文件 | { path, pattern } | { matches: [...] } | fs:read:<path> |
| `filesystem.move` | 移动文件 | { src, dst } | { success } | fs:write:<src>, fs:write:<dst> |
| `filesystem.copy` | 复制文件 | { src, dst } | { bytes_copied } | fs:read:<src>, fs:write:<dst> |
| `filesystem.delete` | 删除文件 | { path } | { success } | fs:write:<path> (高风险，需授权) |
| `process.list` | 列出进程 | {} | { processes: [...] } | proc:read |
| `process.inspect` | 查看进程 | { pid } | { process } | proc:read |
| `process.start` | 启动进程 | { command, args } | { pid } | proc:exec |
| `process.stop` | 停止进程 | { pid, signal } | { success } | proc:control:<pid> |
| `application.list` | 列出应用 | {} | { apps: [...] } | app:read |
| `application.launch` | 启动应用 | { app_id } | { window_id } | app:launch:<app_id> |
| `application.close` | 关闭应用 | { window_id } | { success } | app:control:<window_id> |
| `window.list` | 列出窗口 | {} | { windows: [...] } | win:read |
| `window.inspect` | 查看窗口 | { id } | { window } | win:read |
| `window.focus` | 聚焦窗口 | { id } | { success } | win:control:<id> |
| `window.move` | 移动窗口 | { id, x, y } | { success } | win:control:<id> |
| `window.resize` | 调整窗口 | { id, w, h } | { success } | win:control:<id> |
| `task.create` | 创建任务 | { description, agent_id } | { task_id } | task:create |
| `task.inspect` | 查看任务 | { task_id } | { task } | task:read:<task_id> |
| `task.cancel` | 取消任务 | { task_id } | { success } | task:control:<task_id> |
| `task.pause` | 暂停任务 | { task_id } | { success } | task:control:<task_id> |
| `task.resume` | 恢复任务 | { task_id } | { success } | task:control:<task_id> |
| `agent.list` | 列出 Agent | {} | { agents: [...] } | agent:read |
| `agent.inspect` | 查看 Agent | { id } | { agent } | agent:read |
| `agent.start` | 启动 Agent | { id } | { success } | agent:control:<id> |
| `agent.stop` | 停止 Agent | { id } | { success } | agent:control:<id> |
| `agent.delegate` | 委托任务 | { from, to, task } | { delegation_id } | agent:delegate |
| `system.status` | 系统状态 | {} | { status } | sys:read |
| `system.resources` | 资源使用 | {} | { cpu, memory, storage, network } | sys:read |
| `system.logs` | 系统日志 | { level, limit } | { entries: [...] } | sys:read:logs |
| `settings.read` | 读取设置 | { key } | { value } | settings:read:<key> |
| `settings.update` | 更新设置 | { key, value } | { success } | settings:write:<key> |

---

## 4. 接口标准

### 4.1 输入/输出 Schema

所有能力使用 JSON Schema 定义输入和输出，供 Shell (`--json`)、GUI、Agent API 共享校验逻辑。

### 4.2 权限检查

每个能力执行前，Interaction Gateway 检查调用者是否持有 `required_permissions` 中的所有权限。权限检查结果记录入审计日志。

### 4.3 超时与取消

每个请求可指定 `timeout`，超时自动返回 `CapabilityError { code: Timeout, retryable: true }`。`CancellationToken` 支持主动取消长耗时操作。

### 4.4 进度事件

长耗时能力执行过程中通过 `CapabilityEvent` 发送进度（EventType::Progress)，GUI 可订阅并显示进度条，Shell 可周期性查询，Agent 可收到流式事件。

---

## 5. 与现有代码的映射

| 能力 | 现有代码 | 封装方式 | 状态 |
|------|---------|---------|------|
| filesystem.* | `kernel/vfs.rs` | 封装 VFS 操作为能力 | PARTIAL |
| process.* | `kernel/syscall/mod.rs` | 封装 AiSystemCall::CreateProcess/QueryProcess | PARTIAL |
| application.* | `kernel/gui/apps.rs` | 封装应用启动/关闭 | PARTIAL |
| window.* | `kernel/gui/wm.rs` | 封装窗口管理操作 | PARTIAL |
| agent.* | `kernel/agent.rs` | 封装 spawn/task/stop | PARTIAL |
| task.* | — (新建) | 新建任务管理器 | MISSING |
| settings.* | — (新建) | 新建设置能力 | MISSING |
| notification.* | — (新建) | 新建通知能力 | MISSING |
| system.* | `kernel/monitor.rs` | 封装监控统计 | PARTIAL |

---

## 6. 实现计划 (P0)

1. 创建 `kernel/capability/` 模块，定义 `CapabilityRequest/Response/Error/Event/ExecutionContext/AuditInfo`
2. 创建 `CapabilityRegistry` 和 `SystemCapability` trait
3. 封装现有 VFS、syscall、wm、agent 能力为 `SystemCapability` 实现
4. 创建 `Interaction Gateway` 路由层，处理权限检查、超时、取消、审计
5. 为 Shell、GUI、Agent 各提供适配器，调用统一能力层

---
