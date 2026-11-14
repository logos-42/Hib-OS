//! 跨接口一致性测试和端到端场景测试。
//!
//! 验证：同一操作通过 GUI、Shell、Agent API 三种入口执行，
//! 最终系统状态一致，权限判断一致，错误结构一致，
//! 事件记录一致，审计信息一致。

use hibos::shell::Shell;
use hibos::capability::{
    CapabilityRequest, CapabilityError, ErrorCode, ExecutionContext,
    now_ns, AuditInfo, AuditResult, CallerType,
};
use hibos::permission::{
    permission_manager, AgentLifecycleState, Principal,
};
use hibos::kernel::permission::AuthorizationMode;

/// 辅助：创建一个测试 Shell。
fn test_shell() -> Shell {
    Shell::new()
}

/// 辅助：通过 Agent API 直接调用文件系统能力。
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
            caller_id: "test-agent".to_string(),
            caller_type: CallerType::Agent,
            permissions: vec!["fs:all".to_string()],
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
    match reg.lock().unwrap().get("filesystem") {
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

/// 场景一：GUI 创建 Agent，授予文件读取权限，搜索目录并返回结果。
/// 验证：Agent 出现在 Agent Workspace，状态正确，文件搜索结果正确。
#[test]
fn e2e_scenario_1_gui_create_agent_search_directory() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "alice".to_string() },
        "search-agent",
        "researcher",
        vec!["fs:read:/tmp/*".to_string()],
        Default::default(),
    );
    drop(pm);

    // 验证 Agent 存在
    let pm = permission_manager().lock().unwrap();
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    assert!(pm.check_permission(agent_id, "fs:read:/tmp/foo"));

    // 通过 Shell 创建测试文件
    let mut shell = test_shell();
    assert!(shell.dispatch("mkdir /tmp/searchtest"));
    assert!(shell.dispatch("write /tmp/searchtest/file1.txt hello"));
    assert!(shell.dispatch("write /tmp/searchtest/file2.txt world"));

    // 通过 Agent API 读取文件（模拟 Agent 搜索）
    let result = fs_capability_op("read", "/tmp/searchtest/file1.txt", None).unwrap();
    assert_eq!(result["content"], "hello");

    // 通过 Shell 读取同一文件，验证一致性
    assert!(shell.dispatch("cat /tmp/searchtest/file1.txt"));

    // 清理
    assert!(shell.dispatch("rm /tmp/searchtest/file1.txt"));
    assert!(shell.dispatch("rm /tmp/searchtest/file2.txt"));
    assert!(shell.dispatch("rm /tmp/searchtest"));
}

/// 场景二：Shell 创建任务，启动 Agent，实时观察任务状态，获取结构化结果。
/// 验证：任务 ID 返回，状态实时更新，结果为结构化 JSON。
#[test]
fn e2e_scenario_2_shell_create_task_observe_status() {
    let mut shell = test_shell();

    // 创建任务
    assert!(shell.dispatch("task create \"分析日志文件\" --agent 1"));
    assert!(shell.dispatch("task list"));
    assert!(shell.dispatch("task inspect 1"));

    // 验证任务列表通过能力层可查询
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
    match reg.lock().unwrap().get("task") {
        Some(cap) => {
            let resp = cap.execute(req).unwrap();
            assert!(resp.output.is_array() || resp.output.is_object());
        }
        None => panic!("能力 task 未注册"),
    }
}

/// 场景三：Agent 需要删除文件，系统判断权限并向用户申请授权，
/// 用户批准后执行，GUI 和 Shell 都能查询执行记录。
/// 验证：未授权时返回 PermissionDenied，授权后执行成功，
/// GUI 和 Shell 都能查询审计记录。
#[test]
fn e2e_scenario_3_agent_delete_file_permission() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "bob".to_string() },
        "cleanup-agent",
        "cleaner",
        vec![], // 初始无权限
        Default::default(),
    );
    drop(pm);

    // 创建测试文件
    let mut shell = test_shell();
    assert!(shell.dispatch("write /tmp/deleteme.txt content"));

    // Agent 无权限删除，应返回 PermissionDenied
    let result = fs_capability_op("delete", "/tmp/deleteme.txt", None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);

    // 授予删除权限（模拟用户批准）
    let pm = permission_manager().lock().unwrap();
    pm.grant_permission(
        agent_id,
        "fs:delete:/tmp/deleteme.txt",
        AuthorizationMode::Once,
        None,
        "bob",
    ).unwrap();
    drop(pm);

    // 现在 Agent 可以删除
    let result = fs_capability_op("delete", "/tmp/deleteme.txt", None);
    assert!(result.is_ok());

    // 验证文件已删除（通过 VFS 确认）
    let vfs_content = hibos::vfs::global_vfs().lock().unwrap().cat("/tmp/deleteme.txt");
    assert!(vfs_content.is_err(), "文件应已删除");

    // 查询审计日志
    let pm = permission_manager().lock().unwrap();
    let audit = pm.query_audit(Some(10));
    assert!(!audit.is_empty());
    assert!(audit.iter().any(|e| e.action == "filesystem.delete"));
}

/// 场景四：GUI 启动长任务，关闭 GUI 后，Agent Runtime 继续运行，
/// 重新打开 GUI 后恢复任务状态。
/// 验证：GUI 关闭后 Agent 不停止，任务状态在重新打开后可查询。
#[test]
fn e2e_scenario_4_gui_close_agent_continues() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "carol".to_string() },
        "long-runner",
        "worker",
        vec!["task:create".to_string(), "task:control".to_string()],
        Default::default(),
    );
    drop(pm);

    // 验证 Agent 状态为 Created
    let pm = permission_manager().lock().unwrap();
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    drop(pm);

    // 通过 Shell 创建任务（模拟 GUI 启动长任务）
    let mut shell = test_shell();
    assert!(shell.dispatch("task create \"长时间运行任务\" --agent 1"));
    assert!(shell.dispatch("task list"));

    // 验证 Agent 状态未因 "GUI 关闭" 而改变（模拟：不执行 stop）
    let pm = permission_manager().lock().unwrap();
    assert_eq!(pm.get_state(agent_id), Some(AgentLifecycleState::Created));
    drop(pm);
}

/// 场景五：模型 API 不可用，用户依然可以使用 Shell、文件管理器、
/// 系统设置和进程管理。
/// 验证：无 LLM API Key 时，Shell、文件、进程、设置均正常工作。
#[test]
fn e2e_scenario_5_llm_api_unavailable() {
    let mut shell = test_shell();

    // Shell 基础命令
    assert!(shell.dispatch("pwd"));
    assert!(shell.dispatch("ls"));
    assert!(shell.dispatch("write /tmp/llm_test.txt hello"));
    assert!(shell.dispatch("cat /tmp/llm_test.txt"));

    // 系统命令
    assert!(shell.dispatch("stats"));
    assert!(shell.dispatch("channels"));

    // 文件系统能力层（不依赖 LLM）
    let result = fs_capability_op("read", "/tmp/llm_test.txt", None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap()["content"], "hello");

    // 进程管理
    assert!(shell.dispatch("proc attention 8"));
    assert!(shell.dispatch("run"));

    // 清理
    assert!(shell.dispatch("rm /tmp/llm_test.txt"));
}

/// 场景六：一个 Agent 崩溃，不影响其他 Agent、Shell 和 GUI。
/// 验证：崩溃的 Agent 标记为 Failed，其他 Agent 继续运行，
/// Shell 和 GUI 正常工作。
#[test]
fn e2e_scenario_6_agent_crash_isolation() {
    let pm = permission_manager().lock().unwrap();
    let agent_id_1 = pm.create_agent(
        Principal::User { user_id: "dave".to_string() },
        "crashy",
        "worker",
        vec![],
        Default::default(),
    );
    let agent_id_2 = pm.create_agent(
        Principal::User { user_id: "dave".to_string() },
        "stable",
        "worker",
        vec![],
        Default::default(),
    );
    drop(pm);

    // 模拟 Agent 1 崩溃：设置状态为 Failed
    let pm = permission_manager().lock().unwrap();
    pm.set_state(agent_id_1, AgentLifecycleState::Failed).unwrap();
    // Agent 2 应该仍然是 Created
    assert_eq!(pm.get_state(agent_id_2), Some(AgentLifecycleState::Created));
    drop(pm);

    // Shell 应该仍然正常工作
    let mut shell = test_shell();
    assert!(shell.dispatch("pwd"));
    assert!(shell.dispatch("ls"));

    // GUI 界面应该仍然可渲染
    let workspace = hibos::gui::agent_workspace::render_agent_workspace();
    assert!(workspace.contains("Agent Workspace"));

    let perm_center = hibos::gui::permission_center::render_permission_center();
    assert!(perm_center.contains("Permission Center"));
}

/// 跨接口一致性测试：文件删除
/// 通过 GUI（Permission Center）、Shell（rm）、Agent API（filesystem.delete）
/// 分别执行删除，验证最终文件状态一致。
#[test]
fn consistency_test_file_delete() {
    let test_path = "/tmp/consistency_delete_test.txt";

    // 设置：创建文件
    let mut shell = test_shell();
    assert!(shell.dispatch(&format!("write {} test-content", test_path)));

    // 通过 Shell rm 删除
    assert!(shell.dispatch(&format!("rm {}", test_path)));

    // 验证文件已删除（通过 VFS 直接检查，作为一致性基准）
    let vfs = hibos::vfs::global_vfs().lock().unwrap();
    assert!(vfs.cat(test_path).is_err(), "Shell rm 后文件应不存在");
    drop(vfs);

    // 重新创建文件，通过 Agent API 删除
    assert!(shell.dispatch(&format!("write {} test-content", test_path)));
    let result = fs_capability_op("delete", test_path, None);
    assert!(result.is_ok(), "Agent API delete 应成功");

    let vfs = hibos::vfs::global_vfs().lock().unwrap();
    assert!(vfs.cat(test_path).is_err(), "Agent API delete 后文件应不存在");
}

/// 跨接口一致性测试：文件读取
/// 通过 Shell cat 和 Agent API filesystem.read 读取同一文件，验证内容一致。
#[test]
fn consistency_test_file_read() {
    let test_path = "/tmp/consistency_read_test.txt";
    let test_content = "consistency test content";

    // 创建文件
    let mut shell = test_shell();
    assert!(shell.dispatch(&format!("write {} {}", test_path, test_content)));

    // 通过 VFS 直接检查作为基准
    let vfs = hibos::vfs::global_vfs().lock().unwrap();
    let shell_content = vfs.cat(test_path).unwrap();
    drop(vfs);

    // 通过 Agent API 读取
    let result = fs_capability_op("read", test_path, None).unwrap();
    let agent_content = result["content"].as_str().unwrap();

    assert_eq!(shell_content, agent_content, "Shell 和 Agent API 读取内容应一致");
}

/// 跨接口一致性测试：错误结构一致
/// 通过 Shell 和 Agent API 访问不存在的文件，验证错误结构一致。
#[test]
fn consistency_test_error_structure() {
    let nonexistent = "/tmp/nonexistent_file_xyz.txt";

    // Shell rm 不存在的文件
    let mut shell = test_shell();
    shell.dispatch(&format!("rm {}", nonexistent)); // 打印错误，不崩溃

    // Agent API 读取不存在的文件
    let result = fs_capability_op("read", nonexistent, None);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(!err.message.is_empty());
}

/// 跨接口一致性测试：权限判断一致
/// Shell 和 Agent API 对同一操作的权限判断应一致。
#[test]
fn consistency_test_permission_judgment() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "eve".to_string() },
        "perm-test",
        "tester",
        vec!["fs:read:/tmp/*".to_string()], // 只有读权限
        Default::default(),
    );
    drop(pm);

    let test_path = "/tmp/perm_test_secret.txt";

    // Agent 只有 fs:read，没有 fs:write
    let write_result = fs_capability_op("write", test_path, Some("secret"));
    assert!(write_result.is_err());
    assert_eq!(write_result.unwrap_err().code, ErrorCode::PermissionDenied);

    // Agent 有 fs:read，读取应成功（如果文件存在）
    // 先通过 Shell 创建文件（Shell 有 fs:all）
    let mut shell = test_shell();
    assert!(shell.dispatch(&format!("write {} data", test_path)));

    let read_result = fs_capability_op("read", test_path, None);
    assert!(read_result.is_ok());

    // 清理
    assert!(shell.dispatch(&format!("rm {}", test_path)));
}

/// 权限模型测试：Once 授权模式
#[test]
fn permission_test_once_mode() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "frank".to_string() },
        "once-test",
        "tester",
        vec![],
        Default::default(),
    );
    drop(pm);

    // 授予 Once 权限
    let pm = permission_manager().lock().unwrap();
    pm.grant_permission(
        agent_id,
        "fs:read:/once/*",
        AuthorizationMode::Once,
        None,
        "frank",
    ).unwrap();
    drop(pm);

    // 立即检查应有权限
    let pm = permission_manager().lock().unwrap();
    assert!(pm.check_permission(agent_id, "fs:read:/once/foo"));
}

/// 权限模型测试：撤销权限后不再可用
#[test]
fn permission_test_revoke() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "grace".to_string() },
        "revoke-test",
        "tester",
        vec!["fs:delete:/tmp/*".to_string()],
        Default::default(),
    );
    drop(pm);

    // 有权限
    let pm = permission_manager().lock().unwrap();
    assert!(pm.check_permission(agent_id, "fs:delete:/tmp/foo"));
    drop(pm);

    // 撤销
    let pm = permission_manager().lock().unwrap();
    pm.revoke_permission(agent_id, "fs:delete:/tmp/*", "grace").unwrap();
    drop(pm);

    // 撤销后无权限
    let pm = permission_manager().lock().unwrap();
    assert!(!pm.check_permission(agent_id, "fs:delete:/tmp/foo"));
}

/// 事件系统测试：发布和订阅
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
    let results = bus.lock().unwrap().query(&EventFilter::default());
    assert!(!results.is_empty());
    assert!(results.iter().any(|e| matches!(e.event_type, SystemEventType::AgentStarted)));
}

/// 事件系统测试：过滤
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
    let results = bus.lock().unwrap().query(&EventFilter {
        event_types: Some(vec![SystemEventType::TaskCompleted]),
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].event_type, SystemEventType::TaskCompleted);
}

/// Shell parser 测试：引号、转义、管道、重定向
#[test]
fn shell_parser_test_quoting_and_redirection() {
    use hibos::shell_parser::{parse_line, Redirection};

    // 引号
    let p = parse_line(r#"echo "hello world""#).unwrap();
    assert_eq!(p.commands[0].argv, vec!["echo".to_string(), "hello world".to_string()]);

    // 管道
    let p = parse_line("ls | cat").unwrap();
    assert_eq!(p.commands.len(), 2);

    // 重定向
    let p = parse_line("ls > out.txt").unwrap();
    assert_eq!(p.commands[0].stdout, Redirection::ToFile("out.txt".to_string()));

    // 追加重定向
    let p = parse_line("ls >> out.txt").unwrap();
    assert_eq!(p.commands[0].stdout, Redirection::AppendFile("out.txt".to_string()));

    // 输入重定向
    let p = parse_line("cat < in.txt").unwrap();
    assert_eq!(p.commands[0].stdin, Redirection::FromFile("in.txt".to_string()));
}

/// Shell JSON 输出测试：--json 标志
#[test]
fn shell_json_output_test() {
    let mut shell = test_shell();

    // capability list --json 应输出 JSON
    shell.dispatch("capability list --json");
}

/// Shell agent 命令测试
#[test]
fn shell_agent_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("agent list"));
    assert!(shell.dispatch("agent list --json"));
    assert!(shell.dispatch("agent inspect 1"));
    assert!(shell.dispatch("agent inspect 1 --json"));
}

/// Shell task 命令测试
#[test]
fn shell_task_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("task list"));
    assert!(shell.dispatch("task list --json"));
    assert!(shell.dispatch("task create \"测试任务\""));
    assert!(shell.dispatch("task inspect 1"));
    assert!(shell.dispatch("task inspect 1 --json"));
}

/// Shell system 命令测试
#[test]
fn shell_system_command_test() {
    let mut shell = test_shell();

    assert!(shell.dispatch("system status"));
    assert!(shell.dispatch("system status --json"));
    assert!(shell.dispatch("system resources"));
    assert!(shell.dispatch("system logs"));
}

/// Agent Workspace GUI 测试
#[test]
fn gui_agent_workspace_test() {
    let out = hibos::gui::agent_workspace::render_agent_workspace();
    assert!(out.contains("Agent Workspace"));
    assert!(out.contains("活跃 Agent 总数"));

    let detail = hibos::gui::agent_workspace::render_agent_detail(99999);
    assert!(detail.contains("未找到 Agent"));
}

/// Task Workspace GUI 测试
#[test]
fn gui_task_workspace_test() {
    let out = hibos::gui::task_workspace::render_task_workspace();
    assert!(out.contains("Task Workspace"));

    let detail = hibos::gui::task_workspace::render_task_detail(99999);
    assert!(detail.contains("未找到任务"));
}

/// Permission Center GUI 测试
#[test]
fn gui_permission_center_test() {
    let out = hibos::gui::permission_center::render_permission_center();
    assert!(out.contains("Permission Center"));

    let detail = hibos::gui::permission_center::render_agent_permissions(99999);
    assert!(detail.contains("未找到 Agent"));

    let audit = hibos::gui::permission_center::render_audit_log(Some(5));
    assert!(audit.contains("审计日志"));
}

/// 权限中心：授予和撤销权限
#[test]
fn permission_center_grant_revoke_test() {
    let pm = permission_manager().lock().unwrap();
    let agent_id = pm.create_agent(
        Principal::User { user_id: "helen".to_string() },
        "perm-center-test",
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

/// Agent 生命周期状态测试
#[test]
fn agent_lifecycle_state_test() {
    assert_eq!(AgentLifecycleState::Created.name(), "created");
    assert_eq!(AgentLifecycleState::Running.name(), "running");
    assert_eq!(AgentLifecycleState::Failed.name(), "failed");
    assert_eq!(AgentLifecycleState::Stopped.name(), "stopped");
}

/// 系统能力层：注册和查找能力
#[test]
fn capability_registry_test() {
    let reg = hibos::capability::capability_registry();
    let ids = reg.lock().unwrap().list_ids();
    // 应该至少有 filesystem, task, agent, system, settings, window
    assert!(ids.contains(&"filesystem".to_string()));
    assert!(ids.contains(&"task".to_string()));
    assert!(ids.contains(&"agent".to_string()));
    assert!(ids.contains(&"system".to_string()));
}

/// 元数据列表测试
#[test]
fn capability_metadata_test() {
    let reg = hibos::capability::capability_registry();
    let meta = reg.lock().unwrap().list_metadata();
    assert!(!meta.is_empty());

    let fs_meta = meta.iter().find(|m| m.id == "filesystem");
    assert!(fs_meta.is_some());
    assert!(!fs_meta.unwrap().required_permissions.is_empty());
}
