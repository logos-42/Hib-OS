//! 用户空间 / 超级开发者空间 —— `sec/acl` 与 `realm/current`。
//!
//! Linux 用 uid/euid 区分用户态与 root；Hib 用 **Realm + 通道 ACL**：
//!
//! * `user` —— 普通意识空间。默认只读，写任何通道都必须被显式授权；
//! * `dev`  —— 超级开发者空间（root）。全通道可写，可 force 租约、加载模块。
//!
//! 校验下沉到 `sense::act()` 单一入口，一次行动前问一句：
//! "当前意识体，有没有权力对这条通道做这个动作？"
//! 这与"先跑起来再被内核拒绝"的 Linux 思路相反 —— **意图先过权限**。
//!
//! * `sense sec/acl` / `sense realm/current`
//! * `act sec/acl realm <user|dev>`
//! * `act sec/acl allow|deny <通道前缀> [r|w]`
//! * `act sec/acl check <通道> <r|w>`

use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realm {
    User,
    Dev,
}

impl Realm {
    pub fn name(self) -> &'static str {
        match self {
            Realm::User => "user",
            Realm::Dev => "dev",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "user" | "userspace" => Some(Realm::User),
            "dev" | "root" | "superdev" => Some(Realm::Dev),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub realm: Realm,
    pub pattern: String,
    pub verb: String,
    pub allow: bool,
}

pub struct Acl {
    /// 默认是超级开发者空间，保证既有脚本与测试不受影响。
    pub current: Realm,
    pub rules: Vec<Rule>,
    pub denials: u64,
}

impl Acl {
    fn new() -> Self {
        Self {
            current: Realm::Dev,
            // user 空间唯一默认可写的通道：ACL 自身（申请授权 / 切回 dev 空间），
            // 否则一旦切入 user 就无法再管理权限，形成死锁。
            rules: vec![Rule {
                realm: Realm::User,
                pattern: "sec/acl".to_string(),
                verb: "w".to_string(),
                allow: true,
            }],
            denials: 0,
        }
    }

    /// 校验一次行动意图；`verb` 为 `r` 或 `w`。
    pub fn check(&mut self, channel: &str, verb: &str) -> Result<String, String> {
        if self.current == Realm::Dev {
            return Ok(format!("{} 空间: {} {} 允许", self.current.name(), verb, channel));
        }
        // 最长前缀匹配；deny 优先于 allow。
        let mut best: Option<&Rule> = None;
        for r in &self.rules {
            if r.realm != self.current {
                continue;
            }
            if !(r.verb == verb || r.verb == "rw" || r.verb == "*") {
                continue;
            }
            if !channel.starts_with(&r.pattern) {
                continue;
            }
            if best.map(|b| r.pattern.len() > b.pattern.len()).unwrap_or(true) {
                best = Some(r);
            }
        }
        match best {
            Some(r) if r.allow => Ok(format!("ACL 显式放行: {} {}", verb, channel)),
            Some(_r) => {
                self.denials += 1;
                Err(format!("ACL 拒绝: {} 空间不得 {} {}", self.current.name(), verb, channel))
            }
            None if verb == "r" => Ok(format!("{} 空间: 默认可读 {}", self.current.name(), channel)),
            None => {
                self.denials += 1;
                Err(format!(
                    "ACL 拒绝: {} 空间默认不可写 {} (用 `act sec/acl allow {}` 授权)",
                    self.current.name(), channel, channel
                ))
            }
        }
    }

    pub fn set_realm(&mut self, r: Realm) -> String {
        self.current = r;
        format!("已切入 {} 空间", r.name())
    }

    pub fn add_rule(&mut self, pattern: &str, verb: &str, allow: bool) -> Result<String, String> {
        if !matches!(verb, "r" | "w" | "rw" | "*") {
            return Err("动作只能是 r|w|rw|*".to_string());
        }
        if pattern.is_empty() {
            return Err("通道前缀不能为空".to_string());
        }
        let realm = self.current;
        self.rules.retain(|x| !(x.realm == realm && x.pattern == pattern && x.verb == verb));
        self.rules.push(Rule {
            realm,
            pattern: pattern.to_string(),
            verb: verb.to_string(),
            allow,
        });
        Ok(format!(
            "{} 空间: {} {} {}",
            realm.name(),
            if allow { "allow" } else { "deny" },
            verb,
            pattern
        ))
    }

    pub fn status(&self) -> String {
        let mut out = format!(
            "realm={} (user=用户空间 / dev=超级开发者空间)\nrules={} denials={}",
            self.current.name(),
            self.rules.len(),
            self.denials
        );
        if self.rules.is_empty() {
            out.push_str("\n  (无规则: dev 全放行; user 默认只读)");
        }
        for r in &self.rules {
            out.push_str(&format!(
                "\n  {:<5} {:<4} {:<3} {}",
                r.realm.name(),
                if r.allow { "allow" } else { "deny" },
                r.verb,
                r.pattern
            ));
        }
        out
    }
}

static ACL: OnceLock<Mutex<Acl>> = OnceLock::new();

pub fn acl() -> &'static Mutex<Acl> {
    ACL.get_or_init(|| Mutex::new(Acl::new()))
}

/// 供 `sense::act` 在写入前统一调用。
pub fn check_act(channel: &str) -> Result<(), String> {
    acl().lock().unwrap().check(channel, "w").map(|_| ())
}

/// 供 `sense::probe` 在读取前统一调用。
pub fn check_sense(channel: &str) -> Result<(), String> {
    acl().lock().unwrap().check(channel, "r").map(|_| ())
}

pub fn set_realm(s: &str) -> Result<String, String> {
    let r = Realm::parse(s).ok_or_else(|| "空间只能是 user|dev".to_string())?;
    Ok(acl().lock().unwrap().set_realm(r))
}

pub fn realm_status() -> String {
    acl().lock().unwrap().status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_realm_allows_everything() {
        let mut a = Acl::new();
        assert_eq!(a.current, Realm::Dev);
        assert!(a.check("gpu/vram", "w").is_ok());
        assert!(a.check("sec/acl", "w").is_ok());
    }

    #[test]
    fn user_realm_denies_writes_until_authorized() {
        let mut a = Acl::new();
        a.set_realm(Realm::User);
        assert!(a.check("dev/timer", "r").is_ok(), "读应默认放行");
        let e = a.check("dev/timer", "w").unwrap_err();
        assert!(e.contains("ACL 拒绝"), "{}", e);
        assert_eq!(a.denials, 1);
        a.add_rule("dev/", "w", true).unwrap();
        assert!(a.check("dev/timer", "w").is_ok());
    }

    #[test]
    fn deny_wins_and_longest_prefix_wins() {
        let mut a = Acl::new();
        a.set_realm(Realm::User);
        a.add_rule("gpu/", "w", true).unwrap();
        a.add_rule("gpu/vram", "w", false).unwrap();
        assert!(a.check("gpu/scheduler", "w").is_ok());
        assert!(a.check("gpu/vram", "w").is_err());
    }

    #[test]
    fn realm_parsing_and_rule_validation() {
        assert_eq!(Realm::parse("root"), Some(Realm::Dev));
        assert_eq!(Realm::parse("userspace"), Some(Realm::User));
        assert!(Realm::parse("nope").is_none());
        let mut a = Acl::new();
        assert!(a.add_rule("x", "bad", true).is_err());
        assert!(a.add_rule("", "w", true).is_err());
    }
}
