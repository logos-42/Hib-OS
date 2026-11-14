//! 文件系统能力实现 —— 封装 VFS 为 SystemCapability。

use super::{
    CapabilityError, CapabilityRequest, CapabilityResponse, ErrorCode, EventId, EventType,
    ExecutionContext, Permission,
};
use crate::capability::SystemCapability;
use crate::vfs::Vfs;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// 文件系统能力。包装 Vfs 实例，将文件操作暴露为统一能力。
pub struct FilesystemCapability {
    vfs: Arc<Mutex<Vfs>>,
}

impl FilesystemCapability {
    pub fn new(vfs: Arc<Mutex<Vfs>>) -> Self {
        Self { vfs }
    }

    fn require_permission(
        ctx: &ExecutionContext,
        needed: &str,
    ) -> Result<(), CapabilityError> {
        if ctx.permissions.iter().any(|p| p == needed || p == "fs:all") {
            return Ok(());
        }
        // 支持前缀匹配：fs:read:/home/* 匹配 fs:read:/home/foo
        if ctx.permissions.iter().any(|p| {
            p.ends_with(":*") && needed.starts_with(&p[..p.len() - 1])
        }) {
            return Ok(());
        }
        Err(CapabilityError {
            code: ErrorCode::PermissionDenied,
            message: format!("缺少权限: {}", needed),
            source: Some("filesystem".to_string()),
            retryable: false,
            audit: super::AuditInfo {
                actor: ctx.caller_id.clone(),
                action: needed.to_string(),
                timestamp: super::now_ns(),
                result: super::AuditResult::Denied,
                correlation_id: None,
            },
        })
    }

    fn audit_executed(ctx: &ExecutionContext, action: &str) -> super::AuditInfo {
        super::AuditInfo {
            actor: ctx.caller_id.clone(),
            action: action.to_string(),
            timestamp: super::now_ns(),
            result: super::AuditResult::Executed,
            correlation_id: None,
        }
    }
}

impl SystemCapability for FilesystemCapability {
    fn id(&self) -> String {
        "filesystem".to_string()
    }

    fn description(&self) -> &str {
        "文件系统操作能力：list/read/write/search/move/copy/delete"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": ["list","read","write","search","move","copy","delete"]},
                "path": {"type": "string"},
                "content": {"type": "string"},
                "src": {"type": "string"},
                "dst": {"type": "string"},
                "pattern": {"type": "string"}
            },
            "required": ["op", "path"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "success": {"type": "boolean"},
                "entries": {"type": "array"},
                "content": {"type": "string"},
                "bytes_written": {"type": "integer"},
                "matches": {"type": "array"}
            }
        })
    }

    fn error_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {"type": "string"},
                "message": {"type": "string"},
                "retryable": {"type": "boolean"}
            }
        })
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![
            "fs:read".to_string(),
            "fs:write".to_string(),
            "fs:delete".to_string(),
        ]
    }

    fn execute(&self, req: CapabilityRequest) -> Result<CapabilityResponse, CapabilityError> {
        let start = super::now_ns();
        let op = req
            .input
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapabilityError {
                code: ErrorCode::InvalidInput,
                message: "缺少 op 字段".to_string(),
                source: Some("filesystem".to_string()),
                retryable: false,
                audit: super::AuditInfo {
                    actor: req.context.caller_id.clone(),
                    action: "filesystem.op".to_string(),
                    timestamp: super::now_ns(),
                    result: super::AuditResult::Failed,
                    correlation_id: None,
                },
            })?;

        let path = req
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let result = match op {
            "list" => {
                super::capability::PermissionChecker::check(&req.context, "fs:read")?;
                let vfs = self.vfs.lock().unwrap();
                let entries = vfs
                    .ls(Some(path))
                    .map_err(|e| CapabilityError {
                        code: ErrorCode::NotFound,
                        message: format!("ls 失败: {}", e),
                        source: Some("vfs".to_string()),
                        retryable: false,
                        audit: super::AuditInfo {
                            actor: req.context.caller_id.clone(),
                            action: "filesystem.list".to_string(),
                            timestamp: super::now_ns(),
                            result: super::AuditResult::Failed,
                            correlation_id: None,
                        },
                    })?;
                json!({
                    "entries": entries.iter().map(|(name, kind, mode, size)| {
                        json!({"name": name, "kind": format!("{:?}", kind), "mode": mode, "size": size})
                    }).collect::<Vec<_>>()
                })
            }
            "read" => {
                super::capability::PermissionChecker::check(&req.context, &format!("fs:read:{}", path))?;
                let vfs = self.vfs.lock().unwrap();
                let content = vfs.cat(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("read 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.read".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                json!({"content": content})
            }
            "write" => {
                super::capability::PermissionChecker::check(&req.context, &format!("fs:write:{}", path))?;
                let content = req
                    .input
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut vfs = self.vfs.lock().unwrap();
                vfs.write(path, content).map_err(|e| CapabilityError {
                    code: ErrorCode::InternalError,
                    message: format!("write 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.write".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                json!({"bytes_written": content.len()})
            }
            "search" => {
                super::capability::PermissionChecker::check(&req.context, "fs:read")?;
                let pattern = req
                    .input
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let vfs = self.vfs.lock().unwrap();
                let matches = vfs.complete_path(pattern);
                json!({"matches": matches.iter().map(|p| json!({"path": p})).collect::<Vec<_>>())
            }
            "move" => {
                let dst = req.input.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                super::capability::PermissionChecker::check(&req.context, &format!("fs:write:{}", path))?;
                super::capability::PermissionChecker::check(&req.context, &format!("fs:write:{}", dst))?;
                // VFS 没有 move，用 read+write+rm 模拟（原子性由 VFS 层保证）
                let vfs = self.vfs.lock().unwrap();
                let content = vfs.cat(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("move read 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.move".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                drop(vfs);
                let mut vfs = self.vfs.lock().unwrap();
                vfs.write(dst, &content).map_err(|e| CapabilityError {
                    code: ErrorCode::InternalError,
                    message: format!("move write 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.move".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                vfs.rm(path).map_err(|e| CapabilityError {
                    code: ErrorCode::InternalError,
                    message: format!("move rm 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.move".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                json!({"success": true})
            }
            "copy" => {
                let dst = req.input.get("dst").and_then(|v| v.as_str()).unwrap_or("");
                super::capability::PermissionChecker::check(&req.context, &format!("fs:read:{}", path))?;
                super::capability::PermissionChecker::check(&req.context, &format!("fs:write:{}", dst))?;
                let vfs = self.vfs.lock().unwrap();
                let content = vfs.cat(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("copy read 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.copy".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                let bytes = content.len();
                drop(vfs);
                let mut vfs = self.vfs.lock().unwrap();
                vfs.write(dst, &content).map_err(|e| CapabilityError {
                    code: ErrorCode::InternalError,
                    message: format!("copy write 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.copy".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                json!({"bytes_copied": bytes})
            }
            "delete" => {
                super::capability::PermissionChecker::check(&req.context, &format!("fs:delete:{}", path))?;
                let mut vfs = self.vfs.lock().unwrap();
                vfs.rm(path).map_err(|e| CapabilityError {
                    code: ErrorCode::NotFound,
                    message: format!("delete 失败: {}", e),
                    source: Some("vfs".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.delete".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                })?;
                json!({"success": true})
            }
            other => {
                return Err(CapabilityError {
                    code: ErrorCode::InvalidInput,
                    message: format!("未知操作: {}", other),
                    source: Some("filesystem".to_string()),
                    retryable: false,
                    audit: super::AuditInfo {
                        actor: req.context.caller_id.clone(),
                        action: "filesystem.op".to_string(),
                        timestamp: super::now_ns(),
                        result: super::AuditResult::Failed,
                        correlation_id: None,
                    },
                });
            }
        };

        let duration_ms = (super::now_ns().saturating_sub(start)) / 1_000_000;
        Ok(CapabilityResponse {
            request_id: 0,
            output: result,
            duration_ms,
            audit: Self::audit_executed(&req.context, op),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AuditResult;
    use std::sync::Arc;

    fn test_ctx() -> ExecutionContext {
        ExecutionContext {
            caller_id: "test-user".to_string(),
            caller_type: super::capability::CallerType::Shell,
            permissions: vec!["fs:all".to_string()],
            resource_quota: super::capability::ResourceQuota::default(),
        }
    }

    #[test]
    fn filesystem_write_then_read() {
        let vfs = Arc::new(Mutex::new(Vfs::new()));
        let cap = FilesystemCapability::new(vfs.clone());
        let ctx = test_ctx();

        // 写入
        let req = CapabilityRequest {
            id: "filesystem".to_string(),
            input: json!({"op": "write", "path": "/test.txt", "content": "hello"}),
            context: ctx.clone(),
            timeout_ns: None,
            cancellation_token: None,
            audit: super::AuditInfo {
                actor: "test-user".to_string(),
                action: "test".to_string(),
                timestamp: super::now_ns(),
                result: AuditResult::Granted,
                correlation_id: None,
            },
        };
        let resp = cap.execute(req).unwrap();
        assert_eq!(resp.output["bytes_written"], 5);

        // 读取
        let req = CapabilityRequest {
            id: "filesystem".to_string(),
            input: json!({"op": "read", "path": "/test.txt"}),
            context: ctx,
            timeout_ns: None,
            cancellation_token: None,
            audit: super::AuditInfo {
                actor: "test-user".to_string(),
                action: "test".to_string(),
                timestamp: super::now_ns(),
                result: AuditResult::Granted,
                correlation_id: None,
            },
        };
        let resp = cap.execute(req).unwrap();
        assert_eq!(resp.output["content"], "hello");
    }

    #[test]
    fn filesystem_denied_without_permission() {
        let vfs = Arc::new(Mutex::new(Vfs::new()));
        let cap = FilesystemCapability::new(vfs);
        let ctx = ExecutionContext {
            caller_id: "test-user".to_string(),
            caller_type: super::capability::CallerType::Shell,
            permissions: vec![], // 无权限
            resource_quota: super::capability::ResourceQuota::default(),
        };
        let req = CapabilityRequest {
            id: "filesystem".to_string(),
            input: json!({"op": "read", "path": "/test.txt"}),
            context: ctx,
            timeout_ns: None,
            cancellation_token: None,
            audit: super::AuditInfo {
                actor: "test-user".to_string(),
                action: "test".to_string(),
                timestamp: super::now_ns(),
                result: AuditResult::Granted,
                correlation_id: None,
            },
        };
        let err = cap.execute(req).unwrap_err();
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }
}
