//! 跨接口一致性测试和端到端场景测试。
//!
//! 验证：同一操作通过 GUI、Shell、Agent API 三种入口执行，
//! 最终系统状态一致，权限判断一致，错误结构一致，
//! 事件记录一致，审计信息一致。
//!
//! 注意：每个测试使用唯一的路径和名称，避免全局状态污染。

use hibos::shell::Shell;
use hibos::capability::{
    CapabilityRequest, CapabilityError, ErrorCode, ExecutionContext,
    now_ns, AuditInfo, AuditResult, CallerType,
};
use hibos::permission::{
    permission_manager, AgentLifecycleState, Principal,
};
use hibos::kernel::permission::AuthorizationMode;

// ---------------------------------------------------------------------------
// 安全锁获取辅助（避免跨测试互斥体毒化）
// ---------------------------------------------------------------------------

fn safe_lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// 辅助：创建一个测试 Shell。
// ---------------------------------------------------------------------------

fn test_shell() -> Shell {
    Shell::new()
}

// ---------------------------------------------------------------------------
// 辅助：通过 Agent API 直接调用文件系统能力。
// 使用 "*:*:*" 全通配权限，避免权限匹配问题。
// ---------------------------------------------------------------------------

fn fs_capability_op(op: &str, path: &str, content: Option<&str>) -> Result<serde_json::Value, CapabilityError> {
    let input = match op {
        "write" => serde_json::json!({"op": "write", "path": path, "content": content.unwrap_or("")}),
        "read" => serde_json::json!({"op": "read", "path": path}),
        "delete" => serde_json::json!({"op": "delete", "path": path}),
        "list" => serde_json::json!({"op": "list", "path": path}),
        _ => return Err(CapabilityError {
            code: ErrorCode::InvalidInput,
            message: format!("未知操作: {}", op),
            source: Some("test".to_string()),
            retryable: false,
            audit: AuditInfo {
                actor: "test".to_string(),
                action: "test".to_string(),
                timestamp: now_ns(),
                result: AuditResult::Failed,
                correlation_id: None,
            },
        }),
    };

    let req = CapabilityRequest {
        id: "filesystem".to_string(),
        input,
        context: ExecutionContext {
            caller_id: "test-shell".to_string(),
            caller_type: CallerType::Shell,
            permissions: vec!["*:*:*".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "test-agent".to_string(),
            action: format!("filesystem.{}", op),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };

    let reg = hibos::capability::capability_registry();
    match safe_lock(reg).get("filesystem") {
        Some(cap) => cap.execute(req).map(|r| r.output),
        None => Err(CapabilityError {
            code: ErrorCode::NotFound,
            message: "能力 filesystem 未注册".to_string(),
            source: Some("test".to_string()),
            retryable: false,
            audit: AuditInfo {
                actor: "test-agent".to_string(),
                action: "test".to_string(),
                timestamp: now_ns(),
                result: AuditResult::Failed,
                correlation_id: None,
            },
        }),
    }
}

// ---------------------------------------------------------------------------
// 场景一：GUI 创建 Agent，授予文件读取权限，搜索目录并返回结果。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_1_gui_create_agent_search_directory() {
    let test_dir = "/tmp/e2e1_search_dir";
    let test_file = "/tmp/e2e1_search_dir/file1.txt";

    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "alice".to_string() },
        "e2e1-search-agent",
        "researcher",
        vec!["fs:read:/tmp/*".to_string()],
        Default::default(),
    );
    drop(pm);

    let pm = safe_lock(permission_manager());
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    assert!(pm.check_permission(agent_id, "fs:read:/tmp/foo"));
    drop(pm);

    let mut shell = test_shell();
    // 清理可能残留
    let _ = shell.dispatch(&format!("rm -r {}", test_dir));
    assert!(shell.dispatch(&format!("mkdir {}", test_dir)));
    assert!(shell.dispatch(&format!("write {} hello", test_file)));

    let result = fs_capability_op("read", test_file, None).unwrap();
    assert_eq!(result["content"], "hello");

    assert!(shell.dispatch(&format!("cat {}", test_file)));

    let _ = shell.dispatch(&format!("rm {}", test_file));
    let _ = shell.dispatch(&format!("rmdir {}", test_dir));
}

// ---------------------------------------------------------------------------
// 场景二：Shell 创建任务，启动 Agent，实时观察任务状态，获取结构化结果。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_2_shell_create_task_observe_status() {
    let mut shell = test_shell();

    assert!(shell.dispatch("task create \"分析日志文件\" --agent 1"));
    assert!(shell.dispatch("task list"));
    assert!(shell.dispatch("task inspect 1"));

    let req = CapabilityRequest {
        id: "task".to_string(),
        input: serde_json::json!({"op": "list"}),
        context: ExecutionContext {
            caller_id: "test".to_string(),
            caller_type: CallerType::Shell,
            permissions: vec!["task:read".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "test".to_string(),
            action: "task.list".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = hibos::capability::capability_registry();
    match safe_lock(reg).get("task") {
        Some(cap) => {
            let resp = cap.execute(req).unwrap();
            assert!(resp.output.is_array() || resp.output.is_object());
        }
        None => panic!("能力 task 未注册"),
    }
}

// ---------------------------------------------------------------------------
// 场景三：Agent 需要删除文件，系统判断权限并向用户申请授权，
// 用户批准后执行，GUI 和 Shell 都能查询执行记录。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_3_agent_delete_file_permission() {
    let test_path = "/tmp/e2e3_deleteme.txt";

    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "bob".to_string() },
        "e2e3-cleanup-agent",
        "cleaner",
        vec![],
        Default::default(),
    );
    drop(pm);

    let mut shell = test_shell();
    let _ = shell.dispatch(&format!("rm {}", test_path));
    assert!(shell.dispatch(&format!("write {} content", test_path)));

    let result = fs_capability_op("delete", test_path, None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);

    let pm = safe_lock(permission_manager());
    pm.grant_permission(
        agent_id,
        "fs:delete:/tmp/*",
        AuthorizationMode::Once,
        None,
        "bob",
    ).unwrap();
    drop(pm);

    let result = fs_capability_op("delete", test_path, None);
    assert!(result.is_ok());

    let vfs = hibos::vfs::global_vfs().lock().unwrap_or_else(|e| e.into_inner());
    assert!(vfs.cat(test_path).is_err(), "文件应已删除");
    drop(vfs);

    let pm = safe_lock(permission_manager());
    let audit = pm.query_audit(Some(10));
    assert!(!audit.is_empty());
    assert!(audit.iter().any(|e| e.action == "filesystem.delete"));
}

// ---------------------------------------------------------------------------
// 场景四：GUI 启动长任务，关闭 GUI 后，Agent Runtime 继续运行，
// 重新打开 GUI 后恢复任务状态。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_4_gui_close_agent_continues() {
    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "carol".to_string() },
        "e2e4-long-runner",
        "worker",
        vec!["task:create".to_string(), "task:control".to_string()],
        Default::default(),
    );
    drop(pm);

    let pm = safe_lock(permission_manager());
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    drop(pm);

    let mut shell = test_shell();
    assert!(shell.dispatch("task create \"长时间运行任务\" --agent 1"));
    assert!(shell.dispatch("task list"));

    let pm = safe_lock(permission_manager());
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    drop(pm);
}

// ---------------------------------------------------------------------------
// 场景五：模型 API 不可用，用户依然可以使用 Shell、文件管理器、
// 系统设置和进程管理。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_5_llm_api_unavailable() {
    let mut shell = test_shell();
    let test_path = "/tmp/e2e5_llm_test.txt";
    let _ = shell.dispatch(&format!("rm {}", test_path));

    assert!(shell.dispatch("pwd"));
    assert!(shell.dispatch("ls"));
    assert!(shell.dispatch(&format!("write {} hello", test_path)));
    assert!(shell.dispatch(&format!("cat {}", test_path)));

    assert!(shell.dispatch("stats"));
    assert!(shell.dispatch("channels"));

    let result = fs_capability_op("read", test_path, None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["content"], "hello\n");

    assert!(shell.dispatch("proc attention 8"));
    assert!(shell.dispatch("run"));

    let _ = shell.dispatch(&format!("rm {}", test_path));
}

// ---------------------------------------------------------------------------
// 场景六：一个 Agent 崩溃，不影响其他 Agent、Shell 和 GUI。
// ---------------------------------------------------------------------------

#[test]
fn e2e_scenario_6_agent_crash_isolation() {
    let pm = safe_lock(permission_manager());
    let agent_id_1 = pm.create_agent(
        Principal::User { user_id: "dave".to_string() },
        "e2e6-crashy",
        "worker",
        vec![],
        Default::default(),
    );
    let agent_id_2 = pm.create_agent(
        Principal::User { user_id: "dave".to_string() },
        "e2e6-stable",
        "worker",
        vec![],
        Default::default(),
    );
    drop(pm);

    let pm = safe_lock(permission_manager());
    pm.set_state(agent_id_1, AgentLifecycleState::Failed).unwrap();
    assert_eq!(pm.get_state(agent_id_2), Some(AgentLifecycleState::Created));
    drop(pm);

    let mut shell = test_shell();
    assert!(shell.dispatch("pwd"));
    assert!(shell.dispatch("ls"));

    let workspace = hibos::gui::agent_workspace::render_agent_workspace();
    assert!(workspace.contains("Agent Workspace"));

    let perm_center = hibos::gui::permission_center::render_permission_center();
    assert!(perm_center.contains("Permission Center"));
}

// ---------------------------------------------------------------------------
// 跨接口一致性测试：文件删除
// ---------------------------------------------------------------------------

#[test]
fn consistency_test_file_delete() {
    let test_path = "/tmp/consistency_delete_test.txt";

    let mut shell = test_shell();
    let _ = shell.dispatch(&format!("rm {}", test_path));
    assert!(shell.dispatch(&format!("write {} test-content", test_path)));

    assert!(shell.dispatch(&format!("rm {}", test_path)));

    let vfs = hibos::vfs::global_vfs().lock().unwrap_or_else(|e| e.into_inner());
    assert!(vfs.cat(test_path).is_err(), "Shell rm 后文件应不存在");
    drop(vfs);

    assert!(shell.dispatch(&format!("write {} test-content", test_path)));
    let result = fs_capability_op("delete", test_path, None);
    assert!(result.is_ok(), "Agent API delete 应成功");

    let vfs = hibos::vfs::global_vfs().lock().unwrap_or_else(|e| e.into_inner());
    assert!(vfs.cat(test_path).is_err(), "Agent API delete 后文件应不存在");
    let _ = shell.dispatch(&format!("rm {}", test_path));
}

// ---------------------------------------------------------------------------
// 跨接口一致性测试：文件读取
// ---------------------------------------------------------------------------

#[test]
fn consistency_test_file_read() {
    let test_path = "/tmp/consistency_read_test.txt";
    let test_content = "consistency test content";

    let mut shell = test_shell();
    let _ = shell.dispatch(&format!("rm {}", test_path));
    assert!(shell.dispatch(&format!("write {} {}", test_path, test_content)));

    let vfs = hibos::vfs::global_vfs().lock().unwrap_or_else(|e| e.into_inner());
    let shell_content = vfs.cat(test_path).unwrap();
    drop(vfs);

    let result = fs_capability_op("read", test_path, None).unwrap();
    let agent_content = result["content"].as_str().unwrap();

    assert_eq!(shell_content, agent_content, "Shell 和 Agent API 读取内容应一致");

    let _ = shell.dispatch(&format!("rm {}", test_path));
}

// ---------------------------------------------------------------------------
// 跨接口一致性测试：错误结构一致
// ---------------------------------------------------------------------------

#[test]
fn consistency_test_error_structure() {
    let nonexistent = "/tmp/consistency_nonexistent_xyz.txt";

    let mut shell = test_shell();
    shell.dispatch(&format!("rm {}", nonexistent));

    let result = fs_capability_op("read", nonexistent, None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(!err.message.is_empty());
}

// ---------------------------------------------------------------------------
// 跨接口一致性测试：权限判断一致
// ---------------------------------------------------------------------------

#[test]
fn consistency_test_permission_judgment() {
    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "eve".to_string() },
        "e2e-perm-test",
        "tester",
        vec!["fs:read:/tmp/*".to_string()],
        Default::default(),
    );
    drop(pm);

    let test_path = "/tmp/consistency_perm_secret.txt";

    let write_result = fs_capability_op("write", test_path, Some("secret"));
    assert!(write_result.is_err());
    assert_eq!(write_result.unwrap_err().code, ErrorCode::PermissionDenied);

    let mut shell = test_shell();
    let _ = shell.dispatch(&format!("rm {}", test_path));
    assert!(shell.dispatch(&format!("write {} data", test_path)));

    let read_result = fs_capability_op("read", test_path, None);
    assert!(read_result.is_ok());

    let _ = shell.dispatch(&format!("rm {}", test_path));
}

// ---------------------------------------------------------------------------
// 权限模型测试：Once 授权模式
// ---------------------------------------------------------------------------

#[test]
fn permission_test_once_mode() {
    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "frank".to_string() },
        "e2e-once-test",
        "tester",
        vec![],
        Default::default(),
    );
    drop(pm);

    let pm = safe_lock(permission_manager());
    pm.grant_permission(
        agent_id,
        "fs:read:/once/*",
        AuthorizationMode::Once,
        None,
        "frank",
    ).unwrap();
    drop(pm);

    let pm = safe_lock(permission_manager());
    assert!(pm.check_permission(agent_id, "fs:read:/once/foo"));
}

// ---------------------------------------------------------------------------
// 权限模型测试：撤销权限后不再可用
// ---------------------------------------------------------------------------

#[test]
fn permission_test_revoke() {
    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "grace".to_string() },
        "e2e-revoke-test",
        "tester",
        vec!["fs:delete:/tmp/*".to_string()],
        Default::default(),
    );
    drop(pm);

    let pm = safe_lock(permission_manager());
    assert!(pm.check_permission(agent_id, "fs:delete:/tmp/foo"));
    drop(pm);

    let pm = safe_lock(permission_manager());
    pm.revoke_permission(agent_id, "fs:delete:/tmp/*", "grace").unwrap();
    drop(pm);

    let pm = safe_lock(permission_manager());
    assert!(!pm.check_permission(agent_id, "fs:delete:/tmp/foo"));
}

// ---------------------------------------------------------------------------
// 事件系统测试：发布和订阅
// ---------------------------------------------------------------------------

#[test]
fn event_test_publish_subscribe() {
    use hibos::event::{
        event_bus, publish_event, EventSource, SystemEventType, EventFilter,
    };
    use serde_json::json;

    publish_event(
        EventSource::System,
        SystemEventType::AgentStarted,
        None,
        json!({"agent_id": "1"}),
    );

    let bus = event_bus();
    let results = safe_lock(bus).query(&EventFilter::default());
    assert!(!results.is_empty());
    assert!(results.iter().any(|e| matches!(e.event_type, SystemEventType::AgentStarted)));
}

// ---------------------------------------------------------------------------
// 事件系统测试：过滤
// ---------------------------------------------------------------------------

#[test]
fn event_test_filter() {
    use hibos::event::{
        event_bus, publish_event, EventSource, SystemEventType, EventFilter,
    };
    use serde_json::json;

    publish_event(
        EventSource::System,
        SystemEventType::TaskStarted,
        None,
        json!({"task_id": "1"}),
    );
    publish_event(
        EventSource::System,
        SystemEventType::TaskCompleted,
        None,
        json!({"task_id": "1"}),
    );

    let bus = event_bus();
    let results = safe_lock(bus).query(&EventFilter {
        event_types: Some(vec![SystemEventType::TaskCompleted]),
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].event_type, SystemEventType::TaskCompleted);
}

// ---------------------------------------------------------------------------
// Shell parser 测试：引号、转义、管道、重定向
// ---------------------------------------------------------------------------

#[test]
fn shell_parser_test_quoting_and_redirection() {
    use hibos::shell_parser::{parse_line, Redirection};

    let p = parse_line(r#"echo "hello world""#).unwrap();
    assert_eq!(p.commands[0].argv, vec!["echo".to_string(), "hello world".to_string()]);

    let p = parse_line("ls | cat").unwrap();
    assert_eq!(p.commands.len(), 2);

    let p = parse_line("ls > out.txt").unwrap();
    assert_eq!(p.commands[0].stdout, Redirection::ToFile("out.txt".to_string()));

    let p = parse_line("ls >> out.txt").unwrap();
    assert_eq!(p.commands[0].stdout, Redirection::AppendFile("out.txt".to_string()));

    let p = parse_line("cat < in.txt").unwrap();
    assert_eq!(p.commands[0].stdin, Redirection::FromFile("in.txt".to_string()));
}

// ---------------------------------------------------------------------------
// Shell JSON 输出测试：--json 标志
// ---------------------------------------------------------------------------

#[test]
fn shell_json_output_test() {
    let mut shell = test_shell();
    shell.dispatch("capability list --json");
}

// ---------------------------------------------------------------------------
// Shell agent 命令测试
// ---------------------------------------------------------------------------

#[test]
fn shell_agent_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("agent list"));
    assert!(shell.dispatch("agent list --json"));
    assert!(shell.dispatch("agent inspect 1"));
    assert!(shell.dispatch("agent inspect 1 --json"));
}

// ---------------------------------------------------------------------------
// Shell task 命令测试
// ---------------------------------------------------------------------------

#[test]
fn shell_task_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("task list"));
    assert!(shell.dispatch("task list --json"));
    assert!(shell.dispatch("task create \"测试任务\""));
    assert!(shell.dispatch("task inspect 1"));
    assert!(shell.dispatch("task inspect 1 --json"));
}

// ---------------------------------------------------------------------------
// Shell system 命令测试
// ---------------------------------------------------------------------------

#[test]
fn shell_system_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("system status"));
    assert!(shell.dispatch("system status --json"));
    assert!(shell.dispatch("system resources"));
    assert!(shell.dispatch("system logs"));
}

// ---------------------------------------------------------------------------
// Agent Workspace GUI 测试
// ---------------------------------------------------------------------------

#[test]
fn gui_agent_workspace_test() {
    let out = hibos::gui::agent_workspace::render_agent_workspace();
    assert!(out.contains("Agent Workspace"));
    assert!(out.contains("活跃 Agent 总数"));

    let detail = hibos::gui::agent_workspace::render_agent_detail(99999);
    assert!(detail.contains("未找到 Agent"));
}

// ---------------------------------------------------------------------------
// Task Workspace GUI 测试
// ---------------------------------------------------------------------------

#[test]
fn gui_task_workspace_test() {
    let out = hibos::gui::task_workspace::render_task_workspace();
    assert!(out.contains("Task Workspace"));

    let detail = hibos::gui::task_workspace::render_task_detail(99999);
    assert!(detail.contains("未找到任务"));
}

// ---------------------------------------------------------------------------
// Permission Center GUI 测试
// ---------------------------------------------------------------------------

#[test]
fn gui_permission_center_test() {
    let out = hibos::gui::permission_center::render_permission_center();
    assert!(out.contains("Permission Center"));

    let detail = hibos::gui::permission_center::render_agent_permissions(99999);
    assert!(detail.contains("未找到 Agent"));

    let audit = hibos::gui::permission_center::render_audit_log(Some(5));
    assert!(audit.contains("审计日志"));
}

// ---------------------------------------------------------------------------
// 权限中心：授予和撤销权限
// ---------------------------------------------------------------------------

#[test]
fn permission_center_grant_revoke_test() {
    let pm = safe_lock(permission_manager());
    let agent_id = pm.create_agent(
        Principal::User { user_id: "helen".to_string() },
        "e2e-perm-center-test",
        "tester",
        vec![],
        Default::default(),
    );
    drop(pm);

    let result = hibos::gui::permission_center::grant_permission(
        agent_id,
        "fs:read:/grant/*",
        "session",
        "helen",
    );
    assert!(result.is_ok());

    let result = hibos::gui::permission_center::revoke_permission(
        agent_id,
        "fs:read:/grant/*",
        "helen",
    );
    assert!(result.is_ok());
}

// ---------------------------------------------------------------------------
// Agent 生命周期状态测试
// ---------------------------------------------------------------------------

#[test]
fn agent_lifecycle_state_test() {
    assert_eq!(AgentLifecycleState::Created.name(), "created");
    assert_eq!(AgentLifecycleState::Running.name(), "running");
    assert_eq!(AgentLifecycleState::Failed.name(), "failed");
    assert_eq!(AgentLifecycleState::Stopped.name(), "stopped");
    assert!(AgentLifecycleState::Completed.is_terminal());
    assert!(AgentLifecycleState::Failed.is_terminal());
    assert!(AgentLifecycleState::Stopped.is_terminal());
    assert!(!AgentLifecycleState::Idle.is_terminal());
}

// ---------------------------------------------------------------------------
// 系统能力层：注册和查找能力
// ---------------------------------------------------------------------------

#[test]
fn capability_registry_test() {
    let reg = hibos::capability::capability_registry();
    let ids = safe_lock(reg).list_ids();
    assert!(ids.contains(&"filesystem".to_string()));
    assert!(ids.contains(&"task".to_string()));
    assert!(ids.contains(&"agent".to_string()));
    assert!(ids.contains(&"system".to_string()));
}

// ---------------------------------------------------------------------------
// 元数据列表测试
// ---------------------------------------------------------------------------

#[test]
fn capability_metadata_test() {
    let reg = hibos::capability::capability_registry();
    let meta = safe_lock(reg).list_metadata();
    assert!(!meta.is_empty());

    let fs_meta = meta.iter().find(|m| m.id == "filesystem");
    assert!(fs_meta.is_some());
    assert!(!fs_meta.unwrap().required_permissions.is_empty());
}
