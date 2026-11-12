//! 外部世界网关 —— AI 原生的「出站策略与流量闸门」。
//!
//! 所有离开内核的请求（目前是 `browser://gateway` 抓取页面）都要先过这里：
//! 命中规则的按规则放行 / 拦截，未命中的按默认策略处理，
//! 每一次判定都会累加到 `allowed` / `blocked` 计数。
//!
//! * `sense gateway://net`
//! * `act gateway://net upstream <direct|http(s)://...>`
//! * `act gateway://net policy <allow|deny>`
//! * `act gateway://net allow|deny <域名>` / `del <域名>`
//! * `act gateway://net check <域名>`

use std::sync::{Mutex, OnceLock};

/// 一条出站规则：域名后缀 -> 是否放行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub domain: String,
    pub allow: bool,
}

/// 网关状态。
pub struct Gateway {
    /// 上游出口：`direct` 表示直连，否则为代理地址。
    pub upstream: String,
    /// 默认策略：allow / deny。
    pub policy: String,
    pub rules: Vec<Rule>,
    pub allowed: u64,
    pub blocked: u64,
    pub bytes: u64,
    pub last: String,
}

impl Gateway {
    fn new() -> Self {
        Self {
            upstream: "direct".to_string(),
            policy: "allow".to_string(),
            rules: vec![Rule {
                domain: "localhost".to_string(),
                allow: true,
            }],
            allowed: 0,
            blocked: 0,
            bytes: 0,
            last: "（尚无出站请求）".to_string(),
        }
    }

    /// 感知快照。
    pub fn status(&self) -> String {
        let rules = if self.rules.is_empty() {
            "  (无规则)".to_string()
        } else {
            self.rules
                .iter()
                .map(|r| {
                    format!(
                        "  {:<24} {}",
                        r.domain,
                        if r.allow { "allow" } else { "deny" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "upstream={} policy={}\nrules:\n{}\nallowed={} blocked={} bytes={}\nlast: {}",
            self.upstream, self.policy, rules, self.allowed, self.blocked, self.bytes, self.last
        )
    }

    /// 设置上游出口。
    pub fn set_upstream(&mut self, u: &str) -> Result<String, String> {
        let u = u.trim();
        if u != "direct" && !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err("上游必须是 direct 或 http(s):// 地址".to_string());
        }
        self.upstream = u.to_string();
        Ok(format!("网关上游设为 {}", u))
    }

    /// 设置默认策略。
    pub fn set_policy(&mut self, p: &str) -> Result<String, String> {
        let p = p.trim().to_lowercase();
        if p != "allow" && p != "deny" {
            return Err("策略只能是 allow|deny".to_string());
        }
        self.policy = p.clone();
        Ok(format!("默认出站策略: {}", p))
    }

    /// 添加 / 覆盖一条规则。
    pub fn add_rule(&mut self, domain: &str, allow: bool) -> Result<String, String> {
        let d = normalize_domain(domain)?;
        self.rules.retain(|r| r.domain != d);
        self.rules.push(Rule {
            domain: d.clone(),
            allow,
        });
        Ok(format!("规则 {} -> {}", d, if allow { "allow" } else { "deny" }))
    }

    /// 删除一条规则。
    pub fn del_rule(&mut self, domain: &str) -> Result<String, String> {
        let d = normalize_domain(domain)?;
        let before = self.rules.len();
        self.rules.retain(|r| r.domain != d);
        if self.rules.len() == before {
            Err(format!("无规则 {}", d))
        } else {
            Ok(format!("已删除规则 {}", d))
        }
    }

    /// 判定某主机是否允许出站：最长域名后缀规则优先，未命中则用默认策略。
    pub fn check(&self, host: &str) -> bool {
        let h = host.trim().to_lowercase();
        let h = h.split(':').next().unwrap_or("").to_string();
        let mut best: Option<(usize, bool)> = None;
        for r in &self.rules {
            let hit = h == r.domain || h.ends_with(&format!(".{}", r.domain));
            if !hit {
                continue;
            }
            let len = r.domain.len();
            if best.map(|(l, _)| len > l).unwrap_or(true) {
                best = Some((len, r.allow));
            }
        }
        best.map(|(_, allow)| allow)
            .unwrap_or(self.policy == "allow")
    }

    /// 记录一次出站判定。
    pub fn record(&mut self, host: &str, allowed: bool, bytes: usize) {
        if allowed {
            self.allowed += 1;
            self.bytes += bytes as u64;
        } else {
            self.blocked += 1;
        }
        self.last = format!(
            "{} {} ({}B)",
            if allowed { "allow" } else { "deny" },
            host,
            bytes
        );
    }
}

/// 归一化域名：去空白、去前导点、转小写。
fn normalize_domain(d: &str) -> Result<String, String> {
    let d = d.trim().trim_start_matches('.').to_lowercase();
    if d.is_empty() {
        return Err("域名不能为空".to_string());
    }
    Ok(d)
}

static GATEWAY: OnceLock<Mutex<Gateway>> = OnceLock::new();

/// 全局网关（`gateway://net` 的实体）。
pub fn gateway() -> &'static Mutex<Gateway> {
    GATEWAY.get_or_init(|| Mutex::new(Gateway::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gw() -> Gateway {
        Gateway::new()
    }

    #[test]
    fn default_allows_everything() {
        let g = gw();
        assert_eq!(g.policy, "allow");
        assert!(g.check("example.com"));
        assert!(g.check("localhost"));
        assert!(g.status().contains("upstream=direct"));
    }

    #[test]
    fn policy_deny_blocks_unlisted() {
        let mut g = gw();
        g.set_policy("deny").unwrap();
        assert!(!g.check("example.com"));
        assert!(g.check("localhost"), "白名单规则应仍然放行");
        assert!(g.set_policy("maybe").is_err());
    }

    #[test]
    fn longest_suffix_wins() {
        let mut g = gw();
        g.add_rule("example.com", false).unwrap();
        g.add_rule("api.example.com", true).unwrap();
        assert!(!g.check("www.example.com"));
        assert!(g.check("api.example.com"));
        assert!(!g.check("example.com"));
        // 不同域不受影响（默认 allow）
        assert!(g.check("other.org"));
    }

    #[test]
    fn rule_add_replace_delete() {
        let mut g = gw();
        g.add_rule(".example.com", true).unwrap();
        assert_eq!(g.rules.len(), 2);
        g.add_rule("example.com", false).unwrap();
        assert_eq!(g.rules.len(), 2, "同域名应覆盖而不是新增");
        assert!(!g.check("example.com"));
        assert!(g.del_rule("example.com").is_ok());
        assert!(g.del_rule("example.com").is_err());
        assert!(g.add_rule("  ", true).is_err());
    }

    #[test]
    fn upstream_and_counters() {
        let mut g = gw();
        assert!(g.set_upstream("direct").is_ok());
        assert!(g.set_upstream("socks5://x").is_err());
        g.set_upstream("http://10.0.0.1:8080").unwrap();
        g.record("example.com", true, 100);
        g.record("bad.example", false, 0);
        assert_eq!(g.allowed, 1);
        assert_eq!(g.blocked, 1);
        assert_eq!(g.bytes, 100);
        assert!(g.status().contains("blocked=1"));
    }

    #[test]
    fn host_with_port_and_subdomain() {
        let mut g = gw();
        // 默认 deny，才能看出规则是否精确命中
        g.set_policy("deny").unwrap();
        g.add_rule("example.com", true).unwrap();
        assert!(g.check("example.com:443"));
        assert!(g.check("a.b.example.com"));
        assert!(!g.check("notexample.com"), "后缀匹配必须按点边界");
    }
}
