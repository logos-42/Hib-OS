//! 模拟以太网栈 (net://eth0) —— 把“网络”也当作 AI 的感知通道。
//!
//! 这是一个演示级协议栈：eth0 接口、ARP 表、路由表、发送/接收队列、
//! 统计计数器、ping/send 操作，全部驻留内存，不依赖真实网络硬件。
//! 它的目的在于证明「统一层」能把网络包、文件、显存、智能体、
//! 桌面全部揉进同一组 `sense/act` 接口里。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// 网卡名。
pub const ETH_NAME: &str = "eth0";
/// 网卡 MAC 地址 (本地管理地址)。
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

/// 一个网络包。
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

/// 模拟协议栈实体。
pub struct NetStack {
    pub iface: Interface,
    /// ARP 表：IP -> MAC。
    pub arp: HashMap<[u8; 4], [u8; 6]>,
    /// 路由表。
    pub routes: Vec<RouteEntry>,
    pub tx: VecDeque<Packet>,
    pub rx: VecDeque<Packet>,
    pub stats: NetStats,
}

impl NetStack {
    pub fn new() -> Self {
        // 默认路由：0.0.0.0/0 -> 10.0.0.1
        let mut routes = Vec::new();
        routes.push(RouteEntry {
            network: [0, 0, 0, 0],
            mask: [0, 0, 0, 0],
            gateway: [10, 0, 0, 1],
            iface: ETH_NAME.to_string(),
        });
        Self {
            iface: Interface::default_eth0(),
            arp: HashMap::new(),
            routes,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            stats: NetStats::default(),
        }
    }

    /// 选路：返回应当发包的下一跳 IP；若无路由则返回 None。
    pub fn lookup_route(&self, dst: [u8; 4]) -> Option<[u8; 4]> {
        // 1. 本子网直接投递
        if self.iface.is_local(dst) {
            return Some(dst);
        }
        // 2. 最长前缀匹配
        let mut best: Option<(&RouteEntry, u32)> = None;
        for r in &self.routes {
            let net = r.network;
            let mask = r.mask;
            let match_net = [
                net[0] & mask[0],
                net[1] & mask[1],
                net[2] & mask[2],
                net[3] & mask[3],
            ];
            let match_dst = [
                dst[0] & mask[0],
                dst[1] & mask[1],
                dst[2] & mask[2],
                dst[3] & mask[3],
            ];
            if match_net == match_dst {
                let plen: u32 = mask.iter().map(|m| m.count_ones() as u32).sum();
                match best {
                    None => best = Some((r, plen)),
                    Some((_, bp)) if plen > bp => best = Some((r, plen)),
                    _ => {}
                }
            }
        }
        best.map(|(r, _)| r.gateway)
    }

    /// 查 ARP 表；未知则返回广播 MAC。
    fn arp_resolve(&self, ip: [u8; 4]) -> [u8; 6] {
        self.arp.get(&ip).copied().unwrap_or([0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
    }

    /// 发送一个包到目的 IP；链路未启通则拒绝。
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
        let dst_mac = self.arp_resolve(next_hop);
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
        // 记录发往下一跳的 MAC (调试可见)
        let _ = dst_mac;
        Ok(())
    }

    /// 从接收队列取一个包。
    pub fn recv(&mut self) -> Option<Packet> {
        self.rx.pop_front().map(|p| {
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += p.payload.len() as u64;
            p
        })
    }

    /// 接收队列长度。
    pub fn rx_len(&self) -> usize {
        self.rx.len()
    }

    /// 发送队列长度。
    pub fn tx_len(&self) -> usize {
        self.tx.len()
    }

    /// 统计快照。
    pub fn stats(&self) -> NetStats {
        self.stats.clone()
    }

    /// 添加 ARP 条目。
    pub fn arp_add(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        self.arp.insert(ip, mac);
    }

    /// 删除 ARP 条目。
    pub fn arp_del(&mut self, ip: [u8; 4]) {
        self.arp.remove(&ip);
    }

    /// 添加路由。
    pub fn route_add(&mut self, entry: RouteEntry) {
        self.routes.push(entry);
    }

    /// 删除路由（按网络号匹配）。
    pub fn route_del(&mut self, network: [u8; 4]) {
        self.routes.retain(|r| r.network != network);
    }
}

impl Default for NetStack {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局协议栈 (感知通道 `net://eth0` 的实体)。
pub fn net() -> &'static Mutex<NetStack> {
    static NET: OnceLock<Mutex<NetStack>> = OnceLock::new();
    NET.get_or_init(|| Mutex::new(NetStack::new()))
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
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
}

/// 判断两个 IP 是否在同一子网。
pub fn same_subnet(a: [u8; 4], b: [u8; 4], mask: [u8; 4]) -> bool {
    a[0] & mask[0] == b[0] & mask[0]
        && a[1] & mask[1] == b[1] & mask[1]
        && a[2] & mask[2] == b[2] & mask[2]
        && a[3] & mask[3] == b[3] & mask[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_defaults() {
        let st = NetStack::new();
        assert!(st.iface.link_up);
        assert_eq!(st.iface.ip, ETH_IP);
        assert_eq!(st.iface.mtu, ETH_MTU);
        assert_eq!(st.tx_len(), 0);
        assert_eq!(st.rx_len(), 0);
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
        // 默认路由 0.0.0.0/0 -> 10.0.0.1，非本子网应走网关
        assert!(st.send([8, 8, 8, 8], b"ping".to_vec()).is_ok());
        assert_eq!(st.tx_len(), 1);
    }

    #[test]
    fn send_no_route() {
        let mut st = NetStack::new();
        // 删除默认路由后，非本子网应失败
        st.route_del([0, 0, 0, 0]);
        assert!(st.send([8, 8, 8, 8], b"ping".to_vec()).is_err());
        assert_eq!(st.tx_len(), 0);
    }

    #[test]
    fn arp_add_and_resolve() {
        let mut st = NetStack::new();
        let ip = [10, 0, 0, 3];
        let mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x03];
        st.arp_add(ip, mac);
        assert_eq!(st.arp_resolve(ip), mac);
        // 未知 IP 返回广播
        assert_eq!(st.arp_resolve([10, 0, 0, 9]), [0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        st.arp_del(ip);
        assert_eq!(st.arp_resolve(ip), [0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
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
        // 应能通过新路由发到 192.168.1.5
        assert!(st.send([192, 168, 1, 5], b"x".to_vec()).is_ok());
        st.route_del([192, 168, 1, 0]);
        // 默认路由还在，所以仍能发（走默认网关）
        assert!(st.send([192, 168, 1, 5], b"x".to_vec()).is_ok());
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
}
