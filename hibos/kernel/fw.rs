//! 防火墙规则 (类 iptables/nftables) —— AI 原生的「安全边界」。

use std::sync::{Mutex, OnceLock};

pub struct Rule {
    pub id: u64,
    pub action: String,
    pub proto: String,
    pub port: String,
    pub src: String,
}

pub struct Firewall {
    pub next_id: u64,
    pub policy: String,
    pub rules: Vec<Rule>,
}

impl Firewall {
    fn new() -> Self {
        Self { next_id: 1, policy: "ACCEPT".to_string(), rules: Vec::new() }
    }

    pub fn list(&self) -> String {
        let mut out = format!("默认策略: {}\n规则 {} 条:\n", self.policy, self.rules.len());
        for r in &self.rules {
            out.push_str(&format!(
                "  #{} {} proto={} port={} src={}\n",
                r.id, r.action, r.proto, r.port, r.src
            ));
        }
        out.trim_end().to_string()
    }

    pub fn add(&mut self, action: &str, proto: &str, port: &str, src: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.rules.push(Rule {
            id,
            action: action.to_string(),
            proto: proto.to_string(),
            port: port.to_string(),
            src: src.to_string(),
        });
        format!("已添加规则 #{} {} proto={} port={} src={}", id, action, proto, port, src)
    }

    pub fn del(&mut self, id: u64) -> Result<String, String> {
        match self.rules.iter().position(|r| r.id == id) {
            Some(i) => {
                self.rules.remove(i);
                Ok(format!("已删除规则 #{}", id))
            }
            None => Err(format!("规则 #{} 不存在", id)),
        }
    }

    pub fn set_policy(&mut self, p: &str) -> Result<String, String> {
        let p = p.to_uppercase();
        if p != "ACCEPT" && p != "DROP" {
            return Err("策略只能是 ACCEPT 或 DROP".to_string());
        }
        self.policy = p.clone();
        Ok(format!("默认策略已设为 {}", p))
    }
}

static FW: OnceLock<Mutex<Firewall>> = OnceLock::new();

pub fn firewall() -> &'static Mutex<Firewall> {
    FW.get_or_init(|| Mutex::new(Firewall::new()))
}
