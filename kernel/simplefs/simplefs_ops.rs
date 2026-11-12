//! simplefs 操作 —— P0-4 simplefs 子模块。

use std::sync::{Mutex, OnceLock};

use crate::simplefs::superblock::{format, get_superblock, is_formatted, alloc_block};
use crate::simplefs::inode_bitmap::{init as init_inode_bitmap, alloc as alloc_inode};
use crate::simplefs::block_bitmap::{init as init_block_bitmap};
use crate::simplefs::directory::{init_root, insert, remove};

/// 简单文件系统句柄。
pub struct SimpleFs {
    pub label: String,
}

static SIMPLEFS: OnceLock<Mutex<Option<SimpleFs>>> = OnceLock::new();

fn simplefs() -> &'static Mutex<Option<SimpleFs>> {
    SIMPLEFS.get_or_init(|| Mutex::new(None))
}

/// 格式化并创建 simplefs。
pub fn simplefs_format(block_size: u64, total_blocks: u64, label: &str) -> Result<(), String> {
    format(block_size, total_blocks, label)?;
    init_inode_bitmap(total_blocks / 4);
    init_block_bitmap(total_blocks);
    init_root();
    let mut g = simplefs().lock().unwrap();
    *g = Some(SimpleFs { label: label.to_string() });
    Ok(())
}

/// 创建文件。
pub fn simplefs_create(name: &str) -> Result<u64, String> {
    if !is_formatted() {
        return Err("文件系统未格式化".to_string());
    }
    let ino = alloc_inode()?;
    insert(name, ino, 0)?;
    Ok(ino)
}

/// 读取文件（模拟：返回空）。
pub fn simplefs_read(_name: &str, _buf: &mut [u8]) -> Result<u64, String> {
    if !is_formatted() {
        return Err("文件系统未格式化".to_string());
    }
    Ok(0)
}

/// 写入文件（模拟：分配块）。
pub fn simplefs_write(_name: &str, _data: &[u8]) -> Result<u64, String> {
    if !is_formatted() {
        return Err("文件系统未格式化".to_string());
    }
    let _block = alloc_block()?;
    Ok(0)
}

/// 删除文件。
pub fn simplefs_delete(name: &str) -> Result<(), String> {
    if !is_formatted() {
        return Err("文件系统未格式化".to_string());
    }
    if !remove(name) {
        return Err("文件未找到".to_string());
    }
    Ok(())
}

/// 文件系统统计。
pub fn simplefs_stats() -> String {
    let sb = get_superblock();
    match sb {
        Some(s) => format!(
            "simplefs label={} blocks={} free={} magic=0x{:x}",
            s.label, s.total_blocks, s.free_blocks, s.magic
        ),
        None => "simplefs 未格式化".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_create_delete() {
        assert!(simplefs_format(4096, 1000, "testfs").is_ok());
        let ino = simplefs_create("hello.txt").unwrap();
        assert_ne!(ino, 0);
        assert!(simplefs_delete("hello.txt").is_ok());
    }

    #[test]
    fn stats() {
        let _ = simplefs_format(4096, 500, "stats");
        let s = simplefs_stats();
        assert!(s.contains("stats"));
    }
}
