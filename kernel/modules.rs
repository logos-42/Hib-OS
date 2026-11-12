//! 内核模块系统 —— 支持热插拔的可扩展模块。

use std::collections::BTreeMap;

/// 模块名称。
pub type ModuleName = String;

/// 语义化版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 主版本相同即视为兼容。
    pub fn is_compatible_with(&self, other: &Version) -> bool {
        self.major == other.major
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 模块错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleError {
    InitializationFailed(String),
    CleanupFailed(String),
    NotLoaded,
    AlreadyLoaded,
    InvalidArgs,
    AccessDenied,
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::InitializationFailed(m) => write!(f, "模块初始化失败: {}", m),
            ModuleError::CleanupFailed(m) => write!(f, "模块清理失败: {}", m),
            ModuleError::NotLoaded => write!(f, "模块未加载"),
            ModuleError::AlreadyLoaded => write!(f, "模块已加载"),
            ModuleError::InvalidArgs => write!(f, "非法参数"),
            ModuleError::AccessDenied => write!(f, "访问被拒绝"),
        }
    }
}

/// 模块状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleState {
    Unloaded,
    Loading,
    Loaded,
    Unloading,
    Error,
}

impl ModuleState {
    pub fn name(self) -> &'static str {
        match self {
            ModuleState::Unloaded => "Unloaded",
            ModuleState::Loading => "Loading",
            ModuleState::Loaded => "Loaded",
            ModuleState::Unloading => "Unloading",
            ModuleState::Error => "Error",
        }
    }
}

/// 模块入口点。
#[derive(Clone)]
pub struct ModuleEntry {
    pub name: ModuleName,
    pub version: Version,
    pub init: fn() -> Result<(), ModuleError>,
    pub cleanup: fn(),
}

impl std::fmt::Debug for ModuleEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleEntry")
            .field("name", &self.name)
            .field("version", &self.version)
            .finish()
    }
}

/// 模块可导出的协议类别。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleProtocol {
    GpuDriver,
    Filesystem,
    Network,
    AiConsciousness,
}

/// 内核模块管理器。
pub struct KernelModuleManager {
    entries: BTreeMap<ModuleName, ModuleEntry>,
    states: BTreeMap<ModuleName, ModuleState>,
    refs: BTreeMap<ModuleName, u32>,
    protocols: BTreeMap<ModuleName, ModuleProtocol>,
}

impl KernelModuleManager {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            states: BTreeMap::new(),
            refs: BTreeMap::new(),
            protocols: BTreeMap::new(),
        }
    }

    /// 加载模块。
    pub fn load_module(
        &mut self,
        entry: ModuleEntry,
        protocol: ModuleProtocol,
    ) -> Result<(), ModuleError> {
        if self.entries.contains_key(&entry.name) {
            return Err(ModuleError::AlreadyLoaded);
        }
        self.states.insert(entry.name.clone(), ModuleState::Loading);
        match (entry.init)() {
            Ok(()) => {
                self.states.insert(entry.name.clone(), ModuleState::Loaded);
                self.refs.insert(entry.name.clone(), 1);
                self.protocols.insert(entry.name.clone(), protocol);
                println!(
                    "[module] 已加载 {} v{}",
                    entry.name, entry.version
                );
                self.entries.insert(entry.name.clone(), entry);
                Ok(())
            }
            Err(e) => {
                self.states.insert(entry.name.clone(), ModuleState::Error);
                Err(e)
            }
        }
    }

    /// 卸载模块 (尊重引用计数)。
    pub fn unload_module(&mut self, name: &str) -> Result<(), ModuleError> {
        let entry = self.entries.get(name).ok_or(ModuleError::NotLoaded)?.clone();
        let refs = self.refs.get(name).copied().unwrap_or(0);
        if refs > 1 {
            *self.refs.get_mut(name).unwrap() -= 1;
            return Ok(());
        }
        self.states.insert(name.to_string(), ModuleState::Unloading);
        (entry.cleanup)();
        self.entries.remove(name);
        self.states.insert(name.to_string(), ModuleState::Unloaded);
        self.refs.remove(name);
        self.protocols.remove(name);
        println!("[module] 已卸载 {}", name);
        Ok(())
    }

    pub fn add_ref(&mut self, name: &str) -> Result<(), ModuleError> {
        let r = self.refs.get_mut(name).ok_or(ModuleError::NotLoaded)?;
        *r += 1;
        Ok(())
    }

    pub fn remove_ref(&mut self, name: &str) {
        if let Some(r) = self.refs.get_mut(name) {
            *r = r.saturating_sub(1);
        }
    }

    pub fn get_entry(&self, name: &str) -> Option<&ModuleEntry> {
        self.entries.get(name)
    }

    pub fn get_state(&self, name: &str) -> Option<ModuleState> {
        self.states.get(name).copied()
    }

    /// 列出已加载模块。
    pub fn list_loaded(&self) -> Vec<&ModuleName> {
        self.entries.keys().collect()
    }

    /// 查询模块协议。
    pub fn protocol_of(&self, name: &str) -> Option<&ModuleProtocol> {
        self.protocols.get(name)
    }

    pub fn loaded_count(&self) -> usize {
        self.entries.len()
    }
}

impl Default for KernelModuleManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 模块初始化桩 (供内核加载器调用)。
pub fn hibos_module_init() -> Result<(), ModuleError> {
    Ok(())
}

/// 模块清理桩。
pub fn hibos_module_cleanup() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ModuleEntry {
        ModuleEntry {
            name: name.to_string(),
            version: Version::new(1, 0, 0),
            init: || Ok(()),
            cleanup: || {},
        }
    }

    #[test]
    fn load_and_unload() {
        let mut m = KernelModuleManager::new();
        m.load_module(entry("gpu"), ModuleProtocol::GpuDriver)
            .unwrap();
        assert_eq!(m.loaded_count(), 1);
        assert_eq!(m.get_state("gpu"), Some(ModuleState::Loaded));
        assert_eq!(m.protocol_of("gpu"), Some(&ModuleProtocol::GpuDriver));
        assert_eq!(m.list_loaded(), vec!["gpu"]);

        m.unload_module("gpu").unwrap();
        assert_eq!(m.loaded_count(), 0);
        assert_eq!(m.get_state("gpu"), Some(ModuleState::Unloaded));
    }

    #[test]
    fn duplicate_and_missing() {
        let mut m = KernelModuleManager::new();
        m.load_module(entry("dup"), ModuleProtocol::Network).unwrap();
        assert_eq!(
            m.load_module(entry("dup"), ModuleProtocol::Network),
            Err(ModuleError::AlreadyLoaded)
        );
        assert_eq!(m.unload_module("nope"), Err(ModuleError::NotLoaded));
    }

    #[test]
    fn init_failure_sets_error_state() {
        let mut m = KernelModuleManager::new();
        let bad = ModuleEntry {
            name: "bad".into(),
            version: Version::new(1, 0, 0),
            init: || Err(ModuleError::InitializationFailed("boom".into())),
            cleanup: || {},
        };
        assert!(m.load_module(bad, ModuleProtocol::Filesystem).is_err());
        assert_eq!(m.get_state("bad"), Some(ModuleState::Error));
        assert_eq!(m.loaded_count(), 0);
    }

    #[test]
    fn ref_counting_delays_unload() {
        let mut m = KernelModuleManager::new();
        m.load_module(entry("r"), ModuleProtocol::AiConsciousness)
            .unwrap();
        m.add_ref("r").unwrap();
        m.unload_module("r").unwrap();
        assert_eq!(m.loaded_count(), 1);
        m.unload_module("r").unwrap();
        assert_eq!(m.loaded_count(), 0);
    }

    #[test]
    fn version_compatibility() {
        assert!(Version::new(2, 1, 0).is_compatible_with(&Version::new(2, 9, 9)));
        assert!(!Version::new(2, 1, 0).is_compatible_with(&Version::new(3, 0, 0)));
    }
}

