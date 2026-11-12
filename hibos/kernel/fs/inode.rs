//! inode 节点 —— P0-3 子模块（no_std 内核版本）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::ksync::{SpinLock, OnceLock};

/// inode 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InodeType {
    File,
    Dir,
    Symlink,
}

/// inode 元数据。
#[derive(Clone, Debug)]
pub struct Inode {
    pub ino: u64,
    pub kind: InodeType,
    pub size: u64,
    pub blocks: u64,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub link_count: u32,
    pub data: Vec<u8>,
}

impl Inode {
    pub fn new(ino: u64, kind: InodeType) -> Self {
        let now = crate::time::get_time_ns();
        Self {
            ino,
            kind,
            size: 0,
            blocks: 0,
            uid: 0,
            gid: 0,
            mode: 0o644,
            atime: now,
            mtime: now,
            ctime: now,
            link_count: 1,
            data: Vec::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, InodeType::Dir)
    }

    pub fn is_file(&self) -> bool {
        matches!(self.kind, InodeType::File)
    }
}

/// 全局 inode 表。
struct InodeTable {
    inodes: BTreeMap<u64, Inode>,
    next_ino: u64,
}

static INODE_TABLE: OnceLock<SpinLock<InodeTable>> = OnceLock::new();

fn inode_table() -> &'static SpinLock<InodeTable> {
    INODE_TABLE.get_or_init(|| SpinLock::new(InodeTable {
        inodes: BTreeMap::new(),
        next_ino: 1,
    }))
}

/// 分配新 inode。
pub fn alloc_inode(kind: InodeType) -> u64 {
    let mut g = inode_table().lock();
    let ino = g.next_ino;
    g.next_ino += 1;
    let inode = Inode::new(ino, kind);
    g.inodes.insert(ino, inode);
    ino
}

/// 查询 inode。
pub fn get_inode(ino: u64) -> Option<Inode> {
    let g = inode_table().lock();
    g.inodes.get(&ino).cloned()
}

/// 获取可变 inode 引用。
pub fn get_inode_mut<F>(ino: u64, f: F) -> Option<()>
where
    F: FnOnce(&mut Inode),
{
    let mut g = inode_table().lock();
    g.inodes.get_mut(&ino).map(|i| {
        f(i);
    })
}

/// 释放 inode。
pub fn free_inode(ino: u64) -> bool {
    let mut g = inode_table().lock();
    g.inodes.remove(&ino).is_some()
}

/// 当前已分配的 inode 总数（不含已释放）。
pub fn inode_count() -> usize {
    inode_table().lock().inodes.len()
}

/// 已分配 inode 的总字节数（data 部分），用于 `df` 类感知。
pub fn inode_bytes() -> u64 {
    inode_table()
        .lock()
        .inodes
        .values()
        .map(|i| i.data.len() as u64)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_get_inode() {
        let ino = alloc_inode(InodeType::File);
        let inode = get_inode(ino).unwrap();
        assert_eq!(ino, inode.ino);
        assert!(inode.is_file());
        assert!(!inode.is_dir());
    }

    #[test]
    fn inode_data_write() {
        let ino = alloc_inode(InodeType::File);
        get_inode_mut(ino, |i| {
            i.data = b"hello".to_vec();
            i.size = 5;
        }).unwrap();
        let inode = get_inode(ino).unwrap();
        assert_eq!(inode.size, 5);
    }

    #[test]
    fn free_inode_works() {
        let ino = alloc_inode(InodeType::Dir);
        assert!(free_inode(ino));
        assert!(get_inode(ino).is_none());
    }
}

