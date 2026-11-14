# HibOS 交互测试文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P5 集成)

---

## 1. 测试金字塔

```
        E2E 场景测试 (场景一~六)
       /                          \
  跨接口一致性测试                Agent 集成测试
 /               \                /      |      \
Shell测试        GUI测试         生命周期  任务   权限  事件  恢复
(Parser/Exec/...) (Window/Nav/...) (Agent) (Task) (Cap) (Stream) (Crash)
```

---

## 2. Shell 测试

| 测试 | 说明 | 状态 |
|------|------|------|
| Parser Tests | 测试引号、转义、管道、重定向、变量替换 | MISSING |
| Executor Tests | 测试内置命令、外部命令、退出码 | MISSING |
| Pipeline Tests | 测试 `\|` 管道吞吐 | MISSING |
| Redirection Tests | 测试 `>`/`<`/`>>` 重定向 | MISSING |
| Completion Tests | 测试 Tab 补全、参数补全、路径补全 | MISSING |
| History Tests | 测试历史加载、Ctrl+R 搜索 | MISSING |
| PTY Tests | 测试 PTY、交互式程序、终端尺寸变化 | MISSING |
| Signal Tests | 测试 Ctrl+C/D/R/L、进程中断 | MISSING |
| Process Tests | 测试后台作业、jobs、kill | MISSING |
| JSON Output Tests | 测试 `--json` 机器可读输出 | MISSING |

---

## 3. GUI 测试

| 测试 | 说明 | 状态 |
|------|------|------|
| Window Tests | 测试窗口创建/聚焦/移动/缩放/最小化/最大化/关闭 | COMPLETE (wm.rs 单元测试) |
| Navigation Tests | 测试一级导航切换 | MISSING |
| Keyboard Tests | 测试键盘导航、Alt+Tab、快捷键 | MISSING |
| File Manager Tests | 测试文件浏览、复制、移动、删除、权限 | PARTIAL |
| Agent Workspace Tests | 测试 Agent 列表、状态、授权修改 | MISSING |
| Task Workspace Tests | 测试任务创建、取消、进度、时间线 | MISSING |
| Permission Dialog Tests | 测试权限请求对话框 (Once/Session/Persistent) | MISSING |
| Notification Tests | 测试通知显示和订阅 | MISSING |
| Recovery Tests | 测试 GUI 关闭后 Agent 继续运行，重新打开恢复状态 | MISSING |

---

## 4. Agent 集成测试

| 测试 | 说明 | 状态 |
|------|------|------|
| Agent Lifecycle Tests | 测试 Created→Initializing→Idle→Running→Waiting→Blocked→Paused→Completed/Failed→Stopped | PARTIAL (agent.rs 有基础测试) |
| Task Lifecycle Tests | 测试任务创建/启动/取消/暂停/恢复/重试 | MISSING |
| Delegation Tests | 测试 Agent 之间的任务委托 | MISSING |
| Capability Invocation Tests | 测试 filesystem/process/window/agent/task 能力调用 | MISSING |
| Permission Tests | 测试权限检查、拒绝、授权模式 | MISSING |
| Cancellation Tests | 测试任务取消、CancellationToken | MISSING |
| Event Streaming Tests | 测试事件订阅、过滤、推送 | MISSING |
| Recovery Tests | 测试 Agent 崩溃不影响其他 Agent、Shell、GUI | MISSING |

---

## 5. 跨接口一致性测试

同一操作通过 GUI、Shell、Agent API 三种入口执行，验证最终系统状态一致：

| 操作 | GUI | Shell | Agent API | 验证项 |
|------|-----|-------|-----------|--------|
| 文件删除 | 文件管理器删除 | `rm <path>` | `filesystem.delete` | 最终文件状态、权限判断、错误结构、事件记录、审计信息、取消/超时行为 |
| 文件读取 | 文件预览 | `cat <path>` | `filesystem.read` | 同上 |
| 进程列表 | 系统监控 | `ps` | `process.list` | 同上 |
| 窗口关闭 | 点击关闭按钮 | `gui close <id>` | `window.close` (未来) | 同上 |
| Agent 启动 | Agent Workspace 启动 | `agent start <id>` | `agent.start` | 同上 |

**验收条件**：
1. 最终文件/系统状态一致
2. 权限判断一致
3. 错误结构一致 (CapabilityError)
4. 事件记录一致 (相同 event_type, correlation_id)
5. 审计信息一致 (相同 actor, action, result)
6. 取消和超时行为一致

不同入口可以拥有不同的交互反馈（GUI 确认窗口、Shell 交互提示、Agent API 结构化授权请求），但不得为了接口一致而牺牲各自的交互体验。

---

## 6. 端到端场景测试 (E2E)

### 场景一：GUI 创建 Agent，搜索目录

**步骤**：
1. 用户通过 GUI 创建一个 Agent
2. 授予文件读取权限 (`fs:read:/specified/dir/*`)
3. Agent 搜索指定目录
4. 返回结果

**验收**：
- Agent 出现在 Agent Workspace
- 状态为 Idle → Running → Completed
- 文件搜索结果正确
- 权限审计记录完整

### 场景二：Shell 创建任务，观察状态

**步骤**：
1. 用户通过 Shell 创建任务：`task create "分析日志文件" --agent <id>`
2. 启动任务：`task run <id>`
3. 实时观察任务状态：`task inspect <id>`
4. 获取结构化结果：`task inspect <id> --json`

**验收**：
- 任务 ID 返回
- 状态实时更新 (来自事件系统)
- 结果为结构化 JSON

### 场景三：Agent 删除文件，权限申请

**步骤**：
1. Agent 需要删除文件
2. 系统判断权限 (fs:delete)
3. 向用户申请授权 (GUI 确认窗口 / Shell 交互提示 / Agent API 结构化请求)
4. 用户批准后执行
5. GUI 和 Shell 均可查询执行记录

**验收**：
- 未授权时返回 PermissionDenied
- 授权后执行成功
- GUI 和 Shell 都能查询审计记录
- 事件记录一致

### 场景四：GUI 关闭后 Agent 继续运行

**步骤**：
1. 用户通过 GUI 启动长任务
2. 关闭 GUI 窗口
3. Agent Runtime 继续运行
4. 重新打开 GUI，恢复任务状态

**验收**：
- GUI 关闭后 Agent 不停止
- 任务状态在重新打开 GUI 后可查询
- 进度事件不丢失

### 场景五：模型 API 不可用

**步骤**：
1. 模拟 LLM API 不可用
2. 用户使用 Shell (文件、进程、系统命令)
3. 用户使用文件管理器
4. 用户使用系统设置
5. 用户使用进程管理

**验收**：
- GUI 正常工作
- Shell 正常工作
- 文件管理正常工作
- 进程管理正常工作
- Agent Runtime 报告模型服务不可用
- 本地 Agent 或非模型系统功能仍可运行

### 场景六：Agent 崩溃隔离

**步骤**：
1. 启动多个 Agent
2. 使一个 Agent 崩溃
3. 验证其他 Agent、Shell、GUI 不受影响

**验收**：
- 崩溃的 Agent 标记为 Failed
- 其他 Agent 继续运行
- Shell 和 GUI 正常工作
- 资源被清理

---

## 7. 测试基础设施

| 组件 | 技术选型 | 说明 |
|------|---------|------|
| 单元测试 | Rust `#[cfg(test)]` | 现有代码已有模式 |
| 集成测试 | `tests/` 目录 | 现有 `ai_os_integration.rs` |
| 模拟/Stub | `mockall` 或手动 mock | 用于测试 LLM API 不可用等场景 |
| 断言库 | `assert_eq!`, `assert!` | 标准 Rust |
| 异步测试 | `tokio::test` | 宿主 std 模式 |
| 性能测试 | `benches/` 目标 | 现有 `benches/ai_os_bench.rs` |

---

## 8. 验收标准总览

P0 阶段：架构文档完成，能力层/事件系统/权限模型代码实现并通过单元测试。

P1 阶段：Shell Parser/Executor 单元测试通过，支持 agent/task/capability 命令和 `--json`。

P2 阶段：GUI Agent Workspace / Task Workspace / Permission Center 界面实现，支持事件订阅。

P3 阶段：Agent 完整生命周期、权限配额、任务取消/恢复、MCP 集成测试通过。

P5 阶段：跨接口一致性测试通过，6 个 E2E 场景全部通过，性能基准建立，无障碍支持。

---
