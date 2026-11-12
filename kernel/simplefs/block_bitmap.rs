//! 块位图 —— P0-4 simplefs 子模块。

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// 块位图：管理数据块的分配状态。
pub struct BlockBitmap {
    total: u64,
    allocated: BTreeSet<u64>,
}

static BLOCK_BITMAP: OnceLock<Mutex<Option<BlockBitmap>>> = OnceLock::new();

fn block_bitmap() -> &'static Mutex<Option<BlockBitmap>> {
    BLOCK_BITMAP.get_or_init(|| Mutex::new(None))
}

/// 初始化块位图。
pub fn init(total: u64) {
    let mut g = block_bitmap().lock().unwrap();
    *g = Some(BlockBitmap {
        total,
        allocated: BTreeSet::new(),
    });
}

/// 分配一个块。
pub fn alloc() -> Result<u64, String> {
    let mut g = block_bitmap().lock().unwrap();
    let bitmap = g.as_mut().ok_or("位图未初始化")?;
    for i in 0..bitmap.total {
        if !bitmap.allocated.contains(&i) {
            bitmap.allocated.insert(i);
            return Ok(i);
        }
    }
    Err("块耗尽".to_string())
}

/// 释放一个块。
pub fn free(block: u64) -> bool {
    let mut g = block_bitmap().lock().unwrap();
    match g.as_mut() {
        Some(bitmap) => bitmap.allocated.remove(&block),
        None => false,
    }
}

/// 查询块是否已分配。
pub fn is_allocated(block: u64) -> bool {
    let g = block_bitmap().lock().unwrap();
    match g.as_ref() {
        Some(bitmap) => bitmap.allocated.contains(&block),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_free() {
        init(100);
        let b1 = alloc().unwrap();
        assert_eq!(b1, 0);
        assert!(is_allocated(0));
        assert!(free(0));
        assert!(!is_allocated(0));
    }
}
