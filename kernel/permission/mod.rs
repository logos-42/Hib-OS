//! 权限模型 (Permission Model)
//!
//! Agent 的权限必须独立管理，采用最小权限原则。
//! 每个 Agent 有：Agent ID / Owner / Principal / 能力集 / 权限作用域 /
//! 执行上下文 / 资源配额 / 生命周期状态 / 审计轨迹。

use super::capability::{now_ns, AuditInfo, AuditResult, CapabilityError, ExecutionContext, Permission};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Agent ID。
pub type AgentId = u64;

/// 会话 ID。
pub type SessionId = u64;

/// 权限字符串，格式为 `resource:action:scope`。
pub type PermissionStr = String;

/// Agent 身份。
#[derive(Clone, Debug)]
pub struct AgentIdentity {
    pub agent_id: AgentId,
    pub owner: Principal,
    pub name: String,
    pub role: String,
    pub created_at: u64,
}

/// Agent 拥有者。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Principal {
    User { user_id: String },
    System,
    Agent { agent_id: AgentId },
}

/// Agent 生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLifecycleState {
    Created,
    Initializing,
    Idle,
    Running,
    Waiting,
    Blocked,
    Paused,
    Completed,
    Failed,
    Stopped,
}

impl AgentLifecycleState {
    pub fn name(self) -> &'static str {
        match self {
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
        }
    }
}

/// 授权模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationMode {
    /// 仅当次有效。
    Once,
    /// 当前会话有效。
    Session,
    /// 持久有效，直到撤销。
    Persistent,
}

/// 授权记录。
#[derive(Clone, Debug)]
pub struct Authorization {
    pub agent_id: AgentId,
    pub permission: PermissionStr,
    pub mode: AuthorizationMode,
    pub session_id: Option<SessionId>,
    pub granted_at: u64,
    pub expires_at: Option<u64>,
    pub granted_by: String,
}

/// Agent 注册表项。
pub struct AgentEntry {
    pub identity: AgentIdentity,
    pub state: AgentLifecycleState,
    pub capabilities: BTreeSet<PermissionStr>,
    pub authorizations: BTreeMap<PermissionStr, Authorization>,
    pub quota: super::capability::ResourceQuota,
    pub audit_log: Vec<AuditEntry>,
}

/// 审计条目。
#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub actor: String,
    pub action: String,
    pub permission: Option<PermissionStr>,
    pub result: AuditResult,
    pub correlation_id: Option<u64>,
}

/// 权限管理器。
pub struct PermissionManager {
    agents: Mutex<BTreeMap<AgentId, AgentEntry>>,
    next_id: Mutex<AgentId>,
    next_session: Mutex<SessionId>,
    global_audit: Mutex<Vec<AuditEntry>>,
}

impl PermissionManager {
    pub fn new() -> Self {
        Self {
            agents: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
            next_session: Mutex::new(1),
            global_audit: Mutex::new(Vec::new()),
        }
    }

    /// 创建 Agent。
    pub fn create_agent(
        &self,
        owner: Principal,
        name: &str,
        role: &str,
        initial_caps: Vec<PermissionStr>,
        quota: super::capability::ResourceQuota,
    ) -> AgentId {
        let id = {
            let mut next = self.next_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        let agent = AgentEntry {
            identity: AgentIdentity {
                agent_id: id,
                owner,
                name: name.to_string(),
                role: role.to_string(),
                created_at: now_ns(),
            },
            state: AgentLifecycleState::Created,
            capabilities: initial_caps.into_iter().collect(),
            authorizations: BTreeMap::new(),
            quota,
            audit_log: Vec::new(),
        };
        self.agents.lock().unwrap().insert(id, agent);
        id
    }

    /// 授予权限。
    pub fn grant_permission(
        &self,
        agent_id: AgentId,
        permission: &str,
        mode: AuthorizationMode,
        session_id: Option<SessionId>,
        granted_by: &str,
    ) -> Result<(), String> {
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .get_mut(&agent_id)
            .ok_or_else(|| format!("Agent {} 不存在", agent_id))?;

        let auth = Authorization {
            agent_id,
            permission: permission.to_string(),
            mode,
            session_id,
            granted_at: now_ns(),
            expires_at: match mode {
                AuthorizationMode::Once => Some(now_ns() + 60_000_000_000), // 60秒后过期
                AuthorizationMode::Session => None,
                AuthorizationMode::Persistent => None,
            },
            granted_by: granted_by.to_string(),
        };
        agent.authorizations.insert(permission.to_string(), auth);
        agent.capabilities.insert(permission.to_string());
        agent.audit_log.push(AuditEntry {
            timestamp: now_ns(),
            actor: granted_by.to_string(),
            action: "grant_permission".to_string(),
            permission: Some(permission.to_string()),
            result: AuditResult::Granted,
            correlation_id: None,
        });
        self.record_global_audit(granted_by, "grant_permission", permission, AuditResult::Granted, None);
        Ok(())
    }

    /// 撤销权限。
    pub fn revoke_permission(&self, agent_id: AgentId, permission: &str, revoked_by: &str) -> Result<(), String> {
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .get_mut(&agent_id)
            .ok_or_else(|| format!("Agent {} 不存在", agent_id))?;
        agent.authorizations.remove(permission);
        agent.capabilities.remove(permission);
        agent.audit_log.push(AuditEntry {
            timestamp: now_ns(),
            actor: revoked_by.to_string(),
            action: "revoke_permission".to_string(),
            permission: Some(permission.to_string()),
            result: AuditResult::Revoked,
            correlation_id: None,
        });
        self.record_global_audit(revoked_by, "revoke_permission", permission, AuditResult::Revoked, None);
        Ok(())
    }

    /// 检查 Agent 是否持有指定权限。
    pub fn check_permission(&self, agent_id: AgentId, permission: &str) -> bool {
        let agents = self.agents.lock().unwrap();
        let agent = match agents.get(&agent_id) {
            Some(a) => a,
            None => return false,
        };
        // 直接能力匹配
        if agent.capabilities.contains(permission) {
            return true;
        }
        // 前缀通配符匹配：fs:read:* 匹配 fs:read:/foo
        if agent.capabilities.iter().any(|p| {
            p.ends_with(":*") && permission.starts_with(&p[..p.len() - 1])
        }) {
            return true;
        }
        false
    }

    /// 设置 Agent 状态。
    pub fn set_state(&self, agent_id: AgentId, state: AgentLifecycleState) -> Result<(), String> {
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .get_mut(&agent_id)
            .ok_or_else(|| format!("Agent {} 不存在", agent_id))?;
        agent.state = state;
        Ok(())
    }

    /// 获取 Agent 状态。
    pub fn get_state(&self, agent_id: AgentId) -> Option<AgentLifecycleState> {
        self.agents.lock().unwrap().get(&agent_id).map(|a| a.state)
    }

    /// 列出所有 Agent。
    pub fn list_agents(&self) -> Vec<AgentSummary> {
        let agents = self.agents.lock().unwrap();
        agents
            .values()
            .map(|a| AgentSummary {
                agent_id: a.identity.agent_id,
                name: a.identity.name.clone(),
                role: a.identity.role.clone(),
                state: a.state,
                capabilities: a.capabilities.iter().cloned().collect(),
            })
            .collect()
    }

    /// 记录审计日志。
    pub fn record_audit(
        &self,
        actor: &str,
        action: &str,
        permission: Option<&str>,
        result: AuditResult,
        correlation_id: Option<u64>,
    ) {
        self.global_audit.lock().unwrap().push(AuditEntry {
            timestamp: now_ns(),
            actor: actor.to_string(),
            action: action.to_string(),
            permission: permission.map(|s| s.to_string()),
            result,
            correlation_id,
        });
        // 限制全局审计日志大小
        {
            let len = {
                let ga = self.global_audit.lock().unwrap();
                ga.len()
            };
            if len > 10000 {
                let mut ga = self.global_audit.lock().unwrap();
                ga.drain(0..len - 10000);
            }
        }
    }

    fn record_global_audit(
        &self,
        actor: &str,
        action: &str,
        permission: &str,
        result: AuditResult,
        correlation_id: Option<u64>,
    ) {
        self.record_audit(actor, action, Some(permission), result, correlation_id);
    }

    /// 查询审计日志。
    pub fn query_audit(&self, limit: Option<usize>) -> Vec<AuditEntry> {
        let ga = self.global_audit.lock().unwrap();
        let start = ga.len().saturating_sub(limit.unwrap_or(ga.len()));
        ga.iter().skip(start).cloned().collect()
    }

    /// 创建会话 ID。
    pub fn create_session(&self) -> SessionId {
        let mut next = self.next_session.lock().unwrap();
        let id = *next;
        *next += 1;
        id
    }

    /// 获取 Agent 配额。
    pub fn get_quota(&self, agent_id: AgentId) -> Option<super::capability::ResourceQuota> {
        self.agents.lock().unwrap().get(&agent_id).map(|a| a.quota.clone())
    }

    /// 设置 Agent 配额。
    pub fn set_quota(&self, agent_id: AgentId, quota: super::capability::ResourceQuota) -> Result<(), String> {
        let mut agents = self.agents.lock().unwrap();
        let agent = agents
            .get_mut(&agent_id)
            .ok_or_else(|| format!("Agent {} 不存在", agent_id))?;
        agent.quota = quota;
        Ok(())
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent 摘要（供 list 接口使用）。
#[derive(Clone, Debug)]
pub struct AgentSummary {
    pub agent_id: AgentId,
    pub name: String,
    pub role: String,
    pub state: AgentLifecycleState,
    pub capabilities: Vec<PermissionStr>,
}

/// 全局权限管理器单例。
static PERM_MANAGER: std::sync::OnceLock<Mutex<PermissionManager>> =
    std::sync::OnceLock::new();

/// 获取全局权限管理器。
pub fn permission_manager() -> &'static Mutex<PermissionManager> {
    PERM_MANAGER.get_or_init(|| Mutex::new(PermissionManager::new()))
}

/// 权限检查器：用于在能力执行前检查调用者权限。
pub struct PermissionChecker;

impl PermissionChecker {
    /// 检查执行上下文中的调用者是否持有所需权限。
    /// 调用者类型为 Agent 时，查询 PermissionManager；Shell/Gui 使用上下文中的权限列表。
    pub fn check(ctx: &ExecutionContext, needed: &str) -> Result<(), CapabilityError> {
        match ctx.caller_type {
            super::capability::CallerType::Agent => {
                let agent_id: AgentId = match ctx.caller_id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        return Err(CapabilityError {
                            code: super::capability::ErrorCode::NotAuthorized,
                            message: format!("无效的 Agent ID: {}", ctx.caller_id),
                            source: Some("permission".to_string()),
                            retryable: false,
                            audit: AuditInfo {
                                actor: ctx.caller_id.clone(),
                                action: needed.to_string(),
                                timestamp: now_ns(),
                                result: AuditResult::Denied,
                                correlation_id: None,
                            },
                        });
                    }
                };
                let pm = permission_manager().lock().unwrap();
                if pm.check_permission(agent_id, needed) {
                    pm.record_audit(
                        &ctx.caller_id,
                        "check_permission",
                        Some(needed),
                        AuditResult::Granted,
                        None,
                    );
                    Ok(())
                } else {
                    pm.record_audit(
                        &ctx.caller_id,
                        "check_permission",
                        Some(needed),
                        AuditResult::Denied,
                        None,
                    );
                    Err(CapabilityError {
                        code: super::capability::ErrorCode::PermissionDenied,
                        message: format!("Agent {} 缺少权限: {}", agent_id, needed),
                        source: Some("permission".to_string()),
                        retryable: false,
                        audit: AuditInfo {
                            actor: ctx.caller_id.clone(),
                            action: needed.to_string(),
                            timestamp: now_ns(),
                            result: AuditResult::Denied,
                            correlation_id: None,
                        },
                    })
                }
            }
            super::capability::CallerType::Shell
            | super::capability::CallerType::Gui
            | super::capability::CallerType::System => {
                if ctx.permissions.iter().any(|p| p == needed || p == "*:*:*") {
                    Ok(())
                } else if ctx.permissions.iter().any(|p| {
                    p.ends_with(":*") && needed.starts_with(&p[..p.len() - 1])
                }) {
                    Ok(())
                } else {
                    Err(CapabilityError {
                        code: super::capability::ErrorCode::PermissionDenied,
                        message: format!("缺少权限: {}", needed),
                        source: Some("permission".to_string()),
                        retryable: false,
                        audit: AuditInfo {
                            actor: ctx.caller_id.clone(),
                            action: needed.to_string(),
                            timestamp: now_ns(),
                            result: AuditResult::Denied,
                            correlation_id: None,
                        },
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::capability::{CallerType, ExecutionContext, ResourceQuota};

    #[test]
    fn create_and_check_agent() {
        let pm = PermissionManager::new();
        let id = pm.create_agent(
            Principal::User { user_id: "alice".to_string() },
            "test-agent",
            "worker",
            vec!["fs:read:/home/*".to_string()],
            ResourceQuota::default(),
        );
        assert!(pm.check_permission(id, "fs:read:/home/foo"));
        assert!(!pm.check_permission(id, "fs:delete:/home/foo"));
        assert_eq!(pm.get_state(id), Some(AgentLifecycleState::Created));
    }

    #[test]
    fn grant_and_revoke() {
        let pm = PermissionManager::new();
        let id = pm.create_agent(
            Principal::User { user_id: "alice".to_string() },
            "test-agent",
            "worker",
            vec![],
            ResourceQuota::default(),
        );
        pm.grant_permission(id, "fs:delete:/tmp/*", AuthorizationMode::Once, None, "alice").unwrap();
        assert!(pm.check_permission(id, "fs:delete:/tmp/foo"));
        pm.revoke_permission(id, "fs:delete:/tmp/*", "alice").unwrap();
        assert!(!pm.check_permission(id, "fs:delete:/tmp/foo"));
    }

    #[test]
    fn wildcard_permission_match() {
        let pm = PermissionManager::new();
        let id = pm.create_agent(
            Principal::System,
            "sys-agent",
            "system",
            vec!["fs:*:*".to_string()],
            ResourceQuota::default(),
        );
        assert!(pm.check_permission(id, "fs:read:/anything"));
        assert!(pm.check_permission(id, "fs:delete:/anything"));
    }

    #[test]
    fn permission_checker_shell() {
        let ctx = ExecutionContext {
            caller_id: "shell-1".to_string(),
            caller_type: CallerType::Shell,
            permissions: vec!["fs:read".to_string()],
            resource_quota: ResourceQuota::default(),
        };
        assert!(PermissionChecker::check(&ctx, "fs:read").is_ok());
        assert!(PermissionChecker::check(&ctx, "fs:delete").is_err());
    }

    #[test]
    fn list_agents() {
        let pm = PermissionManager::new();
        let id1 = pm.create_agent(
            Principal::User { user_id: "alice".to_string() },
            "a1",
            "worker",
            vec!["fs:read".to_string()],
            ResourceQuota::default(),
        );
        let id2 = pm.create_agent(
            Principal::User { user_id: "bob".to_string() },
            "a2",
            "worker",
            vec!["fs:write".to_string()],
            ResourceQuota::default(),
        );
        let list = pm.list_agents();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|a| a.agent_id == id1 && a.name == "a1"));
        assert!(list.iter().any(|a| a.agent_id == id2 && a.name == "a2"));
    }
}
