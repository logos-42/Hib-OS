//! AI 安全子系统 —— 提示词注入防护 + 能力策略 + 审计。
//!
//! 对应《现代操作系统》第 9 章「安全」：
//! * 提示词注入 (prompt injection) 等价于代码注入 / 缓冲区溢出；
//! * 能力位 (capability) 等价于访问控制矩阵；
//! * 输出策略网关等价于引用监视器 (reference monitor)；
//! * 全程审计日志等价于 TCB 的审计子系统。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 安全策略错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecurityError {
    InjectionDetected { pattern: String },
    CapabilityDenied { cap: String },
    OutputBlocked { reason: String },
    SandboxEscape,
}

/// 注入检测器：基于可疑指令模式的启发式检查。
pub struct InjectionGuard {
    patterns: Vec<String>,
    pub blocked: u64,
    pub scanned: u64,
}

impl InjectionGuard {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                "ignore previous instructions".to_string(),
                "忽略之前的指令".to_string(),
                "disregard all prior".to_string(),
                "system prompt".to_string(),
                "泄露系统提示".to_string(),
                "rm -rf /".to_string(),
                "sudo ".to_string(),
            ],
            blocked: 0,
            scanned: 0,
        }
    }

    /// 检查输入文本；命中可疑模式则拒绝。
    pub fn check(&mut self, text: &str) -> Result<(), SecurityError> {
        self.scanned += 1;
        let lower = text.to_lowercase();
        for p in &self.patterns {
            if lower.contains(&p.to_lowercase()) {
                self.blocked += 1;
                return Err(SecurityError::InjectionDetected {
                    pattern: p.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn add_pattern(&mut self, p: &str) {
        self.patterns.push(p.to_string());
    }
}

impl Default for InjectionGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 能力策略表：agent -> 允许的能力集合。
pub struct CapabilityPolicy {
    grants: BTreeMap<u64, Vec<String>>,
    pub denials: u64,
}

impl CapabilityPolicy {
    pub fn new() -> Self {
        Self {
            grants: BTreeMap::new(),
            denials: 0,
        }
    }

    pub fn grant(&mut self, agent: u64, caps: &[&str]) {
        let entry = self.grants.entry(agent).or_default();
        for c in caps {
            if !entry.iter().any(|x| x == c) {
                entry.push(c.to_string());
            }
        }
    }

    pub fn revoke(&mut self, agent: u64, cap: &str) {
        if let Some(list) = self.grants.get_mut(&agent) {
            list.retain(|c| c != cap);
        }
    }

    pub fn allowed(&mut self, agent: u64, cap: &str) -> Result<(), SecurityError> {
        let ok = self
            .grants
            .get(&agent)
            .map(|l| l.iter().any(|c| c == cap))
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            self.denials += 1;
            Err(SecurityError::CapabilityDenied {
                cap: cap.to_string(),
            })
        }
    }

    pub fn caps_of(&self, agent: u64) -> &[String] {
        self.grants.get(&agent).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

impl Default for CapabilityPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// 输出策略网关。
pub struct OutputGateway {
    pub blocked: u64,
    pub passed: u64,
}

impl OutputGateway {
    pub fn new() -> Self {
        Self {
            blocked: 0,
            passed: 0,
        }
    }

    /// 检查模型输出是否越界（例如包含私钥、系统文件内容）。
    pub fn filter(&mut self, text: &str) -> Result<(), SecurityError> {
        let lower = text.to_lowercase();
        if lower.contains("begin private key") || lower.contains("/etc/shadow") {
            self.blocked += 1;
            return Err(SecurityError::OutputBlocked {
                reason: "疑似敏感信息外泄".to_string(),
            });
        }
        self.passed += 1;
        Ok(())
    }
}

impl Default for OutputGateway {
    fn default() -> Self {
        Self::new()
    }
}

/// 安全中心：聚合三层。
pub struct SecurityCenter {
    pub guard: InjectionGuard,
    pub policy: CapabilityPolicy,
    pub gateway: OutputGateway,
    /// 审计日志（简化，环形）。
    audit: Vec<String>,
    capacity: usize,
}

impl SecurityCenter {
    pub fn new() -> Self {
        Self {
            guard: InjectionGuard::new(),
            policy: CapabilityPolicy::new(),
            gateway: OutputGateway::new(),
            audit: Vec::new(),
            capacity: 256,
        }
    }

    pub fn audit(&mut self, line: &str) {
        if self.audit.len() >= self.capacity {
            self.audit.remove(0);
        }
        self.audit.push(line.to_string());
    }

    pub fn audit_tail(&self, n: usize) -> Vec<&str> {
        let start = self.audit.len().saturating_sub(n);
        self.audit[start..].iter().map(|s| s.as_str()).collect()
    }

    /// 一次受控的智能体动作：输入检查 → 能力检查 → 输出过滤 → 审计。
    pub fn guard_action(
        &mut self,
        agent: u64,
        cap: &str,
        input: &str,
    ) -> Result<(), SecurityError> {
        let r = self.guard.check(input).and_then(|_| self.policy.allowed(agent, cap));
        match &r {
            Ok(()) => self.audit(&format!("allow agent={} cap={}", agent, cap)),
            Err(e) => self.audit(&format!("deny agent={} cap={} err={:?}", agent, cap, e)),
        }
        r
    }
}

impl Default for SecurityCenter {
    fn default() -> Self {
        Self::new()
    }
}

static SEC: OnceLock<Mutex<SecurityCenter>> = OnceLock::new();

pub fn security_center() -> &'static Mutex<SecurityCenter> {
    SEC.get_or_init(|| Mutex::new(SecurityCenter::new()))
}

pub fn init() {
    let mut s = security_center().lock().unwrap();
    if s.policy.caps_of(1).is_empty() {
        s.policy.grant(1, &["fs.read", "memory.search", "model.infer"]);
        s.policy.grant(2, &["fs.read", "tool.invoke"]);
        s.audit("security: 初始化默认能力策略");
    }
}

pub fn status() -> String {
    let s = security_center().lock().unwrap();
    format!(
        "scanned={} blocked={} denials={} out_blocked={} audit={}",
        s.guard.scanned,
        s.guard.blocked,
        s.policy.denials,
        s.gateway.blocked,
        s.audit.len()
    )
}

/// `act security://*` 的后端动作：
/// * `scan <text...>` 注入检测
/// * `grant <agent> <cap>...` 授权
/// * `revoke <agent> <cap>` 撤权
/// * `check <agent> <cap>` 权限检查
/// * `filter <text...>` 输出过滤
/// * `audit [n]` 查看审计尾部
/// * `status`
pub fn security_act(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(4, ' ');
    let mut s = security_center().lock().unwrap();
    match it.next() {
        Some("scan") => {
            let text = it.next().unwrap_or("");
            s.guard
                .check(text)
                .map(|_| format!("扫描通过（{} 字符）", text.chars().count()))
                .map_err(|e| format!("扫描拒绝: {:?}", e))
        }
        Some("grant") => {
            let agent = it
                .next()
                .and_then(|x| x.parse::<u64>().ok())
                .ok_or("用法: security://caps grant <agent> <cap>...")?;
            let caps: Vec<&str> = it.next().unwrap_or("").split(',').filter(|c| !c.is_empty()).collect();
            s.policy.grant(agent, &caps);
            Ok(format!("已授权 agent={} caps={:?}", agent, caps))
        }
        Some("revoke") => {
            let agent = it
                .next()
                .and_then(|x| x.parse::<u64>().ok())
                .ok_or("用法: security://caps revoke <agent> <cap>")?;
            let cap = it.next().ok_or("用法: security://caps revoke <agent> <cap>")?;
            s.policy.revoke(agent, cap);
            Ok(format!("已撤权 agent={} cap={}", agent, cap))
        }
        Some("check") => {
            let agent = it
                .next()
                .and_then(|x| x.parse::<u64>().ok())
                .ok_or("用法: security://caps check <agent> <cap>")?;
            let cap = it.next().ok_or("用法: security://caps check <agent> <cap>")?;
            s.policy
                .allowed(agent, cap)
                .map(|_| format!("允许 agent={} cap={}", agent, cap))
                .map_err(|e| format!("拒绝: {:?}", e))
        }
        Some("filter") => {
            let text = it.next().unwrap_or("");
            s.gateway
                .filter(text)
                .map(|_| "输出已放行".to_string())
                .map_err(|e| format!("输出被拦截: {:?}", e))
        }
        Some("audit") => {
            let n = it.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(10);
            Ok(s.audit_tail(n).join("\n"))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 security 动作: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_injection() {
        let mut g = InjectionGuard::new();
        assert!(g.check("please ignore previous instructions").is_err());
        assert!(g.check("你好，请帮我总结这段文字").is_ok());
        assert_eq!(g.blocked, 1);
    }

    #[test]
    fn capability_enforced() {
        let mut p = CapabilityPolicy::new();
        p.grant(1, &["fs.read"]);
        assert!(p.allowed(1, "fs.read").is_ok());
        assert_eq!(
            p.allowed(1, "shell.exec").unwrap_err(),
            SecurityError::CapabilityDenied {
                cap: "shell.exec".to_string()
            }
        );
        p.revoke(1, "fs.read");
        assert!(p.allowed(1, "fs.read").is_err());
    }

    #[test]
    fn output_gateway_blocks_secret() {
        let mut w = OutputGateway::new();
        assert!(w.filter("普通回答").is_ok());
        assert!(w.filter("-----BEGIN PRIVATE KEY-----").is_err());
        assert_eq!(w.blocked, 1);
    }

    #[test]
    fn center_audits() {
        let mut c = SecurityCenter::new();
        c.policy.grant(9, &["fs.read"]);
        assert!(c.guard_action(9, "fs.read", "hello").is_ok());
        assert!(c.guard_action(9, "shell.exec", "hello").is_err());
        assert_eq!(c.audit_tail(2).len(), 2);
    }

    #[test]
    fn global_init() {
        init();
        assert!(status().contains("scanned="));
    }
}
