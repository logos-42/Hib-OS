//! inode 位图 —— P0-4 simplefs 子模块。

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// inode 位图：管理 inode 的分配状态。
pub struct InodeBitmap {
    total: u64,
    allocated: BTreeSet<u64>,
}

static INODE_BITMAP: OnceLock<Mutex<Option<InodeBitmap>>> = OnceLock::new();

fn inode_bitmap() -> &'static Mutex<Option<InodeBitmap>> {
    INODE_BITMAP.get_or_init(|| Mutex::new(None))
}

/// 初始化 inode 位图。
pub fn init(total: u64) {
    let mut g = inode_bitmap().lock().unwrap();
    *g = Some(InodeBitmap {
        total,
        allocated: BTreeSet::new(),
    });
}

/// 分配一个 inode。
pub fn alloc() -> Result<u64, String> {
    let mut g = inode_bitmap().lock().unwrap();
    let bitmap = g.as_mut().ok_or("位图未初始化")?;
    for i in 0..bitmap.total {
        if !bitmap.allocated.contains(&i) {
            bitmap.allocated.insert(i);
            return Ok(i);
        }
    }
    Err("inode 耗尽".to_string())
}

/// 释放一个 inode。
pub fn free(ino: u64) -> bool {
    let mut g = inode_bitmap().lock().unwrap();
    match g.as_mut() {
        Some(bitmap) => bitmap.allocated.remove(&ino),
        None => false,
    }
}

/// 查询 inode 是否已分配。
pub fn is_allocated(ino: u64) -> bool {
    let g = inode_bitmap().lock().unwrap();
    match g.as_ref() {
        Some(bitmap) => bitmap.allocated.contains(&ino),
        None => false,
    }
}

/// 已分配数量。
pub fn allocated_count() -> u64 {
    let g = inode_bitmap().lock().unwrap();
    match g.as_ref() {
        Some(bitmap) => bitmap.allocated.len() as u64,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_free() {
        init(100);
        let i1 = alloc().unwrap();
        assert_eq!(i1, 0);
        assert!(is_allocated(0));
        assert!(free(0));
        assert!(!is_allocated(0));
    }

    #[test]
    fn alloc_exhaustion() {
        init(3);
        alloc().unwrap();
        alloc().unwrap();
        alloc().unwrap();
        assert!(alloc().is_err());
    }

    #[test]
    fn allocated_count() {
        init(10);
        alloc().unwrap();
        alloc().unwrap();
        assert_eq!(super::allocated_count(), 2);
    }
}
