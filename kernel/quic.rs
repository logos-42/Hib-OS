//! QUIC 去中心化通道 —— 基于 DID 身份的点对点通信。
//!
//! 每个 QUIC 会话以对端 DID 作为身份锚点，而非 IP/端口硬编码。
//! 超级用户 (did:hib:root) 可枚举全部会话；普通用户只能操作自己发起/监听的会话。
//!
//! 通道命名：
//!   quic/peers        感知：列出已知对端 DID 与会话摘要
//!   quic/sessions     感知：列出本地 QUIC 会话
//!   quic/listen       行动：在本地地址上监听，绑定当前 DID
//!   quic/connect      行动：向对端 DID 发起 QUIC 连接
//!   quic/send         行动：向指定会话发送数据
//!   quic/close        行动：关闭指定会话

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

use crate::user::{userdb, ROOT_DID};

/// QUIC 会话状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuicState {
    Listening,
    Connecting,
    Established,
    Closed,
}

/// QUIC 会话条目。
#[derive(Clone, Debug)]
pub struct QuicSession {
    pub id: u64,
    /// 本地 DID（会话拥有者）。
    pub local_did: String,
    /// 对端 DID（去中心化身份标识）。
    pub peer_did: Option<String>,
    pub local_addr: String,
    pub peer_addr: Option<String>,
    pub state: QuicState,
    pub sent_bytes: u64,
    pub recv_bytes: u64,
    pub created_ns: u64,
}

/// QUIC 管理器（内存模型，供演示与去中心化通道集成）。
pub struct QuicManager {
    sessions: BTreeMap<u64, QuicSession>,
    next_id: u64,
    peers: BTreeSet<String>,
    listen_addrs: BTreeSet<String>,
}

impl QuicManager {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next_id: 1,
            peers: BTreeSet::new(),
            listen_addrs: BTreeSet::new(),
        }
    }

    /// 当前会话身份的 DID。
    fn current_did(&self) -> String {
        userdb().lock().unwrap().current_did.clone()
    }

    /// 是否为超级用户。
    fn is_root(&self) -> bool {
        self.current_did() == ROOT_DID
    }

    /// 在地址上监听，绑定当前 DID。
    pub fn listen(&mut self, addr: &str) -> Result<u64, String> {
        let did = self.current_did();
        if self.listen_addrs.contains(addr) {
            return Err(format!("地址 {} 已被监听", addr));
        }
        self.listen_addrs.insert(addr.to_string());
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::monitor::get_time_ns();
        self.sessions.insert(
            id,
            QuicSession {
                id,
                local_did: did.clone(),
                peer_did: None,
                local_addr: addr.to_string(),
                peer_addr: None,
                state: QuicState::Listening,
                sent_bytes: 0,
                recv_bytes: 0,
                created_ns: now,
            },
        );
        Ok(id)
    }

    /// 向对端 DID 发起连接。
    pub fn connect(&mut self, peer_did: &str, addr: &str) -> Result<u64, String> {
        let did = self.current_did();
        // 对端必须是已知 DID。
        {
            let udb = userdb().lock().unwrap();
            if !udb.users.iter().any(|u| u.did == peer_did) {
                return Err(format!("未知对端 DID: {}", peer_did));
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = crate::monitor::get_time_ns();
        self.peers.insert(peer_did.to_string());
        self.sessions.insert(
            id,
            QuicSession {
                id,
                local_did: did,
                peer_did: Some(peer_did.to_string()),
                local_addr: addr.to_string(),
                peer_addr: Some(addr.to_string()),
                state: QuicState::Established,
                sent_bytes: 0,
                recv_bytes: 0,
                created_ns: now,
            },
        );
        Ok(id)
    }

    /// 向会话发送数据。
    pub fn send(&mut self, id: u64, data: &str) -> Result<u64, String> {
        let did = self.current_did();
        let is_root = self.is_root();
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| format!("会话 {} 不存在", id))?;
        if !is_root && session.local_did != did {
            return Err(format!("会话 {} 不属于当前身份 {}", id, did));
        }
        if session.state == QuicState::Closed {
            return Err(format!("会话 {} 已关闭", id));
        }
        session.sent_bytes += data.len() as u64;
        if let Some(peer) = &session.peer_did {
            self.peers.insert(peer.clone());
        }
        Ok(data.len() as u64)
    }

    /// 关闭会话。
    pub fn close(&mut self, id: u64) -> Result<(), String> {
        let did = self.current_did();
        let is_root = self.is_root();
        let session = self
            .sessions
            .get_mut(&id)
            .ok_or_else(|| format!("会话 {} 不存在", id))?;
        if !is_root && session.local_did != did {
            return Err(format!("会话 {} 不属于当前身份 {}", id, did));
        }
        session.state = QuicState::Closed;
        if let Some(addr) = session.local_addr.clone() {
            self.listen_addrs.remove(&addr);
        }
        Ok(())
    }

    /// 列出会话（超级用户可见全部，否则仅可见自身会话）。
    pub fn sessions(&self) -> Vec<QuicSession> {
        let did = self.current_did();
        if self.is_root() {
            return self.sessions.values().cloned().collect();
        }
        self.sessions
            .values()
            .filter(|s| s.local_did == did)
            .cloned()
            .collect()
    }

    /// 列出对端 peers。
    pub fn peers(&self) -> Vec<String> {
        self.peers.iter().cloned().collect()
    }

    /// 会话总数。
    pub fn len(&self) -> usize {
        self.sessions.len()
    }
}

static QUIC: OnceLock<Mutex<QuicManager>> = OnceLock::new();

/// 全局 QUIC 管理器单例。
pub fn quic() -> &'static Mutex<QuicManager> {
    QUIC.get_or_init(|| Mutex::new(QuicManager::new()))
}
