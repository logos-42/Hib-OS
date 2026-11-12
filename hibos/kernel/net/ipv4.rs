//! IPv4：寻址、路由、分片/重组、ICMP、向上层（TCP/UDP）分发。
//!
//! 主机协议栈（不做转发）：收到的包若目的地址不是本机即丢弃。

use std::collections::HashMap;

use crate::net::{RouteEntry};
use crate::net::tcp::TcpSegment;
use crate::net::udp::UdpHeader;

/// 1 的补码校验和：对多个字节片段连续求和。
pub(crate) fn compute_checksum(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    for part in parts {
        let mut i = 0;
        while i + 1 < part.len() {
            sum += u16::from_be_bytes([part[i], part[i + 1]]) as u32;
            i += 2;
        }
        if i < part.len() {
            sum += (part[i] as u32) << 8;
        }
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

/// IPv4 协议号。
pub const IP_PROTO_ICMP: u8 = 1;
pub const IP_PROTO_TCP: u8 = 6;
pub const IP_PROTO_UDP: u8 = 17;

const IP_VERSION_IHL: u8 = 0x45; // IPv4, 20 字节首部，无选项

/// IPv4 首部（20 字节，无选项）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ipv4Header {
    pub dscp_ecn: u8,
    pub total_len: u16,
    pub identification: u16,
    /// 首部 3 位标志 + 13 位片偏移（以 8 字节为单位）。
    pub flags_fragment: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub src: [u8; 4],
    pub dst: [u8; 4],
}

const IP_FLAG_MF: u16 = 0x2000; // More Fragments

impl Ipv4Header {
    pub const LEN: usize = 20;

    pub fn more_fragments(&self) -> bool { self.flags_fragment & IP_FLAG_MF != 0 }
    pub fn fragment_offset(&self) -> u16 { self.flags_fragment & 0x1FFF }
}

/// IPv4 数据报。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ipv4Packet {
    pub header: Ipv4Header,
    pub payload: Vec<u8>,
}

impl Ipv4Packet {
    pub fn new(src: [u8; 4], dst: [u8; 4], ttl: u8, protocol: u8, payload: Vec<u8>) -> Self {
        let mut packet = Self {
            header: Ipv4Header {
                dscp_ecn: 0,
                total_len: (Self::header_len() + payload.len()) as u16,
                identification: 0,
                flags_fragment: 0,
                ttl,
                protocol,
                checksum: 0,
                src,
                dst,
            },
            payload,
        };
        packet.finalize();
        packet
    }

    pub const fn header_len() -> usize { 20 }

    /// 按当前首部字段重算校验和并填充。
    pub fn finalize(&mut self) {
        self.header.total_len = (Self::header_len() + self.payload.len()) as u16;
        let bytes = self.serialize_placeholder();
        let csum = compute_checksum(&[&bytes]);
        self.header.checksum = csum;
    }

    /// 序列化首部（校验和字段按当前值写入，通常为 0 placeholder，由 finalize 填真值）。
    pub fn serialize(&self) -> Vec<u8> {
        self.serialize_placeholder()
    }

    fn serialize_placeholder(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::header_len());
        b.push(IP_VERSION_IHL);
        b.push(self.header.dscp_ecn);
        b.extend_from_slice(&self.header.total_len.to_be_bytes());
        b.extend_from_slice(&self.header.identification.to_be_bytes());
        b.extend_from_slice(&self.header.flags_fragment.to_be_bytes());
        b.push(self.header.ttl);
        b.push(self.header.protocol);
        b.extend_from_slice(&self.header.checksum.to_be_bytes());
        b.extend_from_slice(&self.header.src);
        b.extend_from_slice(&self.header.dst);
        b
    }

    /// 解析一段字节为 IPv4 数据报（不校验校验和，校验在 receive 中显式进行）。
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::header_len() { return None; }
        let total_len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        if bytes.len() < total_len { return None; }
        Some(Self {
            header: Ipv4Header {
                dscp_ecn: bytes[1],
                total_len: total_len as u16,
                identification: u16::from_be_bytes([bytes[4], bytes[5]]),
                flags_fragment: u16::from_be_bytes([bytes[6], bytes[7]]),
                ttl: bytes[8],
                protocol: bytes[9],
                checksum: u16::from_be_bytes([bytes[10], bytes[11]]),
                src: [bytes[12], bytes[13], bytes[14], bytes[15]],
                dst: [bytes[16], bytes[17], bytes[18], bytes[19]],
            },
            payload: bytes[20..total_len].to_vec(),
        })
    }

    /// 验证首部校验和。
    pub fn verify_checksum(&self) -> bool {
        let bytes = self.serialize_placeholder();
        // 校验和字段置零后再求 1 的补码和，应为 0xFFFF（即算入字段自身后为 0）。
        let mut b = bytes;
        b[10] = 0;
        b[11] = 0;
        compute_checksum(&[&b]) == 0xFFFF
    }
}

/// 把一个 IPv4 数据报按 MTU 分片；每片数据长度为 8 的倍数（最后一片除外）。
pub fn fragment(packet: &Ipv4Packet, mtu: usize) -> Result<Vec<Ipv4Packet>, String> {
    let mtu = mtu.min(65535);
    let max_data = mtu.saturating_sub(Ipv4Packet::header_len());
    if max_data < 8 {
        return Err(format!("MTU 过小，无法分片 (mtu={})", mtu));
    }
    let max_data = max_data & !7;
    let payload = &packet.payload;
    if payload.len() <= max_data {
        return Ok(vec![packet.clone()]);
    }

    let id = packet.header.identification;
    let _src = packet.header.src;
    let _dst = packet.header.dst;
    let _ttl = packet.header.ttl;
    let _protocol = packet.header.protocol;

    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < payload.len() {
        let mut take = payload.len() - offset;
        take = take.min(max_data);
        let more = offset + take < payload.len();
        let mut hdr = packet.header.clone();
        hdr.identification = id;
        hdr.flags_fragment = if more { IP_FLAG_MF } else { 0 } | ((offset / 8) as u16);
        hdr.total_len = (Ipv4Packet::header_len() + take) as u16;
        let mut frag = Ipv4Packet {
            header: hdr,
            payload: payload[offset..offset + take].to_vec(),
        };
        frag.finalize();
        out.push(frag);
        offset += take;
    }
    Ok(out)
}

/// 单个分片重组状态。
struct ReassemblyEntry {
    header: Ipv4Header,
    /// (片偏移字节, 数据)
    fragments: Vec<(usize, Vec<u8>)>,
    /// 最后一片（MF=0）到达后才可知
    total: Option<usize>,
}

/// IPv4 分片重组缓冲区，key = (src, dst, identification)。
pub struct ReassemblyBuffer {
    entries: BTreeMap<([u8; 4], [u8; 4], u16), ReassemblyEntry>,
}

impl ReassemblyBuffer {
    pub fn new() -> Self {
        Self { entries: BTreeMap::new() }
    }

    /// 插入一片；全部齐全后返回重组好的包，否则返回 None（等待后续分片）。
    pub fn insert(&mut self, fragment: Ipv4Packet) -> Option<Ipv4Packet> {
        let key = (fragment.header.src, fragment.header.dst, fragment.header.identification);
        let offset = fragment.header.fragment_offset() as usize * 8;
        let more = fragment.header.more_fragments();
        let entry = self.entries.entry(key).or_insert_with(|| ReassemblyEntry {
            header: fragment.header.clone(),
            fragments: Vec::new(),
            total: None,
        });

        // 重复分片直接忽略
        if entry.fragments.iter().any(|(o, _)| *o == offset) {
            return None;
        }
        entry.fragments.push((offset, fragment.payload.clone()));
        if !more {
            entry.total = Some(offset + fragment.payload.len());
        }

        let total = match entry.total {
            Some(t) => t,
            None => return None,
        };

        // 校验覆盖范围是否从 0 开始且连续到 total
        let mut sorted = entry.fragments.clone();
        sorted.sort_by_key(|(o, _)| *o);
        let mut expect = 0usize;
        for (o, data) in &sorted {
            if *o != expect {
                return None;
            }
            expect += data.len();
        }
        if expect != total {
            return None;
        }

        let mut payload = Vec::with_capacity(total);
        for (_, data) in &sorted {
            payload.extend_from_slice(data);
        }
        let mut header = entry.header.clone();
        header.total_len = (Ipv4Packet::header_len() + total) as u16;
        header.flags_fragment = 0;
        let mut packet = Ipv4Packet { header, payload };
        packet.finalize();
        self.entries.remove(&key);
        Some(packet)
    }
}

/// ICMP 数据包（目前仅处理 Echo Request/Reply，用于 ping）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcmpPacket {
    pub icmp_type: u8,
    pub code: u8,
    pub checksum: u16,
    pub rest: [u8; 4], // Echo 里前两字节 identifier，后两字节 sequence
    pub payload: Vec<u8>,
}

pub const ICMP_ECHO_REQUEST: u8 = 8;
pub const ICMP_ECHO_REPLY: u8 = 0;

impl IcmpPacket {
    pub fn echo_request(id: u16, seq: u16, payload: Vec<u8>) -> Self {
        let mut pkt = Self {
            icmp_type: ICMP_ECHO_REQUEST,
            code: 0,
            checksum: 0,
            rest: [0, 0, 0, 0],
            payload,
        };
        pkt.rest[0..2].copy_from_slice(&id.to_be_bytes());
        pkt.rest[2..4].copy_from_slice(&seq.to_be_bytes());
        pkt.finalize();
        pkt
    }

    pub fn echo_reply(id: u16, seq: u16, payload: Vec<u8>) -> Self {
        let mut pkt = Self {
            icmp_type: ICMP_ECHO_REPLY,
            code: 0,
            checksum: 0,
            rest: [0, 0, 0, 0],
            payload,
        };
        pkt.rest[0..2].copy_from_slice(&id.to_be_bytes());
        pkt.rest[2..4].copy_from_slice(&seq.to_be_bytes());
        pkt.finalize();
        pkt
    }

    pub fn identifier(&self) -> u16 { u16::from_be_bytes([self.rest[0], self.rest[1]]) }
    pub fn sequence(&self) -> u16 { u16::from_be_bytes([self.rest[2], self.rest[3]]) }

    fn finalize(&mut self) {
        let b = self.serialize_placeholder();
        self.checksum = compute_checksum(&[&b]);
    }

    fn serialize_placeholder(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(4 + self.payload.len());
        b.push(self.icmp_type);
        b.push(self.code);
        b.extend_from_slice(&self.checksum.to_be_bytes());
        b.extend_from_slice(&self.rest);
        b.extend_from_slice(&self.payload);
        b
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut b = self.serialize_placeholder();
        b[2..4].copy_from_slice(&self.checksum.to_be_bytes());
        b
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 { return None; }
        Some(Self {
            icmp_type: bytes[0],
            code: bytes[1],
            checksum: u16::from_be_bytes([bytes[2], bytes[3]]),
            rest: [bytes[4], bytes[5], bytes[6], bytes[7]],
            payload: bytes[8..].to_vec(),
        })
    }

    /// 验证 ICMP 校验和（checksum 字段为 0 时跳过）。
    pub fn verify_checksum(&self) -> bool {
        if self.checksum == 0 { return true; }
        let mut b = self.serialize_placeholder();
        b[2] = 0;
        b[3] = 0;
        compute_checksum(&[&b]) == 0xFFFF
    }
}

/// IPv4 接收分发结果。
pub enum Ipv4Event {
    /// 丢弃。
    Drop,
    /// ICMP Echo Request 需要回应 Echo Reply。
    IcmpEchoRequest { src_ip: [u8; 4], payload: Vec<u8> },
    /// TCP 段交给 TCP 层。
    TcpSegment { src_ip: [u8; 4], dst_ip: [u8; 4], segment: TcpSegment },
    /// UDP 数据报交给 UDP 层。
    UdpDatagram { src_ip: [u8; 4], src_port: u16, dst_port: u16, payload: Vec<u8> },
}

/// IPv4 层实体。
pub struct Ipv4Layer {
    pub our_ip: [u8; 4],
    pub next_id: u16,
    pub reassembly: ReassemblyBuffer,
}

impl Ipv4Layer {
    pub fn new(our_ip: [u8; 4]) -> Self {
        Self {
            our_ip,
            next_id: 1,
            reassembly: ReassemblyBuffer::new(),
        }
    }

    /// 构建发往 dst 的 IPv4 数据报（按 mtu 必要时分片），返回尚未以太网封装的包列表。
    pub fn transmit(&mut self, dst: [u8; 4], proto: u8, payload: &[u8], mtu: usize) -> Result<Vec<Ipv4Packet>, String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let packet = Ipv4Packet::new(self.our_ip, dst, 64, proto, payload.to_vec());
        let mut packet = packet;
        packet.header.identification = id;
        packet.finalize();
        fragment(&packet, mtu)
    }

    /// 收到一个已以太网解封装的 IPv4 数据报，返回上层事件。
    pub fn receive(&mut self, packet: Ipv4Packet) -> Ipv4Event {
        let src_ip = packet.header.src;
        let dst_ip = packet.header.dst;

        // 主机协议栈：不做转发，非本机目的地址直接丢弃。
        if dst_ip != self.our_ip {
            return Ipv4Event::Drop;
        }
        if packet.header.ttl == 0 {
            return Ipv4Event::Drop;
        }
        if !packet.verify_checksum() {
            return Ipv4Event::Drop;
        }

        let packet = if packet.header.more_fragments() || packet.header.fragment_offset() != 0 {
            match self.reassembly.insert(packet) {
                Some(p) => p,
                None => return Ipv4Event::Drop, // 等待剩余分片
            }
        } else {
            packet
        };

        match packet.header.protocol {
            IP_PROTO_ICMP => {
                let icmp = match IcmpPacket::parse(&packet.payload) {
                    Some(i) => i,
                    None => return Ipv4Event::Drop,
                };
                if !icmp.verify_checksum() {
                    return Ipv4Event::Drop;
                }
                if icmp.icmp_type == ICMP_ECHO_REQUEST {
                    Ipv4Event::IcmpEchoRequest { src_ip, payload: icmp.payload }
                } else {
                    Ipv4Event::Drop
                }
            }
            IP_PROTO_TCP => {
                let segment = match TcpSegment::parse(&packet.payload, src_ip, dst_ip) {
                    Some(s) => s,
                    None => return Ipv4Event::Drop,
                };
                Ipv4Event::TcpSegment { src_ip, dst_ip, segment }
            }
            IP_PROTO_UDP => {
                if packet.payload.len() < UdpHeader::len() {
                    return Ipv4Event::Drop;
                }
                let header = match UdpHeader::parse(&packet.payload[..UdpHeader::len()]) {
                    Some(h) => h,
                    None => return Ipv4Event::Drop,
                };
                if header.checksum != 0 {
                    // 伪首部 + UDP 首部(校验和字段置零) + 数据
                    let pseudo = [
                        src_ip[0], src_ip[1], src_ip[2], src_ip[3],
                        dst_ip[0], dst_ip[1], dst_ip[2], dst_ip[3],
                        0, IP_PROTO_UDP,
                        (header.len >> 8) as u8, (header.len & 0xFF) as u8,
                    ];
                    let mut udp_bytes = header.serialize();
                    udp_bytes.extend_from_slice(&packet.payload[UdpHeader::len()..]);
                    if compute_checksum(&[&pseudo, &udp_bytes]) != 0xFFFF {
                        return Ipv4Event::Drop;
                    }
                }
                let payload = packet.payload[UdpHeader::len()..].to_vec();
                Ipv4Event::UdpDatagram { src_ip, src_port: header.src_port, dst_port: header.dst_port, payload }
            }
            _ => Ipv4Event::Drop,
        }
    }
}

/// 最长前缀匹配路由：返回应当发包的下一跳 IP；无路由返回 None。
pub fn lookup_route(routes: &[RouteEntry], iface_local: impl Fn([u8; 4]) -> bool, dst: [u8; 4]) -> Option<[u8; 4]> {
    // 1. 本子网直接投递
    if iface_local(dst) {
        return Some(dst);
    }
    // 2. 最长前缀匹配
    let mut best: Option<(&RouteEntry, u32)> = None;
    for r in routes {
        let match_net = [
            r.network[0] & r.mask[0], r.network[1] & r.mask[1],
            r.network[2] & r.mask[2], r.network[3] & r.mask[3],
        ];
        let match_dst = [
            dst[0] & r.mask[0], dst[1] & r.mask[1],
            dst[2] & r.mask[2], dst[3] & r.mask[3],
        ];
        if match_net == match_dst {
            let plen: u32 = r.mask.iter().map(|m| m.count_ones() as u32).sum();
            match best {
                None => best = Some((r, plen)),
                Some((_, bp)) if plen > bp => best = Some((r, plen)),
                _ => {}
            }
        }
    }
    best.map(|(r, _)| r.gateway)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Interface;

    fn test_iface() -> Interface {
        Interface {
            name: "eth0".to_string(),
            mac: [0x02, 0, 0, 0, 0, 1],
            ip: [10, 0, 0, 2],
            mask: [255, 255, 255, 0],
            link_up: true,
            mtu: 1500,
        }
    }

    #[test]
    fn header_serialize_parse_roundtrip() {
        let pkt = Ipv4Packet::new([10, 0, 0, 2], [10, 0, 0, 3], 64, IP_PROTO_ICMP, b"ping payload".to_vec());
        let bytes = pkt.serialize();
        assert_eq!(bytes.len(), 20 + 12);
        let parsed = Ipv4Packet::parse(&bytes).unwrap();
        assert_eq!(parsed.header.src, [10, 0, 0, 2]);
        assert_eq!(parsed.header.dst, [10, 0, 0, 3]);
        assert_eq!(parsed.header.ttl, 64);
        assert_eq!(parsed.header.protocol, IP_PROTO_ICMP);
        assert_eq!(parsed.payload, b"ping payload");
        assert!(parsed.verify_checksum());
    }

    #[test]
    fn fragment_and_reassemble() {
        let mut layer = Ipv4Layer::new([10, 0, 0, 2]);
        let payload: Vec<u8> = (0..3000u16).map(|x| x as u8).collect();
        let packets = layer.transmit([10, 0, 0, 3], IP_PROTO_UDP, &payload, 1500).unwrap();
        assert!(packets.len() > 1, "应当分片: {}", packets.len());
        let id = packets[0].header.identification;
        for p in &packets {
            assert_eq!(p.header.identification, id);
            assert_eq!(p.header.src, [10, 0, 0, 2]);
            assert_eq!(p.header.dst, [10, 0, 0, 3]);
            // 除最后一片外，数据长度必须是 8 的倍数
            let is_last = !p.header.more_fragments();
            if !is_last {
                assert_eq!(p.payload.len() % 8, 0);
            }
        }

        // 乱序重组
        let mut buf = ReassemblyBuffer::new();
        let mut completed = None;
        for p in packets.iter().rev() {
            completed = buf.insert(p.clone());
            if completed.is_some() { break; }
        }
        let completed = completed.expect("分片应能重组");
        assert_eq!(completed.payload, payload);
        assert_eq!(completed.header.protocol, IP_PROTO_UDP);
        assert!(completed.verify_checksum());
    }

    #[test]
    fn fragment_incomplete_waits() {
        let mut layer = Ipv4Layer::new([10, 0, 0, 2]);
        let payload: Vec<u8> = (0..3000u16).map(|x| x as u8).collect();
        let packets = layer.transmit([10, 0, 0, 3], IP_PROTO_UDP, &payload, 1500).unwrap();
        let mut buf = ReassemblyBuffer::new();
        // 只投递第一片和最后一片，缺中间一片应返回 None
        let r1 = buf.insert(packets[0].clone());
        assert!(r1.is_none());
        let r2 = buf.insert(packets[packets.len() - 1].clone());
        assert!(r2.is_none());
    }

    #[test]
    fn icmp_echo_roundtrip() {
        let req = IcmpPacket::echo_request(0x1234, 1, b"hello ping".to_vec());
        let parsed = IcmpPacket::parse(&req.serialize()).unwrap();
        assert_eq!(parsed.icmp_type, ICMP_ECHO_REQUEST);
        assert_eq!(parsed.identifier(), 0x1234);
        assert_eq!(parsed.sequence(), 1);
        assert_eq!(parsed.payload, b"hello ping");
        assert!(parsed.verify_checksum());

        let rep = IcmpPacket::echo_reply(0x1234, 1, b"hello ping".to_vec());
        let parsed = IcmpPacket::parse(&rep.serialize()).unwrap();
        assert_eq!(parsed.icmp_type, ICMP_ECHO_REPLY);
        assert!(parsed.verify_checksum());
    }

    #[test]
    fn transmit_no_fragment_when_small() {
        let mut layer = Ipv4Layer::new([10, 0, 0, 2]);
        let packets = layer.transmit([10, 0, 0, 3], IP_PROTO_ICMP, b"tiny", 1500).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].payload, b"tiny");
    }
}

