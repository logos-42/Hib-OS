//! 统一事件系统 (Event System)
//!
//! 支持订阅、过滤、查询、重放（持久化时）和实时推送。
//! GUI 应订阅事件而非轮询；Shell 可查询事件；Agent 可订阅有权限的事件。

use super::capability::{now_ns, ExecutionContext};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// 事件 ID。
pub type SystemEventId = u64;

/// 订阅 ID。
pub type SubscriptionId = u64;

/// 事件来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventSource {
    Agent { agent_id: String },
    Shell { session_id: String },
    Gui { session_id: String },
    System,
}

/// 事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEventType {
    AgentStarted,
    AgentStopped,
    AgentStateChanged,
    TaskCreated,
    TaskStarted,
    TaskProgress,
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    TaskPaused,
    TaskResumed,
    ToolInvoked,
    ToolCompleted,
    ToolFailed,
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    PermissionRevoked,
    FileCreated,
    FileChanged,
    FileDeleted,
    FileMoved,
    ProcessStarted,
    ProcessExited,
    ProcessSignaled,
    WindowCreated,
    WindowClosed,
    WindowFocused,
    WindowMoved,
    WindowResized,
    SystemResourceChanged,
    Custom { name: &'static str },
}

/// 系统事件。
#[derive(Clone, Debug)]
pub struct SystemEvent {
    pub event_id: SystemEventId,
    pub timestamp: u64,
    pub source: EventSource,
    pub event_type: SystemEventType,
    pub correlation_id: Option<u64>,
    pub payload: Value,
}

/// 事件订阅过滤器。
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    pub event_types: Option<Vec<SystemEventType>>,
    pub sources: Option<Vec<EventSource>>,
    pub correlation_ids: Option<Vec<u64>>,
}

/// 事件订阅者 trait。
pub trait EventSubscriber: Send + Sync {
    fn on_event(&self, event: &SystemEvent);
}

/// 事件总线。
pub struct EventBus {
    next_id: Mutex<SystemEventId>,
    next_sub_id: Mutex<SubscriptionId>,
    subscribers: Mutex<Vec<(SubscriptionId, EventFilter, Box<dyn EventSubscriber>)>>,
    history: Mutex<VecDeque<SystemEvent>>,
    max_history: usize,
}

impl EventBus {
    pub fn new(max_history: usize) -> Self {
        Self {
            next_id: Mutex::new(1),
            next_sub_id: Mutex::new(1),
            subscribers: Mutex::new(Vec::new()),
            history: Mutex::new(VecDeque::new()),
            max_history,
        }
    }

    /// 发布事件。所有匹配的订阅者会收到事件。
    pub fn publish(&self, event: SystemEvent) {
        // 记录历史
        {
            let mut h = self.history.lock().unwrap();
            h.push_back(event.clone());
            while h.len() > self.max_history {
                h.pop_front();
            }
        }

        // 分发给订阅者
        let subs = self.subscribers.lock().unwrap();
        for (_, filter, subscriber) in subs.iter() {
            if Self::matches_filter(&event, filter) {
                subscriber.on_event(&event);
            }
        }
    }

    /// 订阅事件，返回订阅 ID。
    pub fn subscribe(
        &self,
        filter: EventFilter,
        subscriber: Box<dyn EventSubscriber>,
    ) -> SubscriptionId {
        let id = {
            let mut next = self.next_sub_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        self.subscribers
            .lock()
            .unwrap()
            .push((id, filter, subscriber));
        id
    }

    /// 取消订阅。
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let mut subs = self.subscribers.lock().unwrap();
        if let Some(pos) = subs.iter().position(|(sid, _, _)| *sid == id) {
            subs.remove(pos);
            true
        } else {
            false
        }
    }

    /// 查询历史事件。
    pub fn query(&self, filter: &EventFilter) -> Vec<SystemEvent> {
        let h = self.history.lock().unwrap();
        h.iter()
            .filter(|e| Self::matches_filter(e, filter))
            .cloned()
            .collect()
    }

    /// 重放从指定事件 ID 开始的所有事件。
    pub fn replay(&self, from_event_id: SystemEventId) -> Vec<SystemEvent> {
        let h = self.history.lock().unwrap();
        h.iter()
            .filter(|e| e.event_id >= from_event_id)
            .cloned()
            .collect()
    }

    /// 获取订阅者数量。
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    fn matches_filter(event: &SystemEvent, filter: &EventFilter) -> bool {
        if let Some(types) = &filter.event_types {
            if !types.contains(&event.event_type) {
                return false;
            }
        }
        if let Some(sources) = &filter.sources {
            if !sources.iter().any(|s| std::mem::discriminant(s) == std::mem::discriminant(&event.source)) {
                return false;
            }
        }
        if let Some(corr_ids) = &filter.correlation_ids {
            match event.correlation_id {
                Some(cid) => {
                    if !corr_ids.contains(&cid) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1000)
    }
}

/// 全局事件总线单例。
static EVENT_BUS: std::sync::OnceLock<Mutex<EventBus>> = std::sync::OnceLock::new();

/// 获取全局事件总线。
pub fn event_bus() -> &'static Mutex<EventBus> {
    EVENT_BUS.get_or_init(|| Mutex::new(EventBus::default()))
}

/// 发布一个系统事件（便捷函数）。
pub fn publish_event(
    source: EventSource,
    event_type: SystemEventType,
    correlation_id: Option<u64>,
    payload: Value,
) {
    let bus = event_bus();
    let bus_guard = bus.lock().unwrap();
    let event_id = {
        let mut next = bus_guard.next_id.lock().unwrap();
        let id = *next;
        *next += 1;
        id
    };
    drop(bus_guard); // 释放总线锁，避免嵌套锁

    let event = SystemEvent {
        event_id,
        timestamp: now_ns(),
        source,
        event_type,
        correlation_id,
        payload,
    };
    bus.lock().unwrap().publish(event);
}

/// 从能力执行结果发布事件（便捷函数）。
pub fn publish_from_capability(
    ctx: &ExecutionContext,
    event_type: SystemEventType,
    payload: Value,
) {
    let source = match ctx.caller_type {
        super::capability::CallerType::Agent => EventSource::Agent {
            agent_id: ctx.caller_id.clone(),
        },
        super::capability::CallerType::Shell => EventSource::Shell {
            session_id: ctx.caller_id.clone(),
        },
        super::capability::CallerType::Gui => EventSource::Gui {
            session_id: ctx.caller_id.clone(),
        },
        super::capability::CallerType::System => EventSource::System,
    };
    publish_event(source, event_type, None, payload);
}

/// 订阅者适配器：将闭包作为 EventSubscriber。
pub struct ClosureSubscriber<F>
where
    F: Fn(&SystemEvent) + Send + Sync,
{
    f: Arc<Mutex<F>>,
}

impl<F> ClosureSubscriber<F>
where
    F: Fn(&SystemEvent) + Send + Sync,
{
    pub fn new(f: F) -> Self {
        Self {
            f: Arc::new(Mutex::new(f)),
        }
    }
}

impl<F> EventSubscriber for ClosureSubscriber<F>
where
    F: Fn(&SystemEvent) + Send + Sync,
{
    fn on_event(&self, event: &SystemEvent) {
        let f = self.f.lock().unwrap();
        f(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use serde_json::json;

    #[test]
    fn publish_and_subscribe() {
        let bus = EventBus::new(100);
        let received = Arc::new(Mutex::new(Vec::<u64>::new()));
        let received_clone = received.clone();
        let sub = Box::new(ClosureSubscriber::new(move |e| {
            received_clone.lock().unwrap().push(e.event_id);
        }));
        let id = bus.subscribe(EventFilter::default(), sub);

        bus.publish(SystemEvent {
            event_id: 0,
            timestamp: now_ns(),
            source: EventSource::System,
            event_type: SystemEventType::TaskStarted,
            correlation_id: None,
            payload: json!({}),
        });

        assert_eq!(received.lock().unwrap().len(), 1);
        assert!(bus.unsubscribe(id));
        assert!(!bus.unsubscribe(99999));
    }

    #[test]
    fn event_filter_by_type() {
        let bus = EventBus::new(100);
        let received = Arc::new(Mutex::new(Vec::<SystemEventType>::new()));
        let received_clone = received.clone();
        let filter = EventFilter {
            event_types: Some(vec![SystemEventType::TaskCompleted]),
            ..Default::default()
        };
        let sub = Box::new(ClosureSubscriber::new(move |e| {
            received_clone.lock().unwrap().push(e.event_type);
        }));
        bus.subscribe(filter, sub);

        bus.publish(SystemEvent {
            event_id: 0,
            timestamp: now_ns(),
            source: EventSource::System,
            event_type: SystemEventType::TaskStarted,
            correlation_id: None,
            payload: json!({}),
        });
        bus.publish(SystemEvent {
            event_id: 0,
            timestamp: now_ns(),
            source: EventSource::System,
            event_type: SystemEventType::TaskCompleted,
            correlation_id: None,
            payload: json!({}),
        });

        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(received.lock().unwrap()[0], SystemEventType::TaskCompleted);
    }

    #[test]
    fn query_and_replay() {
        let bus = EventBus::new(100);
        bus.publish(SystemEvent {
            event_id: 0,
            timestamp: now_ns(),
            source: EventSource::System,
            event_type: SystemEventType::TaskStarted,
            correlation_id: None,
            payload: json!({}),
        });
        let results = bus.query(&EventFilter::default());
        assert!(!results.is_empty());
    }

    #[test]
    fn publish_event_convenience() {
        publish_event(
            EventSource::System,
            SystemEventType::AgentStarted,
            None,
            json!({"agent_id": "1"}),
        );
        let results = event_bus().lock().unwrap().query(&EventFilter::default());
        assert!(!results.is_empty());
    }
}
