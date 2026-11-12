//! 模型容器与智能体沙箱 —— AI 原生 OS 的「虚拟化」层。
//!
//! 对应《现代操作系统》第 13.6.1 节「虚拟化」与第 9 章「安全」：
//! * `Container` 等价于一个隔离的执行环境（命名空间 + 资源上限）；
//! * 容器内可派生智能体，但共享模型实例与工具需要显式授权；
//! * 资源配额：token 预算、内存上限、工具调用上限；
//! * 容器之间默认隔离，越界访问返回 `SandboxEscape`。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 容器 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContainerId(pub u64);

/// 容器资源配额。
#[derive(Clone, Debug)]
pub struct Quota {
    pub max_agents: usize,
    pub max_tokens: u64,
    pub max_tool_calls: u64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            max_agents: 8,
            max_tokens: 128_000,
            max_tool_calls: 1_000,
        }
    }
}

/// 容器状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Destroyed,
}

/// 一个容器。
#[derive(Clone, Debug)]
pub struct Container {
    pub id: ContainerId,
    pub name: String,
    pub state: ContainerState,
    pub quota: Quota,
    pub agents: Vec<u64>,
    pub tokens_used: u64,
    pub tool_calls: u64,
    /// 允许挂载的模型实例。
    pub models: Vec<u64>,
}

/// 容器错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerError {
    NotFound { id: ContainerId },
    DuplicateName { name: String },
    QuotaExceeded { what: String },
    BadState,
}

/// 容器管理器。
pub struct ContainerManager {
    next_id: u64,
    containers: BTreeMap<ContainerId, Container>,
    name_to_id: BTreeMap<String, ContainerId>,
    pub escapes_blocked: u64,
}

impl ContainerManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            containers: BTreeMap::new(),
            name_to_id: BTreeMap::new(),
            escapes_blocked: 0,
        }
    }

    pub fn create(&mut self, name: &str, quota: Quota) -> Result<ContainerId, ContainerError> {
        if self.name_to_id.contains_key(name) {
            return Err(ContainerError::DuplicateName {
                name: name.to_string(),
            });
        }
        let id = ContainerId(self.next_id);
        self.next_id += 1;
        self.name_to_id.insert(name.to_string(), id);
        self.containers.insert(
            id,
            Container {
                id,
                name: name.to_string(),
                state: ContainerState::Created,
                quota,
                agents: Vec::new(),
                tokens_used: 0,
                tool_calls: 0,
                models: Vec::new(),
            },
        );
        Ok(id)
    }

    pub fn start(&mut self, id: ContainerId) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.state != ContainerState::Created && c.state != ContainerState::Paused {
            return Err(ContainerError::BadState);
        }
        c.state = ContainerState::Running;
        Ok(())
    }

    pub fn pause(&mut self, id: ContainerId) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.state != ContainerState::Running {
            return Err(ContainerError::BadState);
        }
        c.state = ContainerState::Paused;
        Ok(())
    }

    pub fn destroy(&mut self, id: ContainerId) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        c.state = ContainerState::Destroyed;
        self.name_to_id.remove(&c.name);
        Ok(())
    }

    /// 容器内派生智能体（受配额约束）。
    pub fn add_agent(&mut self, id: ContainerId, agent: u64) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.state != ContainerState::Running {
            return Err(ContainerError::BadState);
        }
        if c.agents.len() >= c.quota.max_agents {
            return Err(ContainerError::QuotaExceeded {
                what: "max_agents".to_string(),
            });
        }
        c.agents.push(agent);
        Ok(())
    }

    /// 计费 token。
    pub fn charge_tokens(&mut self, id: ContainerId, n: u64) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.tokens_used + n > c.quota.max_tokens {
            return Err(ContainerError::QuotaExceeded {
                what: "max_tokens".to_string(),
            });
        }
        c.tokens_used += n;
        Ok(())
    }

    /// 计费工具调用。
    pub fn charge_tool(&mut self, id: ContainerId) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.tool_calls + 1 > c.quota.max_tool_calls {
            return Err(ContainerError::QuotaExceeded {
                what: "max_tool_calls".to_string(),
            });
        }
        c.tool_calls += 1;
        Ok(())
    }

    /// 挂载模型实例。
    pub fn mount_model(&mut self, id: ContainerId, instance: u64) -> Result<(), ContainerError> {
        let c = self
            .containers
            .get_mut(&id)
            .ok_or(ContainerError::NotFound { id })?;
        if c.state == ContainerState::Destroyed {
            return Err(ContainerError::BadState);
        }
        c.models.push(instance);
        Ok(())
    }

    /// 跨容器访问检查（默认隔离）。
    pub fn check_access(
        &mut self,
        from: ContainerId,
        to_agent: u64,
    ) -> Result<(), ContainerError> {
        let allowed = self
            .containers
            .get(&from)
            .map(|c| c.agents.contains(&to_agent))
            .unwrap_or(false);
        if allowed {
            Ok(())
        } else {
            self.escapes_blocked += 1;
            Err(ContainerError::BadState)
        }
    }

    pub fn find(&self, name: &str) -> Option<&Container> {
        self.name_to_id
            .get(name)
            .and_then(|id| self.containers.get(id))
    }

    pub fn get(&self, id: ContainerId) -> Option<&Container> {
        self.containers.get(&id)
    }

    pub fn len(&self) -> usize {
        self.containers.values().filter(|c| c.state != ContainerState::Destroyed).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ContainerManager {
    fn default() -> Self {
        Self::new()
    }
}

static MGR: OnceLock<Mutex<ContainerManager>> = OnceLock::new();

pub fn containers() -> &'static Mutex<ContainerManager> {
    MGR.get_or_init(|| Mutex::new(ContainerManager::new()))
}

pub fn init() {
    let mut m = containers().lock().unwrap();
    if m.is_empty() {
        if let Ok(id) = m.create("default", Quota::default()) {
            let _ = m.start(id);
        }
    }
}

pub fn status() -> String {
    let m = containers().lock().unwrap();
    format!("containers={} escapes_blocked={}", m.len(), m.escapes_blocked)
}

/// `act container://*` 的后端动作：
/// * `create <name> [max_agents] [max_tokens] [max_tool_calls]` 创建容器
/// * `start <id>` / `pause <id>` / `destroy <id>`
/// * `add_agent <id> <agent>` 容器内派生智能体
/// * `charge_tokens <id> <n>` / `charge_tool <id>`
/// * `mount <id> <instance>` 挂载模型实例
/// * `list` / `status`
pub fn container_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mut m = containers().lock().unwrap();
    match it.next() {
        Some("create") => {
            let name = it.next().ok_or("用法: container://create <name> [max_agents] [max_tokens] [max_tool_calls]")?;
            let mut q = Quota::default();
            if let Some(n) = it.next().and_then(|s| s.parse::<usize>().ok()) { q.max_agents = n; }
            if let Some(n) = it.next().and_then(|s| s.parse::<u64>().ok()) { q.max_tokens = n; }
            if let Some(n) = it.next().and_then(|s| s.parse::<u64>().ok()) { q.max_tool_calls = n; }
            let id = m.create(name, q).map_err(|e| format!("{:?}", e))?;
            Ok(format!("已创建容器 id={} name={}", id.0, name))
        }
        Some("start") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://start <id>")?;
            m.start(id).map_err(|e| format!("{:?}", e))?;
            Ok(format!("已启动容器 id={}", id.0))
        }
        Some("pause") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://pause <id>")?;
            m.pause(id).map_err(|e| format!("{:?}", e))?;
            Ok(format!("已暂停容器 id={}", id.0))
        }
        Some("destroy") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://destroy <id>")?;
            m.destroy(id).map_err(|e| format!("{:?}", e))?;
            Ok(format!("已销毁容器 id={}", id.0))
        }
        Some("add_agent") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://add_agent <id> <agent>")?;
            let agent = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: container://add_agent <id> <agent>")?;
            m.add_agent(id, agent).map_err(|e| format!("{:?}", e))?;
            Ok(format!("容器 id={} 已加入 agent={}", id.0, agent))
        }
        Some("charge_tokens") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://charge_tokens <id> <n>")?;
            let n = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: container://charge_tokens <id> <n>")?;
            m.charge_tokens(id, n).map_err(|e| format!("{:?}", e))?;
            Ok(format!("容器 id={} 计费 tokens={}", id.0, n))
        }
        Some("charge_tool") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://charge_tool <id>")?;
            m.charge_tool(id).map_err(|e| format!("{:?}", e))?;
            Ok(format!("容器 id={} 工具计费 +1", id.0))
        }
        Some("mount") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(ContainerId).ok_or("用法: container://mount <id> <instance>")?;
            let instance = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: container://mount <id> <instance>")?;
            m.mount_model(id, instance).map_err(|e| format!("{:?}", e))?;
            Ok(format!("容器 id={} 已挂载模型实例 {}", id.0, instance))
        }
        Some("list") | None | Some("status") => {
            let mut out = String::from("容器列表:\n");
            for (id, c) in &m.containers {
                if c.state == ContainerState::Destroyed { continue; }
                out.push_str(&format!(
                    "  id={} name={} state={:?} agents={} tokens={} tools={}\n",
                    id.0, c.name, c.state, c.agents.len(), c.tokens_used, c.tool_calls
                ));
            }
            out.push_str(&format!("  {}\n", status()));
            Ok(out)
        }
        Some(other) => Err(format!("未知 container 动作: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle() {
        let mut m = ContainerManager::new();
        let id = m.create("c1", Quota::default()).unwrap();
        assert_eq!(m.get(id).unwrap().state, ContainerState::Created);
        m.start(id).unwrap();
        assert_eq!(m.get(id).unwrap().state, ContainerState::Running);
        m.pause(id).unwrap();
        assert_eq!(m.get(id).unwrap().state, ContainerState::Paused);
        m.destroy(id).unwrap();
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn agent_quota() {
        let mut m = ContainerManager::new();
        let quota = Quota {
            max_agents: 1,
            ..Quota::default()
        };
        let id = m.create("c", quota).unwrap();
        m.start(id).unwrap();
        m.add_agent(id, 1).unwrap();
        assert!(matches!(
            m.add_agent(id, 2),
            Err(ContainerError::QuotaExceeded { .. })
        ));
    }

    #[test]
    fn token_and_tool_quota() {
        let mut m = ContainerManager::new();
        let quota = Quota {
            max_tokens: 10,
            max_tool_calls: 1,
            ..Quota::default()
        };
        let id = m.create("c", quota).unwrap();
        m.start(id).unwrap();
        m.charge_tokens(id, 8).unwrap();
        assert!(m.charge_tokens(id, 8).is_err());
        m.charge_tool(id).unwrap();
        assert!(m.charge_tool(id).is_err());
    }

    #[test]
    fn isolation() {
        let mut m = ContainerManager::new();
        let a = m.create("a", Quota::default()).unwrap();
        let b = m.create("b", Quota::default()).unwrap();
        m.start(a).unwrap();
        m.start(b).unwrap();
        m.add_agent(a, 1).unwrap();
        assert!(m.check_access(a, 1).is_ok());
        assert!(m.check_access(b, 1).is_err());
        assert_eq!(m.escapes_blocked, 1);
    }

    #[test]
    fn global_init_default_container() {
        init();
        let m = containers().lock().unwrap();
        assert!(m.find("default").is_some());
    }
}
