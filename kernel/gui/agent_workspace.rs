//! Agent Workspace —— 智能体管理和观察界面。
//!
//! 展示每个 Agent 的：名称、ID、角色、身份、模型、运行时状态、当前任务、
//! 可用工具、权限、资源占用、最近事件、执行历史。
//! 状态区分：Created / Initializing / Idle / Running / Waiting / Blocked / Paused /
//! Completed / Failed / Stopped。

use crate::permission::{
    AgentLifecycleState, AgentSummary, permission_manager,
};
use crate::capability::{CapabilityRequest, CapabilityResponse, ErrorCode, ExecutionContext,
    now_ns, AuditInfo, AuditResult, CallerType};
use serde_json::json;

/// 渲染 Agent Workspace 的文本表示（供终端和 GUI 共用）。
pub fn render_agent_workspace() -> String {
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();

    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                           Agent Workspace\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    if agents.is_empty() {
        out.push_str("  (暂无 Agent)\n");
        out.push_str("\n  创建 Agent: agent create <名称> <角色>\n");
        return out;
    }

    out.push_str(&format!("  活跃 Agent 总数: {}\n\n", agents.len()));
    out.push_str("  ID   名称       角色        状态          权限数    操作\n");
    out.push_str("  --   ----       ----        ------        ------    ------\n");

    for a in &agents {
        let state_str = match a.state {
            AgentLifecycleState::Created => "created",
            AgentLifecycleState::Initializing => "initializing",
            AgentLifecycleState::Idle => "idle",
            AgentLifecycleState::Running => "running",
            AgentLifecycleState::Waiting => "waiting",
            AgentLifecycleState::Blocked => "blocked",
            AgentLifecycleState::Paused => "paused",
            AgentLifecycleState::Completed => "completed",
            AgentLifecycleState::Failed => "failed",
            AgentLifecycleState::Stopped => "stopped",
        };
        out.push_str(&format!(
            "  {:<3} {:<11} {:<11} {:<13} {:<8}    [start|stop|inspect|logs]\n",
            a.agent_id, a.name, a.role, state_str, a.capabilities.len()
        ));
    }

    out.push_str("\n  状态图例:\n");
    out.push_str("    created      = 已创建，未初始化\n");
    out.push_str("    initializing = 正在加载模型/上下文/工具\n");
    out.push_str("    idle         = 空闲，等待任务\n");
    out.push_str("    running      = 正在执行任务\n");
    out.push_str("    waiting      = 等待外部事件（用户输入/权限批准/其他 Agent）\n");
    out.push_str("    blocked      = 被阻塞（权限缺失/资源不足/依赖未完成）\n");
    out.push_str("    paused       = 已暂停\n");
    out.push_str("    completed    = 任务成功完成\n");
    out.push_str("    failed       = 任务失败\n");
    out.push_str("    stopped      = 已停止\n");

    out
}

/// 渲染单个 Agent 的详细信息。
pub fn render_agent_detail(agent_id: u64) -> String {
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();
    let agent = agents.iter().find(|a| a.agent_id == agent_id);

    match agent {
        Some(a) => {
            let mut out = String::new();
            out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n"));
            out.push_str(&format!("                         Agent #{}\n", a.agent_id));
            out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n\n"));
            out.push_str(&format!("  名称: {}\n", a.name));
            out.push_str(&format!("  角色: {}\n", a.role));
            out.push_str(&format!("  状态: {:?}\n", a.state));
            out.push_str(&format!("  权限 ({}): {}\n", a.capabilities.len(), a.capabilities.join(", ")));
            out.push_str("\n  可用操作:\n");
            out.push_str(&format!("    agent start {}   - 启动 Agent\n", a.agent_id));
            out.push_str(&format!("    agent stop {}    - 停止 Agent\n", a.agent_id));
            out.push_str(&format!("    agent inspect {} - 查看详细信息\n", a.agent_id));
            out.push_str(&format!("    agent logs {}    - 查看执行日志\n", a.agent_id));
            out
        }
        None => format!("未找到 Agent #{}\n", agent_id),
    }
}

/// 通过能力层启动 Agent。
pub fn start_agent(agent_id: u64) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "agent".to_string(),
        input: json!({"op": "start", "id": agent_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["agent:control".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "agent.start".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("agent") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("Agent #{} 已启动", agent_id)),
            Err(e) => Err(format!("启动失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 agent 未注册".to_string()),
    }
}

/// 通过能力层停止 Agent。
pub fn stop_agent(agent_id: u64) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "agent".to_string(),
        input: json!({"op": "stop", "id": agent_id}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["agent:control".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "agent.stop".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.lock().unwrap().get("agent") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("Agent #{} 已停止", agent_id)),
            Err(e) => Err(format!("停止失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 agent 未注册".to_string()),
    }
}

/// 检查 Agent 是否存在。
pub fn agent_exists(agent_id: u64) -> bool {
    let pm = permission_manager().lock().unwrap();
    pm.get_state(agent_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_agent_workspace_not_empty() {
        let out = render_agent_workspace();
        assert!(out.contains("Agent Workspace"));
    }

    #[test]
    fn render_agent_detail_not_found() {
        let out = render_agent_detail(99999);
        assert!(out.contains("未找到 Agent"));
    }
}
