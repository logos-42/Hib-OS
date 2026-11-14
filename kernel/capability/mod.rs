//! 统一系统能力层 (System Capability Layer)
//!
//! Shell、GUI、Agent API 不应各自直接实现系统操作，而是通过
//! CapabilityRegistry 调用统一的系统能力。
//!
//! 每项能力定义：Capability ID / 输入 Schema / 输出 Schema / 错误 Schema /
//! 权限要求 / 执行上下文 / 超时 / 取消 / 进度事件 / 审计信息。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod caps_impl;

/// 能力 ID，全局唯一，格式为 `domain.action`，例如 `filesystem.read`。
pub type CapabilityId = String;

/// 请求 ID，用于关联请求与响应。
pub type RequestId = u64;

/// 事件 ID。
pub type EventId = u64;

/// 权限字符串，格式为 `resource:action:scope`，例如 `fs:read:/home/*`。
pub type Permission = String;

/// 纳秒时间戳。
pub type Timestamp = u64;

/// 全局请求 ID 生成器。
static REQ_ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static EVT_ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_request_id() -> RequestId {
    REQ_ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn next_event_id() -> EventId {
    EVT_ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 获取当前纳秒时间戳。
pub fn now_ns() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ============================================================================
// 核心执行抽象
// ============================================================================

/// 能力请求。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityRequest {
    /// 能力 ID，例如 `filesystem.read`。
    pub id: CapabilityId,
    /// 输入参数（JSON）。
    pub input: Value,
    /// 执行上下文（调用者身份、权限、配额）。
    pub context: ExecutionContext,
    /// 超时（纳秒），None 表示无超时。
    pub timeout_ns: Option<u64>,
    /// 取消令牌（不序列化）。
    #[serde(skip)]
    pub cancellation_token: Option<CancellationToken>,
    /// 审计信息。
    pub audit: AuditInfo,
}

/// 能力响应。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityResponse {
    /// 关联的请求 ID。
    pub request_id: RequestId,
    /// 输出结果（JSON）。
    pub output: Value,
    /// 执行耗时（毫秒）。
    pub duration_ms: u64,
    /// 审计信息。
    pub audit: AuditInfo,
}

/// 能力错误。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityError {
    /// 错误码。
    pub code: ErrorCode,
    /// 人类可读的错误信息。
    pub message: String,
    /// 错误来源（可选）。
    pub source: Option<String>,
    /// 是否可重试。
    pub retryable: bool,
    /// 审计信息。
    pub audit: AuditInfo,
}

/// 错误码。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorCode {
    Success,
    NotFound,
    InvalidInput,
    PermissionDenied,
    NotAuthorized,
    Timeout,
    Cancelled,
    ResourceExhausted,
    InternalError,
    Unavailable,
    Conflict,
    AlreadyExists,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCode::Success => write!(f, "Success"),
            ErrorCode::NotFound => write!(f, "NotFound"),
            ErrorCode::InvalidInput => write!(f, "InvalidInput"),
            ErrorCode::PermissionDenied => write!(f, "PermissionDenied"),
            ErrorCode::NotAuthorized => write!(f, "NotAuthorized"),
            ErrorCode::Timeout => write!(f, "Timeout"),
            ErrorCode::Cancelled => write!(f, "Cancelled"),
            ErrorCode::ResourceExhausted => write!(f, "ResourceExhausted"),
            ErrorCode::InternalError => write!(f, "InternalError"),
            ErrorCode::Unavailable => write!(f, "Unavailable"),
            ErrorCode::Conflict => write!(f, "Conflict"),
            ErrorCode::AlreadyExists => write!(f, "AlreadyExists"),
        }
    }
}

/// 进度事件。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityEvent {
    /// 事件 ID。
    pub event_id: EventId,
    /// 时间戳（纳秒）。
    pub timestamp: Timestamp,
    /// 能力 ID。
    pub capability_id: CapabilityId,
    /// 事件类型。
    pub event_type: EventType,
    /// 关联 ID（用于追踪同一任务的事件链）。
    pub correlation_id: Option<RequestId>,
    /// 事件载荷（JSON）。
    pub payload: Value,
}

/// 事件类型。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    Progress,
    Started,
    Completed,
    Failed,
    Cancelled,
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    StateChanged,
}

/// 执行上下文。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// 调用者 ID（Agent ID / Shell session ID / GUI session ID）。
    pub caller_id: String,
    /// 调用者类型。
    pub caller_type: CallerType,
    /// 调用者持有的权限列表。
    pub permissions: Vec<Permission>,
    /// 资源配额。
    pub resource_quota: ResourceQuota,
}

/// 调用者类型。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallerType {
    Agent,
    Shell,
    Gui,
    System,
}

/// 资源配额。
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ResourceQuota {
    /// 最大 CPU 时间（纳秒），0 表示无限制。
    pub max_cpu_ns: u64,
    /// 最大内存（字节），0 表示无限制。
    pub max_memory_bytes: u64,
    /// 最大存储（字节），0 表示无限制。
    pub max_storage_bytes: u64,
    /// 最大网络带宽（字节/秒），0 表示无限制。
    pub max_network_bps: u64,
    /// 最大并发任务数，0 表示无限制。
    pub max_concurrent_tasks: u64,
}

/// 审计信息。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditInfo {
    /// 操作者（Agent ID 或用户 ID）。
    pub actor: String,
    /// 动作名称。
    pub action: String,
    /// 时间戳（纳秒）。
    pub timestamp: Timestamp,
    /// 执行结果。
    pub result: AuditResult,
    /// 关联 ID。
    pub correlation_id: Option<RequestId>,
}

/// 审计结果。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditResult {
    Granted,
    Denied,
    Revoked,
    Executed,
    Failed,
    Cancelled,
}

/// 取消令牌。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 标记为已取消。
    pub fn cancel(&self) {
        self.inner.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ============================================================================
// SystemCapability trait
// ============================================================================

/// 系统能力 trait。每项能力实现此 trait 并注册到 CapabilityRegistry。
pub trait SystemCapability: Send + Sync {
    /// 能力 ID。
    fn id(&self) -> CapabilityId;
    /// 能力描述。
    fn description(&self) -> &str;
    /// JSON Schema：输入参数模式。
    fn input_schema(&self) -> Value;
    /// JSON Schema：输出结果模式。
    fn output_schema(&self) -> Value;
    /// JSON Schema：错误模式。
    fn error_schema(&self) -> Value;
    /// 执行该能力所需的权限列表。
    fn required_permissions(&self) -> Vec<Permission>;
    /// 执行能力。
    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError>;
}

/// 能力注册表。存储所有已注册的系统能力。
///
/// 注册表是典型的「读多写少」：启动时注册一次，运行期绝大多数调用都是
/// `get` / `list_ids` / `list_metadata` 这类只读查询。
/// 因此用 `RwLock` 而不是 `Mutex`，让多个入口（Shell / GUI / Agent）可以并发查询，
/// 只有注册新能力时才独占写锁。
pub struct CapabilityRegistry {
    capabilities: RwLock<HashMap<CapabilityId, Arc<dyn SystemCapability>>>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            capabilities: RwLock::new(HashMap::new()),
        }
    }

    /// 注册一个能力（独占写锁）。
    pub fn register(&self, cap: Arc<dyn SystemCapability>) {
        let id = cap.id().clone();
        self.capabilities
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, cap);
    }

    /// 查找一个能力（共享读锁，返回 Arc 克隆，避免生命周期问题）。
    pub fn get(&self, id: &str) -> Option<Arc<dyn SystemCapability>> {
        let caps = self.capabilities.read().unwrap_or_else(|e| e.into_inner());
        caps.get(id).cloned()
    }

    /// 列出所有能力 ID（共享读锁）。
    pub fn list_ids(&self) -> Vec<CapabilityId> {
        let caps = self.capabilities.read().unwrap_or_else(|e| e.into_inner());
        caps.keys().cloned().collect()
    }

    /// 获取所有能力的元数据（共享读锁，用于 capability list --json）。
    pub fn list_metadata(&self) -> Vec<CapabilityMetadata> {
        let caps = self.capabilities.read().unwrap_or_else(|e| e.into_inner());
        caps.values()
            .map(|c| CapabilityMetadata {
                id: c.id().clone(),
                description: c.description().to_string(),
                required_permissions: c.required_permissions(),
            })
            .collect()
    }
}

/// 能力元数据（供 list 接口使用）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityMetadata {
    pub id: CapabilityId,
    pub description: String,
    pub required_permissions: Vec<Permission>,
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局能力注册表单例。
static CAP_REGISTRY: std::sync::OnceLock<RwLock<CapabilityRegistry>> =
    std::sync::OnceLock::new();

/// 获取全局能力注册表。
///
/// 只读查询请用 `.read()`，注册新能力请用 `.write()`。
/// 返回值仍是一个锁，保持了原有调用形态；锁中毒时用 `into_inner()` 恢复，
/// 避免一个调用方 panic 导致整个能力层永久不可用。
pub fn capability_registry() -> &'static RwLock<CapabilityRegistry> {
    CAP_REGISTRY.get_or_init(|| RwLock::new(CapabilityRegistry::new()))
}

/// 注册一个能力到全局注册表（写锁）。
pub fn register_capability(cap: Arc<dyn SystemCapability>) {
    capability_registry()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .register(cap);
}

/// 以共享读锁执行一次只读查询，省去每个调用点手写 `.read()` 的样板。
pub fn with_registry_read<R>(f: impl FnOnce(&CapabilityRegistry) -> R) -> R {
    let reg = capability_registry()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    f(&reg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn request_id_monotonic() {
        let a = next_request_id();
        let b = next_request_id();
        assert!(b > a);
    }

    #[test]
    fn event_id_monotonic() {
        let a = next_event_id();
        let b = next_event_id();
        assert!(b > a);
    }

    #[test]
    fn registry_register_and_list() {
        let reg = CapabilityRegistry::new();
        assert!(reg.list_ids().is_empty());
    }

    #[test]
    fn error_code_display() {
        assert_eq!(ErrorCode::PermissionDenied.to_string(), "PermissionDenied");
        assert_eq!(ErrorCode::Timeout.to_string(), "Timeout");
    }

    #[test]
    fn cancellation_token_default_not_cancelled() {
        let t = CancellationToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
    }

    #[test]
    fn now_ns_nonzero() {
        let t = now_ns();
        assert!(t > 0);
    }

    /// 读多写少：多个线程同时查询注册表不应互相阻塞，也不应死锁。
    #[test]
    fn registry_allows_concurrent_reads() {
        struct DummyCapability;
        impl SystemCapability for DummyCapability {
            fn id(&self) -> CapabilityId { "test.dummy".to_string() }
            fn description(&self) -> &str { "测试用能力" }
            fn input_schema(&self) -> Value { Value::Null }
            fn output_schema(&self) -> Value { Value::Null }
            fn error_schema(&self) -> Value { Value::Null }
            fn required_permissions(&self) -> Vec<Permission> { Vec::new() }
            fn execute(&self, _req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
                unimplemented!()
            }
        }

        let reg = Arc::new(RwLock::new(CapabilityRegistry::new()));
        reg.write().unwrap().register(Arc::new(DummyCapability));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let reg = Arc::clone(&reg);
                std::thread::spawn(move || {
                    let guard = reg.read().unwrap();
                    assert_eq!(guard.list_ids(), vec!["test.dummy".to_string()]);
                    assert!(guard.get("test.dummy").is_some());
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn audit_info_eq() {
        let a = AuditInfo {
            actor: "x".to_string(),
            action: "y".to_string(),
            timestamp: 1,
            result: AuditResult::Granted,
            correlation_id: None,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
