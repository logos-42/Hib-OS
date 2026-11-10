//! 多智能体社会层 —— AI 原生 OS 的「智能体社会」 (Multi-Agent Society)
//!
//! 本模块实现 AI 原生 OS 的多智能体交互与社会组织，对应传统 OS 中的
//! 「进程间通信 / 用户管理 / 访问控制」概念，但重新映射到智能体社会域：
//!
//! * AgentSociety (智能体社会) —— 智能体群体的整体结构，等价于操作系统用户空间；
//! * AgentIdentity (智能体身份) —— 每个智能体的唯一标识与元数据，等价于 UID/PID；
//! * AgentProtocol (智能体协议) —— 智能体间通信协议，等价于 IPC 协议；
//! * MessageBus (消息总线) —— 智能体间消息传递中枢，等价于消息队列 / 总线；
//! * Conversation (对话) —— 智能体间的对话会话，等价于会话 / 连接；
//! * SocialGraph (社会图) —— 智能体间的关系网络，等价于社交图 / 访问控制矩阵；
//! * Negotiation (协商) —— 智能体间资源与目标协商，等价于死锁避免 / 资源谈判；
//! * ConflictResolution (冲突解决) —— 智能体冲突的调解机制，等价于调度冲突解决；
//! * Reputation (信誉) —— 智能体的信任度评分，等价于进程优先级 / 信用评分；
//! * SocietyEngine (社会引擎) —— 统一入口，通过 society://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 智能体身份 (AgentIdentity)
// =====================================================================================

/// 智能体身份：每个智能体的唯一标识与元数据。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AgentIdentity {
    pub id: u64,
    pub name: String,
    pub agent_type: String,
    pub created_at: u128,
    pub capabilities: BTreeSet<String>,
    pub metadata: BTreeMap<String, String>,
}

impl AgentIdentity {
    pub fn new(id: u64, name: &str, agent_type: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            agent_type: agent_type.to_string(),
            created_at: crate::ai_os::native::now_ns(),
            capabilities: BTreeSet::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn add_capability(&mut self, cap: &str) {
        self.capabilities.insert(cap.to_string());
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.contains(cap)
    }
}

// =====================================================================================
// 消息 (Message)
// =====================================================================================

/// 消息类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageType {
    Request,    // 请求
    Response,   // 响应
    Broadcast,  // 广播
    Event,      // 事件
    Command,    // 命令
    Query,      // 查询
}

/// 消息：智能体间的通信单元。
#[derive(Clone, Debug)]
pub struct Message {
    pub id: u64,
    pub from: u64,           // 发送者 ID
    pub to: Option<u64>,     // 接收者 ID (None 表示广播)
    pub r#type: MessageType,
    pub content: String,
    pub timestamp_ns: u128,
    pub ttl_ns: u128,        // 生存时间
    pub priority: f32,       // 0~1
    pub metadata: BTreeMap<String, String>,
}

impl Message {
    pub fn new(from: u64, to: Option<u64>, r#type: MessageType, content: &str) -> Self {
        Self {
            id: 0,
            from,
            to,
            r#type,
            content: content.to_string(),
            timestamp_ns: crate::ai_os::native::now_ns(),
            ttl_ns: 60_000_000_000, // 60 秒
            priority: 0.5,
            metadata: BTreeMap::new(),
        }
    }

    pub fn is_expired(&self, now_ns: u128) -> bool {
        now_ns - self.timestamp_ns > self.ttl_ns
    }
}

// =====================================================================================
// 消息总线 (MessageBus)
// =====================================================================================

/// 消息总线：智能体间消息传递中枢。
pub struct MessageBus {
    messages: VecDeque<Message>,
    subscriptions: BTreeMap<u64, BTreeSet<MessageType>>,
    next_msg_id: u64,
    pub max_queue_size: usize,
    pub total_sent: u64,
    pub total_delivered: u64,
    pub total_dropped: u64,
}

impl MessageBus {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            subscriptions: BTreeMap::new(),
            next_msg_id: 1,
            max_queue_size: 100000,
            total_sent: 0,
            total_delivered: 0,
            total_dropped: 0,
        }
    }

    pub fn subscribe(&mut self, agent_id: u64, msg_type: MessageType) {
        self.subscriptions.entry(agent_id).or_default().insert(msg_type);
    }

    pub fn unsubscribe(&mut self, agent_id: u64, msg_type: MessageType) {
        if let Some(types) = self.subscriptions.get_mut(&agent_id) {
            types.remove(&msg_type);
        }
    }

    pub fn publish(&mut self, msg: Message) -> u64 {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        let mut msg = msg;
        msg.id = id;

        // 检查过期
        let now = crate::ai_os::native::now_ns();
        if msg.is_expired(now) {
            self.total_dropped += 1;
            return id;
        }

        // 投递给订阅者
        let mut delivered = 0;
        for (agent_id, types) in &self.subscriptions {
            if types.contains(&msg.r#type) {
                if let Some(to) = msg.to {
                    if to == *agent_id || to == 0 {
                        delivered += 1;
                    }
                } else {
                    delivered += 1; // 广播
                }
            }
        }

        self.messages.push_back(msg);
        if self.messages.len() > self.max_queue_size {
            self.messages.pop_front();
            self.total_dropped += 1;
        }

        self.total_sent += 1;
        self.total_delivered += delivered as u64;
        id
    }

    pub fn poll(&mut self, agent_id: u64) -> Option<Message> {
        // 返回该代理订阅的最新消息
        let types = self.subscriptions.get(&agent_id)?;
        for i in (0..self.messages.len()).rev() {
            if let Some(msg) = self.messages.get(i) {
                if types.contains(&msg.r#type) {
                    if let Some(to) = msg.to {
                        if to == agent_id || to == 0 {
                            return Some(self.messages.remove(i).unwrap());
                        }
                    } else {
                        return Some(self.messages.remove(i).unwrap());
                    }
                }
            }
        }
        None
    }

    pub fn queue_len(&self) -> usize {
        self.messages.len()
    }

    pub fn stats(&self) -> String {
        format!(
            "sent={} delivered={} dropped={} queue={}",
            self.total_sent, self.total_delivered, self.total_dropped, self.messages.len()
        )
    }
}

// =====================================================================================
// 对话 (Conversation)
// =====================================================================================

/// 对话参与者角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationRole {
    Initiator,  // 发起者
    Responder,  // 响应者
    Observer,   // 观察者
}

/// 对话：智能体间的对话会话。
#[derive(Clone, Debug)]
pub struct Conversation {
    pub id: u64,
    pub participants: BTreeSet<u64>,
    pub messages: Vec<u64>,           // 消息 ID 列表
    pub created_at: u128,
    pub last_activity_ns: u128,
    pub status: ConversationStatus,
}

/// 对话状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationStatus {
    Active,
    Paused,
    Completed,
    Aborted,
}

/// 对话管理器。
pub struct ConversationManager {
    conversations: BTreeMap<u64, Conversation>,
    next_id: u64,
    agent_conversations: BTreeMap<u64, BTreeSet<u64>>, // agent_id -> conversation ids
}

impl ConversationManager {
    pub fn new() -> Self {
        Self {
            conversations: BTreeMap::new(),
            next_id: 1,
            agent_conversations: BTreeMap::new(),
        }
    }

    pub fn create(&mut self, participants: &[u64]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::ai_os::native::now_ns();
        let conv = Conversation {
            id,
            participants: participants.iter().copied().collect(),
            messages: Vec::new(),
            created_at: now,
            last_activity_ns: now,
            status: ConversationStatus::Active,
        };
        self.conversations.insert(id, conv);
        for &p in participants {
            self.agent_conversations.entry(p).or_default().insert(id);
        }
        id
    }

    pub fn add_message(&mut self, conv_id: u64, msg_id: u64) -> bool {
        if let Some(conv) = self.conversations.get_mut(&conv_id) {
            conv.messages.push(msg_id);
            conv.last_activity_ns = crate::ai_os::native::now_ns();
            true
        } else {
            false
        }
    }

    pub fn get(&self, id: u64) -> Option<&Conversation> {
        self.conversations.get(&id)
    }

    pub fn agent_conversations(&self, agent_id: u64) -> Vec<&Conversation> {
        self.agent_conversations
            .get(&agent_id)
            .map(|ids| ids.iter().filter_map(|cid| self.conversations.get(cid)).collect())
            .unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.conversations.len()
    }
}

// =====================================================================================
// 社会图 (SocialGraph)
// =====================================================================================

/// 关系类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationType {
    Peer,       // 对等
    Mentor,     // 导师
    Mentee,     // 学徒
    Collaborator, // 协作者
    Competitor, // 竞争者
    Unknown,    // 未知
}

/// 边：两个智能体间的关系。
#[derive(Clone, Debug)]
pub struct SocialEdge {
    pub from: u64,
    pub to: u64,
    pub r#type: RelationType,
    pub strength: f32,    // 0~1
    pub updated_at: u128,
}

/// 社会图：智能体间的关系网络。
pub struct SocialGraph {
    edges: BTreeMap<(u64, u64), SocialEdge>,
    agents: BTreeSet<u64>,
}

impl SocialGraph {
    pub fn new() -> Self {
        Self {
            edges: BTreeMap::new(),
            agents: BTreeSet::new(),
        }
    }

    pub fn add_agent(&mut self, agent_id: u64) {
        self.agents.insert(agent_id);
    }

    pub fn add_edge(&mut self, from: u64, to: u64, r#type: RelationType, strength: f32) {
        self.agents.insert(from);
        self.agents.insert(to);
        self.edges.insert(
            (from, to),
            SocialEdge {
                from,
                to,
                r#type,
                strength: strength.clamp(0.0, 1.0),
                updated_at: crate::ai_os::native::now_ns(),
            },
        );
    }

    pub fn get_edge(&self, from: u64, to: u64) -> Option<&SocialEdge> {
        self.edges.get(&(from, to))
    }

    pub fn neighbors(&self, agent_id: u64) -> Vec<(u64, RelationType, f32)> {
        self.edges
            .iter()
            .filter(|((f, _), _)| *f == agent_id)
            .map(|((_, to), e)| (*to, e.r#type, e.strength))
            .collect()
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

// =====================================================================================
// 协商 (Negotiation)
// =====================================================================================

/// 协商状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegotiationStatus {
    Proposed,
    Accepted,
    Rejected,
    Completed,
    Aborted,
}

/// 协商：智能体间资源与目标协商。
pub struct Negotiation {
    pub id: u64,
    pub initiator: u64,
    pub responder: u64,
    pub resource: String,
    pub proposal: String,
    pub status: NegotiationStatus,
    pub created_at: u128,
    pub resolved_at: Option<u128>,
}

/// 协商管理器。
pub struct NegotiationManager {
    negotiations: BTreeMap<u64, Negotiation>,
    next_id: u64,
}

impl NegotiationManager {
    pub fn new() -> Self {
        Self {
            negotiations: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn propose(&mut self, initiator: u64, responder: u64, resource: &str, proposal: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let neg = Negotiation {
            id,
            initiator,
            responder,
            resource: resource.to_string(),
            proposal: proposal.to_string(),
            status: NegotiationStatus::Proposed,
            created_at: crate::ai_os::native::now_ns(),
            resolved_at: None,
        };
        self.negotiations.insert(id, neg);
        id
    }

    pub fn accept(&mut self, id: u64) -> bool {
        if let Some(neg) = self.negotiations.get_mut(&id) {
            if neg.status == NegotiationStatus::Proposed {
                neg.status = NegotiationStatus::Accepted;
                neg.resolved_at = Some(crate::ai_os::native::now_ns());
                return true;
            }
        }
        false
    }

    pub fn reject(&mut self, id: u64) -> bool {
        if let Some(neg) = self.negotiations.get_mut(&id) {
            if neg.status == NegotiationStatus::Proposed {
                neg.status = NegotiationStatus::Rejected;
                neg.resolved_at = Some(crate::ai_os::native::now_ns());
                return true;
            }
        }
        false
    }

    pub fn get(&self, id: u64) -> Option<&Negotiation> {
        self.negotiations.get(&id)
    }

    pub fn count(&self) -> usize {
        self.negotiations.len()
    }

    pub fn pending_count(&self) -> usize {
        self.negotiations.values().filter(|n| n.status == NegotiationStatus::Proposed).count()
    }
}

// =====================================================================================
// 信誉 (Reputation)
// =====================================================================================

/// 信誉评分：智能体的信任度。
#[derive(Clone, Debug)]
pub struct ReputationScore {
    pub agent_id: u64,
    pub score: f32,           // 0~1
    pub interactions: u64,
    pub positive: u64,
    pub negative: u64,
    pub updated_at: u128,
}

/// 信誉系统。
pub struct ReputationSystem {
    scores: BTreeMap<u64, ReputationScore>,
}

impl ReputationSystem {
    pub fn new() -> Self {
        Self {
            scores: BTreeMap::new(),
        }
    }

    pub fn get_or_create(&mut self, agent_id: u64) -> &mut ReputationScore {
        self.scores.entry(agent_id).or_insert(ReputationScore {
            agent_id,
            score: 0.5,
            interactions: 0,
            positive: 0,
            negative: 0,
            updated_at: crate::ai_os::native::now_ns(),
        })
    }

    pub fn rate(&mut self, agent_id: u64, positive: bool) {
        let score = self.get_or_create(agent_id);
        score.interactions += 1;
        if positive {
            score.positive += 1;
            score.score = (score.score + 0.05).min(1.0);
        } else {
            score.negative += 1;
            score.score = (score.score - 0.05).max(0.0);
        }
        score.updated_at = crate::ai_os::native::now_ns();
    }

    pub fn score(&self, agent_id: u64) -> f32 {
        self.scores.get(&agent_id).map(|s| s.score).unwrap_or(0.5)
    }

    pub fn count(&self) -> usize {
        self.scores.len()
    }
}

// =====================================================================================
// 冲突解决 (ConflictResolution)
// =====================================================================================

/// 冲突类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictType {
    Resource,   // 资源冲突
    Goal,       // 目标冲突
    Priority,   // 优先级冲突
    Value,      // 价值观冲突
}

/// 冲突记录。
#[derive(Clone, Debug)]
pub struct Conflict {
    pub id: u64,
    pub r#type: ConflictType,
    pub agents: Vec<u64>,
    pub description: String,
    pub resolved: bool,
    pub resolution: Option<String>,
    pub created_at: u128,
}

/// 冲突解决器。
pub struct ConflictResolver {
    conflicts: BTreeMap<u64, Conflict>,
    next_id: u64,
}

impl ConflictResolver {
    pub fn new() -> Self {
        Self {
            conflicts: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn report(&mut self, r#type: ConflictType, agents: Vec<u64>, description: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.conflicts.insert(
            id,
            Conflict {
                id,
                r#type,
                agents,
                description: description.to_string(),
                resolved: false,
                resolution: None,
                created_at: crate::ai_os::native::now_ns(),
            },
        );
        id
    }

    pub fn resolve(&mut self, id: u64, resolution: &str) -> bool {
        if let Some(c) = self.conflicts.get_mut(&id) {
            c.resolved = true;
            c.resolution = Some(resolution.to_string());
            true
        } else {
            false
        }
    }

    pub fn unresolved_count(&self) -> usize {
        self.conflicts.values().filter(|c| !c.resolved).count()
    }

    pub fn count(&self) -> usize {
        self.conflicts.len()
    }
}

// =====================================================================================
// 社会引擎 (SocietyEngine) —— 统一入口
// =====================================================================================

/// 社会引擎：整合智能体身份、消息总线、对话、社会图、协商、信誉、冲突解决。
pub struct SocietyEngine {
    pub agents: BTreeMap<u64, AgentIdentity>,
    pub bus: MessageBus,
    pub conversations: ConversationManager,
    pub social_graph: SocialGraph,
    pub negotiations: NegotiationManager,
    pub reputation: ReputationSystem,
    pub conflicts: ConflictResolver,
    pub next_agent_id: u64,
    pub enabled: bool,
    pub total_interactions: u64,
}

impl SocietyEngine {
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            bus: MessageBus::new(),
            conversations: ConversationManager::new(),
            social_graph: SocialGraph::new(),
            negotiations: NegotiationManager::new(),
            reputation: ReputationSystem::new(),
            conflicts: ConflictResolver::new(),
            next_agent_id: 1,
            enabled: true,
            total_interactions: 0,
        }
    }

    pub fn register_agent(&mut self, name: &str, agent_type: &str) -> u64 {
        let id = self.next_agent_id;
        self.next_agent_id += 1;
        let agent = AgentIdentity::new(id, name, agent_type);
        self.agents.insert(id, agent);
        self.social_graph.add_agent(id);
        id
    }

    pub fn get_agent(&self, id: u64) -> Option<&AgentIdentity> {
        self.agents.get(&id)
    }

    pub fn send_message(&mut self, from: u64, to: Option<u64>, r#type: MessageType, content: &str) -> u64 {
        let msg = Message::new(from, to, r#type, content);
        let id = self.bus.publish(msg);
        self.total_interactions += 1;
        id
    }

    pub fn subscribe(&mut self, agent_id: u64, msg_type: MessageType) {
        self.bus.subscribe(agent_id, msg_type);
    }

    pub fn poll_messages(&mut self, agent_id: u64) -> Option<Message> {
        self.bus.poll(agent_id)
    }

    pub fn create_conversation(&mut self, participants: &[u64]) -> u64 {
        self.total_interactions += 1;
        self.conversations.create(participants)
    }

    pub fn add_social_edge(&mut self, from: u64, to: u64, r#type: RelationType, strength: f32) {
        self.social_graph.add_edge(from, to, r#type, strength);
        self.total_interactions += 1;
    }

    pub fn propose_negotiation(&mut self, initiator: u64, responder: u64, resource: &str, proposal: &str) -> u64 {
        self.total_interactions += 1;
        self.negotiations.propose(initiator, responder, resource, proposal)
    }

    pub fn accept_negotiation(&mut self, id: u64) -> bool {
        self.negotiations.accept(id)
    }

    pub fn reject_negotiation(&mut self, id: u64) -> bool {
        self.negotiations.reject(id)
    }

    pub fn rate_agent(&mut self, agent_id: u64, positive: bool) {
        self.reputation.rate(agent_id, positive);
    }

    pub fn agent_reputation(&self, agent_id: u64) -> f32 {
        self.reputation.score(agent_id)
    }

    pub fn report_conflict(&mut self, r#type: ConflictType, agents: Vec<u64>, description: &str) -> u64 {
        self.conflicts.report(r#type, agents, description)
    }

    pub fn resolve_conflict(&mut self, id: u64, resolution: &str) -> bool {
        self.conflicts.resolve(id, resolution)
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub fn tick(&mut self) {
        // 周期性维护
        let _ = &self.bus;
    }
}

impl Default for SocietyEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SOCIETY: OnceLock<Mutex<SocietyEngine>> = OnceLock::new();

pub fn society_engine() -> &'static Mutex<SocietyEngine> {
    SOCIETY.get_or_init(|| Mutex::new(SocietyEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = society_engine();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = society_engine().lock().unwrap();
    format!(
        "society: agents={} messages_sent={} messages_delivered={} conversations={} edges={} negotiations={} pending_negotiations={} reputations={} conflicts={} unresolved_conflicts={} total_interactions={}",
        eng.agent_count(),
        eng.bus.total_sent,
        eng.bus.total_delivered,
        eng.conversations.count(),
        eng.social_graph.edge_count(),
        eng.negotiations.count(),
        eng.negotiations.pending_count(),
        eng.reputation.count(),
        eng.conflicts.count(),
        eng.conflicts.unresolved_count(),
        eng.total_interactions
    )
}

pub fn society_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn society_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("register-agent") => {
            let name = it.next().ok_or("用法: society:// register-agent <名称> <类型>")?;
            let agent_type = it.next().unwrap_or("generic");
            let mut eng = society_engine().lock().unwrap();
            let id = eng.register_agent(name, agent_type);
            Ok(format!("已注册智能体 id={} 名称={} 类型={}", id, name, agent_type))
        }
        Some("send") => {
            let from = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少发送者id")?;
            let to_str = it.next().unwrap_or("broadcast");
            let to = if to_str == "broadcast" { None } else { Some(to_str.parse::<u64>().unwrap_or(0)) };
            let type_str = it.next().unwrap_or("request");
            let msg_type = match type_str {
                "request" => MessageType::Request,
                "response" => MessageType::Response,
                "broadcast" => MessageType::Broadcast,
                "event" => MessageType::Event,
                "command" => MessageType::Command,
                "query" => MessageType::Query,
                _ => MessageType::Request,
            };
            let content: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = society_engine().lock().unwrap();
            let id = eng.send_message(from, to, msg_type, &content);
            Ok(format!("已发送消息 id={} 来自={}", id, from))
        }
        Some("subscribe") => {
            let agent_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少智能体id")?;
            let type_str = it.next().unwrap_or("request");
            let msg_type = match type_str {
                "request" => MessageType::Request,
                "response" => MessageType::Response,
                "broadcast" => MessageType::Broadcast,
                "event" => MessageType::Event,
                "command" => MessageType::Command,
                "query" => MessageType::Query,
                _ => MessageType::Request,
            };
            let mut eng = society_engine().lock().unwrap();
            eng.subscribe(agent_id, msg_type);
            Ok(format!("智能体 {} 已订阅 {:?}", agent_id, msg_type))
        }
        Some("poll") => {
            let agent_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少智能体id")?;
            let mut eng = society_engine().lock().unwrap();
            match eng.poll_messages(agent_id) {
                Some(msg) => Ok(format!("消息[id={}] 来自={} 内容={}", msg.id, msg.from, msg.content)),
                None => Ok("无新消息".into()),
            }
        }
        Some("conversation") => {
            let participants: Vec<u64> = it.filter_map(|s| s.parse().ok()).collect();
            if participants.is_empty() {
                return Err("缺少参与者id".into());
            }
            let mut eng = society_engine().lock().unwrap();
            let id = eng.create_conversation(&participants);
            Ok(format!("已创建对话 id={} 参与者={:?}", id, participants))
        }
        Some("edge") => {
            let from = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少源id")?;
            let to = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少目标id")?;
            let type_str = it.next().unwrap_or("peer");
            let rel_type = match type_str {
                "peer" => RelationType::Peer,
                "mentor" => RelationType::Mentor,
                "mentee" => RelationType::Mentee,
                "collaborator" => RelationType::Collaborator,
                "competitor" => RelationType::Competitor,
                _ => RelationType::Unknown,
            };
            let strength = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.5);
            let mut eng = society_engine().lock().unwrap();
            eng.add_social_edge(from, to, rel_type, strength);
            Ok(format!("已添加社会边 {} --{:?}--> {} 强度={:.2}", from, rel_type, to, strength))
        }
        Some("negotiate") => {
            let initiator = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少发起者id")?;
            let responder = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少响应者id")?;
            let resource = it.next().ok_or("缺少资源")?;
            let proposal = it.next().unwrap_or("");
            let mut eng = society_engine().lock().unwrap();
            let id = eng.propose_negotiation(initiator, responder, resource, proposal);
            Ok(format!("已发起协商 id={} 资源={}", id, resource))
        }
        Some("accept") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少协商id")?;
            let mut eng = society_engine().lock().unwrap();
            if eng.accept_negotiation(id) {
                Ok(format!("协商 id={} 已接受", id))
            } else {
                Err(format!("协商 id={} 无法接受", id))
            }
        }
        Some("reject") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少协商id")?;
            let mut eng = society_engine().lock().unwrap();
            if eng.reject_negotiation(id) {
                Ok(format!("协商 id={} 已拒绝", id))
            } else {
                Err(format!("协商 id={} 无法拒绝", id))
            }
        }
        Some("rate") => {
            let agent_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少智能体id")?;
            let positive = it.next().and_then(|s| s.parse::<bool>().ok()).unwrap_or(true);
            let mut eng = society_engine().lock().unwrap();
            eng.rate_agent(agent_id, positive);
            Ok(format!("已评价智能体 {} 正面={}", agent_id, positive))
        }
        Some("reputation") => {
            let agent_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少智能体id")?;
            let eng = society_engine().lock().unwrap();
            Ok(format!("智能体 {} 信誉={:.2}", agent_id, eng.agent_reputation(agent_id)))
        }
        Some("conflict") => {
            let type_str = it.next().unwrap_or("resource");
            let ctype = match type_str {
                "resource" => ConflictType::Resource,
                "goal" => ConflictType::Goal,
                "priority" => ConflictType::Priority,
                "value" => ConflictType::Value,
                _ => ConflictType::Resource,
            };
            let agents: Vec<u64> = it.filter_map(|s| s.parse().ok()).collect();
            if agents.len() < 2 {
                return Err("需要至少2个智能体id".into());
            }
            let description: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = society_engine().lock().unwrap();
            let id = eng.report_conflict(ctype, agents, &description);
            Ok(format!("已报告冲突 id={} 类型={:?}", id, ctype))
        }
        Some("resolve-conflict") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少冲突id")?;
            let resolution: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = society_engine().lock().unwrap();
            if eng.resolve_conflict(id, &resolution) {
                Ok(format!("冲突 id={} 已解决: {}", id, resolution))
            } else {
                Err(format!("冲突 id={} 不存在", id))
            }
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 society 动作: {}", other)),
    }
}

pub fn society_act_dispatch(payload: &str) -> Result<String, String> {
    society_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_identity() {
        let mut agent = AgentIdentity::new(1, "alice", "assistant");
        agent.add_capability("reasoning");
        agent.add_capability("planning");
        assert!(agent.has_capability("reasoning"));
        assert!(!agent.has_capability("coding"));
        assert_eq!(agent.name, "alice");
    }

    #[test]
    fn message_creation() {
        let msg = Message::new(1, Some(2), MessageType::Request, "hello");
        assert_eq!(msg.from, 1);
        assert_eq!(msg.to, Some(2));
        assert_eq!(msg.content, "hello");
        assert!(!msg.is_expired(crate::ai_os::native::now_ns()));
    }

    #[test]
    fn message_expired() {
        let mut msg = Message::new(1, None, MessageType::Broadcast, "test");
        msg.ttl_ns = 1;
        msg.timestamp_ns = 0;
        assert!(msg.is_expired(crate::ai_os::native::now_ns()));
    }

    #[test]
    fn message_bus_publish_poll() {
        let mut bus = MessageBus::new();
        bus.subscribe(2, MessageType::Request);
        let msg = Message::new(1, Some(2), MessageType::Request, "hello");
        bus.publish(msg);
        let received = bus.poll(2);
        assert!(received.is_some());
        assert_eq!(received.unwrap().content, "hello");
    }

    #[test]
    fn message_bus_broadcast() {
        let mut bus = MessageBus::new();
        bus.subscribe(2, MessageType::Broadcast);
        bus.subscribe(3, MessageType::Broadcast);
        let msg = Message::new(1, None, MessageType::Broadcast, "hello all");
        bus.publish(msg);
        assert_eq!(bus.poll(2).is_some(), true);
        assert_eq!(bus.poll(3).is_some(), true);
    }

    #[test]
    fn conversation_create() {
        let mut cm = ConversationManager::new();
        let id = cm.create(&[1, 2, 3]);
        assert_eq!(id, 1);
        let conv = cm.get(id).unwrap();
        assert_eq!(conv.participants.len(), 3);
        assert_eq!(cm.count(), 1);
    }

    #[test]
    fn conversation_add_message() {
        let mut cm = ConversationManager::new();
        let id = cm.create(&[1, 2]);
        assert!(cm.add_message(id, 100));
        let conv = cm.get(id).unwrap();
        assert_eq!(conv.messages, vec![100]);
    }

    #[test]
    fn social_graph() {
        let mut sg = SocialGraph::new();
        sg.add_edge(1, 2, RelationType::Collaborator, 0.8);
        sg.add_edge(1, 3, RelationType::Mentor, 0.6);
        assert_eq!(sg.agent_count(), 3);
        assert_eq!(sg.edge_count(), 2);
        let neighbors = sg.neighbors(1);
        assert_eq!(neighbors.len(), 2);
    }

    #[test]
    fn negotiation() {
        let mut nm = NegotiationManager::new();
        let id = nm.propose(1, 2, "gpu", "share for 1 hour");
        assert_eq!(id, 1);
        assert_eq!(nm.pending_count(), 1);
        assert!(nm.accept(id));
        assert_eq!(nm.pending_count(), 0);
    }

    #[test]
    fn negotiation_reject() {
        let mut nm = NegotiationManager::new();
        let id = nm.propose(1, 2, "gpu", "share");
        assert!(nm.reject(id));
        assert_eq!(nm.pending_count(), 0);
    }

    #[test]
    fn reputation() {
        let mut rep = ReputationSystem::new();
        rep.rate(1, true);
        rep.rate(1, true);
        rep.rate(1, false);
        assert!(rep.score(1) > 0.5);
        assert_eq!(rep.count(), 1);
    }

    #[test]
    fn conflict_resolver() {
        let mut cr = ConflictResolver::new();
        let id = cr.report(ConflictType::Resource, vec![1, 2], "gpu争用");
        assert_eq!(id, 1);
        assert_eq!(cr.unresolved_count(), 1);
        assert!(cr.resolve(id, "轮流使用"));
        assert_eq!(cr.unresolved_count(), 0);
    }

    #[test]
    fn society_engine_register() {
        let mut eng = SocietyEngine::new();
        let id = eng.register_agent("alice", "assistant");
        assert_eq!(id, 1);
        assert_eq!(eng.agent_count(), 1);
        let agent = eng.get_agent(id).unwrap();
        assert_eq!(agent.name, "alice");
    }

    #[test]
    fn society_engine_send() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("alice", "assistant");
        eng.register_agent("bob", "assistant");
        eng.subscribe(2, MessageType::Request);
        let id = eng.send_message(1, Some(2), MessageType::Request, "help");
        assert_eq!(id, 1);
        let msg = eng.poll_messages(2).unwrap();
        assert_eq!(msg.content, "help");
    }

    #[test]
    fn society_engine_conversation() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("a", "t");
        eng.register_agent("b", "t");
        let id = eng.create_conversation(&[1, 2]);
        assert_eq!(id, 1);
    }

    #[test]
    fn society_engine_edge() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("a", "t");
        eng.register_agent("b", "t");
        eng.add_social_edge(1, 2, RelationType::Peer, 0.7);
        let neighbors = eng.social_graph.neighbors(1);
        assert_eq!(neighbors.len(), 1);
    }

    #[test]
    fn society_engine_negotiation() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("a", "t");
        eng.register_agent("b", "t");
        let id = eng.propose_negotiation(1, 2, "cpu", "share");
        assert_eq!(id, 1);
        assert!(eng.accept_negotiation(id));
    }

    #[test]
    fn society_engine_reputation() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("a", "t");
        eng.rate_agent(1, true);
        eng.rate_agent(1, true);
        assert!(eng.agent_reputation(1) > 0.5);
    }

    #[test]
    fn society_engine_conflict() {
        let mut eng = SocietyEngine::new();
        eng.register_agent("a", "t");
        eng.register_agent("b", "t");
        let id = eng.report_conflict(ConflictType::Resource, vec![1, 2], "争用");
        assert_eq!(id, 1);
        assert!(eng.resolve_conflict(id, "协商解决"));
        assert_eq!(eng.conflicts.unresolved_count(), 0);
    }

    #[test]
    fn society_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("society:"));
        assert!(s.contains("agents="));
    }

    #[test]
    fn society_act_register() {
        init();
        let res = society_act("register-agent alice assistant").unwrap();
        assert!(res.contains("已注册智能体"));
    }

    #[test]
    fn society_act_send() {
        init();
        society_act("register-agent alice assistant").unwrap();
        society_act("register-agent bob assistant").unwrap();
        society_act("subscribe 2 request").unwrap();
        let res = society_act("send 1 2 request help").unwrap();
        assert!(res.contains("已发送消息"));
    }

    #[test]
    fn society_act_poll() {
        init();
        society_act("register-agent alice assistant").unwrap();
        society_act("register-agent bob assistant").unwrap();
        society_act("subscribe 2 request").unwrap();
        society_act("send 1 2 request help").unwrap();
        let res = society_act("poll 2").unwrap();
        assert!(res.contains("help"));
    }

    #[test]
    fn society_act_conversation() {
        init();
        society_act("register-agent a t").unwrap();
        society_act("register-agent b t").unwrap();
        let res = society_act("conversation 1 2").unwrap();
        assert!(res.contains("对话"));
    }

    #[test]
    fn society_act_edge() {
        init();
        society_act("register-agent a t").unwrap();
        society_act("register-agent b t").unwrap();
        let res = society_act("edge 1 2 collaborator 0.8").unwrap();
        assert!(res.contains("社会边"));
    }

    #[test]
    fn society_act_negotiate() {
        init();
        society_act("register-agent a t").unwrap();
        society_act("register-agent b t").unwrap();
        let res = society_act("negotiate 1 2 gpu share").unwrap();
        assert!(res.contains("协商"));
    }

    #[test]
    fn society_act_reputation() {
        init();
        society_act("register-agent a t").unwrap();
        let res = society_act("rate 1 true").unwrap();
        assert!(res.contains("评价"));
        let res = society_act("reputation 1").unwrap();
        assert!(res.contains("信誉"));
    }

    #[test]
    fn society_act_conflict() {
        init();
        society_act("register-agent a t").unwrap();
        society_act("register-agent b t").unwrap();
        let res = society_act("conflict resource 1 2 争用资源").unwrap();
        assert!(res.contains("冲突"));
        let res = society_act("resolve-conflict 1 协商解决").unwrap();
        assert!(res.contains("已解决"));
    }

    #[test]
    fn society_act_status() {
        init();
        let res = society_act("status").unwrap();
        assert!(res.contains("society:"));
    }

    #[test]
    fn relation_types() {
        assert_eq!(RelationType::Peer as u32, 0);
        assert_eq!(RelationType::Competitor as u32, 4);
    }

    #[test]
    fn message_types() {
        assert_eq!(MessageType::Request as u32, 0);
        assert_eq!(MessageType::Query as u32, 5);
    }

    #[test]
    fn conversation_statuses() {
        assert_eq!(ConversationStatus::Active as u32, 0);
        assert_eq!(ConversationStatus::Aborted as u32, 3);
    }

    #[test]
    fn negotiation_statuses() {
        assert_eq!(NegotiationStatus::Proposed as u32, 0);
        assert_eq!(NegotiationStatus::Aborted as u32, 4);
    }

    #[test]
    fn conflict_types() {
        assert_eq!(ConflictType::Resource as u32, 0);
        assert_eq!(ConflictType::Value as u32, 3);
    }
}

// =====================================================================================
// 群体智能 (SwarmIntelligence)
// =====================================================================================

/// 群体行为类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwarmBehavior {
    Foraging,    // 觅食
    Clustering,  // 聚类
    Migrating,   // 迁徙
    Defending,   // 防御
}

/// 群体智能：多智能体协同搜索和优化。
pub struct SwarmIntelligence {
    pub behavior: SwarmBehavior,
    pub agents: Vec<u64>,
    pub pheromone_map: BTreeMap<String, f32>,
    pub iteration: u64,
    pub best_solution: Option<String>,
    pub best_fitness: f32,
}

impl SwarmIntelligence {
    pub fn new() -> Self {
        Self {
            behavior: SwarmBehavior::Foraging,
            agents: Vec::new(),
            pheromone_map: BTreeMap::new(),
            iteration: 0,
            best_solution: None,
            best_fitness: 0.0,
        }
    }

    pub fn set_behavior(&mut self, behavior: SwarmBehavior) {
        self.behavior = behavior;
    }

    pub fn add_agent(&mut self, agent_id: u64) {
        self.agents.push(agent_id);
    }

    pub fn update_pheromone(&mut self, path: &str, strength: f32) {
        let current = self.pheromone_map.get(path).copied().unwrap_or(0.0);
        self.pheromone_map.insert(path.to_string(), (current + strength).clamp(0.0, 1.0));
    }

    pub fn evaporate_pheromones(&mut self, rate: f32) {
        let rate = rate.clamp(0.0, 1.0);
        for v in self.pheromone_map.values_mut() {
            *v *= 1.0 - rate;
        }
    }

    pub fn iterate(&mut self) {
        self.iteration += 1;
        self.evaporate_pheromones(0.1);
    }

    pub fn record_solution(&mut self, solution: &str, fitness: f32) {
        if fitness > self.best_fitness {
            self.best_fitness = fitness;
            self.best_solution = Some(solution.to_string());
        }
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub fn pheromone_count(&self) -> usize {
        self.pheromone_map.len()
    }
}

// =====================================================================================
// 领导力选举 (LeaderElection)
// =====================================================================================

/// 选举状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectionState {
    Idle,
    Candidate,
    Leader,
    Follower,
}

/// 领导力选举：选举群体领导者。
pub struct LeaderElection {
    pub agents: BTreeMap<u64, ElectionState>,
    pub votes: BTreeMap<u64, u64>,    // candidate_id -> vote_count
    pub current_leader: Option<u64>,
    pub term: u64,
    pub next_vote_id: u64,
}

impl LeaderElection {
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            votes: BTreeMap::new(),
            current_leader: None,
            term: 0,
            next_vote_id: 1,
        }
    }

    pub fn add_agent(&mut self, agent_id: u64) {
        self.agents.insert(agent_id, ElectionState::Idle);
    }

    pub fn nominate(&mut self, agent_id: u64) -> Result<(), String> {
        if self.agents.contains_key(&agent_id) {
            self.agents.insert(agent_id, ElectionState::Candidate);
            self.term += 1;
            Ok(())
        } else {
            Err("代理不存在".into())
        }
    }

    pub fn vote(&mut self, voter_id: u64, candidate_id: u64) -> Result<(), String> {
        if !self.agents.contains_key(&voter_id) {
            return Err("选民不存在".into());
        }
        if self.agents.get(&candidate_id) != Some(&ElectionState::Candidate) {
            return Err("候选人无效".into());
        }
        *self.votes.entry(candidate_id).or_insert(0) += 1;
        self.next_vote_id += 1;
        Ok(())
    }

    pub fn finalize(&mut self) -> Option<u64> {
        let mut best = None;
        let mut max_votes = 0;
        for (candidate, votes) in &self.votes {
            if *votes > max_votes {
                max_votes = *votes;
                best = Some(*candidate);
            }
        }
        if let Some(leader) = best {
            self.current_leader = Some(leader);
            for state in self.agents.values_mut() {
                *state = if *leader == self.agents.keys().next().copied().unwrap_or(0) {
                    ElectionState::Leader
                } else {
                    ElectionState::Follower
                };
            }
        }
        self.current_leader
    }

    pub fn leader(&self) -> Option<u64> {
        self.current_leader
    }

    pub fn vote_count(&self) -> usize {
        self.votes.len()
    }
}

// =====================================================================================
// 共识机制 (Consensus)
// =====================================================================================

/// 共识算法类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsensusAlgorithm {
    Majority,   // 多数决
    Unanimous,  // 一致同意
    Weighted,   // 加权决策
}

/// 共识提案。
#[derive(Clone, Debug)]
pub struct ConsensusProposal {
    pub id: u64,
    pub content: String,
    pub proposer: u64,
    pub votes_for: u64,
    pub votes_against: u64,
    pub decided: bool,
    pub result: bool,
}

/// 共识机制：群体决策。
pub struct ConsensusMechanism {
    pub algorithm: ConsensusAlgorithm,
    pub proposals: BTreeMap<u64, ConsensusProposal>,
    pub next_id: u64,
    pub total_voters: u64,
}

impl ConsensusMechanism {
    pub fn new() -> Self {
        Self {
            algorithm: ConsensusAlgorithm::Majority,
            proposals: BTreeMap::new(),
            next_id: 1,
            total_voters: 1,
        }
    }

    pub fn set_algorithm(&mut self, algorithm: ConsensusAlgorithm) {
        self.algorithm = algorithm;
    }

    pub fn set_total_voters(&mut self, count: u64) {
        self.total_voters = count;
    }

    pub fn propose(&mut self, proposer: u64, content: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.proposals.insert(id, ConsensusProposal {
            id,
            content: content.to_string(),
            proposer,
            votes_for: 0,
            votes_against: 0,
            decided: false,
            result: false,
        });
        id
    }

    pub fn vote_for(&mut self, proposal_id: u64) -> Result<(), String> {
        let proposal = self.proposals.get_mut(&proposal_id).ok_or("提案不存在")?;
        proposal.votes_for += 1;
        Ok(())
    }

    pub fn vote_against(&mut self, proposal_id: u64) -> Result<(), String> {
        let proposal = self.proposals.get_mut(&proposal_id).ok_or("提案不存在")?;
        proposal.votes_against += 1;
        Ok(())
    }

    pub fn decide(&mut self, proposal_id: u64) -> Result<bool, String> {
        let proposal = self.proposals.get_mut(&proposal_id).ok_or("提案不存在")?;
        if proposal.decided {
            return Ok(proposal.result);
        }
        let total = self.total_voters;
        let result = match self.algorithm {
            ConsensusAlgorithm::Majority => proposal.votes_for > total / 2,
            ConsensusAlgorithm::Unanimous => proposal.votes_for == total && proposal.votes_against == 0,
            ConsensusAlgorithm::Weighted => proposal.votes_for > proposal.votes_against,
        };
        proposal.decided = true;
        proposal.result = result;
        Ok(result)
    }

    pub fn proposal_count(&self) -> usize {
        self.proposals.len()
    }

    pub fn decided_count(&self) -> usize {
        self.proposals.values().filter(|p| p.decided).count()
    }
}

// =====================================================================================
// 声誉传播 (ReputationPropagation)
// =====================================================================================

/// 声誉变化原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepChangeReason {
    PositiveInteraction,
    NegativeInteraction,
    Endorsement,
    Condemnation,
}

/// 声誉传播：声誉在社会网络中的传播。
pub struct ReputationPropagation {
    pub reputations: BTreeMap<u64, f32>,
    pub trust_network: BTreeMap<(u64, u64), f32>, // (from, to) -> trust
    pub history: Vec<(u64, f32, RepChangeReason, u128)>,
}

impl ReputationPropagation {
    pub fn new() -> Self {
        Self {
            reputations: BTreeMap::new(),
            trust_network: BTreeMap::new(),
            history: Vec::new(),
        }
    }

    pub fn set_reputation(&mut self, agent_id: u64, score: f32) {
        self.reputations.insert(agent_id, score.clamp(0.0, 1.0));
    }

    pub fn set_trust(&mut self, from: u64, to: u64, trust: f32) {
        self.trust_network.insert((from, to), trust.clamp(0.0, 1.0));
    }

    pub fn propagate(&mut self, source: u64, change: f32, reason: RepChangeReason) {
        let current = self.reputations.get(&source).copied().unwrap_or(0.5);
        let new_score = (current + change).clamp(0.0, 1.0);
        self.reputations.insert(source, new_score);
        self.history.push((source, change, reason, crate::ai_os::native::now_ns()));
    }

    pub fn get_reputation(&self, agent_id: u64) -> f32 {
        self.reputations.get(&agent_id).copied().unwrap_or(0.5)
    }

    pub fn get_trust(&self, from: u64, to: u64) -> f32 {
        self.trust_network.get(&(from, to)).copied().unwrap_or(0.0)
    }

    pub fn history_count(&self) -> usize {
        self.history.len()
    }
}

// =====================================================================================
// 社会网络分析 (SocialNetworkAnalysis)
// =====================================================================================

/// 中心性类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CentralityType {
    Degree,      // 度中心性
    Betweenness, // 介数中心性
    Closeness,   // 接近中心性
}

/// 社会网络分析：分析智能体社会结构。
pub struct SocialNetworkAnalysis {
    pub agents: BTreeSet<u64>,
    pub edges: BTreeMap<(u64, u64), f32>,
}

impl SocialNetworkAnalysis {
    pub fn new() -> Self {
        Self {
            agents: BTreeSet::new(),
            edges: BTreeMap::new(),
        }
    }

    pub fn add_agent(&mut self, agent_id: u64) {
        self.agents.insert(agent_id);
    }

    pub fn add_edge(&mut self, from: u64, to: u64, weight: f32) {
        self.agents.insert(from);
        self.agents.insert(to);
        self.edges.insert((from, to), weight.clamp(0.0, 1.0));
    }

    pub fn degree_centrality(&self, agent_id: u64) -> f32 {
        let degree = self.edges.iter().filter(|((a, b), _)| *a == agent_id || *b == agent_id).count();
        let max_degree = self.agents.len().saturating_sub(1);
        if max_degree == 0 {
            0.0
        } else {
            degree as f32 / max_degree as f32
        }
    }

    pub fn edge_weight(&self, from: u64, to: u64) -> f32 {
        self.edges.get(&(from, to)).copied().unwrap_or(0.0)
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

// =====================================================================================
// 社会扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn swarm_intelligence() {
        let mut swarm = SwarmIntelligence::new();
        swarm.set_behavior(SwarmBehavior::Foraging);
        swarm.add_agent(1);
        swarm.add_agent(2);
        swarm.add_agent(3);
        assert_eq!(swarm.agent_count(), 3);
        swarm.update_pheromone("path_a", 0.5);
        swarm.update_pheromone("path_b", 0.3);
        assert_eq!(swarm.pheromone_count(), 2);
        swarm.iterate();
        assert_eq!(swarm.iteration, 1);
        swarm.record_solution("solution_1", 0.85);
        assert_eq!(swarm.best_fitness, 0.85);
        assert_eq!(swarm.best_solution, Some("solution_1".to_string()));
    }

    #[test]
    fn leader_election() {
        let mut le = LeaderElection::new();
        le.add_agent(1);
        le.add_agent(2);
        le.add_agent(3);
        le.nominate(1).unwrap();
        le.nominate(2).unwrap();
        le.vote(1, 1).unwrap();
        le.vote(2, 2).unwrap();
        le.vote(3, 2).unwrap();
        let leader = le.finalize().unwrap();
        assert_eq!(leader, 2);
        assert_eq!(le.leader(), Some(2));
        assert_eq!(le.vote_count(), 2);
    }

    #[test]
    fn consensus_mechanism() {
        let mut cm = ConsensusMechanism::new();
        cm.set_algorithm(ConsensusAlgorithm::Majority);
        cm.set_total_voters(3);
        let pid = cm.propose(1, "提案A").unwrap();
        assert_eq!(pid, 1);
        cm.vote_for(pid).unwrap();
        cm.vote_for(pid).unwrap();
        let result = cm.decide(pid).unwrap();
        assert!(result);
        assert_eq!(cm.proposal_count(), 1);
        assert_eq!(cm.decided_count(), 1);
    }

    #[test]
    fn consensus_unanimous() {
        let mut cm = ConsensusMechanism::new();
        cm.set_algorithm(ConsensusAlgorithm::Unanimous);
        cm.set_total_voters(2);
        let pid = cm.propose(1, "提案B").unwrap();
        cm.vote_for(pid).unwrap();
        let result = cm.decide(pid).unwrap();
        assert!(!result);
    }

    #[test]
    fn reputation_propagation() {
        let mut rp = ReputationPropagation::new();
        rp.set_reputation(1, 0.5);
        rp.set_trust(1, 2, 0.8);
        rp.propagate(1, 0.2, RepChangeReason::PositiveInteraction);
        assert!((rp.get_reputation(1) - 0.7).abs() < 1e-6);
        assert!((rp.get_trust(1, 2) - 0.8).abs() < 1e-6);
        assert_eq!(rp.history_count(), 1);
    }

    #[test]
    fn social_network_analysis() {
        let mut sna = SocialNetworkAnalysis::new();
        sna.add_agent(1);
        sna.add_agent(2);
        sna.add_agent(3);
        sna.add_edge(1, 2, 0.8);
        sna.add_edge(2, 3, 0.6);
        assert_eq!(sna.agent_count(), 3);
        assert_eq!(sna.edge_count(), 2);
        let centrality = sna.degree_centrality(2);
        assert!((centrality - 1.0).abs() < 1e-6);
        assert!((sna.edge_weight(1, 2) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn swarm_behaviors() {
        assert_eq!(SwarmBehavior::Foraging as u32, 0);
        assert_eq!(SwarmBehavior::Defending as u32, 3);
    }

    #[test]
    fn election_states() {
        assert_eq!(ElectionState::Idle as u32, 0);
        assert_eq!(ElectionState::Leader as u32, 3);
    }

    #[test]
    fn consensus_algorithms() {
        assert_eq!(ConsensusAlgorithm::Majority as u32, 0);
        assert_eq!(ConsensusAlgorithm::Weighted as u32, 2);
    }

    #[test]
    fn rep_change_reasons() {
        assert_eq!(RepChangeReason::PositiveInteraction as u32, 0);
        assert_eq!(RepChangeReason::Condemnation as u32, 3);
    }

    #[test]
    fn centrality_types() {
        assert_eq!(CentralityType::Degree as u32, 0);
        assert_eq!(CentralityType::Closeness as u32, 2);
    }
}
