//! 自造插件接口 —— 让 AI 与用户都能**注册自造插件**，并统一纳管。
//!
//! 与 `modules.rs` 的内核模块管理器不同，这里面向"自造插件"：
//! * 插件声明自身协议（kind）、能力位（caps）与对外暴露的通道（channel）；
//! * 安装后即拥有自己的通道 `plugin://<name>`，可被 AI 直接 `sense` / `act`；
//! * 支持热重载（hot reload）：卸载后按原规格重装，并记录重载次数。
//!
//! 统一入口：`plugin://modules`（安装/卸载/列表）、`plugin://hotreload`（热重载）。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 插件协议种类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PluginKind {
    Gpu,
    Network,
    Filesystem,
    Ai,
    Generic,
}

impl PluginKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginKind::Gpu => "gpu",
            PluginKind::Network => "network",
            PluginKind::Filesystem => "filesystem",
            PluginKind::Ai => "ai",
            PluginKind::Generic => "generic",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "gpu" => Some(PluginKind::Gpu),
            "network" | "net" => Some(PluginKind::Network),
            "filesystem" | "fs" => Some(PluginKind::Filesystem),
            "ai" => Some(PluginKind::Ai),
            "generic" | "user" => Some(PluginKind::Generic),
            _ => None,
        }
    }
}

/// 自造插件规格。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSpec {
    /// 插件名（唯一）。
    pub name: String,
    /// 版本号。
    pub version: String,
    /// 协议种类。
    pub kind: PluginKind,
    /// 能力位（caps 权限位掩码）。
    pub caps: u32,
    /// 对外暴露的通道名（默认 `plugin://<name>`）。
    pub channel: String,
    /// 来源：builtin（内置）/ user（用户自造）。
    pub source: String,
}

impl PluginSpec {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "0.2.0".to_string(),
            kind: PluginKind::Generic,
            caps: 0,
            channel: format!("plugin://{}", name),
            source: "user".to_string(),
        }
    }
}

/// 插件注册表。
#[derive(Default)]
pub struct PluginRegistry {
    /// name -> spec
    pub plugins: BTreeMap<String, PluginSpec>,
    /// 插件收到的 act 负载（用于状态回读）。
    pub actions: BTreeMap<String, Vec<String>>,
    /// 热重载次数。
    pub hot_reloads: u64,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: BTreeMap::new(),
            actions: BTreeMap::new(),
            hot_reloads: 0,
        }
    }

    /// 安装一个自造插件（同名拒绝）。
    pub fn install(&mut self, spec: PluginSpec) -> Result<PluginSpec, String> {
        if spec.name.trim().is_empty() {
            return Err("插件名不能为空".to_string());
        }
        if self.plugins.contains_key(&spec.name) {
            return Err(format!("插件 {} 已安装", spec.name));
        }
        let installed = spec.clone();
        self.plugins.insert(spec.name.clone(), spec);
        self.actions.entry(installed.name.clone()).or_default();
        Ok(installed)
    }

    /// 卸载插件。
    pub fn uninstall(&mut self, name: &str) -> Result<PluginSpec, String> {
        let spec = self
            .plugins
            .remove(name)
            .ok_or_else(|| format!("插件 {} 未安装", name))?;
        self.actions.remove(name);
        Ok(spec)
    }

    /// 热重载：按原规格卸载后重装，并计数。
    pub fn hot_reload(&mut self, name: &str) -> Result<PluginSpec, String> {
        let spec = self
            .plugins
            .get(name)
            .cloned()
            .ok_or_else(|| format!("插件 {} 未安装", name))?;
        self.plugins.remove(name);
        self.plugins.insert(spec.name.clone(), spec.clone());
        self.hot_reloads += 1;
        Ok(spec)
    }

    /// 记录一次对插件的行动（插件通道 act 的落点）。
    pub fn record_action(&mut self, name: &str, payload: &str) -> Result<String, String> {
        if !self.plugins.contains_key(name) {
            return Err(format!("插件 {} 未安装", name));
        }
        let entry = self.actions.entry(name.to_string()).or_default();
        entry.push(payload.to_string());
        if entry.len() > 64 {
            entry.remove(0);
        }
        Ok(format!(
            "plugin {} 已接收行动: {} (累计 {} 次)",
            name,
            payload,
            entry.len()
        ))
    }

    /// 快照文本。
    pub fn list(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "插件 {} 个 (热重载 {} 次)\n",
            self.plugins.len(),
            self.hot_reloads
        ));
        for (name, p) in &self.plugins {
            out.push_str(&format!(
                "  {} v{} kind={} caps=0x{:02x} channel={} source={}\n",
                p.name, p.version, p.kind.as_str(), p.caps, p.channel, p.source
            ));
            if let Some(log) = self.actions.get(name) {
                if !log.is_empty() {
                    out.push_str(&format!("    最近行动: {}\n", log.last().unwrap()));
                }
            }
        }
        out.trim_end().to_string()
    }

    /// 按通道名找插件（`plugin://<name>` → 插件名）。
    pub fn plugin_of_channel(&self, channel: &str) -> Option<&PluginSpec> {
        let name = channel.strip_prefix("plugin://")?;
        self.plugins.get(name)
    }
}

static PLUGINS: OnceLock<Mutex<PluginRegistry>> = OnceLock::new();

/// 全局插件注册表。
pub fn plugins() -> &'static Mutex<PluginRegistry> {
    PLUGINS.get_or_init(|| Mutex::new(PluginRegistry::new()))
}

/// `sense plugin://modules`：插件快照。
pub fn plugin_sense() -> String {
    plugins().lock().unwrap().list()
}

/// `sense plugin://hotreload`：热重载状态。
pub fn plugin_hotreload_sense() -> String {
    let p = plugins().lock().unwrap();
    format!("hot_reload count={} plugins={}", p.hot_reloads, p.plugins.len())
}

/// `sense plugin://<name>`：单个自造插件状态。
pub fn plugin_sense_one(channel: &str) -> Option<String> {
    let p = plugins().lock().unwrap();
    let spec = p.plugin_of_channel(channel)?;
    let acts = p.actions.get(&spec.name).map(|v| v.len()).unwrap_or(0);
    Some(format!(
        "plugin {} v{} kind={} caps=0x{:02x} channel={} source={} actions={}",
        spec.name,
        spec.version,
        spec.kind.as_str(),
        spec.caps,
        spec.channel,
        spec.source,
        acts
    ))
}

/// `act plugin://modules <payload>`：
/// `install <名称> [版本] [kind] [caps]` | `uninstall <名称>` | `list`
pub fn plugin_act_modules(payload: &str) -> Result<String, String> {
    let mut p = plugins().lock().unwrap();
    let parts: Vec<&str> = payload.split_whitespace().collect();
    match parts.first().copied() {
        Some("install") | Some("add") => {
            let name = parts.get(1).copied().unwrap_or("");
            let mut spec = PluginSpec::new(name);
            if let Some(v) = parts.get(2) {
                spec.version = v.to_string();
            }
            if let Some(k) = parts.get(3) {
                spec.kind = PluginKind::from_str(k).unwrap_or(PluginKind::Generic);
            }
            if let Some(c) = parts.get(4) {
                spec.caps = u32::from_str_radix(c.trim_start_matches("0x"), 16).unwrap_or(0);
            }
            let installed = p.install(spec)?;
            Ok(format!(
                "已安装插件 {} v{} kind={} channel={}",
                installed.name,
                installed.version,
                installed.kind.as_str(),
                installed.channel
            ))
        }
        Some("uninstall") | Some("remove") | Some("rm") => {
            let name = parts.get(1).copied().unwrap_or("");
            let s = p.uninstall(name)?;
            Ok(format!("已卸载插件 {} v{}", s.name, s.version))
        }
        Some("list") => Ok(p.list()),
        _ => Err("用法: act plugin://modules install <名称> [版本] [kind] [caps] | uninstall <名称> | list".to_string()),
    }
}

/// `act plugin://hotreload <payload>`：`reload <名称>` | `status`
pub fn plugin_act_hotreload(payload: &str) -> Result<String, String> {
    let mut p = plugins().lock().unwrap();
    let parts: Vec<&str> = payload.split_whitespace().collect();
    match parts.first().copied() {
        Some("reload") => {
            let name = parts.get(1).copied().unwrap_or("");
            let s = p.hot_reload(name)?;
            Ok(format!(
                "已热重载插件 {} v{} (累计 {} 次)",
                s.name,
                s.version,
                p.hot_reloads
            ))
        }
        Some("status") => Ok(plugin_hotreload_sense()),
        _ => Err("用法: act plugin://hotreload reload <名称> | status".to_string()),
    }
}

/// `act plugin://<name> <payload>`：把负载投递给自造插件并记录。
pub fn plugin_act_dynamic(channel: &str, payload: &str) -> Result<String, String> {
    let name = channel
        .strip_prefix("plugin://")
        .ok_or_else(|| format!("非法插件通道: {}", channel))?
        .to_string();
    plugins().lock().unwrap().record_action(&name, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_act() {
        let mut r = PluginRegistry::new();
        let spec = r.install(PluginSpec::new("demo")).unwrap();
        assert_eq!(spec.channel, "plugin://demo");
        assert!(r.install(PluginSpec::new("demo")).is_err());
        assert!(r.record_action("demo", "hello").is_ok());
        assert!(r.record_action("nope", "x").is_err());
        assert_eq!(r.plugin_of_channel("plugin://demo").unwrap().name, "demo");
    }

    #[test]
    fn hot_reload_keeps_spec() {
        let mut r = PluginRegistry::new();
        r.install(PluginSpec::new("hot")).unwrap();
        r.hot_reload("hot").unwrap();
        assert_eq!(r.hot_reloads, 1);
        assert!(r.plugins.contains_key("hot"));
        assert!(r.hot_reload("missing").is_err());
    }
}
