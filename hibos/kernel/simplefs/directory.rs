//! 目录 —— P0-4 simplefs 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 目录项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub ino: u64,
    pub kind: u8, // 0=file, 1=dir
}

/// 目录：名字 -> 条目。
pub struct Directory {
    entries: BTreeMap<String, DirEntry>,
}

static DIRECTORY: OnceLock<Mutex<Option<Directory>>> = OnceLock::new();

fn directory() -> &'static Mutex<Option<Directory>> {
    DIRECTORY.get_or_init(|| Mutex::new(None))
}

/// 初始化根目录。
pub fn init_root() {
    let mut g = directory().lock().unwrap();
    if g.is_none() {
        let mut entries = BTreeMap::new();
        entries.insert(
            ".".to_string(),
            DirEntry { name: ".".to_string(), ino: 1, kind: 1 },
        );
        entries.insert(
            "..".to_string(),
            DirEntry { name: "..".to_string(), ino: 1, kind: 1 },
        );
        *g = Some(Directory { entries });
    }
}

/// 查找条目。
pub fn lookup(name: &str) -> Option<DirEntry> {
    let g = directory().lock().unwrap();
    match g.as_ref() {
        Some(dir) => dir.entries.get(name).cloned(),
        None => None,
    }
}

/// 插入条目。
pub fn insert(name: &str, ino: u64, kind: u8) -> Result<(), String> {
    let mut g = directory().lock().unwrap();
    match g.as_mut() {
        Some(dir) => {
            if dir.entries.contains_key(name) {
                return Err("条目已存在".to_string());
            }
            dir.entries.insert(
                name.to_string(),
                DirEntry { name: name.to_string(), ino, kind },
            );
            Ok(())
        }
        None => Err("目录未初始化".to_string()),
    }
}

/// 删除条目。
pub fn remove(name: &str) -> bool {
    let mut g = directory().lock().unwrap();
    match g.as_mut() {
        Some(dir) => dir.entries.remove(name).is_some(),
        None => false,
    }
}

/// 列出所有条目。
pub fn list() -> Vec<DirEntry> {
    let g = directory().lock().unwrap();
    match g.as_ref() {
        Some(dir) => dir.entries.values().cloned().collect(),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_and_lookup() {
        init_root();
        assert_eq!(lookup(".").map(|e| e.ino), Some(1));
        assert_eq!(lookup("nonexistent"), None);
    }

    #[test]
    fn insert_and_list() {
        init_root();
        assert!(insert("a.txt", 10, 0).is_ok());
        assert!(insert("b.txt", 11, 0).is_ok());
        let entries = list();
        assert!(entries.iter().any(|e| e.name == "a.txt"));
        assert!(entries.iter().any(|e| e.name == "b.txt"));
    }

    #[test]
    fn remove_works() {
        init_root();
        insert("c.txt", 12, 0).unwrap();
        assert!(remove("c.txt"));
        assert_eq!(lookup("c.txt"), None);
    }
}
