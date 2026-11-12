//! UDP 协议栈完善（kernel/net/udp_stack.rs）。
//!
//! 本模块实现更完整的 UDP 传输层：端口多路复用、已连接/未连接模式、
//! 发送/接收队列、伪首部 + UDP 首部 + 数据的校验和计算与验证。
//! 与现有轻量 `kernel/net/udp.rs` 并存，面向更重的收发队列与多路复用场景。
//!
//! 特性:
//! * UdpSocket：本地端口、远程地址端口（已连接模式）、接收队列、发送队列
//! * UdpStack：create_socket / bind / sendto / recvfrom / connect / close
//! * 校验和计算（伪首部 + UDP 首部 + 数据）、校验和为 0 时按 RFC 发送 0xFFFF
//! * 端口 → socket 的多路复用映射
//! * sense/act 通道：`net://udp`

use std::collections::{BTreeMap, VecDeque};
use crate::ksync::{SpinLock, OnceLock};

use crate::net::ipv4::compute_checksum;
use crate::net::udp::{verify_udp_checksum, UdpHeader, UDP_HEADER_LEN};

// =======================================================================
// 地址类型
// =======================================================================

/// IPv4 地址（4 字节）。
pub type Ipv4Addr = [u8; 4];

/// 端口。
pub type Port = u16;

/// 网络端点（IP + 端口）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SockAddr {
    pub ip: Ipv4Addr,
    pub port: Port,
}

impl SockAddr {
    pub fn new(ip: Ipv4Addr, port: Port) -> Self {
        Self { ip, port }
    }

    /// 解析 `ip:port` 字符串。
    pub fn parse(s: &str) -> Result<Self, String> {
        let (ip_s, port_s) = s.rsplit_once(':')
            .ok_or_else(|| format!("地址格式应为 <ip>:<port>: {}", s))?;
        let ip = parse_ip4(ip_s)?;
        let port: Port = port_s.parse().map_err(|_| format!("无效端口: {}", port_s))?;
        Ok(Self { ip, port })
    }

    pub fn fmt(&self) -> String {
        format!("{}.{}.{}.{}:{}", self.ip[0], self.ip[1], self.ip[2], self.ip[3], self.port)
    }
}

/// 解析点分十进制 IPv4；失败返回 0.0.0.0。
fn parse_ip4(s: &str) -> Result<Ipv4Addr, String> {
    let mut out = [0u8; 4];
    for (i, part) in s.split('.').enumerate().take(4) {
        out[i] = part.trim().parse::<u8>().map_err(|_| format!("无效 IPv4 地址: {}", s))?;
    }
    Ok(out)
}

// =======================================================================
// UDP 数据报（接收项）
// =======================================================================

/// 接收到的 UDP 数据报：源地址 + 载荷。
#[derive(Clone, Debug)]
pub struct UdpDatagram {
    pub src: SockAddr,
    pub data: Vec<u8>,
}

// =======================================================================
// UdpSocket：单个 UDP socket
// ======================================================================

/// 单个 UDP socket 的状态。
pub struct UdpSocket {
    /// socket ID。
    pub id: UdpSocketId,
    /// 本地 IP。
    pub ip: Ipv4Addr,
    /// 本地端口（0 表示未绑定）。
    pub port: Port,
    /// 已 connect 时限定只与该对端通信。
    pub connected: Option<SockAddr>,
    /// 接收队列（源地址、数据）。
    pub rx: VecDeque<UdpDatagram>,
    /// 发送队列（待发数据 + 目标地址）。
    pub tx: VecDeque<(SockAddr, Vec<u8>)>,
    /// 接收字节计数。
    pub rx_bytes: u64,
    /// 发送字节计数。
    pub tx_bytes: u64,
    /// 接收包计数。
    pub rx_packets: u64,
    /// 发送包计数。
    pub tx_packets: u64,
}

/// UDP socket 句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UdpSocketId(pub u64);

impl UdpSocket {
    fn new(id: UdpSocketId, ip: Ipv4Addr) -> Self {
        Self {
            id, ip, port: 0, connected: None,
            rx: VecDeque::new(), tx: VecDeque::new(),
            rx_bytes: 0, tx_bytes: 0, rx_packets: 0, tx_packets: 0,
        }
    }

    /// 是否已绑定端口。
    pub fn is_bound(&self) -> bool { self.port != 0 }

    /// 是否已连接。
    pub fn is_connected(&self) -> bool { self.connected.is_some() }
}

// =======================================================================
// UDP 校验和辅助
// ======================================================================

/// 计算 UDP 数据报校验和（伪首部 + 首部 + 数据）。
pub fn compute_udp_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, src_port: Port, dst_port: Port, payload: &[u8]) -> u16 {
    let len = (UDP_HEADER_LEN + payload.len()) as u16;
    let pseudo = [
        src_ip[0], src_ip[1], src_ip[2], src_ip[3],
        dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3],
        0, 17, // protocol = UDP
        (len >> 8) as u8, (len & 0xFF) as u8,
    ];
    let mut udp_bytes = Vec::with_capacity(UDP_HEADER_LEN + payload.len());
    udp_bytes.extend_from_slice(&src_port.to_be_bytes());
    udp_bytes.extend_from_slice(&dst_port.to_be_bytes());
    udp_bytes.extend_from_slice(&len.to_be_bytes());
    udp_bytes.extend_from_slice(&[0, 0]); // checksum placeholder
    udp_bytes.extend_from_slice(payload);
    let csum = compute_checksum(&[&pseudo, &udp_bytes]);
    if csum == 0 { 0xFFFF } else { csum }
}

/// 验证收到的 UDP 数据报校验和（checksum 字段为 0 时跳过，视为有效）。
pub fn validate_udp_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, header: &UdpHeader, payload: &[u8]) -> bool {
    verify_udp_checksum(src_ip, dst_ip, header, payload)
}

// =======================================================================
// UdpStack：UDP 多路复用协议栈
// ======================================================================

/// UDP 协议栈：维护端口 → socket 的多路复用映射。
pub struct UdpStack {
    /// 本机 IPv4 地址。
    pub local_ip: Ipv4Addr,
    /// 端口 → socket id。
    port_map: BTreeMap<Port, UdpSocketId>,
    /// socket id → socket。
    sockets: BTreeMap<UdpSocketId, UdpSocket>,
    next_id: u64,
    next_ephemeral: Port,
}

impl UdpStack {
    /// 创建一个新的 UDP 协议栈实例。
    pub fn new(local_ip: Ipv4Addr) -> Self {
        Self {
            local_ip,
            port_map: BTreeMap::new(),
            sockets: BTreeMap::new(),
            next_id: 1,
            next_ephemeral: 49152,
        }
    }

    /// 全局 UDP 协议栈实例（`net://udp` 通道后端）。
    pub fn stack() -> &'static SpinLock<UdpStack> {
        static STACK: OnceLock<SpinLock<UdpStack>> = OnceLock::new();
        STACK.get_or_init(|| SpinLock::new(Self::new([10, 0, 0, 2])))
    }

    /// 分配下一个临时端口。
    fn next_ephemeral_port(&mut self) -> Port {
        let port = self.next_ephemeral;
        self.next_ephemeral = self.next_ephemeral.saturating_add(1);
        if self.next_ephemeral > 65535 { self.next_ephemeral = 49152; }
        port
    }

    // ------ socket 生命周期 ------

    /// 创建一个 UDP socket，返回句柄。
    pub fn create_socket(&mut self) -> UdpSocketId {
        let id = UdpSocketId(self.next_id);
        self.next_id += 1;
        self.sockets.insert(id, UdpSocket::new(id, self.local_ip));
        id
    }

    /// 将 socket 绑定到本地端口（0 表示自动分配临时端口）。
    pub fn bind(&mut self, socket: UdpSocketId, port: Port) -> Result<(), String> {
        // 先确定目标端口并检查冲突，再做可变借用，避免借用冲突。
        let target_port = if port == 0 { self.next_ephemeral_port() } else { port };
        if self.port_map.contains_key(&target_port) {
            return Err("端口已被占用".to_string());
        }
        let socket = self.sockets.get_mut(&socket).ok_or_else(|| "socket 不存在".to_string())?;
        if socket.is_bound() {
            return Err("socket 已绑定".to_string());
        }
        socket.port = target_port;
        self.port_map.insert(target_port, socket.id);
        Ok(())
    }

    /// 将 socket 连接到指定远端（进入已连接模式，后续 send/recv 限定该对端）。
    pub fn connect(&mut self, socket: UdpSocketId, remote_ip: Ipv4Addr, remote_port: Port) -> Result<(), String> {
        let socket = self.sockets.get_mut(&socket).ok_or_else(|| "socket 不存在".to_string())?;
        if !socket.is_bound() {
            return Err("socket 未绑定本地端口".to_string());
        }
        socket.connected = Some(SockAddr::new(remote_ip, remote_port));
        Ok(())
    }

    /// 关闭 socket，释放端口。
    pub fn close(&mut self, socket: UdpSocketId) -> Result<(), String> {
        let socket = self.sockets.get_mut(&socket).ok_or_else(|| "socket 不存在".to_string())?;
        if socket.is_bound() {
            self.port_map.remove(&socket.port);
            socket.port = 0;
        }
        socket.connected = None;
        Ok(())
    }

    // ------ 发送 / 接收 ------

    /// 向指定远端发送数据报（未连接模式必须指定目标；已连接模式忽略 remote）。
    pub fn sendto(&mut self, socket: UdpSocketId, remote_ip: Ipv4Addr, remote_port: Port, data: &[u8]) -> Result<usize, String> {
        let socket = self.sockets.get_mut(&socket).ok_or_else(|| "socket 不存在".to_string())?;
        if !socket.is_bound() {
            return Err("socket 未绑定本地端口".to_string());
        }
        let dst = if let Some(connected) = socket.connected {
            // 已连接：只能向 connected 对端发送。
            if connected.ip != remote_ip || connected.port != remote_port {
                return Err("已连接 socket 不可向其他对端发送".to_string());
            }
            connected
        } else {
            SockAddr::new(remote_ip, remote_port)
        };
        let src = SockAddr::new(socket.ip, socket.port);
        let _checksum = compute_udp_checksum(socket.ip, dst.ip, src.port, dst.port, data);
        // 入队发送。
        socket.tx.push_back((dst, data.to_vec()));
        socket.tx_packets += 1;
        socket.tx_bytes += data.len() as u64;
        Ok(data.len())
    }

    /// 从接收队列取一个数据报，返回 (字节数, 源地址, 源端口)。
    pub fn recvfrom(&mut self, socket: UdpSocketId, buf: &mut [u8]) -> Option<(usize, Ipv4Addr, Port)> {
        let socket = self.sockets.get_mut(&socket)?;
        let dgram = socket.rx.pop_front()?;
        // 已连接模式：只返回来自 connected 对端的数据报（已在 deliver 时过滤，此处再保险）。
        if let Some(connected) = socket.connected {
            if dgram.src.ip != connected.ip || dgram.src.port != connected.port {
                // 放回队首（不匹配，暂不投递）。
                socket.rx.push_front(dgram);
                return None;
            }
        }
        let take = buf.len().min(dgram.data.len());
        buf[..take].copy_from_slice(&dgram.data[..take]);
        socket.rx_packets += 1;
        socket.rx_bytes += take as u64;
        Some((take, dgram.src.ip, dgram.src.port))
    }

    /// 由协议栈下层（IPv4）投递一个接收到的 UDP 数据报到对应端口的 socket。
    pub fn deliver(&mut self, src_ip: Ipv4Addr, src_port: Port, dst_ip: Ipv4Addr, dst_port: Port, payload: &[u8]) -> Result<(), String> {
        // 校验目的 IP 是否本机。
        if dst_ip != self.local_ip && dst_ip != [255, 255, 255, 255] {
            return Err("目的 IP 非本机".to_string());
        }
        let socket_id = self.port_map.get(&dst_port).cloned();
        let socket_id = match socket_id {
            Some(id) => id,
            None => return Err("端口未监听".to_string()),
        };
        let socket = self.sockets.get_mut(&socket_id).unwrap();
        // 已连接模式：只投递来自 connected 对端的数据报。
        if let Some(connected) = socket.connected {
            if connected.ip != src_ip || connected.port != src_port {
                return Ok(()); // 丢弃非对端数据报。
            }
        }
        let dgram = UdpDatagram { src: SockAddr::new(src_ip, src_port), data: payload.to_vec() };
        socket.rx.push_back(dgram);
        Ok(())
    }

    /// 查询 socket 数量（供统计）。
    pub fn socket_count(&self) -> usize { self.sockets.len() }

    /// 查询某 socket 的接收队列长度。
    pub fn rx_len(&self, socket: UdpSocketId) -> Option<usize> {
        self.sockets.get(&socket).map(|s| s.rx.len())
    }
}

// =======================================================================
// sense/act 接口（net://udp）
// ======================================================================

/// 查询 UDP 协议栈状态快照（供 `sense net://udp`）。
pub fn udp_sense_snapshot() -> String {
    let st = UdpStack::stack().lock();
    format!("udp sockets={} local_ip={}.{}.{}.{}", st.socket_count(), st.local_ip[0], st.local_ip[1], st.local_ip[2], st.local_ip[3])
}

/// 对 UDP 通道执行行动（供 `act net://udp`）。负载：`stats` / `sockets`。
pub fn udp_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(udp_sense_snapshot()),
        Some("sockets") => {
            let st = UdpStack::stack().lock();
            let mut lines = Vec::new();
            for (id, sock) in &st.sockets {
                lines.push(format!("  {} port={} bound={} conn={} rx={} tx={}",
                    id.0, sock.port, sock.is_bound(), sock.is_connected(), sock.rx.len(), sock.tx.len()));
            }
            Ok(lines.join("\n"))
        }
        _ => Err("用法: act net://udp stats | sockets".to_string()),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_stack() -> UdpStack {
        UdpStack::new([10, 0, 0, 2])
    }

    #[test]
    fn create_bind_connect_close() {
        let mut st = mk_stack();
        let s = st.create_socket();
        assert!(!st.sockets.get(&s).unwrap().is_bound());
        assert!(st.bind(s, 5000).is_ok());
        assert!(st.sockets.get(&s).unwrap().is_bound());
        assert!(st.bind(s, 5000).is_err()); // 已绑定
        // 端口冲突。
        let s2 = st.create_socket();
        assert!(st.bind(s2, 5000).is_err());
        // 连接。
        assert!(st.connect(s, [10, 0, 0, 3], 9000).is_ok());
        assert!(st.sockets.get(&s).unwrap().is_connected());
        // 关闭释放端口。
        assert!(st.close(s).is_ok());
        assert!(!st.sockets.get(&s).unwrap().is_bound());
        assert!(!st.sockets.get(&s).unwrap().is_connected());
        // 端口释放后可重新绑定。
        let s3 = st.create_socket();
        assert!(st.bind(s3, 5000).is_ok());
    }

    #[test]
    fn sendto_unconnected() {
        let mut st = mk_stack();
        let s = st.create_socket();
        assert!(st.bind(s, 5001).is_ok());
        let n = st.sendto(s, [10, 0, 0, 3], 9001, b"hello").unwrap();
        assert_eq!(n, 5);
        assert_eq!(st.sockets.get(&s).unwrap().tx_packets, 1);
    }

    #[test]
    fn sendto_connected_restricts_target() {
        let mut st = mk_stack();
        let s = st.create_socket();
        assert!(st.bind(s, 5002).is_ok());
        assert!(st.connect(s, [10, 0, 0, 3], 9002).is_ok());
        // 向 connected 对端发送：成功。
        assert!(st.sendto(s, [10, 0, 0, 3], 9002, b"a").is_ok());
        // 向其他对端发送：失败。
        assert!(st.sendto(s, [10, 0, 0, 4], 9002, b"b").is_err());
    }

    #[test]
    fn recvfrom_roundtrip() {
        let mut st = mk_stack();
        let s = st.create_socket();
        assert!(st.bind(s, 5003).is_ok());
        // 投递两个数据报。
        assert!(st.deliver([10, 0, 0, 3], 9003, [10, 0, 0, 2], 5003, b"msg1").is_ok());
        assert!(st.deliver([10, 0, 0, 4], 9004, [10, 0, 0, 2], 5003, b"msg2").is_ok());
        let mut buf = [0u8; 16];
        let (n, ip, port) = st.recvfrom(s, &mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..4], b"msg1");
        assert_eq!(ip, [10, 0, 0, 3]);
        assert_eq!(port, 9003);
        let (n2, _, _) = st.recvfrom(s, &mut buf).unwrap();
        assert_eq!(n2, 4);
        // 队列已空。
        assert!(st.recvfrom(s, &mut buf).is_none());
    }

    #[test]
    fn connected_recv_filters_source() {
        let mut st = mk_stack();
        let s = st.create_socket();
        assert!(st.bind(s, 5004).is_ok());
        assert!(st.connect(s, [10, 0, 0, 3], 9004).is_ok());
        // 来自非对端：deliver 返回 Ok 但丢弃（不入队）。
        assert!(st.deliver([10, 0, 0, 5], 9999, [10, 0, 0, 2], 5004, b"noise").is_ok());
        assert!(st.recvfrom(s, &mut [0u8; 16]).is_none());
        // 来自对端：入队并可接收。
        assert!(st.deliver([10, 0, 0, 3], 9004, [10, 0, 0, 2], 5004, b"hi").is_ok());
        assert!(st.recvfrom(s, &mut [0u8; 16]).is_some());
    }

    #[test]
    fn checksum_compute_and_validate() {
        let src = [10, 0, 0, 2];
        let dst = [10, 0, 0, 3];
        let payload = b"test";
        let csum = compute_udp_checksum(src, dst, 5000, 9000, payload);
        assert_ne!(csum, 0);
        let header = UdpHeader::new(5000, 9000, payload.len());
        assert!(validate_udp_checksum(src, dst, &header, payload));
        // 校验和为 0 时跳过验证（视为有效）。
        let mut zero = header;
        zero.checksum = 0;
        assert!(validate_udp_checksum(src, dst, &zero, payload));
    }

    #[test]
    fn socket_count_and_rx_len() {
        let mut st = mk_stack();
        assert_eq!(st.socket_count(), 0);
        let s = st.create_socket();
        assert_eq!(st.socket_count(), 1);
        assert!(st.bind(s, 5005).is_ok());
        assert_eq!(st.rx_len(s), Some(0));
    }

    #[test]
    fn sense_and_act() {
        let snap = udp_sense_snapshot();
        assert!(snap.contains("sockets="));
        assert!(udp_act_raw("stats").is_ok());
        assert!(udp_act_raw("sockets").is_ok());
    }
}

