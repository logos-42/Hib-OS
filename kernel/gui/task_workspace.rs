//! Task Workspace —— 任务管理和时间线界面。
//!
//! 展示：Task ID、任务描述、状态、分配的 Agent、子任务、依赖、进度、
//! 执行时间线、日志、结果、错误、资源消耗。
//! 进度来自实际执行事件，不使用虚假递增百分比。

use crate::capability::{CapabilityRequest, ExecutionContext,
    now_ns, AuditInfo, AuditResult, CallerType};
use serde_json::json;

/// 任务摘要。
#[derive(Clone, Debug)]
pub struct TaskSummary {
    pub id: u64,
    pub description: String,
    pub status: String,
    pub agent_id: Option<String>,
    pub progress: u8,
}

/// 渲染 Task Workspace 的文本表示。
pub fn render_task_workspace() -> String {
    let mut out = String::new();
    out.push_str("══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                           Task Workspace\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    let tasks = list_tasks();
    if tasks.is_empty() {
        out.push_str("  (暂无任务)\n");
        out.push_str("\n  创建任务: task create <描述> [--agent <id>]\n");
        return out;
    }

    out.push_str(&format!("  任务总数: {}\n\n", tasks.len()));
    out.push_str("  ID   状态      进度  描述                          分配 Agent\n");
    out.push_str("  --   -----     ----  ----------                   ------------\n");

    for t in &tasks {
        out.push_str(&format!(
            "  {:<3} {:<9} {:<4}% {:<28} {}\n",
            t.id, t.status, t.progress,
            truncate(&t.description, 28),
            t.agent_id.as_deref().unwrap_or("-")
        ));
    }

    out.push_str("\n  可用操作:\n");
    out.push_str("    task create <描述> [--agent <id>]  - 创建任务\n");
    out.push_str("    task inspect <id>                  - 查看任务详情\n");
    out.push_str("    task cancel <id>                   - 取消任务\n");
    out.push_str("    task pause <id>                    - 暂停任务\n");
    out.push_str("    task resume <id>                   - 恢复任务\n");
    out.push_str("    task logs <id>                     - 查看任务日志\n");

    out
}

/// 渲染单个任务的详细信息。
pub fn render_task_detail(task_id: u64) -> String {
    let mut out = String::new();
    out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n"));
    out.push_str(&format!("                         Task #{}\n", task_id));
    out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n\n"));

    let task = inspect_task(task_id);
    match task {
        Some(t) => {
            out.push_str(&format!("  描述: {}\n", t.description));
            out.push_str(&format!("  状态: {}\n", t.status));
            out.push_str(&format!("  进度: {}%\n", t.progress));
            out.push_str(&format!("  分配 Agent: {}\n", t.agent_id.unwrap_or_else(|| "-".to_string())));
            out.push_str(&format!("  创建时间: {} ns\n", t.created_ns));
            if let Some(completed) = t.completed_ns {
                out.push_str(&format!("  完成时间: {} ns\n", completed));
                out.push_str(&format!("  耗时: {} ns\n", completed - t.created_ns));
            }
            if let Some(result) = &t.result {
                out.push_str(&format!("  结果: {}\n", result));
            }
            if let Some(error) = &t.error {
                out.push_str(&format!("  错误: {}\n", error));
            }
            out.push_str(&format!("  资源消耗: CPU={}ns, 内存={}B\n", t.cpu_ns, t.memory_bytes));
            out.push_str("\n  执行时间线:\n");
            for event in &t.timeline {
                out.push_str(&format!("    [{}] {}\n", event.timestamp, event.message));
            }
            out.push_str("\n  可用操作:\n");
            out.push_str(&format!("    task cancel {}    - 取消任务\n", task_id));
            out.push_str(&format!("    task pause {}     - 暂停任务\n", task_id));
            out.push_str(&format!("    task resume {}    - 恢复任务\n", task_id));
            out.push_str(&format!("    task logs {}      - 查看详细日志\n", task_id));
        }
        None => {
            out.push_str(&format!("  未找到任务 #{}\n", task_id));
        }
    }

    out
}

/// 列出所有任务（通过能力层）。
fn list_tasks() -> Vec<TaskSummary> {
    let req = CapabilityRequest {
        id: "task".to_string(),
        input: json!({"op": "list"}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["task:read".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "task.list".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("task") {
        Some(cap) => match cap.execute(req) {
            Ok(resp) => {
                parse_task_list(&resp.output)
            }
            Err(_) => Vec::new(),
        },
        None => Vec::new(),
    }
}

/// 检查单个任务（通过能力层）。
fn inspect_task(task_id: u64) -> Option<TaskDetail> {
    let req = CapabilityRequest {
        id: "task".to_string(),
        input: json!({"op": "inspect", "id": task_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["task:read".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "task.inspect".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("task") {
        Some(cap) => match cap.execute(req) {
            Ok(resp) => parse_task_detail(&resp.output),
            Err(_) => None,
        },
        None => None,
    }
}

/// 取消任务（通过能力层）。
pub fn cancel_task(task_id: u64) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "task".to_string(),
        input: json!({"op": "cancel", "id": task_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["task:control".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "task.cancel".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("task") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("任务 #{} 已取消", task_id)),
            Err(e) => Err(format!("取消失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 task 未注册".to_string()),
    }
}

/// 暂停任务。
pub fn pause_task(task_id: u64) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "task".to_string(),
        input: json!({"op": "pause", "id": task_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["task:control".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "task.pause".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("task") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("任务 #{} 已暂停", task_id)),
            Err(e) => Err(format!("暂停失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 task 未注册".to_string()),
    }
}

/// 恢复任务。
pub fn resume_task(task_id: u64) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "task".to_string(),
        input: json!({"op": "resume", "id": task_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["task:control".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "task.resume".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("task") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("任务 #{} 已恢复", task_id)),
            Err(e) => Err(format!("恢复失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 task 未注册".to_string()),
    }
}

/// 任务详情。
#[derive(Clone, Debug)]
struct TaskDetail {
    id: u64,
    description: String,
    status: String,
    progress: u8,
    agent_id: Option<String>,
    created_ns: u64,
    completed_ns: Option<u64>,
    result: Option<String>,
    error: Option<String>,
    cpu_ns: u64,
    memory_bytes: u64,
    timeline: Vec<TimelineEvent>,
}

/// 时间线事件。
#[derive(Clone, Debug)]
struct TimelineEvent {
    timestamp: u64,
    message: String,
}

fn parse_task_list(output: &serde_json::Value) -> Vec<TaskSummary> {
    let mut tasks = Vec::new();
    if let Some(arr) = output.as_array() {
        for item in arr {
            tasks.push(TaskSummary {
                id: item.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                description: item.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                status: item.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                agent_id: item.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                progress: item.get("progress").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
            });
        }
    }
    tasks
}

fn parse_task_detail(output: &serde_json::Value) -> Option<TaskDetail> {
    Some(TaskDetail {
        id: output.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
        description: output.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        status: output.get("status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
        progress: output.get("progress").and_then(|v| v.as_u64()).unwrap_or(0) as u8,
        agent_id: output.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        created_ns: output.get("created_ns").and_then(|v| v.as_u64()).unwrap_or(0),
        completed_ns: output.get("completed_ns").and_then(|v| v.as_u64()),
        result: output.get("result").and_then(|v| v.as_str()).map(|s| s.to_string()),
        error: output.get("error").and_then(|v| v.as_str()).map(|s| s.to_string()),
        cpu_ns: output.get("cpu_ns").and_then(|v| v.as_u64()).unwrap_or(0),
        memory_bytes: output.get("memory_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        timeline: Vec::new(),
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_task_workspace_not_empty() {
        let out = render_task_workspace();
        assert!(out.contains("Task Workspace"));
    }

    #[test]
    fn truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hel...");
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hi", 10), "hi");
    }
}
