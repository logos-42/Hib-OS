//! DHCP 客户端（kernel/net/dhcp.rs）。
//!
//! 本模块实现 DHCPv4 客户端，完成动态 IP 地址分配、租约续期与释放。
//! 为演示级协议栈：收发由上层（UDP/IP/以太网）完成，本模块负责报文构造、
//! 状态机（INIT/SELECTING/REQUESTING/BOUND/RENEWING/REBINDING）与租约管理。
//!
//! 特性:
//! * DhcpLease：IP 地址、子网掩码、网关、DNS 服务器、租约时间、剩余时间
//! * DhcpClient：new / discover / request / renew / release / bound / lease
//! * DHCP 报文：DISCOVER/OFFER/REQUEST/ACK/NAK/RELEASE、XID 事务 ID、选项字段
//! * sense/act 通道：`net://dhcp`

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =======================================================================
// DHCP 报文类型与常量
// =======================================================================

/// DHCP 消息类型（选项 53）。
pub const DHCP_DISCOVER: u8 = 1;
pub const DHCP_OFFER: u8 = 2;
pub const DHCP_REQUEST: u8 = 3;
pub const DHCP_DECLINE: u8 = 4;
pub const DHCP_ACK: u8 = 5;
pub const DHCP_NAK: u8 = 6;
pub const DHCP_RELEASE: u8 = 7;
pub const DHCP_INFORM: u8 = 8;

// DHCP 选项代码。
pub const DHCP_OPT_SUBNET_MASK: u8 = 1;
pub const DHCP_OPT_ROUTER: u8 = 3;
pub const DHCP_OPT_DNS_SERVER: u8 = 6;
pub const DHCP_OPT_REQ_IP: u8 = 50;
pub const DHCP_OPT_LEASE_TIME: u8 = 51;
pub const DHCP_OPT_MSG_TYPE: u8 = 53;
pub const DHCP_OPT_SERVER_ID: u8 = 54;
pub const DHCP_OPT_RENEWAL: u8 = 58;   // T1
pub const DHCP_OPT_REBIND: u8 = 59;    // T2
pub const DHCP_OPT_END: u8 = 255;

// DHCP 端口。
pub const DHCP_CLIENT_PORT: u16 = 68;
pub const DHCP_SERVER_PORT: u16 = 67;

// =======================================================================
// DHCP 租约
// ======================================================================

/// DHCP 租约信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DhcpLease {
    /// 分配到的 IP 地址。
    pub ip: [u8; 4],
    /// 子网掩码。
    pub subnet_mask: [u8; 4],
    /// 默认网关。
    pub gateway: [u8; 4],
    /// DNS 服务器（可多个）。
    pub dns_servers: Vec<[u8; 4]>,
    /// 租约总时长（秒）。
    pub lease_time: u32,
    /// 租约剩余时长（秒，由 tick 推进）。
    pub remaining_time: u32,
    /// 服务器标识符（DHCP 服务器 IP）。
    pub server_id: [u8; 4],
    /// 续期时间 T1（秒，通常 lease_time / 2）。
    pub renewal_time: u32,
    /// 绑定时间点（秒，相对启动基准）。
    pub obtained_at_secs: u32,
}

impl DhcpLease {
    /// 租约是否已过期（剩余时间 <= 0）。
    pub fn is_expired(&self) -> bool { self.remaining_time == 0 }

    /// 租约是否已绑定（有效 IP 且非 0.0.0.0）。
    pub fn is_bound(&self) -> bool { self.ip != [0, 0, 0, 0] }

    /// 格式化租约摘要。
    pub fn summary(&self) -> String {
        format!(
            "ip={}.{}.{}.{} mask={}.{}.{}.{} gw={}.{}.{}.{} dns=[{}] lease={}s remain={}s server={}.{}.{}.{}",
            self.ip[0], self.ip[1], self.ip[2], self.ip[3],
            self.subnet_mask[0], self.subnet_mask[1], self.subnet_mask[2], self.subnet_mask[3],
            self.gateway[0], self.gateway[1], self.gateway[2], self.gateway[3],
            self.dns_servers.iter().map(|d| format!("{}.{}.{}.{}", d[0], d[1], d[2], d[3])).collect::<Vec<_>>().join(","),
            self.lease_time, self.remaining_time,
            self.server_id[0], self.server_id[1], self.server_id[2], self.server_id[3],
        )
    }
}

// =======================================================================
// DHCP 客户端状态机
// ======================================================================

/// DHCP 客户端状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DhcpState {
    /// 初始：无租约，准备发送 DISCOVER。
    Init,
    /// 已发送 DISCOVER，等待 OFFER。
    Selecting,
    /// 已发送 REQUEST，等待 ACK/NAK。
    Requesting,
    /// 已绑定 IP，可正常使用。
    Bound,
    /// 续期中：租约过半，发送 REQUEST 续期。
    Renewing,
    /// 重新绑定中：无法联系原服务器，广播 REQUEST。
    Rebinding,
    /// 已释放，回到无租约。
    Released,
}

impl DhcpState {
    pub fn name(self) -> &'static str {
        match self {
            DhcpState::Init => "INIT",
            DhcpState::Selecting => "SELECTING",
            DhcpState::Requesting => "REQUESTING",
            DhcpState::Bound => "BOUND",
            DhcpState::Renewing => "RENEWING",
            DhcpState::Rebinding => "REBINDING",
            DhcpState::Released => "RELEASED",
        }
    }
}

/// DHCP 客户端：管理状态机、XID 事务 ID、租约。
pub struct DhcpClient {
    /// 接口名称（如 "eth0"）。
    pub iface_name: String,
    /// 本机 MAC 地址（CHADDR）。
    pub client_mac: [u8; 6],
    /// 当前状态。
    pub state: DhcpState,
    /// 当前租约（Bound 及以后有效）。
    pub lease: DhcpLease,
    /// 事务 ID（XID）。
    pub xid: u32,
    /// 选中的服务器 IP（Selecting/Requesting 阶段）。
    pub server_id: [u8; 4],
    /// 请求的 IP（Requesting 阶段）。
    pub requested_ip: [u8; 4],
    /// 上次发送 DISCOVER/REQUEST 的时刻（秒，相对基准）。
    pub last_action_secs: u32,
    /// 重传计数。
    pub retries: u32,
    /// 已收到的 OFFER 报文（供选择）。
    pub offers: VecDeque<DhcpOffer>,
    /// 重传超时基数（秒，指数退避）。
    pub timeout_base_secs: u32,
}

/// DHCP OFFER 缓存。
#[derive(Clone, Debug)]
pub struct DhcpOffer {
    pub xid: u32,
    pub server_id: [u8; 4],
    pub offered_ip: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns_servers: Vec<[u8; 4]>,
    pub lease_time: u32,
}

/// DHCP 客户端句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DhcpClientId(pub u64);

impl DhcpClient {
    /// 为指定网卡创建 DHCP 客户端（状态 INIT，无租约）。
    pub fn new(iface_name: &str, client_mac: [u8; 6]) -> Self {
        Self {
            iface_name: iface_name.to_string(),
            client_mac,
            state: DhcpState::Init,
            lease: DhcpLease {
                ip: [0, 0, 0, 0],
                subnet_mask: [0, 0, 0, 0],
                gateway: [0, 0, 0, 0],
                dns_servers: Vec::new(),
                lease_time: 0,
                remaining_time: 0,
                server_id: [0, 0, 0, 0],
                renewal_time: 0,
                obtained_at_secs: 0,
            },
            xid: 0x1234_5678,
            server_id: [0, 0, 0, 0],
            requested_ip: [0, 0, 0, 0],
            last_action_secs: 0,
            retries: 0,
            offers: VecDeque::new(),
            timeout_base_secs: 4,
        }
    }

    /// 全局 DHCP 客户端实例（`net://dhcp` 通道后端，默认 eth0）。
    pub fn client() -> &'static Mutex<DhcpClient> {
        static CLIENT: OnceLock<Mutex<DhcpClient>> = OnceLock::new();
        CLIENT.get_or_init(|| Mutex::new(Self::new("eth0", [0x02, 0x00, 0x00, 0x00, 0x00, 0x01])))
    }

    /// 绑定租约（由 ACK 完成）。
    fn bind_lease(&mut self, ip: [u8; 4], mask: [u8; 4], gateway: [u8; 4], dns: Vec<[u8; 4]>, lease_time: u32, now_secs: u32) {
        self.lease = DhcpLease {
            ip,
            subnet_mask: mask,
            gateway,
            dns_servers: dns,
            lease_time,
            remaining_time: lease_time,
            server_id: self.server_id,
            renewal_time: lease_time / 2,
            obtained_at_secs: now_secs,
        };
        self.state = DhcpState::Bound;
        self.retries = 0;
        self.timeout_base_secs = 4;
    }

    /// 租约流逝（由 tick 调用，每秒推进一次）。
    pub fn tick_lease(&mut self, secs: u32) {
        match self.state {
            DhcpState::Bound => {
                if self.lease.remaining_time > secs {
                    self.lease.remaining_time -= secs;
                } else {
                    self.lease.remaining_time = 0;
                }
                // 过半进入 Renewing。
                if self.lease.remaining_time <= self.lease.lease_time / 2 && self.lease.remaining_time > 0 {
                    self.state = DhcpState::Renewing;
                }
            }
            DhcpState::Renewing => {
                if self.lease.remaining_time > secs {
                    self.lease.remaining_time -= secs;
                } else {
                    self.lease.remaining_time = 0;
                }
                if self.lease.remaining_time == 0 {
                    self.state = DhcpState::Rebinding;
                }
            }
            DhcpState::Rebinding => {
                if self.lease.remaining_time > secs {
                    self.lease.remaining_time -= secs;
                } else {
                    self.lease.remaining_time = 0;
                    self.state = DhcpState::Init; // 租约尽失，重新发现
                    self.lease.ip = [0, 0, 0, 0];
                }
            }
            _ => {}
        }
    }

    // ------ DHCP 报文构造 ------

    /// 构造 DHCP DISCOVER 报文（广播，xid 随机化）。
    pub fn discover(&mut self) -> DhcpMessage {
        self.xid = self.xid.wrapping_add(1);
        self.state = DhcpState::Selecting;
        self.last_action_secs = 0; // 由上层填充
        self.retries = self.retries.saturating_add(1);
        self.offers.clear();
        let mut msg = DhcpMessage {
            op: 1, // BOOTREQUEST
            htype: 1, hlen: 6, hops: 0,
            xid: self.xid,
            secs: 0, flags: 0x8000, // 广播
            ciaddr: [0, 0, 0, 0], yiaddr: [0, 0, 0, 0], siaddr: [0, 0, 0, 0], giaddr: [0, 0, 0, 0],
            chaddr: self.client_mac,
            chaddr_pad: [0; 10],
            xid_pad: [0; 64],
            sname: [0; 64],
            file: [0; 128],
            magic_cookie: [0x63, 0x82, 0x53, 0x63],
            options: Vec::new(),
        };
        msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_DISCOVER);
        msg.add_option_u32(DHCP_OPT_REQ_IP, [0, 0, 0, 0]); // 无请求 IP
        msg.add_option_end();
        msg
    }

    /// 根据 OFFER 构造 REQUEST 报文。
    pub fn request(&mut self, offered: &DhcpOffer) -> DhcpMessage {
        self.xid = offered.xid; // 使用服务器回复的 xid
        self.server_id = offered.server_id;
        self.requested_ip = offered.offered_ip;
        self.state = DhcpState::Requesting;
        self.retries = self.retries.saturating_add(1);
        let mut msg = DhcpMessage {
            op: 1, htype: 1, hlen: 6, hops: 0,
            xid: self.xid,
            secs: 0, flags: 0x8000,
            ciaddr: [0, 0, 0, 0], yiaddr: [0, 0, 0, 0], siaddr: [0, 0, 0, 0], giaddr: [0, 0, 0, 0],
            chaddr: self.client_mac,
            chaddr_pad: [0; 10],
            xid_pad: [0; 64],
            sname: [0; 64],
            file: [0; 128],
            magic_cookie: [0x63, 0x82, 0x53, 0x63],
            options: Vec::new(),
        };
        msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_REQUEST);
        msg.add_option_ip(DHCP_OPT_REQ_IP, offered.offered_ip);
        msg.add_option_ip(DHCP_OPT_SERVER_ID, offered.server_id);
        msg.add_option_end();
        msg
    }

    /// 续期：向原服务器发送 REQUEST（ciaddr 为当前 IP，unicast）。
    pub fn renew(&mut self) -> DhcpMessage {
        if !self.lease.is_bound() {
            // 未绑定：无法续期，返回空请求（调用方应先 discover）。
            return self.discover();
        }
        self.state = DhcpState::Renewing;
        self.xid = self.xid.wrapping_add(1);
        let mut msg = DhcpMessage {
            op: 1, htype: 1, hlen: 6, hops: 0,
            xid: self.xid,
            secs: 0, flags: 0, // unicast
            ciaddr: self.lease.ip, yiaddr: [0, 0, 0, 0], siaddr: [0, 0, 0, 0], giaddr: [0, 0, 0, 0],
            chaddr: self.client_mac,
            chaddr_pad: [0; 10],
            xid_pad: [0; 64],
            sname: [0; 64],
            file: [0; 128],
            magic_cookie: [0x63, 0x82, 0x53, 0x63],
            options: Vec::new(),
        };
        msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_REQUEST);
        msg.add_option_ip(DHCP_OPT_REQ_IP, self.lease.ip);
        msg.add_option_ip(DHCP_OPT_SERVER_ID, self.lease.server_id);
        msg.add_option_end();
        msg
    }

    /// 释放：向服务器发送 RELEASE。
    pub fn release(&mut self) -> DhcpMessage {
        self.xid = self.xid.wrapping_add(1);
        let mut msg = DhcpMessage {
            op: 1, htype: 1, hlen: 6, hops: 0,
            xid: self.xid,
            secs: 0, flags: 0,
            ciaddr: self.lease.ip, yiaddr: [0, 0, 0, 0], siaddr: [0, 0, 0, 0], giaddr: [0, 0, 0, 0],
            chaddr: self.client_mac,
            chaddr_pad: [0; 10],
            xid_pad: [0; 64],
            sname: [0; 64],
            file: [0; 128],
            magic_cookie: [0x63, 0x82, 0x53, 0x63],
            options: Vec::new(),
        };
        msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_RELEASE);
        msg.add_option_end();
        self.state = DhcpState::Released;
        self.lease.ip = [0, 0, 0, 0];
        self.lease.remaining_time = 0;
        msg
    }

    // ------ 报文处理 ------

    /// 处理收到的 DHCP OFFER：缓存，供 request() 选择。
    pub fn handle_offer(&mut self, msg: &DhcpMessage) {
        if let Some(offer) = parse_offer(msg) {
            self.offers.push_back(offer);
        }
    }

    /// 处理收到的 DHCP ACK：绑定租约。
    pub fn handle_ack(&mut self, msg: &DhcpMessage, now_secs: u32) -> Result<DhcpLease, String> {
        if msg.xid != self.xid && msg.xid != self.requested_ip_xid() {
            return Err("XID 不匹配".to_string());
        }
        let (ip, mask, gateway, dns, lease_time, server_id) = parse_ack(msg)?;
        self.bind_lease(ip, mask, gateway, dns, lease_time, now_secs);
        self.server_id = server_id;
        Ok(self.lease.clone())
    }

    /// 处理收到的 DHCP NAK：返回 INIT，重新开始。
    pub fn handle_nak(&mut self) {
        self.state = DhcpState::Init;
        self.lease.ip = [0, 0, 0, 0];
        self.lease.remaining_time = 0;
        self.retries = 0;
    }

    /// 是否已绑定有效租约。
    pub fn bound(&self) -> bool { self.state == DhcpState::Bound && self.lease.is_bound() }

    /// 获取当前租约的克隆。
    pub fn lease(&self) -> DhcpLease { self.lease.clone() }

    /// 当前状态名称。
    pub fn state_name(&self) -> &'static str { self.state.name() }

    /// 请求的事务 ID 校验辅助（REQUEST 阶段的 xid 即 offer.xid）。
    fn requested_ip_xid(&self) -> u32 { self.xid }

    /// 序列化 DHCP 报文为字节流（用于 UDP 发送）。
    pub fn serialize(msg: &DhcpMessage) -> Vec<u8> {
        msg.serialize()
    }

    /// 从字节流解析 DHCP 报文。
    pub fn parse(bytes: &[u8]) -> Option<DhcpMessage> {
        DhcpMessage::parse(bytes)
    }

    /// 状态快照（供 sense）。
    pub fn status(&self) -> String {
        format!(
            "dhcp iface={} state={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} xid={} lease={} retries={}",
            self.iface_name, self.state.name(),
            self.client_mac[0], self.client_mac[1], self.client_mac[2], self.client_mac[3], self.client_mac[4], self.client_mac[5],
            self.xid,
            if self.lease.is_bound() { "bound" } else { "none" },
            self.retries,
        )
    }
}

// =======================================================================
// DHCP 报文（BOOTP/DHCP，236 字节固定头 + 选项）
// ======================================================================

/// DHCP/BOOTP 报文（简化版，固定头 + 选项字段）。
#[derive(Clone, Debug)]
pub struct DhcpMessage {
    pub op: u8,
    pub htype: u8,
    pub hlen: u8,
    pub hops: u8,
    pub xid: u32,
    pub secs: u16,
    pub flags: u16,
    pub ciaddr: [u8; 4],
    pub yiaddr: [u8; 4],
    pub siaddr: [u8; 4],
    pub giaddr: [u8; 4],
    pub chaddr: [u8; 6],
    pub chaddr_pad: [u8; 10],
    pub xid_pad: [u8; 64],
    pub sname: [u8; 64],
    pub file: [u8; 128],
    pub magic_cookie: [u8; 4],
    pub options: Vec<u8>,
}

impl DhcpMessage {
    /// 固定头长度（不含选项与 magic cookie）。
    const FIXED_LEN: usize = 236;

    /// 添加字节选项。
    pub fn add_option_bytes(&mut self, code: u8, data: &[u8]) {
        self.options.push(code);
        self.options.push(data.len() as u8);
        self.options.extend_from_slice(data);
    }

    /// 添加 u8 选项。
    pub fn add_option_u8(&mut self, code: u8, value: u8) {
        self.add_option_bytes(code, &[value]);
    }

    /// 添加 IP 地址选项（4 字节）。
    pub fn add_option_ip(&mut self, code: u8, ip: [u8; 4]) {
        self.add_option_bytes(code, &ip);
    }

    /// 添加结束选项（255）。
    pub fn add_option_end(&mut self) {
        self.options.push(DHCP_OPT_END);
    }

    /// 获取选项值（首个匹配 code）。
    pub fn get_option(&self, code: u8) -> Option<Vec<u8>> {
        let mut i = 0;
        while i + 1 < self.options.len() {
            let c = self.options[i];
            if c == DHCP_OPT_END { break; }
            if c == 0 { i += 1; continue; } // 填充
            let len = self.options[i + 1] as usize;
            if i + 2 + len <= self.options.len() {
                if c == code {
                    return Some(self.options[i + 2..i + 2 + len].to_vec());
                }
                i += 2 + len;
            } else {
                break;
            }
        }
        None
    }

    /// 获取 u8 选项。
    pub fn get_option_u8(&self, code: u8) -> Option<u8> {
        self.get_option(code).and_then(|v| v.first().copied())
    }

    /// 获取 u32（4 字节）选项。
    pub fn get_option_u32(&self, code: u8) -> Option<u32> {
        self.get_option(code).and_then(|v| {
            if v.len() >= 4 {
                Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
            } else { None }
        })
    }

    /// 获取 IP 选项。
    pub fn get_option_ip(&self, code: u8) -> Option<[u8; 4]> {
        self.get_option(code).and_then(|v| {
            if v.len() >= 4 {
                Some([v[0], v[1], v[2], v[3]])
            } else { None }
        })
    }

    /// 序列化为字节流。
    pub fn serialize(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(Self::FIXED_LEN + self.options.len());
        b.push(self.op);
        b.push(self.htype);
        b.push(self.hlen);
        b.push(self.hops);
        b.extend_from_slice(&self.xid.to_be_bytes());
        b.extend_from_slice(&self.secs.to_be_bytes());
        b.extend_from_slice(&self.flags.to_be_bytes());
        b.extend_from_slice(&self.ciaddr);
        b.extend_from_slice(&self.yiaddr);
        b.extend_from_slice(&self.siaddr);
        b.extend_from_slice(&self.giaddr);
        b.extend_from_slice(&self.chaddr);
        b.extend_from_slice(&self.chaddr_pad);
        b.extend_from_slice(&self.xid_pad);
        b.extend_from_slice(&self.sname);
        b.extend_from_slice(&self.file);
        b.extend_from_slice(&self.magic_cookie);
        b.extend_from_slice(&self.options);
        b
    }

    /// 从字节流解析（至少 236 字节固定头）。
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::FIXED_LEN { return None; }
        let mut i = 0;
        let op = bytes[i]; i += 1;
        let htype = bytes[i]; i += 1;
        let hlen = bytes[i]; i += 1;
        let hops = bytes[i]; i += 1;
        let xid = u32::from_be_bytes([bytes[i], bytes[i+1], bytes[i+2], bytes[i+3]]); i += 4;
        let secs = u16::from_be_bytes([bytes[i], bytes[i+1]]); i += 2;
        let flags = u16::from_be_bytes([bytes[i], bytes[i+1]]); i += 2;
        let mut ciaddr = [0u8; 4]; ciaddr.copy_from_slice(&bytes[i..i+4]); i += 4;
        let mut yiaddr = [0u8; 4]; yiaddr.copy_from_slice(&bytes[i..i+4]); i += 4;
        let mut siaddr = [0u8; 4]; siaddr.copy_from_slice(&bytes[i..i+4]); i += 4;
        let mut giaddr = [0u8; 4]; giaddr.copy_from_slice(&bytes[i..i+4]); i += 4;
        let mut chaddr = [0u8; 6]; chaddr.copy_from_slice(&bytes[i..i+6]); i += 6;
        let mut chaddr_pad = [0u8; 10]; chaddr_pad.copy_from_slice(&bytes[i..i+10]); i += 10;
        let mut xid_pad = [0u8; 64]; xid_pad.copy_from_slice(&bytes[i..i+64]); i += 64;
        let mut sname = [0u8; 64]; sname.copy_from_slice(&bytes[i..i+64]); i += 64;
        let mut file = [0u8; 128]; file.copy_from_slice(&bytes[i..i+128]); i += 128;
        let mut magic_cookie = [0u8; 4]; magic_cookie.copy_from_slice(&bytes[i..i+4]); i += 4;
        let options = bytes[i..].to_vec();
        Some(Self { op, htype, hlen, hops, xid, secs, flags, ciaddr, yiaddr, siaddr, giaddr, chaddr, chaddr_pad, xid_pad, sname, file, magic_cookie, options })
    }
}

/// 解析 OFFER 报文为 DhcpOffer。
fn parse_offer(msg: &DhcpMessage) -> Option<DhcpOffer> {
    if msg.get_option_u8(DHCP_OPT_MSG_TYPE)? != DHCP_OFFER { return None; }
    Some(DhcpOffer {
        xid: msg.xid,
        server_id: msg.get_option_ip(DHCP_OPT_SERVER_ID).unwrap_or([0, 0, 0, 0]),
        offered_ip: msg.yiaddr,
        subnet_mask: msg.get_option_ip(DHCP_OPT_SUBNET_MASK).unwrap_or([0, 0, 0, 0]),
        gateway: msg.get_option_ip(DHCP_OPT_ROUTER).unwrap_or([0, 0, 0, 0]),
        dns_servers: msg.get_option(DHCP_OPT_DNS_SERVER).map(|v| v.chunks(4).filter_map(|c| if c.len() == 4 { Some([c[0], c[1], c[2], c[3]]) } else { None }).collect()).unwrap_or_default(),
        lease_time: msg.get_option_u32(DHCP_OPT_LEASE_TIME).unwrap_or(0),
    })
}

/// 解析 ACK 报文。
fn parse_ack(msg: &DhcpMessage) -> Result<([u8; 4], [u8; 4], [u8; 4], Vec<[u8; 4]>, u32, [u8; 4]), String> {
    if msg.get_option_u8(DHCP_OPT_MSG_TYPE)? != DHCP_ACK {
        return Err("非 DHCP ACK".to_string());
    }
    let ip = msg.yiaddr;
    let mask = msg.get_option_ip(DHCP_OPT_SUBNET_MASK).unwrap_or([0, 0, 0, 0]);
    let gateway = msg.get_option_ip(DHCP_OPT_ROUTER).unwrap_or([0, 0, 0, 0]);
    let dns = msg.get_option(DHCP_OPT_DNS_SERVER).map(|v| v.chunks(4).filter_map(|c| if c.len() == 4 { Some([c[0], c[1], c[2], c[3]]) } else { None }).collect()).unwrap_or_default();
    let lease_time = msg.get_option_u32(DHCP_OPT_LEASE_TIME).unwrap_or(86400);
    let server_id = msg.get_option_ip(DHCP_OPT_SERVER_ID).unwrap_or(msg.siaddr);
    Ok((ip, mask, gateway, dns, lease_time, server_id))
}

// =======================================================================
// sense/act 接口（net://dhcp）
// ======================================================================

/// 查询 DHCP 状态快照（供 `sense net://dhcp`）。
pub fn dhcp_sense_snapshot() -> String {
    let c = DhcpClient::client().lock().unwrap();
    c.status()
}

/// 对 DHCP 通道执行行动（供 `act net://dhcp`）。负载：`stats` / `discover` / `request` / `renew` / `release` / `bound` / `lease`。
pub fn dhcp_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(dhcp_sense_snapshot()),
        Some("discover") => {
            let mut c = DhcpClient::client().lock().unwrap();
            let msg = c.discover();
            Ok(format!("dhcp discover sent xid={} yiaddr={}.{}.{}.{} options={}B", msg.xid, msg.yiaddr[0], msg.yiaddr[1], msg.yiaddr[2], msg.yiaddr[3], msg.options.len()))
        }
        Some("request") => {
            let mut c = DhcpClient::client().lock().unwrap();
            // 取第一个 offer；无 offer 则返回错误。
            if let Some(offer) = c.offers.front().cloned() {
                let msg = c.request(&offer);
                Ok(format!("dhcp request sent xid={} req_ip={}.{}.{}.{}", msg.xid, msg.ciaddr[0], msg.ciaddr[1], msg.ciaddr[2], msg.ciaddr[3]))
            } else {
                Err("无可用 DHCP OFFER，先执行 discover".to_string())
            }
        }
        Some("renew") => {
            let mut c = DhcpClient::client().lock().unwrap();
            let msg = c.renew();
            Ok(format!("dhcp renew sent xid={} ciaddr={}.{}.{}.{}", msg.xid, msg.ciaddr[0], msg.ciaddr[1], msg.ciaddr[2], msg.ciaddr[3]))
        }
        Some("release") => {
            let mut c = DhcpClient::client().lock().unwrap();
            let msg = c.release();
            Ok(format!("dhcp release sent xid={}", msg.xid))
        }
        Some("bound") => {
            let c = DhcpClient::client().lock().unwrap();
            Ok(format!("bound={} state={}", c.bound(), c.state_name()))
        }
        Some("lease") => {
            let c = DhcpClient::client().lock().unwrap();
            Ok(c.lease.summary())
        }
        _ => Err("用法: act net://dhcp stats | discover | request | renew | release | bound | lease".to_string()),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_discover_request_bound() {
        let mut c = DhcpClient::new("eth0", [0x02, 0, 0, 0, 0, 0x01]);
        assert_eq!(c.state, DhcpState::Init);
        assert!(!c.bound());
        // discover。
        let disc = c.discover();
        assert_eq!(disc.op, 1);
        assert_eq!(c.get_msg_type(&disc), Some(DHCP_DISCOVER));
        assert_eq!(c.state, DhcpState::Selecting);
        // 收到 offer。
        let offer_ip = [10, 0, 0, 50];
        let offer = DhcpOffer {
            xid: disc.xid, server_id: [10, 0, 0, 1], offered_ip: offer_ip,
            subnet_mask: [255, 255, 255, 0], gateway: [10, 0, 0, 1],
            dns_servers: vec![[8, 8, 8, 8]], lease_time: 86400,
        };
        c.handle_offer(&build_offer_msg(&offer));
        assert_eq!(c.offers.len(), 1);
        // request。
        let req = c.request(&offer);
        assert_eq!(c.get_msg_type(&req), Some(DHCP_REQUEST));
        assert_eq!(c.state, DhcpState::Requesting);
        // ack。
        let ack = build_ack_msg(disc.xid, offer_ip, [255, 255, 255, 0], [10, 0, 0, 1], vec![[8, 8, 8, 8]], 86400);
        let lease = c.handle_ack(&ack, 1000).unwrap();
        assert!(c.bound());
        assert_eq!(lease.ip, offer_ip);
        assert_eq!(lease.remaining_time, 86400);
    }

    #[test]
    fn tick_lease_renewing_rebinding() {
        let mut c = DhcpClient::new("eth0", [0x02, 0, 0, 0, 0, 0x01]);
        c.bind_lease([10, 0, 0, 50], [255, 255, 255, 0], [10, 0, 0, 1], vec![[8, 8, 8, 8]], 100, 1000);
        assert_eq!(c.state, DhcpState::Bound);
        assert_eq!(c.lease.remaining_time, 100);
        // 流逝 40 秒（<= 一半 50 -> Renewing）。
        c.tick_lease(40);
        assert_eq!(c.lease.remaining_time, 60);
        assert_eq!(c.state, DhcpState::Renewing);
        // 流逝 60 秒 -> 剩 0 -> Rebinding。
        c.tick_lease(60);
        assert_eq!(c.lease.remaining_time, 0);
        assert_eq!(c.state, DhcpState::Rebinding);
        // 再流逝 -> Init，IP 清零。
        c.tick_lease(1);
        assert_eq!(c.state, DhcpState::Init);
        assert_eq!(c.lease.ip, [0, 0, 0, 0]);
    }

    #[test]
    fn release_clears_lease() {
        let mut c = DhcpClient::new("eth0", [0x02, 0, 0, 0, 0, 0x01]);
        c.bind_lease([10, 0, 0, 50], [255, 255, 255, 0], [10, 0, 0, 1], vec![], 100, 1000);
        assert!(c.bound());
        let _msg = c.release();
        assert_eq!(c.state, DhcpState::Released);
        assert!(!c.bound());
        assert_eq!(c.lease.ip, [0, 0, 0, 0]);
    }

    #[test]
    fn nak_resets_to_init() {
        let mut c = DhcpClient::new("eth0", [0x02, 0, 0, 0, 0, 0x01]);
        c.bind_lease([10, 0, 0, 50], [255, 255, 255, 0], [10, 0, 0, 1], vec![], 100, 1000);
        c.handle_nak();
        assert_eq!(c.state, DhcpState::Init);
        assert!(!c.bound());
    }

    #[test]
    fn dhcp_message_serialize_parse() {
        let mut msg = DhcpMessage {
            op: 1, htype: 1, hlen: 6, hops: 0,
            xid: 0x12345678, secs: 0, flags: 0x8000,
            ciaddr: [0,0,0,0], yiaddr: [0,0,0,0], siaddr: [0,0,0,0], giaddr: [0,0,0,0],
            chaddr: [0x02,0,0,0,0,0x01], chaddr_pad: [0;10], xid_pad: [0;64],
            sname: [0;64], file: [0;128],
            magic_cookie: [0x63,0x82,0x53,0x63],
            options: Vec::new(),
        };
        msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_DISCOVER);
        msg.add_option_ip(DHCP_OPT_SUBNET_MASK, [255,255,255,0]);
        msg.add_option_end();
        let bytes = DhcpMessage::serialize(&msg);
        assert!(bytes.len() >= DhcpMessage::FIXED_LEN);
        let parsed = DhcpMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.xid, 0x12345678);
        assert_eq!(parsed.get_option_u8(DHCP_OPT_MSG_TYPE), Some(DHCP_DISCOVER));
        assert_eq!(parsed.get_option_ip(DHCP_OPT_SUBNET_MASK), Some([255,255,255,0]));
        assert_eq!(parsed.magic_cookie, [0x63,0x82,0x53,0x63]);
    }

    #[test]
    fn lease_summary_and_is_expired() {
        let lease = DhcpLease {
            ip: [10,0,0,50], subnet_mask: [255,255,255,0], gateway: [10,0,0,1],
            dns_servers: vec![[8,8,8,8]], lease_time: 3600, remaining_time: 0,
            server_id: [10,0,0,1], renewal_time: 1800, obtained_at_secs: 1000,
        };
        assert!(lease.is_expired());
        assert!(lease.is_bound());
        let s = lease.summary();
        assert!(s.contains("10.0.0.50"));
        assert!(s.contains("8.8.8.8"));
        let mut lease2 = lease.clone();
        lease2.remaining_time = 100;
        assert!(!lease2.is_expired());
    }

    #[test]
    fn get_msg_type_helper() {
        let mut c = DhcpClient::new("eth0", [0x02,0,0,0,0,0x01]);
        let msg = c.discover();
        assert_eq!(c.get_msg_type(&msg), Some(DHCP_DISCOVER));
    }

    #[test]
    fn sense_and_act() {
        let snap = dhcp_sense_snapshot();
        assert!(snap.contains("state="));
        assert!(dhcp_act_raw("stats").is_ok());
        assert!(dhcp_act_raw("bound").is_ok());
        assert!(dhcp_act_raw("lease").is_ok());
        assert!(dhcp_act_raw("discover").is_ok());
        assert!(dhcp_act_raw("request").is_err()); // 无 offer
    }
}

// 辅助：在不引入 borrow 检查问题下，从 DhcpClient 读取消息类型。
impl DhcpClient {
    fn get_msg_type(&self, msg: &DhcpMessage) -> Option<u8> {
        msg.get_option_u8(DHCP_OPT_MSG_TYPE)
    }
}

/// 构造一个 DHCP OFFER 报文（测试用）。
fn build_offer_msg(offer: &DhcpOffer) -> DhcpMessage {
    let mut msg = DhcpMessage {
        op: 2, htype: 1, hlen: 6, hops: 0,
        xid: offer.xid, secs: 0, flags: 0x8000,
        ciaddr: [0,0,0,0], yiaddr: offer.offered_ip, siaddr: offer.server_id, giaddr: [0,0,0,0],
        chaddr: [0x02,0,0,0,0,0x01], chaddr_pad: [0;10], xid_pad: [0;64],
        sname: [0;64], file: [0;128],
        magic_cookie: [0x63,0x82,0x53,0x63],
        options: Vec::new(),
    };
    msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_OFFER);
    msg.add_option_ip(DHCP_OPT_SUBNET_MASK, offer.subnet_mask);
    msg.add_option_ip(DHCP_OPT_ROUTER, offer.gateway);
    msg.add_option_ip(DHCP_OPT_DNS_SERVER, offer.dns_servers[0]);
    msg.add_option_u32_opt(DHCP_OPT_LEASE_TIME, offer.lease_time);
    msg.add_option_ip(DHCP_OPT_SERVER_ID, offer.server_id);
    msg.add_option_end();
    msg
}

/// 构造一个 DHCP ACK 报文（测试用）。
fn build_ack_msg(xid: u32, ip: [u8;4], mask: [u8;4], gw: [u8;4], dns: Vec<[u8;4]>, lease_time: u32) -> DhcpMessage {
    let mut msg = DhcpMessage {
        op: 2, htype: 1, hlen: 6, hops: 0,
        xid, secs: 0, flags: 0,
        ciaddr: [0,0,0,0], yiaddr: ip, siaddr: gw, giaddr: [0,0,0,0],
        chaddr: [0x02,0,0,0,0,0x01], chaddr_pad: [0;10], xid_pad: [0;64],
        sname: [0;64], file: [0;128],
        magic_cookie: [0x63,0x82,0x53,0x63],
        options: Vec::new(),
    };
    msg.add_option_u8(DHCP_OPT_MSG_TYPE, DHCP_ACK);
    msg.add_option_ip(DHCP_OPT_SUBNET_MASK, mask);
    msg.add_option_ip(DHCP_OPT_ROUTER, gw);
    for d in &dns { msg.add_option_ip(DHCP_OPT_DNS_SERVER, *d); }
    msg.add_option_u32_opt(DHCP_OPT_LEASE_TIME, lease_time);
    msg.add_option_ip(DHCP_OPT_SERVER_ID, gw);
    msg.add_option_end();
    msg
}

/// 添加 u32 选项（测试用辅助）。
trait DhcpMsgOptU32 {
    fn add_option_u32_opt(&mut self, code: u8, value: u32);
}
impl DhcpMsgOptU32 for DhcpMessage {
    fn add_option_u32_opt(&mut self, code: u8, value: u32) {
        self.add_option_bytes(code, &value.to_be_bytes());
    }
}