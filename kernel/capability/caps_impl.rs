//! 统一系统能力层 —— 具体能力实现。
//!
//! 每项能力封装现有内核子系统，通过 `SystemCapability` trait 暴露统一接口。
//! Shell、GUI、Agent 都通过 `CapabilityRegistry` 调用这些能力，保证三种入口行为一致。

use super::{
    AuditInfo, AuditResult, CallerType, CapabilityError, CapabilityMetadata,
    CapabilityRequest, CapabilityResponse, ErrorCode, ExecutionContext, now_ns,
    Permission, ResourceQuota, SystemCapability, capability_registry,
};
use crate::event::{publish_from_capability, SystemEventType};
use crate::monitor;
use crate::process::{process_manager, spawn_process, ProcessId, ProcessManager, ProcessState};
use crate::vfs::{global_vfs, NodeKind, Vfs, VfsError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

// ============================================================================
// 任务管理器（内存，供 task.* 能力使用）
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskState {
    Created,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct TaskEntry {
    pub id: u64,
    pub description: String,
    pub state: TaskState,
    pub agent_id: Option<String>,
    pub created_ns: u64,
    pub completed_ns: Option<u64>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub progress: u8,
    pub cpu_ns: u64,
    pub memory_bytes: u64,
    pub timeline: Vec<TimelineEvent>,
}

/// 任务时间线事件。
#[derive(Clone, Debug)]
pub struct TimelineEvent {
    pub timestamp: u64,
    pub message: String,
}

struct TaskManager {
    tasks: BTreeMap<u64, TaskEntry>,
    next_id: u64,
}

impl TaskManager {
    fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_id: 1,
        }
    }

    fn create(&mut self, description: String, agent_id: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::monitor::get_time_ns();
        self.tasks.insert(
            id,
            TaskEntry {
                id,
                description,
                state: TaskState::Created,
                agent_id,
                created_ns: now,
                completed_ns: None,
                result: None,
                error: None,
                progress: 0,
                cpu_ns: 0,
                memory_bytes: 0,
                timeline: vec![TimelineEvent {
                    timestamp: now,
                    message: "任务已创建".to_string(),
                }],
            },
        );
        id
    }

    fn get(&self, id: u64) -> Option<TaskEntry> {
        self.tasks.get(&id).cloned()
    }

    fn cancel(&mut self, id: u64) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) if t.state == TaskState::Created || t.state == TaskState::Running => {
                t.state = TaskState::Cancelled;
                true
            }
            _ => false,
        }
    }

    fn pause(&mut self, id: u64) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) if t.state == TaskState::Running => {
                t.state = TaskState::Paused;
                true
            }
            _ => false,
        }
    }

    fn resume(&mut self, id: u64) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) if t.state == TaskState::Paused => {
                t.state = TaskState::Running;
                true
            }
            _ => false,
        }
    }

    fn list(&self) -> Vec<TaskEntry> {
        self.tasks.values().cloned().collect()
    }

    /// 更新任务进度和资源消耗（用于 Agent 执行过程中的实时上报）。
    fn update_progress(
        &mut self,
        id: u64,
        progress: u8,
        cpu_ns: u64,
        memory_bytes: u64,
        message: &str,
    ) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) => {
                t.progress = progress.min(100);
                t.cpu_ns = cpu_ns;
                t.memory_bytes = memory_bytes;
                t.timeline.push(TimelineEvent {
                    timestamp: crate::monitor::get_time_ns(),
                    message: message.to_string(),
                });
                true
            }
            None => false,
        }
    }

    /// 标记任务完成。
    fn complete(&mut self, id: u64, result: String) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) => {
                t.state = TaskState::Completed;
                t.progress = 100;
                t.completed_ns = Some(crate::monitor::get_time_ns());
                t.result = Some(result);
                t.timeline.push(TimelineEvent {
                    timestamp: crate::monitor::get_time_ns(),
                    message: "任务已完成".to_string(),
                });
                true
            }
            None => false,
        }
    }

    /// 标记任务失败。
    fn fail(&mut self, id: u64, error: String) -> bool {
        match self.tasks.get_mut(&id) {
            Some(t) => {
                t.state = TaskState::Failed;
                t.completed_ns = Some(crate::monitor::get_time_ns());
                t.error = Some(error.clone());
                t.timeline.push(TimelineEvent {
                    timestamp: crate::monitor::get_time_ns(),
                    message: format!("任务失败: {error}"),
                });
                true
            }
            None => false,
        }
    }
}

static TASK_MGR: std::sync::OnceLock<std::sync::Mutex<TaskManager>> =
    std::sync::OnceLock::new();

fn task_mgr() -> &'static std::sync::Mutex<TaskManager> {
    TASK_MGR.get_or_init(|| std::sync::Mutex::new(TaskManager::new()))
}

// ============================================================================
// 设置存储（内存，供 settings.* 能力使用）
// ============================================================================

static SETTINGS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, Value>>> =
    std::sync::OnceLock::new();

fn settings_store() -> &'static std::sync::Mutex<BTreeMap<String, Value>> {
    SETTINGS.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

// ============================================================================
// 辅助：构建能力响应
// ============================================================================

fn ok_response(req: &CapabilityRequest, output: Value, action: &str) -> CapabilityResponse {
    CapabilityResponse {
        request_id: 0,
        output,
        duration_ms: 0,
        audit: build_trusted_audit(&req.context, action, AuditResult::Executed),
    }
}

/// 服务器端构建的可信执行上下文（不可被调用方伪造权限）。
pub struct TrustedContext {
    pub caller_id: String,
    pub caller_type: CallerType,
    /// 服务端核验后的真实权限（忽略调用方传入的 permissions）。
    pub verified_permissions: Vec<String>,
}

/// 从调用方提供的 ExecutionContext 构建服务器端可信上下文。
fn build_trusted_context(ctx: &ExecutionContext) -> TrustedContext {
    match ctx.caller_type {
        CallerType::Agent => {
            let agent_id: u64 = ctx.caller_id.parse().unwrap_or(0);
            let verified_permissions = if agent_id != 0 {
                let pm = crate::permission::permission_manager().lock().unwrap();
                let mut perms = Vec::new();
                for summary in pm.list_agents() {
                    if summary.agent_id == agent_id {
                        perms = summary.capabilities.clone();
                        break;
                    }
                }
                perms
            } else {
                Vec::new()
            };
            TrustedContext {
                caller_id: ctx.caller_id.clone(),
                caller_type: CallerType::Agent,
                verified_permissions,
            }
        }
        CallerType::Shell | CallerType::Gui | CallerType::System => {
            // Shell/Gui/System：忽略调用方传入的 permissions，拒绝通配符
            let verified_permissions: Vec<String> = ctx.permissions.iter()
                .filter(|p| !p.contains('*') && !p.is_empty())
                .cloned()
                .collect();
            TrustedContext {
                caller_id: ctx.caller_id.clone(),
                caller_type: ctx.caller_type,
                verified_permissions,
            }
        }
    }
}

/// 构建服务器端可信的审计信息（忽略调用方传入的 actor/action/result/timestamp）。
fn build_trusted_audit(ctx: &ExecutionContext, action: &str, result: AuditResult) -> AuditInfo {
    AuditInfo {
        actor: ctx.caller_id.clone(),
        action: action.to_string(),
        timestamp: now_ns(),
        result,
        correlation_id: None,
    }
}

/// 在执行能力前强制检查所有所需权限（使用可信上下文，不信任 caller.permissions）。
fn enforce_permissions(ctx: &ExecutionContext, required: &[Permission]) -> Result<(), CapabilityError> {
    let trusted = build_trusted_context(ctx);
    for perm in required {
        if !trusted.verified_permissions.iter().any(|p| p == perm || p == "*") {
            return Err(CapabilityError {
                code: ErrorCode::PermissionDenied,
                message: format!("调用方 {} 缺少权限: {}", trusted.caller_id, perm),
                source: Some("permission".to_string()),
                retryable: false,
                audit: build_trusted_audit(ctx, perm, AuditResult::Denied),
            });
        }
    }
    Ok(())
}

/// 检查取消令牌；若已取消则返回 `CapabilityError::Cancelled`。
fn check_cancellation(token: &Option<crate::capability::CancellationToken>) -> Result<(), CapabilityError> {
    if let Some(t) = token {
        if t.is_cancelled() {
            return Err(CapabilityError {
                code: ErrorCode::Cancelled,
                message: "操作已被取消".to_string(),
                source: Some("capability".to_string()),
                retryable: false,
                audit: AuditInfo {
                    actor: "system".to_string(),
                    action: "cancel".to_string(),
                    timestamp: now_ns(),
                    result: AuditResult::Cancelled,
                    correlation_id: None,
                },
            });
        }
    }
    Ok(())
}

/// 检查超时；若已超时则返回 `CapabilityError::Timeout`。
fn check_timeout(start_ns: u64, timeout_ns: Option<u64>) -> Result<(), CapabilityError> {
    if let Some(t) = timeout_ns {
        let elapsed = now_ns().saturating_sub(start_ns);
        if elapsed > t {
            return Err(CapabilityError {
                code: ErrorCode::Timeout,
                message: format!("操作超时 ({}ns > {}ns)", elapsed, t),
                source: Some("capability".to_string()),
                retryable: true,
                audit: AuditInfo {
                    actor: "system".to_string(),
                    action: "timeout".to_string(),
                    timestamp: now_ns(),
                    result: AuditResult::Failed,
                    correlation_id: None,
                },
            });
        }
    }
    Ok(())
}

// ============================================================================
// Filesystem Capability
// ============================================================================

pub struct FilesystemCapability;

impl FilesystemCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

/// 一次文件系统操作持有的锁守卫。
///
/// `ls` / `cat` / `stat` 这类只读操作拿共享读锁，多个入口可以同时查；
/// `write` / `mkdir` / `touch` / `rm` 这类写操作才拿独占写锁。
/// 这样既保留原有「一个 guard 覆盖整段 match」的写法，又真正获得读并发。
enum VfsGuard<'a> {
    Read(std::sync::RwLockReadGuard<'a, Vfs>),
    Write(std::sync::RwLockWriteGuard<'a, Vfs>),
}

impl std::ops::Deref for VfsGuard<'_> {
    type Target = Vfs;

    fn deref(&self) -> &Vfs {
        match self {
            VfsGuard::Read(g) => g,
            VfsGuard::Write(g) => g,
        }
    }
}

impl std::ops::DerefMut for VfsGuard<'_> {
    fn deref_mut(&mut self) -> &mut Vfs {
        match self {
            // 只有写操作会调用需要 &mut Vfs 的方法；读操作走到这里是编程错误。
            VfsGuard::Write(g) => g,
            VfsGuard::Read(_) => unreachable!("只读文件系统操作不应请求可变引用"),
        }
    }
}

impl SystemCapability for FilesystemCapability {
    fn id(&self) -> String {
        "filesystem".to_string()
    }

    fn description(&self) -> &str {
        "文件系统操作：列表/读取/写入/创建/删除/状态"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["list", "read", "write", "mkdir", "touch", "rm", "delete", "stat"]},
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["op", "path"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["fs:read".to_string(), "fs:write".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let path = req.input.get("path").and_then(|v| v.as_str()).unwrap_or("");

        // 写操作独占，读操作共享：这是 PERF-002 的核心。
        let write_op = matches!(op, "write" | "mkdir" | "touch" | "rm" | "delete");
        let mut vfs = if write_op {
            VfsGuard::Write(
                global_vfs()
                    .write()
                    .unwrap_or_else(|e| e.into_inner()),
            )
        } else {
            VfsGuard::Read(
                global_vfs()
                    .read()
                    .unwrap_or_else(|e| e.into_inner()),
            )
        };

        let result = match op {
            "list" => {
                let entries = vfs
                    .ls(if path.is_empty() { None } else { Some(path) })
                    .map_err(|e| CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("ls {path}: {e:?}"),
                        source: Some("filesystem".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                json!({
                    "path": path,
                    "entries": entries
                        .iter()
                        .map(|(name, kind, mode, size)| {
                            json!({
                                "name": name,
                                "kind": match kind {
                                    NodeKind::Dir => "dir",
                                    NodeKind::File => "file",
                                    NodeKind::Device => "device",
                                },
                                "mode": format!("{:o}", mode),
                                "size": size,
                            })
                        })
                        .collect::<Vec<_>>()
                })
            }
            "read" => {
                let content = vfs.cat(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("cat {path}: {e:?}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                json!({"path": path, "content": content})
            }
            "write" => {
                let content = req
                    .input
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                vfs.write(path, content).map_err(|e| CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("write {path}: {e:?}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                publish_from_capability(
                    &req.context,
                    SystemEventType::FileChanged,
                    json!({"path": path, "op": "write", "bytes": content.len()}),
                );
                json!({"path": path, "bytes_written": content.len()})
            }
            "mkdir" => {
                vfs.mkdir(path).map_err(|e| CapabilityError {
                    code: ErrorCode::AlreadyExists,
                    message: format!("mkdir {path}: {e:?}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                publish_from_capability(
                    &req.context,
                    SystemEventType::FileCreated,
                    json!({"path": path, "op": "mkdir"}),
                );
                json!({"path": path, "created": true})
            }
            "touch" => {
                vfs.touch(path).map_err(|e| CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("touch {path}: {e:?}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                publish_from_capability(
                    &req.context,
                    SystemEventType::FileCreated,
                    json!({"path": path, "op": "touch"}),
                );
                json!({"path": path, "created": true})
            }
            // `delete` 是 `rm` 的别名：Shell、GUI、Agent API 用同一个能力，
            // 避免「同一删除操作三个入口三种叫法」导致的跨接口不一致。
            "rm" | "delete" => {
                vfs.rm(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("rm {path}: {e:?}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                publish_from_capability(
                    &req.context,
                    SystemEventType::FileDeleted,
                    json!({"path": path, "op": "rm"}),
                );
                json!({"path": path, "deleted": true})
            }
            "stat" => {
                // 无法直接访问私有 node_at，改用公共 API 判断：
                // 先尝试 cat（文件），失败且为 IsADirectory 时尝试 ls（目录）。
                let stat_result = match vfs.cat(path) {
                    Ok(content) => {
                        json!({
                            "path": path,
                            "kind": "file",
                            "mode": "0644",
                            "size": content.len(),
                        })
                    }
                    Err(VfsError::IsADirectory) => {
                        let _entries = vfs.ls(Some(path)).map_err(|e| CapabilityError {
                            code: ErrorCode::NotFound,
                            message: format!("stat {path}: {e:?}"),
                            source: Some("filesystem".to_string()),
                            retryable: false,
                            audit: req.audit.clone(),
                        })?;
                        json!({
                            "path": path,
                            "kind": "dir",
                            "mode": "0755",
                            "size": 0,
                        })
                    }
                    Err(e) => {
                        return Err(CapabilityError {
                            code: ErrorCode::NotFound,
                            message: format!("stat {path}: {e:?}"),
                            source: Some("filesystem".to_string()),
                            retryable: false,
                            audit: req.audit.clone(),
                        })
                    }
                };
                stat_result
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("filesystem.{}", op)))
    }
}

// ============================================================================
// Process Capability
// ============================================================================

pub struct ProcessCapability;

impl ProcessCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

impl SystemCapability for ProcessCapability {
    fn id(&self) -> String {
        "process".to_string()
    }

    fn description(&self) -> &str {
        "进程操作：列表/查看/启动/停止"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["list", "inspect", "start", "stop"]},
                "pid": {"type": "integer"},
                "name": {"type": "string"},
                "priority": {"type": "integer"},
                "domain": {"type": "string"}
            },
            "required": ["op"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["proc:read".to_string(), "proc:exec".to_string(), "proc:control".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");

        let result = match op {
            "list" => {
                let processes = process_manager().lock().unwrap();
                let entries: Vec<Value> = processes
                    .processes
                    .iter()
                    .map(|(pid, pcb)| {
                        json!({
                            "pid": pid.0,
                            "name": pcb.name,
                            "state": match pcb.state {
                                ProcessState::Created => "created",
                                ProcessState::Ready => "ready",
                                ProcessState::Running => "running",
                                ProcessState::Blocked => "blocked",
                                ProcessState::Zombie => "zombie",
                                ProcessState::Terminated => "terminated",
                            },
                            "priority": pcb.priority,
                            "domain": pcb.domain,
                        })
                    })
                    .collect();
                json!({"processes": entries})
            }
            "inspect" => {
                let pid_val = req
                    .input
                    .get("pid")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 pid 参数".to_string(),
                        source: Some("process".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let pid = ProcessId(pid_val);
                // ProcessManager::get 是公共 API
                let pcb = ProcessManager::get(pid).ok_or_else(|| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("进程 {pid_val} 不存在"),
                    source: Some("process".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                json!({
                    "pid": pcb.pid.0,
                    "name": pcb.name,
                    "state": match pcb.state {
                        ProcessState::Created => "created",
                        ProcessState::Ready => "ready",
                        ProcessState::Running => "running",
                        ProcessState::Blocked => "blocked",
                        ProcessState::Zombie => "zombie",
                        ProcessState::Terminated => "terminated",
                    },
                    "priority": pcb.priority,
                    "domain": pcb.domain,
                    "parent": pcb.parent.map(|p| p.0),
                    "children": pcb.children.iter().map(|c| c.0).collect::<Vec<_>>(),
                    "tokens_used": pcb.token_used,
                })
            }
            "start" => {
                let name = req
                    .input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("process");
                let priority = req.input.get("priority").and_then(|v| v.as_u64()).unwrap_or(10) as u8;
                let domain = req.input.get("domain").and_then(|v| v.as_str()).unwrap_or("governance");
                let pid = spawn_process(name, priority, domain);
                publish_from_capability(
                    &req.context,
                    SystemEventType::ProcessStarted,
                    json!({"pid": pid.0, "name": name, "domain": domain}),
                );
                json!({"pid": pid.0, "created": true})
            }
            "stop" => {
                let pid_val = req
                    .input
                    .get("pid")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 pid 参数".to_string(),
                        source: Some("process".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let pid = ProcessId(pid_val);
                let ok = ProcessManager::exit(pid);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("进程 {pid_val} 不存在"),
                        source: Some("process".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                publish_from_capability(
                    &req.context,
                    SystemEventType::ProcessExited,
                    json!({"pid": pid_val}),
                );
                json!({"pid": pid_val, "stopped": true})
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("process".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("process.{}", op)))
    }
}

// ============================================================================
// Window Capability (仅在 gui feature 下可用)
// ============================================================================

#[cfg(feature = "gui")]
pub struct WindowCapability;

#[cfg(feature = "gui")]
impl WindowCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

#[cfg(feature = "gui")]
impl SystemCapability for WindowCapability {
    fn id(&self) -> String {
        "window".to_string()
    }

    fn description(&self) -> &str {
        "窗口操作：列表/查看/聚焦/移动/缩放/关闭"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["list", "inspect", "focus", "move", "resize", "close"]},
                "id": {"type": "integer"},
                "x": {"type": "integer"},
                "y": {"type": "integer"},
                "w": {"type": "integer"},
                "h": {"type": "integer"}
            },
            "required": ["op"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["win:read".to_string(), "win:control".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let mut desktop = crate::gui::desktop().lock().unwrap();

        let result = match op {
            "list" => {
                let windows: Vec<Value> = desktop
                    .windows()
                    .iter()
                    .map(|w| {
                        let r = w.rect();
                        json!({
                            "id": w.id,
                            "title": w.title,
                            "x": r.x,
                            "y": r.y,
                            "w": r.w,
                            "h": r.h,
                            "focused": desktop.focused() == Some(w.id),
                        })
                    })
                    .collect();
                json!({"windows": windows})
            }
            "inspect" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                // 检查窗口是否存在
                if desktop.window(id).is_none() {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("窗口 {id} 不存在"),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                let w = desktop.window(id).unwrap();
                let r = w.rect();
                json!({
                    "id": w.id,
                    "title": w.title.clone(),
                    "x": r.x,
                    "y": r.y,
                    "w": r.w,
                    "h": r.h,
                    "focused": desktop.focused() == Some(w.id),
                })
            }
            "focus" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                if desktop.window(id).is_none() {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("窗口 {id} 不存在"),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                desktop.focus(id);
                json!({"id": id, "focused": true})
            }
            "move" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let x = req.input.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let y = req.input.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                if let Some(win) = desktop.window_mut(id) {
                    win.move_to(x, y);
                    json!({"id": id, "x": x, "y": y, "moved": true})
                } else {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("窗口 {id} 不存在"),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
            }
            "resize" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let wdt = req.input.get("w").and_then(|v| v.as_u64()).unwrap_or(100) as u32;
                let hgt = req.input.get("h").and_then(|v| v.as_u64()).unwrap_or(100) as u32;
                if let Some(win) = desktop.window_mut(id) {
                    win.resize(wdt, hgt);
                    json!({"id": id, "w": wdt, "h": hgt, "resized": true})
                } else {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("窗口 {id} 不存在"),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
            }
            "close" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let ok = desktop.close(id);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("窗口 {id} 不存在"),
                        source: Some("window".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                json!({"id": id, "closed": true})
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("window".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("window.{}", op)))
    }
}

// ============================================================================
// Agent Capability
// ============================================================================

pub struct AgentCapability;

impl AgentCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

impl SystemCapability for AgentCapability {
    fn id(&self) -> String {
        "agent".to_string()
    }

    fn description(&self) -> &str {
        "智能体操作：列表/查看/启动/停止"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["list", "inspect", "start", "stop"]},
                "id": {"type": "integer"}
            },
            "required": ["op"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["agent:read".to_string(), "agent:control".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");

        // 按操作细分权限：查询只要 agent:read，启停只要 agent:control。
        // 原先无条件要求两个权限，导致 Shell 的 inspect 与 GUI 的 start 必然被拒，
        // 三个入口出现「同一个操作有的成功有的失败」的不一致。
        let required: Vec<Permission> = match op {
            "list" | "inspect" => vec!["agent:read".to_string()],
            "start" | "stop" => vec!["agent:control".to_string()],
            _ => self.required_permissions(),
        };
        enforce_permissions(&req.context, &required)?;

        match op {
            "list" => {
                // 结构化快照，不再让调用方去解析 human-readable 文本。
                let agents: Vec<Value> = crate::agent::snapshot()
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.id,
                            "name": a.name,
                            "role": a.role,
                            "state": a.state.name(),
                            "exec_id": a.exec_id,
                        })
                    })
                    .collect();
                Ok(ok_response(&req, json!({"agents": agents}), "agent.list"))
            }
            "inspect" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("agent".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                match crate::agent::snapshot().into_iter().find(|a| a.id == id) {
                    Some(a) => Ok(ok_response(
                        &req,
                        json!({
                            "id": a.id,
                            "name": a.name,
                            "role": a.role,
                            "state": a.state.name(),
                            "exec_id": a.exec_id,
                            "found": true,
                        }),
                        "agent.inspect",
                    )),
                    None => Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("智能体 {id} 不存在"),
                        source: Some("agent".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    }),
                }
            }
            "start" => {
                let id = req.input.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                // 与 Shell/GUI/act 通道共用同一个 agent::start 实现。
                match crate::agent::start(&id.to_string()) {
                    Ok(msg) => {
                        publish_from_capability(
                            &req.context,
                            SystemEventType::AgentStarted,
                            json!({"agent_id": id.to_string(), "id": id, "result": msg}),
                        );
                        Ok(ok_response(
                            &req,
                            json!({"op": "start", "id": id, "result": msg}),
                            "agent.start",
                        ))
                    }
                    Err(e) => Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: e,
                        source: Some("agent".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    }),
                }
            }
            "stop" => {
                let id = req.input.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                match crate::agent::stop(&id.to_string()) {
                    Ok(msg) => {
                        publish_from_capability(
                            &req.context,
                            SystemEventType::AgentStopped,
                            json!({"agent_id": id.to_string(), "id": id, "result": msg}),
                        );
                        Ok(ok_response(
                            &req,
                            json!({"op": "stop", "id": id, "result": msg}),
                            "agent.stop",
                        ))
                    }
                    Err(e) => Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: e,
                        source: Some("agent".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    }),
                }
            }
            _ => Err(CapabilityError {
                code: ErrorCode::InvalidInput,
                message: format!("未知操作: {op}"),
                source: Some("agent".to_string()),
                retryable: false,
                audit: req.audit.clone(),
            }),
        }
    }
}

// ============================================================================
// Task Capability
// ============================================================================

pub struct TaskCapability;

impl TaskCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

impl SystemCapability for TaskCapability {
    fn id(&self) -> String {
        "task".to_string()
    }

    fn description(&self) -> &str {
        "任务操作：创建/查看/取消/暂停/恢复"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["create", "inspect", "cancel", "pause", "resume", "list", "progress"]},
                "id": {"type": "integer"},
                "description": {"type": "string"},
                "agent_id": {"type": "string"},
                "progress": {"type": "integer", "minimum": 0, "maximum": 100},
                "cpu_ns": {"type": "integer"},
                "memory_bytes": {"type": "integer"},
                "message": {"type": "string"}
            },
            "required": ["op"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["task:create".to_string(), "task:read".to_string(), "task:control".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let mut tm = task_mgr().lock().unwrap();

        let result = match op {
            "create" => {
                let description = req
                    .input
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let agent_id = req.input.get("agent_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                let id = tm.create(description.clone(), agent_id.clone());
                publish_from_capability(
                    &req.context,
                    SystemEventType::TaskCreated,
                    json!({"id": id, "description": description, "agent_id": agent_id}),
                );
                json!({"id": id, "created": true})
            }
            "inspect" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let task = tm.get(id).ok_or_else(|| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("任务 {id} 不存在"),
                    source: Some("task".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })?;
                json!({
                    "id": task.id,
                    "description": task.description,
                    "state": match task.state {
                        TaskState::Created => "created",
                        TaskState::Running => "running",
                        TaskState::Paused => "paused",
                        TaskState::Completed => "completed",
                        TaskState::Failed => "failed",
                        TaskState::Cancelled => "cancelled",
                    },
                    "agent_id": task.agent_id,
                    "progress": task.progress,
                    "cpu_ns": task.cpu_ns,
                    "memory_bytes": task.memory_bytes,
                    "created_ns": task.created_ns,
                    "completed_ns": task.completed_ns,
                    "result": task.result,
                    "error": task.error,
                    "timeline": task.timeline.iter().map(|e| json!({
                        "timestamp": e.timestamp,
                        "message": e.message,
                    })).collect::<Vec<_>>()
                })
            }
            "cancel" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let ok = tm.cancel(id);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("任务 {id} 不存在或无法取消"),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                publish_from_capability(
                    &req.context,
                    SystemEventType::TaskCancelled,
                    json!({"id": id}),
                );
                json!({"id": id, "cancelled": true})
            }
            "pause" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let ok = tm.pause(id);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("任务 {id} 不存在或无法暂停"),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                publish_from_capability(
                    &req.context,
                    SystemEventType::TaskPaused,
                    json!({"id": id}),
                );
                json!({"id": id, "paused": true})
            }
            "resume" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let ok = tm.resume(id);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("任务 {id} 不存在或无法恢复"),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                publish_from_capability(
                    &req.context,
                    SystemEventType::TaskResumed,
                    json!({"id": id}),
                );
                json!({"id": id, "resumed": true})
            }
            "list" => {
                let tasks: Vec<Value> = tm
                    .list()
                    .iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "description": t.description,
                            "state": match t.state {
                                TaskState::Created => "created",
                                TaskState::Running => "running",
                                TaskState::Paused => "paused",
                                TaskState::Completed => "completed",
                                TaskState::Failed => "failed",
                                TaskState::Cancelled => "cancelled",
                            },
                            "agent_id": t.agent_id,
                        })
                    })
                    .collect();
                json!({"tasks": tasks})
            }
            "progress" => {
                let id = req
                    .input
                    .get("id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| CapabilityError {
                        code: ErrorCode::InvalidInput,
                        message: "缺少 id 参数".to_string(),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    })?;
                let progress = req
                    .input
                    .get("progress")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u8;
                let cpu_ns = req.input.get("cpu_ns").and_then(|v| v.as_u64()).unwrap_or(0);
                let memory_bytes = req
                    .input
                    .get("memory_bytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let message = req
                    .input
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("进度更新")
                    .to_string();
                let ok = tm.update_progress(id, progress, cpu_ns, memory_bytes, &message);
                if !ok {
                    return Err(CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("任务 {id} 不存在"),
                        source: Some("task".to_string()),
                        retryable: false,
                        audit: req.audit.clone(),
                    });
                }
                json!({"id": id, "progress": progress, "updated": true})
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("task".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("task.{}", op)))
    }
}

// ============================================================================
// System Capability
// ============================================================================

pub struct SystemCapabilityImpl;

impl SystemCapabilityImpl {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

impl SystemCapability for SystemCapabilityImpl {
    fn id(&self) -> String {
        "system".to_string()
    }

    fn description(&self) -> &str {
        "系统操作：状态/资源/日志"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["status", "resources", "logs"]}
            },
            "required": ["op"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["sys:read".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");

        let result = match op {
            "status" => {
                let stats = monitor::get_stats();
                json!({
                    "uptime_ns": stats.system_uptime_ns,
                    "total_tokens": stats.total_tokens,
                    "active_tokens": stats.active_tokens,
                    "tasks_completed": stats.total_tasks_completed,
                    "tasks_failed": stats.total_tasks_failed,
                    "avg_task_time_ns": stats.avg_task_time_ns,
                    "vram_used": stats.total_vram_used,
                    "vram_capacity": stats.total_vram_capacity,
                })
            }
            "resources" => {
                let stats = monitor::get_stats();
                json!({
                    "cpu": {"usage_percent": 0}, // 占位，实际需从宿主获取
                    "memory": {"used": stats.total_vram_used, "capacity": stats.total_vram_capacity},
                    "storage": {"used": 0, "capacity": 0}, // 占位
                    "network": {"bytes_in": 0, "bytes_out": 0}, // 占位
                })
            }
            "logs" => {
                let level = req.input.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                let limit = req.input.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let audit = crate::permission::permission_manager().lock().unwrap().query_audit(Some(limit));
                json!({
                    "level": level,
                    "entries": audit.iter().map(|e| json!({
                        "timestamp": e.timestamp,
                        "actor": e.actor,
                        "action": e.action,
                        "result": match e.result {
                            crate::capability::AuditResult::Granted => "granted",
                            crate::capability::AuditResult::Denied => "denied",
                            crate::capability::AuditResult::Revoked => "revoked",
                            crate::capability::AuditResult::Executed => "executed",
                            crate::capability::AuditResult::Failed => "failed",
                            crate::capability::AuditResult::Cancelled => "cancelled",
                        }
                    })).collect::<Vec<_>>()
                })
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("system".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("system.{}", op)))
    }
}

// ============================================================================
// Settings Capability
// ============================================================================

pub struct SettingsCapability;

impl SettingsCapability {
    pub fn new() -> Arc<dyn SystemCapability> {
        Arc::new(Self)
    }
}

impl SystemCapability for SettingsCapability {
    fn id(&self) -> String {
        "settings".to_string()
    }

    fn description(&self) -> &str {
        "设置操作：读取/更新"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["read", "update"]},
                "key": {"type": "string"},
                "value": {}
            },
            "required": ["op", "key"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn error_schema(&self) -> Value {
        json!({"type": "object", "properties": {"code": {"type": "string"}, "message": {"type": "string"}}})
    }

    fn required_permissions(&self) -> Vec<String> {
        vec!["settings:read".to_string(), "settings:write".to_string()]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        enforce_permissions(&req.context, &self.required_permissions())?;
        check_cancellation(&req.cancellation_token)?;
        let start_ns = now_ns();
        check_timeout(start_ns, req.timeout_ns)?;
        let op = req.input.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let key = req.input.get("key").and_then(|v| v.as_str()).unwrap_or("");

        let mut store = settings_store().lock().unwrap();

        let result = match op {
            "read" => {
                let value = store.get(key).cloned().unwrap_or(Value::Null);
                json!({"key": key, "value": value})
            }
            "update" => {
                let value = req.input.get("value").cloned().unwrap_or(Value::Null);
                store.insert(key.to_string(), value.clone());
                json!({"key": key, "value": value, "updated": true})
            }
            _ => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {op}"),
                    source: Some("settings".to_string()),
                    retryable: false,
                    audit: req.audit.clone(),
                })
            }
        };

        Ok(ok_response(&req, result, &format!("settings.{}", op)))
    }
}

// ============================================================================
// 注册所有能力
// ============================================================================

/// 注册所有内置系统能力到全局注册表。
pub fn register_all_capabilities() {
    let reg = capability_registry();
    // 注册是写操作；注意 CapabilityRegistry::register 内部还有一层写锁，
    // 这里持有的是外层写锁，两层都是写锁、顺序固定，不会形成读写互相等待。
    let caps = reg.write().unwrap_or_else(|e| e.into_inner());

    caps.register(FilesystemCapability::new());
    caps.register(ProcessCapability::new());
    caps.register(AgentCapability::new());
    caps.register(TaskCapability::new());
    caps.register(SystemCapabilityImpl::new());
    caps.register(SettingsCapability::new());

    #[cfg(feature = "gui")]
    caps.register(WindowCapability::new());
}

/// 获取所有能力的元数据。
pub fn list_capability_metadata() -> Vec<CapabilityMetadata> {
    let reg = capability_registry();
    reg.read()
        .unwrap_or_else(|e| e.into_inner())
        .list_metadata()
}
