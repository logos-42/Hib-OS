//! 以太网帧与 ARP —— 链路层。
//!
//! 提供以太网 II 帧的封装/解析、MAC 地址表示、ARP 请求/应答包的序列化，
//! 以及协议栈共用的 ARP 表查询辅助（ARP 条目本身由 `NetStack` 持有，见 `mod.rs`）。

use std::collections::BTreeMap;

/// 以太网 II 帧头固定 14 字节。
pub const ETH_HEADER_LEN: usize = 14;
/// 以太网帧最小长度（不含 CRC）。
pub const ETH_MIN_FRAME_LEN: usize = 64;
/// 默认链路 MTU。
pub const ETH_MTU: u16 = 1500;

/// EtherType。
pub const ETH_TYPE_IPV4: u16 = 0x0800;
pub const ETH_TYPE_ARP: u16 = 0x0806;

/// 广播 MAC。
pub const ETH_BROADCAST: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

/// 以太网 II 帧头。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetHeader {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
}

/// 以太网 II 帧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetFrame {
    pub header: EthernetHeader,
    pub payload: Vec<u8>,
}

impl EthernetFrame {
    pub fn new(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: Vec<u8>) -> Self {
        Self { header: EthernetHeader { dst, src, ethertype }, payload }
    }

    /// 序列化为字节流；帧总长不足 ETH_MIN_FRAME_LEN 时用零填充。
    pub fn serialize(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ETH_HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(&self.header.dst);
        bytes.extend_from_slice(&self.header.src);
        bytes.extend_from_slice(&self.header.ethertype.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        let pad = ETH_MIN_FRAME_LEN.saturating_sub(bytes.len());
        bytes.extend(core::iter::repeat(0u8).take(pad));
        bytes
    }

    /// 解析一段字节为以太网帧；不足 14 字节返回 None。
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < ETH_HEADER_LEN {
            return None;
        }
        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];
        dst.copy_from_slice(&bytes[0..6]);
        src.copy_from_slice(&bytes[6..12]);
        let ethertype = u16::from_be_bytes([bytes[12], bytes[13]]);
        Some(Self {
            header: EthernetHeader { dst, src, ethertype },
            payload: bytes[14..].to_vec(),
        })
    }
}

/// ARP 操作码。
pub const ARP_OP_REQUEST: u16 = 1;
pub const ARP_OP_REPLY: u16 = 2;

/// ARP 包（硬件类型 1/以太网，协议类型 0x0800/IPv4，地址长度 6/4），固定 28 字节。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArpPacket {
    pub op: u16,
    pub sender_mac: [u8; 6],
    pub sender_ip: [u8; 4],
    pub target_mac: [u8; 6],
    pub target_ip: [u8; 4],
}

impl ArpPacket {
    pub const LEN: usize = 28;

    pub fn request(sender_mac: [u8; 6], sender_ip: [u8; 4], target_ip: [u8; 4]) -> Self {
        Self {
            op: ARP_OP_REQUEST,
            sender_mac,
            sender_ip,
            target_mac: [0, 0, 0, 0, 0, 0],
            target_ip,
        }
    }

    pub fn reply(sender_mac: [u8; 6], sender_ip: [u8; 4],
                 target_mac: [u8; 6], target_ip: [u8; 4]) -> Self {
        Self {
            op: ARP_OP_REPLY,
            sender_mac,
            sender_ip,
            target_mac,
            target_ip,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::LEN);
        bytes.extend_from_slice(&1u16.to_be_bytes());      // hardware type: Ethernet
        bytes.extend_from_slice(&0x0800u16.to_be_bytes()); // protocol type: IPv4
        bytes.push(6);  // hardware size
        bytes.push(4);  // protocol size
        bytes.extend_from_slice(&self.op.to_be_bytes());
        bytes.extend_from_slice(&self.sender_mac);
        bytes.extend_from_slice(&self.sender_ip);
        bytes.extend_from_slice(&self.target_mac);
        bytes.extend_from_slice(&self.target_ip);
        bytes
    }

    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::LEN { return None; }
        Some(Self {
            op: u16::from_be_bytes([bytes[6], bytes[7]]),
            sender_mac: [bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13]],
            sender_ip: [bytes[14], bytes[15], bytes[16], bytes[17]],
            target_mac: [bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23]],
            target_ip: [bytes[24], bytes[25], bytes[26], bytes[27]],
        })
    }
}

/// ARP 表查询辅助：IP -> MAC；未知条目返回广播 MAC（与旧 net.rs 的 arp_resolve 语义一致）。
pub fn arp_resolve(arp: &BTreeMap<[u8; 4], [u8; 6]>, ip: [u8; 4]) -> [u8; 6] {
    arp.get(&ip).copied().unwrap_or(ETH_BROADCAST)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eth_frame() -> EthernetFrame {
        EthernetFrame::new(
            [0x02, 0x00, 0x00, 0x00, 0x00, 0x02],
            [0x02, 0x00, 0x00, 0x00, 0x00, 0x01],
            ETH_TYPE_IPV4,
            b"hello ethernet".to_vec(),
        )
    }

    #[test]
    fn frame_serialize_and_parse_roundtrip() {
        let f = eth_frame();
        let bytes = f.serialize();
        assert!(bytes.len() >= ETH_MIN_FRAME_LEN);
        let parsed = EthernetFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.header.dst, f.header.dst);
        assert_eq!(parsed.header.src, f.header.src);
        assert_eq!(parsed.header.ethertype, ETH_TYPE_IPV4);
        assert_eq!(parsed.payload, b"hello ethernet");
    }

    #[test]
    fn frame_pad_to_min_length() {
        let f = EthernetFrame::new([0; 6], [0; 6], ETH_TYPE_ARP, b"x".to_vec());
        let bytes = f.serialize();
        assert!(bytes.len() >= ETH_MIN_FRAME_LEN);
        // 以太网头 + 负载 + 填充
        assert_eq!(&bytes[12..14], &ETH_TYPE_ARP.to_be_bytes());
    }

    #[test]
    fn frame_parse_too_short() {
        assert!(EthernetFrame::parse(&[0u8; 10]).is_none());
    }

    #[test]
    fn arp_request_and_reply_roundtrip() {
        let req = ArpPacket::request([0x02, 0, 0, 0, 0, 1], [10, 0, 0, 2], [10, 0, 0, 3]);
        let bytes = req.serialize();
        assert_eq!(bytes.len(), ArpPacket::LEN);
        let parsed = ArpPacket::parse(&bytes).unwrap();
        assert_eq!(parsed.op, ARP_OP_REQUEST);
        assert_eq!(parsed.sender_ip, [10, 0, 0, 2]);
        assert_eq!(parsed.target_ip, [10, 0, 0, 3]);

        let rep = ArpPacket::reply([0x02, 0x00, 0x00, 0x00, 0x00, 0x03], [10, 0, 0, 3],
                                   [0x02, 0x00, 0x00, 0x00, 0x00, 0x01], [10, 0, 0, 2]);
        let parsed = ArpPacket::parse(&rep.serialize()).unwrap();
        assert_eq!(parsed.op, ARP_OP_REPLY);
        assert_eq!(parsed.target_mac, [0x02, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn arp_resolve_known_and_unknown() {
        let mut arp = BTreeMap::new();
        arp.insert([10, 0, 0, 3], [0x02, 0x00, 0x00, 0x00, 0x00, 0x03]);
        assert_eq!(arp_resolve(&arp, [10, 0, 0, 3]), [0x02, 0x00, 0x00, 0x00, 0x00, 0x03]);
        assert_eq!(arp_resolve(&arp, [10, 0, 0, 9]), ETH_BROADCAST);
        assert!(arp_resolve(&arp, [0, 0, 0, 0]) == ETH_BROADCAST);
    }
}

