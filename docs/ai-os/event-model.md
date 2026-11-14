# HibOS 事件模型文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P0 架构基础)

---

## 1. 设计目标

系统需要统一的事件系统，支持订阅、过滤、重放（有持久化时）和实时推送。

GUI 应能订阅事件，而不是不断轮询所有系统状态。
Shell 应能查询任务和事件。
Agent 应能订阅自己有权限访问的事件。

---

## 2. 事件结构

```rust
pub struct SystemEvent {
    pub event_id: Uuid,              // 全局唯一事件 ID
    pub timestamp: u64,              // 纳秒时间戳
    pub source: EventSource,         // 事件来源 (Agent/Shell/GUI/System)
    pub event_type: EventType,       // 事件类型
    pub correlation_id: Option<Uuid>, // 关联 ID (用于追踪同一任务的事件链)
    pub payload: serde_json::Value,   // 事件载荷
    pub schema_version: u32,          // Schema 版本
}

pub enum EventSource {
    Agent { agent_id: AgentId },
    Shell { session_id: String },
    GUI { session_id: String },
    System,
}

pub enum EventType {
    // Agent 生命周期
    AgentStarted,
    AgentStopped,
    AgentStateChanged,
    // Task 生命周期
    TaskCreated,
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    TaskPaused,
    TaskResumed,
    // 工具调用
    ToolInvoked,
    ToolCompleted,
    ToolFailed,
    // 权限
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    PermissionRevoked,
    // 文件系统
    FileCreated,
    FileChanged,
    FileDeleted,
    FileMoved,
    // 进程
    ProcessStarted,
    ProcessExited,
    ProcessSignaled,
    // 窗口
    WindowCreated,
    WindowClosed,
    WindowFocused,
    WindowMoved,
    WindowResized,
    // 系统资源
    SystemResourceChanged,
    // 自定义
    Custom { name: String },
}
```

---

## 3. 事件总线 (Event Bus)

```rust
pub trait EventSubscriber: Send + Sync {
    fn on_event(&self, event: &SystemEvent);
}

pub struct EventBus {
    subscribers: Vec<Box<dyn EventSubscriber>>,
    filters: HashMap<SubscriptionId, EventFilter>,
    history: Option<EventHistory>,  // 持久化时启用重放
}

pub struct EventFilter {
    pub event_types: Option<Vec<EventType>>,
    pub sources: Option<Vec<EventSource>>,
    pub correlation_ids: Option<Vec<Uuid>>,
    pub min_priority: Option<u8>,
}

impl EventBus {
    pub fn subscribe(&self, filter: EventFilter, subscriber: Box<dyn EventSubscriber>) -> SubscriptionId;
    pub fn unsubscribe(&self, id: SubscriptionId);
    pub fn publish(&self, event: SystemEvent);
    pub fn query(&self, filter: EventFilter, time_range: TimeRange) -> Vec<SystemEvent>;
    pub fn replay(&self, from_event_id: Uuid) -> impl Iterator<Item = SystemEvent>;
}
```

---

## 4. 必须支持的事件

| 事件类型 | 说明 | 载荷示例 |
|----------|------|----------|
| AgentStarted | Agent 启动 | { agent_id, name, role } |
| AgentStopped | Agent 停止 | { agent_id, exit_code } |
| AgentStateChanged | Agent 状态变更 | { agent_id, from, to } |
| TaskCreated | 任务创建 | { task_id, description, agent_id } |
| TaskStarted | 任务开始 | { task_id, started_at } |
| TaskProgress | 任务进度 | { task_id, percent, message } |
| TaskCompleted | 任务完成 | { task_id, result, duration_ms } |
| TaskFailed | 任务失败 | { task_id, error, error_code } |
| TaskCancelled | 任务取消 | { task_id, cancelled_by } |
| ToolInvoked | 工具调用 | { agent_id, tool, input } |
| ToolCompleted | 工具完成 | { agent_id, tool, output, duration_ms } |
| PermissionRequested | 权限请求 | { agent_id, permission, mode } |
| PermissionGranted | 权限批准 | { agent_id, permission, mode } |
| PermissionDenied | 权限拒绝 | { agent_id, permission, reason } |
| FileCreated | 文件创建 | { path, actor } |
| FileChanged | 文件变更 | { path, actor, size } |
| FileDeleted | 文件删除 | { path, actor } |
| ProcessStarted | 进程启动 | { pid, command } |
| ProcessExited | 进程退出 | { pid, exit_code } |
| WindowCreated | 窗口创建 | { id, title } |
| WindowClosed | 窗口关闭 | { id } |
| SystemResourceChanged | 系统资源变更 | { cpu, memory, storage } |

---

## 5. 订阅示例

### GUI 订阅 Agent 事件

```rust
let filter = EventFilter {
    event_types: Some(vec![EventType::AgentStarted, EventType::AgentStopped, EventType::AgentStateChanged]),
    sources: None,
    correlation_ids: None,
};
event_bus.subscribe(filter, Box::new(GuiAgentSubscriber));
```

### Agent 订阅自身任务事件

```rust
let filter = EventFilter {
    event_types: Some(vec![EventType::TaskStarted, EventType::TaskProgress, EventType::TaskCompleted]),
    sources: Some(vec![EventSource::System]),
    correlation_ids: Some(vec![task_correlation_id]),
};
event_bus.subscribe(filter, Box::new(AgentTaskSubscriber));
```

### Shell 查询事件

```rust
// Shell 命令：event list --type TaskCompleted --limit 10
let events = event_bus.query(filter, time_range);
```

---

## 6. 与现有代码的映射

| 组件 | 现有代码 | 状态 |
|------|---------|------|
| 事件总线 | — | MISSING |
| 事件结构 | — | MISSING |
| 订阅/过滤 | — | MISSING |
| 事件重放 | — | MISSING |
| GUI 轮询 → 订阅 | `kernel/gui/mod.rs:234` (`refresh_live`) | PARTIAL (目前是轮询刷新) |
| Agent 邮箱 | `kernel/agent.rs:71` (`mailbox: VecDeque<String>`) | PARTIAL (仅 Agent 内部) |

---

## 7. 实现计划 (P0)

1. 创建 `kernel/event/` 模块
2. 定义 `SystemEvent`, `EventType`, `EventSource`
3. 实现 `EventBus` (发布/订阅/过滤/查询)
4. 将现有轮询机制 (`refresh_live` 等) 迁移为事件订阅
5. 在 CapabilityRegistry 执行路径中发布事件
6. 为 Shell 提供 `event list/query` 命令

---
