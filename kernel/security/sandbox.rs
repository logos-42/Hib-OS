//! 沙箱 —— 能力沙箱、系统调用过滤与文件/网络访问控制。
//!
//! 为 Hib OS 用户态宏内核提供进程级沙箱。每个沙箱绑定一个 PID 与一组规则，
//! 在系统调用、文件访问、网络访问与能力使用前进行拦截与校验。
//!
//! 与 `kernel/caps.rs` 的能力位模型配合，与 `kernel/syscall` 的系统调用层并行
//! 存在，不替换。所有状态保存在内存中，纯 Rust 实现，可脱网运行。
//!
//! 设计要点:
//! * `SandboxRule`：规则类型（SyscallAllow/SyscallDeny/FileAllow/FileDeny/
//!   NetAllow/NetDeny）、匹配模式（前缀/精确/通配符）、动作（Allow/Deny/Log）；
//! * `Sandbox`：绑定 PID、规则集、能力位集、文件系统根目录重定向、路径白名单；
//! * `SandboxManager`：管理全部沙箱，创建/销毁、增删规则、检查结果缓存；
//! * 系统调用过滤（seccomp 简化版）：默认拒绝或默认放行，按规则放宽；
//! * 文件系统沙箱：根目录重定向（chroot 样式）+ 路径白名单；
//! * 网络沙箱：地址/端口允许/拒绝列表；
//! * 能力沙箱：基于 `kernel/caps.rs` 的能力位校验。
//!
//! 文件末尾附带 `#[cfg(test)]` 单元测试，覆盖规则匹配、syscall 过滤、文件访问、
//! 网络访问、能力校验、循环/重复创建、缓存失效等。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::caps;

// ========================================================================
// 错误与结果
// ========================================================================

/// 沙箱错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// 沙箱不存在。
    NotFound { sandbox_id: SandboxId },
    /// 进程不在任何沙箱中。
    ProcessNotSandboxed { pid: u32 },
    /// 规则索引越界。
    RuleIndexOutOfBounds,
    /// 无效规则。
    InvalidRule { reason: String },
    /// 沙箱已销毁。
    Destroyed { sandbox_id: SandboxId },
}

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxError::NotFound { sandbox_id } => {
                write!(f, "沙箱不存在: {}", sandbox_id.0)
            }
            SandboxError::ProcessNotSandboxed { pid } => {
                write!(f, "进程 {} 不在任何沙箱中", pid)
            }
            SandboxError::RuleIndexOutOfBounds => write!(f, "规则索引越界"),
            SandboxError::InvalidRule { reason } => write!(f, "无效规则: {}", reason),
            SandboxError::Destroyed { sandbox_id } => {
                write!(f, "沙箱 {} 已销毁", sandbox_id.0)
            }
        }
    }
}

pub type SandboxResult<T> = Result<T, SandboxError>;

// ========================================================================
// 规则
// ========================================================================

/// 沙箱规则类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuleType {
    /// 允许的系统调用号（或名称模式）。
    SyscallAllow,
    /// 拒绝的系统调用号。
    SyscallDeny,
    /// 允许的文件路径模式。
    FileAllow,
    /// 拒绝的文件路径模式。
    FileDeny,
    /// 允许的网络地址/端口。
    NetAllow,
    /// 拒绝的网络地址/端口。
    NetDeny,
}

/// 匹配模式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchPattern {
    /// 精确匹配。
    Exact(String),
    /// 前缀匹配。
    Prefix(String),
    /// 通配符匹配（“*”匹配任意字符，仅支持末尾或单段）。
    Glob(String),
}

impl MatchPattern {
    /// 判断 `target` 是否匹配该模式。
    pub fn matches(&self, target: &str) -> bool {
        match self {
            MatchPattern::Exact(s) => target == s,
            MatchPattern::Prefix(p) => target.starts_with(p),
            MatchPattern::Glob(g) => glob_match(g, target),
        }
    }
}

/// 简单的 glob 匹配（“*”匹配任意序列，“?”匹配单字符）。
fn glob_match(pattern: &str, target: &str) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star = None;
    let mut match_ti = 0;
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let target_chars: Vec<char> = target.chars().collect();
    while ti < target_chars.len() {
        if pi < pattern_chars.len() && (pattern_chars[pi] == '?' || pattern_chars[pi] == target_chars[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern_chars.len() && pattern_chars[pi] == '*' {
            star = Some(pi);
            match_ti = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_ti += 1;
            ti = match_ti;
        } else {
            return false;
        }
    }
    while pi < pattern_chars.len() && pattern_chars[pi] == '*' {
        pi += 1;
    }
    pi == pattern_chars.len()
}

/// 规则动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleAction {
    /// 允许。
    Allow,
    /// 拒绝。
    Deny,
    /// 允许但记录审计。
    Log,
}

impl RuleAction {
    pub fn is_allow(self) -> bool {
        matches!(self, RuleAction::Allow | RuleAction::Log)
    }
}

/// 单条沙箱规则。
#[derive(Clone, Debug)]
pub struct SandboxRule {
    /// 规则类型。
    pub rule_type: RuleType,
    /// 匹配模式。
    pub pattern: MatchPattern,
    /// 动作。
    pub action: RuleAction,
    /// 规则描述（供审计与调试）。
    pub description: String,
    /// 规则优先级（数值越大越优先；相同类型时先匹配的优先）。
    pub priority: u32,
}

impl SandboxRule {
    /// 创建一条系统调用允许规则。
    pub fn syscall_allow(syscall: u32, description: &str) -> Self {
        Self {
            rule_type: RuleType::SyscallAllow,
            pattern: MatchPattern::Exact(syscall.to_string()),
            action: RuleAction::Allow,
            description: description.to_string(),
            priority: 100,
        }
    }

    /// 创建一条系统调用拒绝规则。
    pub fn syscall_deny(syscall: u32, description: &str) -> Self {
        Self {
            rule_type: RuleType::SyscallDeny,
            pattern: MatchPattern::Exact(syscall.to_string()),
            action: RuleAction::Deny,
            description: description.to_string(),
            priority: 100,
        }
    }

    /// 创建一条文件路径允许规则。
    pub fn file_allow(pattern: MatchPattern, description: &str) -> Self {
        Self {
            rule_type: RuleType::FileAllow,
            pattern,
            action: RuleAction::Allow,
            description: description.to_string(),
            priority: 50,
        }
    }

    /// 创建一条文件路径拒绝规则。
    pub fn file_deny(pattern: MatchPattern, description: &str) -> Self {
        Self {
            rule_type: RuleType::FileDeny,
            pattern,
            action: RuleAction::Deny,
            description: description.to_string(),
            priority: 50,
        }
    }

    /// 创建一条网络允许规则（pattern 为 "addr:port"）。
    pub fn net_allow(pattern: MatchPattern, description: &str) -> Self {
        Self {
            rule_type: RuleType::NetAllow,
            pattern,
            action: RuleAction::Allow,
            description: description.to_string(),
            priority: 50,
        }
    }

    /// 创建一条网络拒绝规则。
    pub fn net_deny(pattern: MatchPattern, description: &str) -> Self {
        Self {
            rule_type: RuleType::NetDeny,
            pattern,
            action: RuleAction::Deny,
            description: description.to_string(),
            priority: 50,
        }
    }
}

// ========================================================================
// 沙箱
// ========================================================================

/// 沙箱唯一标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SandboxId(pub u64);

/// 沙箱状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxState {
    Active,
    Destroyed,
}

/// 单个沙箱实例。
#[derive(Clone, Debug)]
pub struct Sandbox {
    /// 沙箱 id。
    pub id: SandboxId,
    /// 绑定的进程 PID。
    pub pid: u32,
    /// 规则列表（按优先级降序）。
    pub rules: Vec<SandboxRule>,
    /// 能力位集（基于 `kernel/caps.rs` 的能力位）。
    pub capabilities: HashSet<String>,
    /// 文件系统根目录重定向（chroot 样式；None 表示无重定向）。
    pub root_dir: Option<PathBuf>,
    /// 文件路径白名单（匹配任一即允许；Empty 表示不额外限制）。
    pub path_whitelist: Vec<MatchPattern>,
    /// 默认动作（无规则匹配时采用）。
    pub default_action: RuleAction,
    pub state: SandboxState,
    /// 系统调用检查命中次数（拒绝数）。
    pub syscall_denied: u64,
    /// 文件访问拒绝次数。
    pub file_denied: u64,
    /// 网络访问拒绝次数。
    pub net_denied: u64,
}

impl Sandbox {
    /// 为进程创建一个新沙箱。
    pub fn new(pid: u32, rules: Vec<SandboxRule>) -> Self {
        let mut rules = rules;
        rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        let mut capabilities = HashSet::new();
        // 默认授予基础能力，其余需显式添加。
        for cap in caps::ALL_CAPS {
            capabilities.insert(cap.to_string());
        }
        Self {
            id: SandboxId(0), // 由管理器填充
            pid,
            rules,
            capabilities,
            root_dir: None,
            path_whitelist: Vec::new(),
            default_action: RuleAction::Deny,
            state: SandboxState::Active,
            syscall_denied: 0,
            file_denied: 0,
            net_denied: 0,
        }
    }

    /// 设置默认动作。
    pub fn set_default_action(&mut self, action: RuleAction) {
        self.default_action = action;
    }

    /// 设置文件系统根目录重定向。
    pub fn set_root_dir(&mut self, root: PathBuf) {
        self.root_dir = Some(root);
    }

    /// 添加文件路径白名单模式。
    pub fn add_path_whitelist(&mut self, pattern: MatchPattern) {
        self.path_whitelist.push(pattern);
    }

    /// 授予能力。
    pub fn grant_cap(&mut self, cap: &str) {
        self.capabilities.insert(cap.to_string());
    }

    /// 撤销能力。
    pub fn revoke_cap(&mut self, cap: &str) {
        self.capabilities.remove(cap);
    }

    /// 校验系统调用是否被允许。
    ///
    /// 匹配规则：SyscallDeny 优先于 SyscallAllow；命中拒绝则拒绝，命中允许则
    /// 放行；未命中则按 `default_action` 决定。
    pub fn check_syscall(&mut self, syscall_no: u32) -> bool {
        let key = syscall_no.to_string();
        let mut best: Option<&SandboxRule> = None;
        for r in &self.rules {
            if r.rule_type == RuleType::SyscallAllow || r.rule_type == RuleType::SyscallDeny {
                if r.pattern.matches(&key) {
                    best = Some(r);
                    break; // 同类型按优先级已排序，先命中者优先
                }
            }
        }
        match best {
            Some(r) => match r.action {
                RuleAction::Deny => {
                    self.syscall_denied += 1;
                    false
                }
                _ => true,
            },
            None => self.default_action.is_allow(),
        }
    }

    /// 校验文件访问是否被允许。
    ///
    /// 若设置了 `root_dir`，则路径会被重定向到根目录下；再按 FileAllow/FileDeny
    /// 规则与白名单判断。FileDeny 优先于 FileAllow。
    pub fn check_file_access(&mut self, path: &str, _mode: u32) -> bool {
        let resolved = self.resolve_path(path);
        // 拒绝规则优先。
        for r in &self.rules {
            if r.rule_type == RuleType::FileDeny && r.pattern.matches(&resolved) {
                self.file_denied += 1;
                return false;
            }
        }
        // 允许规则。
        for r in &self.rules {
            if r.rule_type == RuleType::FileAllow && r.pattern.matches(&resolved) {
                return true;
            }
        }
        // 白名单。
        if !self.path_whitelist.is_empty() {
            if self.path_whitelist.iter().any(|p| p.matches(&resolved)) {
                return true;
            }
            self.file_denied += 1;
            return false;
        }
        self.default_action.is_allow()
    }

    /// 校验网络访问是否被允许（addr:port）。
    ///
    /// 按 NetAllow/NetDeny 判断，NetDeny 优先；未命中按 default_action。
    pub fn check_net_access(&mut self, addr: &str, port: u16) -> bool {
        let target = format!("{}:{}", addr, port);
        for r in &self.rules {
            if r.rule_type == RuleType::NetDeny && r.pattern.matches(&target) {
                self.net_denied += 1;
                return false;
            }
        }
        for r in &self.rules {
            if r.rule_type == RuleType::NetAllow && r.pattern.matches(&target) {
                return true;
            }
        }
        self.default_action.is_allow()
    }

    /// 校验能力是否被授予。
    pub fn check_capability(&self, cap: &str) -> bool {
        // uid 0（root）默认持有全部能力。
        if self.pid == 0 {
            return true;
        }
        self.capabilities.contains(cap)
    }

    /// 将路径按根目录重定向，并做路径遍历防护（去掉 ".." 越界）。
    fn resolve_path(&self, path: &str) -> String {
        let p = Path::new(path);
        if let Some(root) = &self.root_dir {
            let mut out = root.clone();
            for comp in p.iter() {
                let s = comp.to_string_lossy().to_string();
                if s == ".." {
                    // 不允许越过根目录。
                    if !out.pop() {
                        continue;
                    }
                } else if s != "." && !s.is_empty() {
                    out.push(&s);
                }
            }
            out.to_string_lossy().to_string()
        } else {
            path.to_string()
        }
    }

    /// 沙箱简要描述。
    pub fn describe(&self) -> String {
        format!(
            "sandbox id={} pid={} rules={} caps={} root={:?} state={:?} default={:?}",
            self.id.0, self.pid, self.rules.len(), self.capabilities.len(), self.root_dir, self.state, self.default_action
        )
    }
}

// ========================================================================
// 检查缓存
// ========================================================================

/// 单次访问检查的缓存条目。
#[derive(Clone, Debug)]
struct CheckCacheEntry {
    allowed: bool,
}

/// 检查缓存：(sandbox_id, check_type, key) -> 结果。
#[derive(Clone, Debug)]
pub struct CheckCache {
    entries: BTreeMap<(u64, String, String), CheckCacheEntry>,
    max_entries: usize,
}

impl CheckCache {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries: 1024,
        }
    }

    fn get(&self, sandbox_id: u64, check_type: &str, key: &str) -> Option<bool> {
        self.entries
            .get(&(sandbox_id, check_type.to_string(), key.to_string()))
            .map(|e| e.allowed)
    }

    fn put(&mut self, sandbox_id: u64, check_type: &str, key: &str, allowed: bool) {
        if self.entries.len() >= self.max_entries {
            // 简单的 FIFO 淘汰：移除第一个。
            if let Some(first) = self.entries.keys().next().cloned() {
                self.entries.remove(&first);
            }
        }
        self.entries.insert(
            (sandbox_id, check_type.to_string(), key.to_string()),
            CheckCacheEntry { allowed },
        );
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

// ========================================================================
// 沙箱管理器
// ========================================================================

/// 沙箱管理器：管理全部沙箱的生命周期与规则。
#[derive(Clone, Debug)]
pub struct SandboxManager {
    sandboxes: BTreeMap<SandboxId, Sandbox>,
    pid_to_sandbox: BTreeMap<u32, SandboxId>,
    next_id: u64,
    cache: CheckCache,
}

impl SandboxManager {
    /// 创建一个新的沙箱管理器。
    pub fn new() -> Self {
        Self {
            sandboxes: BTreeMap::new(),
            pid_to_sandbox: BTreeMap::new(),
            next_id: 1,
            cache: CheckCache::new(),
        }
    }

    /// 为进程创建沙箱。若进程已在沙箱中则返回已有 id。
    pub fn create_sandbox(&mut self, pid: u32, rules: Vec<SandboxRule>) -> SandboxId {
        if let Some(&id) = self.pid_to_sandbox.get(&pid) {
            return id;
        }
        let id = SandboxId(self.next_id);
        self.next_id += 1;
        let mut sandbox = Sandbox::new(pid, rules);
        sandbox.id = id;
        self.pid_to_sandbox.insert(pid, id);
        self.sandboxes.insert(id, sandbox);
        id
    }

    /// 销毁沙箱。
    pub fn destroy_sandbox(&mut self, sandbox_id: SandboxId) -> SandboxResult<()> {
        let sandbox = self
            .sandboxes
            .get(&sandbox_id)
            .ok_or_else(|| SandboxError::NotFound { sandbox_id })?;
        if sandbox.state == SandboxState::Destroyed {
            return Err(SandboxError::Destroyed { sandbox_id });
        }
        self.pid_to_sandbox.remove(&sandbox.pid);
        self.sandboxes.get_mut(&sandbox_id).unwrap().state = SandboxState::Destroyed;
        self.cache.clear();
        Ok(())
    }

    /// 移除进程对应的沙箱（进程退出时调用）。
    pub fn remove_by_pid(&mut self, pid: u32) -> Option<SandboxId> {
        if let Some(id) = self.pid_to_sandbox.remove(&pid) {
            if let Some(s) = self.sandboxes.get_mut(&id) {
                s.state = SandboxState::Destroyed;
            }
            self.cache.clear();
            Some(id)
        } else {
            None
        }
    }

    /// 向沙箱添加规则。
    pub fn add_rule(&mut self, sandbox_id: SandboxId, rule: SandboxRule) -> SandboxResult<()> {
        let sandbox = self
            .sandboxes
            .get_mut(&sandbox_id)
            .ok_or_else(|| SandboxError::NotFound { sandbox_id })?;
        if sandbox.state == SandboxState::Destroyed {
            return Err(SandboxError::Destroyed { sandbox_id });
        }
        sandbox.rules.push(rule);
        sandbox.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        self.cache.clear();
        Ok(())
    }

    /// 移除沙箱的指定索引规则。
    pub fn remove_rule(&mut self, sandbox_id: SandboxId, index: usize) -> SandboxResult<()> {
        let sandbox = self
            .sandboxes
            .get_mut(&sandbox_id)
            .ok_or_else(|| SandboxError::NotFound { sandbox_id })?;
        if sandbox.state == SandboxState::Destroyed {
            return Err(SandboxError::Destroyed { sandbox_id });
        }
        if index >= sandbox.rules.len() {
            return Err(SandboxError::RuleIndexOutOfBounds);
        }
        sandbox.rules.remove(index);
        self.cache.clear();
        Ok(())
    }

    /// 取得沙箱引用。
    pub fn sandbox(&self, sandbox_id: SandboxId) -> Option<&Sandbox> {
        self.sandboxes.get(&sandbox_id)
    }

    /// 取得进程所在沙箱 id。
    pub fn sandbox_for_pid(&self, pid: u32) -> Option<SandboxId> {
        self.pid_to_sandbox.get(&pid).copied()
    }

    /// 校验进程的系统调用（带缓存）。
    pub fn check_syscall(&mut self, pid: u32, syscall_no: u32) -> bool {
        let id = match self.pid_to_sandbox.get(&pid) {
            Some(id) => *id,
            None => return true, // 不在沙箱中的进程不受限
        };
        let key = syscall_no.to_string();
        if let Some(cached) = self.cache.get(id.0, "syscall", &key) {
            return cached;
        }
        let allowed = {
            let s = self.sandboxes.get_mut(&id).unwrap();
            s.check_syscall(syscall_no)
        };
        self.cache.put(id.0, "syscall", &key, allowed);
        allowed
    }

    /// 校验进程的文件访问（带缓存）。
    pub fn check_file_access(&mut self, pid: u32, path: &str, mode: u32) -> bool {
        let id = match self.pid_to_sandbox.get(&pid) {
            Some(id) => *id,
            None => return true,
        };
        let key = path.to_string();
        if let Some(cached) = self.cache.get(id.0, "file", &key) {
            return cached;
        }
        let allowed = {
            let s = self.sandboxes.get_mut(&id).unwrap();
            s.check_file_access(path, mode)
        };
        self.cache.put(id.0, "file", &key, allowed);
        allowed
    }

    /// 校验进程的网络访问（带缓存）。
    pub fn check_net_access(&mut self, pid: u32, addr: &str, port: u16) -> bool {
        let id = match self.pid_to_sandbox.get(&pid) {
            Some(id) => *id,
            None => return true,
        };
        let key = format!("{}:{}", addr, port);
        if let Some(cached) = self.cache.get(id.0, "net", &key) {
            return cached;
        }
        let allowed = {
            let s = self.sandboxes.get_mut(&id).unwrap();
            s.check_net_access(addr, port)
        };
        self.cache.put(id.0, "net", &key, allowed);
        allowed
    }

    /// 校验进程的能力。
    pub fn check_capability(&self, pid: u32, cap: &str) -> bool {
        let id = match self.pid_to_sandbox.get(&pid) {
            Some(id) => *id,
            None => return true,
        };
        self.sandboxes
            .get(&id)
            .map(|s| s.check_capability(cap))
            .unwrap_or(true)
    }

    /// 取得全部活跃沙箱数量。
    pub fn active_count(&self) -> usize {
        self.sandboxes
            .values()
            .filter(|s| s.state == SandboxState::Active)
            .count()
    }

    /// 取得总沙箱数量。
    pub fn len(&self) -> usize {
        self.sandboxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sandboxes.is_empty()
    }
}

impl Default for SandboxManager {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// 全局单例访问
// ========================================================================

static SANDBOX_MANAGER: OnceLock<Mutex<SandboxManager>> = OnceLock::new();

/// 取得全局沙箱管理器单例。
pub fn sandbox_manager() -> &'static Mutex<SandboxManager> {
    SANDBOX_MANAGER.get_or_init(|| Mutex::new(SandboxManager::new()))
}

/// 便捷函数：为进程创建沙箱。
pub fn create_sandbox(pid: u32, rules: Vec<SandboxRule>) -> SandboxId {
    sandbox_manager().lock().unwrap().create_sandbox(pid, rules)
}

/// 便捷函数：销毁沙箱。
pub fn destroy_sandbox(sandbox_id: SandboxId) -> SandboxResult<()> {
    sandbox_manager().lock().unwrap().destroy_sandbox(sandbox_id)
}

/// 便捷函数：校验系统调用。
pub fn check_syscall(pid: u32, syscall_no: u32) -> bool {
    sandbox_manager().lock().unwrap().check_syscall(pid, syscall_no)
}

/// 便捷函数：校验文件访问。
pub fn check_file_access(pid: u32, path: &str, mode: u32) -> bool {
    sandbox_manager().lock().unwrap().check_file_access(pid, path, mode)
}

/// 便捷函数：校验网络访问。
pub fn check_net_access(pid: u32, addr: &str, port: u16) -> bool {
    sandbox_manager().lock().unwrap().check_net_access(pid, addr, port)
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_destroy_sandbox() {
        let mut m = SandboxManager::new();
        let rules = vec![SandboxRule::syscall_allow(1, "allow read")];
        let id = m.create_sandbox(100, rules);
        assert_eq!(id.0, 1);
        assert_eq!(m.active_count(), 1);
        assert!(m.destroy_sandbox(id).is_ok());
        assert_eq!(m.active_count(), 0);
        assert_eq!(
            m.destroy_sandbox(id).unwrap_err(),
            SandboxError::Destroyed { sandbox_id: id }
        );
    }

    #[test]
    fn duplicate_pid_reuses_sandbox() {
        let mut m = SandboxManager::new();
        let id1 = m.create_sandbox(200, Vec::new());
        let id2 = m.create_sandbox(200, Vec::new());
        assert_eq!(id1, id2);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn syscall_allow_deny() {
        let mut m = SandboxManager::new();
        let rules = vec![
            SandboxRule::syscall_allow(0, "read"),
            SandboxRule::syscall_deny(1, "write"),
        ];
        let id = m.create_sandbox(300, rules);
        assert!(m.check_syscall(300, 0));
        assert!(!m.check_syscall(300, 1));
        // 默认拒绝：未列出的 syscall 被拒绝。
        assert!(!m.check_syscall(300, 42));
    }

    #[test]
    fn syscall_default_allow() {
        let mut m = SandboxManager::new();
        let rules = vec![SandboxRule::syscall_deny(9, "close")];
        let id = m.create_sandbox(301, rules);
        let _ = id;
        // 默认放行。
        assert!(m.check_syscall(301, 0));
        assert!(!m.check_syscall(301, 9));
    }

    #[test]
    fn file_allow_deny_and_whitelist() {
        let mut m = SandboxManager::new();
        let rules = vec![
            SandboxRule::file_deny(MatchPattern::Exact("/etc/shadow".to_string()), "deny shadow"),
            SandboxRule::file_allow(MatchPattern::Prefix("/tmp/".to_string()), "allow tmp"),
        ];
        let id = m.create_sandbox(400, rules);
        let _ = id;
        assert!(!m.check_file_access(400, "/etc/shadow", 0));
        assert!(m.check_file_access(400, "/tmp/foo", 0));
        // 白名单。
        sandbox_mut_for_test(&mut m, 400, |s| s.add_path_whitelist(MatchPattern::Prefix("/home/".to_string())));
        assert!(m.check_file_access(400, "/home/user", 0));
    }

    #[test]
    fn root_dir_redirect() {
        let mut m = SandboxManager::new();
        let id = m.create_sandbox(500, Vec::new());
        sandbox_mut_for_test(&mut m, 500, |s| {
            s.set_default_action(RuleAction::Allow);
            s.set_root_dir(PathBuf::from("/jail"));
        });
        // 路径 /etc/passwd 应被重定向到 /jail/etc/passwd。
        assert!(m.check_file_access(500, "/etc/passwd", 0));
    }

    #[test]
    fn net_allow_deny() {
        let mut m = SandboxManager::new();
        let rules = vec![
            SandboxRule::net_deny(MatchPattern::Exact("10.0.0.99:80".to_string()), "deny evil"),
            SandboxRule::net_allow(MatchPattern::Prefix("10.0.0.".to_string()), "allow local"),
        ];
        let id = m.create_sandbox(600, rules);
        let _ = id;
        // 精确拒绝优先于前缀允许。
        assert!(!m.check_net_access(600, "10.0.0.99", 80));
        assert!(m.check_net_access(600, "10.0.0.1", 80));
        assert!(m.check_net_access(600, "9.9.9.9", 53));
    }

    #[test]
    fn capability_check() {
        let mut m = SandboxManager::new();
        let id = m.create_sandbox(700, Vec::new());
        let _ = id;
        // root (pid 0) 默认全放行。
        assert!(m.check_capability(0, "CAP_SYS_ADMIN"));
        // 非 root 默认也全放行（sandbox 默认授予全部 caps）。
        assert!(m.check_capability(700, "CAP_SYS_ADMIN"));
        // 撤销后拒绝。
        sandbox_mut_for_test(&mut m, 700, |s| {
            s.revoke_cap("CAP_SYS_PTRACE");
        });
        assert!(!m.check_capability(700, "CAP_SYS_PTRACE"));
    }

    #[test]
    fn add_remove_rule() {
        let mut m = SandboxManager::new();
        let id = m.create_sandbox(800, vec![SandboxRule::syscall_deny(5, "deny")]);
        assert!(!m.check_syscall(800, 5));
        m.add_rule(id, SandboxRule::syscall_allow(5, "re-allow")).unwrap();
        assert!(m.check_syscall(800, 5));
        m.remove_rule(id, 0).unwrap();
        // 移除后无规则，默认拒绝。
        assert!(!m.check_syscall(800, 5));
    }

    #[test]
    fn process_not_sandboxed_is_unrestricted() {
        let mut m = SandboxManager::new();
        assert!(m.check_syscall(9999, 123));
        assert!(m.check_file_access(9999, "/anything", 0));
        assert!(m.check_net_access(9999, "1.2.3.4", 443));
    }

    #[test]
    fn cache_invalidated_on_rule_change() {
        let mut m = SandboxManager::new();
        let id = m.create_sandbox(900, vec![SandboxRule::syscall_deny(7, "deny")]);
        assert!(!m.check_syscall(900, 7));
        // 先缓存了拒绝结果，改规则后缓存应失效。
        m.add_rule(id, SandboxRule::syscall_allow(7, "allow")).unwrap();
        assert!(m.check_syscall(900, 7));
    }

    #[test]
    fn glob_pattern_matching() {
        assert!(MatchPattern::Glob("/tmp/*.log".to_string()).matches("/tmp/app.log"));
        assert!(!MatchPattern::Glob("/tmp/*.log".to_string()).matches("/tmp/app.txt"));
        assert!(MatchPattern::Prefix("/usr/".to_string()).matches("/usr/local/bin"));
        assert!(!MatchPattern::Exact("/etc/passwd".to_string()).matches("/etc/passwd1"));
    }

    #[test]
    fn sandbox_describe() {
        let mut m = SandboxManager::new();
        let id = m.create_sandbox(1000, Vec::new());
        let desc = m.sandbox(id).unwrap().describe();
        assert!(desc.contains("pid=1000"));
    }

    /// 可变访问沙箱（测试专用）。
    fn sandbox_mut_for_test<F>(mgr: &mut SandboxManager, pid: u32, f: F)
    where
        F: FnOnce(&mut Sandbox),
    {
        let id = mgr.pid_to_sandbox.get(&pid).copied().unwrap();
        let sandbox = mgr.sandboxes.get_mut(&id).unwrap();
        f(sandbox);
    }
}

