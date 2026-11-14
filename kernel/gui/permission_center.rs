//! Permission Center —— 统一权限中心界面。
//!
//! 展示：Agent 身份、已授予能力、待处理请求、权限作用域、过期时间、
//! 执行历史、风险等级。
//! 授权模式：Once / Session / Persistent。

use crate::permission::{
    AgentLifecycleState, AgentSummary, Authorization, AuthorizationMode, Principal,
    permission_manager,
};
use crate::capability::{CapabilityRequest, ExecutionContext, now_ns, AuditInfo, AuditResult,
    CallerType};

/// 渲染 Permission Center 的文本表示。
pub fn render_permission_center() -> String {
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();

    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                         Permission Center (统一权限中心)\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    if agents.is_empty() {
        out.push_str("  (暂无 Agent)\n");
        return out;
    }

    out.push_str(&format!("  Agent 总数: {}\n\n", agents.len()));
    out.push_str("  ID   名称       状态          权限数    风险等级\n");
    out.push_str("  --   ----       ------        ------    ----------\n");

    for a in &agents {
        let risk = risk_level(&a.capabilities);
        out.push_str(&format!(
            "  {:<3} {:<11} {:<13} {:<8}    {}\n",
            a.agent_id, a.name, format!("{:?}", a.state), a.capabilities.len(), risk
        ));
    }

    out.push_str("\n  风险等级:\n");
    out.push_str("    低    = 仅读取权限，无删除/网络/安装权限\n");
    out.push_str("    中    = 有写入权限，无高风险操作权限\n");
    out.push_str("    高    = 有删除/外部网络/安装/系统配置修改权限\n");
    out.push_str("\n  授权模式:\n");
    out.push_str("    Once    = 仅当次操作有效（高风险一次性操作）\n");
    out.push_str("    Session = 当前会话有效\n");
    out.push_str("    Persistent = 持久有效，直到撤销\n");
    out.push_str("\n  可用操作:\n");
    out.push_str("    agent permissions <id>   - 查看 Agent 权限详情\n");
    out.push_str("    capability permissions <id> - 查看能力权限要求\n");

    out
}

/// 渲染单个 Agent 的权限详情。
pub fn render_agent_permissions(agent_id: u64) -> String {
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();
    let agent = agents.iter().find(|a| a.agent_id == agent_id);

    match agent {
        Some(a) => {
            let mut out = String::new();
            out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n"));
            out.push_str(&format!("                    Agent #{} 权限详情\n", a.agent_id));
            out.push_str(&format!("══════════════════════════════════════════════════════════════════════════════\n\n"));
            out.push_str(&format!("  名称: {}\n", a.name));
            out.push_str(&format!("  角色: {}\n", a.role));
            out.push_str(&format!("  状态: {:?}\n", a.state));
            out.push_str(&format!("  已授予权限 ({}):\n", a.capabilities.len()));
            for cap in &a.capabilities {
                out.push_str(&format!("    - {}\n", cap));
            }
            if a.capabilities.is_empty() {
                out.push_str("    (无)\n");
            }
            out.push_str(&format!("\n  风险等级: {}\n", risk_level(&a.capabilities)));
            out.push_str("\n  可用操作:\n");
            out.push_str(&format!("    agent start {}   - 启动\n", a.agent_id));
            out.push_str(&format!("    agent stop {}    - 停止\n", a.agent_id));
            out
        }
        None => format!("未找到 Agent #{}\n", agent_id),
    }
}

/// 计算风险等级。
fn risk_level(caps: &[String]) -> &'static str {
    let high_risk = [
        "fs:delete",
        "net:connect:external",
        "app:install",
        "sys:settings:write",
        "agent:delegate",
    ];
    let medium_risk = [
        "fs:write",
        "proc:exec",
        "win:control",
    ];

    let has_high = caps.iter().any(|c| high_risk.iter().any(|hr| c.starts_with(hr)));
    let has_medium = caps.iter().any(|c| medium_risk.iter().any(|mr| c.starts_with(mr)));

    if has_high {
        "高"
    } else if has_medium {
        "中"
    } else {
        "低"
    }
}

/// 授予权限（通过能力层 / PermissionManager）。
pub fn grant_permission(agent_id: u64, permission: &str, mode: &str, granted_by: &str) -> Result<String, String> {
    let auth_mode = match mode {
        "once" => AuthorizationMode::Once,
        "session" => AuthorizationMode::Session,
        "persistent" => AuthorizationMode::Persistent,
        _ => return Err("无效的授权模式 (once/session/persistent)".to_string()),
    };

    let pm = permission_manager().lock().unwrap();
    let result = pm.grant_permission(
        agent_id,
        permission,
        auth_mode,
        None,
        granted_by,
    );
    drop(pm);

    match result {
        Ok(()) => Ok(format!("已授予 Agent #{} 权限: {} (模式: {})", agent_id, permission, mode)),
        Err(e) => Err(format!("授权失败: {}", e)),
    }
}

/// 撤销权限。
pub fn revoke_permission(agent_id: u64, permission: &str, revoked_by: &str) -> Result<String, String> {
    let pm = permission_manager().lock().unwrap();
    let result = pm.revoke_permission(agent_id, permission, revoked_by);
    drop(pm);

    match result {
        Ok(()) => Ok(format!("已撤销 Agent #{} 权限: {}", agent_id, permission)),
        Err(e) => Err(format!("撤销失败: {}", e)),
    }
}

/// 查询审计日志。
pub fn render_audit_log(limit: Option<usize>) -> String {
    let pm = permission_manager().lock().unwrap();
    let entries = pm.query_audit(limit);
    drop(pm);

    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                         审计日志 (Audit Trail)\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    if entries.is_empty() {
        out.push_str("  (暂无审计记录)\n");
        return out;
    }

    out.push_str("  时间戳          操作者          动作              权限                结果\n");
    out.push_str("  --------------  --------------  ----------------- --------------------  ------\n");

    for e in entries.iter().rev().take(limit.unwrap_or(20)) {
        let result_str = match e.result {
            AuditResult::Granted => "GRANTED",
            AuditResult::Denied => "DENIED",
            AuditResult::Revoked => "REVOKED",
            AuditResult::Executed => "EXECUTED",
            AuditResult::Failed => "FAILED",
            AuditResult::Cancelled => "CANCELLED",
        };
        let perm = e.permission.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "  {:<16} {:<16} {:<17} {:<20} {}\n",
            e.timestamp, e.actor, e.action, perm, result_str
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_permission_center_not_empty() {
        let out = render_permission_center();
        assert!(out.contains("Permission Center"));
    }

    #[test]
    fn risk_level_low() {
        assert_eq!(risk_level(&["fs:read:/home/*".to_string()]), "低");
    }

    #[test]
    fn risk_level_high() {
        assert_eq!(risk_level(&["fs:delete:/tmp/*".to_string()]), "高");
    }

    #[test]
    fn risk_level_medium() {
        assert_eq!(risk_level(&["fs:write:/tmp/*".to_string()]), "中");
    }

    #[test]
    fn render_agent_permissions_not_found() {
        let out = render_agent_permissions(99999);
        assert!(out.contains("未找到 Agent"));
    }
}
