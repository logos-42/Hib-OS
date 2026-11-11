//! 文件系统 VFS 子系统 —— P0-3 路线图。
//! 
//! AI 原生映射（docs/19）：
//! - 传统文件系统 / inode / 目录 → 知识单元 (内容+向量+置信度+epoch)
//! - 路径查找 → 语义/向量检索 (RAG)
//! - 日志文件系统 (WAL) → 知识提交日志 + epoch 快照
//! 
//! 通道：`fs://*`，可被 `sense`/`act` 操作。

// 子模块（由子智能体并行填充）
pub mod vfs;
pub mod inode;
pub mod dentry;
pub mod fd_table;
// pub mod mount_mgr;      // Phase 3: 挂载管理
// pub mod procfs_ext;     // Phase 3: procfs 扩展

// 公共类型
pub use vfs::{Vfs, vfs, vfs_open, vfs_read, vfs_write, vfs_close, vfs_unlink, vfs_stats, write_file, read_file, read_file_text, list_dir, make_dir, remove_path, stat_path, fs_sense_snapshot, fs_act_dispatch, resolve_inode};
pub use inode::{Inode, InodeType, inode_count, inode_bytes};
pub use dentry::{Dentry, dentry_lookup, dentry_create, dentry_create_typed, dentry_list};

// 文件操作类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Create,
    Unlink,
    Seek,
    Stat,
}

/// VFS 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidArgument,
    IoError,
    NotDirectory,
    IsDirectory,
}

impl core::fmt::Display for VfsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VfsError::NotFound => write!(f, "文件未找到"),
            VfsError::PermissionDenied => write!(f, "权限不足"),
            VfsError::AlreadyExists => write!(f, "文件已存在"),
            VfsError::InvalidArgument => write!(f, "无效参数"),
            VfsError::IoError => write!(f, "IO 错误"),
            VfsError::NotDirectory => write!(f, "不是目录"),
            VfsError::IsDirectory => write!(f, "是目录"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_error_display() {
        assert_eq!(format!("{}", VfsError::NotFound), "文件未找到");
        assert_eq!(format!("{}", VfsError::PermissionDenied), "权限不足");
    }

    #[test]
    fn file_op_variants() {
        let ops = [FileOp::Read, FileOp::Write, FileOp::Create, FileOp::Unlink, FileOp::Seek, FileOp::Stat];
        assert_eq!(ops.len(), 6);
    }
}
