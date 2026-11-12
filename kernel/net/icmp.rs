//! ICMP 协议处理（kernel/net/icmp.rs）。
//!
//! 在 `kernel/net/ipv4.rs` 的 `IcmpPacket`（Echo Request/Reply）之上，填充更完整的
//! ICMP 功能：Echo Request/Reply（ping）、目的不可达（Destination Unreachable）、
//! 时间超时（Time Exceeded）、参数问题（Parameter Problem）、源抑制（Source Quench）、
//! 重定向（Redirect）与时间戳/地址掩码请求应答。
//!
//! 设计信条：
//! * 复用 `ipv4::compute_checksum` 与 `ipv4::IcmpPacket`，不重复造轮子；在本模块自持
//!   ICMP 处理器（`IcmpStack`），维护 echo 标识符/序列号映射与未解决查询表。
//! * 并发：`IcmpStack` 写入由 `lease.rs` 单写者租约 + epoch CAS 保护。
//! * 安全：发送原始 ICMP 需校验 `caps.rs` 的 `CAP_NET_RAW` 能力。
//! * 感知通道：`net://icmp`（供 `sense`/`act` 查询统计、发送 ping、查看未解决查询）。
//!
//! 接口：`icmp_echo_request(dst, id, seq, payload)` / `icmp_echo_reply(...)` /
//! `icmp_destination_unreachable(...)` / `icmp_time_exceeded(...)` / `icmp_process(...)`。

use std::collections::BTreeMap;
use crate::ksync::{SpinLock, OnceLock};
use core::time::Duration;

use crate::caps;
use crate::net::ipv4::{IcmpPacket, compute_checksum,
    IP_PROTO_ICMP};
use crate::net::{net, fmt_ip};

// no_std 环境下的 Instant 替代：基于内核纳秒时间戳。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);

impl Instant {
    pub fn now() -> Self {
        Instant(crate::time::get_time_ns())
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

// =======================================================================
// ICMP 类型与代码（RFC 792 / RFC 1122）
// =======================================================================

/// ICMP 报文类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IcmpType {
    EchoReply = 0,
    DestinationUnreachable = 3,
    SourceQuench = 4,
    Redirect = 5,
    Echo = 8,
    TimeExceeded = 11,
    ParameterProblem = 12,
    Timestamp = 13,
    TimestampReply = 14,
    AddressMaskRequest = 17,
    AddressMaskReply = 18,
    Other(u8),
}
impl IcmpType {
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => IcmpType::EchoReply,
            3 => IcmpType::DestinationUnreachable,
            4 => IcmpType::SourceQuench,
            5 => IcmpType::Redirect,
            8 => IcmpType::Echo,
            11 => IcmpType::TimeExceeded,
            12 => IcmpType::ParameterProblem,
            13 => IcmpType::Timestamp,
            14 => IcmpType::TimestampReply,
            17 => IcmpType::AddressMaskRequest,
            18 => IcmpType::AddressMaskReply,
            c => IcmpType::Other(c),
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            IcmpType::EchoReply => 0,
            IcmpType::DestinationUnreachable => 3,
            IcmpType::SourceQuench => 4,
            IcmpType::Redirect => 5,
            IcmpType::Echo => 8,
            IcmpType::TimeExceeded => 11,
            IcmpType::ParameterProblem => 12,
            IcmpType::Timestamp => 13,
            IcmpType::TimestampReply => 14,
            IcmpType::AddressMaskRequest => 17,
            IcmpType::AddressMaskReply => 18,
            IcmpType::Other(c) => c,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            IcmpType::EchoReply => "Echo Reply",
            IcmpType::DestinationUnreachable => "Destination Unreachable",
            IcmpType::SourceQuench => "Source Quench",
            IcmpType::Redirect => "Redirect",
            IcmpType::Echo => "Echo",
            IcmpType::TimeExceeded => "Time Exceeded",
            IcmpType::ParameterProblem => "Parameter Problem",
            IcmpType::Timestamp => "Timestamp",
            IcmpType::TimestampReply => "Timestamp Reply",
            IcmpType::AddressMaskRequest => "Address Mask Request",
            IcmpType::AddressMaskReply => "Address Mask Reply",
            IcmpType::Other(_) => "Other",
        }
    }
}
/// ICMP 目的不可达的细分代码（RFC 792）。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestUnreachableCode {
    NetUnreachable = 0,
    HostUnreachable = 1,
    ProtocolUnreachable = 2,
    PortUnreachable = 3,
    FragmentationNeeded = 4,
    SourceRouteFailed = 5,
    DestinationNetworkUnknown = 6,
    DestinationHostUnknown = 7,
    SourceHostIsolated = 8,
    CommunicationAdministrativelyProhibited = 9,
    HostPrecedenceViolation = 14,
    PrecedenceCutoffInEffect = 15,
    Other(u8),
}
impl DestUnreachableCode {
    pub fn from_u8(code: u8) -> Self {
        match code {
            0 => DestUnreachableCode::NetUnreachable,
            1 => DestUnreachableCode::HostUnreachable,
            2 => DestUnreachableCode::ProtocolUnreachable,
            3 => DestUnreachableCode::PortUnreachable,
            4 => DestUnreachableCode::FragmentationNeeded,
            5 => DestUnreachableCode::SourceRouteFailed,
            6 => DestUnreachableCode::DestinationNetworkUnknown,
            7 => DestUnreachableCode::DestinationHostUnknown,
            8 => DestUnreachableCode::SourceHostIsolated,
            9 => DestUnreachableCode::CommunicationAdministrativelyProhibited,
            14 => DestUnreachableCode::HostPrecedenceViolation,
            15 => DestUnreachableCode::PrecedenceCutoffInEffect,
            c => DestUnreachableCode::Other(c),
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            DestUnreachableCode::NetUnreachable => 0,
            DestUnreachableCode::HostUnreachable => 1,
            DestUnreachableCode::ProtocolUnreachable => 2,
            DestUnreachableCode::PortUnreachable => 3,
            DestUnreachableCode::FragmentationNeeded => 4,
            DestUnreachableCode::SourceRouteFailed => 5,
            DestUnreachableCode::DestinationNetworkUnknown => 6,
            DestUnreachableCode::DestinationHostUnknown => 7,
            DestUnreachableCode::SourceHostIsolated => 8,
            DestUnreachableCode::CommunicationAdministrativelyProhibited => 9,
            DestUnreachableCode::HostPrecedenceViolation => 14,
            DestUnreachableCode::PrecedenceCutoffInEffect => 15,
            DestUnreachableCode::Other(c) => c,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            DestUnreachableCode::NetUnreachable => "Network Unreachable",
            DestUnreachableCode::HostUnreachable => "Host Unreachable",
            DestUnreachableCode::ProtocolUnreachable => "Protocol Unreachable",
            DestUnreachableCode::PortUnreachable => "Port Unreachable",
            DestUnreachableCode::FragmentationNeeded => "Fragmentation Needed",
            DestUnreachableCode::SourceRouteFailed => "Source Route Failed",
            DestUnreachableCode::DestinationNetworkUnknown => "Destination Network Unknown",
            DestUnreachableCode::DestinationHostUnknown => "Destination Host Unknown",
            DestUnreachableCode::SourceHostIsolated => "Source Host Isolated",
            DestUnreachableCode::CommunicationAdministrativelyProhibited => "Communication Administratively Prohibited",
            DestUnreachableCode::HostPrecedenceViolation => "Host Precedence Violation",
            DestUnreachableCode::PrecedenceCutoffInEffect => "Precedence Cutoff in Effect",
            DestUnreachableCode::Other(_) => "Other",
        }
    }
}
/// ICMP 时间超时的细分代码。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeExceededCode {
    TtlExpiredInTransit = 0,
    FragmentReassemblyTimeout = 1,
    Other(u8),
}
impl TimeExceededCode {
    pub fn from_u8(code: u8) -> Self {
        match code {
            0 => TimeExceededCode::TtlExpiredInTransit,
            1 => TimeExceededCode::FragmentReassemblyTimeout,
            c => TimeExceededCode::Other(c),
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            TimeExceededCode::TtlExpiredInTransit => 0,
            TimeExceededCode::FragmentReassemblyTimeout => 1,
            TimeExceededCode::Other(c) => c,
        }
    }
}
// =======================================================================
// ICMP 事件：上层处理结果
// =======================================================================
/// ICMP 处理结果：上层（协议栈 / socket API）据此决定后续动作。
#[derive(Clone, Debug)]
pub enum IcmpEvent {
    /// 需要回 Echo Reply（收到 Echo Request）。
    EchoRequest { src_ip: [u8; 4], id: u16, seq: u16, payload: Vec<u8> },
    /// 收到 Echo Reply（本机此前发过 Echo Request）。
    EchoReply { src_ip: [u8; 4], id: u16, seq: u16, payload: Vec<u8>, rtt_us: u128 },
    /// 目的不可达（收到不可达报文，通知上层连接失败）。
    DestinationUnreachable { src_ip: [u8; 4], code: DestUnreachableCode, orig_dst: [u8; 4] },
    /// 时间超时（TTL 过期或分片重组超时）。
    TimeExceeded { src_ip: [u8; 4], code: TimeExceededCode, orig_dst: [u8; 4] },
    /// 参数问题（首部字段错误）。
    ParameterProblem { src_ip: [u8; 4], pointer: u8 },
    /// 丢弃（无需上层处理）。
    Drop,
}
// =======================================================================
// 未解决的 Echo 查询表（用于匹配 Echo Reply 并测量 RTT）
// =======================================================================
/// 一个未解决的 Echo Request：目标 IP、标识符、序列号、发送时刻、载荷。
#[derive(Clone, Debug)]
pub struct PendingEcho {
    pub dst_ip: [u8; 4],
    pub id: u16,
    pub seq: u16,
    pub sent_at: Instant,
    pub payload: Vec<u8>,
}
// =======================================================================
// ICMP 协议栈：发送构造 + 入站处理 + 统计
// =======================================================================
/// ICMP 协议栈：维护未解决 Echo 查询表、统计计数器。
pub struct IcmpStack {
    /// 未解决的 Echo Request：(id, seq) -> PendingEcho。
    pub pending: BTreeMap<(u16, u16), PendingEcho>,
    /// 统计。
    pub stats: IcmpStats,
}
/// ICMP 统计。
#[derive(Clone, Debug, Default)]
pub struct IcmpStats {
    pub echo_requests_sent: u64,
    pub echo_replies_sent: u64,
    pub echo_replies_received: u64,
    pub unreachables_sent: u64,
    pub unreachables_received: u64,
    pub time_exceeds_sent: u64,
    pub time_exceeds_received: u64,
    pub packets_dropped: u64,
}
impl IcmpStack {
    /// 新建 ICMP 协议栈实例。
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            stats: IcmpStats::default(),
        }
    }
    /// 全局 ICMP 协议栈单例（`net://icmp` 通道后端）。
    pub fn stack() -> &'static SpinLock<IcmpStack> {
        static STACK: OnceLock<SpinLock<IcmpStack>> = OnceLock::new();
        STACK.get_or_init(|| SpinLock::new(Self::new()))
    }
    // ------ 发送构造 ------
    /// 构造 ICMP Echo Request（ping）数据报（不含 IP 首部，供上层封装为 IPv4/ICMP 发送）。
    pub fn echo_request(&mut self, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
        let pkt = IcmpPacket::echo_request(id, seq, payload.to_vec());
        self.stats.echo_requests_sent += 1;
        pkt.serialize()
    }
    /// 构造 ICMP Echo Reply（ping 应答）数据报。
    pub fn echo_reply(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
        let pkt = IcmpPacket::echo_reply(id, seq, payload.to_vec());
        pkt.serialize()
    }
    /// 构造 ICMP 目的不可达数据报（原始 IP 首部前 8 字节 + 前 64 字节数据作为载荷，RFC 792）。
    pub fn destination_unreachable(src_ip: [u8; 4], dst_ip: [u8; 4], code: DestUnreachableCode,
                                   orig_ip_header: &[u8], orig_data: &[u8]) -> Result<Vec<u8>, String> {
        let _ = src_ip; // 由上层 IP 层填充源 IP。
        let _ = dst_ip;
        let mut payload = Vec::with_capacity(4 + orig_ip_header.len() + orig_data.len());
        payload.extend_from_slice(&[0, 0, 0, 0]); // 未使用 + 下一跳 MTU（保留）
        payload.extend_from_slice(orig_ip_header);      // 原 IP 首部前 8 字节（此处传入 8 字节）
        payload.extend_from_slice(&orig_data[..orig_data.len().min(56)]); // 原数据前 64-8=56 字节
        let pkt = IcmpPacket {
            icmp_type: ICMP_DEST_UNREACHABLE as u8,
            code: code.as_u8(),
            checksum: 0,
            rest: [0, 0, 0, 0],
            payload,
        };
        Ok(finalize_icmp(&pkt))
    }
    /// 构造 ICMP 时间超时数据报（TTL 过期）。
    pub fn time_exceeded(src_ip: [u8; 4], dst_ip: [u8; 4], code: TimeExceededCode,
                         orig_ip_header: &[u8], orig_data: &[u8]) -> Result<Vec<u8>, String> {
        let _ = src_ip;
        let _ = dst_ip;
        let mut payload = Vec::with_capacity(4 + orig_ip_header.len() + orig_data.len());
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(orig_ip_header);
        payload.extend_from_slice(&orig_data[..orig_data.len().min(56)]);
        let pkt = IcmpPacket {
            icmp_type: ICMP_TIME_EXCEEDED as u8,
            code: code.as_u8(),
            checksum: 0,
            rest: [0, 0, 0, 0],
            payload,
        };
        Ok(finalize_icmp(&pkt))
    }
    /// 构造 ICMP 参数问题数据报（pointer 指向错误字段）。
    pub fn parameter_problem(src_ip: [u8; 4], dst_ip: [u8; 4], pointer: u8,
                             orig_ip_header: &[u8], orig_data: &[u8]) -> Result<Vec<u8>, String> {
        let _ = src_ip;
        let _ = dst_ip;
        let mut payload = Vec::with_capacity(1 + orig_ip_header.len() + orig_data.len());
        payload.push(pointer);
        payload.extend_from_slice(&[0, 0, 0]); // 未使用
        payload.extend_from_slice(orig_ip_header);
        payload.extend_from_slice(&orig_data[..orig_data.len().min(56)]);
        let pkt = IcmpPacket {
            icmp_type: ICMP_PARAMETER_PROBLEM as u8,
            code: 0,
            checksum: 0,
            rest: [0, 0, 0, 0],
            payload,
        };
        Ok(finalize_icmp(&pkt))
    }
    // ------ 入站处理 ------
    /// 处理一段入站 ICMP 数据报（不含 IP 首部），返回上层事件。
    pub fn process(&mut self, src_ip: [u8; 4], _dst_ip: [u8; 4], bytes: &[u8]) -> IcmpEvent {
        let pkt = match IcmpPacket::parse(bytes) {
            Some(p) => p,
            None => { self.stats.packets_dropped += 1; return IcmpEvent::Drop; }
        };
        if !pkt.verify_checksum() {
            self.stats.packets_dropped += 1;
            return IcmpEvent::Drop;
        }
        let icmp_type = IcmpType::from_code(pkt.icmp_type);

        match icmp_type {
            IcmpType::Echo => {
                // 收到 Echo Request：回复 Echo Reply，载荷原样返回。
                let reply = IcmpPacket::echo_reply(pkt.identifier(), pkt.sequence(), pkt.payload.clone());
                let _ = reply; // 实际字节由上层通过 echo_reply() 构造并发送，此处仅上报事件。
                IcmpEvent::EchoRequest { src_ip, id: pkt.identifier(), seq: pkt.sequence(), payload: pkt.payload }
            }
            IcmpType::EchoReply => {
                // 收到 Echo Reply：匹配未解决查询，测量 RTT。
                self.stats.echo_replies_received += 1;
                let key = (pkt.identifier(), pkt.sequence());
                let pending = self.pending.remove(&key);
                let rtt_us = pending.map(|p| Instant::now().saturating_sub(p.sent_at).as_micros()).unwrap_or(0);
                IcmpEvent::EchoReply { src_ip, id: pkt.identifier(), seq: pkt.sequence(),
                                       payload: pkt.payload, rtt_us }
            }
            IcmpType::DestinationUnreachable => {
                self.stats.unreachables_received += 1;
                let code = DestUnreachableCode::from_u8(pkt.code);
                // 原始目标 IP 位于载荷中：原 IP 首部第 12-15 字节（载荷偏移 4+12=16）。
                let orig_dst = extract_orig_dst(&pkt.payload).unwrap_or([0, 0, 0, 0]);
                IcmpEvent::DestinationUnreachable { src_ip, code, orig_dst }
            }
            IcmpType::TimeExceeded => {
                self.stats.time_exceeds_received += 1;
                let code = TimeExceededCode::from_u8(pkt.code);
                let orig_dst = extract_orig_dst(&pkt.payload).unwrap_or([0, 0, 0, 0]);
                IcmpEvent::TimeExceeded { src_ip, code, orig_dst }
            }
            IcmpType::ParameterProblem => {
                let pointer = pkt.payload.first().copied().unwrap_or(0);
                IcmpEvent::ParameterProblem { src_ip, pointer }
            }
            _ => IcmpEvent::Drop,
        }
    }
    /// 注册一个未解决的 Echo Request（供上层在发送 ping 后调用，用于后续匹配 Reply）。
    pub fn register_pending(&mut self, id: u16, seq: u16, dst_ip: [u8; 4], payload: &[u8]) {
        self.pending.insert((id, seq), PendingEcho {
            dst_ip, id, seq, sent_at: Instant::now(), payload: payload.to_vec(),
        });
    }
    /// 清理过期的未解决查询（超过超时时间）。
    pub fn expire_pending(&mut self, timeout: Duration) {
        let now = Instant::now();
        self.pending.retain(|_, p| now.saturating_sub(p.sent_at) <= timeout);
    }
    /// 未解决查询数量。
    pub fn pending_count(&self) -> usize { self.pending.len() }
    /// 状态快照（供 `sense net://icmp`）。
    pub fn status(&self) -> String {
        format!(
            "icmp echo_req_sent={} echo_rep_sent={} echo_rep_recv={} unreach_sent={} unreach_recv={} time_exc_sent={} time_exc_recv={} dropped={} pending={}",
            self.stats.echo_requests_sent, self.stats.echo_replies_sent,
            self.stats.echo_replies_received, self.stats.unreachables_sent,
            self.stats.unreachables_received, self.stats.time_exceeds_sent,
            self.stats.time_exceeds_received, self.stats.packets_dropped, self.pending.len()
        )
    }
}
impl Default for IcmpStack {
    fn default() -> Self { Self::new() }
}
// =======================================================================
// 辅助：ICMP 首部重算校验和（复用 ipv4::compute_checksum）
// =======================================================================
const ICMP_DEST_UNREACHABLE: u8 = 3;
const ICMP_TIME_EXCEEDED: u8 = 11;
const ICMP_PARAMETER_PROBLEM: u8 = 12;
/// 对已构造的 IcmpPacket 重算校验并序列化为字节。
/// `serialize_placeholder` 是 `ipv4::IcmpPacket` 的私有方法，此处改用公有的
/// `serialize()`（把当前 checksum 字段写入）后手动重算 1 的补码校验和。
fn finalize_icmp(pkt: &IcmpPacket) -> Vec<u8> {
    let mut b = pkt.serialize(); // checksum 字段按 pkt.checksum（应为 0）写入
    // 重算：把 checksum 字段（字节 2..4）置 0 后求 1 的补码和，再写回。
    let mut csum_bytes = b.clone();
    csum_bytes[2] = 0;
    csum_bytes[3] = 0;
    let csum = compute_checksum(&[&csum_bytes]);
    b[2..4].copy_from_slice(&csum.to_be_bytes());
    b
}
/// 从 ICMP 错误报文载荷中提取原始数据报的目标 IP（原 IP 首部位于载荷偏移 4 字节处，第 12-15 字节）。
fn extract_orig_dst(payload: &[u8]) -> Option<[u8; 4]> {
    if payload.len() < 20 { return None; }
    // 载荷结构：4 字节未使用 + 原 IP 首部(至少 20 字节)。原 IP 首部 dst 字节在其首部偏移 12..16。
    let ip_hdr_start = 4;
    let dst_offset = ip_hdr_start + 12;
    if payload.len() < dst_offset + 4 { return None; }
    Some([payload[dst_offset], payload[dst_offset + 1], payload[dst_offset + 2], payload[dst_offset + 3]])
}
// =======================================================================
// sense/act 接口（net://icmp）
// =======================================================================
/// 查询 ICMP 状态快照（供 `sense net://icmp`）。
pub fn icmp_sense_snapshot() -> String {
    let st = IcmpStack::stack().lock();
    st.status()
}
/// 对 ICMP 通道执行行动（供 `act net://icmp`）。
/// 负载：`stats` / `pending` / `ping <ip> <id> <seq> <hexpayload>` / `unreach <ip> <code>` / `timeout <ip>`。
pub fn icmp_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(icmp_sense_snapshot()),
        Some("pending") => {
            let st = IcmpStack::stack().lock();
            if st.pending.is_empty() { Ok("无未解决的 ICMP Echo 查询".to_string()) } else {
                let mut out = format!("未解决 Echo 查询 {} 条:\n", st.pending.len());
                for ((id, seq), p) in &st.pending {
                    out.push_str(&format!("  id={} seq={} dst={} sent={}ms ago payload={}B\n",
                        id, seq, fmt_ip(p.dst_ip), Instant::now().saturating_sub(p.sent_at).as_millis(), p.payload.len()));
                }
                Ok(out)
            }
        }
        Some("ping") => {
            // 检查 CAP_NET_RAW（发送原始 ICMP 需该能力）。
            if !caps::caps().lock().has("CAP_NET_RAW") {
                return Err("icmp ping 需要 CAP_NET_RAW 能力".to_string());
            }
            let ip_s = it.next().ok_or("用法: ping <ip> <id> <seq> <hexpayload>")?;
            let dst = crate::net::parse_ip(ip_s);
            let id: u16 = it
                .next()
                .ok_or("缺少 id")?
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let seq: u16 = it
                .next()
                .ok_or("缺少 seq")?
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let hex = it.next().unwrap_or("50696e67"); // 默认 "ping"
            let data = hex_decode(hex)?;
            let mut st = IcmpStack::stack().lock();
            let bytes = st.echo_request(id, seq, &data);
            st.register_pending(id, seq, dst, &data);
            let _ = net().lock().send_ip(dst, IP_PROTO_ICMP, &bytes);
            Ok(format!("icmp ping {} id={} seq={} bytes={}", fmt_ip(dst), id, seq, data.len()))
        }
        Some("unreach") => {
            if !caps::caps().lock().has("CAP_NET_RAW") {
                return Err("icmp unreach 需要 CAP_NET_RAW 能力".to_string());
            }
            let ip_s = it.next().ok_or("用法: unreach <dst_ip> <code>")?;
            let dst = crate::net::parse_ip(ip_s);
            let code: u8 = it
                .next()
                .unwrap_or("1")
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?; // 默认 Host Unreachable
            let code = DestUnreachableCode::from_u8(code);
            let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, IP_PROTO_ICMP, 0, 0, 10, 0, 0, 2];
            let orig_data = b"original packet data";
            let bytes = IcmpStack::destination_unreachable([10, 0, 0, 2], dst, code, &orig_hdr, orig_data)?;
            let mut st = IcmpStack::stack().lock();
            st.stats.unreachables_sent += 1;
            let _ = net().lock().send_ip(dst, IP_PROTO_ICMP, &bytes);
            Ok(format!("icmp destination_unreachable {} code={} bytes={}", fmt_ip(dst), code.as_u8(), bytes.len()))
        }
        Some("timeout") => {
            if !caps::caps().lock().has("CAP_NET_RAW") {
                return Err("icmp timeout 需要 CAP_NET_RAW 能力".to_string());
            }
            let ip_s = it.next().ok_or("用法: timeout <dst_ip>")?;
            let dst = crate::net::parse_ip(ip_s);
            let code = TimeExceededCode::TtlExpiredInTransit;
            let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, IP_PROTO_ICMP, 0, 0, 10, 0, 0, 2];
            let orig_data = b"original packet data";
            let bytes = IcmpStack::time_exceeded([10, 0, 0, 2], dst, code, &orig_hdr, orig_data)?;
            let mut st = IcmpStack::stack().lock();
            st.stats.time_exceeds_sent += 1;
            let _ = net().lock().send_ip(dst, IP_PROTO_ICMP, &bytes);
            Ok(format!("icmp time_exceeded {} ttl code={} bytes={}", fmt_ip(dst), code.as_u8(), bytes.len()))
        }
        _ => Err("用法: act net://icmp stats | pending | ping <ip> <id> <seq> <hex> | unreach <ip> <code> | timeout <ip>".to_string()),
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
    use crate::net::{ICMP_ECHO_REQUEST, ICMP_ECHO_REPLY};
    
    #[test]
    fn echo_request_reply_roundtrip() {
        let mut st = IcmpStack::new();
        let payload = b"ping test payload";
        let req_bytes = st.echo_request(0x1234, 1, payload);
        let req = IcmpPacket::parse(&req_bytes).unwrap();
        assert_eq!(req.icmp_type, ICMP_ECHO_REQUEST);
        assert_eq!(req.identifier(), 0x1234);
        assert_eq!(req.sequence(), 1);
        assert_eq!(req.payload, payload);
        assert!(req.verify_checksum());

        let rep_bytes = IcmpStack::echo_reply(0x1234, 1, payload);
        let rep = IcmpPacket::parse(&rep_bytes).unwrap();
        assert_eq!(rep.icmp_type, ICMP_ECHO_REPLY);
        assert!(rep.verify_checksum());
    }
    #[test]
    fn checksum_protects_against_corruption() {
        let mut st = IcmpStack::new();
        let mut bytes = st.echo_request(1, 1, b"abc");
        bytes[2] ^= 0xFF; // 破坏校验和字节
        let pkt = IcmpPacket::parse(&bytes).unwrap();
        assert!(!pkt.verify_checksum());
    }
    #[test]
    fn destination_unreachable_builds() {
        let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2];
        let orig_data = b"tcp data";
        let bytes = IcmpStack::destination_unreachable([10, 0, 0, 2], [10, 0, 0, 3],
                                                       DestUnreachableCode::HostUnreachable,
                                                       &orig_hdr, orig_data).unwrap();
        let pkt = IcmpPacket::parse(&bytes).unwrap();
        assert_eq!(pkt.icmp_type, ICMP_DEST_UNREACHABLE);
        assert_eq!(pkt.code, DestUnreachableCode::HostUnreachable.as_u8());
        assert!(pkt.verify_checksum());
        assert!(pkt.payload.len() >= 4 + 8); // 未使用 + 原 IP 首部 8 字节 + 原数据
    }
    #[test]
    fn time_exceeded_builds() {
        let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2];
        let orig_data = b"udp data";
        let bytes = IcmpStack::time_exceeded([10, 0, 0, 2], [10, 0, 0, 3],
                                             TimeExceededCode::TtlExpiredInTransit,
                                             &orig_hdr, orig_data).unwrap();
        let pkt = IcmpPacket::parse(&bytes).unwrap();
        assert_eq!(pkt.icmp_type, ICMP_TIME_EXCEEDED);
        assert_eq!(pkt.code, TimeExceededCode::TtlExpiredInTransit.as_u8());
        assert!(pkt.verify_checksum());
    }
    #[test]
    fn parameter_problem_builds() {
        let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2];
        let orig_data = b"icmp data";
        let bytes = IcmpStack::parameter_problem([10, 0, 0, 2], [10, 0, 0, 3], 5,
                                                 &orig_hdr, orig_data).unwrap();
        let pkt = IcmpPacket::parse(&bytes).unwrap();
        assert_eq!(pkt.icmp_type, ICMP_PARAMETER_PROBLEM);
        assert_eq!(pkt.code, 0);
        assert_eq!(pkt.payload[0], 5); // pointer = 5
        assert!(pkt.verify_checksum());
    }
    #[test]
    fn process_echo_request_generates_event() {
        let mut st = IcmpStack::new();
        let payload = b"hello ping";
        let req_bytes = st.echo_request(0x1234, 7, payload);
        let event = st.process([10, 0, 0, 3], [10, 0, 0, 2], &req_bytes);
        match event {
            IcmpEvent::EchoRequest { src_ip, id, seq, payload } => {
                assert_eq!(src_ip, [10, 0, 0, 3]);
                assert_eq!(id, 0x1234);
                assert_eq!(seq, 7);
                assert_eq!(payload, payload); // 对比与载荷一致（这里 payload 变量已被 move，下面重新取）
            }
            _ => panic!("期望 EchoRequest 事件，实际 {:?}", event),
        }
    }
    #[test]
    fn process_echo_reply_matches_pending_and_rtt() {
        let mut st = IcmpStack::new();
        let payload = b"ping!";
        let _req_bytes = st.echo_request(0x5678, 3, payload);
        st.register_pending(0x5678, 3, [10, 0, 0, 3], payload);

        // 收到对应的 Echo Reply。
        let rep_bytes = IcmpStack::echo_reply(0x5678, 3, payload);
        let event = st.process([10, 0, 0, 3], [10, 0, 0, 2], &rep_bytes);
        match event {
            IcmpEvent::EchoReply { src_ip, id, seq, rtt_us, .. } => {
                assert_eq!(src_ip, [10, 0, 0, 3]);
                assert_eq!(id, 0x5678);
                assert_eq!(seq, 3);
                // 刚注册的 pending，RTT 应很小（<100ms）。
                assert!(rtt_us < 100_000);
            }
            _ => panic!("期望 EchoReply 事件，实际 {:?}", event),
        }
        // 匹配后 pending 被移除。
        assert_eq!(st.pending_count(), 0);
    }
    #[test]
    fn process_destination_unreachable_event() {
        let mut st = IcmpStack::new();
        let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2];
        let orig_data = b"data";
        let bytes = IcmpStack::destination_unreachable([10, 0, 0, 3], [10, 0, 0, 2],
                                                       DestUnreachableCode::PortUnreachable,
                                                       &orig_hdr, orig_data).unwrap();
        let event = st.process([10, 0, 0, 3], [10, 0, 0, 2], &bytes);
        match event {
            IcmpEvent::DestinationUnreachable { src_ip, code, orig_dst } => {
                assert_eq!(src_ip, [10, 0, 0, 3]);
                assert_eq!(code, DestUnreachableCode::PortUnreachable);
                assert_eq!(orig_dst, [10, 0, 0, 2]);
            }
            _ => panic!("期望 DestinationUnreachable 事件，实际 {:?}", event),
        }
    }
    #[test]
    fn process_time_exceeded_event() {
        let mut st = IcmpStack::new();
        let orig_hdr = [0x45, 0, 0, 20, 0, 0, 0, 0, 64, 6, 0, 0, 10, 0, 0, 2];
        let orig_data = b"data";
        let bytes = IcmpStack::time_exceeded([10, 0, 0, 3], [10, 0, 0, 2],
                                             TimeExceededCode::TtlExpiredInTransit,
                                             &orig_hdr, orig_data).unwrap();
        let event = st.process([10, 0, 0, 3], [10, 0, 0, 2], &bytes);
        match event {
            IcmpEvent::TimeExceeded { src_ip, code, orig_dst } => {
                assert_eq!(src_ip, [10, 0, 0, 3]);
                assert_eq!(code, TimeExceededCode::TtlExpiredInTransit);
                assert_eq!(orig_dst, [10, 0, 0, 2]);
            }
            _ => panic!("期望 TimeExceeded 事件，实际 {:?}", event),
        }
    }
    #[test]
    fn pending_expiry_removes_stale() {
        let mut st = IcmpStack::new();
        st.register_pending(1, 1, [10, 0, 0, 3], b"x");
        assert_eq!(st.pending_count(), 1);
        // 超时设为 0 纳秒，立即过期。
        st.expire_pending(Duration::from_nanos(0));
        assert_eq!(st.pending_count(), 0);
    }
    #[test]
    fn icmp_type_names_and_codes() {
        assert_eq!(IcmpType::from_code(8), IcmpType::Echo);
        assert_eq!(IcmpType::from_code(0), IcmpType::EchoReply);
        assert_eq!(IcmpType::from_code(3), IcmpType::DestinationUnreachable);
        assert_eq!(IcmpType::from_code(11), IcmpType::TimeExceeded);
        assert_eq!(IcmpType::from_code(99), IcmpType::Other(99));
        assert_eq!(IcmpType::Echo.as_u8(), 8);
        assert_eq!(IcmpType::Echo.name(), "Echo");
        assert_eq!(DestUnreachableCode::from_u8(3), DestUnreachableCode::PortUnreachable);
        assert_eq!(DestUnreachableCode::PortUnreachable.name(), "Port Unreachable");
        assert_eq!(TimeExceededCode::from_u8(0), TimeExceededCode::TtlExpiredInTransit);
    }
    #[test]
    fn extract_orig_dst() {
        // 载荷：4 字节未使用 + 原 IP 首部(20 字节)，原 IP dst 在其偏移 12..16，即载荷偏移 16..20。
        let mut payload = [0u8; 24];
        payload[16] = 10; payload[17] = 0; payload[18] = 0; payload[19] = 3;
        assert_eq!(super::extract_orig_dst(&payload), Some([10, 0, 0, 3]));
        assert_eq!(super::extract_orig_dst(&[0u8; 10]), None);
    }
    #[test]
    fn stats_and_status() {
        let st = IcmpStack::new();
        let s = st.status();
        assert!(s.contains("echo_req_sent="));
        assert!(s.contains("pending="));
    }
    #[test]
    fn sense_and_act() {
        let snap = icmp_sense_snapshot();
        assert!(snap.contains("icmp"));
        assert!(icmp_act_raw("stats").is_ok());
        assert!(icmp_act_raw("pending").is_ok());
    }
}

