//! 简单文件系统（类 ext2 简化版）—— P0-4 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - 文件系统 / inode / 目录 → 知识单元
//! - 磁盘空间管理：位图、链表 → 知识单元索引位图
//! - 文件分配：连续/链表/索引分配 → 知识单元块分配
//! 
//! 通道：`simplefs/*`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod superblock;
pub mod inode_bitmap;
pub mod block_bitmap;
pub mod directory;
pub mod simplefs_ops;

// 公共类型
pub use superblock::SuperBlock;
pub use inode_bitmap::InodeBitmap;
pub use block_bitmap::BlockBitmap;
pub use directory::Directory;
pub use simplefs_ops::{SimpleFs, simplefs_format, simplefs_create, simplefs_read, simplefs_write, simplefs_delete, simplefs_stats};

// 简单文件系统错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimpleFsError {
    NotFormatted,
    NoSpace,
    NotFound,
    AlreadyExists,
    Corrupted,
    IoError,
}

impl std::fmt::Display for SimpleFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimpleFsError::NotFormatted => write!(f, "文件系统未格式化"),
            SimpleFsError::NoSpace => write!(f, "无可用空间"),
            SimpleFsError::NotFound => write!(f, "条目未找到"),
            SimpleFsError::AlreadyExists => write!(f, "条目已存在"),
            SimpleFsError::Corrupted => write!(f, "文件系统损坏"),
            SimpleFsError::IoError => write!(f, "IO 错误"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplefs_error_display() {
        assert_eq!(format!("{}", SimpleFsError::NoSpace), "无可用空间");
        assert_eq!(format!("{}", SimpleFsError::Corrupted), "文件系统损坏");
    }
}
