# HibOS 权限模型文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P0 架构基础)

---

## 1. 设计原则

- **Agent 不等于管理员**。Agent 的权限必须独立管理，采用最小权限原则。
- 每个 Agent 都有明确的身份、能力集、权限范围、资源配额。
- Agent 只能使用明确授权的系统能力。
- 高风险操作必须经过授权（Once / Session / Persistent）。
- 不能默认所有 Agent 拥有全部权限。

---

## 2. Agent 身份模型

```rust
pub struct AgentIdentity {
    pub agent_id: AgentId,        // u64，全局唯一
    pub owner: Principal,         // 拥有者（用户或系统）
    pub name: String,
    pub role: String,
    pub created_at: u64,
}

pub enum Principal {
    User { user_id: String },
    System,
    Agent { agent_id: AgentId },  // 由另一个 Agent 创建
}
```

---

## 3. 权限模型

### 3.1 权限格式

权限字符串格式：`<resource>:<action>:<scope>`

示例：
- `fs:read:/home/user/*` — 读取用户目录下所有文件
- `fs:write:/home/user/docs/*` — 写入文档目录
- `fs:delete:/tmp/*` — 删除临时目录文件 (高风险)
- `proc:exec` — 执行进程
- `proc:control:<pid>` — 控制特定进程
- `net:connect:external` — 连接外部网络 (高风险)
- `sys:settings:write` — 修改系统设置 (高风险)
- `agent:delegate` — 委托任务给其他 Agent
- `win:control:<id>` — 控制特定窗口

### 3.2 授权模式

| 模式 | 说明 | 适用场景 |
|------|------|----------|
| Once | 仅当次操作有效 | 高风险一次性操作 (rm -rf, 格式化) |
| Session | 当前会话有效 | 普通开发操作 |
| Persistent | 持久有效，直到撤销 | 日常使用的信任 Agent |

### 3.3 权限请求流程

```
Agent 请求能力 → Permission Center 检查 → 
  若已有权限 → 直接执行
  若无权限 → 返回 PermissionRequired 事件 → GUI/Shell 显示确认 → 用户批准/拒绝 → 
    批准 → 创建临时/持久授权 → 执行 → 审计记录
    拒绝 → 返回 AccessDenied 错误 → 审计记录
```

---

## 4. 资源配额

每个 Agent 可配置资源配额，防止资源耗尽：

| 配额项 | 说明 |
|--------|------|
| CPU 时间片 | 每个时间窗内的 CPU 使用 |
| 内存 | 最大可分配内存 |
| 存储 | 最大可占用磁盘空间 |
| 网络带宽 | 最大网络 I/O |
| Token 消耗 | LLM API Token 配额 (与 CPU/内存分离) |
| 并发任务数 | 最大同时运行任务数 |

---

## 5. 高风险操作

以下操作属于高风险，必须经过用户授权：

| 操作 | 权限 | 授权模式建议 |
|------|------|-------------|
| 文件删除 (rm) | `fs:delete:<path>` | Once / Session |
| 系统配置修改 | `sys:settings:write` | Once |
| 外部网络访问 | `net:connect:external` | Session |
| 安装软件 | `app:install` | Once |
| 执行高风险命令 | `proc:exec:<command>` | Once |
| Agent 委托 | `agent:delegate` | Session |

---

## 6. 审计轨迹

每次权限检查和执行都记录：

```rust
pub struct AuditEntry {
    pub timestamp: u64,
    pub actor: String,           // Agent ID 或用户
    pub action: String,          // 执行的操作
    pub resource: String,        // 访问的资源
    pub permission: String,      // 检查的权限
    pub result: AuditResult,     // Granted / Denied / Revoked
    pub correlation_id: Option<Uuid>,
}
```

GUI 和 Shell 均可查询执行记录。

---

## 7. 与现有代码的映射

| 组件 | 现有代码 | 状态 |
|------|---------|------|
| Channel 权限 | `kernel/channels.rs` (Access: Read/Write/ReadWrite) | PARTIAL |
| Agent 身份 | `kernel/agent.rs:26` (`AgentId = u64`) | PARTIAL (缺少 owner/principal) |
| 权限检查 | — | MISSING |
| 授权中心 | — | MISSING |
| 审计日志 | — | MISSING |
| 资源配额 | — | MISSING |

---

## 8. 实现计划 (P0)

1. 创建 `kernel/permission/` 模块
2. 定义 `AgentIdentity`, `Permission`, `PermissionScope`, `AuthorizationMode`
3. 实现 `PermissionChecker` (检查调用者是否持有所需权限)
4. 实现 `AuthorizationManager` (授权/撤销/查询)
5. 将权限检查集成到 `CapabilityRegistry` 的执行路径
6. 为每个系统能力定义 `required_permissions`
7. 实现审计日志 (Audit Trail)

---
