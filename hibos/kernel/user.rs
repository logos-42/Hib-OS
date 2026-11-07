//! 用户 / 身份 / 权限模型 —— AI 原生的「身份意识」。
//!
//! 不复制 Linux 的 /etc/passwd，而是把「当前以谁的身份行动」建模成
//! 可感知 (sense) / 可切换 (act) 的通道 `user://db`。

use std::sync::{Mutex, OnceLock};

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
            Some(u) => format!("uid={} name={} groups=[{}]", u.uid, u.name, u.groups.join(",")),
            None => format!("uid={} (unknown)", self.current_uid),
        }
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
            "已在 root 身份下".to_string()
        } else {
            self.current_uid = 0;
            "已提权到 root (uid=0)".to_string()
        }
    }
}

static USERDB: OnceLock<Mutex<UserDb>> = OnceLock::new();

/// 全局用户库 / 当前会话身份。
pub fn userdb() -> &'static Mutex<UserDb> {
    USERDB.get_or_init(|| Mutex::new(UserDb::new()))
}
