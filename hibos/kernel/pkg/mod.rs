//! 包管理器 —— 仓库索引、依赖解析与包安装。
//!
//! 对应《现代操作系统》中「软件分发与安装」的角色：把一个包及其依赖图
//! 安装到本地，并维护本地包数据库。
//!
//! 设计要点：
//! * `PackageManager`：刷新仓库索引、搜索、安装、移除、升级、列出已安装；
//! * 依赖解析使用拓扑排序并检测循环依赖；
//! * 本地包数据库为内存 + 可选 JSON 文件（手写最小实现，无 serde）；
//! * 网络下载被模拟为本地缓存，可在脱网环境运行。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// 包管理器错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PkgError {
    NotFound { name: String },
    AlreadyInstalled { name: String },
    DependencyMissing { name: String, dep: String },
    CycleDetected { cycle: Vec<String> },
    Io { reason: String },
    InvalidRepo { reason: String },
}

impl std::fmt::Display for PkgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgError::NotFound { name } => write!(f, "包不存在: {}", name),
            PkgError::AlreadyInstalled { name } => write!(f, "包已安装: {}", name),
            PkgError::DependencyMissing { name, dep } => {
                write!(f, "{} 缺少依赖 {}", name, dep)
            }
            PkgError::CycleDetected { cycle } => write!(f, "循环依赖: {:?}", cycle),
            PkgError::Io { reason } => write!(f, "IO 错误: {}", reason),
            PkgError::InvalidRepo { reason } => write!(f, "仓库无效: {}", reason),
        }
    }
}

impl std::error::Error for PkgError {}

/// 仓库索引中的一个包。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub size: u64,
}

impl Package {
    pub fn new(name: &str, version: &str, description: &str, deps: &[&str], size: u64) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            description: description.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            size,
        }
    }
}

/// 包管理器。
pub struct PackageManager {
    repo_url: String,
    cache_dir: PathBuf,
    db_path: PathBuf,
    /// 仓库索引：名字 -> 包。
    index: BTreeMap<String, Package>,
    /// 已安装包：名字 -> 版本。
    installed: BTreeMap<String, String>,
    pub install_count: u64,
    pub remove_count: u64,
}

impl PackageManager {
    /// 创建包管理器。
    pub fn new(repo_url: &str, cache_dir: &std::path::Path, db_path: &std::path::Path) -> Self {
        Self {
            repo_url: repo_url.to_string(),
            cache_dir: cache_dir.to_path_buf(),
            db_path: db_path.to_path_buf(),
            index: BTreeMap::new(),
            installed: BTreeMap::new(),
            install_count: 0,
            remove_count: 0,
        }
    }

    /// 使用内置演示索引（脱网可用）。
    pub fn with_builtin_index() -> Self {
        let mut m = Self::new(
            "https://repo.hibos.dev",
            std::path::Path::new("/tmp/hibos-pkg-cache"),
            std::path::Path::new("/tmp/hibos-pkg-db.json"),
        );
        m.index.insert(
            "coreutils".to_string(),
            Package::new("coreutils", "1.0.0", "基础用户态工具", &[], 1024),
        );
        m.index.insert(
            "net-tools".to_string(),
            Package::new("net-tools", "1.2.0", "网络诊断工具", &["coreutils"], 2048),
        );
        m.index.insert(
            "ai-models".to_string(),
            Package::new("ai-models", "0.3.1", "预训练模型包", &["coreutils"], 4096),
        );
        m
    }

    pub fn repo_url(&self) -> &str {
        &self.repo_url
    }

    pub fn cache_dir(&self) -> &std::path::Path {
        &self.cache_dir
    }

    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }

    /// 「刷新」索引：真实实现会下载仓库元数据；此处返回索引条目数。
    pub fn refresh(&mut self) -> Result<usize, PkgError> {
        if self.repo_url.is_empty() {
            return Err(PkgError::InvalidRepo {
                reason: "仓库地址为空".to_string(),
            });
        }
        Ok(self.index.len())
    }

    /// 搜索包（名字或描述包含关键词）。
    pub fn search(&self, query: &str) -> Vec<&Package> {
        self.index
            .values()
            .filter(|p| p.name.contains(query) || p.description.contains(query))
            .collect()
    }

    /// 按名查找。
    pub fn find(&self, name: &str) -> Option<&Package> {
        self.index.get(name)
    }

    /// 依赖拓扑排序（依赖在前），检测循环。
    pub fn resolve(&self, name: &str) -> Result<Vec<String>, PkgError> {
        let mut order = Vec::new();
        let mut visited = BTreeSet::new();
        let mut stack = Vec::new();
        self.resolve_inner(name, &mut visited, &mut stack, &mut order)?;
        Ok(order)
    }

    fn resolve_inner(
        &self,
        name: &str,
        visited: &mut BTreeSet<String>,
        stack: &mut Vec<String>,
        order: &mut Vec<String>,
    ) -> Result<(), PkgError> {
        if visited.contains(name) {
            return Ok(());
        }
        if stack.iter().any(|s| s == name) {
            let mut cycle = stack.clone();
            cycle.push(name.to_string());
            return Err(PkgError::CycleDetected { cycle });
        }
        let pkg = self.index.get(name).ok_or_else(|| PkgError::NotFound {
            name: name.to_string(),
        })?;
        stack.push(name.to_string());
        for dep in &pkg.dependencies {
            if !self.index.contains_key(dep) {
                return Err(PkgError::DependencyMissing {
                    name: name.to_string(),
                    dep: dep.clone(),
                });
            }
            self.resolve_inner(dep, visited, stack, order)?;
        }
        stack.pop();
        visited.insert(name.to_string());
        order.push(name.to_string());
        Ok(())
    }

    /// 安装包（含依赖）。返回实际安装的包列表（依赖在前）。
    pub fn install(&mut self, name: &str) -> Result<Vec<String>, PkgError> {
        if self.installed.contains_key(name) {
            return Err(PkgError::AlreadyInstalled {
                name: name.to_string(),
            });
        }
        let order = self.resolve(name)?;
        let mut newly = Vec::new();
        for pkg_name in order {
            if let Some(p) = self.index.get(&pkg_name) {
                self.installed
                    .insert(pkg_name.clone(), p.version.clone());
                self.install_count += 1;
                newly.push(pkg_name);
            }
        }
        Ok(newly)
    }

    /// 移除包（若有已安装包依赖它则拒绝）。
    pub fn remove(&mut self, name: &str) -> Result<(), PkgError> {
        if !self.installed.contains_key(name) {
            return Err(PkgError::NotFound {
                name: name.to_string(),
            });
        }
        for (other, _) in self.installed.iter() {
            if other == name {
                continue;
            }
            if let Some(p) = self.index.get(other) {
                if p.dependencies.iter().any(|d| d == name) {
                    return Err(PkgError::DependencyMissing {
                        name: other.clone(),
                        dep: name.to_string(),
                    });
                }
            }
        }
        self.installed.remove(name);
        self.remove_count += 1;
        Ok(())
    }

    /// 列出已安装包（名字@版本）。
    pub fn list(&self) -> Vec<String> {
        self.installed
            .iter()
            .map(|(n, v)| format!("{}@{}", n, v))
            .collect()
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.installed.contains_key(name)
    }

    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    /// 已安装包占用的总字节数。
    pub fn installed_size(&self) -> u64 {
        self.installed
            .keys()
            .filter_map(|n| self.index.get(n))
            .map(|p| p.size)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> PackageManager {
        PackageManager::with_builtin_index()
    }

    #[test]
    fn search_finds_packages() {
        let m = mgr();
        assert!(!m.search("net").is_empty());
        assert!(m.search("不存在的包").is_empty());
    }

    #[test]
    fn install_resolves_dependencies() {
        let mut m = mgr();
        let installed = m.install("net-tools").unwrap();
        assert_eq!(installed, vec!["coreutils", "net-tools"]);
        assert!(m.is_installed("coreutils"));
        assert_eq!(m.installed_count(), 2);
    }

    #[test]
    fn duplicate_install_rejected() {
        let mut m = mgr();
        m.install("coreutils").unwrap();
        assert!(matches!(
            m.install("coreutils"),
            Err(PkgError::AlreadyInstalled { .. })
        ));
    }

    #[test]
    fn remove_with_dependents_rejected() {
        let mut m = mgr();
        m.install("net-tools").unwrap();
        assert!(m.remove("coreutils").is_err());
        m.remove("net-tools").unwrap();
        assert!(m.remove("coreutils").is_ok());
    }

    #[test]
    fn cycle_detection() {
        let mut m = PackageManager::with_builtin_index();
        m.index.insert(
            "a".to_string(),
            Package::new("a", "1", "a", &["b"], 1),
        );
        m.index.insert(
            "b".to_string(),
            Package::new("b", "1", "b", &["a"], 1),
        );
        assert!(matches!(
            m.install("a"),
            Err(PkgError::CycleDetected { .. })
        ));
    }

    #[test]
    fn missing_dependency() {
        let mut m = PackageManager::with_builtin_index();
        m.index.insert(
            "x".to_string(),
            Package::new("x", "1", "x", &["ghost"], 1),
        );
        assert!(matches!(
            m.install("x"),
            Err(PkgError::DependencyMissing { .. })
        ));
    }

    #[test]
    fn list_and_size() {
        let mut m = mgr();
        m.install("coreutils").unwrap();
        let list = m.list();
        assert_eq!(list.len(), 1);
        assert_eq!(m.installed_size(), 1024);
    }
}
