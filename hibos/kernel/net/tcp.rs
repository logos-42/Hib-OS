//! 简化版 TCP：状态机（LISTEN/SYN-SENT/ESTABLISHED 等）、三次握手、
//! 序列号/确认号、滑动窗口。
//!
//! 本层不依赖以太网：`TcpLayer` 只处理 TCP 段字节流，收发由上层（`NetStack`）经
//! IPv4 层封装成以太网帧送出。

use alloc::collections::{BTreeMap, VecDeque};

use crate::net::ipv4::compute_checksum;

/// TCP 控制位。
pub const TCP_FIN: u8 = 0x01;
pub const TCP_SYN: u8 = 0x02;
pub const TCP_RST: u8 = 0x04;
pub const TCP_PSH: u8 = 0x08;
pub const TCP_ACK: u8 = 0x10;
pub const TCP_URG: u8 = 0x20;

/// TCP 首部长度（无选项，20 字节）。
pub const TCP_HEADER_LEN: usize = 20;
/// 默认最大段长度 (1500 - 20 - 20)。
pub const DEFAULT_MSS: usize = 1460;
/// 初始序列号基数。
const ISS_BASE: u32 = 1000;

/// TCP 连接状态（简化版）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

impl TcpState {
    pub fn name(self) -> &'static str {
        match self {
            TcpState::Closed => "CLOSED",
            TcpState::Listen => "LISTEN",
            TcpState::SynSent => "SYN-SENT",
            TcpState::SynReceived => "SYN-RECEIVED",
            TcpState::Established => "ESTABLISHED",
            TcpState::FinWait1 => "FIN-WAIT-1",
            TcpState::FinWait2 => "FIN-WAIT-2",
            TcpState::CloseWait => "CLOSE-WAIT",
            TcpState::Closing => "CLOSING",
            TcpState::LastAck => "LAST-ACK",
            TcpState::TimeWait => "TIME-WAIT",
        }
    }
}

/// TCP 四元组键（本端端口 + 对端地址）。监听 socket 的对端为 0.0.0.0:0。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TcpConnKey {
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
}

/// TCP 段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpSegment {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub data_offset: u8, // 32 位字字数，无选项时为 5
    pub flags: u8,
    pub window: u16,
    pub checksum: u16,
    pub urgent_ptr: u16,
    pub payload: Vec<u8>,
}

impl TcpSegment {
    pub fn new(src_port: u16, dst_port: u16, seq: u32, ack: u32, flags: u8,
               window: u16, payload: Vec<u8>) -> Self {
        Self {
            src_port, dst_port, seq, ack, data_offset: 5, flags, window,
            checksum: 0, urgent_ptr: 0, payload,
        }
    }

    /// 序列化并填入伪首部校验和。
    pub fn serialize(&self, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
        let mut b = Vec::with_capacity(TCP_HEADER_LEN + self.payload.len());
        b.extend_from_slice(&self.src_port.to_be_bytes());
        b.extend_from_slice(&self.dst_port.to_be_bytes());
        b.extend_from_slice(&self.seq.to_be_bytes());
        b.extend_from_slice(&self.ack.to_be_bytes());
        b.push((self.data_offset << 4) & 0xF0);
        b.push(self.flags);
        b.extend_from_slice(&self.window.to_be_bytes());
        b.extend_from_slice(&[0, 0]); // checksum 占位
        b.extend_from_slice(&self.urgent_ptr.to_be_bytes());
        b.extend_from_slice(&self.payload);

        let seg_len = (TCP_HEADER_LEN + self.payload.len()) as u16;
        let pseudo = [
            src_ip[0], src_ip[1], src_ip[2], src_ip[3],
            dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3],
            0, 6, // protocol = TCP
            (seg_len >> 8) as u8, (seg_len & 0xFF) as u8,
        ];
        let csum = compute_checksum(&[&pseudo, &b]);
        b[16..18].copy_from_slice(&csum.to_be_bytes());
        b
    }

    /// 解析并校验（checksum 字段为 0 时跳过校验）。
    pub fn parse(bytes: &[u8], src_ip: [u8; 4], dst_ip: [u8; 4]) -> Option<Self> {
        if bytes.len() < TCP_HEADER_LEN { return None; }
        let data_offset = (bytes[12] >> 4) as usize;
        let header_len = data_offset * 4;
        if bytes.len() < header_len { return None; }

        let checksum = u16::from_be_bytes([bytes[16], bytes[17]]);
        if checksum != 0 {
            let mut b = bytes.to_vec();
            b[16] = 0;
            b[17] = 0;
            let seg_len = bytes.len() as u16;
            let pseudo = [
                src_ip[0], src_ip[1], src_ip[2], src_ip[3],
                dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3],
                0, 6, (seg_len >> 8) as u8, (seg_len & 0xFF) as u8,
            ];
            if compute_checksum(&[&pseudo, &b]) != checksum {
                return None;
            }
        }

        Some(Self {
            src_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            dst_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            seq: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            ack: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            data_offset: data_offset as u8,
            flags: bytes[13],
            window: u16::from_be_bytes([bytes[14], bytes[15]]),
            checksum,
            urgent_ptr: u16::from_be_bytes([bytes[18], bytes[19]]),
            payload: bytes[header_len..].to_vec(),
        })
    }
}

/// 单个 TCP socket 的状态。
#[derive(Clone)]
pub struct TcpSocket {
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub state: TcpState,

    pub iss: u32,
    pub snd_una: u32, // 已发送但未确认的最早序号
    pub snd_nxt: u32, // 下一个待发送序号
    pub rcv_nxt: u32, // 下一个期望接收的序号

    pub snd_wnd: u16, // 对端通告的发送窗口
    pub rcv_wnd: u16, // 我们向对端通告的接收窗口

    pub send_buf: VecDeque<u8>,     // 已发未确认 + 待发数据
    pub recv_buf: VecDeque<u8>,     // 已到序数据，等待应用读取
    pub accept_queue: VecDeque<TcpSocket>, // 仅监听 socket：已完成握手的子连接
}

impl TcpSocket {
    fn rcv_window(&self) -> u16 {
        const CAP: usize = 65535;
        CAP.saturating_sub(self.recv_buf.len()) as u16
    }
}

/// TCP 层：按本端端口索引的 socket 表。
pub struct TcpLayer {
    pub our_ip: [u8; 4],
    pub sockets: BTreeMap<TcpConnKey, TcpSocket>,
    pub next_ephemeral: u16,
}

impl TcpLayer {
    pub fn new(our_ip: [u8; 4]) -> Self {
        Self {
            our_ip,
            sockets: BTreeMap::new(),
            next_ephemeral: 49152,
        }
    }

    /// 绑定本地端口（状态 Closed）。
    pub fn bind(&mut self, local_ip: [u8; 4], local_port: u16) -> Result<(), String> {
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        if self.sockets.contains_key(&key) {
            return Err(format!("端口 {} 已绑定", local_port));
        }
        self.sockets.insert(key, TcpSocket {
            local_ip, local_port, remote_ip: [0, 0, 0, 0], remote_port: 0,
            state: TcpState::Closed,
            iss: 0, snd_una: 0, snd_nxt: 0, rcv_nxt: 0,
            snd_wnd: 65535, rcv_wnd: 65535,
            send_buf: VecDeque::new(), recv_buf: VecDeque::new(), accept_queue: VecDeque::new(),
        });
        Ok(())
    }

    /// 监听端口。
    pub fn listen(&mut self, local_port: u16) -> Result<(), String> {
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let socket = self.sockets.get_mut(&key).ok_or_else(|| format!("端口 {} 未绑定", local_port))?;
        if socket.state != TcpState::Closed {
            return Err(format!("端口 {} 状态非法", local_port));
        }
        socket.state = TcpState::Listen;
        Ok(())
    }

    /// 主动打开：发送 SYN，返回待发送的段列表（字节流，由上层封装）。
    pub fn connect(&mut self, local_port: u16, remote_ip: [u8; 4], remote_port: u16)
        -> Result<Vec<TcpSegment>, String> {
        let key = TcpConnKey { local_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let socket = self.sockets.get_mut(&key).ok_or_else(|| format!("端口 {} 未绑定", local_port))?;
        if socket.state != TcpState::Closed {
            return Err(format!("端口 {} 状态非法 (connect)", socket.state.name()));
        }
        socket.remote_ip = remote_ip;
        socket.remote_port = remote_port;
        socket.iss = ISS_BASE + local_port as u32;
        socket.snd_una = socket.iss;
        socket.snd_nxt = socket.iss + 1;
        socket.state = TcpState::SynSent;

        let new_key = TcpConnKey { local_port, remote_ip, remote_port };
        let iss = socket.iss;
        drop(socket);
        let moved = self.sockets.remove(&key).unwrap();
        self.sockets.insert(new_key.clone(), moved);

        let syn = TcpSegment::new(local_port, remote_port, iss, 0, TCP_SYN, 65535, Vec::new());
        Ok(vec![syn])
    }

    /// 处理一段到达的 TCP 段（原始字节），返回需要发送的段列表。
    pub fn receive_segment(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4],
                           bytes: &[u8]) -> Result<Vec<TcpSegment>, String> {
        let mut out = Vec::new();
        let seg = match TcpSegment::parse(bytes, src_ip, dst_ip) {
            Some(s) => s,
            None => return Ok(out), // 校验不通过：静默丢弃
        };

        let exact = TcpConnKey { local_port: seg.dst_port, remote_ip: src_ip, remote_port: seg.src_port };
        let listener_key = TcpConnKey { local_port: seg.dst_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let key = if self.sockets.contains_key(&exact) {
            exact.clone()
        } else if let Some(s) = self.sockets.get(&listener_key) {
            if s.state == TcpState::Listen { listener_key.clone() } else { return Ok(out); }
        } else {
            return Ok(out); // 无监听端口：静默丢弃（简化版不发 RST）
        };

        let is_syn = seg.flags & TCP_SYN != 0;
        let is_ack = seg.flags & TCP_ACK != 0;
        let is_fin = seg.flags & TCP_FIN != 0;
        let is_rst = seg.flags & TCP_RST != 0;

        if is_rst {
            if let Some(s) = self.sockets.get_mut(&key) {
                s.state = TcpState::Closed;
            }
            return Ok(out);
        }

        let state = self.sockets.get(&key).unwrap().state;
        let mut new_state = state;
        let mut rcv_nxt_advance: u32 = 0;
        let mut reply_ack = false;

        match state {
            TcpState::Listen if is_syn => {
                // 被动打开：创建子连接，并同时写入主表，以便后续段能精确匹配
                let iss_child = ISS_BASE + seg.dst_port as u32;
                let child_key = TcpConnKey { local_port: seg.dst_port, remote_ip: src_ip, remote_port: seg.src_port };
                let mut child = TcpSocket {
                    local_ip: dst_ip, local_port: seg.dst_port,
                    remote_ip: src_ip, remote_port: seg.src_port,
                    state: TcpState::SynReceived,
                    iss: iss_child, snd_una: iss_child, snd_nxt: iss_child + 1,
                    rcv_nxt: seg.seq.wrapping_add(1),
                    snd_wnd: seg.window, rcv_wnd: 65535,
                    send_buf: VecDeque::new(), recv_buf: VecDeque::new(), accept_queue: VecDeque::new(),
                };
                child.rcv_nxt = seg.seq.wrapping_add(1);
                let listener = self.sockets.get_mut(&listener_key).unwrap();
                listener.accept_queue.push_back(child.clone());
                self.sockets.insert(child_key.clone(), child);
                out.push(TcpSegment::new(seg.dst_port, seg.src_port, iss_child,
                                         seg.seq.wrapping_add(1), TCP_SYN | TCP_ACK,
                                         65535, Vec::new()));
            }
            TcpState::SynSent => {
                if is_syn && is_ack && seg.ack == self.snd_nxt_of(&key) {
                    // 对端 SYN-ACK 确认了我们的 SYN
                    new_state = TcpState::Established;
                    let sock = self.sockets.get_mut(&key).unwrap();
                    sock.rcv_nxt = seg.seq.wrapping_add(1);
                    sock.snd_wnd = seg.window;
                    out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.snd_nxt,
                                             sock.rcv_nxt, TCP_ACK, sock.rcv_window(), Vec::new()));
                } else if is_syn {
                    // 同步打开
                    let sock = self.sockets.get_mut(&key).unwrap();
                    sock.state = TcpState::SynReceived;
                    sock.rcv_nxt = seg.seq.wrapping_add(1);
                    out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.iss,
                                             seg.seq.wrapping_add(1), TCP_SYN | TCP_ACK,
                                             sock.rcv_window(), Vec::new()));
                }
            }
            TcpState::SynReceived => {
                if is_ack && seg.ack == self.snd_nxt_of(&key) {
                    new_state = TcpState::Established;
                }
            }
            TcpState::Established => {
                if is_ack {
                    self.ack_received(&key, seg.ack, seg.window);
                }
                if !seg.payload.is_empty() {
                    let sock = self.sockets.get(&key).unwrap();
                    if seg.seq == sock.rcv_nxt {
                        rcv_nxt_advance = seg.payload.len() as u32;
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

        if rcv_nxt_advance != 0 {
            if let Some(sock) = self.sockets.get_mut(&key) {
                let n = rcv_nxt_advance as usize;
                let data = &seg.payload[..n];
                sock.recv_buf.extend(data.iter().cloned());
                sock.rcv_nxt = sock.rcv_nxt.wrapping_add(rcv_nxt_advance);
            }
        }

        if new_state != state {
            if let Some(sock) = self.sockets.get_mut(&key) {
                sock.state = new_state;
            }
        }

        if reply_ack {
            if let Some(sock) = self.sockets.get(&key) {
                out.push(TcpSegment::new(seg.dst_port, seg.src_port, sock.snd_nxt,
                                         sock.rcv_nxt, TCP_ACK, sock.rcv_window(), Vec::new()));
            }
        }

        Ok(out)
    }

    fn snd_nxt_of(&self, key: &TcpConnKey) -> u32 {
        self.sockets.get(key).map(|s| s.snd_nxt).unwrap_or(0)
    }

    /// 对已确认的序号进行滑动窗口收缩。
    fn ack_received(&mut self, key: &TcpConnKey, ack: u32, window: u16) {
        let sock = match self.sockets.get_mut(key) {
            Some(s) => s,
            None => return,
        };
        sock.snd_wnd = window;
        let newly = ack.wrapping_sub(sock.snd_una) as usize;
        if newly > 0 && newly <= sock.send_buf.len() {
            sock.snd_una = ack;
            for _ in 0..newly { sock.send_buf.pop_front(); }
        }
    }

    /// 向已连接 socket 发送数据，返回待发送的段列表（受滑动窗口限制）。
    pub fn send(&mut self, key: &TcpConnKey, data: &[u8]) -> Result<Vec<TcpSegment>, String> {
        let sock = self.sockets.get_mut(key).ok_or_else(|| "socket 不存在".to_string())?;
        if sock.state != TcpState::Established {
            return Err(format!("socket 未建立 (state={})", sock.state.name()));
        }
        sock.send_buf.extend(data.iter().cloned());

        let mut out = Vec::new();
        let window = sock.snd_wnd.max(1) as usize;
        while !sock.send_buf.is_empty() {
            let in_flight = sock.snd_nxt.wrapping_sub(sock.snd_una) as usize;
            if in_flight >= window { break; }
            let seg_len = sock.send_buf.len().min(DEFAULT_MSS).min(window - in_flight);
            let chunk: Vec<u8> = sock.send_buf.iter().take(seg_len).cloned().collect();
            let seg = TcpSegment::new(sock.local_port, sock.remote_port, sock.snd_nxt,
                                      sock.rcv_nxt, TCP_ACK | TCP_PSH,
                                      sock.rcv_window(), chunk);
            sock.snd_nxt = sock.snd_nxt.wrapping_add(seg_len as u32);
            out.push(seg);
        }
        Ok(out)
    }

    /// 从 socket 接收缓冲读取最多 len 字节。
    pub fn recv(&mut self, key: &TcpConnKey, len: usize) -> Result<Vec<u8>, String> {
        let sock = self.sockets.get_mut(key).ok_or_else(|| "socket 不存在".to_string())?;
        let mut data = Vec::new();
        for _ in 0..len.min(sock.recv_buf.len()) {
            data.push(sock.recv_buf.pop_front().unwrap());
        }
        Ok(data)
    }

    /// 监听 socket 接收一个已完成握手的子连接。
    pub fn accept(&mut self, listener_port: u16) -> Result<(TcpConnKey, [u8; 4], u16), String> {
        let listener_key = TcpConnKey { local_port: listener_port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
        let listener = self.sockets.get_mut(&listener_key)
            .ok_or_else(|| format!("监听 socket 不存在 (port={})", listener_port))?;
        if listener.state != TcpState::Listen {
            return Err("socket 未监听".to_string());
        }
        let child = listener.accept_queue.pop_front()
            .ok_or_else(|| "没有等待中的连接".to_string())?;
        let remote = (child.remote_ip, child.remote_port);
        let key = TcpConnKey { local_port: listener_port, remote_ip: child.remote_ip, remote_port: child.remote_port };
        // 子连接已在 SYN 阶段写入主表，此处仅确保存在
        if !self.sockets.contains_key(&key) {
            self.sockets.insert(key.clone(), child);
        }
        Ok((key, remote.0, remote.1))
    }

    /// 应用层 close：发送 FIN。
    pub fn close(&mut self, key: &TcpConnKey) -> Result<Vec<TcpSegment>, String> {
        let state = self.sockets.get(key).map(|s| s.state).ok_or_else(|| "socket 不存在".to_string())?;
        let mut out = Vec::new();
        match state {
            TcpState::Established | TcpState::CloseWait => {
                let (seq, ack) = {
                    let sock = self.sockets.get(key).unwrap();
                    (sock.snd_nxt, sock.rcv_nxt)
                };
                out.push(TcpSegment::new(
                    self.sockets.get(key).unwrap().local_port,
                    self.sockets.get(key).unwrap().remote_port,
                    seq, ack, TCP_FIN | TCP_ACK,
                    self.sockets.get(key).unwrap().rcv_window(), Vec::new(),
                ));
                if let Some(s) = self.sockets.get_mut(key) {
                    s.snd_nxt = s.snd_nxt.wrapping_add(1);
                    s.state = match state {
                        TcpState::Established => TcpState::FinWait1,
                        _ => TcpState::LastAck,
                    };
                }
            }
            _ => return Err(format!("无法关闭 (state={})", state.name())),
        }
        Ok(out)
    }

    pub fn state_of(&self, key: &TcpConnKey) -> Option<TcpState> {
        self.sockets.get(key).map(|s| s.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ip() -> [u8; 4] { [10, 0, 0, 2] }
    fn peer_ip() -> [u8; 4] { [10, 0, 0, 3] }

    fn make_layer() -> TcpLayer { TcpLayer::new(test_ip()) }

    fn server_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: [0, 0, 0, 0], remote_port: 0 }
    }
    fn child_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: peer_ip(), remote_port: 50001 }
    }
    fn client_key(port: u16) -> TcpConnKey {
        TcpConnKey { local_port: port, remote_ip: peer_ip(), remote_port: 8080 }
    }

    /// 单向驱动握手：client 发 SYN → server 回 SYN-ACK → client 回 ACK。
    #[test]
    fn three_way_handshake() {
        let mut layer = make_layer();
        layer.bind(test_ip(), 8080).unwrap();
        layer.listen(8080).unwrap();
        layer.bind(test_ip(), 50001).unwrap();

        let syn = layer.connect(50001, peer_ip(), 8080).unwrap();
        assert_eq!(syn.len(), 1);
        assert_eq!(syn[0].flags & TCP_SYN, TCP_SYN);
        assert_eq!(syn[0].seq, ISS_BASE + 50001);

        // server 端收到 SYN
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let out = layer.receive_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & (TCP_SYN | TCP_ACK), TCP_SYN | TCP_ACK);
        assert_eq!(out[0].ack, syn[0].seq.wrapping_add(1));
        assert_eq!(layer.state_of(&server_key(8080)).unwrap(), TcpState::Listen);
        let ck = child_key(8080);
        assert_eq!(layer.state_of(&ck).unwrap(), TcpState::SynReceived);

        // client 收到 SYN-ACK
        let synack = out[0].clone();
        let out = layer.receive_segment(test_ip(), peer_ip(), &synack.serialize(test_ip(), peer_ip())).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & TCP_ACK, TCP_ACK);
        assert_eq!(out[0].ack, synack.seq.wrapping_add(1));
        assert_eq!(layer.state_of(&client_key(50001)).unwrap(), TcpState::Established);

        // server 收到 ACK
        let out = layer.receive_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();
        assert_eq!(out.len(), 0);
        assert_eq!(layer.state_of(&ck).unwrap(), TcpState::Established);
    }

    #[test]
    fn send_and_recv_data() {
        let mut layer = make_layer();
        layer.bind(test_ip(), 8080).unwrap();
        layer.listen(8080).unwrap();
        layer.bind(test_ip(), 50001).unwrap();

        let syn = layer.connect(50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = layer.receive_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = layer.receive_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        layer.receive_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = child_key(8080);
        // client 发送 "hello"
        let segs = layer.send(&client_key(50001), b"hello").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(&segs[0].payload, b"hello");
        assert_eq!(segs[0].seq, (ISS_BASE + 50001) + 1);
        assert_eq!(segs[0].ack, ISS_BASE + 8080 + 1);

        // server 端收到数据
        let data_bytes = segs[0].serialize(test_ip(), peer_ip());
        let out = layer.receive_segment(peer_ip(), test_ip(), &data_bytes).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags & TCP_ACK, TCP_ACK);
        assert_eq!(out[0].ack, segs[0].seq.wrapping_add(5));
        let recv = layer.recv(&ck, 64).unwrap();
        assert_eq!(recv, b"hello");
    }

    #[test]
    fn sliding_window_limits_in_flight() {
        let mut layer = make_layer();
        layer.bind(test_ip(), 8080).unwrap();
        layer.listen(8080).unwrap();
        layer.bind(test_ip(), 50001).unwrap();

        let syn = layer.connect(50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = layer.receive_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = layer.receive_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        layer.receive_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = client_key(50001);
        // 白箱：把对端通告窗口压到 3 字节
        if let Some(s) = layer.sockets.get_mut(&ck) {
            s.snd_wnd = 3;
        }

        let segs = layer.send(&ck, b"abcdefghij").unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].payload.len(), 3);
        assert_eq!(&segs[0].payload, b"abc");
        // 窗口耗尽，再发应不再出段
        let segs2 = layer.send(&ck, b"more").unwrap();
        assert_eq!(segs2.len(), 0);

        // 对端确认前 3 字节，窗口滚动，再可发 3 字节
        let ack_seg = TcpSegment::new(8080, 50001, ISS_BASE + 8080 + 1,
                                      (ISS_BASE + 50001) + 1 + 3, TCP_ACK, 3, Vec::new());
        let ack_bytes = ack_seg.serialize(peer_ip(), test_ip());
        let out = layer.receive_segment(peer_ip(), test_ip(), &ack_bytes).unwrap();
        assert_eq!(out.len(), 0); // 纯 ACK 不产生回应
        let segs3 = layer.send(&ck, b"more").unwrap();
        assert_eq!(segs3.len(), 1);
        assert_eq!(&segs3[0].payload, b"def");
    }

    #[test]
    fn close_exchanges_fin() {
        let mut layer = make_layer();
        layer.bind(test_ip(), 8080).unwrap();
        layer.listen(8080).unwrap();
        layer.bind(test_ip(), 50001).unwrap();

        let syn = layer.connect(50001, peer_ip(), 8080).unwrap();
        let syn_bytes = syn[0].serialize(peer_ip(), test_ip());
        let synack = layer.receive_segment(peer_ip(), test_ip(), &syn_bytes).unwrap();
        let out = layer.receive_segment(test_ip(), peer_ip(), &synack[0].serialize(test_ip(), peer_ip())).unwrap();
        layer.receive_segment(peer_ip(), test_ip(), &out[0].serialize(test_ip(), peer_ip())).unwrap();

        let ck = child_key(8080);
        let fin_segs = layer.close(&ck).unwrap();
        assert_eq!(fin_segs.len(), 1);
        assert_eq!(fin_segs[0].flags & TCP_FIN, TCP_FIN);
        assert_eq!(layer.state_of(&ck).unwrap(), TcpState::FinWait1);

        // 对端 ACK 我们的 FIN
        let ack_of_fin = TcpSegment::new(8080, 50001, ISS_BASE + 8080 + 1,
                                         (ISS_BASE + 50001) + 2, TCP_ACK, 65535, Vec::new());
        let out = layer.receive_segment(peer_ip(), test_ip(), &ack_of_fin.serialize(peer_ip(), test_ip())).unwrap();
        assert_eq!(layer.state_of(&ck).unwrap(), TcpState::FinWait2);
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn segment_checksum_protects_corruption() {
        let mut layer = make_layer();
        layer.bind(test_ip(), 8080).unwrap();
        layer.listen(8080).unwrap();

        let syn = layer.connect(50002, peer_ip(), 8080).unwrap();
        let mut bytes = syn[0].serialize(peer_ip(), test_ip());
        bytes[4] ^= 0xFF; // 破坏序列号但不重算校验和
        let out = layer.receive_segment(peer_ip(), test_ip(), &bytes).unwrap();
        assert!(out.is_empty()); // 校验不通过应被丢弃
    }
}
