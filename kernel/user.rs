//! 用户 / 身份 / 权限模型 —— AI 原生的「身份意识」。
//!
//! 不复制 Linux 的 /etc/passwd，而是把「当前以谁的身份行动」建模成
//! 可感知 (sense) / 可切换 (act) 的通道 `user://db`。

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use crate::caps::ALL_CAPS;

#[derive(Clone, Debug)]
pub struct User {
    pub uid: u32,
    pub name: String,
    pub groups: Vec<String>,
}

pub struct UserDb {
    pub users: Vec<User>,
    pub current_uid: u32,
}

impl UserDb {
    fn new() -> Self {
        Self {
            users: vec![
                User { uid: 0, name: "root".into(), groups: vec!["root".into()] },
                User { uid: 1000, name: "hib".into(), groups: vec!["users".into()] },
                User {
                    uid: 1001,
                    name: "ai".into(),
                    groups: vec!["users".into(), "ai".into()],
                },
            ],
            // 默认以「ai」身份行动 —— 一切皆模型的意图。
            current_uid: 1001,
        }
    }

    pub fn list(&self) -> String {
        let mut out = String::new();
        for u in &self.users {
            let marker = if u.uid == self.current_uid { "*" } else { " " };
            out.push_str(&format!(
                "{} uid={:<4} {:<8} groups=[{}]\n",
                marker,
                u.uid,
                u.name,
                u.groups.join(",")
            ));
        }
        out.trim_end().to_string()
    }

    pub fn whoami(&self) -> String {
        match self.users.iter().find(|u| u.uid == self.current_uid) {
            Some(u) => format!(
                "uid={} name={} groups=[{}] caps={}",
                u.uid,
                u.name,
                u.groups.join(","),
                self.current_caps().iter().cloned().collect::<Vec<_>>().join(",")
            ),
            None => format!("uid={} (unknown)", self.current_uid),
        }
    }

    /// 给定 uid 能持有的 capability 集合。
    ///
    /// root (uid=0) 持有全部能力；其它用户默认只持有 `CAP_SYS_ADMIN`
    ///（演示环境下可自行提权），其余能力需通过 `sec://caps` 显式授予。
    /// 这是「root 以能力集合而非 uid=0 全权」的具体体现。
    pub fn capabilities_of(uid: u32) -> BTreeSet<String> {
        if uid == 0 {
            return ALL_CAPS.iter().map(|s| s.to_string()).collect();
        }
        match uid {
            1000 | 1001 => ["CAP_SYS_ADMIN"].iter().map(|s| s.to_string()).collect(),
            _ => BTreeSet::new(),
        }
    }

    /// 当前会话身份持有的 capability 集合。
    pub fn current_caps(&self) -> BTreeSet<String> {
        Self::capabilities_of(self.current_uid)
    }

    pub fn adduser(&mut self, name: &str) -> Result<u32, String> {
        if self.users.iter().any(|u| u.name == name) {
            return Err(format!("用户 {} 已存在", name));
        }
        let uid = self.users.iter().map(|u| u.uid).max().unwrap_or(1000) + 1;
        self.users.push(User { uid, name: name.to_string(), groups: vec!["users".into()] });
        Ok(uid)
    }

    pub fn su(&mut self, who: &str) -> Result<String, String> {
        let target = who
            .parse::<u32>()
            .ok()
            .or_else(|| self.users.iter().find(|u| u.name == who).map(|u| u.uid));
        match target.and_then(|uid| {
            self.users.iter().find(|u| u.uid == uid).map(|u| (uid, u.name.clone()))
        }) {
            Some((uid, name)) => {
                self.current_uid = uid;
                Ok(format!("已切换到 {} (uid={})", name, uid))
            }
            None => Err(format!("用户 {} 不存在", who)),
        }
    }

    pub fn sudo(&mut self) -> String {
        if self.current_uid == 0 {
            return "已在 root 身份下".to_string();
        }
        // root 以能力集合表示：提权需持有 CAP_SYS_ADMIN，而非凭 uid 硬编码。
        if !self.current_caps().contains("CAP_SYS_ADMIN") {
            return "缺少 CAP_SYS_ADMIN 能力，无法提权到 root".to_string();
        }
        self.current_uid = 0;
        "已提权到 root (capability 集合: 全部)".to_string()
    }
}

static USERDB: OnceLock<Mutex<UserDb>> = OnceLock::new();

/// 全局用户库 / 当前会话身份。
pub fn userdb() -> &'static Mutex<UserDb> {
    USERDB.get_or_init(|| Mutex::new(UserDb::new()))
}
