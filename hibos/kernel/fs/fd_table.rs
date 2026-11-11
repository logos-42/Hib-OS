//! 文件描述符表 —— 进程级 fd 分配与打开文件状态（no_std 内核版本）。
//!
//! 每进程独立 fd 表，记录打开的文件句柄、访问权限、当前偏移量。
//! 与 `kernel/fs/inode.rs`、`kernel/fs/dentry.rs` 配合，为 VFS 提供 POSIX 风格 fd 访问。

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::ksync::{SpinLock, OnceLock};

/// 文件访问权限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

impl Access {
    pub fn can_read(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }
    pub fn can_write(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }
}

/// 打开的文件句柄。
#[derive(Clone, Debug)]
pub struct OpenFile {
    pub ino: u64,
    pub offset: u64,
    pub access: Access,
}

/// 进程级 fd 表。
#[derive(Clone, Debug, Default)]
pub struct FdTable {
    entries: BTreeMap<u64, OpenFile>,
    next_fd: u64,
}

impl FdTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配一个新 fd，关联 inode 和访问权限。
    pub fn alloc(&mut self, ino: u64, access: Access) -> u64 {
        let fd = self.next_fd;
        self.entries.insert(fd, OpenFile { ino, offset: 0, access });
        self.next_fd += 1;
        fd
    }

    /// 获取 fd 对应的 OpenFile（不可变）。
    pub fn get(&self, fd: u64) -> Option<&OpenFile> {
        self.entries.get(&fd)
    }

    /// 获取 fd 对应的 OpenFile（可变）。
    pub fn get_mut(&mut self, fd: u64) -> Option<&mut OpenFile> {
        self.entries.get_mut(&fd)
    }

    /// 关闭并移除 fd。
    pub fn close(&mut self, fd: u64) -> bool {
        self.entries.remove(&fd).is_some()
    }

    /// 当前打开的 fd 数量。
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

/// 全局 fd 注册表：pid -> FdTable。
pub(crate) struct FdRegistry {
    pub(crate) tables: BTreeMap<u64, FdTable>,
}

static FD_REGISTRY: OnceLock<SpinLock<FdRegistry>> = OnceLock::new();

fn fd_registry_inner() -> &'static SpinLock<FdRegistry> {
    FD_REGISTRY.get_or_init(|| SpinLock::new(FdRegistry { tables: BTreeMap::new() }))
}

/// 为进程分配 fd。
pub fn fd_alloc(pid: u64, ino: u64, access: Access) -> u64 {
    let mut g = fd_registry_inner().lock();
    let table = g.tables.entry(pid).or_default();
    table.alloc(ino, access)
}

/// 获取进程的 fd 句柄（不可变）。
pub fn fd_get(pid: u64, fd: u64) -> Option<OpenFile> {
    let g = fd_registry_inner().lock();
    g.tables.get(&pid).and_then(|t| t.get(fd)).cloned()
}

/// 关闭进程的 fd。
pub fn fd_close(pid: u64, fd: u64) -> bool {
    let mut g = fd_registry_inner().lock();
    if let Some(table) = g.tables.get_mut(&pid) {
        table.close(fd)
    } else {
        false
    }
}

/// 进程打开的 fd 数量。
pub fn fd_count(pid: u64) -> usize {
    let g = fd_registry_inner().lock();
    g.tables.get(&pid).map(|t| t.count()).unwrap_or(0)
}

/// 返回全局 fd 注册表引用（供 VFS 等模块直接锁访问）。
pub fn fd_registry() -> &'static SpinLock<FdRegistry> {
    fd_registry_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_alloc_and_get() {
        let fd = fd_alloc(1, 100, Access::ReadWrite);
        let of = fd_get(1, fd).unwrap();
        assert_eq!(of.ino, 100);
        assert!(of.access.can_read());
        assert!(of.access.can_write());
    }

    #[test]
    fn fd_close_works() {
        let fd = fd_alloc(2, 200, Access::Read);
        assert!(fd_close(2, fd));
        assert!(fd_get(2, fd).is_none());
    }

    #[test]
    fn fd_count_per_process() {
        fd_alloc(3, 1, Access::Read);
        fd_alloc(3, 2, Access::Write);
        fd_alloc(4, 3, Access::Read);
        assert_eq!(fd_count(3), 2);
        assert_eq!(fd_count(4), 1);
    }
}

