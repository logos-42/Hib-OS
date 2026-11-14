//! Hib OS 真实 TCP/IP 协议栈（kernel/net/），替代原内存模拟以太网（kernel/net.rs）。
//!
//! 分层设计（与 Linux 协议栈对齐的简化版）：
//!   设备驱动(device) → 以太网/ARP(ethernet) → IPv4(ipv4: 路由/分片/重组/ICMP) →
//!   TCP(tcp) / UDP(udp) → Socket API(socket)
//!
//! 上层通过 `net/eth0`（链路/ARP/路由/统计）和 `socket/tcp`（BSD socket）
//! 感知通道供 `sense`/`act` 使用（见 kernel/sense.rs、kernel/channels.rs）。
//!
//! 兼容：`NetStack`、`Interface`、`RouteEntry`、`Packet`、`NetStats`、`net()`、
//! `parse_ip/parse_mac/fmt_ip/fmt_mac/same_subnet`、`send/recv/arp_add/...`
//! 与原 kernel/net.rs 接口一致，供 kernel/sense.rs / kernel/netif.rs / kernel/shell.rs 继续使用。

use std::collections::{BTreeMap, VecDeque};
use crate::ksync::{SpinLock, OnceLock};

pub mod device;
pub mod ethernet;
pub mod ipv4;
pub mod socket;
pub mod tcp;
pub mod udp;
pub mod tcp_stack;
pub mod udp_stack;
pub mod icmp;
pub mod dhcp;
pub mod dns;
pub mod socket_api;
pub mod netfilter;

// ---- 公开底层类型，供上层和测试使用 ----
pub use device::{LoopbackDevice, NetDevice, TapDevice};
pub use ethernet::{
    ArpPacket, EthernetFrame, EthernetHeader, ETH_BROADCAST, ETH_HEADER_LEN, ETH_TYPE_ARP,
    ETH_TYPE_IPV4, arp_resolve,
};
pub use ipv4::{
    fragment, IcmpPacket, Ipv4Event, Ipv4Header, Ipv4Packet, ReassemblyBuffer,
    ICMP_ECHO_REPLY, ICMP_ECHO_REQUEST, IP_PROTO_ICMP, IP_PROTO_TCP, IP_PROTO_UDP,
};
pub use socket::{accept, bind, close, connect, listen, recv, send, socket, udp_connect,
    SocketAddr, SocketFd, SocketType};
pub use tcp::{TcpConnKey, TcpSegment, TcpState, DEFAULT_MSS, TCP_ACK, TCP_FIN, TCP_SYN,
    TCP_HEADER_LEN};
pub use udp::{UdpHeader, udp_checksum, verify_udp_checksum};

// =======================================================================
// 与原 kernel/net.rs 对齐的旧版 API（供 sense.rs / netif.rs / shell.rs 使用）
// =======================================================================

/// 网卡名。
pub const ETH_NAME: &str = "eth0";
/// 网卡 MAC 地址（本地管理地址）。
pub const ETH_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
/// 网卡 IP 地址。
pub const ETH_IP: [u8; 4] = [10, 0, 0, 2];
/// 网卡子网掩码。
pub const ETH_MASK: [u8; 4] = [255, 255, 255, 0];
/// MTU。
pub const ETH_MTU: u16 = 1500;

/// 网络接口。
#[derive(Clone, Debug)]
pub struct Interface {
    pub name: String,
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub link_up: bool,
    pub mtu: u16,
}

impl Interface {
    pub fn default_eth0() -> Self {
        Self {
            name: ETH_NAME.to_string(),
            mac: ETH_MAC,
            ip: ETH_IP,
            mask: ETH_MASK,
            link_up: true,
            mtu: ETH_MTU,
        }
    }

    /// 判断目标 IP 是否在本子网。
    pub fn is_local(&self, dst: [u8; 4]) -> bool {
        (self.ip[0] & self.mask[0]) == (dst[0] & self.mask[0])
            && (self.ip[1] & self.mask[1]) == (dst[1] & self.mask[1])
            && (self.ip[2] & self.mask[2]) == (dst[2] & self.mask[2])
            && (self.ip[3] & self.mask[3]) == (dst[3] & self.mask[3])
    }
}

/// 路由表条目。
#[derive(Clone, Debug)]
pub struct RouteEntry {
    pub network: [u8; 4],
    pub mask: [u8; 4],
    pub gateway: [u8; 4],
    pub iface: String,
}

/// 一个网络包（旧版裸 IP 包视图，供 net/eth0 send/recv 兼容使用）。
#[derive(Clone, Debug)]
pub struct Packet {
    pub src: [u8; 4],
    pub dst: [u8; 4],
    pub ttl: u8,
    pub payload: Vec<u8>,
}

/// 网络统计。
#[derive(Clone, Debug, Default)]
pub struct NetStats {
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// 真实协议栈实体：拥有接口、ARP 表、路由表、设备驱动与 TCP/IPv4/UDP 四层。
pub struct NetStack {
    pub iface: Interface,
    /// ARP 表：IP -> MAC。
    pub arp: BTreeMap<[u8; 4], [u8; 6]>,
    /// 路由表。
    pub routes: Vec<RouteEntry>,
    /// 旧版发送队列（裸 IP 包，供 net/eth0 send 兼容）。
    pub tx: VecDeque<Packet>,
    /// 旧版接收队列（裸 IP 包，供 net/eth0 recv 兼容）。
    pub rx: VecDeque<Packet>,
    pub stats: NetStats,

    device: Box<dyn NetDevice>,
    ip: ipv4::Ipv4Layer,
    tcp: tcp::TcpLayer,
    udp: udp::UdpLayer,
    sockets: socket::SocketTable,
    out_queue: VecDeque<Vec<u8>>,
}

impl NetStack {
    pub fn new() -> Self {
        let mut routes = Vec::new();
        routes.push(RouteEntry {
            network: [0, 0, 0, 0],
            mask: [0, 0, 0, 0],
            gateway: [10, 0, 0, 1],
            iface: ETH_NAME.to_string(),
        });
        Self {
            iface: Interface::default_eth0(),
            arp: BTreeMap::new(),
            routes,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            stats: NetStats::default(),
            device: Box::new(LoopbackDevice::new(ETH_NAME)),
            ip: ipv4::Ipv4Layer::new(ETH_IP),
            tcp: tcp::TcpLayer::new(ETH_IP),
            udp: udp::UdpLayer::new(ETH_IP),
            sockets: socket::SocketTable::new(),
            out_queue: VecDeque::new(),
        }
    }

    /// 选路：返回应当发包的下一跳 IP；若无路由则返回 None（与旧版语义一致）。
    pub fn lookup_route(&self, dst: [u8; 4]) -> Option<[u8; 4]> {
        ipv4::lookup_route(&self.routes, |d| self.iface.is_local(d), dst)
    }

    /// 查 ARP 表；未知则返回广播 MAC。
    fn arp_resolve(&self, ip: [u8; 4]) -> [u8; 6] {
        arp_resolve(&self.arp, ip)
    }

    /// 旧版发送：把载荷当作裸 IP 包投递到发送队列（与原 net.rs 行为一致）。
    pub fn send(&mut self, dst: [u8; 4], payload: Vec<u8>) -> Result<(), String> {
        if !self.iface.link_up {
            return Err("链路未启通 (link down)".to_string());
        }
        if payload.len() > self.iface.mtu as usize {
            return Err("超过 MTU".to_string());
        }
        let next_hop = match self.lookup_route(dst) {
            Some(nh) => nh,
            None => return Err("无路由到目标 (no route to host)".to_string()),
        };
        let _dst_mac = self.arp_resolve(next_hop);
        let pkt = Packet {
            src: self.iface.ip,
            dst,
            ttl: 64,
            payload,
        };
        let bytes = pkt.payload.len();
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += bytes as u64;
        self.tx.push_back(pkt);
        Ok(())
    }

    /// 从旧版接收队列取一个包。
    pub fn recv(&mut self) -> Option<Packet> {
        self.rx.pop_front().map(|p| {
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += p.payload.len() as u64;
            p
        })
    }

    pub fn rx_len(&self) -> usize { self.rx.len() }
    pub fn tx_len(&self) -> usize { self.tx.len() }
    pub fn stats(&self) -> NetStats { self.stats.clone() }

    /// 添加 ARP 条目。
    pub fn arp_add(&mut self, ip: [u8; 4], mac: [u8; 6]) { self.arp.insert(ip, mac); }
    /// 删除 ARP 条目。
    pub fn arp_del(&mut self, ip: [u8; 4]) { self.arp.remove(&ip); }
    /// 添加路由。
    pub fn route_add(&mut self, entry: RouteEntry) { self.routes.push(entry); }
    /// 删除路由（按网络号匹配）。
    pub fn route_del(&mut self, network: [u8; 4]) { self.routes.retain(|r| r.network != network); }

    /// 发送一条原始 IPv4 数据报（真实协议栈路径：路由 → ARP → 以太网封装 → 发送队列）。
    pub fn send_ip(&mut self, dst: [u8; 4], proto: u8, payload: &[u8]) -> Result<(), String> {
        let packets = self.ip.transmit(dst, proto, payload, self.iface.mtu as usize)?;
        self.frame_and_queue(dst, packets)
    }

    /// 把一段原始链路层帧送入协议栈（以太网 → ARP/IPv4 → TCP/UDP/ICMP）。
    pub fn process_frame(&mut self, bytes: &[u8]) -> Result<(), String> {
        let frame = EthernetFrame::parse(bytes).ok_or_else(|| "以太网帧过短".to_string())?;
        match frame.header.ethertype {
            ETH_TYPE_IPV4 => {
                let packet = Ipv4Packet::parse(&frame.payload)
                    .ok_or_else(|| "IPv4 包过短".to_string())?;
                // 以太网帧源 MAC -> IP 源地址：学习 ARP
                self.arp.insert(packet.header.src, frame.header.src);
                match self.ip.receive(packet) {
                    Ipv4Event::Drop => {}
                    Ipv4Event::IcmpEchoRequest { src_ip, payload } => {
                        let rep = IcmpPacket::echo_reply(0, 0, payload);
                        let bytes = rep.serialize();
                        let packets = self.ip.transmit(src_ip, IP_PROTO_ICMP, &bytes, self.iface.mtu as usize)?;
                        self.frame_and_queue(src_ip, packets)?;
                    }
                    Ipv4Event::TcpSegment { src_ip, dst_ip, segment } => {
                        let seg_bytes = segment.serialize(src_ip, dst_ip);
                        let out = self.tcp.receive_segment(src_ip, dst_ip, &seg_bytes)?;
                        for seg in out {
                            self.enqueue_tcp_segment(dst_ip, &seg)?;
                        }
                    }
                    Ipv4Event::UdpDatagram { src_ip, src_port, dst_port, payload } => {
                        let _ = self.udp.deliver(src_ip, src_port, self.iface.ip, dst_port, payload);
                    }
                }
            }
            ETH_TYPE_ARP => {
                let arp = ArpPacket::parse(&frame.payload).ok_or_else(|| "ARP 包过短".to_string())?;
                // 学习发送方
                self.arp.insert(arp.sender_ip, arp.sender_mac);
                if arp.op == ethernet::ARP_OP_REQUEST && arp.target_ip == self.iface.ip {
                    let reply = ArpPacket::reply(
                        self.iface.mac, self.iface.ip, arp.sender_mac, arp.sender_ip,
                    );
                    let frame = EthernetFrame::new(arp.sender_mac, self.iface.mac, ETH_TYPE_ARP, reply.serialize());
                    self.out_queue.push_back(frame.serialize());
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 驱动协议栈一轮（环回设备下，发出的帧会被送回接收队列再处理一次）。
    pub fn poll(&mut self) {
        for _ in 0..16 {
            let mut progressed = false;
            while let Some(bytes) = self.device.recv() {
                let _ = self.process_frame(&bytes);
                progressed = true;
            }
            while let Some(frame) = self.out_queue.pop_front() {
                let _ = self.device.send(&frame);
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
    }

    pub fn outbound_len(&self) -> usize { self.out_queue.len() }
    pub fn pop_outbound(&mut self) -> Option<Vec<u8>> { self.out_queue.pop_front() }

    /// 把一段 TCP 段封装成 IPv4 数据报并入发送队列（以太网封装由 frame_and_queue 完成）。
    fn enqueue_tcp_segment(&mut self, dst_ip: [u8; 4], seg: &TcpSegment) -> Result<(), String> {
        let seg_bytes = seg.serialize(self.iface.ip, dst_ip);
        let packets = self.ip.transmit(dst_ip, IP_PROTO_TCP, &seg_bytes, self.iface.mtu as usize)?;
        self.frame_and_queue(dst_ip, packets)
    }

    fn frame_and_queue(&mut self, dst_ip: [u8; 4], packets: Vec<Ipv4Packet>) -> Result<(), String> {
        let next_hop = self.lookup_route(dst_ip).ok_or_else(|| {
            format!("无路由到 {}", fmt_ip(dst_ip))
        })?;
        let dst_mac = arp_resolve(&self.arp, next_hop);
        for p in packets {
            let bytes = p.serialize();
            let frame = EthernetFrame::new(dst_mac, self.iface.mac, ETH_TYPE_IPV4, bytes);
            self.out_queue.push_back(frame.serialize());
        }
        Ok(())
    }
}

impl Default for NetStack {
    fn default() -> Self { Self::new() }
}

/// 全局协议栈（感知通道 `net/eth0` 的实体）。
pub fn net() -> &'static SpinLock<NetStack> {
    static NET: OnceLock<SpinLock<NetStack>> = OnceLock::new();
    NET.get_or_init(|| SpinLock::new(NetStack::new()))
}

/// 把点分十进制字符串解析为 IPv4 地址；解析失败返回 0.0.0.0。
pub fn parse_ip(s: &str) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (i, part) in s.split('.').enumerate().take(4) {
        out[i] = part.trim().parse::<u8>().unwrap_or(0);
    }
    out
}

/// 把点分十进制字符串解析为 MAC 地址 (xx:xx:xx:xx:xx:xx)。
pub fn parse_mac(s: &str) -> [u8; 6] {
    let mut out = [0u8; 6];
    let parts: Vec<&str> = s.split(':').collect();
    for (i, part) in parts.iter().enumerate().take(6) {
        out[i] = u8::from_str_radix(part, 16).unwrap_or(0);
    }
    out
}

/// IPv4 地址格式化。
pub fn fmt_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// MAC 地址格式化。
pub fn fmt_mac(mac: [u8; 6]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
}

/// 判断两个 IP 是否在同一子网。
pub fn same_subnet(a: [u8; 4], b: [u8; 4], mask: [u8; 4]) -> bool {
    a[0] & mask[0] == b[0] & mask[0]
        && a[1] & mask[1] == b[1] & mask[1]
        && a[2] & mask[2] == b[2] & mask[2]
        && a[3] & mask[3] == b[3] & mask[3]
}

// =======================================================================
// 单元测试
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_ip() -> [u8; 4] { [10, 0, 0, 3] }
    fn peer_mac() -> [u8; 6] { [0x02, 0x00, 0x00, 0x00, 0x00, 0x03] }

    #[test]
    fn stack_defaults() {
        let st = NetStack::new();
        assert!(st.iface.link_up);
        assert_eq!(st.iface.ip, ETH_IP);
        assert_eq!(st.iface.mtu, ETH_MTU);
        assert_eq!(st.tx_len(), 0);
        assert_eq!(st.rx_len(), 0);
        assert_eq!(st.lookup_route([10, 0, 0, 3]), Some([10, 0, 0, 3])); // 本子网直连
    }

    #[test]
    fn send_local_subnet() {
        let mut st = NetStack::new();
        let dst = [10, 0, 0, 3];
        assert!(st.send(dst, b"hello".to_vec()).is_ok());
        assert_eq!(st.tx_len(), 1);
        assert_eq!(st.stats().tx_packets, 1);
        assert_eq!(st.stats().tx_bytes, 5);
    }

    #[test]
    fn send_remote_via_default_route() {
        let mut st = NetStack::new();
        assert!(st.send([8, 8, 8, 8], b"ping".to_vec()).is_ok());
        assert_eq!(st.tx_len(), 1);
    }

    #[test]
    fn send_no_route() {
        let mut st = NetStack::new();
        st.route_del([0, 0, 0, 0]);
        assert!(st.send([8, 8, 8, 8], b"ping".to_vec()).is_err());
        assert_eq!(st.tx_len(), 0);
    }

    #[test]
    fn send_ip_queues_ethernet_frame() {
        let mut st = NetStack::new();
        st.arp_add(peer_ip(), peer_mac());
        assert!(st.send_ip(peer_ip(), IP_PROTO_ICMP, b"ping!").is_ok());
        assert_eq!(st.outbound_len(), 1);
        let frame = st.pop_outbound().unwrap();
        let eth = EthernetFrame::parse(&frame).unwrap();
        assert_eq!(eth.header.ethertype, ETH_TYPE_IPV4);
        assert_eq!(eth.header.dst, peer_mac());
        let pkt = Ipv4Packet::parse(&eth.payload).unwrap();
        assert_eq!(pkt.header.src, ETH_IP);
        assert_eq!(pkt.header.dst, peer_ip());
        assert_eq!(pkt.header.protocol, IP_PROTO_ICMP);
        assert_eq!(pkt.payload, b"ping!");
        assert!(pkt.verify_checksum());
    }

    #[test]
    fn receive_frame_icmp_echo_generates_reply() {
        let mut st = NetStack::new();
        // 对自身发 ICMP Echo Request，应收到 Echo Reply
        assert!(st.send_ip(ETH_IP, IP_PROTO_ICMP, b"ping!").is_ok());
        let frame = st.pop_outbound().unwrap();
        assert!(st.process_frame(&frame).is_ok());
        assert_eq!(st.outbound_len(), 1);
        let reply = st.pop_outbound().unwrap();
        let eth = EthernetFrame::parse(&reply).unwrap();
        let pkt = Ipv4Packet::parse(&eth.payload).unwrap();
        assert_eq!(pkt.header.protocol, IP_PROTO_ICMP);
        let icmp = IcmpPacket::parse(&pkt.payload).unwrap();
        assert_eq!(icmp.icmp_type, ICMP_ECHO_REPLY);
        assert_eq!(icmp.payload, b"ping!");
    }

    #[test]
    fn arp_add_and_resolve() {
        let mut st = NetStack::new();
        let ip = peer_ip();
        let mac = peer_mac();
        st.arp_add(ip, mac);
        assert_eq!(st.arp_resolve(ip), mac);
        assert_eq!(st.arp_resolve([10, 0, 0, 9]), ETH_BROADCAST);
        st.arp_del(ip);
        assert_eq!(st.arp_resolve(ip), ETH_BROADCAST);
    }

    #[test]
    fn route_add_and_del() {
        let mut st = NetStack::new();
        let entry = RouteEntry {
            network: [192, 168, 1, 0],
            mask: [255, 255, 255, 0],
            gateway: [10, 0, 0, 10],
            iface: ETH_NAME.to_string(),
        };
        st.route_add(entry.clone());
        assert!(st.send([192, 168, 1, 5], b"x".to_vec()).is_ok());
        st.route_del([192, 168, 1, 0]);
        assert!(st.send([192, 168, 1, 5], b"x".to_vec()).is_ok()); // 默认路由还在
    }

    #[test]
    fn link_down_rejects_send() {
        let mut st = NetStack::new();
        st.iface.link_up = false;
        assert!(st.send([10, 0, 0, 3], b"x".to_vec()).is_err());
    }

    #[test]
    fn ip_and_mac_parse() {
        assert_eq!(parse_ip("10.0.0.2"), [10, 0, 0, 2]);
        assert_eq!(parse_ip("not-an-ip"), [0, 0, 0, 0]);
        assert_eq!(fmt_ip([10, 0, 0, 2]), "10.0.0.2");
        assert_eq!(parse_mac("02:00:00:00:00:03"), [0x02, 0x00, 0x00, 0x00, 0x00, 0x03]);
        assert_eq!(parse_mac("bad"), [0, 0, 0, 0, 0, 0]);
        assert_eq!(fmt_mac([0x02, 0x00, 0x00, 0x00, 0x00, 0x03]), "02:00:00:00:00:03");
    }

    #[test]
    fn same_subnet_check() {
        assert!(same_subnet([10, 0, 0, 2], [10, 0, 0, 3], [255, 255, 255, 0]));
        assert!(!same_subnet([10, 0, 0, 2], [10, 0, 1, 3], [255, 255, 255, 0]));
        assert!(!same_subnet([10, 0, 0, 2], [8, 8, 8, 8], [255, 255, 255, 0]));
    }

    #[test]
    fn interface_is_local() {
        let iface = Interface::default_eth0();
        assert!(iface.is_local([10, 0, 0, 5]));
        assert!(!iface.is_local([10, 0, 1, 5]));
    }
}
