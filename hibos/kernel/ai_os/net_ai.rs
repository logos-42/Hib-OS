//! AI 原生网络层 —— AI 原生 OS 的「智能网络」 (AI-Native Network)
//!
//! 本模块实现 AI 原生 OS 的网络栈，对应传统 OS 中的
//! 「TCP/IP 协议栈 / 套接字 / 路由 / 防火墙」概念，但重新映射到 AI 域：
//!
//! * SemanticPacket (语义数据包) —— 带语义标签的网络包，等价于传统网络包；
//! * IntentRoute (意图路由) —— 基于通信意图的路由决策，等价于 IP 路由；
//! * AgentSocket (智能体套接字) —— 智能体间通信端点，等价于 TCP/UDP 套接字；
//! * TrafficAwareScheduler (流量感知调度) —— 基于流量模式的包调度，等价于 QoS；
//! * SemanticFirewall (语义防火墙) —— 基于语义内容的包过滤，等价于包过滤防火墙；
//! * NetworkTopology (网络拓扑) —— 智能体网络拓扑图，等价于网络拓扑；
//! * BandwidthManager (带宽管理器) —— 动态带宽分配，等价于流量整形；
//! * LatencyMonitor (延迟监控) —— 端到端延迟监测，等价于 ping / traceroute；
//! * CongestionControl (拥塞控制) —— 基于 AI 的拥塞避免，等价于 TCP 拥塞控制；
//! * NetAI (AI 原生网络引擎) —— 统一入口，通过 net_ai://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无真实网络依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 语义数据包 (SemanticPacket)
// =====================================================================================

/// 数据包类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    Data,       // 数据
    Control,    // 控制
    Heartbeat,  // 心跳
    Semantic,   // 语义语义包
    Broadcast,  // 广播
}

/// 语义数据包：带语义标签的网络包。
#[derive(Clone, Debug)]
pub struct SemanticPacket {
    pub id: u64,
    pub src_agent: u64,
    pub dst_agent: Option<u64>,
    pub r#type: PacketType,
    pub payload: Vec<u8>,
    pub semantic_tag: Option<String>,
    pub priority: f32,       // 0~1
    pub ttl: u32,
    pub timestamp_ns: u128,
    pub size_bytes: u64,
}

impl SemanticPacket {
    pub fn new(src_agent: u64, dst_agent: Option<u64>, r#type: PacketType, payload: &[u8]) -> Self {
        let now = crate::ai_os::native::now_ns();
        Self {
            id: 0,
            src_agent,
            dst_agent,
            r#type,
            payload: payload.to_vec(),
            semantic_tag: None,
            priority: 0.5,
            ttl: 64,
            timestamp_ns: now,
            size_bytes: payload.len() as u64,
        }
    }

    pub fn with_semantic_tag(mut self, tag: &str) -> Self {
        self.semantic_tag = Some(tag.to_string());
        self
    }

    pub fn with_priority(mut self, p: f32) -> Self {
        self.priority = p.clamp(0.0, 1.0);
        self
    }

    pub fn is_expired(&self, now_ns: u128) -> bool {
        now_ns - self.timestamp_ns > (self.ttl as u128) * 1_000_000_000
    }
}

// =====================================================================================
// 意图路由 (IntentRoute)
// =====================================================================================

/// 路由条目。
#[derive(Clone, Debug)]
pub struct RouteEntry {
    pub dst_agent: u64,
    pub next_hop: u64,
    pub cost: f32,
    pub metric: String,
    pub updated_at: u128,
}

/// 意图路由：基于通信意图的路由决策。
pub struct IntentRouter {
    routes: BTreeMap<u64, RouteEntry>,
    pub default_gateway: Option<u64>,
}

impl IntentRouter {
    pub fn new() -> Self {
        Self {
            routes: BTreeMap::new(),
            default_gateway: None,
        }
    }

    pub fn add_route(&mut self, dst: u64, next_hop: u64, cost: f32, metric: &str) {
        self.routes.insert(
            dst,
            RouteEntry {
                dst_agent: dst,
                next_hop,
                cost: cost.clamp(0.0, 1.0),
                metric: metric.to_string(),
                updated_at: crate::ai_os::native::now_ns(),
            },
        );
    }

    pub fn lookup(&self, dst: u64) -> Option<&RouteEntry> {
        self.routes.get(&dst).or_else(|| {
            self.default_gateway.and_then(|gw| self.routes.get(&gw))
        })
    }

    pub fn route_by_intent(&self, intent: &str) -> Option<u64> {
        // 简化：根据意图关键词匹配路由
        for (dst, route) in &self.routes {
            if route.metric.contains(intent) {
                return Some(*dst);
            }
        }
        self.default_gateway
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

// =====================================================================================
// 智能体套接字 (AgentSocket)
// =====================================================================================

/// 套接字状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketState {
    Closed,
    Listening,
    Connected,
    Connecting,
    Closing,
}

/// 智能体套接字：智能体间通信端点。
#[derive(Clone, Debug)]
pub struct AgentSocket {
    pub fd: u64,
    pub local_agent: u64,
    pub remote_agent: Option<u64>,
    pub state: SocketState,
    pub local_port: u16,
    pub remote_port: Option<u16>,
    pub rx_buffer: VecDeque<SemanticPacket>,
    pub tx_buffer: VecDeque<SemanticPacket>,
    pub created_at: u128,
}

impl AgentSocket {
    pub fn new(fd: u64, local_agent: u64, local_port: u16) -> Self {
        Self {
            fd,
            local_agent,
            remote_agent: None,
            state: SocketState::Closed,
            local_port,
            remote_port: None,
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
            created_at: crate::ai_os::native::now_ns(),
        }
    }

    pub fn connect(&mut self, remote_agent: u64, remote_port: u16) {
        self.remote_agent = Some(remote_agent);
        self.remote_port = Some(remote_port);
        self.state = SocketState::Connected;
    }

    pub fn listen(&mut self) {
        self.state = SocketState::Listening;
    }

    pub fn send(&mut self, packet: SemanticPacket) {
        self.tx_buffer.push_back(packet);
    }

    pub fn recv(&mut self) -> Option<SemanticPacket> {
        self.rx_buffer.pop_front()
    }

    pub fn close(&mut self) {
        self.state = SocketState::Closed;
        self.remote_agent = None;
        self.remote_port = None;
        self.rx_buffer.clear();
        self.tx_buffer.clear();
    }
}

// =====================================================================================
// 流量感知调度 (TrafficAwareScheduler)
// =====================================================================================

/// 流量类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrafficClass {
    RealTime,    // 实时
    Interactive, // 交互
    Bulk,        // 批量
    Background,  // 后台
}

/// 流量感知调度：基于流量模式的包调度。
pub struct TrafficAwareScheduler {
    queues: BTreeMap<TrafficClass, VecDeque<SemanticPacket>>,
    pub weights: BTreeMap<TrafficClass, f32>,
    pub scheduled_count: u64,
    pub dropped_count: u64,
}

impl TrafficAwareScheduler {
    pub fn new() -> Self {
        let mut weights = BTreeMap::new();
        weights.insert(TrafficClass::RealTime, 1.0);
        weights.insert(TrafficClass::Interactive, 0.8);
        weights.insert(TrafficClass::Bulk, 0.5);
        weights.insert(TrafficClass::Background, 0.2);
        Self {
            queues: BTreeMap::new(),
            weights,
            scheduled_count: 0,
            dropped_count: 0,
        }
    }

    pub fn enqueue(&mut self, packet: SemanticPacket, traffic_class: TrafficClass) {
        self.queues.entry(traffic_class).or_default().push_back(packet);
    }

    pub fn dequeue(&mut self) -> Option<SemanticPacket> {
        // 按权重轮询
        let classes = [
            TrafficClass::RealTime,
            TrafficClass::Interactive,
            TrafficClass::Bulk,
            TrafficClass::Background,
        ];
        for class in &classes {
            if let Some(packet) = self.queues.get_mut(class).and_then(|q| q.pop_front()) {
                self.scheduled_count += 1;
                return Some(packet);
            }
        }
        None
    }

    pub fn drop_if_full(&mut self, class: TrafficClass, max_size: usize) {
        if let Some(q) = self.queues.get_mut(&class) {
            while q.len() > max_size {
                q.pop_front();
                self.dropped_count += 1;
            }
        }
    }

    pub fn queue_len(&self, class: TrafficClass) -> usize {
        self.queues.get(&class).map(|q| q.len()).unwrap_or(0)
    }
}

// =====================================================================================
// 语义防火墙 (SemanticFirewall)
// =====================================================================================

/// 规则动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallAction {
    Accept,
    Drop,
    Reject,
}

/// 防火墙规则。
#[derive(Clone, Debug)]
pub struct FirewallRule {
    pub id: u64,
    pub src_agent: Option<u64>,
    pub dst_agent: Option<u64>,
    pub semantic_tag: Option<String>,
    pub action: FirewallAction,
    pub enabled: bool,
}

/// 语义防火墙：基于语义内容的包过滤。
pub struct SemanticFirewall {
    rules: BTreeMap<u64, FirewallRule>,
    next_id: u64,
    pub accept_count: u64,
    pub drop_count: u64,
}

impl SemanticFirewall {
    pub fn new() -> Self {
        Self {
            rules: BTreeMap::new(),
            next_id: 1,
            accept_count: 0,
            drop_count: 0,
        }
    }

    pub fn add_rule(&mut self, src: Option<u64>, dst: Option<u64>, tag: Option<String>, action: FirewallAction) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.rules.insert(
            id,
            FirewallRule {
                id,
                src_agent: src,
                dst_agent: dst,
                semantic_tag: tag,
                action,
                enabled: true,
            },
        );
        id
    }

    pub fn check(&self, packet: &SemanticPacket) -> FirewallAction {
        for rule in self.rules.values() {
            if !rule.enabled {
                continue;
            }
            if let Some(src) = rule.src_agent {
                if packet.src_agent != src {
                    continue;
                }
            }
            if let Some(dst) = rule.dst_agent {
                if packet.dst_agent != Some(dst) {
                    continue;
                }
            }
            if let Some(tag) = &rule.semantic_tag {
                if packet.semantic_tag.as_ref() != Some(tag) {
                    continue;
                }
            }
            return rule.action;
        }
        FirewallAction::Accept // 默认允许
    }

    pub fn filter(&mut self, packet: &SemanticPacket) -> bool {
        match self.check(packet) {
            FirewallAction::Accept => {
                self.accept_count += 1;
                true
            }
            FirewallAction::Drop | FirewallAction::Reject => {
                self.drop_count += 1;
                false
            }
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

// =====================================================================================
// 网络拓扑 (NetworkTopology)
// =====================================================================================

/// 链路状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Up,
    Down,
    Degraded,
}

/// 拓扑边。
#[derive(Clone, Debug)]
pub struct TopologyEdge {
    pub from: u64,
    pub to: u64,
    pub state: LinkState,
    pub bandwidth: f64,      // Mbps
    pub latency_ms: f64,
}

/// 网络拓扑：智能体网络拓扑图。
pub struct NetworkTopology {
    edges: BTreeMap<(u64, u64), TopologyEdge>,
    nodes: BTreeSet<u64>,
}

impl NetworkTopology {
    pub fn new() -> Self {
        Self {
            edges: BTreeMap::new(),
            nodes: BTreeSet::new(),
        }
    }

    pub fn add_node(&mut self, agent: u64) {
        self.nodes.insert(agent);
    }

    pub fn add_edge(&mut self, from: u64, to: u64, state: LinkState, bandwidth: f64, latency_ms: f64) {
        self.nodes.insert(from);
        self.nodes.insert(to);
        self.edges.insert(
            (from, to),
            TopologyEdge {
                from,
                to,
                state,
                bandwidth,
                latency_ms,
            },
        );
    }

    pub fn get_edge(&self, from: u64, to: u64) -> Option<&TopologyEdge> {
        self.edges.get(&(from, to))
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn avg_latency(&self) -> f64 {
        if self.edges.is_empty() {
            0.0
        } else {
            self.edges.values().map(|e| e.latency_ms).sum::<f64>() / self.edges.len() as f64
        }
    }
}

// =====================================================================================
// 带宽管理器 (BandwidthManager)
// =====================================================================================

/// 带宽分配。
#[derive(Clone, Debug)]
pub struct BandwidthAllocation {
    pub agent: u64,
    pub allocated_mbps: f64,
    pub used_mbps: f64,
}

/// 带宽管理器：动态带宽分配。
pub struct BandwidthManager {
    allocations: BTreeMap<u64, BandwidthAllocation>,
    pub total_mbps: f64,
}

impl BandwidthManager {
    pub fn new(total_mbps: f64) -> Self {
        Self {
            allocations: BTreeMap::new(),
            total_mbps,
        }
    }

    pub fn allocate(&mut self, agent: u64, mbps: f64) -> bool {
        let used: f64 = self.allocations.values().map(|a| a.allocated_mbps).sum();
        if used + mbps > self.total_mbps {
            return false;
        }
        self.allocations.insert(
            agent,
            BandwidthAllocation {
                agent,
                allocated_mbps: mbps,
                used_mbps: 0.0,
            },
        );
        true
    }

    pub fn release(&mut self, agent: u64) -> bool {
        self.allocations.remove(&agent).is_some()
    }

    pub fn update_usage(&mut self, agent: u64, mbps: f64) {
        if let Some(alloc) = self.allocations.get_mut(&agent) {
            alloc.used_mbps = mbps.min(alloc.allocated_mbps);
        }
    }

    pub fn allocation_count(&self) -> usize {
        self.allocations.len()
    }

    pub fn utilization(&self) -> f64 {
        if self.total_mbps == 0.0 {
            0.0
        } else {
            let used: f64 = self.allocations.values().map(|a| a.used_mbps).sum();
            used / self.total_mbps
        }
    }
}

// =====================================================================================
// 延迟监控 (LatencyMonitor)
// =====================================================================================

/// 延迟样本。
#[derive(Clone, Debug)]
pub struct LatencySample {
    pub src: u64,
    pub dst: u64,
    pub latency_ms: f64,
    pub timestamp_ns: u128,
}

/// 延迟监控：端到端延迟监测。
pub struct LatencyMonitor {
    samples: VecDeque<LatencySample>,
    max_samples: usize,
    pub avg_latency_ms: f64,
    pub max_latency_ms: f64,
}

impl LatencyMonitor {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
            max_samples: 10000,
            avg_latency_ms: 0.0,
            max_latency_ms: 0.0,
        }
    }

    pub fn record(&mut self, src: u64, dst: u64, latency_ms: f64) {
        self.samples.push_back(LatencySample {
            src,
            dst,
            latency_ms,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        if self.samples.len() > self.max_samples {
            self.samples.pop_front();
        }
        let sum: f64 = self.samples.iter().map(|s| s.latency_ms).sum();
        self.avg_latency_ms = sum / self.samples.len() as f64;
        self.max_latency_ms = self.samples.iter().map(|s| s.latency_ms).fold(0.0f64, f64::max);
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

// =====================================================================================
// 拥塞控制 (CongestionControl)
// =====================================================================================

/// 拥塞状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CongestionState {
    Normal,
    Warning,
    Critical,
}

/// 拥塞控制：基于 AI 的拥塞避免。
pub struct CongestionControl {
    pub state: CongestionState,
    pub threshold_warning: f64,
    pub threshold_critical: f64,
    pub congestion_events: u64,
}

impl CongestionControl {
    pub fn new() -> Self {
        Self {
            state: CongestionState::Normal,
            threshold_warning: 0.7,
            threshold_critical: 0.9,
            congestion_events: 0,
        }
    }

    pub fn update(&mut self, utilization: f64) {
        let new_state = if utilization >= self.threshold_critical {
            CongestionState::Critical
        } else if utilization >= self.threshold_warning {
            CongestionState::Warning
        } else {
            CongestionState::Normal
        };
        if new_state != self.state && new_state == CongestionState::Critical {
            self.congestion_events += 1;
        }
        self.state = new_state;
    }

    pub fn should_throttle(&self) -> bool {
        self.state == CongestionState::Critical
    }
}

// =====================================================================================
// AI 原生网络引擎 (NetAI) —— 统一入口
// =====================================================================================

/// AI 原生网络引擎：整合语义包、意图路由、智能体套接字、流量调度、语义防火墙、
/// 网络拓扑、带宽管理、延迟监控、拥塞控制。
pub struct NetAI {
    pub packets: BTreeMap<u64, SemanticPacket>,
    pub router: IntentRouter,
    pub sockets: BTreeMap<u64, AgentSocket>,
    pub scheduler: TrafficAwareScheduler,
    pub firewall: SemanticFirewall,
    pub topology: NetworkTopology,
    pub bandwidth: BandwidthManager,
    pub latency: LatencyMonitor,
    pub congestion: CongestionControl,
    pub next_packet_id: u64,
    pub next_socket_fd: u64,
    pub enabled: bool,
    pub total_packets: u64,
    pub total_bytes: u64,
}

impl NetAI {
    pub fn new() -> Self {
        Self {
            packets: BTreeMap::new(),
            router: IntentRouter::new(),
            sockets: BTreeMap::new(),
            scheduler: TrafficAwareScheduler::new(),
            firewall: SemanticFirewall::new(),
            topology: NetworkTopology::new(),
            bandwidth: BandwidthManager::new(1000.0),
            latency: LatencyMonitor::new(),
            congestion: CongestionControl::new(),
            next_packet_id: 1,
            next_socket_fd: 1,
            enabled: true,
            total_packets: 0,
            total_bytes: 0,
        }
    }

    pub fn send_packet(&mut self, src: u64, dst: Option<u64>, payload: &[u8], tag: Option<&str>) -> u64 {
        let id = self.next_packet_id;
        self.next_packet_id += 1;
        let mut pkt = SemanticPacket::new(src, dst, PacketType::Data, payload);
        pkt.id = id;
        if let Some(t) = tag {
            pkt = pkt.with_semantic_tag(t);
        }
        self.total_packets += 1;
        self.total_bytes += pkt.size_bytes;
        self.packets.insert(id, pkt);

        // 防火墙检查
        let pkt = self.packets.get(&id).unwrap().clone();
        if !self.firewall.filter(&pkt) {
            // 被丢弃
        }

        // 路由查找
        if let Some(dst) = dst {
            if let Some(route) = self.router.lookup(dst) {
                // 记录延迟
                self.latency.record(src, route.next_hop, route.cost * 100.0);
            }
        }

        id
    }

    pub fn create_socket(&mut self, agent: u64, port: u16) -> u64 {
        let fd = self.next_socket_fd;
        self.next_socket_fd += 1;
        let socket = AgentSocket::new(fd, agent, port);
        self.sockets.insert(fd, socket);
        fd
    }

    pub fn connect_socket(&mut self, fd: u64, remote_agent: u64, remote_port: u16) -> Result<(), String> {
        let socket = self.sockets.get_mut(&fd).ok_or("套接字不存在")?;
        socket.connect(remote_agent, remote_port);
        Ok(())
    }

    pub fn socket_send(&mut self, fd: u64, payload: &[u8]) -> Result<u64, String> {
        let socket = self.sockets.get_mut(&fd).ok_or("套接字不存在")?;
        let pkt = SemanticPacket::new(socket.local_agent, socket.remote_agent, PacketType::Data, payload);
        socket.send(pkt);
        self.total_packets += 1;
        self.total_bytes += payload.len() as u64;
        Ok(payload.len() as u64)
    }

    pub fn socket_recv(&mut self, fd: u64) -> Option<Vec<u8>> {
        let socket = self.sockets.get_mut(&fd)?;
        socket.recv().map(|p| p.payload)
    }

    pub fn add_route(&mut self, dst: u64, next_hop: u64, cost: f32, metric: &str) {
        self.router.add_route(dst, next_hop, cost, metric);
    }

    pub fn add_topology_edge(&mut self, from: u64, to: u64, state: LinkState, bandwidth: f64, latency_ms: f64) {
        self.topology.add_edge(from, to, state, bandwidth, latency_ms);
    }

    pub fn allocate_bandwidth(&mut self, agent: u64, mbps: f64) -> bool {
        self.bandwidth.allocate(agent, mbps)
    }

    pub fn record_latency(&mut self, src: u64, dst: u64, ms: f64) {
        self.latency.record(src, dst, ms);
        self.congestion.update(self.latency.avg_latency_ms / 1000.0);
    }

    pub fn packet_count(&self) -> usize {
        self.packets.len()
    }

    pub fn socket_count(&self) -> usize {
        self.sockets.len()
    }

    pub fn route_count(&self) -> usize {
        self.router.route_count()
    }
}

impl Default for NetAI {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static NET_AI: OnceLock<Mutex<NetAI>> = OnceLock::new();

pub fn net_ai() -> &'static Mutex<NetAI> {
    NET_AI.get_or_init(|| Mutex::new(NetAI::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = net_ai();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = net_ai().lock().unwrap();
    format!(
        "net_ai: packets={} bytes={} sockets={} routes={} topology_nodes={} topology_edges={} bandwidth_alloc={} utilization={:.2} avg_latency_ms={:.2} congestion={:?} firewall_accept={} firewall_drop={}",
        eng.packet_count(),
        eng.total_bytes,
        eng.socket_count(),
        eng.route_count(),
        eng.topology.node_count(),
        eng.topology.edge_count(),
        eng.bandwidth.allocation_count(),
        eng.bandwidth.utilization(),
        eng.latency.avg_latency_ms,
        eng.congestion.state,
        eng.firewall.accept_count,
        eng.firewall.drop_count
    )
}

pub fn net_ai_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn net_ai_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("send") => {
            let src = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少源代理")?;
            let dst_str = it.next().unwrap_or("broadcast");
            let dst = if dst_str == "broadcast" { None } else { Some(dst_str.parse::<u64>().unwrap_or(0)) };
            let tag = it.next();
            let data: Vec<u8> = it.collect::<Vec<&str>>().join(" ").into_bytes();
            if data.is_empty() {
                return Err("缺少数据".into());
            }
            let mut eng = net_ai().lock().unwrap();
            let id = eng.send_packet(src, dst, &data, tag);
            Ok(format!("已发送数据包 id={} 源={} 大小={}B", id, src, data.len()))
        }
        Some("socket") => {
            let agent = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少代理")?;
            let port = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(8080);
            let mut eng = net_ai().lock().unwrap();
            let fd = eng.create_socket(agent, port);
            Ok(format!("已创建套接字 fd={} 代理={} 端口={}", fd, agent, port))
        }
        Some("connect") => {
            let fd = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 fd")?;
            let remote = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少远程代理")?;
            let port = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(8080);
            let mut eng = net_ai().lock().unwrap();
            eng.connect_socket(fd, remote, port)?;
            Ok(format!("套接字 {} 已连接到 {}:{}", fd, remote, port))
        }
        Some("sock-send") => {
            let fd = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 fd")?;
            let data: Vec<u8> = it.collect::<Vec<&str>>().join(" ").into_bytes();
            if data.is_empty() {
                return Err("缺少数据".into());
            }
            let mut eng = net_ai().lock().unwrap();
            let n = eng.socket_send(fd, &data)?;
            Ok(format!("套接字 {} 已发送 {} 字节", fd, n))
        }
        Some("sock-recv") => {
            let fd = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少 fd")?;
            let mut eng = net_ai().lock().unwrap();
            match eng.socket_recv(fd) {
                Some(data) => Ok(format!("套接字 {} 已接收 {} 字节", fd, data.len())),
                None => Ok("无数据".into()),
            }
        }
        Some("route") => {
            let dst = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少目标")?;
            let next_hop = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少下一跳")?;
            let cost = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            let metric = it.next().unwrap_or("default");
            let mut eng = net_ai().lock().unwrap();
            eng.add_route(dst, next_hop, cost, metric);
            Ok(format!("已添加路由 {} -> {} 代价={}", dst, next_hop, cost))
        }
        Some("route-intent") => {
            let intent = it.next().ok_or("缺少意图")?;
            let eng = net_ai().lock().unwrap();
            match eng.router.route_by_intent(intent) {
                Some(dst) => Ok(format!("意图 {} 路由到代理 {}", intent, dst)),
                None => Err(format!("意图 {} 无路由", intent)),
            }
        }
        Some("topology") => {
            let from = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少源")?;
            let to = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少目标")?;
            let state_str = it.next().unwrap_or("up");
            let state = match state_str {
                "up" => LinkState::Up,
                "down" => LinkState::Down,
                "degraded" => LinkState::Degraded,
                _ => LinkState::Up,
            };
            let bw = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(100.0);
            let lat = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(10.0);
            let mut eng = net_ai().lock().unwrap();
            eng.add_topology_edge(from, to, state, bw, lat);
            Ok(format!("已添加拓扑边 {} -> {} 带宽={}Mbps 延迟={}ms", from, to, bw, lat))
        }
        Some("bandwidth") => {
            let agent = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少代理")?;
            let mbps = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(10.0);
            let mut eng = net_ai().lock().unwrap();
            if eng.allocate_bandwidth(agent, mbps) {
                Ok(format!("已为代理 {} 分配带宽 {}Mbps", agent, mbps))
            } else {
                Err("带宽不足".into())
            }
        }
        Some("latency") => {
            let src = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少源")?;
            let dst = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少目标")?;
            let ms = it.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let mut eng = net_ai().lock().unwrap();
            eng.record_latency(src, dst, ms);
            Ok(format!("已记录延迟 {} -> {} {}ms", src, dst, ms))
        }
        Some("firewall") => {
            let src = it.next().and_then(|s| s.parse::<u64>().ok()).ok();
            let dst = it.next().and_then(|s| s.parse::<u64>().ok()).ok();
            let tag = it.next();
            let action_str = it.next().unwrap_or("accept");
            let action = match action_str {
                "accept" => FirewallAction::Accept,
                "drop" => FirewallAction::Drop,
                "reject" => FirewallAction::Reject,
                _ => FirewallAction::Accept,
            };
            let mut eng = net_ai().lock().unwrap();
            let id = eng.firewall.add_rule(src, dst, tag.map(|s| s.to_string()), action);
            Ok(format!("已添加防火墙规则 id={}", id))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 net_ai 动作: {}", other)),
    }
}

pub fn net_ai_act_dispatch(payload: &str) -> Result<String, String> {
    net_ai_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_creation() {
        let pkt = SemanticPacket::new(1, Some(2), PacketType::Data, b"hello");
        assert_eq!(pkt.src_agent, 1);
        assert_eq!(pkt.dst_agent, Some(2));
        assert_eq!(pkt.payload, b"hello");
        assert!(pkt.size_bytes > 0);
    }

    #[test]
    fn packet_semantic_tag() {
        let pkt = SemanticPacket::new(1, None, PacketType::Broadcast, b"test")
            .with_semantic_tag("urgent")
            .with_priority(0.9);
        assert_eq!(pkt.semantic_tag, Some("urgent".into()));
        assert!((pkt.priority - 0.9).abs() < 1e-6);
    }

    #[test]
    fn packet_expired() {
        let mut pkt = SemanticPacket::new(1, None, PacketType::Data, b"test");
        pkt.ttl = 0;
        pkt.timestamp_ns = 0;
        assert!(pkt.is_expired(crate::ai_os::native::now_ns()));
    }

    #[test]
    fn router_lookup() {
        let mut router = IntentRouter::new();
        router.add_route(2, 3, 0.5, "fast_path");
        assert!(router.lookup(2).is_some());
        assert!(router.lookup(999).is_none());
    }

    #[test]
    fn router_default_gateway() {
        let mut router = IntentRouter::new();
        router.default_gateway = Some(10);
        assert_eq!(router.lookup(999).unwrap().dst_agent, 10);
    }

    #[test]
    fn router_intent() {
        let mut router = IntentRouter::new();
        router.add_route(5, 6, 0.3, "video_stream");
        assert_eq!(router.route_by_intent("video"), Some(5));
    }

    #[test]
    fn socket_lifecycle() {
        let mut sock = AgentSocket::new(1, 10, 8080);
        assert_eq!(sock.state, SocketState::Closed);
        sock.listen();
        assert_eq!(sock.state, SocketState::Listening);
        sock.connect(20, 9090);
        assert_eq!(sock.state, SocketState::Connected);
        assert_eq!(sock.remote_agent, Some(20));
        sock.close();
        assert_eq!(sock.state, SocketState::Closed);
    }

    #[test]
    fn socket_send_recv() {
        let mut sock = AgentSocket::new(1, 10, 8080);
        sock.connect(20, 9090);
        let pkt = SemanticPacket::new(10, Some(20), PacketType::Data, b"ping");
        sock.send(pkt);
        let received = sock.recv().unwrap();
        assert_eq!(received.payload, b"ping");
    }

    #[test]
    fn scheduler_enqueue_dequeue() {
        let mut sched = TrafficAwareScheduler::new();
        let pkt = SemanticPacket::new(1, None, PacketType::Data, b"rt");
        sched.enqueue(pkt, TrafficClass::RealTime);
        let dequeued = sched.dequeue().unwrap();
        assert_eq!(dequeued.payload, b"rt");
    }

    #[test]
    fn scheduler_priority() {
        let mut sched = TrafficAwareScheduler::new();
        let pkt_low = SemanticPacket::new(1, None, PacketType::Data, b"low");
        let pkt_high = SemanticPacket::new(1, None, PacketType::Data, b"high");
        sched.enqueue(pkt_low, TrafficClass::Background);
        sched.enqueue(pkt_high, TrafficClass::RealTime);
        let first = sched.dequeue().unwrap();
        assert_eq!(first.payload, b"high");
    }

    #[test]
    fn firewall_check() {
        let mut fw = SemanticFirewall::new();
        fw.add_rule(Some(1), None, Some("urgent".into()), FirewallAction::Drop);
        let pkt = SemanticPacket::new(1, None, PacketType::Data, b"test").with_semantic_tag("urgent");
        assert!(!fw.filter(&pkt));
        assert_eq!(fw.drop_count, 1);
    }

    #[test]
    fn firewall_accept() {
        let mut fw = SemanticFirewall::new();
        let pkt = SemanticPacket::new(1, None, PacketType::Data, b"test");
        assert!(fw.filter(&pkt));
        assert_eq!(fw.accept_count, 1);
    }

    #[test]
    fn topology() {
        let mut topo = NetworkTopology::new();
        topo.add_node(1);
        topo.add_node(2);
        topo.add_edge(1, 2, LinkState::Up, 100.0, 10.0);
        assert_eq!(topo.node_count(), 2);
        assert_eq!(topo.edge_count(), 1);
        assert_eq!(topo.avg_latency(), 10.0);
    }

    #[test]
    fn bandwidth() {
        let mut bw = BandwidthManager::new(100.0);
        assert!(bw.allocate(1, 30.0));
        assert!(bw.allocate(2, 40.0));
        assert!(!bw.allocate(3, 50.0)); // 不足
        assert_eq!(bw.allocation_count(), 2);
        bw.update_usage(1, 15.0);
        assert!((bw.utilization() - 0.55).abs() < 1e-6); // (15+0)/100
    }

    #[test]
    fn congestion_control() {
        let mut cc = CongestionControl::new();
        cc.update(0.5);
        assert_eq!(cc.state, CongestionState::Normal);
        cc.update(0.8);
        assert_eq!(cc.state, CongestionState::Warning);
        cc.update(0.95);
        assert_eq!(cc.state, CongestionState::Critical);
        assert!(cc.should_throttle());
    }

    #[test]
    fn latency_monitor() {
        let mut lat = LatencyMonitor::new();
        lat.record(1, 2, 10.0);
        lat.record(1, 2, 20.0);
        assert_eq!(lat.sample_count(), 2);
        assert!((lat.avg_latency_ms - 15.0).abs() < 1e-6);
        assert_eq!(lat.max_latency_ms, 20.0);
    }

    #[test]
    fn net_ai_send() {
        let mut eng = NetAI::new();
        let id = eng.send_packet(1, Some(2), b"hello", Some("urgent"));
        assert_eq!(id, 1);
        assert_eq!(eng.total_packets, 1);
        assert_eq!(eng.total_bytes, 5);
    }

    #[test]
    fn net_ai_socket() {
        let mut eng = NetAI::new();
        let fd = eng.create_socket(1, 8080);
        assert_eq!(fd, 1);
        eng.connect_socket(fd, 2, 9090).unwrap();
        eng.socket_send(fd, b"data").unwrap();
        let data = eng.socket_recv(fd);
        assert_eq!(data, Some(b"data".to_vec()));
    }

    #[test]
    fn net_ai_route() {
        let mut eng = NetAI::new();
        eng.add_route(10, 20, 0.5, "fast");
        assert_eq!(eng.route_count(), 1);
    }

    #[test]
    fn net_ai_bandwidth() {
        let mut eng = NetAI::new();
        assert!(eng.allocate_bandwidth(1, 50.0));
    }

    #[test]
    fn net_ai_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("net_ai:"));
        assert!(s.contains("packets="));
    }

    #[test]
    fn net_ai_act_send() {
        init();
        let res = net_ai_act("send 1 2 hello urgent").unwrap();
        assert!(res.contains("已发送数据包"));
    }

    #[test]
    fn net_ai_act_socket() {
        init();
        let res = net_ai_act("socket 1 8080").unwrap();
        assert!(res.contains("已创建套接字"));
    }

    #[test]
    fn net_ai_act_route() {
        init();
        let res = net_ai_act("route 10 20 0.5 fast").unwrap();
        assert!(res.contains("已添加路由"));
    }

    #[test]
    fn net_ai_act_topology() {
        init();
        let res = net_ai_act("topology 1 2 up 100 10").unwrap();
        assert!(res.contains("拓扑边"));
    }

    #[test]
    fn net_ai_act_bandwidth() {
        init();
        let res = net_ai_act("bandwidth 1 50").unwrap();
        assert!(res.contains("分配带宽"));
    }

    #[test]
    fn net_ai_act_latency() {
        init();
        let res = net_ai_act("latency 1 2 15.5").unwrap();
        assert!(res.contains("已记录延迟"));
    }

    #[test]
    fn net_ai_act_firewall() {
        init();
        let res = net_ai_act("firewall 1 2 urgent drop").unwrap();
        assert!(res.contains("防火墙规则"));
    }

    #[test]
    fn net_ai_act_status() {
        init();
        let res = net_ai_act("status").unwrap();
        assert!(res.contains("net_ai:"));
    }

    #[test]
    fn packet_types() {
        assert_eq!(PacketType::Data as u32, 0);
        assert_eq!(PacketType::Broadcast as u32, 4);
    }

    #[test]
    fn traffic_classes() {
        assert_eq!(TrafficClass::RealTime as u32, 0);
        assert_eq!(TrafficClass::Background as u32, 3);
    }

    #[test]
    fn link_states() {
        assert_eq!(LinkState::Up as u32, 0);
        assert_eq!(LinkState::Degraded as u32, 2);
    }

    #[test]
    fn firewall_actions() {
        assert_eq!(FirewallAction::Accept as u32, 0);
        assert_eq!(FirewallAction::Reject as u32, 2);
    }

    #[test]
    fn congestion_states() {
        assert_eq!(CongestionState::Normal as u32, 0);
        assert_eq!(CongestionState::Critical as u32, 2);
    }
}

// =====================================================================================
// 网络拓扑分析 (NetworkTopologyAnalysis)
// =====================================================================================

/// 中心性度量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CentralityMeasure {
    Degree,       // 度中心性
    Betweenness,  // 介数中心性
    Closeness,    // 接近中心性
    Eigenvector,  // 特征向量中心性
}

/// 网络拓扑分析器：分析网络结构和关键节点。
pub struct NetworkTopologyAnalysis {
    pub nodes: BTreeSet<u64>,
    pub edges: BTreeMap<(u64, u64), f32>, // (from, to) -> weight
}

impl NetworkTopologyAnalysis {
    pub fn new() -> Self {
        Self {
            nodes: BTreeSet::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: u64) {
        self.nodes.insert(node);
    }

    pub fn add_edge(&mut self, from: u64, to: u64, weight: f32) {
        self.nodes.insert(from);
        self.nodes.insert(to);
        self.edges.insert((from, to), weight.clamp(0.0, 1.0));
    }

    pub fn degree_centrality(&self, node: u64) -> f32 {
        let degree = self.edges.iter().filter(|((a, b), _)| *a == node || *b == node).count();
        let max_degree = self.nodes.len().saturating_sub(1);
        if max_degree == 0 {
            0.0
        } else {
            degree as f32 / max_degree as f32
        }
    }

    pub fn find_bridges(&self) -> Vec<(u64, u64)> {
        // 简化：返回权重最低的边作为潜在桥边
        let mut edges: Vec<((u64, u64), f32)> = self.edges.iter().map(|(k, v)| (*k, *v)).collect();
        edges.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        edges.iter().take(3).map(|(k, _)| *k).collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

// =====================================================================================
// 流量分析 (TrafficAnalysis)
// =====================================================================================

/// 流量模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrafficPattern {
    Burst,      // 突发
    Steady,     // 稳定
    Periodic,   // 周期性
    Random,     // 随机
}

/// 流量样本。
#[derive(Clone, Debug)]
pub struct TrafficSample {
    pub timestamp_ns: u128,
    pub bytes: u64,
    pub packets: u64,
    pub src: u64,
    pub dst: u64,
}

/// 流量分析器：分析网络流量模式。
pub struct TrafficAnalyzer {
    pub samples: Vec<TrafficSample>,
    pub max_samples: usize,
    pub total_bytes: u64,
    pub total_packets: u64,
}

impl TrafficAnalyzer {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            max_samples: 100000,
            total_bytes: 0,
            total_packets: 0,
        }
    }

    pub fn add_sample(&mut self, timestamp_ns: u128, bytes: u64, packets: u64, src: u64, dst: u64) {
        self.samples.push(TrafficSample {
            timestamp_ns,
            bytes,
            packets,
            src,
            dst,
        });
        self.total_bytes += bytes;
        self.total_packets += packets;
        if self.samples.len() > self.max_samples {
            self.samples.remove(0);
        }
    }

    pub fn detect_pattern(&self) -> TrafficPattern {
        if self.samples.len() < 3 {
            return TrafficPattern::Random;
        }
        let bytes: Vec<u64> = self.samples.iter().map(|s| s.bytes).collect();
        let mean: f64 = bytes.iter().map(|&b| b as f64).sum::<f64>() / bytes.len() as f64;
        let variance: f64 = bytes.iter().map(|&b| (b as f64 - mean) * (b as f64 - mean)).sum::<f64>() / bytes.len() as f64;
        let cv = if mean > 0.0 {
            variance.sqrt() / mean
        } else {
            0.0
        };
        if cv < 0.1 {
            TrafficPattern::Steady
        } else if cv > 1.0 {
            TrafficPattern::Burst
        } else {
            TrafficPattern::Periodic
        }
    }

    pub fn bandwidth_usage(&self, window_ns: u128) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let latest = self.samples.iter().max_by_key(|s| s.timestamp_ns).unwrap();
        let cutoff = latest.timestamp_ns.saturating_sub(window_ns);
        let window_bytes: u64 = self.samples.iter()
            .filter(|s| s.timestamp_ns >= cutoff)
            .map(|s| s.bytes)
            .sum();
        window_bytes as f64 / (window_ns as f64 / 1_000_000_000.0) / 1_000_000.0 // Mbps
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

// =====================================================================================
// 网络安全 (NetworkSecurity)
// =====================================================================================

/// 威胁类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreatType {
    DDoS,          // 分布式拒绝服务
    PortScan,      // 端口扫描
    Intrusion,     // 入侵
    Malware,       // 恶意软件
    Phishing,      // 钓鱼
}

/// 安全告警。
#[derive(Clone, Debug)]
pub struct SecurityAlert {
    pub id: u64,
    pub threat_type: ThreatType,
    pub src_ip: u64,
    pub dst_ip: u64,
    pub severity: f32,   // 0~1
    pub timestamp_ns: u128,
    pub acknowledged: bool,
}

/// 网络安全监控器：检测和响应网络威胁。
pub struct NetworkSecurityMonitor {
    pub alerts: Vec<SecurityAlert>,
    pub next_id: u64,
    pub blocked_ips: BTreeSet<u64>,
    pub threat_count: BTreeMap<ThreatType, u64>,
}

impl NetworkSecurityMonitor {
    pub fn new() -> Self {
        Self {
            alerts: Vec::new(),
            next_id: 1,
            blocked_ips: BTreeSet::new(),
            threat_count: BTreeMap::new(),
        }
    }

    pub fn alert(&mut self, threat_type: ThreatType, src_ip: u64, dst_ip: u64, severity: f32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.alerts.push(SecurityAlert {
            id,
            threat_type,
            src_ip,
            dst_ip,
            severity: severity.clamp(0.0, 1.0),
            timestamp_ns: crate::ai_os::native::now_ns(),
            acknowledged: false,
        });
        *self.threat_count.entry(threat_type).or_insert(0) += 1;
        if severity > 0.8 {
            self.blocked_ips.insert(src_ip);
        }
        id
    }

    pub fn acknowledge(&mut self, id: u64) -> bool {
        if let Some(alert) = self.alerts.iter_mut().find(|a| a.id == id) {
            alert.acknowledged = true;
            true
        } else {
            false
        }
    }

    pub fn is_blocked(&self, ip: u64) -> bool {
        self.blocked_ips.contains(&ip)
    }

    pub fn alert_count(&self) -> usize {
        self.alerts.len()
    }

    pub fn unacknowledged_count(&self) -> usize {
        self.alerts.iter().filter(|a| !a.acknowledged).count()
    }
}

// =====================================================================================
// QoS 管理 (QoSManager)
// =====================================================================================

/// 服务质量类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QoSClass {
    Voice,        // 语音
    Video,        // 视频
    Data,         // 数据
    Background,   // 后台
}

/// QoS 策略。
#[derive(Clone, Debug)]
pub struct QoSPolicy {
    pub class: QoSClass,
    pub min_bandwidth: f64,   // Mbps
    pub max_bandwidth: f64,   // Mbps
    pub max_latency: u128,    // ns
    pub max_jitter: u128,     // ns
    pub max_loss: f32,        // 0~1
}

/// QoS 管理器：流量服务质量管理。
pub struct QoSManager {
    pub policies: BTreeMap<QoSClass, QoSPolicy>,
    pub allocations: BTreeMap<u64, QoSClass>, // flow_id -> class
}

impl QoSManager {
    pub fn new() -> Self {
        let mut policies = BTreeMap::new();
        policies.insert(QoSClass::Voice, QoSPolicy {
            class: QoSClass::Voice,
            min_bandwidth: 0.1,
            max_bandwidth: 1.0,
            max_latency: 50_000_000,   // 50ms
            max_jitter: 5_000_000,     // 5ms
            max_loss: 0.01,
        });
        policies.insert(QoSClass::Video, QoSPolicy {
            class: QoSClass::Video,
            min_bandwidth: 1.0,
            max_bandwidth: 10.0,
            max_latency: 100_000_000,  // 100ms
            max_jitter: 10_000_000,    // 10ms
            max_loss: 0.05,
        });
        policies.insert(QoSClass::Data, QoSPolicy {
            class: QoSClass::Data,
            min_bandwidth: 0.01,
            max_bandwidth: 100.0,
            max_latency: 500_000_000,  // 500ms
            max_jitter: 50_000_000,    // 50ms
            max_loss: 0.1,
        });
        policies.insert(QoSClass::Background, QoSPolicy {
            class: QoSClass::Background,
            min_bandwidth: 0.0,
            max_bandwidth: 10.0,
            max_latency: 5_000_000_000, // 5s
            max_jitter: 100_000_000,    // 100ms
            max_loss: 0.2,
        });
        Self {
            policies,
            allocations: BTreeMap::new(),
        }
    }

    pub fn assign_flow(&mut self, flow_id: u64, class: QoSClass) {
        self.allocations.insert(flow_id, class);
    }

    pub fn get_policy(&self, class: QoSClass) -> Option<&QoSPolicy> {
        self.policies.get(&class)
    }

    pub fn get_flow_class(&self, flow_id: u64) -> Option<QoSClass> {
        self.allocations.get(&flow_id).copied()
    }

    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }

    pub fn flow_count(&self) -> usize {
        self.allocations.len()
    }
}

// =====================================================================================
// 网络扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn topology_analysis() {
        let mut ta = NetworkTopologyAnalysis::new();
        ta.add_node(1);
        ta.add_node(2);
        ta.add_node(3);
        ta.add_edge(1, 2, 0.8);
        ta.add_edge(2, 3, 0.6);
        ta.add_edge(1, 3, 0.3);
        assert_eq!(ta.node_count(), 3);
        assert_eq!(ta.edge_count(), 3);
        let centrality = ta.degree_centrality(2);
        assert!((centrality - 1.0).abs() < 1e-6);
        let bridges = ta.find_bridges();
        assert!(!bridges.is_empty());
    }

    #[test]
    fn traffic_analyzer() {
        let mut ta = TrafficAnalyzer::new();
        ta.add_sample(1_000_000_000, 1000, 10, 1, 2);
        ta.add_sample(2_000_000_000, 1000, 10, 1, 2);
        ta.add_sample(3_000_000_000, 1000, 10, 1, 2);
        assert_eq!(ta.sample_count(), 3);
        let pattern = ta.detect_pattern();
        assert_eq!(pattern, TrafficPattern::Steady);
        let bw = ta.bandwidth_usage(5_000_000_000);
        assert!(bw >= 0.0);
    }

    #[test]
    fn traffic_burst() {
        let mut ta = TrafficAnalyzer::new();
        ta.add_sample(1_000_000_000, 10, 1, 1, 2);
        ta.add_sample(2_000_000_000, 1000000, 10000, 1, 2);
        ta.add_sample(3_000_000_000, 10, 1, 1, 2);
        let pattern = ta.detect_pattern();
        assert_eq!(pattern, TrafficPattern::Burst);
    }

    #[test]
    fn network_security() {
        let mut ns = NetworkSecurityMonitor::new();
        let id = ns.alert(ThreatType::DDoS, 100, 200, 0.9);
        assert_eq!(id, 1);
        assert!(ns.is_blocked(100));
        assert_eq!(ns.alert_count(), 1);
        assert_eq!(ns.unacknowledged_count(), 1);
        ns.acknowledge(1);
        assert_eq!(ns.unacknowledged_count(), 0);
        assert!(!ns.is_blocked(300));
    }

    #[test]
    fn qos_manager() {
        let mut qos = QoSManager::new();
        qos.assign_flow(1, QoSClass::Voice);
        qos.assign_flow(2, QoSClass::Video);
        assert_eq!(qos.get_flow_class(1), Some(QoSClass::Voice));
        assert_eq!(qos.get_flow_class(2), Some(QoSClass::Video));
        let policy = qos.get_policy(QoSClass::Voice).unwrap();
        assert!((policy.min_bandwidth - 0.1).abs() < 1e-6);
        assert_eq!(qos.policy_count(), 4);
        assert_eq!(qos.flow_count(), 2);
    }

    #[test]
    fn traffic_patterns() {
        assert_eq!(TrafficPattern::Burst as u32, 0);
        assert_eq!(TrafficPattern::Random as u32, 3);
    }

    #[test]
    fn threat_types() {
        assert_eq!(ThreatType::DDoS as u32, 0);
        assert_eq!(ThreatType::Phishing as u32, 4);
    }

    #[test]
    fn qos_classes() {
        assert_eq!(QoSClass::Voice as u32, 0);
        assert_eq!(QoSClass::Background as u32, 3);
    }

    #[test]
    fn centrality_measures() {
        assert_eq!(CentralityMeasure::Degree as u32, 0);
        assert_eq!(CentralityMeasure::Eigenvector as u32, 3);
    }
}
