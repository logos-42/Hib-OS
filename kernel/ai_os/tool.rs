//! 工具调用子系统 (Tool Use) —— AI 原生 OS 的「设备驱动」层。
//!
//! 在经典操作系统中，进程通过系统调用与设备驱动访问外部世界；在 AI 原生
//! 操作系统中，智能体通过**工具 (Tool)** 访问外部世界。工具注册表等价于
//! 设备注册表，工具调用等价于一次 I/O 请求，工具租约等价于独占设备。
//!
//! 设计要点（对应《现代操作系统》第 5 章「输入/输出」）：
//! * `ToolSpec`：工具名、描述、参数模式、能力需求、是否幂等、超时；
//! * `ToolRegistry`：注册/注销/查找工具，按能力过滤（谁能调用）；
//! * `ToolCall` / `ToolResult`：一次调用的请求与响应，带 trace id 便于审计；
//! * 并发安全：租约式独占（`lease.rs`）保证有副作用的工具不被并发调用；
//! * 全部为内存原型，不依赖外部 crate，可脱网运行。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 工具唯一标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolId(pub u64);

/// 工具风险级别（决定是否需要人工确认或沙箱）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 只读、无副作用。
    ReadOnly,
    /// 有副作用但可回滚。
    Reversible,
    /// 不可逆或有外部影响，需要显式授权。
    Destructive,
}

/// 工具规格（注册表中的条目）。
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub id: ToolId,
    pub name: String,
    pub description: String,
    /// 参数名列表（简化 JSON schema）。
    pub params: Vec<String>,
    /// 调用所需能力位（对应 `caps.rs`）。
    pub required_caps: Vec<String>,
    pub risk: RiskLevel,
    /// 是否幂等（可安全重试）。
    pub idempotent: bool,
    /// 超时（毫秒），0 表示不限。
    pub timeout_ms: u64,
    pub call_count: u64,
    pub fail_count: u64,
}

impl ToolSpec {
    pub fn new(id: ToolId, name: &str, description: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            description: description.to_string(),
            params: Vec::new(),
            required_caps: Vec::new(),
            risk: RiskLevel::ReadOnly,
            idempotent: true,
            timeout_ms: 0,
            call_count: 0,
            fail_count: 0,
        }
    }
}

/// 一次工具调用请求。
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub trace_id: u64,
    pub tool: ToolId,
    pub args: BTreeMap<String, String>,
    /// 调用方智能体 id。
    pub caller: u64,
}

/// 一次工具调用结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub trace_id: u64,
    pub ok: bool,
    pub output: String,
    pub elapsed_ms: u64,
}

/// 工具注册表错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolError {
    NotFound { name: String },
    Duplicate { name: String },
    MissingCap { cap: String },
    DestructiveNeedsGrant { name: String },
    Disabled,
}

/// 工具注册表。
pub struct ToolRegistry {
    next_id: u64,
    tools: BTreeMap<ToolId, ToolSpec>,
    name_to_id: BTreeMap<String, ToolId>,
    /// 工具名 -> 持有租约的调用者（独占）。
    leases: BTreeMap<String, u64>,
    pub total_calls: u64,
    pub total_failures: u64,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            tools: BTreeMap::new(),
            name_to_id: BTreeMap::new(),
            leases: BTreeMap::new(),
            total_calls: 0,
            total_failures: 0,
        }
    }

    /// 注册一个工具。
    pub fn register(&mut self, mut spec: ToolSpec) -> Result<ToolId, ToolError> {
        if self.name_to_id.contains_key(&spec.name) {
            return Err(ToolError::Duplicate {
                name: spec.name.clone(),
            });
        }
        let id = ToolId(self.next_id);
        self.next_id += 1;
        spec.id = id;
        self.name_to_id.insert(spec.name.clone(), id);
        self.tools.insert(id, spec);
        Ok(id)
    }

    /// 注销工具。
    pub fn unregister(&mut self, name: &str) -> Result<(), ToolError> {
        let id = *self
            .name_to_id
            .get(name)
            .ok_or_else(|| ToolError::NotFound {
                name: name.to_string(),
            })?;
        self.name_to_id.remove(name);
        self.tools.remove(&id);
        Ok(())
    }

    /// 按名称查找。
    pub fn find(&self, name: &str) -> Option<&ToolSpec> {
        self.name_to_id.get(name).and_then(|id| self.tools.get(id))
    }

    /// 列出全部工具名。
    pub fn names(&self) -> Vec<String> {
        self.name_to_id.keys().cloned().collect()
    }

    /// 按调用方能力位过滤出可调用的工具。
    pub fn available_for(&self, caps: &[String]) -> Vec<&ToolSpec> {
        self.tools
            .values()
            .filter(|t| t.required_caps.iter().all(|c| caps.contains(c)))
            .collect()
    }

    /// 校验一次调用：工具存在、能力满足、破坏性工具需显式授权。
    pub fn check(&self, call: &ToolCall, caps: &[String], granted: bool) -> Result<(), ToolError> {
        let spec = self
            .tools
            .get(&call.tool)
            .ok_or_else(|| ToolError::NotFound {
                name: format!("{:?}", call.tool),
            })?;
        for cap in &spec.required_caps {
            if !caps.contains(cap) {
                return Err(ToolError::MissingCap { cap: cap.clone() });
            }
        }
        if spec.risk == RiskLevel::Destructive && !granted {
            return Err(ToolError::DestructiveNeedsGrant {
                name: spec.name.clone(),
            });
        }
        Ok(())
    }

    /// 获取工具独占租约（有副作用工具使用）。
    pub fn acquire(&mut self, name: &str, caller: u64) -> Result<(), ToolError> {
        let spec = self.find(name).ok_or_else(|| ToolError::NotFound {
            name: name.to_string(),
        })?;
        if !spec.idempotent {
            if let Some(holder) = self.leases.get(name) {
                if *holder != caller {
                    return Err(ToolError::Disabled);
                }
            }
            self.leases.insert(name.to_string(), caller);
        }
        Ok(())
    }

    /// 释放工具租约。
    pub fn release(&mut self, name: &str, caller: u64) {
        if let Some(holder) = self.leases.get(name) {
            if *holder == caller {
                self.leases.remove(name);
            }
        }
    }

    /// 记录一次调用结果。
    pub fn record(&mut self, result: &ToolResult) {
        self.total_calls += 1;
        if !result.ok {
            self.total_failures += 1;
        }
        if let Some(spec) = self.tools.values_mut().find(|t| t.id.0 == result.trace_id % 1_000_000) {
            spec.call_count += 1;
            if !result.ok {
                spec.fail_count += 1;
            }
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static TOOLS: OnceLock<Mutex<ToolRegistry>> = OnceLock::new();

/// 全局工具注册表。
pub fn tools() -> &'static Mutex<ToolRegistry> {
    TOOLS.get_or_init(|| Mutex::new(ToolRegistry::new()))
}

/// 初始化：注册一组内置工具（对应内核内置驱动）。
pub fn init() {
    let mut reg = tools().lock().unwrap();
    if !reg.names().is_empty() {
        return;
    }
    let builtin = [
        ("fs.read", "读取文件内容", RiskLevel::ReadOnly, true),
        ("fs.write", "写入文件", RiskLevel::Reversible, false),
        ("net.fetch", "发起网络请求", RiskLevel::ReadOnly, true),
        ("shell.exec", "执行命令", RiskLevel::Destructive, false),
        ("gpu.compute", "提交 GPU 计算任务", RiskLevel::Reversible, true),
        ("memory.search", "语义检索长期记忆", RiskLevel::ReadOnly, true),
    ];
    for (name, desc, risk, idem) in builtin {
        let mut spec = ToolSpec::new(ToolId(0), name, desc);
        spec.risk = risk;
        spec.idempotent = idem;
        let _ = reg.register(spec);
    }
}

/// 状态摘要，供 `sense tool/registry` 使用。
pub fn status() -> String {
    let reg = tools().lock().unwrap();
    format!(
        "tools={} calls={} failures={}",
        reg.names().len(),
        reg.total_calls,
        reg.total_failures
    )
}

/// `act tool/*` 的后端动作：
/// * `list` 列出工具
/// * `available <cap>...` 按能力过滤
/// * `call <工具名> [k=v]...` 模拟一次调用
/// * `status` 摘要
pub fn tool_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mut reg = tools().lock().unwrap();
    match it.next() {
        Some("list") | None => {
            let mut out = String::from("工具注册表:\n");
            for name in reg.names() {
                if let Some(t) = reg.find(&name) {
                    out.push_str(&format!(
                        "  {} risk={:?} idem={} calls={}\n",
                        t.name, t.risk, t.idempotent, t.call_count
                    ));
                }
            }
            Ok(out)
        }
        Some("available") => {
            let caps: Vec<String> = it.map(|s| s.to_string()).collect();
            let list = reg.available_for(&caps);
            let mut out = String::from("可调用工具:\n");
            for t in list {
                out.push_str(&format!("  {}\n", t.name));
            }
            Ok(out)
        }
        Some("call") => {
            let name = it.next().ok_or("用法: tool/exec call <工具名> [k=v]...")?;
            let spec = reg
                .find(name)
                .ok_or_else(|| format!("工具不存在: {}", name))?;
            let id = spec.id;
            let idem = spec.idempotent;
            let mut args = BTreeMap::new();
            for kv in it {
                if let Some((k, v)) = kv.split_once('=') {
                    args.insert(k.to_string(), v.to_string());
                }
            }
            let call = ToolCall {
                trace_id: reg.total_calls + 1,
                tool: id,
                args,
                caller: 0,
            };
            if !idem {
                reg.acquire(name, 0).map_err(|e| format!("获取租约失败: {:?}", e))?;
            }
            let result = ToolResult {
                trace_id: call.trace_id,
                ok: true,
                output: format!("工具 {} 已执行（模拟）", name),
                elapsed_ms: 1,
            };
            reg.record(&result);
            if !idem {
                reg.release(name, 0);
            }
            Ok(result.output)
        }
        Some("status") => Ok(status()),
        Some(other) => Err(format!("未知 tool 动作: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_find() {
        let mut reg = ToolRegistry::new();
        let id = reg
            .register(ToolSpec::new(ToolId(0), "fs.read", "读文件"))
            .unwrap();
        assert_eq!(reg.find("fs.read").unwrap().id, id);
        assert!(reg.find("nope").is_none());
    }

    #[test]
    fn duplicate_register_rejected() {
        let mut reg = ToolRegistry::new();
        reg.register(ToolSpec::new(ToolId(0), "x", "x")).unwrap();
        assert_eq!(
            reg.register(ToolSpec::new(ToolId(0), "x", "x")).unwrap_err(),
            ToolError::Duplicate { name: "x".to_string() }
        );
    }

    #[test]
    fn capability_filter() {
        let mut reg = ToolRegistry::new();
        let mut spec = ToolSpec::new(ToolId(0), "shell.exec", "执行");
        spec.required_caps.push("shell".to_string());
        let id = reg.register(spec).unwrap();
        assert!(reg.available_for(&[]).is_empty());
        assert_eq!(reg.available_for(&["shell".to_string()]).len(), 1);

        let call = ToolCall {
            trace_id: 1,
            tool: id,
            args: BTreeMap::new(),
            caller: 7,
        };
        assert_eq!(
            reg.check(&call, &[], false).unwrap_err(),
            ToolError::MissingCap { cap: "shell".to_string() }
        );
        assert!(reg.check(&call, &["shell".to_string()], false).is_ok());
    }

    #[test]
    fn destructive_requires_grant() {
        let mut reg = ToolRegistry::new();
        let mut spec = ToolSpec::new(ToolId(0), "rm", "删除");
        spec.risk = RiskLevel::Destructive;
        spec.idempotent = false;
        let id = reg.register(spec).unwrap();
        let call = ToolCall {
            trace_id: 2,
            tool: id,
            args: BTreeMap::new(),
            caller: 1,
        };
        assert!(matches!(
            reg.check(&call, &[], false),
            Err(ToolError::DestructiveNeedsGrant { .. })
        ));
        assert!(reg.check(&call, &[], true).is_ok());
        assert!(reg.acquire("rm", 1).is_ok());
        assert!(reg.acquire("rm", 2).is_err());
        reg.release("rm", 1);
        assert!(reg.acquire("rm", 2).is_ok());
    }

    #[test]
    fn builtin_init_is_idempotent() {
        init();
        init();
        let reg = tools().lock().unwrap();
        assert!(reg.find("fs.read").is_some());
        assert!(status().contains("tools="));
    }
}
