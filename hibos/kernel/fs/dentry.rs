//! dentry 目录项 —— P0-3 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::fs::inode::{InodeType, alloc_inode, get_inode, get_inode_mut};

/// 目录项：文件名 -> inode。
#[derive(Clone, Debug)]
pub struct Dentry {
    pub name: String,
    pub ino: u64,
    pub parent: Option<u64>,
}

/// 全局 dentry 表：(parent_ino, name) -> dentry。
struct DentryTable {
    entries: BTreeMap<(u64, String), Dentry>,
}

static DENTRY_TABLE: OnceLock<Mutex<DentryTable>> = OnceLock::new();

fn dentry_table() -> &'static Mutex<DentryTable> {
    DENTRY_TABLE.get_or_init(|| Mutex::new(DentryTable {
        entries: BTreeMap::new(),
    }))
}

/// 查找 dentry。
pub fn dentry_lookup(parent_ino: u64, name: &str) -> Option<u64> {
    let g = dentry_table().lock().unwrap();
    g.entries.get(&(parent_ino, name.to_string())).map(|d| d.ino)
}

/// 创建 dentry。
pub fn dentry_create(parent_ino: u64, name: &str, kind: InodeType) -> Result<u64, String> {
    let mut g = dentry_table().lock().unwrap();
    if g.entries.contains_key(&(parent_ino, name.to_string())) {
        return Err(format!("已存在: {}", name));
    }
    let ino = alloc_inode(kind);
    let dentry = Dentry {
        name: name.to_string(),
        ino,
        parent: Some(parent_ino),
    };
    g.entries.insert((parent_ino, name.to_string()), dentry);
    Ok(ino)
}

/// 删除 dentry。
pub fn dentry_delete(parent_ino: u64, name: &str) -> bool {
    let mut g = dentry_table().lock().unwrap();
    g.entries.remove(&(parent_ino, name.to_string())).is_some()
}

/// 列出目录下的所有条目。
pub fn dentry_list(parent_ino: u64) -> Vec<(String, u64)> {
    let g = dentry_table().lock().unwrap();
    g.entries
        .iter()
        .filter(|((p, _), _)| *p == parent_ino)
        .map(|((_, name), d)| (name.clone(), d.ino))
        .collect()
}

/// 根目录 ino。
pub const ROOT_INO: u64 = 1;

/// 初始化根目录。
pub fn init_root() {
    let mut g = dentry_table().lock().unwrap();
    if g.entries.is_empty() {
        alloc_inode(InodeType::Dir); // 确保 ino 1 是根目录
        let _ = dentry_create(0, ".", InodeType::Dir);
        let _ = dentry_create(0, "..", InodeType::Dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dentry_create_and_lookup() {
        init_root();
        let ino = dentry_create(ROOT_INO, "test.txt", InodeType::File).unwrap();
        assert_eq!(dentry_lookup(ROOT_INO, "test.txt"), Some(ino));
        assert_eq!(dentry_lookup(ROOT_INO, "nonexistent"), None);
    }

    #[test]
    fn dentry_delete() {
        init_root();
        dentry_create(ROOT_INO, "del.txt", InodeType::File).unwrap();
        assert!(dentry_delete(ROOT_INO, "del.txt"));
        assert_eq!(dentry_lookup(ROOT_INO, "del.txt"), None);
    }

    #[test]
    fn dentry_list_works() {
        init_root();
        dentry_create(ROOT_INO, "a.txt", InodeType::File).unwrap();
        dentry_create(ROOT_INO, "b.txt", InodeType::File).unwrap();
        let entries = dentry_list(ROOT_INO);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"b.txt"));
    }
}
