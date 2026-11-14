//! 包管理器 —— 仓库索引、依赖解析与包安装（模块化版）。
//!
//! 与 `kernel/pkg/mod.rs` 并行存在，由主智能体统一集成到 `pkg/` 子树。
//! 本模块纯 Rust 实现，不依赖外部 crate，可在脱网用户态环境下独立编译与测试。
//!
//! 设计信条:
//! * 仓库优先：所有安装源自仓库索引，本地仅缓存与元数据；
//! * 依赖先行：安装前用拓扑排序解析依赖，检测循环依赖并拒绝；
//! * 原子感知：安装/移除/升级要么全部成功，要么保持前一致性状态；
//! * 能力沙箱友好：包操作本身不依赖 uid，仅依赖调用方显式能力校验；
//! * 可持久化：本地包数据库以手写最小 JSON 保存，脱网可重启恢复。
//!
//! 通道遵循 sense/act 对：`pkg/store` 通道的 `probe` 返回已安装包列表；
//! `act` 支持 install/remove/upgrade/search/list 操作（供 shell/sense 层调用）。
//!
//! 并发: 所有公共方法对内部状态加锁；读取为短锁快照，写入独占，优先于悲观锁。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ========================================================================
// 错误
// ========================================================================

/// 包管理器错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PkgError {
    /// 仓库与已安装中均未找到包。
    NotFound { name: String },
    /// 包已安装。
    AlreadyInstalled { name: String },
    /// 包未安装。
    NotInstalled { name: String },
    /// 依赖缺失。
    DependencyMissing { name: String, dep: String },
    /// 检测到循环依赖。
    CycleDetected { cycle: Vec<String> },
    /// IO 错误。
    Io { reason: String },
    /// 仓库无效。
    InvalidRepo { reason: String },
    /// 版本字符串非法。
    InvalidVersion { version: String },
    /// 已是最新版本。
    UpToDate { name: String },
}

impl std::fmt::Display for PkgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkgError::NotFound { name } => write!(f, "包不存在: {}", name),
            PkgError::AlreadyInstalled { name } => write!(f, "包已安装: {}", name),
            PkgError::NotInstalled { name } => write!(f, "包未安装: {}", name),
            PkgError::DependencyMissing { name, dep } => {
                write!(f, "{} 缺少依赖 {}", name, dep)
            }
            PkgError::CycleDetected { cycle } => write!(f, "循环依赖: {}", cycle.join(" -> ")),
            PkgError::Io { reason } => write!(f, "IO 错误: {}", reason),
            PkgError::InvalidRepo { reason } => write!(f, "仓库无效: {}", reason),
            PkgError::InvalidVersion { version } => write!(f, "无效版本: {}", version),
            PkgError::UpToDate { name } => write!(f, "已是最新版本: {}", name),
        }
    }
}

impl std::error::Error for PkgError {}

pub type PkgResult<T> = Result<T, PkgError>;

// ========================================================================
// 版本（语义化简化版：major.minor.patch）
// ========================================================================

/// 语义化版本，支持比较与字符串解析。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// 构造版本。
    pub fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch }
    }

    /// 解析 "major"、"major.minor" 或 "major.minor.patch" 字符串（缺省 minor/patch 为 0）。
    pub fn parse(s: &str) -> PkgResult<Version> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.is_empty() || parts.len() > 3 {
            return Err(PkgError::InvalidVersion { version: s.to_string() });
        }
        let major = parts[0]
            .parse::<u64>()
            .map_err(|_| PkgError::InvalidVersion { version: s.to_string() })?;
        let minor = if parts.len() >= 2 {
            parts[1]
                .parse::<u64>()
                .map_err(|_| PkgError::InvalidVersion { version: s.to_string() })?
        } else {
            0
        };
        let patch = if parts.len() == 3 {
            parts[2]
                .parse::<u64>()
                .map_err(|_| PkgError::InvalidVersion { version: s.to_string() })?
        } else {
            0
        };
        Ok(Version::new(major, minor, patch))
    }

    /// 返回 "major.minor.patch" 字符串。
    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

// ========================================================================
// 包
// ========================================================================

/// 仓库索引中的一个包。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: Version,
    pub description: String,
    pub dependencies: Vec<String>,
    pub size: u64,
    pub installed: bool,
}

impl Package {
    /// 创建一个包。
    pub fn new(
        name: &str,
        version: &str,
        description: &str,
        deps: &[&str],
        size: u64,
    ) -> PkgResult<Self> {
        Ok(Self {
            name: name.to_string(),
            version: Version::parse(version)?,
            description: description.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            size,
            installed: false,
        })
    }

    /// 返回 "name@version"。
    pub fn display_name(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

// ========================================================================
// 仓库
// ========================================================================

/// 一个软件仓库：名称、URL、索引（名字 -> 包）。
#[derive(Clone, Debug)]
pub struct Repository {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub index: BTreeMap<String, Package>,
}

impl Repository {
    pub fn new(name: &str, url: &str) -> Self {
        Self {
            name: name.to_string(),
            url: url.to_string(),
            enabled: true,
            index: BTreeMap::new(),
        }
    }

    /// 插入或更新索引中的一个包。
    pub fn upsert(&mut self, pkg: Package) {
        self.index.insert(pkg.name.clone(), pkg);
    }

    /// 索引条目数。
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

// ========================================================================
// 包管理器
// ========================================================================

/// 包管理器：维护多个仓库、本地已安装数据库与缓存目录。
pub struct PackageManager {
    /// 仓库名 -> 仓库。
    repos: BTreeMap<String, Repository>,
    /// 下载缓存目录。
    cache_dir: PathBuf,
    /// 本地包数据库路径（JSON 文本）。
    db_path: PathBuf,
    /// 已安装包：名字 -> 包（含已安装标记）。
    installed: BTreeMap<String, Package>,
    pub install_count: u64,
    pub remove_count: u64,
    pub upgrade_count: u64,
}

impl PackageManager {
    /// 创建包管理器，指定缓存目录与数据库路径。
    pub fn new(cache_dir: &Path, db_path: &Path) -> Self {
        Self {
            repos: BTreeMap::new(),
            cache_dir: cache_dir.to_path_buf(),
            db_path: db_path.to_path_buf(),
            installed: BTreeMap::new(),
            install_count: 0,
            remove_count: 0,
            upgrade_count: 0,
        }
    }

    /// 使用内置演示索引（脱网可用）。
    pub fn with_builtin_index() -> Self {
        let mut m = Self::new(
            Path::new("/tmp/hibos-pkg-cache"),
            Path::new("/tmp/hibos-pkg-db.json"),
        );
        m.add_repo("core", "https://repo.hibos.dev/core").unwrap();
        m.repos.get_mut("core").unwrap().upsert(
            Package::new("coreutils", "1.0.0", "基础用户态工具", &[], 1024).unwrap(),
        );
        m.repos.get_mut("core").unwrap().upsert(
            Package::new("net-tools", "1.2.0", "网络诊断工具", &["coreutils"], 2048).unwrap(),
        );
        m.repos.get_mut("core").unwrap().upsert(
            Package::new(
                "ai-models",
                "0.3.1",
                "预训练模型包",
                &["coreutils"],
                4096,
            )
            .unwrap(),
        );
        m.repos.get_mut("core").unwrap().upsert(
            Package::new("libc", "2.42", "C 标准库", &[], 3072).unwrap(),
        );
        m.repos.get_mut("core").unwrap().upsert(
            Package::new("libssl", "3.2.0", "TLS/SSL 库", &["libc"], 2048).unwrap(),
        );
        m
    }

    /// 添加一个仓库。
    pub fn add_repo(&mut self, name: &str, url: &str) -> PkgResult<()> {
        if self.repos.contains_key(name) {
            return Err(PkgError::InvalidRepo {
                reason: format!("仓库已存在: {}", name),
            });
        }
        if url.is_empty() {
            return Err(PkgError::InvalidRepo {
                reason: "仓库地址为空".to_string(),
            });
        }
        self.repos.insert(name.to_string(), Repository::new(name, url));
        Ok(())
    }

    /// 移除一个仓库（其索引包将不再可用）。
    pub fn remove_repo(&mut self, name: &str) -> PkgResult<()> {
        if self.repos.remove(name).is_none() {
            return Err(PkgError::InvalidRepo {
                reason: format!("仓库不存在: {}", name),
            });
        }
        Ok(())
    }

    /// 列出所有仓库名。
    pub fn repo_names(&self) -> Vec<String> {
        self.repos.keys().cloned().collect()
    }

    /// 「刷新」全部启用仓库索引：真实实现会下载仓库元数据；此处返回索引条目总数。
    pub fn refresh(&mut self) -> PkgResult<usize> {
        let mut total = 0usize;
        for repo in self.repos.values_mut() {
            if !repo.enabled {
                continue;
            }
            if repo.url.is_empty() {
                return Err(PkgError::InvalidRepo {
                    reason: format!("仓库 {} 地址为空", repo.name),
                });
            }
            total += repo.index.len();
        }
        Ok(total)
    }

    /// 在全部仓库索引中搜索（名字或描述包含关键词）。
    pub fn search(&self, query: &str) -> Vec<&Package> {
        let q = query.to_lowercase();
        self.repos
            .values()
            .flat_map(|r| r.index.values())
            .filter(|p| p.name.to_lowercase().contains(&q) || p.description.to_lowercase().contains(&q))
            .collect()
    }

    /// 按名在全部仓库中查找（先启用的优先）。
    pub fn find(&self, name: &str) -> Option<&Package> {
        for repo in self.repos.values() {
            if repo.enabled {
                if let Some(p) = repo.index.get(name) {
                    return Some(p);
                }
            }
        }
        None
    }

    /// 依赖拓扑排序（依赖在前），检测循环。只在启用仓库的索引中解析。
    pub fn resolve(&self, name: &str) -> PkgResult<Vec<String>> {
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
    ) -> PkgResult<()> {
        if visited.contains(name) {
            return Ok(());
        }
        if stack.iter().any(|s| s == name) {
            let mut cycle = stack.clone();
            cycle.push(name.to_string());
            return Err(PkgError::CycleDetected { cycle });
        }
        let pkg = self.find(name).ok_or_else(|| PkgError::NotFound {
            name: name.to_string(),
        })?;
        stack.push(name.to_string());
        for dep in &pkg.dependencies {
            if self.find(dep).is_none() {
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
    pub fn install(&mut self, name: &str) -> PkgResult<Vec<String>> {
        if self.installed.contains_key(name) {
            return Err(PkgError::AlreadyInstalled {
                name: name.to_string(),
            });
        }
        let order = self.resolve(name)?;
        let mut newly = Vec::new();
        for pkg_name in &order {
            // 先把包克隆为所有值，避免在 insert 的可变借用期间持有 find 的不可变借用。
            let pkg = self.find(pkg_name).cloned();
            if let Some(pkg) = pkg {
                let mut installed_pkg = pkg;
                installed_pkg.installed = true;
                self.installed.insert(pkg_name.clone(), installed_pkg);
                self.install_count += 1;
                newly.push(pkg_name.clone());
            }
        }
        let _ = self.save_db();
        Ok(newly)
    }

    /// 移除包（若有已安装包依赖它则拒绝）。
    pub fn remove(&mut self, name: &str) -> PkgResult<()> {
        if !self.installed.contains_key(name) {
            return Err(PkgError::NotInstalled {
                name: name.to_string(),
            });
        }
        // 先检查依赖关系（不可变借用），在可变 remove 之前完成，避免借用冲突。
        let dependent = self
            .installed
            .iter()
            .find_map(|(other, pkg)| {
                if other != name && pkg.dependencies.iter().any(|d| d == name) {
                    Some(other.clone())
                } else {
                    None
                }
            });
        if let Some(other) = dependent {
            return Err(PkgError::DependencyMissing {
                name: other,
                dep: name.to_string(),
            });
        }
        self.installed.remove(name);
        self.remove_count += 1;
        let _ = self.save_db();
        Ok(())
    }

    /// 升级单个包到仓库中的最新版本（同名、更高版本）。不自动升级依赖。
    pub fn upgrade(&mut self, name: &str) -> PkgResult<Option<Version>> {
        let installed = self
            .installed
            .get(name)
            .ok_or_else(|| PkgError::NotInstalled {
                name: name.to_string(),
            })?
            .clone();
        // find 返回的是对 self 的不可变借用；先克隆为所有值，使借用在可变操作前结束。
        let repo_pkg = self
            .find(name)
            .cloned()
            .ok_or_else(|| PkgError::NotFound {
                name: name.to_string(),
            })?;
        if repo_pkg.version <= installed.version {
            return Err(PkgError::UpToDate {
                name: name.to_string(),
            });
        }
        let mut new_pkg = repo_pkg.clone();
        new_pkg.installed = true;
        self.installed.insert(name.to_string(), new_pkg);
        self.upgrade_count += 1;
        let _ = self.save_db();
        Ok(Some(repo_pkg.version))
    }

    /// 升级全部可升级的已安装包。返回 (名字, 旧版本, 新版本) 列表。
    pub fn upgrade_all(&mut self) -> PkgResult<Vec<(String, Version, Version)>> {
        let mut upgraded = Vec::new();
        // 先快照已安装包名，避免在可变借用期间迭代集合；再取旧版本（upgrade
        // 会替换版本，必须在调用前读取）。
        let names: Vec<String> = self.installed.keys().cloned().collect();
        for name in names {
            let old_ver = self.installed.get(&name).unwrap().version.clone();
            match self.upgrade(&name) {
                Ok(Some(new_ver)) => {
                    upgraded.push((name.clone(), old_ver, new_ver));
                }
                Ok(None) | Err(PkgError::UpToDate { .. }) | Err(PkgError::NotInstalled { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(upgraded)
    }

    /// 列出已安装包（"name@version"）。
    pub fn list(&self) -> Vec<String> {
        self.installed
            .iter()
            .map(|(_n, p)| p.display_name())
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
        self.installed.values().map(|p| p.size).sum()
    }

    /// 缓存目录。
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// 数据库路径。
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 将已安装数据库持久化到磁盘（手写最小 JSON，无 serde）。
    pub fn save_db(&self) -> std::io::Result<()> {
        if let Some(parent) = self.db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut lines = Vec::new();
        for (name, pkg) in &self.installed {
            lines.push(format!(
                "{{\"name\":{},\"version\":{},\"size\":{}}}",
                json_escape(name),
                json_escape(&pkg.version.to_string()),
                pkg.size
            ));
        }
        let content = if lines.is_empty() {
            String::new()
        } else {
            lines.join("\n") + "\n"
        };
        fs::write(&self.db_path, content)
    }

    /// 从磁盘加载已安装数据库。
    pub fn load_db(&mut self) -> std::io::Result<()> {
        let data = match fs::read_to_string(&self.db_path) {
            Ok(d) => d,
            Err(_) => return Ok(()),
        };
        self.installed.clear();
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(pkg) = parse_db_line(line) {
                self.installed.insert(pkg.name.clone(), pkg);
            }
        }
        Ok(())
    }
}

impl Default for PackageManager {
    fn default() -> Self {
        Self::new(Path::new("/tmp/hibos-pkg-cache"), Path::new("/tmp/hibos-pkg-db.json"))
    }
}

// ========================================================================
// sense/act 通道接入（供 shell / sense 层）
// ========================================================================

/// `pkg/store` 通道的 probe：返回已安装包列表的快照。
pub fn pkg_probe(_name: &str) -> Option<String> {
    // 全局单例惰性初始化（读取无副作用）。
    let mgr = package_manager();
    let g = mgr.lock().unwrap();
    if g.installed.is_empty() {
        return Some("(无已安装包)".to_string());
    }
    Some(g.list().join("\n"))
}

/// `pkg/store` 通道的 act：执行包管理操作。
///
/// 支持的负载:
/// * `install <名>` —— 安装包（含依赖）
/// * `remove <名>` —— 移除包
/// * `upgrade <名>` —— 升级包
/// * `search <关键词>` —— 搜索仓库
/// * `list` —— 列出已安装
/// * `refresh` —— 刷新仓库索引
pub fn pkg_act(_name: &str, payload: &str) -> Option<Result<String, String>> {
    let mgr = package_manager();
    let mut g = mgr.lock().unwrap();
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("install") => {
            let name = match it.next() {
                Some(n) => n,
                None => return Some(Err("用法: install <名>".to_string())),
            };
            Some(match g.install(name) {
                Ok(newly) => Ok(format!("已安装: {}", newly.join(", "))),
                Err(e) => Err(e.to_string()),
            })
        }
        Some("remove") => {
            let name = match it.next() {
                Some(n) => n,
                None => return Some(Err("用法: remove <名>".to_string())),
            };
            Some(match g.remove(name) {
                Ok(()) => Ok(format!("已移除: {}", name)),
                Err(e) => Err(e.to_string()),
            })
        }
        Some("upgrade") => {
            let name = match it.next() {
                Some(n) => n,
                None => return Some(Err("用法: upgrade <名>".to_string())),
            };
            Some(match g.upgrade(name) {
                Ok(Some(v)) => Ok(format!("{} 已升级到 {}", name, v)),
                Ok(None) => Ok(format!("{} 已是最新", name)),
                Err(e) => Err(e.to_string()),
            })
        }
        Some("search") => {
            let q = match it.next() {
                Some(n) => n,
                None => return Some(Err("用法: search <关键词>".to_string())),
            };
            let results = g.search(q);
            Some(if results.is_empty() {
                Ok("未找到匹配包".to_string())
            } else {
                Ok(results
                    .iter()
                    .map(|p| p.display_name())
                    .collect::<Vec<_>>()
                    .join("\n"))
            })
        }
        Some("list") => Some(Ok(g.list().join("\n"))),
        Some("refresh") => Some(match g.refresh() {
            Ok(n) => Ok(format!("索引已刷新，{} 个条目", n)),
            Err(e) => Err(e.to_string()),
        }),
        _ => Some(Err(
            "用法: install|remove|upgrade|search|list|refresh <参数>".to_string(),
        )),
    }
}

// 全局单例（供 sense/act 通道与其他模块使用）。
static PKG_MANAGER: OnceLock<Mutex<PackageManager>> = OnceLock::new();

/// 取得全局包管理器单例。
pub fn package_manager() -> &'static Mutex<PackageManager> {
    PKG_MANAGER.get_or_init(|| Mutex::new(PackageManager::with_builtin_index()))
}

// ========================================================================
// JSON 持久化（手写最小实现，无 serde）
// ========================================================================

/// 对字符串做 JSON 转义（带引号）。
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 从一行数据库 JSON 反序列化包（宽松解析）。
fn parse_db_line(line: &str) -> Option<Package> {
    let get_str = |key: &str, s: &str| -> Option<String> {
        let pattern = format!("\"{}\":", key);
        let start = s.find(&pattern)? + pattern.len();
        // 跳过可选空格与引号
        let s = &s[start..];
        let start = s.find('"')? + 1;
        let end = s[start..].find('"')?;
        Some(s[start..start + end].to_string())
    };
    let get_u64 = |key: &str, s: &str| -> Option<u64> {
        let pattern = format!("\"{}\":", key);
        let start = s.find(&pattern)? + pattern.len();
        let end = s[start..]
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(0);
        s[start..start + end].parse().ok()
    };
    let name = get_str("name", line)?;
    let version_str = get_str("version", line)?;
    let size = get_u64("size", line).unwrap_or(0);
    let version = Version::parse(&version_str).ok()?;
    Some(Package {
        name,
        version,
        description: String::new(),
        dependencies: Vec::new(),
        size,
        installed: true,
    })
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> PackageManager {
        PackageManager::with_builtin_index()
    }

    #[test]
    fn add_remove_repo() {
        let mut m = mgr();
        assert!(m.add_repo("extra", "https://repo.hibos.dev/extra").is_ok());
        assert!(m.repo_names().contains(&"extra".to_string()));
        assert!(m.add_repo("extra", "https://x").is_err()); // 重复
        assert!(m.remove_repo("extra").is_ok());
        assert!(!m.repo_names().contains(&"extra".to_string()));
        assert!(m.remove_repo("nope").is_err());
    }

    #[test]
    fn refresh_counts_index() {
        let mut m = mgr();
        assert_eq!(m.refresh().unwrap(), 5);
    }

    #[test]
    fn search_finds_packages() {
        let m = mgr();
        assert!(!m.search("net").is_empty());
        assert!(!m.search("ai").is_empty());
        assert!(m.search("不存在的包").is_empty());
    }

    #[test]
    fn install_resolves_dependencies() {
        let mut m = mgr();
        let installed = m.install("net-tools").unwrap();
        assert_eq!(installed, vec!["coreutils", "net-tools"]);
        assert!(m.is_installed("coreutils"));
        assert_eq!(m.installed_count(), 2);
        assert_eq!(m.installed_size(), 1024 + 2048);
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
        // 构造两个互相依赖的包：手动在索引中插入。
        let mut m = PackageManager::new(
            Path::new("/tmp/hibos-pkg-cache-test"),
            Path::new("/tmp/hibos-pkg-db-test.json"),
        );
        m.add_repo("core", "https://repo.hibos.dev/core").unwrap();
        let repo = m.repos.get_mut("core").unwrap();
        repo.upsert(Package::new("a", "1", "a", &["b"], 1).unwrap());
        repo.upsert(Package::new("b", "1", "b", &["a"], 1).unwrap());
        assert!(matches!(
            m.install("a"),
            Err(PkgError::CycleDetected { .. })
        ));
    }

    #[test]
    fn missing_dependency() {
        let mut m = PackageManager::new(
            Path::new("/tmp/hibos-pkg-cache-test2"),
            Path::new("/tmp/hibos-pkg-db-test2.json"),
        );
        m.add_repo("core", "https://repo.hibos.dev/core").unwrap();
        let repo = m.repos.get_mut("core").unwrap();
        repo.upsert(Package::new("x", "1", "x", &["ghost"], 1).unwrap());
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
        assert_eq!(list[0], "coreutils@1.0.0");
        assert_eq!(m.installed_size(), 1024);
    }

    #[test]
    fn version_parse_and_compare() {
        let v1 = Version::parse("1.2.3").unwrap();
        let v2 = Version::parse("1.2.4").unwrap();
        let v3 = Version::parse("1.2.3").unwrap();
        assert!(v1 < v2);
        assert_eq!(v1, v3);
        assert_eq!(v1.to_string(), "1.2.3");
        assert!(Version::parse("1.2").is_ok());
        assert!(Version::parse("1").is_ok());
        assert!(Version::parse("").is_err());
        assert!(Version::parse("1.a.3").is_err());
    }

    #[test]
    fn upgrade_and_up_to_date() {
        let mut m = mgr();
        m.install("coreutils").unwrap();
        // 仓库中 coreutils 只有 1.0.0，已安装也是 1.0.0 -> UpToDate。
        assert!(matches!(
            m.upgrade("coreutils"),
            Err(PkgError::UpToDate { .. })
        ));
        // 未安装的包不能 upgrade。
        assert!(matches!(
            m.upgrade("nope"),
            Err(PkgError::NotInstalled { .. })
        ));
    }

    #[test]
    fn not_found_and_not_installed() {
        let m = mgr();
        assert!(m.find("nope").is_none());
        assert!(!m.is_installed("coreutils"));
        let mut m2 = mgr();
        assert!(matches!(
            m2.remove("coreutils"),
            Err(PkgError::NotInstalled { .. })
        ));
    }

    #[test]
    fn save_and_load_db() {
        let mut m = PackageManager::new(
            Path::new("/tmp/hibos-pkg-cache-test3"),
            Path::new("/tmp/hibos-pkg-db-test3.json"),
        );
        m.add_repo("core", "https://repo.hibos.dev/core").unwrap();
        let repo = m.repos.get_mut("core").unwrap();
        repo.upsert(Package::new("coreutils", "1.0.0", "基础工具", &[], 1024).unwrap());
        let _ = std::fs::remove_file(m.db_path());
        m.install("coreutils").unwrap();
        m.save_db().unwrap();
        let mut m2 = PackageManager::new(
            Path::new("/tmp/hibos-pkg-cache-test3"),
            Path::new("/tmp/hibos-pkg-db-test3.json"),
        );
        m2.load_db().unwrap();
        assert!(m2.is_installed("coreutils"));
        let _ = std::fs::remove_file(m.db_path());
    }

    #[test]
    fn pkg_act_install_list() {
        // 确保全局单例干净：先移除（忽略结果），避免受前序测试污染。
        let _ = pkg_act("pkg/store", "remove coreutils");
        let out = pkg_act("pkg/store", "install coreutils").unwrap().unwrap();
        assert!(out.contains("coreutils"));
        let list = pkg_act("pkg/store", "list").unwrap().unwrap();
        assert!(list.contains("coreutils"));
        // 重复安装失败。
        let err = pkg_act("pkg/store", "install coreutils").unwrap().unwrap_err();
        assert!(err.contains("已安装"));
        let _ = pkg_act("pkg/store", "remove coreutils").unwrap();
    }

    #[test]
    fn pkg_probe_snapshot() {
        let _ = pkg_act("pkg/store", "install net-tools").unwrap();
        let snap = pkg_probe("pkg/store").unwrap();
        assert!(snap.contains("net-tools"));
        assert!(snap.contains("coreutils"));
        let _ = pkg_act("pkg/store", "remove net-tools").unwrap();
        let _ = pkg_act("pkg/store", "remove coreutils").unwrap();
    }
}
