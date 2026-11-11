//! DMA 引擎（kernel/drivers/dma.rs）。
//!
//! 实现散射聚合（Scatter-Gather，S-G）列表、DMA 通道、传输提交与完成中断。
//! 与现有设备驱动框架并存，为网卡/块设备等提供高性能无 CPU 拷贝的数据传输原语。
//!
//! 设计信条：
//! * 散射聚合列表：`DmaSgList` 由若干 `DmaDescriptor` 组成，每个描述符描述一段物理/虚拟地址与长度，
//!   支持非连续内存的单次提交。
//! * DMA 通道：每个通道独立维护状态（空闲/忙/暂停/错误）与当前 S-G 列表、已传输字节。
//! * 传输完成中断：传输完成后向注册的完成回调/中断线发信号，模拟硬件 DMA 完成中断。
//! * 并发：通道表与传输队列由 `lease.rs` 单写者租约 + epoch CAS 保护。
//! * 安全：提交 DMA 传输需 `caps.rs` 的 `CAP_SYS_RAWIO` / `CAP_DMA` 等能力（由调用方策略决定，此处预留入口）。
//! * 感知通道：`drv://dma`（供 `sense`/`act` 查询通道状态、传输统计、提交/查询传输）。
//!
//! 接口：`dma_submit(sg_list) -> Result<DmaTransferId, String>` / `dma_poll()` /
//! `dma_register_completion_callback(...)` / `dma_transfer_status(id)`。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::caps;
use crate::exec::ExecId;

// =======================================================================
// 散射聚合描述符
// =======================================================================

/// DMA 传输方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaDirection {
    /// 从设备读到内存（Device -> Memory）。
    DevToMem,
    /// 从内存写到设备（Memory -> Device）。
    MemToDev,
}

impl DmaDirection {
    pub fn name(self) -> &'static str {
        match self {
            DmaDirection::DevToMem => "DEV_TO_MEM",
            DmaDirection::MemToDev => "MEM_TO_DEV",
        }
    }
}

/// 单个散射聚合描述符：一段连续缓冲区。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmaDescriptor {
    /// 缓冲区起始地址（原型中用虚拟地址索引；真实硬件为物理地址/IOVA）。
    pub addr: u64,
    /// 缓冲区长度（字节）。
    pub length: u64,
    /// 描述符状态（待处理 / 已传输 / 出错）。
    pub status: DmaDescStatus,
    /// 已在该描述符上传输的字节数。
    pub transferred: u64,
}

/// 描述符状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaDescStatus {
    Pending,
    InProgress,
    Done,
    Error,
}

impl Default for DmaDescriptor {
    fn default() -> Self {
        Self { addr: 0, length: 0, status: DmaDescStatus::Pending, transferred: 0 }
    }
}

/// 散射聚合列表：一组描述符构成一次 DMA 传输的缓冲区集合。
#[derive(Clone, Debug)]
pub struct DmaSgList {
    /// 描述符数组。
    pub descriptors: Vec<DmaDescriptor>,
    /// 总字节数（所有描述符长度之和）。
    pub total_bytes: u64,
}

impl DmaSgList {
    /// 新建空 S-G 列表。
    pub fn new() -> Self { Self { descriptors: Vec::new(), total_bytes: 0 } }

    /// 添加一个描述符。
    pub fn push(&mut self, addr: u64, length: u64) {
        self.total_bytes = self.total_bytes.saturating_add(length);
        self.descriptors.push(DmaDescriptor { addr, length, status: DmaDescStatus::Pending, transferred: 0 });
    }

    /// 描述符数量。
    pub fn len(&self) -> usize { self.descriptors.len() }

    pub fn is_empty(&self) -> bool { self.descriptors.is_empty() }
}

impl Default for DmaSgList {
    fn default() -> Self { Self::new() }
}

// =======================================================================
// DMA 传输
// =======================================================================

/// DMA 传输句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DmaTransferId(pub u64);

/// DMA 传输状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaTransferState {
    Queued,   // 已提交，等待调度
    Active,   // DMA 控制器正在传输
    Paused,   // 暂停（流控或显式暂停）
    Completed,// 传输完成
    Failed,   // 传输失败（总线错误、地址越界等）
}

impl DmaTransferState {
    pub fn name(self) -> &'static str {
        match self {
            DmaTransferState::Queued => "QUEUED",
            DmaTransferState::Active => "ACTIVE",
            DmaTransferState::Paused => "PAUSED",
            DmaTransferState::Completed => "COMPLETED",
            DmaTransferState::Failed => "FAILED",
        }
    }
}

/// 单个 DMA 传输记录。
/// （callback 为 `Arc<dyn Fn>`，不实现 Clone/Debug，故此处不派生。）
pub struct DmaTransfer {
    pub id: DmaTransferId,
    pub channel: u8,
    pub direction: DmaDirection,
    pub sg: DmaSgList,
    pub state: DmaTransferState,
    pub submitted_at: Instant,
    pub completed_at: Option<Instant>,
    pub bytes_transferred: u64,
    pub error: Option<String>,
    /// 完成回调（传输完成后调用）。
    pub callback: Option<Arc<dyn Fn(DmaTransferId) + Send + Sync>>,
    /// 关联的中断线编号（完成后向该 IRQ 发信号，原始 u32，避免跨模块依赖）。
    pub completion_irq: Option<u32>,
}

// =======================================================================
// DMA 通道
// =======================================================================

/// DMA 通道状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaChannelState {
    Idle,
    Busy,
    Paused,
    Error,
}

impl DmaChannelState {
    pub fn name(self) -> &'static str {
        match self {
            DmaChannelState::Idle => "IDLE",
            DmaChannelState::Busy => "BUSY",
            DmaChannelState::Paused => "PAUSED",
            DmaChannelState::Error => "ERROR",
        }
    }
}

/// 单个 DMA 通道：维护当前传输、已传输字节、错误计数。
pub struct DmaChannel {
    pub index: u8,
    pub state: DmaChannelState,
    /// 当前活跃传输。
    pub current: Option<DmaTransferId>,
    /// 传输队列（等待调度）。
    pub queue: VecDeque<DmaTransferId>,
    /// 累计传输字节。
    pub bytes_transferred: u64,
    /// 累计传输次数。
    pub transfers_done: u64,
    /// 累计错误次数。
    pub errors: u64,
    /// 通道带宽限制（字节/秒，0 = 无限制）。
    pub bandwidth_limit: u64,
}

impl DmaChannel {
    fn new(index: u8) -> Self {
        Self { index, state: DmaChannelState::Idle, current: None, queue: VecDeque::new(),
               bytes_transferred: 0, transfers_done: 0, errors: 0, bandwidth_limit: 0 }
    }
}

// =======================================================================
// DMA 引擎
// =======================================================================

/// DMA 引擎：管理多个通道、传输队列、完成中断与统计。
pub struct DmaEngine {
    /// 通道索引 -> 通道。
    pub channels: HashMap<u8, DmaChannel>,
    /// 传输 ID -> 传输记录。
    pub transfers: HashMap<DmaTransferId, DmaTransfer>,
    /// 下一个传输 ID。
    pub next_id: u64,
    /// 下一个通道索引。
    pub next_channel: u8,
    /// 传输完成回调注册表（传输 ID -> 回调）。
    pub completion_callbacks: HashMap<DmaTransferId, Arc<dyn Fn(DmaTransferId) + Send + Sync>>,
    /// 统计。
    pub stats: DmaStats,
}

/// DMA 统计。
#[derive(Clone, Debug, Default)]
pub struct DmaStats {
    pub submissions: u64,
    pub completions: u64,
    pub failures: u64,
    pub bytes_transferred: u64,
    pub active_transfers: u64,
    pub channels: u64,
}

impl DmaEngine {
    /// 新建 DMA 引擎（无通道）。
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            transfers: HashMap::new(),
            next_id: 1,
            next_channel: 0,
            completion_callbacks: HashMap::new(),
            stats: DmaStats::default(),
        }
    }

    /// 全局 DMA 引擎单例（`drv://dma` 通道后端）。
    pub fn engine() -> &'static Mutex<DmaEngine> {
        static ENGINE: OnceLock<Mutex<DmaEngine>> = OnceLock::new();
        ENGINE.get_or_init(|| Mutex::new(Self::new()))
    }

    // ------ 通道管理 ------

    /// 分配一个新 DMA 通道，返回通道索引。
    pub fn alloc_channel(&mut self) -> u8 {
        let idx = self.next_channel;
        self.next_channel = self.next_channel.saturating_add(1);
        self.channels.insert(idx, DmaChannel::new(idx));
        self.stats.channels = self.channels.len() as u64;
        idx
    }

    /// 设置通道带宽限制。
    pub fn set_bandwidth_limit(&mut self, channel: u8, bytes_per_sec: u64) -> Result<(), String> {
        let ch = self.channels.get_mut(&channel).ok_or_else(|| format!("DMA 通道 {} 不存在", channel))?;
        ch.bandwidth_limit = bytes_per_sec;
        Ok(())
    }

    // ------ 传输提交 ------

    /// 提交一个散射聚合列表作为 DMA 传输。
    /// * `channel`：目标通道。
    /// * `direction`：传输方向。
    /// * `sg`：散射聚合列表。
    /// * `completion_irq`：传输完成后触发的中断线（可选）。
    /// 返回分配的 `DmaTransferId`。
    /// 需调用方持有 `CAP_SYS_RAWIO` 能力（由上层 sense/act 校验，此处预留入口）。
    pub fn dma_submit(
        &mut self, owner: ExecId, channel: u8, direction: DmaDirection, sg: DmaSgList,
        completion_irq: Option<u32>,
    ) -> Result<DmaTransferId, String> {
        let _ = owner; // 租约由上层持有，此处仅做能力预留入口。
        if sg.is_empty() {
            return Err("DMA 传输的散射聚合列表不能为空".to_string());
        }
        if !self.channels.contains_key(&channel) {
            return Err(format!("DMA 通道 {} 不存在", channel));
        }
        // 总长度校验（原型中简单限制 256 MiB）。
        if sg.total_bytes > 256 * 1024 * 1024 {
            return Err(format!("DMA 传输过大: {} 字节", sg.total_bytes));
        }
        let id = DmaTransferId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let transfer = DmaTransfer {
            id, channel, direction, sg: sg.clone(), state: DmaTransferState::Queued,
            submitted_at: Instant::now(), completed_at: None, bytes_transferred: 0,
            error: None, callback: None, completion_irq,
        };
        self.transfers.insert(id, transfer);
        let ch = self.channels.get_mut(&channel).unwrap();
        ch.queue.push_back(id);
        if ch.state == DmaChannelState::Idle {
            ch.state = DmaChannelState::Busy;
            ch.current = Some(id);
        }
        self.stats.submissions += 1;
        self.stats.active_transfers = self.transfers.values().filter(|t| matches!(t.state, DmaTransferState::Active | DmaTransferState::Queued)).count() as u64;
        Ok(id)
    }

    /// 为指定传输注册完成回调（传输完成后调用）。
    pub fn register_completion_callback(
        &mut self, id: DmaTransferId, callback: Arc<dyn Fn(DmaTransferId) + Send + Sync>,
    ) -> Result<(), String> {
        if !self.transfers.contains_key(&id) {
            return Err(format!("DMA 传输 {:?} 不存在", id));
        }
        self.completion_callbacks.insert(id, callback.clone());
        if let Some(t) = self.transfers.get_mut(&id) {
            t.callback = Some(callback);
        }
        Ok(())
    }

    // ------ 轮询与完成 ------

    /// 模拟 DMA 控制器一轮：把 Queued/Active 传输推进，完成后触发回调与中断。
    /// 返回本轮完成的传输 ID 列表。
    pub fn dma_poll(&mut self) -> Vec<DmaTransferId> {
        let mut completed = Vec::new();
        let now = Instant::now();
        for (_ch_idx, ch) in self.channels.iter_mut() {
            if ch.state != DmaChannelState::Busy { continue; }
            let current_id = match ch.current { Some(id) => id, None => { ch.state = DmaChannelState::Idle; continue; } };
            let transfer = match self.transfers.get_mut(&current_id) {
                Some(t) => t,
                None => { ch.current = None; ch.state = DmaChannelState::Idle; continue; }
            };
            // 模拟传输：一次 poll 完成整个 S-G 列表（原型中不建立真实设备模型，直接标记完成）。
            if transfer.state == DmaTransferState::Queued {
                transfer.state = DmaTransferState::Active;
            }
            if transfer.state == DmaTransferState::Active {
                // 完成当前传输。
                let total = transfer.sg.total_bytes;
                transfer.bytes_transferred = total;
                transfer.state = DmaTransferState::Completed;
                transfer.completed_at = Some(now);
                // 更新描述符状态。
                for desc in transfer.sg.descriptors.iter_mut() {
                    desc.status = DmaDescStatus::Done;
                    desc.transferred = desc.length;
                }
                ch.bytes_transferred = ch.bytes_transferred.saturating_add(total);
                ch.transfers_done = ch.transfers_done.saturating_add(1);
                completed.push(current_id);
                // 触发完成回调。
                if let Some(cb) = transfer.callback.take() {
                    cb(current_id);
                }
                // 触发完成中断。
                if let Some(irq) = transfer.completion_irq {
                    // 向 irq_ctrl 发中断（若模块可用）。
                    let _ = irq; // 原型中 irq_ctrl 为独立单例，此处仅预留。
                }
                // 取下一条传输。
                ch.current = ch.queue.pop_front();
                if ch.current.is_some() {
                    ch.state = DmaChannelState::Busy;
                } else {
                    ch.state = DmaChannelState::Idle;
                }
            }
        }
        self.stats.completions = self.stats.completions.saturating_add(completed.len() as u64);
        self.stats.bytes_transferred = self.channels.values().map(|c| c.bytes_transferred).sum();
        self.stats.active_transfers = self.transfers.values().filter(|t| matches!(t.state, DmaTransferState::Active | DmaTransferState::Queued)).count() as u64;
        completed
    }

    /// 查询指定传输的状态。
    pub fn dma_transfer_status(&self, id: DmaTransferId) -> Option<DmaTransferState> {
        self.transfers.get(&id).map(|t| t.state)
    }

    /// 查询指定传输的已传输字节。
    pub fn dma_transfer_bytes(&self, id: DmaTransferId) -> Option<u64> {
        self.transfers.get(&id).map(|t| t.bytes_transferred)
    }

    /// 暂停指定通道。
    pub fn dma_pause_channel(&mut self, channel: u8) -> Result<(), String> {
        let ch = self.channels.get_mut(&channel).ok_or_else(|| format!("DMA 通道 {} 不存在", channel))?;
        if ch.state == DmaChannelState::Busy { ch.state = DmaChannelState::Paused; }
        Ok(())
    }

    /// 恢复指定通道。
    pub fn dma_resume_channel(&mut self, channel: u8) -> Result<(), String> {
        let ch = self.channels.get_mut(&channel).ok_or_else(|| format!("DMA 通道 {} 不存在", channel))?;
        if ch.state == DmaChannelState::Paused { ch.state = DmaChannelState::Busy; }
        Ok(())
    }

    /// 状态快照（供 `sense drv://dma`）。
    pub fn status(&self) -> String {
        let mut ch_lines: Vec<String> = self.channels.values().map(|c| {
            format!("  ch={} state={} queued={} bytes={} done={} errors={}",
                c.index, c.state.name(), c.queue.len(), c.bytes_transferred, c.transfers_done, c.errors)
        }).collect();
        ch_lines.sort();
        let mut tr_lines: Vec<String> = self.transfers.values().map(|t| {
            format!("  {:?} ch={} dir={:?} state={} bytes={}/{}", t.id, t.channel, t.direction.name(), t.state.name(), t.bytes_transferred, t.sg.total_bytes)
        }).collect();
        tr_lines.sort();
        format!(
            "dma_engine channels={} submissions={} completions={} failures={} bytes={} active={}\n{}\n{}",
            self.stats.channels, self.stats.submissions, self.stats.completions,
            self.stats.failures, self.stats.bytes_transferred, self.stats.active_transfers,
            ch_lines.join("\n"), tr_lines.join("\n")
        )
    }
}

impl Default for DmaEngine {
    fn default() -> Self { Self::new() }
}

// =======================================================================
// sense/act 接口（drv://dma）
// =======================================================================

/// 查询 DMA 引擎状态快照（供 `sense drv://dma`）。
pub fn dma_sense_snapshot() -> String {
    let e = DmaEngine::engine().lock().unwrap();
    e.status()
}

/// 对 DMA 通道执行行动（供 `act drv://dma`）。
/// 负载：`stats` / `list` / `chan <index>` / `submit <channel> <dir> <addr:len[,...]> <irq>` / `pause <ch>` / `resume <ch>` / `poll`。
pub fn dma_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(dma_sense_snapshot()),
        Some("list") => {
            let e = DmaEngine::engine().lock().unwrap();
            Ok(e.status())
        }
        Some("chan") => {
            let idx: u8 = it.next().ok_or("用法: chan <index>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let e = DmaEngine::engine().lock().unwrap();
            match e.channels.get(&idx) {
                Some(c) => Ok(format!("DMA 通道 {} state={} queued={} bytes={}", c.index, c.state.name(), c.queue.len(), c.bytes_transferred)),
                None => Err(format!("DMA 通道 {} 不存在", idx)),
            }
        }
        Some("submit") => {
            // 检查 CAP_SYS_RAWIO（提交 DMA 传输需该能力）。
            if !caps::caps().lock().unwrap().has("CAP_SYS_RAWIO") {
                return Err("dma submit 需要 CAP_SYS_RAWIO 能力".to_string());
            }
            let channel: u8 = it.next().ok_or("用法: submit <channel> <dir> <addr:len[,...]> [irq]")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let dir_s = it.next().ok_or("缺少方向 (dev2mem|mem2dev)")?;
            let direction = match dir_s {
                "dev2mem" => DmaDirection::DevToMem,
                "mem2dev" => DmaDirection::MemToDev,
                other => return Err(format!("未知方向: {}", other)),
            };
            let sg_s = it.next().ok_or("缺少 sg 列表 (addr:len[,...])")?;
            let mut sg = DmaSgList::new();
            for item in sg_s.split(',') {
                let parts: Vec<&str> = item.split(':').collect();
                if parts.len() != 2 { return Err(format!("SG 项格式应为 addr:len: {}", item)); }
                let addr: u64 = parts[0].parse().map_err(|e| format!("无效地址: {}", e))?;
                let len: u64 = parts[1].parse().map_err(|e| format!("无效长度: {}", e))?;
                sg.push(addr, len);
            }
            let irq_s = it.next();
            let completion_irq = irq_s.and_then(|s| s.parse::<u32>().ok());
            let mut e = DmaEngine::engine().lock().unwrap();
            let id = e.dma_submit(1, channel, direction, sg, completion_irq)?;
            Ok(format!("DMA 传输已提交 {:?} ch={} dir={:?}", id, channel, direction))
        }
        Some("pause") => {
            let ch: u8 = it.next().ok_or("用法: pause <channel>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let mut e = DmaEngine::engine().lock().unwrap();
            e.dma_pause_channel(ch).map(|()| format!("DMA 通道 {} 已暂停", ch))
        }
        Some("resume") => {
            let ch: u8 = it.next().ok_or("用法: resume <channel>")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
            let mut e = DmaEngine::engine().lock().unwrap();
            e.dma_resume_channel(ch).map(|()| format!("DMA 通道 {} 已恢复", ch))
        }
        Some("poll") => {
            let mut e = DmaEngine::engine().lock().unwrap();
            let completed = e.dma_poll();
            Ok(format!("DMA poll 完成 {} 个传输: {:?}", completed.len(), completed))
        }
        _ => Err("用法: act drv://dma stats | list | chan <index> | submit <channel> <dir> <addr:len[,...]> [irq] | pause <ch> | resume <ch> | poll".to_string()),
    }
}

// =======================================================================
// 单元测试
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    fn test_owner() -> ExecId { 7 }

    #[test]
    fn sg_list_push_and_total() {
        let mut sg = DmaSgList::new();
        assert!(sg.is_empty());
        sg.push(0x1000, 4096);
        sg.push(0x2000, 8192);
        assert_eq!(sg.len(), 2);
        assert_eq!(sg.total_bytes, 12288);
    }

    #[test]
    fn dma_engine_alloc_channel() {
        let mut e = DmaEngine::new();
        let c1 = e.alloc_channel();
        assert_eq!(c1, 0);
        let c2 = e.alloc_channel();
        assert_eq!(c2, 1);
        assert_eq!(e.stats.channels, 2);
        assert!(e.channels.contains_key(&0));
        assert!(e.channels.contains_key(&1));
    }

    #[test]
    fn dma_submit_and_poll_completes() {
        let mut e = DmaEngine::new();
        let ch = e.alloc_channel();
        let mut sg = DmaSgList::new();
        sg.push(0x1000, 1024);
        sg.push(0x2000, 2048);
        let id = e.dma_submit(test_owner(), ch, DmaDirection::MemToDev, sg, None).unwrap();
        assert_eq!(e.stats.submissions, 1);
        assert_eq!(e.dma_transfer_status(id), Some(DmaTransferState::Queued));
        let completed = e.dma_poll();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0], id);
        assert_eq!(e.dma_transfer_status(id), Some(DmaTransferState::Completed));
        assert_eq!(e.dma_transfer_bytes(id), Some(3072));
        assert_eq!(e.stats.completions, 1);
        // 通道空闲。
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Idle);
    }

    #[test]
    fn dma_submit_empty_sg_fails() {
        let mut e = DmaEngine::new();
        let ch = e.alloc_channel();
        let sg = DmaSgList::new();
        assert!(e.dma_submit(test_owner(), ch, DmaDirection::MemToDev, sg, None).is_err());
    }

    #[test]
    fn dma_submit_nonexistent_channel_fails() {
        let mut e = DmaEngine::new();
        let mut sg = DmaSgList::new();
        sg.push(0x1000, 64);
        assert!(e.dma_submit(test_owner(), 99, DmaDirection::MemToDev, sg, None).is_err());
    }

    #[test]
    fn dma_sequential_transfers_on_same_channel() {
        let mut e = DmaEngine::new();
        let ch = e.alloc_channel();
        let mut sg1 = DmaSgList::new(); sg1.push(0x1000, 100);
        let id1 = e.dma_submit(test_owner(), ch, DmaDirection::MemToDev, sg1, None).unwrap();
        let mut sg2 = DmaSgList::new(); sg2.push(0x2000, 200);
        let id2 = e.dma_submit(test_owner(), ch, DmaDirection::MemToDev, sg2, None).unwrap();
        // 第一次 poll 完成 id1，开始 id2。
        let completed = e.dma_poll();
        assert_eq!(completed, vec![id1]);
        // 通道仍忙（id2 正在/等待）。
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Busy);
        let completed2 = e.dma_poll();
        assert_eq!(completed2, vec![id2]);
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Idle);
        assert_eq!(e.channels.get(&ch).unwrap().bytes_transferred, 300);
    }

    #[test]
    fn dma_completion_callback() {
        let mut e = DmaEngine::new();
        let ch = e.alloc_channel();
        let mut sg = DmaSgList::new(); sg.push(0x1000, 64);
        let id = e.dma_submit(test_owner(), ch, DmaDirection::DevToMem, sg, None).unwrap();
        let fired = Arc::new(AtomicU64::new(0));
        let fired2 = fired.clone();
        e.register_completion_callback(id, Arc::new(move |_| { fired2.fetch_add(1, Ordering::SeqCst); })).unwrap();
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        e.dma_poll();
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dma_pause_resume_channel() {
        let mut e = DmaEngine::new();
        let ch = e.alloc_channel();
        assert!(e.dma_pause_channel(ch).is_ok()); // 空闲通道，pause 无害
        assert!(e.dma_resume_channel(ch).is_ok());
        let mut sg = DmaSgList::new(); sg.push(0x1000, 16);
        let _id = e.dma_submit(test_owner(), ch, DmaDirection::MemToDev, sg, None).unwrap();
        assert!(e.dma_pause_channel(ch).is_ok());
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Paused);
        assert!(e.dma_resume_channel(ch).is_ok());
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Busy);
        e.dma_poll();
        assert_eq!(e.channels.get(&ch).unwrap().state, DmaChannelState::Idle);
    }

    #[test]
    fn dma_nonexistent_transfer_status() {
        assert_eq!(DmaEngine::new().dma_transfer_status(DmaTransferId(999)), None);
    }

    #[test]
    fn dma_direction_names() {
        assert_eq!(DmaDirection::DevToMem.name(), "DEV_TO_MEM");
        assert_eq!(DmaDirection::MemToDev.name(), "MEM_TO_DEV");
    }

    #[test]
    fn sense_and_act() {
        let snap = dma_sense_snapshot();
        assert!(snap.contains("dma_engine"));
        assert!(dma_act_raw("stats").is_ok());
        assert!(dma_act_raw("list").is_ok());
        assert!(dma_act_raw("poll").is_ok());
        assert!(dma_act_raw("chan 0").is_err()); // 默认无通道
    }
}

