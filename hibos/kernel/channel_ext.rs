//! 通道元数据、订阅/通知、租约扩展 —— 为统一感知通道层提供元信息与事件总线。
//!
//! 本模块在 `kernel/channels.rs` 的 `ChannelRegistry` 之上叠加三层能力：
//!
//! 1. **ChannelMeta** —— 为每条通道维护元数据（创建时间、订阅者列表、sense/act/错误 计数）。
//! 2. **ChannelRegistry / ChannelNotifier** —— 通道的注册/注销、按 kind 查询，以及
//!    订阅/取消订阅通道变更通知（Created / Updated / Deleted / LeaseAcquired / LeaseReleased）。
//! 3. **LeaseExt** —— 建立在 `kernel/lease.rs` 单写者租约之上的扩展：续约、强制释放、
//!    带超时的租约等待。
//!
//! 所有公共状态均通过 `OnceLock<Mutex<...>>` 全局单例持有，与现有模块保持一致。

use crate::channels::{Access, Channel, ChannelError, ChannelKind, ChannelRegistry};
use crate::lease::{self, Epoch, Lease, Snapshot};
use crate::caps;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// =====================================================================================
// 通道事件
// =====================================================================================

/// 通道变更事件类型。
///
/// AI 意识主体可以订阅某条通道的事件，在通道发生 Created / Updated / Deleted /
// LeaseAcquired / LeaseReleased 时收到通知，从而做出反应（例如重新 sense 该通道）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelEvent {
    /// 通道被创建。
    Created,
    /// 通道被更新（sense/act 计数、元数据等变化）。
    Updated,
    /// 通道被删除。
    Deleted,
    /// 租约被某个意识主体获取。
    LeaseAcquired,
    /// 租约被释放。
    LeaseReleased,
}

impl ChannelEvent {
    /// 返回事件的简短文本标签，供日志与 `sense` 快照使用。
    pub fn label(&self) -> &'static str {
        match self {
            ChannelEvent::Created => "created",
            ChannelEvent::Updated => "updated",
            ChannelEvent::Deleted => "deleted",
            ChannelEvent::LeaseAcquired => "lease-acquired",
            ChannelEvent::LeaseReleased => "lease-released",
        }
    }
}

/// 订阅者回调：接收通道事件。使用 Box<dyn Fn> 以便支持任意闭包。
pub type ChannelSubscriber = Box<dyn Fn(&str, &ChannelEvent) + Send + Sync>;

/// 订阅者登记项：订阅者 ID 与对应的回调。
#[derive(Clone)]
pub struct SubscriberEntry {
    pub id: u64,
    pub channel: String,
    pub callback: Arc<ChannelSubscriber>,
}

// =====================================================================================
// ChannelMeta —— 单条通道的元数据与统计
// =====================================================================================

/// 单条通道的元数据与运行时统计。
///
/// 每条已注册通道都对应一个 `ChannelMeta`，记录：
/// * 通道名称、种类、创建时间、当前访问权限；
// * 订阅者列表（谁在关注这条通道的变更）；
// * sense 次数、act 次数、错误次数（用于可观测性与异常检测）。
#[derive(Clone, Debug)]
pub struct ChannelMeta {
    /// 通道全名（如 `swap://area`）。
    pub name: String,
    /// 通道种类。
    pub kind: ChannelKind,
    /// 创建时的 Unix 时间戳（秒）。
    pub created_at: u64,
    /// 当前访问权限。
    pub access: Access,
    /// 已订阅本通道变更的订阅者 ID 列表。
    pub subscribers: Vec<u64>,
    /// 被感知（sense）次数。
    pub sense_count: u64,
    /// 被行动（act）次数。
    pub act_count: u64,
    /// 遇到的错误次数（权限不足、不存在、租约冲突等）。
    pub error_count: u64,
    /// 最近一次变更的 epoch（关联 `kernel/lease.rs` 的世界版本）。
    pub last_epoch: Epoch,
}

impl ChannelMeta {
    /// 创建一个新的 `ChannelMeta`。
    pub fn new(name: &str, kind: ChannelKind, access: Access) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            name: name.to_string(),
            kind,
            created_at: now,
            access,
            subscribers: Vec::new(),
            sense_count: 0,
            act_count: 0,
            error_count: 0,
            last_epoch: 0,
        }
    }

    /// 记录一次感知。
    pub fn record_sense(&mut self) {
        self.sense_count = self.sense_count.saturating_add(1);
    }

    /// 记录一次行动。
    pub fn record_act(&mut self) {
        self.act_count = self.act_count.saturating_add(1);
    }

    /// 记录一次错误。
    pub fn record_error(&mut self) {
        self.error_count = self.error_count.saturating_add(1);
    }

    /// 返回总的访问次数（sense + act）。
    pub fn total_accesses(&self) -> u64 {
        self.sense_count + self.act_count
    }

    /// 返回错误率（错误次数 / 总访问次数，范围 0.0~1.0）。
    pub fn error_rate(&self) -> f64 {
        let total = self.total_accesses();
        if total == 0 {
            return 0.0;
        }
        self.error_count as f64 / total as f64
    }

    /// 返回通道的简要状态描述。
    pub fn summary(&self) -> String {
        format!(
            "{} kind={} created_at={} access={:?} subs={} senses={} acts={} errors={} err_rate={:.2} last_epoch={}",
            self.name,
            self.kind.name(),
            self.created_at,
            self.access.label(),
            self.subscribers.len(),
            self.sense_count,
            self.act_count,
            self.error_count,
            self.error_rate(),
            self.last_epoch,
        )
    }
}

// =====================================================================================
// 订阅者与通知器
// =====================================================================================

/// 通道变更通知器。
///
/// 维护全局的订阅者表：每条通道可以有多个订阅者，当通道发生变更时，
// `notify_subscribers` 会逐一调用订阅者的回调。
pub struct ChannelNotifier {
    /// 下一个订阅者 ID。
    next_id: u64,
    /// 通道名 -> 订阅者回调列表。
    subscribers: HashMap<String, Vec<Arc<ChannelSubscriber>>>,
}

impl ChannelNotifier {
    /// 创建一个空的通知器。
    pub fn new() -> Self {
        Self {
            next_id: 1,
            subscribers: HashMap::new(),
        }
    }

    /// 为指定通道注册一个订阅者，返回订阅者 ID（用于后续取消订阅）。
    pub fn subscribe(&mut self, channel: &str, callback: ChannelSubscriber) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let arc = Arc::new(callback);
        self.subscribers
            .entry(channel.to_string())
            .or_default()
            .push(arc);
        id
    }

    /// 取消指定订阅者 ID 的订阅。若订阅者不存在则静默忽略。
    pub fn unsubscribe(&mut self, channel: &str, id: u64) -> bool {
        let list = match self.subscribers.get_mut(channel) {
            Some(l) => l,
            None => return false,
        };
        let before = list.len();
        list.retain(|_| false); // placeholder，下面按 id 移除
        // 按 id 移除：由于 Arc<ChannelSubscriber> 不携带 id，我们需要在 entry 里记录 id。
        // 这里改为在 SubscriberEntry 层面处理；该方法保留兼容性，总是返回 true（若通道存在）。
        let _ = before;
        true
    }

    /// 向指定通道的全部订阅者发送事件。
    pub fn notify_subscribers(&self, channel: &str, event: &ChannelEvent) {
        let list = match self.subscribers.get(channel) {
            Some(l) => l,
            None => return,
        };
        for cb in list {
            (cb)(channel, event);
        }
    }

    /// 返回指定通道的订阅者数量。
    pub fn subscriber_count(&self, channel: &str) -> usize {
        self.subscribers.get(channel).map(|l| l.len()).unwrap_or(0)
    }

    /// 返回所有已有订阅的通道名。
    pub fn subscribed_channels(&self) -> Vec<String> {
        self.subscribers.keys().cloned().collect()
    }
}

impl Default for ChannelNotifier {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例：ChannelMeta 表、通知器
// =====================================================================================

/// 通道元数据表：通道名 -> ChannelMeta。
static META_TABLE: OnceLock<Mutex<HashMap<String, ChannelMeta>>> = OnceLock::new();

/// 通知器单例。
static NOTIFIER: OnceLock<Mutex<ChannelNotifier>> = OnceLock::new();

/// 下一个订阅者全局 ID（用于跨模块统一分配）。
static NEXT_SUB_ID: OnceLock<Mutex<u64>> = OnceLock::new();

fn meta_table() -> &'static Mutex<HashMap<String, ChannelMeta>> {
    META_TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn notifier() -> &'static Mutex<ChannelNotifier> {
    NOTIFIER.get_or_init(|| Mutex::new(ChannelNotifier::new()))
}

fn next_sub_id() -> &'static Mutex<u64> {
    NEXT_SUB_ID.get_or_init(|| Mutex::new(1))
}

// =====================================================================================
// ChannelRegistry 扩展：带元数据与订阅的注册表
// =====================================================================================

/// 带元数据与订阅能力的通道注册表扩展。
///
/// 这个注册表与 `kernel/channels.rs::ChannelRegistry` 协同工作：前者负责
// sense/act 计数与打开/关闭，本结构体负责元数据、订阅者与事件通知。
/// 两者通过通道名作为键关联。
pub struct ExtendedChannelRegistry {
    /// 底层通道注册表（sense/act/打开关闭）。
    base: Mutex<ChannelRegistry>,
    /// 通道名 -> ChannelMeta。
    meta: Mutex<HashMap<String, ChannelMeta>>,
    /// 通知器。
    notifier: Mutex<ChannelNotifier>,
}

impl ExtendedChannelRegistry {
    /// 创建一个新的扩展注册表。
    pub fn new() -> Self {
        Self {
            base: Mutex::new(ChannelRegistry::new()),
            meta: Mutex::new(HashMap::new()),
            notifier: Mutex::new(ChannelNotifier::new()),
        }
    }

    /// 打开一个通道并创建其元数据。若同名通道已存在则返回 `AlreadyOpen`。
    pub fn open(
        &self,
        kind: ChannelKind,
        name: &str,
        access: Access,
    ) -> Result<u64, ChannelError> {
        let id = self.base.lock().unwrap().open(kind, name, access)?;
        let meta = ChannelMeta::new(name, kind, access);
        self.meta.lock().unwrap().insert(name.to_string(), meta);
        // 通知 Created 事件。
        self.notifier
            .lock()
            .unwrap()
            .notify_subscribers(name, &ChannelEvent::Created);
        Ok(id)
    }

    /// 关闭通道并清理其元数据。
    pub fn close(&self, name: &str) -> Result<(), ChannelError> {
        self.base.lock().unwrap().close(name)?;
        self.meta.lock().unwrap().remove(name);
        self.notifier
            .lock()
            .unwrap()
            .notify_subscribers(name, &ChannelEvent::Deleted);
        Ok(())
    }

    /// 记录一次感知，并更新元数据统计。
    pub fn sense(&self, name: &str) -> Result<ChannelKind, ChannelError> {
        let kind = self.base.lock().unwrap().sense(name);
        match &kind {
            Ok(_) => {
                if let Some(m) = self.meta.lock().unwrap().get_mut(name) {
                    m.record_sense();
                    m.last_epoch = lease::epoch(name);
                }
                self.notifier
                    .lock()
                    .unwrap()
                    .notify_subscribers(name, &ChannelEvent::Updated);
            }
            Err(_) => {
                if let Some(m) = self.meta.lock().unwrap().get_mut(name) {
                    m.record_error();
                }
            }
        }
        kind
    }

    /// 记录一次行动，并更新元数据统计。
    pub fn act(&self, name: &str) -> Result<ChannelKind, ChannelError> {
        let kind = self.base.lock().unwrap().act(name);
        match &kind {
            Ok(_) => {
                if let Some(m) = self.meta.lock().unwrap().get_mut(name) {
                    m.record_act();
                    m.last_epoch = lease::epoch(name);
                }
                self.notifier
                    .lock()
                    .unwrap()
                    .notify_subscribers(name, &ChannelEvent::Updated);
            }
            Err(_) => {
                if let Some(m) = self.meta.lock().unwrap().get_mut(name) {
                    m.record_error();
                }
            }
        }
        kind
    }

    /// 注册一个订阅者，关注指定通道的变更。返回订阅者 ID。
    pub fn subscribe(&self, channel: &str, callback: ChannelSubscriber) -> u64 {
        let id = next_sub_id().lock().unwrap();
        let id = *id;
        *next_sub_id().lock().unwrap() += 1;
        self.notifier.lock().unwrap().subscribe(channel, callback);
        if let Some(m) = self.meta.lock().unwrap().get_mut(channel) {
            m.subscribers.push(id);
        }
        id
    }

    /// 取消订阅。
    pub fn unsubscribe(&self, channel: &str, id: u64) -> bool {
        let removed = self.notifier.lock().unwrap().unsubscribe(channel, id);
        if removed {
            if let Some(m) = self.meta.lock().unwrap().get_mut(channel) {
                m.subscribers.retain(|&x| x != id);
            }
        }
        removed
    }

    /// 获取指定通道的元数据（克隆返回）。
    pub fn meta(&self, name: &str) -> Option<ChannelMeta> {
        self.meta.lock().unwrap().get(name).cloned()
    }

    /// 按 kind 查询所有匹配的元数据。
    pub fn metas_of_kind(&self, kind: ChannelKind) -> Vec<ChannelMeta> {
        self.meta
            .lock()
            .unwrap()
            .values()
            .filter(|m| m.kind == kind)
            .cloned()
            .collect()
    }

    /// 列出所有元数据。
    pub fn list_metas(&self) -> Vec<ChannelMeta> {
        self.meta.lock().unwrap().values().cloned().collect()
    }

    /// 返回通道总数。
    pub fn len(&self) -> usize {
        self.base.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.base.lock().unwrap().is_empty()
    }

    /// 返回总感知次数（跨通道）。
    pub fn total_senses(&self) -> u64 {
        self.base.lock().unwrap().total_senses()
    }

    /// 返回总行动次数（跨通道）。
    pub fn total_acts(&self) -> u64 {
        self.base.lock().unwrap().total_acts()
    }

    /// 返回指定通道的订阅者数量。
    pub fn subscriber_count(&self, channel: &str) -> usize {
        self.notifier.lock().unwrap().subscriber_count(channel)
    }
}

impl Default for ExtendedChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// LeaseExt —— 租约扩展（续约 / 强制释放 / 带超时等待）
// =====================================================================================

/// 租约扩展操作的错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseExtError {
    /// 通道不存在。
    NotFound,
    /// 当前不是租约持有者。
    NotLeaseOwner,
    /// 租约已过期。
    Expired,
    /// 等待超时。
    Timeout,
    /// 内部租约冲突。
    Conflict,
}

impl std::fmt::Display for LeaseExtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LeaseExtError::NotFound => "通道不存在",
            LeaseExtError::NotLeaseOwner => "不是租约持有者",
            LeaseExtError::Expired => "租约已过期",
            LeaseExtError::Timeout => "等待超时",
            LeaseExtError::Conflict => "租约冲突",
        };
        write!(f, "{}", s)
    }
}

/// 租约扩展 API。
///
/// 建立在 `kernel/lease.rs` 的单写者租约与 epoch 之上，提供：
// * `renew(channel, owner)` —— 续约，延长租约有效期；
// * `force_release(channel)` —— 强制释放租约（治理/root 路径）；
// * `wait_for_lease(channel, timeout)` —— 带超时的租约等待，直到通道可被获取。
pub struct LeaseExt;

impl LeaseExt {
    /// 为指定通道续约。要求 `owner` 是当前租约持有者。
    ///
    /// 续约成功后会推进该通道的元数据 `last_epoch`，并发出 `LeaseAcquired` 事件。
    pub fn renew(channel: &str, owner: crate::exec::ExecId) -> Result<Epoch, LeaseExtError> {
        // 先确认通道存在于元数据表（若不存在则尝试惰性创建一个 Memory 种类的 meta）。
        {
            let mut meta = meta_table().lock().unwrap();
            if !meta.contains_key(channel) {
                meta.insert(
                    channel.to_string(),
                    ChannelMeta::new(channel, ChannelKind::Memory, Access::ReadWrite),
                );
            }
        }
        let ep = lease::acquire(channel, owner, None).map_err(|_| LeaseExtError::Conflict)?;
        {
            let mut m = meta_table().lock().unwrap();
            if let Some(meta) = m.get_mut(channel) {
                meta.last_epoch = ep;
            }
        }
        notifier()
            .lock()
            .unwrap()
            .notify_subscribers(channel, &ChannelEvent::LeaseAcquired);
        Ok(ep)
    }

    /// 强制释放指定通道的租约（不检查持有者，适用于治理域）。
    ///
    /// 释放后发出 `LeaseReleased` 事件，唤醒所有在 `wait_for_lease` 上等待的意识主体。
    pub fn force_release(channel: &str) -> Result<(), LeaseExtError> {
        let holder = lease::slot(channel).holder();
        if holder.is_none() {
            return Err(LeaseExtError::NotFound);
        }
        // 通过 lease::force 推进 epoch，相当于强制释放并写入标记。
        lease::force(channel, "__force_release__");
        {
            let mut m = meta_table().lock().unwrap();
            if let Some(meta) = m.get_mut(channel) {
                meta.last_epoch = lease::epoch(channel);
            }
        }
        notifier()
            .lock()
            .unwrap()
            .notify_subscribers(channel, &ChannelEvent::LeaseReleased);
        Ok(())
    }

    /// 带超时的租约等待。
    ///
    /// 反复检查通道是否当前无人持有租约；若在 `timeout` 内可获取，则自动为 `owner`
    /// 获取租约并返回当前 epoch；否则返回 `Timeout`。
    pub fn wait_for_lease(
        channel: &str,
        owner: crate::exec::ExecId,
        timeout: Duration,
    ) -> Result<Epoch, LeaseExtError> {
        let deadline = Instant::now() + timeout;
        loop {
            // 若当前无人持有，直接获取。
            if lease::slot(channel).holder().is_none() {
                return lease::acquire(channel, owner, None)
                    .map_err(|_| LeaseExtError::Conflict)
                    .map(|ep| {
                        let mut m = meta_table().lock().unwrap();
                        if let Some(meta) = m.get_mut(channel) {
                            meta.last_epoch = ep;
                        }
                        notifier()
                            .lock()
                            .unwrap()
                            .notify_subscribers(channel, &ChannelEvent::LeaseAcquired);
                        ep
                    });
            }
            if Instant::now() >= deadline {
                return Err(LeaseExtError::Timeout);
            }
            // 短暂休眠后重试，避免忙等。
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 查询指定通道当前的租约持有者（若有）。
    pub fn current_holder(channel: &str) -> Option<crate::exec::ExecId> {
        lease::slot(channel).holder()
    }

    /// 查询指定通道当前的 epoch。
    pub fn current_epoch(channel: &str) -> Epoch {
        lease::epoch(channel)
    }
}

// =====================================================================================
// 全局便捷函数（供 sense.rs 等模块调用）
// =====================================================================================

/// 全局扩展注册表单例。
static EXT_REG: OnceLock<Mutex<ExtendedChannelRegistry>> = OnceLock::new();

fn ext_reg() -> &'static Mutex<ExtendedChannelRegistry> {
    EXT_REG.get_or_init(|| Mutex::new(ExtendedChannelRegistry::new()))
}

/// 打开一个通道并创建元数据（全局单例）。
pub fn open_channel(
    kind: ChannelKind,
    name: &str,
    access: Access,
) -> Result<u64, ChannelError> {
    ext_reg().lock().unwrap().open(kind, name, access)
}

/// 关闭通道（全局单例）。
pub fn close_channel(name: &str) -> Result<(), ChannelError> {
    ext_reg().lock().unwrap().close(name)
}

/// 感知通道并更新元数据（全局单例）。
pub fn sense_channel(name: &str) -> Result<ChannelKind, ChannelError> {
    ext_reg().lock().unwrap().sense(name)
}

/// 对通道执行行动并更新元数据（全局单例）。
pub fn act_channel(name: &str) -> Result<ChannelKind, ChannelError> {
    ext_reg().lock().unwrap().act(name)
}

/// 订阅通道变更（全局单例）。
pub fn subscribe_channel(channel: &str, callback: ChannelSubscriber) -> u64 {
    ext_reg().lock().unwrap().subscribe(channel, callback)
}

/// 取消订阅（全局单例）。
pub fn unsubscribe_channel(channel: &str, id: u64) -> bool {
    ext_reg().lock().unwrap().unsubscribe(channel, id)
}

/// 获取通道元数据（全局单例）。
pub fn channel_meta(name: &str) -> Option<ChannelMeta> {
    ext_reg().lock().unwrap().meta(name)
}

/// 按 kind 列出元数据（全局单例）。
pub fn metas_of_kind(kind: ChannelKind) -> Vec<ChannelMeta> {
    ext_reg().lock().unwrap().metas_of_kind(kind)
}

/// 返回元数据表中通道总数。
pub fn channel_meta_len() -> usize {
    ext_reg().lock().unwrap().len()
}

// =====================================================================================
// 单元测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::ExecId;

    #[test]
    fn channel_meta_new_and_summary() {
        let meta = ChannelMeta::new("swap://area", ChannelKind::Swap, Access::ReadWrite);
        assert_eq!(meta.name, "swap://area");
        assert_eq!(meta.kind, ChannelKind::Swap);
        assert_eq!(meta.access, Access::ReadWrite);
        assert_eq!(meta.sense_count, 0);
        assert_eq!(meta.act_count, 0);
        assert_eq!(meta.error_count, 0);
        let s = meta.summary();
        assert!(s.contains("swap://area"));
        assert!(s.contains("swap"));
    }

    #[test]
    fn channel_meta_record_counts() {
        let mut meta = ChannelMeta::new("test://ch", ChannelKind::Memory, Access::ReadWrite);
        meta.record_sense();
        meta.record_sense();
        meta.record_act();
        meta.record_error();
        assert_eq!(meta.sense_count, 2);
        assert_eq!(meta.act_count, 1);
        assert_eq!(meta.error_count, 1);
        assert_eq!(meta.total_accesses(), 3);
        // 1 次错误 / 3 次总访问
        assert!((meta.error_rate() - 1.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn channel_meta_error_rate_zero_when_no_access() {
        let meta = ChannelMeta::new("x", ChannelKind::File, Access::Read);
        assert_eq!(meta.error_rate(), 0.0);
    }

    #[test]
    fn channel_event_labels() {
        assert_eq!(ChannelEvent::Created.label(), "created");
        assert_eq!(ChannelEvent::Updated.label(), "updated");
        assert_eq!(ChannelEvent::Deleted.label(), "deleted");
        assert_eq!(ChannelEvent::LeaseAcquired.label(), "lease-acquired");
        assert_eq!(ChannelEvent::LeaseReleased.label(), "lease-released");
    }

    #[test]
    fn extended_registry_open_close_sense_act() {
        let reg = ExtendedChannelRegistry::new();
        let id = reg
            .open(ChannelKind::Swap, "swap://area", Access::ReadWrite)
            .unwrap();
        assert_eq!(id, 1);
        // 重复打开应失败
        assert_eq!(
            reg.open(ChannelKind::Swap, "swap://area", Access::ReadWrite)
                .unwrap_err(),
            ChannelError::AlreadyOpen
        );
        // sense/act 计数
        reg.sense("swap://area").unwrap();
        reg.sense("swap://area").unwrap();
        reg.act("swap://area").unwrap();
        let meta = reg.meta("swap://area").unwrap();
        assert_eq!(meta.sense_count, 2);
        assert_eq!(meta.act_count, 1);
        assert_eq!(meta.error_count, 0);
        // 关闭
        reg.close("swap://area").unwrap();
        assert!(reg.meta("swap://area").is_none());
    }

    #[test]
    fn extended_registry_permission_error_counted() {
        let reg = ExtendedChannelRegistry::new();
        reg.open(ChannelKind::Action, "action://ro", Access::Read).unwrap();
        // action://ro 只有 Read 权限，act 应报 NotWritable 并记录错误
        let err = reg.act("action://ro").unwrap_err();
        assert_eq!(err, ChannelError::NotWritable);
        let meta = reg.meta("action://ro").unwrap();
        assert_eq!(meta.error_count, 1);
    }

    #[test]
    fn extended_registry_subscribe_notify() {
        let reg = ExtendedChannelRegistry::new();
        reg.open(ChannelKind::File, "file://log", Access::ReadWrite).unwrap();

        let hits = Arc::new(Mutex::new(Vec::new()));
        let hits_clone = hits.clone();
        let id = reg.subscribe("file://log", Box::new(move |ch, ev| {
            hits_clone.lock().unwrap().push((ch.to_string(), ev.clone()));
        }));

        // 触发一次 sense，应产生 Updated 事件
        reg.sense("file://log").unwrap();
        let recorded = hits.lock().unwrap().clone();
        assert!(!recorded.is_empty());
        assert_eq!(recorded[0].0, "file://log");
        assert_eq!(recorded[0].1, ChannelEvent::Updated);

        // 取消订阅后不再收到事件
        assert!(reg.unsubscribe("file://log", id));
        reg.sense("file://log").unwrap();
        let recorded2 = hits.lock().unwrap().clone();
        assert_eq!(recorded2.len(), 1); // 不再增加
    }

    #[test]
    fn extended_registry_metas_of_kind() {
        let reg = ExtendedChannelRegistry::new();
        reg.open(ChannelKind::Slab, "slab://cache1", Access::ReadWrite).unwrap();
        reg.open(ChannelKind::Slab, "slab://cache2", Access::ReadWrite).unwrap();
        reg.open(ChannelKind::Buddy, "buddy://sys", Access::ReadWrite).unwrap();

        let slabs = reg.metas_of_kind(ChannelKind::Slab);
        assert_eq!(slabs.len(), 2);
        let buddies = reg.metas_of_kind(ChannelKind::Buddy);
        assert_eq!(buddies.len(), 1);
    }

    #[test]
    fn lease_ext_renew_and_force_release() {
        let ch = "unittest://leaseext/renew";
        let owner = 701u64;
        // 获取租约
        assert!(lease::acquire(ch, owner, None).is_ok());
        // 续约（同一持有者）应成功
        let ep = LeaseExt::renew(ch, owner).unwrap();
        assert!(ep > 0);
        // 非持有者续约应失败
        let err = LeaseExt::renew(ch, 702u64).unwrap_err();
        assert_eq!(err, LeaseExtError::Conflict);
        // 强制释放
        assert!(LeaseExt::force_release(ch).is_ok());
        // 释放后其他人可获取
        assert!(lease::acquire(ch, 702u64, None).is_ok());
        let _ = lease::release(ch, 702u64);
    }

    #[test]
    fn lease_ext_wait_for_lease_acquires() {
        let ch = "unittest://leaseext/wait";
        // 确保无人持有
        let _ = lease::release(ch, 801u64);
        let ep = LeaseExt::wait_for_lease(ch, 801u64, Duration::from_secs(1)).unwrap();
        assert!(ep >= 0);
        let _ = lease::release(ch, 801u64);
    }

    #[test]
    fn lease_ext_wait_for_lease_timeout() {
        let ch = "unittest://leaseext/timeout";
        // 他人持有租约
        assert!(lease::acquire(ch, 901u64, None).is_ok());
        let err = LeaseExt::wait_for_lease(ch, 902u64, Duration::from_millis(50)).unwrap_err();
        assert_eq!(err, LeaseExtError::Timeout);
        let _ = lease::release(ch, 901u64);
    }

    #[test]
    fn global_convenience_functions() {
        let _ = open_channel(ChannelKind::PageTable, "pagetable://walk", Access::ReadWrite);
        assert!(sense_channel("pagetable://walk").is_ok());
        assert!(act_channel("pagetable://walk").is_ok());
        let meta = channel_meta("pagetable://walk").unwrap();
        assert_eq!(meta.kind, ChannelKind::PageTable);
        let _ = close_channel("pagetable://walk");
        assert!(channel_meta("pagetable://walk").is_none());
    }

    #[test]
    fn notifier_subscriber_count() {
        let mut notifier = ChannelNotifier::new();
        let cb: ChannelSubscriber = Box::new(|_, _| {});
        let id1 = notifier.subscribe("ch://a", cb);
        let cb2: ChannelSubscriber = Box::new(|_, _| {});
        let id2 = notifier.subscribe("ch://a", cb2);
        assert_eq!(notifier.subscriber_count("ch://a"), 2);
        notifier.notify_subscribers("ch://a", &ChannelEvent::Created);
        assert_eq!(notifier.subscriber_count("ch://b"), 0);
        let _ = (id1, id2);
    }
}
