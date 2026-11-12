//! Worktree 通道 —— AI 意识实例的**隔离工作区**。
//!
//! 每个 worktree 是一条独立的命名空间根：AI 意识实例 / 闭环循环可以在自己的
//! worktree 内创建文件、目录，彼此互不污染。所有操作通过统一通道
//! `worktree://trees` 暴露，AI 可直接 `sense` / `act`。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 一个 worktree（隔离工作区）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// worktree 名称（唯一）。
    pub name: String,
    /// 命名空间根路径。
    pub root: String,
    /// 归属者（AI 意识实例 / 循环标识）。
    pub owner: String,
}

/// worktree 注册表。
#[derive(Default)]
pub struct WorktreeRegistry {
    /// name -> worktree
    pub trees: BTreeMap<String, Worktree>,
    /// 当前激活的 worktree 名称。
    pub active: Option<String>,
}

impl WorktreeRegistry {
    pub fn new() -> Self {
        Self {
            trees: BTreeMap::new(),
            active: None,
        }
    }

    /// 创建一个 worktree；名称重复则拒绝。
    pub fn create(&mut self, name: &str, owner: &str) -> Result<Worktree, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("worktree 名称不能为空".to_string());
        }
        if name.contains('/') || name.contains("..") {
            return Err(format!("非法 worktree 名称: {}", name));
        }
        if self.trees.contains_key(name) {
            return Err(format!("worktree {} 已存在", name));
        }
        let tree = Worktree {
            name: name.to_string(),
            root: format!("/worktrees/{}", name),
            owner: if owner.trim().is_empty() {
                "default".to_string()
            } else {
                owner.trim().to_string()
            },
        };
        self.trees.insert(name.to_string(), tree.clone());
        if self.active.is_none() {
            self.active = Some(name.to_string());
        }
        Ok(tree)
    }

    /// 删除一个 worktree（激活中的会先取消激活）。
    pub fn remove(&mut self, name: &str) -> Result<Worktree, String> {
        let tree = self
            .trees
            .remove(name)
            .ok_or_else(|| format!("worktree {} 不存在", name))?;
        if self.active.as_deref() == Some(name) {
            self.active = self.trees.keys().next().cloned();
        }
        Ok(tree)
    }

    /// 切换当前激活的 worktree。
    pub fn switch(&mut self, name: &str) -> Result<Worktree, String> {
        let tree = self
            .trees
            .get(name)
            .cloned()
            .ok_or_else(|| format!("worktree {} 不存在", name))?;
        self.active = Some(name.to_string());
        Ok(tree)
    }

    /// 当前激活 worktree 的根路径（未激活则返回 "/"）。
    pub fn current_root(&self) -> String {
        self.active
            .as_ref()
            .and_then(|n| self.trees.get(n))
            .map(|t| t.root.clone())
            .unwrap_or_else(|| "/".to_string())
    }

    /// 快照文本。
    pub fn list(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "worktree {} 个 (active={})\n",
            self.trees.len(),
            self.active.as_deref().unwrap_or("none")
        ));
        for (name, t) in &self.trees {
            out.push_str(&format!(
                "  {}{} root={} owner={}\n",
                if self.active.as_deref() == Some(name.as_str()) {
                    "*"
                } else {
                    " "
                },
                t.name,
                t.root,
                t.owner
            ));
        }
        out.push_str(&format!("current_root={}\n", self.current_root()));
        out.trim_end().to_string()
    }
}

static WORKTREES: OnceLock<Mutex<WorktreeRegistry>> = OnceLock::new();

/// 全局 worktree 注册表。
pub fn worktrees() -> &'static Mutex<WorktreeRegistry> {
    WORKTREES.get_or_init(|| Mutex::new(WorktreeRegistry::new()))
}

/// 当前激活 worktree 的根路径（未激活则为 "/"）。
pub fn current_root() -> String {
    worktrees().lock().unwrap().current_root()
}

/// 把**相对路径**解析到当前 worktree 根下（绝对路径原样返回）。
///
/// 这是 worktree 与 VFS 文件操作真正隔离的关键：AI 在自己的 worktree 里
/// `write note.txt` 会落到 `<worktree_root>/note.txt`，而不会污染其他实例。
pub fn resolve_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() || p.starts_with('/') {
        return p.to_string();
    }
    let root = current_root();
    if root == "/" {
        return p.to_string();
    }
    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        p.trim_start_matches('/')
    )
}

/// 解析 `fs://vfs` 动作负载中的路径参数（write/read/ls/mkdir/rm/stat）。
pub fn resolve_fs_payload(payload: &str) -> String {
    let payload = payload.trim();
    let (cmd, rest) = match payload.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (payload, ""),
    };
    if !matches!(
        cmd,
        "write" | "read" | "ls" | "mkdir" | "rm" | "stat"
    ) {
        return payload.to_string();
    }
    let (path, tail) = match rest.split_once(char::is_whitespace) {
        Some((p, t)) => (p, t),
        None => (rest, ""),
    };
    let resolved = resolve_path(path);
    if tail.is_empty() {
        format!("{} {}", cmd, resolved)
    } else {
        format!("{} {} {}", cmd, resolved, tail)
    }
}

/// `sense worktree://trees`：返回 worktree 快照。
pub fn worktree_sense() -> String {
    worktrees().lock().unwrap().list()
}

/// `act worktree://trees <payload>`：
/// `create <名称> [归属]` | `remove <名称>` | `switch <名称>` | `list` | `current`
pub fn worktree_act(payload: &str) -> Result<String, String> {
    let mut w = worktrees().lock().unwrap();
    let parts: Vec<&str> = payload.split_whitespace().collect();
    match parts.first().copied() {
        Some("create") => {
            let name = parts.get(1).copied().unwrap_or("");
            let owner = parts.get(2).copied().unwrap_or("");
            let t = w.create(name, owner)?;
            Ok(format!(
                "已创建 worktree {} root={} owner={}",
                t.name, t.root, t.owner
            ))
        }
        Some("remove") | Some("rm") => {
            let name = parts.get(1).copied().unwrap_or("");
            let t = w.remove(name)?;
            Ok(format!("已删除 worktree {}", t.name))
        }
        Some("switch") | Some("use") => {
            let name = parts.get(1).copied().unwrap_or("");
            let t = w.switch(name)?;
            Ok(format!("已切换到 worktree {} root={}", t.name, t.root))
        }
        Some("list") => Ok(w.list()),
        Some("current") => Ok(format!(
            "active={} current_root={}",
            w.active.as_deref().unwrap_or("none"),
            w.current_root()
        )),
        _ => Err("用法: act worktree://trees create <名称> [归属] | remove <名称> | switch <名称> | list | current".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_switch_remove() {
        let mut r = WorktreeRegistry::new();
        let t = r.create("wt1", "loop-a").unwrap();
        assert_eq!(t.root, "/worktrees/wt1");
        assert_eq!(r.current_root(), "/worktrees/wt1");
        r.create("wt2", "loop-b").unwrap();
        r.switch("wt2").unwrap();
        assert_eq!(r.current_root(), "/worktrees/wt2");
        r.remove("wt2").unwrap();
        assert_eq!(r.current_root(), "/worktrees/wt1");
        assert!(r.create("wt1", "x").is_err());
    }

    #[test]
    fn rejects_illegal_name() {
        let mut r = WorktreeRegistry::new();
        assert!(r.create("", "x").is_err());
        assert!(r.create("../etc", "x").is_err());
        assert!(r.create("a/b", "x").is_err());
    }
}
