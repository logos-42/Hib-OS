# HibOS Agent-Native 交互文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P3 Agent 交互)

---

## 1. 设计目标

Agent 必须能够原生操作系统，而不是依靠模拟鼠标和键盘作为主要操作方式。

建立结构化能力接口，Agent 通过原生系统能力接口理解环境、获取授权、调用工具、创建任务、协作执行并反馈结果。

---

## 2. Agent 原生能力接口

### 2.1 文件系统

```
filesystem.list     { path }          → { entries: [{name, type, size, mtime}] }
filesystem.read     { path }          → { content: base64|bytes }
filesystem.write    { path, content } → { bytes_written }
filesystem.search   { path, pattern } → { matches: [{path, score}] }
filesystem.move     { src, dst }      → { success }
filesystem.copy     { src, dst }      → { bytes_copied }
filesystem.delete   { path }          → { success }  (高风险，需授权)
```

### 2.2 进程

```
process.list   {}                       → { processes: [{pid, name, cpu, mem, state}] }
process.inspect { pid }                → { process: {...} }
process.start   { command, args }      → { pid }
process.stop    { pid, signal }        → { success }
```

### 2.3 应用

```
application.list   {}            → { apps: [{id, name, version}] }
application.launch { app_id }    → { window_id }
application.close  { window_id } → { success }
```

### 2.4 窗口

```
window.list    {}            → { windows: [{id, title, state, x, y, w, h}] }
window.inspect { id }        → { window: {...} }
window.focus   { id }        → { success }
window.move    { id, x, y }  → { success }
window.resize  { id, w, h }  → { success }
```

### 2.5 任务

```
task.create    { description, agent_id }      → { task_id }
task.inspect   { task_id }                   → { task: {...} }
task.cancel    { task_id }                   → { success }
task.pause     { task_id }                   → { success }
task.resume    { task_id }                   → { success }
```

### 2.6 Agent

```
agent.list     {}            → { agents: [{id, name, role, state, model}] }
agent.inspect  { id }        → { agent: {...} }
agent.start    { id }        → { success }
agent.stop     { id }        → { success }
agent.delegate { from, to, task } → { delegation_id }
```

### 2.7 系统

```
system.status      {}          → { status: {...} }
system.resources   {}          → { cpu, memory, storage, network }
system.logs        { level, limit } → { entries: [...] }
system.settings.read  { key }   → { value }
system.settings.update { key, value } → { success }
```

---

## 3. 所有接口必须支持

| 特性 | 说明 |
|------|------|
| Schema Validation | 输入/输出使用 JSON Schema 校验 |
| Permission Validation | 执行前检查调用者权限 |
| Structured Results | 结构化返回结果 |
| Structured Errors | 结构化错误 (CapabilityError) |
| Cancellation | 支持 CancellationToken 取消 |
| Timeout | 支持超时控制 |
| Progress | 支持进度事件 (CapabilityEvent) |
| Audit Trail | 每次调用记录审计信息 |

---

## 4. Agent API 与 Shell 命令的关系

Agent API 与 Shell 命令可以共享能力定义 (Capability Registry)，但不要求使用完全相同的调用协议。

| 方面 | Shell | Agent API |
|------|-------|-----------|
| 协议 | 文本命令 + 参数 | 结构化 JSON 请求/响应 |
| 输出 | 文本 (可选 `--json`) | 结构化 JSON |
| 错误 | 文本错误信息 | CapabilityError 结构 |
| 授权 | 交互式提示 (y/n) | 结构化授权请求 (permission_required) |
| 进度 | 终端输出 | 事件流 (CapabilityEvent) |

**不要为每个 GUI 按钮创建一个孤立的 Agent Tool。** 应该建立可组合、可复用的系统能力。

---

## 5. Agent 生命周期状态

```
Created → Initializing → Idle → Running → Waiting → Blocked → Paused → Completed / Failed → Stopped
```

- **Created**: Agent 已创建，未初始化
- **Initializing**: 正在加载模型、上下文、工具
- **Idle**: 空闲，等待任务
- **Running**: 正在执行任务
- **Waiting**: 等待外部事件（用户输入、其他 Agent 完成、权限批准）
- **Blocked**: 被阻塞（权限缺失、资源不足、依赖未完成）
- **Paused**: 用户暂停
- **Completed**: 任务成功完成
- **Failed**: 任务失败
- **Stopped**: 用户停止，Agent 已退出

---

## 6. Agent 权限模型 (摘要，详见 permission-model.md)

每个 Agent 必须有：

- Agent ID
- Owner / Principal
- Capability Set
- Permission Scope
- Execution Context
- Resource Quota
- Lifecycle State
- Audit Trail

Agent 只能使用明确授权的系统能力。高风险操作必须经过授权。

---

## 7. 多 Agent 协作

Agent 之间可以：

- **Delegation**: 一个 Agent 委托任务给另一个 Agent (`agent.delegate`)
- **Communication**: 点对点消息通信
- **Shared Resources**: 共享文件、窗口、任务
- **Barrier Synchronization**: 认知对齐 (现有 `barrier` 机制)

协作关系必须来自真实数据，用于 Agent Collaboration Visualization 界面。

---

## 8. 与现有代码的映射

| 组件 | 现有代码 | 状态 | 计划 |
|------|---------|------|------|
| Agent Registry | `kernel/agent.rs:81` (`Orchestrator`) | COMPLETE | 扩展身份/权限/配额 |
| Agent Lifecycle | `kernel/agent.rs:30` (`AgentState`) | PARTIAL | 扩展为完整状态机 |
| Agent Spawn | `kernel/agent.rs:136` (`spawn`) | COMPLETE | 保留 |
| Agent Mailbox | `kernel/agent.rs:61` (`mailbox`) | COMPLETE | 保留 |
| Agent Barrier | `kernel/agent.rs:384` (`barrier`) | COMPLETE | 保留 |
| Agent Submit/Broadcast | `kernel/agent.rs:428/446` | COMPLETE | 保留 |
| Tool Registry | — | MISSING | 新建 |
| MCP Integration | — | MISSING | 新建 |
| Capability Interface | `kernel/capability/` (新建) | MISSING | 新建系统能力层 |
| Event Streaming | — | MISSING | 新建事件总线 |

---
