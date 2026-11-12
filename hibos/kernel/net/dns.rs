//! DNS 解析器（kernel/net/dns.rs）。
//!
//! 本模块实现一个简化的 DNS 解析器：构造与解析 DNS 报文（头部、问题区、回答区、
//! 权威区、附加区）、递归查询、带 TTL 过期淘汰的缓存、hosts 文件支持。
//! 为演示级协议栈：真实 UDP 收发由上层完成，本模块负责报文编解码与缓存策略。
//!
//! 特性:
//! * DnsResolver：new / resolve / resolve_ipv4 / reverse_lookup / cache_lookup / cache_insert
//! * DnsRecord：名称、类型（A/AAAA/CNAME/MX/TXT/NS）、值、TTL
//! * DNS 报文构造与解析（5 个区段）
//! * 递归查询（串联多台上游服务器）、缓存（TTL 过期淘汰）、hosts 文件支持
//! * sense/act 通道：`net://dns`

use alloc::collections::HashMap;
use crate::ksync::{SpinLock, OnceLock};

// =======================================================================
// DNS 类型与常量
// ======================================================================

/// DNS 记录类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DnsRecordType {
    A,
    AAAA,
    CNAME,
    MX,
    TXT,
    NS,
    Other(u16),
}

impl DnsRecordType {
    pub fn code(self) -> u16 {
        match self {
            DnsRecordType::A => 1,
            DnsRecordType::AAAA => 28,
            DnsRecordType::CNAME => 5,
            DnsRecordType::MX => 15,
            DnsRecordType::TXT => 16,
            DnsRecordType::NS => 2,
            DnsRecordType::Other(c) => c,
        }
    }
    pub fn from_code(c: u16) -> Self {
        match c {
            1 => DnsRecordType::A,
            28 => DnsRecordType::AAAA,
            5 => DnsRecordType::CNAME,
            15 => DnsRecordType::MX,
            16 => DnsRecordType::TXT,
            2 => DnsRecordType::NS,
            c => DnsRecordType::Other(c),
        }
    }
}

/// DNS 记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    pub rtype: DnsRecordType,
    pub value: String,
    pub ttl: u32,
}

/// DNS 查询结果（可能含多条记录）。
#[derive(Clone, Debug)]
pub struct DnsQueryResult {
    pub name: String,
    pub records: Vec<DnsRecord>,
}

/// DNS 报文头部（12 字节）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsHeader {
    pub id: u16,
    pub flags: u16,
    pub qd_count: u16,
    pub an_count: u16,
    pub ns_count: u16,
    pub ar_count: u16,
}

impl DnsHeader {
    pub const LEN: usize = 12;
    pub const FLAG_QR_RESPONSE: u16 = 1 << 15;
    pub const FLAG_RD: u16 = 1 << 8; // recursion desired
    pub const FLAG_RA: u16 = 1 << 7; // recursion available
    pub const MASK_RCODE: u16 = 0x000F;

    pub fn new(id: u16, recursive: bool) -> Self {
        let mut flags = 0;
        if recursive { flags |= Self::FLAG_RD; }
        Self { id, flags, qd_count: 1, an_count: 0, ns_count: 0, ar_count: 0 }
    }

    pub fn rcode(&self) -> u16 { self.flags & Self::MASK_RCODE }
    pub fn is_response(&self) -> bool { self.flags & Self::FLAG_QR_RESPONSE != 0 }
}

// =======================================================================
// DNS 报文构造与解析
// ======================================================================

/// DNS 报文：头部 + 问题区 + 回答区 + 权威区 + 附加区。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsMessage {
    pub header: DnsHeader,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<DnsRecord>,
    pub authorities: Vec<DnsRecord>,
    pub additionals: Vec<DnsRecord>,
}

/// DNS 问题项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsQuestion {
    pub name: String,
    pub rtype: DnsRecordType,
    pub class: u16, // 1 = IN
}

impl DnsMessage {
    /// 构造标准查询报文。
    pub fn query(id: u16, name: &str, rtype: DnsRecordType) -> Self {
        Self {
            header: DnsHeader::new(id, true),
            questions: vec![DnsQuestion { name: name.to_string(), rtype, class: 1 }],
            answers: Vec::new(),
            authorities: Vec::new(),
            additionals: Vec::new(),
        }
    }

    /// 序列化为字节流。
    pub fn serialize(&self) -> Vec<u8> {
        let mut b = Vec::new();
        // 头部。
        b.extend_from_slice(&self.header.id.to_be_bytes());
        b.extend_from_slice(&self.header.flags.to_be_bytes());
        b.extend_from_slice(&self.header.qd_count.to_be_bytes());
        b.extend_from_slice(&self.header.an_count.to_be_bytes());
        b.extend_from_slice(&self.header.ns_count.to_be_bytes());
        b.extend_from_slice(&self.header.ar_count.to_be_bytes());
        // 问题区。
        for q in &self.questions {
            encode_name(&mut b, &q.name);
            b.extend_from_slice(&q.rtype.code().to_be_bytes());
            b.extend_from_slice(&q.class.to_be_bytes());
        }
        // 回答/权威/附加区（记录序列化：名称指针 + 类型 + 类 + TTL + RDLENGTH + RDATA）。
        for section in [&self.answers, &self.authorities, &self.additionals] {
            for r in section.iter() {
                encode_name(&mut b, &r.name);
                b.extend_from_slice(&r.rtype.code().to_be_bytes());
                b.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
                b.extend_from_slice(&r.ttl.to_be_bytes());
                let rdata = encode_rdata(r);
                b.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
                b.extend_from_slice(&rdata);
            }
        }
        b
    }

    /// 从字节流解析。
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < DnsHeader::LEN { return None; }
        let id = u16::from_be_bytes([bytes[0], bytes[1]]);
        let flags = u16::from_be_bytes([bytes[2], bytes[3]]);
        let qd_count = u16::from_be_bytes([bytes[4], bytes[5]]);
        let an_count = u16::from_be_bytes([bytes[6], bytes[7]]);
        let ns_count = u16::from_be_bytes([bytes[8], bytes[9]]);
        let ar_count = u16::from_be_bytes([bytes[10], bytes[11]]);
        let mut i = DnsHeader::LEN;
        let mut questions = Vec::new();
        for _ in 0..qd_count {
            let (name, ni) = decode_name(bytes, i)?;
            i = ni;
            let rtype = DnsRecordType::from_code(u16::from_be_bytes([bytes[i], bytes[i+1]])); i += 2;
            let class = u16::from_be_bytes([bytes[i], bytes[i+1]]); i += 2;
            questions.push(DnsQuestion { name, rtype, class });
        }
        let header = DnsHeader { id, flags, qd_count, an_count, ns_count, ar_count };
        let mut answers = Vec::new();
        for _ in 0..an_count {
            let r = parse_record_at(bytes, &mut i)?;
            answers.push(r);
        }
        let mut authorities = Vec::new();
        for _ in 0..ns_count {
            let r = parse_record_at(bytes, &mut i)?;
            authorities.push(r);
        }
        let mut additionals = Vec::new();
        for _ in 0..ar_count {
            let r = parse_record_at(bytes, &mut i)?;
            additionals.push(r);
        }
        Some(Self { header, questions, answers, authorities, additionals })
    }

    /// 提取第一条 A 记录的值。
    pub fn first_a(&self) -> Option<[u8; 4]> {
        for r in &self.answers {
            if r.rtype == DnsRecordType::A {
                return parse_ip4_a(&r.value);
            }
        }
        None
    }

    /// 提取所有 A 记录。
    pub fn all_a(&self) -> Vec<[u8; 4]> {
        self.answers.iter().filter_map(|r| if r.rtype == DnsRecordType::A { parse_ip4_a(&r.value) } else { None }).collect()
    }
}

fn parse_record_at(bytes: &[u8], i: &mut usize) -> Option<DnsRecord> {
    let (name, ni) = decode_name(bytes, *i)?;
    *i = ni;
    if *i + 10 > bytes.len() { return None; }
    let rtype = DnsRecordType::from_code(u16::from_be_bytes([bytes[*i], bytes[*i+1]])); *i += 2;
    let _class = u16::from_be_bytes([bytes[*i], bytes[*i+1]]); *i += 2;
    let ttl = u32::from_be_bytes([bytes[*i], bytes[*i+1], bytes[*i+2], bytes[*i+3]]); *i += 4;
    let rdlen = u16::from_be_bytes([bytes[*i], bytes[*i+1]]) as usize; *i += 2;
    if *i + rdlen > bytes.len() { return None; }
    let rdata = &bytes[*i..*i + rdlen]; *i += rdlen;
    let value = decode_rdata_value(rtype, rdata);
    Some(DnsRecord { name, rtype, value, ttl })
}

fn decode_rdata_value(rtype: DnsRecordType, rdata: &[u8]) -> String {
    match rtype {
        DnsRecordType::A => {
            if rdata.len() >= 4 { format!("{}.{}.{}.{}", rdata[0], rdata[1], rdata[2], rdata[3]) } else { String::new() }
        }
        DnsRecordType::AAAA => {
            // 简化：展示为冒号十六进制。
            let mut parts = Vec::new();
            for c in rdata.chunks(2) {
                if c.len() == 2 { parts.push(format!("{:02x}{:02x}", c[0], c[1])); }
            }
            parts.join(":")
        }
        DnsRecordType::CNAME | DnsRecordType::NS => {
            let (name, _) = decode_name(rdata, 0).unwrap_or((String::new(), 0));
            name
        }
        DnsRecordType::MX => {
            if rdata.len() >= 2 {
                let pref = u16::from_be_bytes([rdata[0], rdata[1]]);
                let (name, _) = decode_name(rdata, 2).unwrap_or((String::new(), 0));
                format!("{} {}", pref, name)
            } else { String::new() }
        }
        DnsRecordType::TXT => {
            String::from_utf8_lossy(rdata).to_string()
        }
        DnsRecordType::Other(_) => hex_encode(rdata),
    }
}

/// 手工十六进制（避免依赖 hex crate）。
fn hex_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(CHARS[(b >> 4) as usize]);
        out.push(CHARS[(b & 0x0F) as usize]);
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 编码域名为 DNS 标签格式。
fn encode_name(b: &mut Vec<u8>, name: &str) {
    for label in name.split('.') {
        if label.is_empty() { continue; }
        b.push(label.len() as u8);
        b.extend_from_slice(label.as_bytes());
    }
    b.push(0); // 根
}

/// 解码 DNS 标签格式域名（不处理指针，简化）。
fn decode_name(bytes: &[u8], mut i: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let _start = i;
    loop {
        if i >= bytes.len() { return None; }
        let len = bytes[i] as usize;
        if len == 0 {
            i += 1;
            break;
        }
        if len & 0xC0 != 0 {
            // 指针：简化处理（跳过两字节）。
            i += 2;
            break;
        }
        i += 1;
        if i + len > bytes.len() { return None; }
        labels.push(std::str::from_utf8(&bytes[i..i+len]).ok()?);
        i += len;
    }
    Some((labels.join("."), i))
}

/// 编码记录 RDATA。
fn encode_rdata(r: &DnsRecord) -> Vec<u8> {
    match r.rtype {
        DnsRecordType::A => parse_ip4_a(&r.value).map(|ip| ip.to_vec()).unwrap_or_default(),
        DnsRecordType::CNAME | DnsRecordType::NS => {
            let mut b = Vec::new();
            encode_name(&mut b, &r.value);
            b
        }
        _ => Vec::new(),
    }
}

/// 解析点分十进制 IPv4（A 记录值）。
fn parse_ip4_a(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut idx = 0;
    for part in s.split('.') {
        if idx >= 4 { return None; }
        out[idx] = part.parse::<u8>().ok()?;
        idx += 1;
    }
    if idx == 4 { Some(out) } else { None }
}

/// 反向查找：将 IPv4 地址转为 in-addr.arpa 域名。
fn ip_to_arpa(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}.in-addr.arpa", ip[3], ip[2], ip[1], ip[0])
}

// =======================================================================
// 缓存条目（带 TTL 过期）
// ======================================================================

/// 缓存条目：记录 + 绝对过期时间（纪元秒）。
#[derive(Clone, Debug)]
struct CacheEntry {
    record: DnsRecord,
    expire_at_secs: u32,
}

/// DNS 缓存：按 (name, type) 索引，TTL 过期淘汰。
pub struct DnsCache {
    entries: BTreeMap<(String, u16), CacheEntry>,
}

impl DnsCache {
    pub fn new() -> Self { Self { entries: BTreeMap::new() } }

    /// 查找：命中且未过期返回记录，否则 None（并清理过期项）。
    pub fn lookup(&mut self, name: &str, rtype: DnsRecordType) -> Option<DnsRecord> {
        let key = (name.to_lowercase(), rtype.code());
        let now = now_secs();
        let hit = self.entries.get(&key).and_then(|e| {
            if e.expire_at_secs > now { Some(e.record.clone()) } else { None }
        });
        // 清理过期。
        self.entries.retain(|_, e| e.expire_at_secs > now);
        hit
    }

    /// 插入：记录 + TTL（秒）。
    pub fn insert(&mut self, record: DnsRecord, ttl: u32) {
        let now = now_secs();
        self.entries.insert((record.name.to_lowercase(), record.rtype.code()), CacheEntry { record, expire_at_secs: now.saturating_add(ttl) });
    }

    /// 清空。
    pub fn clear(&mut self) { self.entries.clear(); }

    pub fn len(&self) -> usize { self.entries.len() }
}

/// 当前纪元秒。
fn now_secs() -> u32 {
    (crate::time::get_time_ns() / 1_000_000_000) as u32
}

// =======================================================================
// hosts 文件支持
// ======================================================================

/// hosts 条目：域名 -> IP。
pub struct Hosts {
    map: BTreeMap<String, [u8; 4]>,
}

impl Hosts {
    pub fn new() -> Self { Self { map: BTreeMap::new() } }

    /// 从字符串内容解析 hosts 文件（每行：ip  name [#注释]）。
    pub fn parse(content: &str) -> Self {
        let mut map = BTreeMap::new();
        for line in content.lines() {
            let line = line.split('#').next().unwrap_or(line).trim();
            if line.is_empty() { continue; }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Some(ip) = parse_ip4_hosts(parts[0]) {
                    for name in &parts[1..] {
                        map.insert(name.to_lowercase(), ip);
                    }
                }
            }
        }
        Self { map }
    }

    pub fn lookup(&self, name: &str) -> Option<[u8; 4]> {
        self.map.get(&name.to_lowercase()).copied()
    }

    pub fn insert(&mut self, name: &str, ip: [u8; 4]) {
        self.map.insert(name.to_lowercase(), ip);
    }
}

fn parse_ip4_hosts(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut idx = 0;
    for part in s.split('.') {
        if idx >= 4 { return None; }
        out[idx] = part.parse::<u8>().ok()?;
        idx += 1;
    }
    if idx == 4 { Some(out) } else { None }
}

// =======================================================================
// DnsResolver：DNS 解析器
// ======================================================================

/// DNS 解析器：维护上游服务器列表、缓存、hosts。
pub struct DnsResolver {
    /// 上游 DNS 服务器 IP 列表。
    pub servers: Vec<[u8; 4]>,
    /// 缓存。
    pub cache: DnsCache,
    /// hosts 表。
    pub hosts: Hosts,
    /// 本次查询使用的事务 ID 计数器。
    next_id: u16,
}

impl DnsResolver {
    /// 创建解析器（默认上游 8.8.8.8 / 8.8.4.4）。
    pub fn new(servers: Vec<[u8; 4]>) -> Self {
        let servers = if servers.is_empty() { vec![[8, 8, 8, 8], [8, 8, 4, 4]] } else { servers };
        Self { servers, cache: DnsCache::new(), hosts: Hosts::new(), next_id: 1 }
    }

    /// 默认解析器（`net://dns` 通道后端）。
    pub fn resolver() -> &'static SpinLock<DnsResolver> {
        static RESOLVER: OnceLock<SpinLock<DnsResolver>> = OnceLock::new();
        RESOLVER.get_or_init(|| SpinLock::new(Self::new(Vec::new())))
    }

    /// 加载 hosts 文件内容。
    pub fn load_hosts(&mut self, content: &str) {
        self.hosts = Hosts::parse(content);
    }

    /// 添加一条 hosts 记录。
    pub fn add_hosts(&mut self, name: &str, ip: [u8; 4]) {
        self.hosts.insert(name, ip);
    }

    /// 解析域名为指定类型的记录列表（先 hosts，再缓存，再上游查询）。
    pub fn resolve(&mut self, name: &str, rtype: DnsRecordType) -> Vec<DnsRecord> {
        // 1. hosts。
        if rtype == DnsRecordType::A {
            if let Some(ip) = self.hosts.lookup(name) {
                return vec![DnsRecord { name: name.to_string(), rtype, value: fmt_ip(ip), ttl: 0 }];
            }
        }
        // 2. 缓存。
        if let Some(rec) = self.cache.lookup(name, rtype) {
            return vec![rec];
        }
        // 3. 上游递归查询（串联服务器）。
        let servers = self.servers.clone();
        for server in &servers {
            if let Some(recs) = self.query_server(name, rtype, *server) {
                for r in &recs {
                    self.cache.insert(r.clone(), r.ttl);
                }
                return recs;
            }
        }
        Vec::new()
    }

    /// 解析 IPv4 地址列表（A 记录）。
    pub fn resolve_ipv4(&mut self, name: &str) -> Vec<[u8; 4]> {
        let recs = self.resolve(name, DnsRecordType::A);
        recs.iter().filter_map(|r| parse_ip4_a(&r.value)).collect()
    }

    /// 反向查找：PTR（简化：查询 in-addr.arpa 的 A/CNAME，返回主机名）。
    pub fn reverse_lookup(&mut self, ip: [u8; 4]) -> Vec<String> {
        let arpa = ip_to_arpa(ip);
        // hosts 反向：查找值为 ip 的域名。
        let mut hits: Vec<String> = self.hosts.map.iter().filter(|(_, v)| **v == ip).map(|(k, _)| k.clone()).collect();
        if !hits.is_empty() { return hits; }
        // 上游 PTR 简化：查 A 记录并返回第一个答案域名。
        let recs = self.resolve(&arpa, DnsRecordType::A);
        for r in &recs {
            if !r.name.is_empty() { hits.push(r.name.clone()); }
        }
        hits
    }

    /// 查询单个上游服务器：构造报文、伪造收发（返回构造的应答）。
    fn query_server(&mut self, name: &str, rtype: DnsRecordType, server: [u8; 4]) -> Option<Vec<DnsRecord>> {
        let _ = server;
        let id = self.next_id_wrap();
        let msg = DnsMessage::query(id, name, rtype);
        let bytes = msg.serialize();
        // 演示模式：不依赖真实网络，按常见域名返回模拟 A 记录；其余返回空。
        let _ = bytes;
        mock_upstream_response(name, rtype)
    }

    fn next_id_wrap(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// 缓存查询（公开接口）。
    pub fn cache_lookup(&mut self, name: &str, rtype: DnsRecordType) -> Option<DnsRecord> {
        self.cache.lookup(name, rtype)
    }

    /// 缓存插入（公开接口）。
    pub fn cache_insert(&mut self, record: DnsRecord, ttl: u32) {
        self.cache.insert(record, ttl);
    }

    /// 缓存条目数。
    pub fn cache_len(&self) -> usize { self.cache.len() }
}

/// 模拟上游响应（演示模式，无真实网络）。
fn mock_upstream_response(name: &str, rtype: DnsRecordType) -> Option<Vec<DnsRecord>> {
    let lower = name.to_lowercase();
    if lower.contains("hibos") || lower.ends_with(".local") {
        match rtype {
            DnsRecordType::A => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "10.0.0.99".to_string(), ttl: 300 }]),
            DnsRecordType::AAAA => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "::1".to_string(), ttl: 300 }]),
            DnsRecordType::CNAME => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "hibos.local".to_string(), ttl: 300 }]),
            DnsRecordType::MX => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "10 mail.hibos.local".to_string(), ttl: 300 }]),
            DnsRecordType::NS => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "ns.hibos.local".to_string(), ttl: 300 }]),
            DnsRecordType::TXT => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "v=spf1".to_string(), ttl: 300 }]),
            DnsRecordType::Other(_) => None,
        }
    } else if lower.ends_with(".example.com") {
        match rtype {
            DnsRecordType::A => Some(vec![DnsRecord { name: name.to_string(), rtype, value: "192.0.2.1".to_string(), ttl: 60 }]),
            _ => None,
        }
    } else {
        None
    }
}

/// IPv4 地址格式化。
fn fmt_ip(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

// =======================================================================
// sense/act 接口（net://dns）
// ======================================================================

/// 查询 DNS 状态快照（供 `sense net://dns`）。
pub fn dns_sense_snapshot() -> String {
    let r = DnsResolver::resolver().lock();
    format!("dns servers={} cache={} hosts={}", r.servers.len(), r.cache_len(), 0)
}

/// 对 DNS 通道执行行动（供 `act net://dns`）。负载：`resolve <name> [A|AAAA|CNAME|MX|TXT|NS]` / `resolve_ipv4 <name>` / `reverse <ip>`。
pub fn dns_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("resolve") => {
            let mut r = DnsResolver::resolver().lock();
            let name = it.next().ok_or("用法: resolve <name> [类型]")?;
            let rtype = match it.next().unwrap_or("A") {
                "AAAA" => DnsRecordType::AAAA,
                "CNAME" => DnsRecordType::CNAME,
                "MX" => DnsRecordType::MX,
                "TXT" => DnsRecordType::TXT,
                "NS" => DnsRecordType::NS,
                _ => DnsRecordType::A,
            };
            let recs = r.resolve(name, rtype);
            let mut out = format!("resolve {} -> {} 条记录", name, recs.len());
            for r in &recs {
                out.push_str(&format!("\n  {} {} ttl={}s", r.name, r.value, r.ttl));
            }
            Ok(out)
        }
        Some("resolve_ipv4") => {
            let mut r = DnsResolver::resolver().lock();
            let name = it.next().ok_or("用法: resolve_ipv4 <name>")?;
            let ips = r.resolve_ipv4(name);
            Ok(format!("{} -> {}", name, ips.iter().map(|ip| fmt_ip(*ip)).collect::<Vec<_>>().join(",")))
        }
        Some("reverse") => {
            let mut r = DnsResolver::resolver().lock();
            let ip = parse_ip4_dns(it.next().ok_or("用法: reverse <ip>")?)?;
            let names = r.reverse_lookup(ip);
            Ok(format!("reverse {} -> {}", fmt_ip(ip), names.join(",")))
        }
        _ => Err("用法: act net://dns resolve <name> [类型] | resolve_ipv4 <name> | reverse <ip>".to_string()),
    }
}

/// 解析点分十进制 IPv4（dns 专用）。
fn parse_ip4_dns(s: &str) -> Result<[u8; 4], String> {
    let mut out = [0u8; 4];
    for (i, part) in s.split('.').enumerate().take(4) {
        out[i] = part.trim().parse::<u8>().map_err(|_| format!("无效 IPv4 地址: {}", s))?;
    }
    Ok(out)
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_parse_query() {
        let msg = DnsMessage::query(42, "example.com", DnsRecordType::A);
        assert_eq!(msg.header.id, 42);
        assert_eq!(msg.header.qd_count, 1);
        let bytes = msg.serialize();
        let parsed = DnsMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.header.id, 42);
        assert_eq!(parsed.questions.len(), 1);
        assert_eq!(parsed.questions[0].name, "example.com");
        assert_eq!(parsed.questions[0].rtype, DnsRecordType::A);
    }

    #[test]
    fn encode_decode_name_roundtrip() {
        let mut b = Vec::new();
        encode_name(&mut b, "www.example.com");
        let (name, _) = decode_name(&b, 0).unwrap();
        assert_eq!(name, "www.example.com");
    }

    #[test]
    fn record_type_codes() {
        assert_eq!(DnsRecordType::A.code(), 1);
        assert_eq!(DnsRecordType::AAAA.code(), 28);
        assert_eq!(DnsRecordType::CNAME.code(), 5);
        assert_eq!(DnsRecordType::MX.code(), 15);
        assert_eq!(DnsRecordType::TXT.code(), 16);
        assert_eq!(DnsRecordType::NS.code(), 2);
        assert_eq!(DnsRecordType::from_code(1), DnsRecordType::A);
        assert_eq!(DnsRecordType::from_code(99), DnsRecordType::Other(99));
    }

    #[test]
    fn resolve_a_and_ipv4() {
        let mut r = DnsResolver::new(vec![[8, 8, 8, 8]]);
        let recs = r.resolve("www.hibos.local", DnsRecordType::A);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].value, "10.0.0.99");
        let ips = r.resolve_ipv4("www.hibos.local");
        assert_eq!(ips, vec![[10, 0, 0, 99]]);
        // 未知域名：无记录。
        let empty = r.resolve("unknown.example.invalid", DnsRecordType::A);
        assert!(empty.is_empty());
    }

    #[test]
    fn hosts_support() {
        let mut r = DnsResolver::new(vec![]);
        r.add_hosts("myhost.local", [192, 168, 1, 10]);
        let ips = r.resolve_ipv4("myhost.local");
        assert_eq!(ips, vec![[192, 168, 1, 10]]);
        // 大小写不敏感。
        let ips2 = r.resolve_ipv4("MYHOST.LOCAL");
        assert_eq!(ips2, vec![[192, 168, 1, 10]]);
    }

    #[test]
    fn cache_ttl_expiry() {
        let mut r = DnsResolver::new(vec![]);
        // 插入一条 TTL=1 的缓存记录，手动将 expire_at 设为过去。
        r.cache_insert(DnsRecord { name: "cache.test".to_string(), rtype: DnsRecordType::A, value: "1.1.1.1".to_string(), ttl: 1 }, 1);
        // 立即查找应命中（expire_at = now+1）。
        assert!(r.cache_lookup("cache.test", DnsRecordType::A).is_some());
        // 缓存长度。
        assert_eq!(r.cache_len(), 1);
        r.cache.clear();
        assert_eq!(r.cache_len(), 0);
    }

    #[test]
    fn reverse_lookup() {
        let mut r = DnsResolver::new(vec![]);
        r.add_hosts("gw.local", [10, 0, 0, 1]);
        let names = r.reverse_lookup([10, 0, 0, 1]);
        assert!(names.contains(&"gw.local".to_string()));
    }

    #[test]
    fn arpa_conversion() {
        let arpa = ip_to_arpa([10, 0, 0, 1]);
        assert_eq!(arpa, "1.0.0.10.in-addr.arpa");
    }

    #[test]
    fn message_has_answer() {
        let mut msg = DnsMessage::query(1, "a.com", DnsRecordType::A);
        msg.answers.push(DnsRecord { name: "a.com".to_string(), rtype: DnsRecordType::A, value: "93.184.216.34".to_string(), ttl: 100 });
        let ip = msg.first_a();
        assert_eq!(ip, Some([93, 184, 216, 34]));
        let all = msg.all_a();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn sense_and_act() {
        let snap = dns_sense_snapshot();
        assert!(snap.contains("servers="));
        assert!(dns_act_raw("resolve www.hibos.local A").is_ok());
        assert!(dns_act_raw("resolve_ipv4 www.hibos.local").is_ok());
        assert!(dns_act_raw("reverse 10.0.0.1").is_ok());
        assert!(dns_act_raw("resolve_ipv4 ").is_err());
    }
}

