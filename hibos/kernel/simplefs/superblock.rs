//! 超级块 —— P0-4 simplefs 子模块。

use std::sync::{Mutex, OnceLock};

/// 文件系统超级块：记录文件系统元数据。
#[derive(Clone, Debug)]
pub struct SuperBlock {
    pub magic: u32,
    pub block_size: u64,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub inode_count: u64,
    pub free_inodes: u64,
    pub label: String,
}

const SIMPLEFS_MAGIC: u32 = 0x4849_424F; // "HIBO"

static SUPERBLOCK: OnceLock<Mutex<Option<SuperBlock>>> = OnceLock::new();

fn superblock() -> &'static Mutex<Option<SuperBlock>> {
    SUPERBLOCK.get_or_init(|| Mutex::new(None))
}

/// 格式化文件系统，创建超级块。
pub fn format(block_size: u64, total_blocks: u64, label: &str) -> Result<(), String> {
    let mut g = superblock().lock().unwrap();
    if g.is_some() {
        return Err("文件系统已格式化".to_string());
    }
    let inode_count = total_blocks / 4; // 每 4 个块 1 个 inode
    *g = Some(SuperBlock {
        magic: SIMPLEFS_MAGIC,
        block_size,
        total_blocks,
        free_blocks: total_blocks,
        inode_count,
        free_inodes: inode_count,
        label: label.to_string(),
    });
    Ok(())
}

/// 查询超级块。
pub fn get_superblock() -> Option<SuperBlock> {
    let g = superblock().lock().unwrap();
    g.clone()
}

/// 检查是否已格式化。
pub fn is_formatted() -> bool {
    superblock().lock().unwrap().is_some()
}

/// 分配一个块。
pub fn alloc_block() -> Result<u64, String> {
    let mut g = superblock().lock().unwrap();
    let sb = g.as_mut().ok_or("文件系统未格式化")?;
    if sb.free_blocks == 0 {
        return Err("无可用块".to_string());
    }
    sb.free_blocks -= 1;
    Ok(sb.total_blocks - sb.free_blocks - 1)
}

/// 释放一个块。
pub fn free_block(_block: u64) -> Result<(), String> {
    let mut g = superblock().lock().unwrap();
    let sb = g.as_mut().ok_or("文件系统未格式化")?;
    sb.free_blocks += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_query() {
        assert!(format(4096, 10000, "myfs").is_ok());
        assert!(is_formatted());
        let sb = get_superblock().unwrap();
        assert_eq!(sb.magic, 0x4849_424F);
        assert_eq!(sb.label, "myfs");
        assert_eq!(sb.total_blocks, 10000);
    }

    #[test]
    fn format_duplicate() {
        format(4096, 1000, "fs1").unwrap();
        assert!(format(4096, 1000, "fs2").is_err());
    }

    #[test]
    fn alloc_free_block() {
        format(4096, 100, "test").unwrap();
        let b = alloc_block().unwrap();
        assert_eq!(b, 0);
        assert!(free_block(b).is_ok());
    }
}
