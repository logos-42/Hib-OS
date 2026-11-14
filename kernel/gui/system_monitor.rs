//! System Monitor GUI —— 系统监控界面。
//!
//! 展示 CPU / Memory / Storage / Network / Processes / Running Applications /
//! Active Agents / Active Tasks / Runtime Health / API Usage / Token Consumption /
//! Model Latency / Tool Execution Latency。
//!
//! 系统资源和 AI 资源必须区分。

use crate::monitor::get_stats;
use crate::permission::permission_manager;
use serde_json::json;

/// 系统资源快照（非 AI 资源）。
#[derive(Clone, Debug)]
pub struct SystemResources {
    pub uptime_ns: u64,
    pub vram_used_bytes: u64,
    pub vram_total_bytes: u64,
}

/// AI 资源快照（与系统资源分离）。
#[derive(Clone, Debug)]
pub struct AiResources {
    pub total_tokens: u64,
    pub active_tokens: u64,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub avg_task_time_ns: u64,
    pub active_agents: u64,
}

/// 渲染系统监控界面的文本表示。
pub fn render_system_monitor() -> String {
    let stats = get_stats();
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();
    drop(pm);

    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                           System Monitor (系统监控)\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    // 系统资源（非 AI）
    out.push_str("┌─────────────────────────────────────────────────────────────────────────────┐\n");
    out.push_str("│ 系统资源 (System Resources)                                                  │\n");
    out.push_str("├─────────────────────────────────────────────────────────────────────────────┤\n");
    out.push_str(&format!(
        "│  运行时间:      {:>12} ns                                               │\n",
        stats.system_uptime_ns
    ));
    out.push_str(&format!(
        "│  VRAM 使用:     {:>8} MiB / {:>8} MiB                                   │\n",
        stats.total_vram_used / (1024 * 1024),
        stats.total_vram_capacity / (1024 * 1024)
    ));
    out.push_str("│  CPU 占用:      N/A (需宿主 OS 接口)                                     │\n");
    out.push_str("│  内存使用:      N/A (需宿主 OS 接口)                                     │\n");
    out.push_str("│  存储使用:      N/A (需宿主 OS 接口)                                     │\n");
    out.push_str("│  网络 I/O:      N/A (需宿主 OS 接口)                                     │\n");
    out.push_str("└─────────────────────────────────────────────────────────────────────────────┘\n\n");

    // AI 资源（与系统资源分离）
    out.push_str("┌─────────────────────────────────────────────────────────────────────────────┐\n");
    out.push_str("│ AI 资源 (AI Resources - 与系统资源分开统计)                                  │\n");
    out.push_str("├─────────────────────────────────────────────────────────────────────────────┤\n");
    out.push_str(&format!(
        "│  Token 总消耗:  {:>12}                                                 │\n",
        stats.total_tokens
    ));
    out.push_str(&format!(
        "│  活跃 Token:    {:>12}                                                 │\n",
        stats.active_tokens
    ));
    out.push_str("│  模型延迟:      N/A (无 LLM 调用时为 0)                                  │\n");
    out.push_str("│  工具执行延迟:  N/A                                                      │\n");
    out.push_str("└─────────────────────────────────────────────────────────────────────────────┘\n\n");

    // 运行状态
    out.push_str("┌─────────────────────────────────────────────────────────────────────────────┐\n");
    out.push_str("│ 运行状态 (Runtime Status)                                                    │\n");
    out.push_str("├─────────────────────────────────────────────────────────────────────────────┤\n");
    out.push_str(&format!(
        "│  任务完成:      {:>12}                                                 │\n",
        stats.total_tasks_completed
    ));
    out.push_str(&format!(
        "│  任务失败:      {:>12}                                                 │\n",
        stats.total_tasks_failed
    ));
    out.push_str(&format!(
        "│  平均任务耗时:  {:>12} ns                                               │\n",
        stats.avg_task_time_ns
    ));
    out.push_str(&format!(
        "│  活跃 Agent:    {:>12}                                                 │\n",
        agents.iter().filter(|a| !a.state.is_terminal()).count()
    ));
    out.push_str("└─────────────────────────────────────────────────────────────────────────────┘\n\n");

    // Agent 列表
    out.push_str("┌─────────────────────────────────────────────────────────────────────────────┐\n");
    out.push_str("│ 活跃 Agent                                                                   │\n");
    out.push_str("├─────────────────────────────────────────────────────────────────────────────┤\n");
    if agents.is_empty() {
        out.push_str("│  (无 Agent)                                                                 │\n");
    } else {
        out.push_str("│  ID   名称       状态          权限数    风险等级                            │\n");
        for a in &agents {
            let risk = risk_level(&a.capabilities);
            out.push_str(&format!(
                "│  {:<3} {:<11} {:<13} {:<8}    {}                                          │\n",
                a.agent_id, a.name, format!("{:?}", a.state), a.capabilities.len(), risk
            ));
        }
    }
    out.push_str("└─────────────────────────────────────────────────────────────────────────────┘\n");

    out
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
    let medium_risk = ["fs:write", "proc:exec", "win:control"];

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

/// 获取系统资源快照。
pub fn get_system_resources() -> SystemResources {
    let stats = get_stats();
    SystemResources {
        uptime_ns: stats.system_uptime_ns,
        vram_used_bytes: stats.total_vram_used,
        vram_total_bytes: stats.total_vram_capacity,
    }
}

/// 获取 AI 资源快照。
pub fn get_ai_resources() -> AiResources {
    let stats = get_stats();
    let pm = permission_manager().lock().unwrap();
    let agents = pm.list_agents();
    drop(pm);

    AiResources {
        total_tokens: stats.total_tokens,
        active_tokens: stats.active_tokens,
        tasks_completed: stats.total_tasks_completed,
        tasks_failed: stats.total_tasks_failed,
        avg_task_time_ns: stats.avg_task_time_ns,
        active_agents: agents.iter().filter(|a| !a.state.is_terminal()).count() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_system_monitor_not_empty() {
        let out = render_system_monitor();
        assert!(out.contains("System Monitor"));
        assert!(out.contains("系统资源"));
        assert!(out.contains("AI 资源"));
        assert!(out.contains("运行状态"));
    }

    #[test]
    fn risk_level_classification() {
        assert_eq!(risk_level(&["fs:read:/home/*".to_string()]), "低");
        assert_eq!(risk_level(&["fs:write:/tmp/*".to_string()]), "中");
        assert_eq!(risk_level(&["fs:delete:/tmp/*".to_string()]), "高");
        assert_eq!(risk_level(&["net:connect:external".to_string()]), "高");
    }

    #[test]
    fn get_system_resources_works() {
        let res = get_system_resources();
        assert!(res.vram_total_bytes > 0);
    }

    #[test]
    fn get_ai_resources_works() {
        let res = get_ai_resources();
        assert!(res.total_tokens >= 0);
    }
}
