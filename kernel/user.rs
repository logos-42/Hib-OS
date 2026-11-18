//! 用户 / 身份 / 权限模型 —— AI 原生的「身份意识」。
//!
//! 不复制 Linux 的 /etc/passwd，而是把「当前以谁的身份行动」建模成
//! 可感知 (sense) / 可切换 (act) 的通道 `user/db`。
//!
//! 身份使用 DID（去中心化标识符）而非数字 uid，超级用户（root）以
//! DID `did:hib:root` 确认，而非 uid=0 硬编码。

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use crate::caps::ALL_CAPS;

/// 超级用户（root）的 DID；持有全部能力。
pub const ROOT_DID: &str = "did:hib:root";

#[derive(Clone, Debug)]
pub struct User {
    pub did: String,
    pub name: String,
    pub groups: Vec<String>,
}

pub struct UserDb {
    pub users: Vec<User>,
    pub current_did: String,
}

impl UserDb {
    fn new() -> Self {
        Self {
            users: vec![
                User {
                    did: ROOT_DID.to_string(),
                    name: "root".into(),
                    groups: vec!["root".into()],
                },
                User {
                    did: "did:hib:hib".to_string(),
                    name: "hib".into(),
                    groups: vec!["users".into()],
                },
                User {
                    did: "did:hib:ai".to_string(),
                    name: "ai".into(),
                    groups: vec!["users".into(), "ai".into()],
                },
            ],
            // 默认以「ai」身份行动 —— 一切皆模型的意图。
            current_did: "did:hib:ai".to_string(),
        }
    }

    pub fn list(&self) -> String {
        let mut out = String::new();
        for u in &self.users {
            let marker = if u.did == self.current_did { "*" } else { " " };
            out.push_str(&format!(
                "{} did={:<20} {:<8} groups=[{}]\n",
                marker,
                u.did,
                u.name,
                u.groups.join(",")
            ));
        }
        out.trim_end().to_string()
    }

    pub fn whoami(&self) -> String {
        match self.users.iter().find(|u| u.did == self.current_did) {
            Some(u) => format!(
                "did={} name={} groups=[{}] caps={}",
                u.did,
                u.name,
                u.groups.join(","),
                self.current_caps().iter().cloned().collect::<Vec<_>>().join(",")
            ),
            None => format!("did={} (unknown)", self.current_did),
        }
    }

    /// 给定 did 能持有的 capability 集合。
    ///
    /// root (did:hib:root) 持有全部能力；其它用户默认只持有 `CAP_SYS_ADMIN`
    ///（演示环境下可自行提权），其余能力需通过 `sec/caps` 显式授予。
    /// 这是「root 以能力集合而非 did 硬编码」的具体体现。
    pub fn capabilities_of(did: &str) -> BTreeSet<String> {
        if did == ROOT_DID {
            return ALL_CAPS.iter().map(|s| s.to_string()).collect();
        }
        match did {
            "did:hib:hib" | "did:hib:ai" => {
                ["CAP_SYS_ADMIN"].iter().map(|s| s.to_string()).collect()
            }
            _ => BTreeSet::new(),
        }
    }

    /// 当前会话身份持有的 capability 集合。
    pub fn current_caps(&self) -> BTreeSet<String> {
        Self::capabilities_of(&self.current_did)
    }

    pub fn adduser(&mut self, name: &str) -> Result<String, String> {
        if self.users.iter().any(|u| u.name == name) {
            return Err(format!("用户 {} 已存在", name));
        }
        let did = format!("did:hib:{}", name);
        if self.users.iter().any(|u| u.did == did) {
            return Err(format!("DID {} 已存在", did));
        }
        self.users.push(User {
            did: did.clone(),
            name: name.to_string(),
            groups: vec!["users".into()],
        });
        Ok(did)
    }

    pub fn su(&mut self, who: &str) -> Result<String, String> {
        // 支持直接以 DID 或用户名切换身份。
        let target = if who.starts_with("did:") {
            self.users.iter().find(|u| u.did == who).map(|u| u.did.clone())
        } else {
            self.users.iter().find(|u| u.name == who).map(|u| u.did.clone())
        };
        match target {
            Some(did) => {
                let name = self
                    .users
                    .iter()
                    .find(|u| u.did == did)
                    .map(|u| u.name.clone())
                    .unwrap_or_default();
                self.current_did = did.clone();
                Ok(format!("已切换到 {} ({})", name, did))
            }
            None => Err(format!("用户 {} 不存在", who)),
        }
    }

    pub fn sudo(&mut self) -> String {
        if self.current_did == ROOT_DID {
            return "已在 root 身份下".to_string();
        }
        // root 以能力集合表示：提权需持有 CAP_SYS_ADMIN，而非凭 did 硬编码。
        if !self.current_caps().contains("CAP_SYS_ADMIN") {
            return "缺少 CAP_SYS_ADMIN 能力，无法提权到 root".to_string();
        }
        self.current_did = ROOT_DID.to_string();
        "已提权到 root (capability 集合: 全部)".to_string()
    }
}

static USERDB: OnceLock<Mutex<UserDb>> = OnceLock::new();

/// 全局用户库 / 当前会话身份。
pub fn userdb() -> &'static Mutex<UserDb> {
    USERDB.get_or_init(|| Mutex::new(UserDb::new()))
}
