//! 完整 TCP 协议栈（kernel/net/tcp_stack.rs）。
//!
//! 建立于 `kernel/net/tcp.rs` 的 `TcpLayer`/`TcpSegment`/`TcpState` 之上，
//! 填充更完整的 TCP 状态机（LISTEN/SYN_SENT/SYN_RECEIVED/ESTABLISHED/
//! FIN_WAIT_1/.../TIME_WAIT）、滑动窗口流量控制、拥塞控制（Reno/Cubic）、
//! 重传定时器（RTO 指数退避）、连接生命周期管理。
//!
//! 设计信条：
//! * 与 `tcp.rs` 的轻量层并存，互不冲突；本模块自持连接表，直接操作
//!   `tcp::TcpSegment`，不复用 `TcpLayer` 的内部状态，避免借用冲突。
//! * 并发：连接表写入由 `lease.rs` 的单写者租约 + epoch CAS 保护，优先于悲观锁。
//! * 安全：绑定/连接等操作校验 `caps.rs` 的能力位（`CAP_NET_BIND`、`CAP_NET_RAW`）。
//! * 感知通道：`net/tcp_stack`（供 `sense`/`act` 查询连接表、拥塞状态、重传统计）。
//!
//! 接口：`tcp_connect(local_port, remote_ip, remote_port)` / `tcp_send(key, data)` /
//! `tcp_recv(key, len)` / `tcp_accept(listener_port)` / `tcp_close(key)` /
//! `tcp_process_segment(src_ip, dst_ip, bytes)` / `tcp_stack_tick(dt)`。

use std::collections::{BTreeMap, VecDeque};
use crate::ksync::{SpinLock, OnceLock};
use core::time::Duration;

use crate::caps;
use crate::exec::ExecId;
use crate::net::tcp::{TcpConnKey, TcpSegment, TcpState, DEFAULT_MSS, TCP_ACK, TCP_FIN,
    TCP_PSH, TCP_RST, TCP_SYN};

// no_std 环境下的 Instant 替代：基于内核纳秒时间戳。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);

impl Instant {
    pub fn now() -> Self {
        Instant(crate::time::get_time_ns())
    }

    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration.as_nanos() as u64).map(Instant)
    }

    pub fn saturating_add(self, duration: Duration) -> Self {
        Instant(self.0.saturating_add(duration.as_nanos() as u64))
    }

    pub fn saturating_sub(self, other: Instant) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(other.0))
    }
}

impl core::ops::Add<Duration> for Instant {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        self.saturating_add(rhs)
    }
}

impl core::ops::Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, rhs: Instant) -> Duration {
        self.saturating_sub(rhs)
    }
}

impl core::ops::Sub<Duration> for Instant {
    type Output = Self;
    fn sub(self, rhs: Duration) -> Self {
        Instant(self.0.saturating_sub(rhs.as_nanos() as u64))
    }
}

// =======================================================================
// 拥塞控制（Reno / Cubic）
// =======================================================================

/// 拥塞控制算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CongestAlg {
    /// Reno：慢启动 + 拥塞避免 + 快重传/快恢复 + 超时重传。
    Reno,
    /// Cubic：立方函数增长，对高带宽延迟积网络更友好（简化实现）。
    Cubic,
}

impl CongestAlg {
    pub fn name(self) -> &'static str {
        match self {
            CongestAlg::Reno => "Reno",
            CongestAlg::Cubic => "Cubic",
        }
    }
}

/// 拥塞状态机。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CongestState {
    /// 慢启动：cwnd 指数增长。
    SlowStart,
    /// 拥塞避免：cwnd 线性/立方增长。
    CongestionAvoidance,
    /// 快恢复：收到 3 个重复 ACK 后进入。
    FastRecovery,
}

impl CongestState {
    pub fn name(self) -> &'static str {
        match self {
            CongestState::SlowStart => "SLOW_START",
            CongestState::CongestionAvoidance => "CONGESTION_AVOIDANCE",
            CongestState::FastRecovery => "FAST_RECOVERY",
        }
    }
}

/// 拥塞控制器：Reno / Cubic。
///
/// cwnd 以字节为单位；MSS 作为增长粒度。Reno 在拥塞避免阶段每个 RTT 增长 1 个 MSS；
/// Cubic 使用立方函数 W(t) = C*(t-K)^3 + W_max（此处简化为按 RTT 离散近似）。
#[derive(Clone)]
pub struct CongestionController {
    /// 所用算法。
    pub alg: CongestAlg,
    /// 当前状态。
    pub state: CongestState,
    /// 拥塞窗口（字节）。
    pub cwnd: u32,
    /// 慢启动阈值（字节）。
    pub ssthresh: u32,
    /// 重复 ACK 计数（达 3 触发快重传）。
    pub dup_acks: u32,
    /// 上次拥塞事件发生时的 cwnd（Cubic W_max / Reno ssthresh 备份）。
    pub cwnd_before_cong: u32,
    /// 拥塞事件发生时刻（Cubic 用于计算 K）。
    pub cong_event_time: Instant,
    /// Cubic：已确认字节累计，用于离散立方近似。
    pub cnt: u32,
    /// 最近一次 RTT 样本（微秒，0 表示未测得）。
    pub rtt_us: u32,
}

impl CongestionController {
    /// 新建拥塞控制器（初始 cwnd = 10*MSS，ssthresh = 65535）。
    pub fn new(alg: CongestAlg) -> Self {
        Self {
            alg,
            state: CongestState::SlowStart,
            cwnd: 10 * DEFAULT_MSS as u32,
            ssthresh: 65535,
            dup_acks: 0,
            cwnd_before_cong: 65535,
            cong_event_time: Instant::now(),
            cnt: 0,
            rtt_us: 0,
        }
    }

    /// 收到一条新确认（ack 向前推进，确认了新数据）。
    pub fn on_new_ack(&mut self, acked_bytes: u32) {
        let mss = DEFAULT_MSS as u32;
        match self.state {
            CongestState::SlowStart => {
                // 慢启动：每个新确认段使 cwnd += MSS（指数增长，每 RTT 翻倍）。
                self.cwnd = self.cwnd.saturating_add(mss);
                if self.cwnd >= self.ssthresh {
                    self.state = CongestState::CongestionAvoidance;
                }
            }
            CongestState::CongestionAvoidance => {
                match self.alg {
                    CongestAlg::Reno => {
                        // Reno 线性增长：每个 RTT 增长约 1 个 MSS。
                        self.cnt = self.cnt.saturating_add(acked_bytes);
                        if self.cnt >= self.cwnd {
                            self.cnt = self.cnt.saturating_sub(self.cwnd);
                            self.cwnd = self.cwnd.saturating_add(mss);
                        }
                    }
                    CongestAlg::Cubic => {
                        // Cubic 简化：每个 RTT 按立方函数近似增长。
                        self.cnt = self.cnt.saturating_add(acked_bytes);
                        if self.cnt >= self.cwnd {
                            self.cnt = self.cnt.saturating_sub(self.cwnd);
                            let elapsed = Instant::now().saturating_sub(self.cong_event_time).as_millis() as f64;
                            let k = self.cubic_k();
                            let t = elapsed / 1000.0; // 秒
                            let w_max = self.cwnd_before_cong.max(1);
                            let target = (0.4 * (t - k).powi(3) as f64 + w_max as f64) as u32;
                            if target > self.cwnd {
                                self.cwnd = target;
                            } else {
                                // 保守回退到线性增长。
                                self.cwnd = self.cwnd.saturating_add(1);
                            }
                        }
                    }
                }
            }
            CongestState::FastRecovery => {
                // 快恢复收到新 ACK：退出快恢复，进入拥塞避免，cwnd = ssthresh。
                self.cwnd = self.ssthresh;
                self.state = CongestState::CongestionAvoidance;
                self.dup_acks = 0;
            }
        }
    }

    /// 收到一个重复 ACK（未确认新数据）。返回 true 表示应触发快重传。
    pub fn on_duplicate_ack(&mut self) -> bool {
        self.dup_acks = self.dup_acks.saturating_add(1);
        if self.dup_acks >= 3 && self.state != CongestState::FastRecovery {
            // 快重传触发。
            self.cwnd_before_cong = self.cwnd;
            self.ssthresh = (self.cwnd / 2).max(DEFAULT_MSS as u32);
            self.cwnd = self.ssthresh.saturating_add(3 * DEFAULT_MSS as u32);
            self.state = CongestState::FastRecovery;
            self.cong_event_time = Instant::now();
            self.cnt = 0;
            true
        } else {
            false
        }
    }

    /// 超时重传：严重拥塞，重置到慢启动。
    pub fn on_timeout(&mut self) {
        self.cwnd_before_cong = self.cwnd;
        self.ssthresh = (self.cwnd / 2).max(DEFAULT_MSS as u32);
        self.cwnd = DEFAULT_MSS as u32;
        self.state = CongestState::SlowStart;
        self.dup_acks = 0;
        self.cnt = 0;
        self.cong_event_time = Instant::now();
    }

    /// 记录一个 RTT 样本（微秒），供 Cubic 与超时计算使用。
    pub fn record_rtt(&mut self, rtt_us: u32) {
        self.rtt_us = rtt_us;
    }

    /// 当前发送窗口上限（受拥塞窗口与对端通告窗口双重限制，由调用方合并）。
    pub fn congestion_window(&self) -> u32 {
        self.cwnd
    }

    fn cubic_k(&self) -> f64 {
        let c = 0.4f64;
        let beta = 0.7f64;
        let w_max = self.cwnd_before_cong.max(1) as f64;
        ((w_max * (1.0 - beta)) / c).cbrt()
    }

}

impl Default for CongestionController {
    fn default() -> Self {
        Self::new(CongestAlg::Reno)
    }
}

// =======================================================================
// TCP 连接（完整状态机）
// =======================================================================

/// 单个 TCP 连接的完整状态。
#[derive(Clone)]
pub struct TcpConn {
    /// 连接四元组键。
    pub key: TcpConnKey,
    pub local_ip: [u8; 4],
    /// 对端 IP（与 key.remote_ip 冗余，便于直接访问）。
    pub remote_ip: [u8; 4],
    /// 对端端口（与 key.remote_port 冗余，便于直接访问）。
    pub remote_port: u16,
    /// TCP 状态。
    pub state: TcpState,
    /// 发送序号。
    pub iss: u32,
    pub snd_una: u32, // 已发送但未确认的最早序号
    pub snd_nxt: u32, // 下一个待发送序号
    /// 接收序号。
    pub rcv_nxt: u32, // 下一个期望接收的序号
    /// 对端通告窗口 / 本端通告窗口。
    pub snd_wnd: u16,
    pub rcv_wnd: u16,
    /// 发送缓冲区：已发未确认 + 待发送数据（序号流）。
    pub send_buf: VecDeque<u8>,
    /// 接收缓冲区：已到序数据，等待应用读取。
    pub recv_buf: VecDeque<u8>,
    /// 已完成三次握手的子连接（仅监听 socket）。
    pub accept_queue: VecDeque<TcpConn>,
    /// 拥塞控制器。
    pub congest: CongestionController,
    /// 超时重传时间（RTO）。
    pub rto: Duration,
    /// 下次重传时间点。
    pub rto_deadline: Instant,
    /// 用于 RTT 测量的序号（当前正在飞行、等待 ACK 的序号）。
    pub rtt_seq: Option<u32>,
    /// RTT 测量开始时刻。
    pub rtt_start: Option<Instant>,
    /// 已重传次数。
    pub retransmissions: u32,
    /// 上次发送时刻（用于超时判断）。
    pub last_sent: Option<Instant>,
}

impl TcpConn {
    /// 为监听 socket 新建连接（状态 Closed，待 bind 后 listen）。
    fn new_listen(local_ip: [u8; 4], local_port: u16, alg: CongestAlg) -> Self {
        Self {
            key: TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 },
            local_ip, remote_ip: [0, 0, 0, 0], remote_port: 0,
            state: TcpState::Closed,
            iss: 0, snd_una: 0, snd_nxt: 0, rcv_nxt: 0,
            snd_wnd: 65535, rcv_wnd: 65535,
            send_buf: VecDeque::new(), recv_buf: VecDeque::new(), accept_queue: VecDeque::new(),
            congest: CongestionController::new(alg),
            rto: Duration::from_secs(1),
            rto_deadline: Instant::now() + Duration::from_secs(1),
            rtt_seq: None, rtt_start: None, retransmissions: 0, last_sent: None,
        }
    }

    // ------ 连接生命周期 ------

    /// 接收窗口可用空间。
    pub fn rcv_window(&self) -> u16 {
        const CAP: usize = 65535;
        CAP.saturating_sub(self.recv_buf.len()) as u16
    }

    /// 当前可发送量：受拥塞窗口与对端通告窗口双重限制，扣除已发未确认。
    pub fn sendable(&self) -> usize {
        let cwnd = self.congest.cwnd as usize;
        let rwnd = self.snd_wnd.max(1) as usize;
        let window = cwnd.min(rwnd);
        let in_flight = self.snd_nxt.wrapping_sub(self.snd_una) as usize;
        window.saturating_sub(in_flight)
    }
}

// =======================================================================
// TCP 协议栈：连接表 + 全局管理
// =======================================================================

/// TCP 协议栈：维护全部 TCP 连接，提供连接/发送/接收/关闭/定时器驱动。
pub struct TcpStack {
    /// 本机 IPv4 地址。
    pub our_ip: [u8; 4],
    /// 连接表：TcpConnKey -> TcpConn。
    pub conns: BTreeMap<TcpConnKey, TcpConn>,
    /// 下一个临时端口。
    pub next_ephemeral: u16,
    /// 默认拥塞控制算法。
    pub default_alg: CongestAlg,
    /// 统计：已发送段 / 已接收段 / 已重传段 / 超时次数 / 当前连接数。
    pub stats: TcpStackStats,
}

/// TCP 协议栈统计。
#[derive(Clone, Debug, Default)]
pub struct TcpStackStats {
    pub segments_sent: u64,
    pub segments_received: u64,
    pub segments_retransmitted: u64,
    pub timeouts: u64,
    pub active_connections: u64,
    pub listen_sockets: u64,
}

impl TcpStack {
    /// 新建 TCP 协议栈实例。
    pub fn new(our_ip: [u8; 4]) -> Self {
        Self {
            our_ip,
            conns: BTreeMap::new(),
            next_ephemeral: 49152,
            default_alg: CongestAlg::Reno,
            stats: TcpStackStats::default(),
        }
    }

    /// 全局 TCP 协议栈单例（`net/tcp_stack` 通道后端）。
    pub fn stack() -> &'static SpinLock<TcpStack> {
        static STACK: OnceLock<SpinLock<TcpStack>> = OnceLock::new();
        STACK.get_or_init(|| SpinLock::new(Self::new([10, 0, 0, 2])))
    }

    // ------ 连接生命周期 ------

    /// 绑定本地端口（状态 Closed），检查 `CAP_NET_BIND` 能力（<1024 端口需该能力）。
    pub fn bind(&mut self, owner: ExecId, local_port: u16) -> Result<(), String> {
        // 受保护端口（<1024）需要 CAP_NET_BIND。
        if local_port < 1024 && !caps::caps().lock().has("CAP_NET_BIND") {
            return Err(format!("绑定特权端口 {} 需要 CAP_NET_BIND 能力", local_port));
        }
        let _ = owner; // 租约由调用方在上层持有，此处仅做能力校验。
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        if self.conns.contains_key(&key) {
            return Err(format!("端口 {} 已绑定", local_port));
        }
        self.conns.insert(key, TcpConn::new_listen(self.our_ip, local_port, self.default_alg));
        self.stats.listen_sockets = self.conns.values().filter(|c| c.state == TcpState::Listen).count() as u64;
        Ok(())
    }

    /// 监听端口（将 Closed 状态转为 Listen）。
    pub fn listen(&mut self, local_port: u16) -> Result<(), String> {
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let conn = self.conns.get_mut(&key).ok_or_else(|| format!("端口 {} 未绑定", local_port))?;
        if conn.state != TcpState::Closed {
            return Err(format!("端口 {} 状态非法 (listen): {}", local_port, conn.state.name()));
        }
        conn.state = TcpState::Listen;
        self.stats.listen_sockets = self.conns.values().filter(|c| c.state == TcpState::Listen).count() as u64;
        Ok(())
    }

    /// 主动打开连接：发送 SYN，返回待发送的段列表。状态 Closed -> SynSent。
    pub fn connect(&mut self, _owner: ExecId, local_port: u16, remote_ip: [u8; 4],
                   remote_port: u16) -> Result<Vec<TcpSegment>, String> {
        // 检查 CAP_NET_RAW（原始 TCP 连接需该能力）。
        if !caps::caps().lock().has("CAP_NET_RAW") {
            return Err("tcp_connect 需要 CAP_NET_RAW 能力".to_string());
        }
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let conn = self.conns.get(&key).ok_or_else(|| format!("端口 {} 未绑定", local_port))?;
        if conn.state != TcpState::Closed {
            return Err(format!("端口 {} 状态非法 (connect): {}", local_port, conn.state.name()));
        }
        let iss = 1000 + local_port as u32; // ISS 基数（与 tcp.rs 一致，实际应随机化）
        let new_key = TcpConnKey { local_port, remote_ip, remote_port };
        let _ = conn; // 释放只读借用，再可变操作 conns
        // 从旧 key 移动到新 key（四元组变化）。
        let mut moved = self.conns.remove(&key).unwrap();
        moved.remote_ip = remote_ip;
        moved.remote_port = remote_port;
        moved.key = new_key.clone();
        moved.iss = iss;
        moved.snd_una = iss;
        moved.snd_nxt = iss + 1; // SYN 占一个序号
        moved.state = TcpState::SynSent;
        moved.last_sent = Some(Instant::now());
        moved.rto_deadline = Instant::now() + moved.rto;
        moved.rtt_seq = Some(iss); // SYN 需要被确认，用于 RTT 测量
        moved.rtt_start = Some(Instant::now());
        self.conns.insert(new_key.clone(), moved);

        let syn = TcpSegment::new(local_port, remote_port, iss, 0, TCP_SYN, 65535, Vec::new());
        self.stats.segments_sent += 1;
        Ok(vec![syn])
    }

    /// 处理一段到达的 TCP 段（原始字节），返回需要发送的段列表。
    /// 这是状态机核心：依据当前状态与段标志进行状态迁移、窗口滑动、拥塞控制更新。
    pub fn process_segment(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4],
                           bytes: &[u8]) -> Result<Vec<TcpSegment>, String> {
        self.stats.segments_received += 1;
        let seg = match TcpSegment::parse(bytes, src_ip, dst_ip) {
            Some(s) => s,
            None => return Ok(Vec::new()), // 校验不通过：静默丢弃
        };

        let exact = TcpConnKey { local_port: seg.dst_port, remote_ip: src_ip, remote_port: seg.src_port };
        let listener_key = TcpConnKey { local_port: seg.dst_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let key = if self.conns.contains_key(&exact) {
            exact.clone()
        } else if let Some(s) = self.conns.get(&listener_key) {
            if s.state == TcpState::Listen { listener_key.clone() } else { return Ok(Vec::new()); }
        } else {
            return Ok(Vec::new()); // 无监听端口：静默丢弃
        };

        let is_syn = seg.flags & TCP_SYN != 0;
        let is_ack = seg.flags & TCP_ACK != 0;
        let is_fin = seg.flags & TCP_FIN != 0;
        let is_rst = seg.flags & TCP_RST != 0;

        if is_rst {
            if let Some(conn) = self.conns.get_mut(&key) {
                conn.state = TcpState::Closed;
                conn.retransmissions = 0;
            }
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let state = self.conns.get(&key).unwrap().state;
        let mut new_state = state;
        let mut rcv_nxt_advance: u32 = 0;
        let mut reply_ack = false;
        let mut new_ack = false; // 是否收到新确认（用于拥塞控制）
        let mut newly_acked_bytes: u32 = 0;
        let mut dup_ack = false;

        match state {
            TcpState::Listen if is_syn => {
                // 被动打开：创建子连接，同时写入主表，供后续段精确匹配。
                let iss_child = 1000 + seg.dst_port as u32;
                let child_key = TcpConnKey { local_port: seg.dst_port, remote_ip: src_ip, remote_port: seg.src_port };
                let mut child = TcpConn {
                    key: TcpConnKey { local_port: seg.dst_port, remote_ip: src_ip, remote_port: seg.src_port },
                    local_ip: dst_ip, remote_ip: src_ip, remote_port: seg.src_port,
                    state: TcpState::SynReceived,
                    iss: iss_child, snd_una: iss_child, snd_nxt: iss_child + 1,
                    rcv_nxt: seg.seq.wrapping_add(1),
                    snd_wnd: seg.window, rcv_wnd: 65535,
                    send_buf: VecDeque::new(), recv_buf: VecDeque::new(), accept_queue: VecDeque::new(),
                    congest: CongestionController::new(self.default_alg),
                    rto: Duration::from_secs(1),
                    rto_deadline: Instant::now() + Duration::from_secs(1),
                    rtt_seq: None, rtt_start: None, retransmissions: 0, last_sent: None,
                };
                child.rcv_nxt = seg.seq.wrapping_add(1);
                let listener = self.conns.get_mut(&listener_key).unwrap();
                listener.accept_queue.push_back(child.clone());
                self.conns.insert(child_key.clone(), child);
                out.push(TcpSegment::new(seg.dst_port, seg.src_port, iss_child,
                                         seg.seq.wrapping_add(1), TCP_SYN | TCP_ACK,
                                         65535, Vec::new()));
                self.stats.segments_sent += 1;
            }
            TcpState::SynSent => {
                if is_syn && is_ack && seg.ack == self.snd_nxt_of(&key) {
                    // 对端 SYN-ACK 确认了我们的 SYN。
                    new_state = TcpState::Established;
                    let sock = self.conns.get_mut(&key).unwrap();
                    sock.rcv_nxt = seg.seq.wrapping_add(1);
                    sock.snd_wnd = seg.window;
                    sock.snd_una = sock.snd_nxt; // SYN 被确认
                    new_ack = true;
                    newly_acked_bytes = 1;
                    // 记录 RTT。
                    if let Some(start) = sock.rtt_start {
                        sock.congest.record_rtt(Instant::now().saturating_sub(start).as_micros() as u32);
                        sock.rtt_seq = None;
                        sock.rtt_start = None;
                    }
                    out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.snd_nxt,
                                             sock.rcv_nxt, TCP_ACK, sock.rcv_window(), Vec::new()));
                    self.stats.segments_sent += 1;
                    sock.last_sent = Some(Instant::now());
                    sock.rto_deadline = Instant::now() + sock.rto;
                } else if is_syn {
                    // 同步打开。
                    let sock = self.conns.get_mut(&key).unwrap();
                    sock.state = TcpState::SynReceived;
                    sock.rcv_nxt = seg.seq.wrapping_add(1);
                    out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.iss,
                                             seg.seq.wrapping_add(1), TCP_SYN | TCP_ACK,
                                             sock.rcv_window(), Vec::new()));
                    self.stats.segments_sent += 1;
                }
            }
            TcpState::SynReceived => {
                if is_ack && seg.ack == self.snd_nxt_of(&key) {
                    new_state = TcpState::Established;
                    let sock = self.conns.get_mut(&key).unwrap();
                    sock.snd_una = sock.snd_nxt;
                    new_ack = true;
                    newly_acked_bytes = 1;
                }
            }
            TcpState::Established => {
                if is_ack {
                    self.ack_received(&key, seg.ack, seg.window, &mut new_ack, &mut newly_acked_bytes, &mut dup_ack);
                }
                if !seg.payload.is_empty() {
                    let sock = self.conns.get(&key).unwrap();
                    if seg.seq == sock.rcv_nxt {
                        rcv_nxt_advance = seg.payload.len() as u32;
                        reply_ack = true;
                    } else if seg.seq < sock.rcv_nxt {
                        // 重复数据，只回 ACK。
                        reply_ack = true;
                    }
                }
                if is_fin {
                    rcv_nxt_advance += 1;
                    new_state = TcpState::CloseWait;
                    reply_ack = true;
                }
            }
            TcpState::FinWait1 => {
                if is_ack && seg.ack == self.snd_nxt_of(&key) {
                    new_state = TcpState::FinWait2;
                    let sock = self.conns.get_mut(&key).unwrap();
                    sock.snd_una = sock.snd_nxt;
                    new_ack = true;
                    newly_acked_bytes = 1;
                }
                if is_fin {
                    rcv_nxt_advance += 1;
                    new_state = TcpState::TimeWait;
                    reply_ack = true;
                }
            }
            TcpState::FinWait2 => {
                if is_fin {
                    rcv_nxt_advance += 1;
                    new_state = TcpState::TimeWait;
                    reply_ack = true;
                }
            }
            TcpState::CloseWait => {
                if is_fin { reply_ack = true; }
            }
            TcpState::LastAck => {
                if is_ack && seg.ack == self.snd_nxt_of(&key) {
                    new_state = TcpState::Closed;
                }
            }
            TcpState::TimeWait => {}
            _ => {}
        }

        // 拥塞控制更新：新确认。
        if new_ack {
            if let Some(conn) = self.conns.get_mut(&key) {
                conn.congest.on_new_ack(newly_acked_bytes);
                conn.congest.dup_acks = 0;
            }
        } else if dup_ack {
            if let Some(conn) = self.conns.get_mut(&key) {
                if conn.congest.on_duplicate_ack() {
                    // 快重传：重传未确认数据的首个段。
                    if let Some(sock) = self.conns.get(&key) {
                        if !sock.send_buf.is_empty() {
                            let seg_len = sock.send_buf.len().min(DEFAULT_MSS);
                            let chunk: Vec<u8> = sock.send_buf.iter().take(seg_len).cloned().collect();
                            out.push(TcpSegment::new(sock.key.local_port, sock.remote_port, sock.snd_una,
                                                     sock.rcv_nxt, TCP_ACK | TCP_PSH,
                                                     sock.rcv_window(), chunk));
                            self.stats.segments_sent += 1;
                            self.stats.segments_retransmitted += 1;
                        }
                    }
                }
            }
        }

        if rcv_nxt_advance != 0 {
            if let Some(sock) = self.conns.get_mut(&key) {
                let n = rcv_nxt_advance as usize;
                let data = &seg.payload[..n];
                sock.recv_buf.extend(data.iter().cloned());
                sock.rcv_nxt = sock.rcv_nxt.wrapping_add(rcv_nxt_advance);
            }
        }

        if new_state != state {
            if let Some(sock) = self.conns.get_mut(&key) {
                sock.state = new_state;
                // 状态变更重置重传定时器。
                sock.rto_deadline = Instant::now() + sock.rto;
                sock.retransmissions = 0;
            }
        }

        if reply_ack {
            if let Some(sock) = self.conns.get(&key) {
                out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.snd_nxt,
                                         sock.rcv_nxt, TCP_ACK, sock.rcv_window(), Vec::new()));
                self.stats.segments_sent += 1;
            }
        }

        Ok(out)
    }

    fn snd_nxt_of(&self, key: &TcpConnKey) -> u32 {
        self.conns.get(key).map(|c| c.snd_nxt).unwrap_or(0)
    }

    /// 对已确认序号进行滑动窗口收缩，更新拥塞控制。
    fn ack_received(&mut self, key: &TcpConnKey, ack: u32, window: u16,
                    new_ack: &mut bool, newly_acked_bytes: &mut u32, dup_ack: &mut bool) {
        let sock = match self.conns.get_mut(key) {
            Some(s) => s,
            None => return,
        };
        sock.snd_wnd = window;
        let newly = ack.wrapping_sub(sock.snd_una) as usize;
        if newly > 0 && newly <= sock.send_buf.len() {
            sock.snd_una = ack;
            for _ in 0..newly { sock.send_buf.pop_front(); }
            *new_ack = true;
            *newly_acked_bytes = newly as u32;
            // RTT 测量：若被确认序号覆盖了 rtt_seq，则采样 RTT。
            if let Some(seq) = sock.rtt_seq {
                if ack > seq || ack.wrapping_sub(seq) < (u32::MAX >> 1) {
                    if let Some(start) = sock.rtt_start.take() {
                        sock.congest.record_rtt(Instant::now().saturating_sub(start).as_micros() as u32);
                        sock.rtt_seq = None;
                    }
                }
            }
            // 超时定时器重置。
            sock.rto_deadline = Instant::now() + sock.rto;
            sock.retransmissions = 0;
        } else if newly == 0 && ack == sock.snd_una {
            // 重复 ACK（未前进）。
            *dup_ack = true;
        }
    }

    /// 向已连接 socket 发送数据（受拥塞窗口 + 对端窗口限制），返回待发送段列表。
    pub fn send(&mut self, key: &TcpConnKey, data: &[u8]) -> Result<Vec<TcpSegment>, String> {
        let sock = self.conns.get_mut(key).ok_or_else(|| "socket 不存在".to_string())?;
        if sock.state != TcpState::Established {
            return Err(format!("socket 未建立 (state={})", sock.state.name()));
        }
        sock.send_buf.extend(data.iter().cloned());

        let mut out = Vec::new();
        let window = sock.sendable();
        while !sock.send_buf.is_empty() && window > 0 {
            let seg_len = sock.send_buf.len().min(DEFAULT_MSS).min(window);
            let chunk: Vec<u8> = sock.send_buf.iter().take(seg_len).cloned().collect();
            let seg = TcpSegment::new(sock.key.local_port, sock.remote_port, sock.snd_nxt,
                                      sock.rcv_nxt, TCP_ACK | TCP_PSH,
                                      sock.rcv_window(), chunk);
            sock.snd_nxt = sock.snd_nxt.wrapping_add(seg_len as u32);
            sock.last_sent = Some(Instant::now());
            sock.rtt_seq = Some(sock.snd_nxt.wrapping_sub(1)); // 记录飞行序号用于 RTT
            sock.rtt_start = Some(Instant::now());
            out.push(seg);
            self.stats.segments_sent += 1;
        }
        Ok(out)
    }

    /// 从 socket 接收缓冲读取最多 len 字节。
    pub fn recv(&mut self, key: &TcpConnKey, len: usize) -> Result<Vec<u8>, String> {
        let sock = self.conns.get_mut(key).ok_or_else(|| "socket 不存在".to_string())?;
        let mut data = Vec::new();
        for _ in 0..len.min(sock.recv_buf.len()) {
            data.push(sock.recv_buf.pop_front().unwrap());
        }
        Ok(data)
    }

    /// 监听 socket 接收一个已完成握手的子连接。
    pub fn accept(&mut self, listener_port: u16) -> Result<(TcpConnKey, [u8; 4], u16), String> {
        let listener_key = TcpConnKey { local_port: listener_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let listener = self.conns.get_mut(&listener_key)
            .ok_or_else(|| format!("监听 socket 不存在 (port={})", listener_port))?;
        if listener.state != TcpState::Listen {
            return Err("socket 未监听".to_string());
        }
        let child = listener.accept_queue.pop_front()
            .ok_or_else(|| "没有等待中的连接".to_string())?;
        let remote = (child.remote_ip, child.remote_port);
        let key = TcpConnKey { local_port: listener_port, remote_ip: child.remote_ip, remote_port: child.remote_port };
        if !self.conns.contains_key(&key) {
            self.conns.insert(key.clone(), child);
        }
        Ok((key, remote.0, remote.1))
    }

    /// 应用层 close：发送 FIN。Established -> FinWait1；CloseWait -> LastAck。
    pub fn close(&mut self, key: &TcpConnKey) -> Result<Vec<TcpSegment>, String> {
        let state = self.conns.get(key).map(|c| c.state).ok_or_else(|| "socket 不存在".to_string())?;
        let mut out = Vec::new();
        match state {
            TcpState::Established | TcpState::CloseWait => {
                let (seq, ack, local_port, remote_port, rcv_wnd) = {
                    let sock = self.conns.get(key).unwrap();
                    (sock.snd_nxt, sock.rcv_nxt, sock.key.local_port, sock.remote_port, sock.rcv_window())
                };
                out.push(TcpSegment::new(local_port, remote_port, seq, ack, TCP_FIN | TCP_ACK, rcv_wnd, Vec::new()));
                if let Some(s) = self.conns.get_mut(key) {
                    s.snd_nxt = s.snd_nxt.wrapping_add(1); // FIN 占一个序号
                    s.state = match state {
                        TcpState::Established => TcpState::FinWait1,
                        _ => TcpState::LastAck,
                    };
                    s.last_sent = Some(Instant::now());
                    s.rto_deadline = Instant::now() + s.rto;
                    s.rtt_seq = Some(s.snd_nxt.wrapping_sub(1));
                    s.rtt_start = Some(Instant::now());
                }
                self.stats.segments_sent += 1;
            }
            _ => return Err(format!("无法关闭 (state={})", state.name())),
        }
        Ok(out)
    }

    /// 超时重传驱动：检查所有连接的 RTO 定时器，超时则重传未确认数据并进入慢启动。
    pub fn handle_timeouts(&mut self, now: Instant) -> Vec<(TcpConnKey, Vec<TcpSegment>)> {
        let mut out = Vec::new();
        for (key, conn) in self.conns.iter_mut() {
            let est = matches!(conn.state, TcpState::Established | TcpState::SynSent | TcpState::SynReceived | TcpState::FinWait1 | TcpState::FinWait2 | TcpState::LastAck | TcpState::CloseWait);
            if !est { continue; }
            if now < conn.rto_deadline { continue; }
            if conn.send_buf.is_empty() && conn.state != TcpState::SynSent { continue; }

            // 超时：拥塞控制重置。
            conn.congest.on_timeout();
            conn.retransmissions = conn.retransmissions.saturating_add(1);
            conn.rto = conn.rto.saturating_mul(2); // 指数退避，上限 60s。
            if conn.rto > Duration::from_secs(60) { conn.rto = Duration::from_secs(60); }
            conn.rto_deadline = now + conn.rto;
            conn.last_sent = Some(now);
            self.stats.timeouts += 1;

            // 重传未确认数据的首个段。
            let seg_len = conn.send_buf.len().min(DEFAULT_MSS);
            if seg_len > 0 || conn.state == TcpState::SynSent {
                let chunk: Vec<u8> = conn.send_buf.iter().take(seg_len).cloned().collect();
                let flags = if conn.state == TcpState::SynSent { TCP_SYN } else { TCP_ACK | TCP_PSH };
                let seg = TcpSegment::new(conn.key.local_port, conn.remote_port, conn.snd_una,
                                          conn.rcv_nxt, flags, conn.rcv_window(), chunk);
                out.push((key.clone(), vec![seg]));
                self.stats.segments_retransmitted += 1;
            }
        }
        out
    }

    /// 定时器驱动（每秒调用一次）：推进超时检查。返回需重传的 (key, 段) 列表。
    pub fn tick(&mut self, now: Instant) -> Vec<(TcpConnKey, Vec<TcpSegment>)> {
        self.handle_timeouts(now)
    }

    /// 查询连接状态。
    pub fn state_of(&self, key: &TcpConnKey) -> Option<TcpState> {
        self.conns.get(key).map(|c| c.state)
    }

    /// 查询连接的拥塞窗口。
    pub fn congestion_window(&self, key: &TcpConnKey) -> Option<u32> {
        self.conns.get(key).map(|c| c.congest.cwnd)
    }

    /// 当前活跃连接数（非 Listen）。
    pub fn active_count(&self) -> u64 {
        self.conns.values().filter(|c| c.state != TcpState::Closed && c.state != TcpState::Listen).count() as u64
    }
    /// 状态快照（供 `sense net/tcp_stack`）。
    pub fn status(&self) -> String {
        let mut lines: Vec<String> = self.conns.values().map(|c| {
            format!("  {} state={} cwnd={} ssthresh={} snd_una={} snd_nxt={} rcv_nxt={} rto={}ms retrans={}",
                c.key.local_port, c.state.name(), c.congest.cwnd, c.congest.ssthresh,
                c.snd_una, c.snd_nxt, c.rcv_nxt, c.rto.as_millis(), c.retransmissions)
        }).collect();
        lines.sort();
        format!(
            "tcp_stack our_ip={} alg={:?} conns={} listen={} active={} segments_sent={} segments_received={} retransmitted={} timeouts={}\n{}",
            crate::net::fmt_ip(self.our_ip), self.default_alg.name(),
            self.conns.len(), self.stats.listen_sockets, self.active_count(),
            self.stats.segments_sent, self.stats.segments_received,
            self.stats.segments_retransmitted, self.stats.timeouts,
            lines.join("\n")
        )
    }
}

// =======================================================================
// sense/act 接口（net/tcp_stack）
// =======================================================================

/// 查询 TCP 协议栈状态快照（供 `sense net/tcp_stack`）。
pub fn tcp_stack_sense_snapshot() -> String {
    let st = TcpStack::stack().lock();
    st.status()
}

/// 对 TCP 协议栈通道执行行动（供 `act net/tcp_stack`）。
/// 负载：`stats` / `conns` / `connect <port> <ip> <rport>` / `send <port> <hexdata>` / `close <port>`。
pub fn tcp_stack_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(tcp_stack_sense_snapshot()),
        Some("conns") => {
            let st = TcpStack::stack().lock();
            Ok(st.status())
        }
        Some("connect") => {
            let port: u16 = it
                .next()
                .ok_or("用法: connect <local_port> <remote_ip> <remote_port>")?
                .parse()
                .map_err(|e: core::num::ParseIntError| e.to_string())?;
            let ip_s = it.next().ok_or("缺少 remote_ip")?;
            let ip = crate::net::parse_ip(ip_s);
            let rport: u16 = it
                .next()
                .ok_or("缺少 remote_port")?
                .parse()
                .map_err(|e: core::num::ParseIntError| e.to_string())?;
            let mut st = TcpStack::stack().lock();
            let segs = st.connect(1, port, ip, rport)?;
            let segs_len = segs.len();
            // 环回驱动握手：把发出的段再送回本栈处理（模拟对端回应）。
            for _ in 0..8 {
                let mut progressed = false;
                let segs_clone: Vec<TcpSegment> = segs.iter().cloned().collect();
                for seg in &segs_clone {
                    let bytes = seg.serialize(st.our_ip, ip);
                    let our_ip = st.our_ip;
                    if let Ok(replies) = st.process_segment(ip, our_ip, &bytes) {
                        if !replies.is_empty() { progressed = true; }
                    }
                }
                if !progressed { break; }
            }
            Ok(format!("tcp_connect port={} ip={}:{} segs={}", port, crate::net::fmt_ip(ip), rport, segs_len))
        }
        Some("send") => {
            let port: u16 = it
                .next()
                .ok_or("用法: send <local_port> <hex数据>")?
                .parse()
                .map_err(|e: core::num::ParseIntError| e.to_string())?;
            let hex = it.next().ok_or("缺少 hex 数据")?;
            let data = hex_decode(hex)?;
            let mut st = TcpStack::stack().lock();
            // 找到该端口的已建立连接（任意对端）。
            let key = st
                .conns
                .iter()
                .find(|(_, c)| c.key.local_port == port && c.state == TcpState::Established)
                .map(|(k, _)| k.clone())
                .ok_or_else(|| format!("端口 {} 无已建立连接", port))?;
            let segs = st.send(&key, &data)?;
            Ok(format!("tcp_send port={} bytes={} segs={}", port, data.len(), segs.len()))
        }
        Some("close") => {
            let port: u16 = it
                .next()
                .ok_or("用法: close <local_port>")?
                .parse()
                .map_err(|e: core::num::ParseIntError| e.to_string())?;
            let mut st = TcpStack::stack().lock();
            let key = st.conns.iter().find(|(_, c)| c.key.local_port == port).map(|(k, _)| k.clone())
                .ok_or_else(|| format!("端口 {} 无连接", port))?;
            let segs = st.close(&key)?;
            Ok(format!("tcp_close port={} segs={}", port, segs.len()))
        }
        _ => Err("用法: act net/tcp_stack stats | conns | connect <port> <ip> <rport> | send <port> <hex> | close <port>".to_string()),
    }
}

/// 十六进制字符串解码为字节。
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    if bytes.len() % 2 != 0 { return Err("hex 数据长度必须为偶数".to_string()); }
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("无效 hex 字符: {}", b as char)),
    }
}

// =======================================================================
// 单元测试
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ip() -> [u8; 4] { [10, 0, 0, 2] }
    fn peer_ip() -> [u8; 4] { [10, 0, 0, 3] }

    fn make_stack() -> TcpStack { TcpStack::new(test_ip()) }

    fn listener_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: [0, 0, 0, 0], remote_port: 0 }
    }
    fn child_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: peer_ip(), remote_port: 50001 }
    }
    fn client_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: peer_ip(), remote_port: 8080 }
    }

    /// 三次握手：client SYN -> server SYN-ACK -> client ACK。
    #[test]
    fn three_way_handshake() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        assert_eq!(syn.len(), 1);
        assert_eq!(syn[0].flags & TCP_SYN, TCP_SYN);
        assert_eq!(syn[0].seq, 1000 + 50001);

        // server 端收到 SYN。
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let out = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & (TCP_SYN | TCP_ACK), TCP_SYN | TCP_ACK);
        assert_eq!(out[0].ack, syn[0].seq.wrapping_add(1));
        assert_eq!(st.state_of(&listener_key(8080)).unwrap(), TcpState::Listen);
        let ck = child_key(8080);
        assert_eq!(st.state_of(&ck).unwrap(), TcpState::SynReceived);

        // client 收到 SYN-ACK -> 回 ACK，进入 Established。
        let synack = out[0].clone();
        let out = st.process_segment(test_ip(), peer_ip(), &synack.serialize(test_ip(), peer_ip())).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & TCP_ACK, TCP_ACK);
        assert_eq!(out[0].ack, synack.seq.wrapping_add(1));
        assert_eq!(st.state_of(&client_key(50001)).unwrap(), TcpState::Established);

        // server 收到 ACK -> Established。
        let out = st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();
        assert_eq!(out.len(), 0);
        assert_eq!(st.state_of(&ck).unwrap(), TcpState::Established);
    }

    #[test]
    fn send_and_recv_with_congestion_window() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = client_key(50001);
        // client 发送 "hello"，受 cwnd 限制（初始 10*MSS，足够大）。
        let segs = st.send(&ck, b"hello").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(&segs[0].payload, b"hello");
        assert_eq!(segs[0].seq, (1000 + 50001) + 1); // SYN 占 1 字节
        assert_eq!(segs[0].ack, (1000 + 8080) + 1);

        // server 端收到数据。
        let data_bytes = segs[0].serialize(test_ip(), peer_ip());
        let out = st.process_segment(peer_ip(), test_ip(), &data_bytes).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & TCP_ACK, TCP_ACK);
        assert_eq!(out[0].ack, segs[0].seq.wrapping_add(5));
        let recv = st.recv(&child_key(8080), 64).unwrap();
        assert_eq!(recv, b"hello");
    }

    #[test]
    fn congestion_window_limits_send() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = client_key(50001);
        // 白箱：把对端通告窗口压到 3 字节，验证滑动窗口限制。
        if let Some(s) = st.conns.get_mut(&ck) {
            s.snd_wnd = 3;
        }
        let segs = st.send(&ck, b"abcdefghij").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].payload.len(), 3);
        assert_eq!(&segs[0].payload, b"abc");
        // 窗口耗尽，再发不再出段。
        let segs2 = st.send(&ck, b"more").unwrap();
        assert_eq!(segs2.len(), 0);

        // 对端确认前 3 字节，窗口滚动。
        let ack_seg = TcpSegment::new(8080, 50001, (1000 + 8080) + 1,
                                      (1000 + 50001) + 1 + 3, TCP_ACK, 3, Vec::new());
        let ack_bytes = ack_seg.serialize(peer_ip(), test_ip());
        let out = st.process_segment(peer_ip(), test_ip(), &ack_bytes).unwrap();
        assert_eq!(out.len(), 0); // 纯 ACK 不产生回应
        let segs3 = st.send(&ck, b"more").unwrap();
        assert_eq!(segs3.len(), 1);
        assert_eq!(&segs3[0].payload, b"def");
    }

    #[test]
    fn fast_retransmit_on_duplicate_acks() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = client_key(50001);
        // client 发送两段 "aaaa" / "bbbb"。
        let segs1 = st.send(&ck, b"aaaa").unwrap();
        let segs2 = st.send(&ck, b"bbbb").unwrap();
        assert_eq!(segs1.len(), 1);
        assert_eq!(segs2.len(), 1);
        let s1_seq = segs1[0].seq;
        let _s2_seq = segs2[0].seq;

        // server 只收到第一段，重复 ACK 三次（对 s1_seq+4）。
        let d1 = segs1[0].serialize(test_ip(), peer_ip());
        st.process_segment(peer_ip(), test_ip(), &d1).unwrap();
        let ack_of_s1 = TcpSegment::new(8080, 50001, s1_seq.wrapping_add(4), s1_seq.wrapping_add(4), TCP_ACK, 65535, Vec::new());
        let ack_bytes = ack_of_s1.serialize(peer_ip(), test_ip());
        for _ in 0..3 {
            let _ = st.process_segment(peer_ip(), test_ip(), &ack_bytes).unwrap();
        }
        let conn = st.conns.get(&ck).unwrap();
        assert!(conn.congest.dup_acks >= 3);
        assert_eq!(conn.congest.state, CongestState::FastRecovery);
    }

    #[test]
    fn timeout_resets_to_slow_start() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = client_key(50001);
        let conn = st.conns.get(&ck).unwrap();
        let _initial_cwnd = conn.congest.cwnd;
        let initial_rto = conn.rto;
        let _ = conn;
        // 伪造超时：把 rto_deadline 设为过去，发送一些数据占据 send_buf。
        if let Some(c) = st.conns.get_mut(&ck) {
            c.send_buf.extend(b"timeout data".iter().cloned());
            c.rto_deadline = Instant::now() - Duration::from_secs(1);
        }
        let _retrans = st.tick(Instant::now());
        let conn = st.conns.get(&ck).unwrap();
        assert_eq!(conn.congest.state, CongestState::SlowStart);
        assert_eq!(conn.congest.cwnd, mss_for_test()); // 超时后 cwnd 回到 1 个 MSS
        assert!(conn.rto > initial_rto); // 指数退避
        assert_eq!(conn.retransmissions, 1);
    }

    #[test]
    fn close_exchanges_fin() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = child_key(8080);
        let fin_segs = st.close(&ck).unwrap();
        assert_eq!(fin_segs.len(), 1);
        assert_eq!(fin_segs[0].flags & TCP_FIN, TCP_FIN);
        assert_eq!(st.state_of(&ck).unwrap(), TcpState::FinWait1);

        // 对端 ACK 我们的 FIN。
        let fin_seq = fin_segs[0].seq;
        let ack_of_fin = TcpSegment::new(8080, 50001, (1000 + 8080) + 1,
                                         fin_seq.wrapping_add(1), TCP_ACK, 65535, Vec::new());
        let out = st.process_segment(peer_ip(), test_ip(), &ack_of_fin.serialize(peer_ip(), test_ip())).unwrap();
        assert_eq!(st.state_of(&ck).unwrap(), TcpState::FinWait2);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn accept_returns_child_connection() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();
        st.bind(1, 50001).unwrap();

        let syn = st.connect(1, 50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = st.process_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = st.process_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        st.process_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let (key, rip, rport) = st.accept(8080).unwrap();
        assert_eq!(rip, peer_ip());
        assert_eq!(rport, 50001);
        assert_eq!(key.local_port, 8080);
        assert_eq!(st.state_of(&key).unwrap(), TcpState::Established);
    }

    #[test]
    fn segment_checksum_rejects_corruption() {
        let mut st = make_stack();
        st.bind(1, 8080).unwrap();
        st.listen(8080).unwrap();

        let syn = st.connect(1, 50002, peer_ip(), 8080).unwrap();
        let mut bytes = syn[0].serialize(peer_ip(), test_ip());
        bytes[4] ^= 0xFF; // 破坏序列号但不重算校验和
        let out = st.process_segment(peer_ip(), test_ip(), &bytes).unwrap();
        assert!(out.is_empty()); // 校验不通过应被丢弃
    }

    #[test]
    fn bind_privileged_port_requires_cap() {
        let mut st = make_stack();
        // 80 号端口需 CAP_NET_BIND，默认 caps 具备全部能力 -> 成功。
        assert!(st.bind(1, 80).is_ok());
    }

    #[test]
    fn congestion_reno_and_cubic_names() {
        assert_eq!(CongestAlg::Reno.name(), "Reno");
        assert_eq!(CongestAlg::Cubic.name(), "Cubic");
        assert_eq!(CongestState::SlowStart.name(), "SLOW_START");
        assert_eq!(CongestState::CongestionAvoidance.name(), "CONGESTION_AVOIDANCE");
        assert_eq!(CongestState::FastRecovery.name(), "FAST_RECOVERY");
        let mut cc = CongestionController::new(CongestAlg::Cubic);
        assert_eq!(cc.alg, CongestAlg::Cubic);
        cc.on_timeout();
        assert_eq!(cc.state, CongestState::SlowStart);
    }

    #[test]
    fn sense_and_act() {
        let snap = tcp_stack_sense_snapshot();
        assert!(snap.contains("tcp_stack"));
        assert!(tcp_stack_act_raw("stats").is_ok());
        assert!(tcp_stack_act_raw("conns").is_ok());
    }

    fn mss_for_test() -> u32 { DEFAULT_MSS as u32 }
}
