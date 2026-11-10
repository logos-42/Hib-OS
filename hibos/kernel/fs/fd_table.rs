//! 文件描述符表 —— P0-3 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::fs::inode::InodeType;

/// 访问权限。
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

/// 一个打开的文件描述符项。
#[derive(Clone, Debug)]
pub struct OpenFile {
    pub ino: u64,
    pub offset: u64,
    pub access: Access,
}

/// 单进程的文件描述符表。
#[derive(Debug, Default)]
pub struct FdTable {
    slots: Vec<Option<OpenFile>>,
}

impl FdTable {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub fn alloc(&mut self, ino: u64, access: Access) -> u64 {
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.is_none() {
                self.slots[i] = Some(OpenFile { ino, offset: 0, access });
                return i as u64;
            }
        }
        let fd = self.slots.len() as u64;
        self.slots.push(Some(OpenFile { ino, offset: 0, access }));
        fd
    }

    pub fn get(&self, fd: u64) -> Option<&OpenFile> {
        self.slots.get(fd as usize).and_then(|s| s.as_ref())
    }

    pub fn get_mut(&mut self, fd: u64) -> Option<&mut OpenFile> {
        self.slots.get_mut(fd as usize).and_then(|s| s.as_mut())
    }

    pub fn close(&mut self, fd: u64) -> bool {
        match self.slots.get_mut(fd as usize) {
            Some(slot) => {
                slot.take();
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

/// 全局 fd 表：按进程 ID 维护独立 fd 表。
struct FdTableRegistry {
    tables: BTreeMap<u64, FdTable>,
}

static FD_REGISTRY: OnceLock<Mutex<FdTableRegistry>> = OnceLock::new();

pub(crate) fn fd_registry() -> &'static Mutex<FdTableRegistry> {
    FD_REGISTRY.get_or_init(|| Mutex::new(FdTableRegistry {
        tables: BTreeMap::new(),
    }))
}

/// 为指定进程分配 fd。
pub fn fd_alloc(pid: u64, ino: u64, access: Access) -> u64 {
    let mut g = fd_registry().lock().unwrap();
    let table = g.tables.entry(pid).or_default();
    table.alloc(ino, access)
}

/// 获取打开的文件。
pub fn fd_get(pid: u64, fd: u64) -> Option<OpenFile> {
    let g = fd_registry().lock().unwrap();
    g.tables
        .get(&pid)
        .and_then(|t| t.get(fd))
        .cloned()
}

/// 关闭 fd。
pub fn fd_close(pid: u64, fd: u64) -> bool {
    let mut g = fd_registry().lock().unwrap();
    match g.tables.get_mut(&pid) {
        Some(table) => table.close(fd),
        None => false,
    }
}

/// 进程打开的 fd 数量。
pub fn fd_count(pid: u64) -> usize {
    let g = fd_registry().lock().unwrap();
    g.tables
        .get(&pid)
        .map(|t| t.slots.iter().filter(|s| s.is_some()).count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fd_alloc_and_get() {
        let fd = fd_alloc(1, 100, Access::ReadWrite);
        assert_eq!(fd, 0);
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
    fn fd_isolation_per_process() {
        let f1 = fd_alloc(1, 1, Access::Write);
        let f2 = fd_alloc(2, 2, Access::Write);
        assert_eq!(f1, f2); // 不同进程可有相同 fd 号
        assert_eq!(fd_count(1), 1);
        assert_eq!(fd_count(2), 1);
    }
}
