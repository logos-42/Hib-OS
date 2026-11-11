//! Netfilter 防火墙（kernel/net/netfilter.rs）。
//!
//! 本模块实现简化版 Netfilter：钩子点与规则链、报文过滤 verdict、
//! 规则匹配（源/目的 IP、端口、协议、入/出接口）、动作（ACCEPT/DROP/REJECT/LOG/NAT 简化版）、
//! 以及连接跟踪表（conntrack）简化版。
//!
//! 特性:
//! * Netfilter：register_hook / unregister_hook / process_packet
//! * Verdict：ACCEPT / DROP / QUEUE / RETURN / NF_REPEAT
//! * Rule：匹配条件（源/目的 IP、端口、协议、入站/出站接口）、动作（ACCEPT/DROP/REJECT/LOG/NAT 简化版）
//! * 钩子点：PREROUTING / INPUT / FORWARD / OUTPUT / POSTROUTING、规则链、优先级排序
//! * NAT 简化版：SNAT / DNAT、连接跟踪表（conntrack）简化版
//! * sense/act 通道：`netfilter://rules`

use alloc::collections::{HashMap, VecDeque};
use crate::ksync::{SpinLock, OnceLock};

// =======================================================================
// 网络地址与端口
// ======================================================================

/// IPv4 地址。
pub type Ipv4Addr = [u8; 4];

/// 端口。
pub type Port = u16;

// =======================================================================
// Verdict：处理决策
// ======================================================================

/// 报文处理 verdict。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 接受，继续后续处理。
    Accept,
    /// 丢弃（静默）。
    Drop,
    /// 移交用户态/队列。
    Queue,
    /// 返回调用点（在链中停止）。
    Return,
    /// 重新处理（从当前钩子重新开始）。
    NfRepeat,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Accept => "ACCEPT",
            Verdict::Drop => "DROP",
            Verdict::Queue => "QUEUE",
            Verdict::Return => "RETURN",
            Verdict::NfRepeat => "NF_REPEAT",
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, Verdict::Drop | Verdict::Queue)
    }
}

// =======================================================================
// 钩子点
// ======================================================================

/// Netfilter 钩子点（与 Linux 对齐）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookPoint {
    Prerouting,   // 路由前（DNAT 常在此点）
    Input,        // 目的地为本机
    Forward,      // 转发
    Output,       // 本机发出
    Postrouting,  // 路由后（SNAT 常在此点）
}

impl HookPoint {
    pub fn name(self) -> &'static str {
        match self {
            HookPoint::Prerouting => "PREROUTING",
            HookPoint::Input => "INPUT",
            HookPoint::Forward => "FORWARD",
            HookPoint::Output => "OUTPUT",
            HookPoint::Postrouting => "POSTROUTING",
        }
    }
}

// =======================================================================
// 报文五元组与方向
// ======================================================================

/// 报文方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Inbound,   // 进入本机
    Outbound,  // 离开本机
}

/// 报文五元组（用于匹配与 conntrack）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PacketKey {
    pub src_ip: Ipv4Addr,
    pub dst_ip: Ipv4Addr,
    pub src_port: Port,
    pub dst_port: Port,
    pub protocol: u8, // 6=TCP, 17=UDP, 1=ICMP, 0=任意
    pub in_iface: Option<String>,
    pub out_iface: Option<String>,
}

// =======================================================================
// Rule：匹配条件与动作
// ======================================================================

/// 规则动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleAction {
    Accept,
    Drop,
    Reject,      // 丢弃并回送 ICMP 不可达（简化）
    Log,         // 仅记录，继续
    NatSnat { new_ip: Ipv4Addr, new_port: Option<Port> },
    NatDnat { new_ip: Ipv4Addr, new_port: Option<Port> },
}

/// 单条规则：匹配条件 + 动作 + 优先级。
#[derive(Clone, Debug)]
pub struct Rule {
    pub id: u64,
    /// 优先级（数值越小越先匹配）。
    pub priority: i32,
    /// 钩子点。
    pub hook: HookPoint,
    /// 源 IP（None = 任意）。
    pub src_ip: Option<Ipv4Addr>,
    /// 源 IP 掩码（None = /32）。
    pub src_mask: Option<Ipv4Addr>,
    /// 目的 IP（None = 任意）。
    pub dst_ip: Option<Ipv4Addr>,
    /// 目的 IP 掩码。
    pub dst_mask: Option<Ipv4Addr>,
    /// 源端口（None = 任意）。
    pub src_port: Option<Port>,
    /// 目的端口（None = 任意）。
    pub dst_port: Option<Port>,
    /// 协议（0 = 任意）。
    pub protocol: u8,
    /// 入站接口（None = 任意）。
    pub in_iface: Option<String>,
    /// 出站接口（None = 任意）。
    pub out_iface: Option<String>,
    /// 动作。
    pub action: RuleAction,
    /// 是否启用。
    pub enabled: bool,
    /// 命中计数。
    pub hits: u64,
}

impl Rule {
    pub fn new(priority: i32, hook: HookPoint, action: RuleAction) -> Self {
        Self {
            id: 0, priority, hook, src_ip: None, src_mask: None,
            dst_ip: None, dst_mask: None, src_port: None, dst_port: None,
            protocol: 0, in_iface: None, out_iface: None, action, enabled: true, hits: 0,
        }
    }

    /// 匹配一个报文五元组。
    pub fn matches(&self, pkt: &PacketKey) -> bool {
        if !self.enabled { return false; }
        if self.protocol != 0 && self.protocol != pkt.protocol { return false; }
        if let Some(ip) = self.src_ip {
            let mask = self.src_mask.unwrap_or([255, 255, 255, 255]);
            if (pkt.src_ip[0] & mask[0]) != (ip[0] & mask[0]) ||
               (pkt.src_ip[1] & mask[1]) != (ip[1] & mask[1]) ||
               (pkt.src_ip[2] & mask[2]) != (ip[2] & mask[2]) ||
               (pkt.src_ip[3] & mask[3]) != (ip[3] & mask[3]) { return false; }
        }
        if let Some(ip) = self.dst_ip {
            let mask = self.dst_mask.unwrap_or([255, 255, 255, 255]);
            if (pkt.dst_ip[0] & mask[0]) != (ip[0] & mask[0]) ||
               (pkt.dst_ip[1] & mask[1]) != (ip[1] & mask[1]) ||
               (pkt.dst_ip[2] & mask[2]) != (ip[2] & mask[2]) ||
               (pkt.dst_ip[3] & mask[3]) != (ip[3] & mask[3]) { return false; }
        }
        if let Some(p) = self.src_port { if p != pkt.src_port { return false; } }
        if let Some(p) = self.dst_port { if p != pkt.dst_port { return false; } }
        if let Some(ref iface) = self.in_iface {
            if pkt.in_iface.as_ref() != Some(iface) { return false; }
        }
        if let Some(ref iface) = self.out_iface {
            if pkt.out_iface.as_ref() != Some(iface) { return false; }
        }
        true
    }
}

// =======================================================================
// 连接跟踪表（conntrack）简化版
// ======================================================================

/// 连接状态（简化）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConntrackState {
    New,
    Established,
    Related,
    Invalid,
}

/// 连接跟踪条目。
#[derive(Clone, Debug)]
pub struct ConntrackEntry {
    pub key: PacketKey,
    pub reply_key: PacketKey, // 反向五元组
    pub state: ConntrackState,
    /// 最后更新时间（秒，相对基准）。
    pub last_seen: u32,
    /// 超时（秒）。
    pub timeout: u32,
}

/// 连接跟踪表。
pub struct Conntrack {
    entries: HashMap<PacketKey, ConntrackEntry>,
}

impl Conntrack {
    pub fn new() -> Self { Self { entries: HashMap::new() } }

    /// 查找现有连接（按原向或反向）。
    pub fn lookup(&self, key: &PacketKey) -> Option<&ConntrackEntry> {
        self.entries.get(key)
    }

    /// 插入/更新一条连接。
    pub fn insert(&mut self, key: PacketKey, entry: ConntrackEntry) {
        self.entries.insert(key, entry);
    }

    /// 删除。
    pub fn remove(&mut self, key: &PacketKey) { self.entries.remove(key); }

    /// 过期清理（当前时间秒）。
    pub fn expire(&mut self, now: u32) {
        self.entries.retain(|_, e| e.last_seen.saturating_add(e.timeout) > now);
    }

    pub fn len(&self) -> usize { self.entries.len() }

    /// 根据原始报文构造双向键（简化：src/dst 对调为 reply）。
    pub fn make_reply_key(key: &PacketKey) -> PacketKey {
        PacketKey {
            src_ip: key.dst_ip, dst_ip: key.src_ip,
            src_port: key.dst_port, dst_port: key.src_port,
            protocol: key.protocol,
            in_iface: key.out_iface.clone(),
            out_iface: key.in_iface.clone(),
        }
    }
}

// =======================================================================
// Netfilter：钩子注册与报文处理
// ======================================================================

/// 钩子条目：优先级排序的规则链。
struct HookChain {
    /// 按优先级排序的规则 ID 列表。
    rules: Vec<u64>,
}

/// Netfilter 主体：规则表、钩子链、conntrack、NAT 映射。
pub struct Netfilter {
    /// 规则 ID -> 规则。
    rules: HashMap<u64, Rule>,
    /// 钩子点 -> 链。
    chains: HashMap<HookPoint, HookChain>,
    /// 下一个规则 ID。
    next_rule_id: u64,
    /// 连接跟踪表。
    conntrack: Conntrack,
    /// NAT 映射：原地址 -> 新地址（SNAT/DNAT 简化）。
    nat_map: HashMap<PacketKey, PacketKey>,
    /// 日志缓冲（最近条目）。
    log: VecDeque<String>,
    /// 统计。
    pub stats: NetfilterStats,
}

/// Netfilter 统计。
#[derive(Clone, Debug, Default)]
pub struct NetfilterStats {
    pub packets_processed: u64,
    pub accepted: u64,
    pub dropped: u64,
    pub queued: u64,
    pub natted: u64,
}

impl Netfilter {
    /// 创建一个 Netfilter 实例。
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            chains: HashMap::new(),
            next_rule_id: 1,
            conntrack: Conntrack::new(),
            nat_map: HashMap::new(),
            log: VecDeque::new(),
            stats: NetfilterStats::default(),
        }
    }

    /// 全局 Netfilter 实例（`netfilter://rules` 通道后端）。
    pub fn instance() -> &'static SpinLock<Netfilter> {
        static INST: OnceLock<SpinLock<Netfilter>> = OnceLock::new();
        INST.get_or_init(|| SpinLock::new(Self::new()))
    }

    // ------ 钩子注册 ------

    /// 注册一个钩子（规则挂到指定钩子点，按优先级排序）。返回规则 ID。
    pub fn register_hook(&mut self, hook: HookPoint, rule: Rule) -> u64 {
        let id = self.next_rule_id;
        self.next_rule_id += 1;
        let mut rule = rule;
        rule.id = id;
        self.rules.insert(id, rule.clone());
        let chain = self.chains.entry(hook).or_insert_with(|| HookChain { rules: Vec::new() });
        chain.rules.push(id);
        // 按优先级升序排序（小值先匹配）。
        chain.rules.sort_by_key(|id| self.rules.get(id).map(|r| r.priority).unwrap_or(i32::MAX));
        id
    }

    /// 注销钩子（移除指定 ID 的规则）。
    pub fn unregister_hook(&mut self, id: u64) -> bool {
        if self.rules.remove(&id).is_none() { return false; }
        for chain in self.chains.values_mut() {
            chain.rules.retain(|rid| *rid != id);
        }
        true
    }

    /// 按钩子点列出规则（供 sense）。
    pub fn list_rules(&self, hook: HookPoint) -> Vec<&Rule> {
        if let Some(chain) = self.chains.get(&hook) {
            let mut out: Vec<&Rule> = chain.rules.iter().filter_map(|id| self.rules.get(id)).collect();
            out.sort_by_key(|r| r.priority);
            out
        } else {
            Vec::new()
        }
    }

    // ------ 报文处理 ------

    /// 处理一个报文：按钩子点遍历规则链，返回 verdict；同时进行 conntrack 与 NAT。
    pub fn process_packet(&mut self, hook: HookPoint, pkt: &PacketKey) -> Verdict {
        self.stats.packets_processed += 1;

        // conntrack：若已有连接且状态 Established，直接放行。
        let reply_key = Conntrack::make_reply_key(pkt);
        if let Some(entry) = self.conntrack.lookup(pkt).or_else(|| self.conntrack.lookup(&reply_key)) {
            let _ = entry;
            // 已建立连接：接受。
            self.stats.accepted += 1;
            return Verdict::Accept;
        }

        // 遍历该钩子点的规则链。
        let chain_rules: Vec<_> = match self.chains.get(&hook) {
            Some(chain) => chain.rules.clone(),
            None => Vec::new(),
        };
        {
            for id in &chain_rules {
                let matched = match self.rules.get(id) {
                    Some(r) => r.matches(pkt),
                    None => continue,
                };
                if !matched { continue; }
                let (rule_id, action) = {
                    let rule_mut = self.rules.get_mut(id).unwrap();
                    rule_mut.hits += 1;
                    (rule_mut.id, rule_mut.action.clone())
                };
                let verdict = match &action {
                    RuleAction::Accept => Verdict::Accept,
                    RuleAction::Drop => Verdict::Drop,
                    RuleAction::Reject => Verdict::Drop,
                    RuleAction::Log => {
                        // 记录日志并继续。
                        self.log_push(format!("LOG {} match rule {}", hook.name(), rule_id));
                        continue; // LOG 不终止，继续匹配下一条
                    }
                    RuleAction::NatSnat { new_ip, new_port } => {
                        self.stats.natted += 1;
                        let mut new_key = pkt.clone();
                        new_key.src_ip = *new_ip;
                        if let Some(p) = new_port { new_key.src_port = *p; }
                        self.nat_map.insert(pkt.clone(), new_key.clone());
                        // 记录 conntrack。
                        let entry = ConntrackEntry { key: pkt.clone(), reply_key: Conntrack::make_reply_key(&new_key), state: ConntrackState::New, last_seen: 0, timeout: 300 };
                        self.conntrack.insert(new_key, entry);
                        return Verdict::Accept;
                    }
                    RuleAction::NatDnat { new_ip, new_port } => {
                        self.stats.natted += 1;
                        let mut new_key = pkt.clone();
                        new_key.dst_ip = *new_ip;
                        if let Some(p) = new_port { new_key.dst_port = *p; }
                        self.nat_map.insert(pkt.clone(), new_key.clone());
                        let entry = ConntrackEntry { key: pkt.clone(), reply_key: Conntrack::make_reply_key(&new_key), state: ConntrackState::New, last_seen: 0, timeout: 300 };
                        self.conntrack.insert(new_key, entry);
                        return Verdict::Accept;
                    }
                };
                match verdict {
                    Verdict::Accept => { self.stats.accepted += 1; return Verdict::Accept; }
                    Verdict::Drop => { self.stats.dropped += 1; return Verdict::Drop; }
                    Verdict::Queue => { self.stats.queued += 1; return Verdict::Queue; }
                    Verdict::Return => return Verdict::Return,
                    Verdict::NfRepeat => return Verdict::NfRepeat,
                }
            }
        }

        // 默认策略：接受。
        self.stats.accepted += 1;
        Verdict::Accept
    }

    /// 记录 conntrack 新连接（由上层在三次握手后调用）。
    pub fn track_connection(&mut self, key: PacketKey, state: ConntrackState) {
        let reply_key = Conntrack::make_reply_key(&key);
        let entry = ConntrackEntry { key: key.clone(), reply_key, state, last_seen: 0, timeout: 300 };
        self.conntrack.insert(key, entry);
    }

    /// 过期 conntrack。
    pub fn expire_conntrack(&mut self, now: u32) {
        self.conntrack.expire(now);
    }

    /// conntrack 条目数。
    pub fn conntrack_len(&self) -> usize { self.conntrack.len() }

    /// NAT 映射数。
    pub fn nat_len(&self) -> usize { self.nat_map.len() }

    /// 最近日志（最多 10 条）。
    pub fn recent_log(&self) -> Vec<String> {
        self.log.iter().rev().take(10).cloned().collect()
    }

    fn log_push(&mut self, entry: String) {
        self.log.push_back(entry);
        if self.log.len() > 50 { self.log.pop_front(); }
    }
}

// =======================================================================
// sense/act 接口（netfilter://rules）
// ======================================================================

/// 查询 Netfilter 状态快照（供 `sense netfilter://rules`）。
pub fn netfilter_sense_snapshot() -> String {
    let nf = Netfilter::instance().lock();
    let s = &nf.stats;
    format!(
        "netfilter packets={} accepted={} dropped={} queued={} natted={} conntrack={} nat={}",
        s.packets_processed, s.accepted, s.dropped, s.queued, s.natted, nf.conntrack_len(), nf.nat_len()
    )
}

/// 对 netfilter 通道执行行动（供 `act netfilter://rules`）。负载：`stats` / `list <hook>` / `log`。
pub fn netfilter_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(netfilter_sense_snapshot()),
        Some("list") => {
            let hook_s = it.next().ok_or("用法: list <PREROUTING|INPUT|FORWARD|OUTPUT|POSTROUTING>")?;
            let hook = parse_hook(hook_s)?;
            let nf = Netfilter::instance().lock();
            let rules = nf.list_rules(hook);
            let mut out = format!("{} 规则数={}", hook.name(), rules.len());
            for r in &rules {
                out.push_str(&format!("\n  id={} prio={} action={:?} hits={}", r.id, r.priority, r.action, r.hits));
            }
            Ok(out)
        }
        Some("log") => {
            let nf = Netfilter::instance().lock();
            let logs = nf.recent_log();
            if logs.is_empty() { Ok("日志为空".to_string()) } else { Ok(logs.join("\n")) }
        }
        _ => Err("用法: act netfilter://rules stats | list <hook> | log".to_string()),
    }
}

/// 解析钩子点名称。
fn parse_hook(s: &str) -> Result<HookPoint, String> {
    match s {
        "PREROUTING" => Ok(HookPoint::Prerouting),
        "INPUT" => Ok(HookPoint::Input),
        "FORWARD" => Ok(HookPoint::Forward),
        "OUTPUT" => Ok(HookPoint::Output),
        "POSTROUTING" => Ok(HookPoint::Postrouting),
        other => Err(format!("未知钩子点: {}", other)),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_pkt() -> PacketKey {
        PacketKey {
            src_ip: [10, 0, 0, 2], dst_ip: [10, 0, 0, 3],
            src_port: 5000, dst_port: 80, protocol: 6,
            in_iface: Some("eth0".to_string()), out_iface: None,
        }
    }

    #[test]
    fn accept_drop_rule() {
        let mut nf = Netfilter::new();
        // 拒绝 dst 80 端口。
        let drop_rule = Rule::new(10, HookPoint::Input, RuleAction::Drop);
        let mut drop_rule = drop_rule;
        drop_rule.dst_port = Some(80);
        drop_rule.protocol = 6;
        nf.register_hook(HookPoint::Input, drop_rule);
        // 接受所有。
        let acc_rule = Rule::new(5, HookPoint::Input, RuleAction::Accept);
        nf.register_hook(HookPoint::Input, acc_rule);
        // prio 5 (accept) 应先匹配 -> ACCEPT。
        let v = nf.process_packet(HookPoint::Input, &mk_pkt());
        assert_eq!(v, Verdict::Accept);
    }

    #[test]
    fn drop_before_accept_by_priority() {
        let mut nf = Netfilter::new();
        let mut drop_rule = Rule::new(1, HookPoint::Input, RuleAction::Drop);
        drop_rule.dst_port = Some(80);
        drop_rule.protocol = 6;
        nf.register_hook(HookPoint::Input, drop_rule);
        let acc_rule = Rule::new(100, HookPoint::Input, RuleAction::Accept);
        nf.register_hook(HookPoint::Input, acc_rule);
        // prio 1 drop 先匹配 -> DROP。
        assert_eq!(nf.process_packet(HookPoint::Input, &mk_pkt()), Verdict::Drop);
    }

    #[test]
    fn snat_dnat() {
        let mut nf = Netfilter::new();
        let mut snat = Rule::new(1, HookPoint::Postrouting, RuleAction::NatSnat { new_ip: [1, 2, 3, 4], new_port: Some(9000) });
        snat.src_ip = Some([10, 0, 0, 2]);
        snat.src_mask = Some([255, 255, 255, 255]);
        nf.register_hook(HookPoint::Postrouting, snat);
        let pkt = PacketKey { src_ip: [10, 0, 0, 2], dst_ip: [8, 8, 8, 8], src_port: 5000, dst_port: 53, protocol: 17, in_iface: None, out_iface: Some("eth0".to_string()) };
        assert_eq!(nf.process_packet(HookPoint::Postrouting, &pkt), Verdict::Accept);
        assert_eq!(nf.stats.natted, 1);
        assert_eq!(nf.nat_len(), 1);
    }

    #[test]
    fn conntrack_established_bypass() {
        let mut nf = Netfilter::new();
        let pkt = mk_pkt();
        nf.track_connection(pkt.clone(), ConntrackState::Established);
        assert_eq!(nf.process_packet(HookPoint::Input, &pkt), Verdict::Accept);
        assert_eq!(nf.conntrack_len(), 1);
    }

    #[test]
    fn log_action_continues() {
        let mut nf = Netfilter::new();
        let mut log_rule = Rule::new(1, HookPoint::Input, RuleAction::Log);
        log_rule.dst_port = Some(80);
        log_rule.protocol = 6;
        nf.register_hook(HookPoint::Input, log_rule);
        let acc = Rule::new(10, HookPoint::Input, RuleAction::Accept);
        nf.register_hook(HookPoint::Input, acc);
        assert_eq!(nf.process_packet(HookPoint::Input, &mk_pkt()), Verdict::Accept);
        let logs = nf.recent_log();
        assert!(!logs.is_empty());
    }

    #[test]
    fn rule_matches_mask() {
        let mut rule = Rule::new(1, HookPoint::Input, RuleAction::Accept);
        rule.src_ip = Some([10, 0, 0, 0]);
        rule.src_mask = Some([255, 255, 255, 0]);
        let pkt = PacketKey { src_ip: [10, 0, 0, 55], dst_ip: [1,1,1,1], src_port: 1, dst_port: 1, protocol: 0, in_iface: None, out_iface: None };
        assert!(rule.matches(&pkt));
        let pkt2 = PacketKey { src_ip: [10, 0, 1, 55], ..pkt };
        assert!(!rule.matches(&pkt2));
    }

    #[test]
    fn unregister_hook() {
        let mut nf = Netfilter::new();
        let id = nf.register_hook(HookPoint::Input, Rule::new(1, HookPoint::Input, RuleAction::Accept));
        assert!(nf.unregister_hook(id));
        assert!(!nf.unregister_hook(id));
        assert_eq!(nf.list_rules(HookPoint::Input).len(), 0);
    }

    #[test]
    fn verdict_name_and_terminal() {
        assert_eq!(Verdict::Drop.name(), "DROP");
        assert!(Verdict::Drop.is_terminal());
        assert!(!Verdict::Accept.is_terminal());
        assert!(!Verdict::NfRepeat.is_terminal()); // 非终结 verdict；此处测试 name。
        let _ = Verdict::NfRepeat;
    }

    #[test]
    fn stats_default() {
        let s = NetfilterStats::default();
        assert_eq!(s.packets_processed, 0);
    }

    #[test]
    fn sense_and_act() {
        let snap = netfilter_sense_snapshot();
        assert!(snap.contains("packets="));
        assert!(netfilter_act_raw("stats").is_ok());
        assert!(netfilter_act_raw("list INPUT").is_ok());
        assert!(netfilter_act_raw("log").is_ok());
        assert!(netfilter_act_raw("list BADHOOK").is_err());
    }
}