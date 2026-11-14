# HibOS Shell 架构文档

**项目**: HibOS — Rust GPU-Centric 宏内核 AI OS  
**状态**: 设计文档 (P1 Shell 基础)

---

## 1. Shell 定位

Shell 是 **System Command Interface + Programmable Execution Environment + Agent Control Interface**。

不能只实现传统命令集合。Shell 必须：

1. 提供人类可读的交互式命令行
2. 支持脚本、管道、重定向的可编程执行环境
3. 提供 Agent 控制命令 (agent list/start/stop, task list/cancel, capability list 等)
4. 支持 JSON 输出 (`--json`)，供机器读取
5. 不占用唯一交互通道（长任务后台运行，用户可继续输入）

---

## 2. 架构概览

```
用户输入
    │
    ▼
┌─────────────────┐
│   Line Editor   │  (history, completion, Ctrl+C/D/R/L)
│   (read_line)   │
└────────┬────────┘
         │
    ┌────▼────┐
    │ Parser  │  (不使用 split_whitespace，支持引号/转义/管道/重定向)
    └────┬────┘
         │
    ┌────▼────┐
    │ Executor│  (built-in / native / host / agent commands)
    └────┬────┘
         │
    ┌────▼────┐
    │ Capability Registry (统一系统能力层)
    └────┬────┘
         │
    ┌────▼────┐
    │ System Services / Host OS
    └─────────┘
```

---

## 3. Shell 语言能力

### 3.1 解析器要求

**不得通过简单字符串 split 实现 Shell Parser。**

必须支持：

| 特性 | 说明 | 状态 |
|------|------|------|
| Command Parsing | 命令与参数解析 | 需实现 |
| Arguments | 位置参数和选项参数 | 需实现 |
| Quoting | 单引号/双引号 | MISSING |
| Escaping | 反斜杠转义 | MISSING |
| Variables | `$VAR` / `${VAR}` | MISSING |
| Environment Variables | `export`/`env`/`unset` | PARTIAL |
| Command Substitution | `$(cmd)` | MISSING |
| Pipes | `\|` 管道 | MISSING |
| Redirection | `>`/`<` | MISSING |
| Append Redirection | `>>` | MISSING |
| Command Chaining | `&&`/`\|/`/`;` | MISSING |
| Conditional Execution | `if`/`&&`/`\|` | MISSING |
| Glob/Wildcards | `*`/`?`/`[...]` | MISSING |
| Exit Codes | 每条命令返回退出码 | PARTIAL |
| Background Jobs | `&` 后台作业 | MISSING |
| Signals | Ctrl+C/SIGINT | MISSING |
| Script Execution | 脚本文件执行 | MISSING |

### 3.2 命令分类

| 类别 | 说明 | 示例 |
|------|------|------|
| Shell Built-ins | Shell 内部命令 | `cd`, `export`, `alias`, `exit` |
| Native OS Commands | HibOS 原生命令 (通过能力层) | `ls`, `cat`, `rm`, `ps`, `kill` |
| Host OS Commands | 宿主 OS 命令 (透传) | 通过 `host:` 前缀 |
| Agent Commands | Agent 控制命令 | `agent list`, `task cancel` |

---

## 4. 基础命令体系 (目标)

### 4.1 文件系统

pwd, ls, cd, mkdir, touch, cp, mv, rm, rmdir, find, tree, stat, file, du, df, cat, head, tail, less, grep, diff

### 4.2 进程

ps, top, kill, jobs, fg, bg, wait, pgrep, pkill, env, export, time

### 4.3 系统

sysinfo, uptime, hostname, whoami, date, status, logs, services, devices, memory, storage, network

### 4.4 应用

app list, app launch, app stop, app info, app install, app uninstall

### 4.5 Agent

agent list, agent inspect, agent create, agent start, agent stop, agent pause, agent resume, agent logs, agent permissions, agent tools

### 4.6 Task

task list, task inspect, task create, task run, task cancel, task pause, task resume, task logs

### 4.7 System Capability

capability list, capability inspect, capability permissions, capability test

---

## 5. 交互体验

| 特性 | 说明 | 状态 |
|------|------|------|
| Command History | 历史记录 | COMPLETE (line_editor) |
| History Search | Ctrl+R 搜索 | PARTIAL |
| Tab Completion | 命令/路径补全 | COMPLETE |
| Double Tab Suggestions | 双 Tab 显示候选 | MISSING |
| Command Suggestions | `--help` 式提示 | PARTIAL |
| Argument Completion | 参数补全 | PARTIAL |
| Path Completion | 路径补全 | COMPLETE |
| Syntax Highlighting | ANSI 语法高亮 | MISSING |
| Error Highlighting | 错误高亮 | MISSING |
| Inline Hints | 内联提示 | PARTIAL |
| Command Help | `help <命令>` | COMPLETE |
| Prompt Customization | 自定义 prompt | PARTIAL |
| Alias | 别名 | MISSING |
| Keyboard Shortcuts | Ctrl+C/D/R/L, ↑↓, Tab | PARTIAL |
| Copy / Paste | 剪贴板 | MISSING |
| Search Output | 输出搜索 | MISSING |
| Multiple Tabs | 多标签 | MISSING |
| Multiple Sessions | 多会话 | MISSING |
| Long Running Task Status | 长任务状态 | MISSING |
| Process Interruption | 进程中断 | MISSING |
| Terminal Resize | 终端尺寸变化 | PARTIAL |
| PTY | 真 PTY | MISSING |
| 交互式程序 | vim/less 等 | MISSING |
| JSON Output | `--json` 机器可读输出 | MISSING |
| Machine-readable Errors | 结构化错误 | MISSING |
| Structured Progress | 进度事件 | MISSING |
| Cancellation | 任务取消 | MISSING |
| Permission Prompts | 授权确认 | MISSING |
| Dry Run | `--dry-run` | MISSING |

---

## 6. Shell 与 Agent 集成

### 6.1 Agent 原生命令

```
agent list --json
agent inspect <id>
agent create <name> <role>
agent start <id>
agent stop <id>
agent pause <id>
agent resume <id>
agent logs <id> [--follow]
agent permissions <id>
agent tools <id>

task list --json
task inspect <id>
task create <description> [--agent <id>]
task run <id>
task cancel <id>
task pause <id>
task resume <id>
task logs <id>

capability list --json
capability inspect <id>
capability permissions <id>
capability test <id>
```

### 6.2 并发执行模型

Agent 执行的长任务不得占用 Shell 的唯一交互通道。用户应能在任务运行时继续输入其他命令。使用后台作业 + 任务 ID 实现。

---

## 7. 与现有代码的映射

| 组件 | 现有代码 | 状态 | 计划 |
|------|---------|------|------|
| Parser | `kernel/shell.rs:310` (`split_whitespace`) | PARTIAL | 重写为真正的解析器 |
| Executor | `kernel/shell.rs:310` (`dispatch`) | PARTIAL | 增加管道/重定向/后台作业 |
| Line Editor | `kernel/line_editor.rs` | COMPLETE | 保留，增加 Ctrl+C/R/L |
| History | `kernel/line_editor.rs` | COMPLETE | 增加 Ctrl+R 搜索 |
| Completion | `kernel/shell.rs:283` | COMPLETE | 增加参数补全 |
| Built-in Commands | `kernel/shell.rs:317-359` | PARTIAL | 增加 agent/task/capability 命令 |
| PTY | — | MISSING | 新建 PTY 层 |
| JSON Output | — | MISSING | 增加 `--json` 标志 |
| Capability Layer | `kernel/capability/` (新建) | MISSING | 集成统一能力层 |

---
