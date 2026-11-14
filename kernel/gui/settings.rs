//! Settings GUI —— 系统设置界面。
//!
//! 展示系统设置并允许通过统一能力层（settings.read / settings.update）读取和更新。
//! 所有设置与真实系统配置关联。

use crate::capability::{
    CapabilityRequest, ExecutionContext, now_ns, AuditInfo, AuditResult, CallerType,
};
use serde_json::json;

/// 设置类别。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsCategory {
    Appearance,
    Display,
    Keyboard,
    Mouse,
    Terminal,
    Network,
    Storage,
    Applications,
    Agents,
    Models,
    Providers,
    ApiConfiguration,
    Permissions,
    Privacy,
    Security,
    Notifications,
    SystemUpdates,
    DeveloperOptions,
}

impl SettingsCategory {
    pub fn key_prefix(&self) -> &'static str {
        match self {
            SettingsCategory::Appearance => "appearance",
            SettingsCategory::Display => "display",
            SettingsCategory::Keyboard => "keyboard",
            SettingsCategory::Mouse => "mouse",
            SettingsCategory::Terminal => "terminal",
            SettingsCategory::Network => "network",
            SettingsCategory::Storage => "storage",
            SettingsCategory::Applications => "applications",
            SettingsCategory::Agents => "agents",
            SettingsCategory::Models => "models",
            SettingsCategory::Providers => "providers",
            SettingsCategory::ApiConfiguration => "api",
            SettingsCategory::Permissions => "permissions",
            SettingsCategory::Privacy => "privacy",
            SettingsCategory::Security => "security",
            SettingsCategory::Notifications => "notifications",
            SettingsCategory::SystemUpdates => "system_updates",
            SettingsCategory::DeveloperOptions => "developer",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SettingsCategory::Appearance => "外观",
            SettingsCategory::Display => "显示",
            SettingsCategory::Keyboard => "键盘",
            SettingsCategory::Mouse => "鼠标",
            SettingsCategory::Terminal => "终端",
            SettingsCategory::Network => "网络",
            SettingsCategory::Storage => "存储",
            SettingsCategory::Applications => "应用程序",
            SettingsCategory::Agents => "智能体",
            SettingsCategory::Models => "模型",
            SettingsCategory::Providers => "提供商",
            SettingsCategory::ApiConfiguration => "API 配置",
            SettingsCategory::Permissions => "权限",
            SettingsCategory::Privacy => "隐私",
            SettingsCategory::Security => "安全",
            SettingsCategory::Notifications => "通知",
            SettingsCategory::SystemUpdates => "系统更新",
            SettingsCategory::DeveloperOptions => "开发者选项",
        }
    }
}

/// 渲染设置界面的文本表示。
pub fn render_settings() -> String {
    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                           Settings (系统设置)\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    out.push_str("  设置类别:\n");
    for cat in [
        SettingsCategory::Appearance,
        SettingsCategory::Display,
        SettingsCategory::Keyboard,
        SettingsCategory::Mouse,
        SettingsCategory::Terminal,
        SettingsCategory::Network,
        SettingsCategory::Storage,
        SettingsCategory::Applications,
        SettingsCategory::Agents,
        SettingsCategory::Models,
        SettingsCategory::Providers,
        SettingsCategory::ApiConfiguration,
        SettingsCategory::Permissions,
        SettingsCategory::Privacy,
        SettingsCategory::Security,
        SettingsCategory::Notifications,
        SettingsCategory::SystemUpdates,
        SettingsCategory::DeveloperOptions,
    ] {
        out.push_str(&format!(
            "    {}  ({})\n",
            cat.label(),
            cat.key_prefix()
        ));
    }

    out.push_str("\n  可用操作:\n");
    out.push_str("    settings read <key>     - 读取设置\n");
    out.push_str("    settings update <key> <值>  - 更新设置\n");
    out.push_str("    system settings         - 查看系统设置状态\n");

    out
}

/// 读取设置（通过统一能力层）。
pub fn settings_read(key: &str) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "settings".to_string(),
        input: json!({"op": "read", "key": key}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["settings:read".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "settings.read".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.read().unwrap_or_else(|e| e.into_inner()).get("settings") {
        Some(cap) => match cap.execute(req) {
            Ok(resp) => {
                let value = resp.output.get("value").cloned().unwrap_or(json!(null));
                Ok(format!("{}: {}", key, value))
            }
            Err(e) => Err(format!("读取失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 settings 未注册".to_string()),
    }
}

/// 更新设置（通过统一能力层）。
pub fn settings_update(key: &str, value: serde_json::Value) -> Result<String, String> {
    let req = CapabilityRequest {
        id: "settings".to_string(),
        input: json!({"op": "update", "key": key, "value": value}),
        context: ExecutionContext {
            caller_id: "gui".to_string(),
            caller_type: CallerType::Gui,
            permissions: vec!["settings:write".to_string()],
            resource_quota: Default::default(),
        },
        timeout_ns: None,
        cancellation_token: None,
        audit: AuditInfo {
            actor: "gui".to_string(),
            action: "settings.update".to_string(),
            timestamp: now_ns(),
            result: AuditResult::Executed,
            correlation_id: None,
        },
    };
    let reg = crate::capability::capability_registry();
    match reg.read().unwrap_or_else(|e| e.into_inner()).get("settings") {
        Some(cap) => match cap.execute(req) {
            Ok(_resp) => Ok(format!("已更新设置: {} = {}", key, value)),
            Err(e) => Err(format!("更新失败: {} (code={})", e.message, e.code)),
        },
        None => Err("能力 settings 未注册".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_settings_not_empty() {
        let out = render_settings();
        assert!(out.contains("Settings"));
        assert!(out.contains("系统设置"));
        assert!(out.contains("外观"));
        assert!(out.contains("权限"));
        assert!(out.contains("开发者选项"));
    }

    #[test]
    fn settings_category_key_prefix() {
        assert_eq!(SettingsCategory::Appearance.key_prefix(), "appearance");
        assert_eq!(SettingsCategory::Security.key_prefix(), "security");
        assert_eq!(SettingsCategory::ApiConfiguration.key_prefix(), "api");
    }

    #[test]
    fn settings_category_label() {
        assert_eq!(SettingsCategory::Appearance.label(), "外观");
        assert_eq!(SettingsCategory::Security.label(), "安全");
    }
}
