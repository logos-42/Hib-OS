//! Notification Center GUI —— 通知中心。
//!
//! 展示系统通知、权限请求、Agent 事件、任务状态变更等。
//! 支持订阅事件总线，实时推送而非轮询。

use crate::event::{publish_event, EventSource, SystemEvent, SystemEventType};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// 通知条目。
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u64,
    pub timestamp: u64,
    pub title: String,
    pub body: String,
    pub r#type: NotificationType,
    pub read: bool,
}

/// 通知类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationType {
    Info,
    Warning,
    Error,
    Success,
    PermissionRequest,
    TaskCompleted,
    TaskFailed,
    AgentStarted,
    AgentStopped,
}

impl NotificationType {
    pub fn label(self) -> &'static str {
        match self {
            NotificationType::Info => "信息",
            NotificationType::Warning => "警告",
            NotificationType::Error => "错误",
            NotificationType::Success => "成功",
            NotificationType::PermissionRequest => "权限请求",
            NotificationType::TaskCompleted => "任务完成",
            NotificationType::TaskFailed => "任务失败",
            NotificationType::AgentStarted => "Agent 启动",
            NotificationType::AgentStopped => "Agent 停止",
        }
    }
}

/// 通知中心（内存，供 GUI 和事件总线使用）。
pub struct NotificationCenter {
    notifications: VecDeque<Notification>,
    next_id: u64,
    max_notifications: usize,
}

impl NotificationCenter {
    pub fn new(max_notifications: usize) -> Self {
        Self {
            notifications: VecDeque::new(),
            next_id: 1,
            max_notifications,
        }
    }

    pub fn push(&mut self, notification: Notification) {
        if self.notifications.len() >= self.max_notifications {
            self.notifications.pop_front();
        }
        self.notifications.push_back(notification);
    }

    pub fn list(&self) -> Vec<Notification> {
        self.notifications.iter().rev().cloned().collect()
    }

    pub fn mark_read(&mut self, id: u64) -> bool {
        if let Some(n) = self.notifications.iter_mut().find(|n| n.id == id) {
            n.read = true;
            true
        } else {
            false
        }
    }

    pub fn unread_count(&self) -> u64 {
        self.notifications.iter().filter(|n| !n.read).count() as u64
    }

    pub fn clear(&mut self) {
        self.notifications.clear();
    }
}

static NOTIFICATION_CENTER: OnceLock<Mutex<NotificationCenter>> = OnceLock::new();

fn notification_center() -> &'static Mutex<NotificationCenter> {
    NOTIFICATION_CENTER.get_or_init(|| Mutex::new(NotificationCenter::new(500)))
}

/// 发送一条通知。
pub fn notify(title: &str, body: &str, r#type: NotificationType) {
    let nc = notification_center();
    let mut nc_guard = nc.lock().unwrap();
    let id = nc_guard.next_id;
    nc_guard.next_id += 1;
    nc_guard.push(Notification {
        id,
        timestamp: crate::monitor::get_time_ns(),
        title: title.to_string(),
        body: body.to_string(),
        r#type,
        read: false,
    });
    drop(nc_guard);

    // 同时发布事件，供订阅者（GUI、Shell、Agent）接收
    publish_event(
        EventSource::System,
        match r#type {
            NotificationType::TaskCompleted => SystemEventType::TaskCompleted,
            NotificationType::TaskFailed => SystemEventType::TaskFailed,
            NotificationType::AgentStarted => SystemEventType::AgentStarted,
            NotificationType::AgentStopped => SystemEventType::AgentStopped,
            _ => SystemEventType::Custom { name: "notification" },
        },
        None,
        json!({
            "notification_id": id,
            "title": title,
            "body": body,
            "type": format!("{:?}", r#type),
        }),
    );
}

/// 从系统事件创建通知（事件总线回调）。
pub fn on_system_event(event: &SystemEvent) {
    let nc = notification_center();
    let mut nc_guard = nc.lock().unwrap();
    let id = nc_guard.next_id;
    nc_guard.next_id += 1;

    let (title, body, r#type) = match event.event_type {
        SystemEventType::AgentStarted => {
            let agent_id = event.payload.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
            (format!("Agent 启动"), format!("Agent #{} 已启动", agent_id), NotificationType::AgentStarted)
        }
        SystemEventType::AgentStopped => {
            let agent_id = event.payload.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
            (format!("Agent 停止"), format!("Agent #{} 已停止", agent_id), NotificationType::AgentStopped)
        }
        SystemEventType::TaskCompleted => {
            let task_id = event.payload.get("task_id").and_then(|v| v.as_u64()).unwrap_or(0);
            (format!("任务完成"), format!("任务 #{} 已完成", task_id), NotificationType::TaskCompleted)
        }
        SystemEventType::TaskFailed => {
            let task_id = event.payload.get("task_id").and_then(|v| v.as_u64()).unwrap_or(0);
            (format!("任务失败"), format!("任务 #{} 失败", task_id), NotificationType::TaskFailed)
        }
        SystemEventType::PermissionRequested => {
            (format!("权限请求"), format!("有新的权限请求需要批准"), NotificationType::PermissionRequest)
        }
        _ => {
            return; // 不创建通知
        }
    };

    nc_guard.push(Notification {
        id,
        timestamp: event.timestamp,
        title,
        body,
        r#type,
        read: false,
    });
}

/// 渲染通知中心的文本表示。
pub fn render_notification_center() -> String {
    let nc = notification_center();
    let nc_guard = nc.lock().unwrap();
    let notifications = nc_guard.list();
    let unread = nc_guard.unread_count();
    drop(nc_guard);

    let mut out = String::new();
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
    out.push_str("                         Notification Center (通知中心)\n");
    out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

    out.push_str(&format!("  未读通知: {}\n\n", unread));

    if notifications.is_empty() {
        out.push_str("  (暂无通知)\n");
        return out;
    }

    out.push_str("  ID   时间戳          类型           标题                      内容                              状态\n");
    out.push_str("  --   --------          ----           -----                     -------                       ------\n");

    for n in notifications.iter().take(20) {
        let status = if n.read { "已读" } else { "未读" };
        out.push_str(&format!(
            "  {:<3} {:<16} {:<14} {:<26} {:<32} {}\n",
            n.id, n.timestamp, n.r#type.label(), n.title, n.body, status
        ));
    }

    if notifications.len() > 20 {
        out.push_str(&format!(
            "\n  ... 还有 {} 条通知未显示\n",
            notifications.len() - 20
        ));
    }

    out.push_str("\n  可用操作:\n");
    out.push_str("    标记已读、清空通知、订阅事件总线\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_and_list() {
        let nc = notification_center();
        let mut guard = nc.lock().unwrap();
        guard.next_id = 1; // 重置以便测试
        drop(guard);

        notify("测试标题", "测试内容", NotificationType::Info);
        notify("权限请求", "需要批准", NotificationType::PermissionRequest);

        let out = render_notification_center();
        assert!(out.contains("Notification Center"));
        assert!(out.contains("测试标题"));
    }

    #[test]
    fn unread_count() {
        let nc = notification_center();
        let mut guard = nc.lock().unwrap();
        guard.next_id = 1;
        guard.notifications.clear();
        drop(guard);

        notify("a", "b", NotificationType::Info);
        notify("c", "d", NotificationType::Warning);

        let nc = notification_center();
        assert_eq!(nc.lock().unwrap().unread_count(), 2);

        // 标记第一条为已读
        let nc = notification_center();
        nc.lock().unwrap().mark_read(1);
        assert_eq!(nc.lock().unwrap().unread_count(), 1);
    }

    #[test]
    fn max_notifications_limit() {
        let nc = notification_center();
        let mut guard = nc.lock().unwrap();
        guard.max_notifications = 3;
        guard.notifications.clear();
        guard.next_id = 1;
        drop(guard);

        notify("1", "b", NotificationType::Info);
        notify("2", "b", NotificationType::Info);
        notify("3", "b", NotificationType::Info);
        notify("4", "b", NotificationType::Info); // 应该挤掉第一条

        let nc = notification_center();
        let list = nc.lock().unwrap().list();
        assert_eq!(list.len(), 3);
        assert!(!list.iter().any(|n| n.title == "1"));
    }
}
