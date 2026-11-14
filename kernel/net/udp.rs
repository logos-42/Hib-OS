//! 轻量 UDP 传输层（kernel/net/udp.rs）。
//!
//! 职责：UDP 首部的构造/解析、伪首部校验和计算与验证、按端口多路复用的
//! 接收队列与发送封装。它与更完整的 `udp_stack.rs` 并存——后者面向
//! 重量级收发队列与 socket 抽象，本模块则作为协议栈内层被
//! `ipv4.rs`（分发）、`socket.rs`（BSD 套接字）与 `mod.rs`（re-export）复用。
//!
//! 校验和算法遵循 RFC 768：伪首部(12B) + UDP 首部(校验和置 0) + 数据，
//! 16 位反码求和后取反；结果为 0 时按惯例发送 0xFFFF。

use std::collections::{BTreeMap, HashSet, VecDeque};

/// UDP 首部固定长度（字节）。
pub const UDP_HEADER_LEN: usize = 8;

/// UDP 首部。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpHeader {
    /// 源端口。
    pub src_port: u16,
    /// 目的端口。
    pub dst_port: u16,
    /// 总长度（首部 + 数据）。
    pub len: u16,
    /// 校验和（0 表示未启用校验）。
    pub checksum: u16,
}

impl UdpHeader {
    /// 构造首部（len 自动 = 8 + 载荷长度，校验和先置 0）。
    pub fn new(src_port: u16, dst_port: u16, payload_len: usize) -> Self {
        Self {
            src_port,
            dst_port,
            len: (UDP_HEADER_LEN + payload_len) as u16,
            checksum: 0,
        }
    }

    /// 首部固定长度。
    pub fn len() -> usize {
        UDP_HEADER_LEN
    }

    /// 从字节流解析首部（不足 8 字节返回 None）。
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < UDP_HEADER_LEN {
            return None;
        }
        Some(Self {
            src_port: u16::from_be_bytes([bytes[0], bytes[1]]),
            dst_port: u16::from_be_bytes([bytes[2], bytes[3]]),
            len: u16::from_be_bytes([bytes[4], bytes[5]]),
            checksum: u16::from_be_bytes([bytes[6], bytes[7]]),
        })
    }

    /// 序列化为 8 字节大端表示。
    pub fn serialize(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(UDP_HEADER_LEN);
        v.extend_from_slice(&self.src_port.to_be_bytes());
        v.extend_from_slice(&self.dst_port.to_be_bytes());
        v.extend_from_slice(&self.len.to_be_bytes());
        v.extend_from_slice(&self.checksum.to_be_bytes());
        v
    }
}

/// 对若干字节片段做 16 位反码求和（返回未取反的和）。
fn sum16(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    for part in parts {
        let mut i = 0;
        while i + 1 < part.len() {
            sum += u16::from_be_bytes([part[i], part[i + 1]]) as u32;
            i += 2;
        }
        // 奇数字节：高字节补 0。
        if i < part.len() {
            sum += (part[i] as u32) << 8;
        }
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum as u16
}

/// 构造 UDP 伪首部（12 字节）。
fn pseudo_header(src_ip: [u8; 4], dst_ip: [u8; 4], len: u16) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0..4].copy_from_slice(&src_ip);
    p[4..8].copy_from_slice(&dst_ip);
    p[8] = 0; // 保留
    p[9] = 17; // 协议号 UDP = 17
    p[10..12].copy_from_slice(&len.to_be_bytes());
    p
}

/// 计算 UDP 校验和（返回可直接写入首部的校验和字段值）。
pub fn udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], header: &UdpHeader, payload: &[u8]) -> u16 {
    let pseudo = pseudo_header(src_ip, dst_ip, header.len);
    let mut h = *header;
    h.checksum = 0; // 计算时校验和字段置 0
    let hdr = h.serialize();
    let sum = sum16(&[&pseudo, &hdr, payload]);
    let c = !sum;
    if c == 0 {
        0xFFFF
    } else {
        c
    }
}

/// 验证 UDP 校验和（首部校验和为 0 表示未启用校验，视为通过）。
pub fn verify_udp_checksum(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    header: &UdpHeader,
    payload: &[u8],
) -> bool {
    if header.checksum == 0 {
        return true;
    }
    let pseudo = pseudo_header(src_ip, dst_ip, header.len);
    let hdr = header.serialize(); // 含校验和字段
    sum16(&[&pseudo, &hdr, payload]) == 0xFFFF
}

/// UDP 层：按端口多路复用，维护已绑定端口与各端口的接收队列。
pub struct UdpLayer {
    /// 本机地址（保留用于后续校验/统计）。
    #[allow(dead_code)]
    local_ip: [u8; 4],
    /// 已绑定端口集合。
    bound: HashSet<u16>,
    /// 端口 -> 已接收数据报队列。
    queues: BTreeMap<u16, VecDeque<Vec<u8>>>,
}

impl UdpLayer {
    /// 新建 UDP 层。
    pub fn new(local_ip: [u8; 4]) -> Self {
        Self {
            local_ip,
            bound: HashSet::new(),
            queues: BTreeMap::new(),
        }
    }

    /// 绑定本地端口。
    pub fn bind(&mut self, port: u16) -> Result<(), String> {
        if self.bound.contains(&port) {
            return Err(format!("端口 {} 已被占用", port));
        }
        self.bound.insert(port);
        self.queues.entry(port).or_default();
        Ok(())
    }

    /// 接收路径：把一个到达的数据报投递到目的端口的队列；未绑定端口则丢弃。
    pub fn deliver(
        &mut self,
        _src_ip: [u8; 4],
        _src_port: u16,
        _dst_ip: [u8; 4],
        dst_port: u16,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        if let Some(q) = self.queues.get_mut(&dst_port) {
            q.push_back(payload);
            Ok(())
        } else {
            Err(format!("端口 {} 未绑定，丢弃", dst_port))
        }
    }

    /// 从指定端口队列取一个数据报（最多 size 字节）。
    pub fn recv(&mut self, port: u16, size: usize) -> Result<Vec<u8>, String> {
        let q = self
            .queues
            .get_mut(&port)
            .ok_or_else(|| format!("端口 {} 未绑定", port))?;
        let mut d = q.pop_front().ok_or_else(|| "无数据".to_string())?;
        d.truncate(size);
        Ok(d)
    }

    /// 发送路径：封装 UDP 首部并计算校验和，返回完整数据报字节。
    pub fn send(
        &mut self,
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        let mut header = UdpHeader::new(src_port, dst_port, payload.len());
        header.checksum = udp_checksum(src_ip, dst_ip, &header, payload);
        let mut out = header.serialize();
        out.extend_from_slice(payload);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(a: [u8; 4]) -> [u8; 4] {
        a
    }

    #[test]
    fn header_roundtrip() {
        let h = UdpHeader::new(1234, 5678, 10);
        assert_eq!(h.len, UDP_HEADER_LEN as u16 + 10);
        let bytes = h.serialize();
        assert_eq!(bytes.len(), UDP_HEADER_LEN);
        let p = UdpHeader::parse(&bytes).unwrap();
        assert_eq!(p, h);
    }

    #[test]
    fn checksum_roundtrip_validates() {
        let s = ip([10, 0, 0, 2]);
        let d = ip([10, 0, 0, 3]);
        let payload = b"hello udp checksum";
        let mut h = UdpHeader::new(5000, 9000, payload.len());
        h.checksum = udp_checksum(s, d, &h, payload);
        assert!(verify_udp_checksum(s, d, &h, payload));
        // 篡改载荷应校验失败。
        let mut bad = payload.to_vec();
        bad[0] ^= 0xFF;
        assert!(!verify_udp_checksum(s, d, &h, &bad));
    }

    #[test]
    fn zero_checksum_is_accepted() {
        let h = UdpHeader::new(1, 2, 3);
        assert_eq!(h.checksum, 0);
        assert!(verify_udp_checksum([0; 4], [0; 4], &h, b"abc"));
    }

    #[test]
    fn layer_bind_deliver_recv() {
        let mut l = UdpLayer::new([10, 0, 0, 2]);
        l.bind(5000).unwrap();
        assert!(l.deliver([10, 0, 0, 3], 6000, [10, 0, 0, 2], 5000, b"hi".to_vec()).is_ok());
        let got = l.recv(5000, 64).unwrap();
        assert_eq!(got, b"hi");
    }

    #[test]
    fn layer_send_builds_datagram() {
        let mut l = UdpLayer::new([10, 0, 0, 2]);
        let dg = l.send([10, 0, 0, 2], 5000, [10, 0, 0, 3], 9000, b"payload").unwrap();
        let h = UdpHeader::parse(&dg[..UDP_HEADER_LEN]).unwrap();
        assert_eq!(h.src_port, 5000);
        assert_eq!(h.dst_port, 9000);
        assert_eq!(h.len, (UDP_HEADER_LEN + 7) as u16);
        assert_eq!(h.checksum != 0, true);
    }

    #[test]
    fn duplicate_bind_fails() {
        let mut l = UdpLayer::new([10, 0, 0, 2]);
        l.bind(5000).unwrap();
        assert!(l.bind(5000).is_err());
    }
}

