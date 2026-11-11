//! dentry 目录项 —— P0-3 子模块（no_std 内核版本）。

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::ksync::{SpinLock, OnceLock};

/// 目录项：文件名 -> inode。
#[derive(Clone, Debug)]
pub struct Dentry {
    pub name: String,
    pub ino: u64,
    pub parent: Option<u64>,
}

/// 根目录 inode 号。
pub const ROOT_INO: u64 = 1;

/// 全局 dentry 表：(parent_ino, name) -> dentry。
struct DentryTable {
    entries: BTreeMap<(u64, String), Dentry>,
}

static DENTRY_TABLE: OnceLock<SpinLock<DentryTable>> = OnceLock::new();

fn dentry_table() -> &'static SpinLock<DentryTable> {
    DENTRY_TABLE.get_or_init(|| SpinLock::new(DentryTable {
        entries: BTreeMap::new(),
    }))
}

/// 查找 dentry。
pub fn dentry_lookup(parent_ino: u64, name: &str) -> Option<u64> {
    let g = dentry_table().lock();
    g.entries.get(&(parent_ino, name.to_string())).map(|d| d.ino)
}

/// 创建 dentry。
pub fn dentry_create(parent_ino: u64, name: &str, kind: u8) -> Result<u64, String> {
    let _ = kind;
    dentry_create_typed(parent_ino, name, crate::fs::inode::InodeType::File)
}

/// 按 inode 类型创建 dentry（`kind` 描述符与 ``InodeType`` 的映射由调用方决定，
/// 这里直接接收类型，避免 `dentry_create` 的 u8 参数在目录场景下失真）。
pub fn dentry_create_typed(
    parent_ino: u64,
    name: &str,
    kind: crate::fs::inode::InodeType,
) -> Result<u64, String> {
    let mut g = dentry_table().lock();
    if g.entries.contains_key(&(parent_ino, name.to_string())) {
        return Err(format!("已存在: {}", name));
    }
    let ino = crate::fs::inode::alloc_inode(kind);
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
    let mut g = dentry_table().lock();
    g.entries.remove(&(parent_ino, name.to_string())).is_some()
}

/// 列出目录下的所有条目。
pub fn dentry_list(parent_ino: u64) -> Vec<(String, u64)> {
    let g = dentry_table().lock();
    g.entries
        .iter()
        .filter(|((p, _), _)| *p == parent_ino)
        .map(|((_, name), d)| (name.clone(), d.ino))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dentry_create_and_lookup() {
        let ino = dentry_create(ROOT_INO, "test.txt", 0).unwrap();
        assert_eq!(dentry_lookup(ROOT_INO, "test.txt"), Some(ino));
        assert_eq!(dentry_lookup(ROOT_INO, "nonexistent"), None);
    }

    #[test]
    fn dentry_delete_works() {
        dentry_create(ROOT_INO, "del.txt", 0).unwrap();
        assert!(dentry_delete(ROOT_INO, "del.txt"));
        assert_eq!(dentry_lookup(ROOT_INO, "del.txt"), None);
    }
}

