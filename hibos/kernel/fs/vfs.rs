//! 文件描述符表与系统调用层（open/read/write/close/llseek）。
//!
//! 每进程独立 fd 表；open/read/write/close/llseek 与类 ext 文件系统
//! (ExtFs) 配合，为 `fs://` 通道提供 POSIX 风格的文件访问。

use std::collections::BTreeMap;

use crate::fs::block::BlockError;
use crate::fs::extfs::{ExtFs, InodeKind};

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
    /// 索引 = fd，None 表示未打开。
    slots: Vec<Option<OpenFile>>,
}

impl FdTable {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// 分配一个新 fd 并关联 inode，返回 fd 号。
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

    pub fn close(&mut self, fd: u64) -> Result<(), BlockError> {
        match self.slots.get_mut(fd as usize) {
            Some(slot) => {
                slot.take();
                Ok(())
            }
            None => Err(BlockError::NotFound),
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

/// 全局 VFS：按进程 ID 维护独立 fd 表。
pub struct Vfs {
    tables: BTreeMap<u64, FdTable>,
}

impl Vfs {
    pub fn new() -> Self {
        Self { tables: BTreeMap::new() }
    }

    fn table_for(&mut self, pid: u64) -> &mut FdTable {
        self.tables.entry(pid).or_default()
    }

    /// open 系统调用：解析路径到 inode（不存在则创建文件），分配 fd。
    pub fn open(
        &mut self,
        pid: u64,
        extfs: &mut ExtFs,
        path: &str,
        access: Access,
    ) -> Result<u64, BlockError> {
        let comps = resolve_path(path);
        if comps.is_empty() {
            return Err(BlockError::NotFound);
        }
        let name = comps.last().unwrap();
        let parent = resolve_parent(extfs, &comps)?;
        let ino = match extfs.dir_lookup(parent, name) {
            Some(ino) => ino,
            None => {
                let ino = extfs.alloc_inode().map_err(|_| BlockError::IoError("no inodes".into()))?;
                extfs.inode_get_mut(ino).map(|i| i.kind = InodeKind::File);
                extfs.dir_add(parent, name, ino).map_err(|e| e)?;
                ino
            }
        };
        // 权限检查：只读打开不可写文件等（此处简化为按 access 放行）
        let _ = access;
        let fd = self.table_for(pid).alloc(ino, access);
        Ok(fd)
    }

    /// read 系统调用：从 fd 读入 buf，返回读取字节数。
    pub fn read(
        &mut self,
        pid: u64,
        extfs: &mut ExtFs,
        fd: u64,
        buf: &mut [u8],
    ) -> Result<u64, BlockError> {
        let of = self
            .table_for(pid)
            .get(fd)
            .ok_or(BlockError::NotFound)?;
        if !of.access.can_read() {
            return Err(BlockError::IoError("not readable".into()));
        }
        let _inode = extfs
            .inode_get(of.ino)
            .ok_or(BlockError::NotFound)?;
        let data = extfs.file_read(of.ino, of.offset, buf.len() as u64).map_err(|e| e)?;
        let n = data.len();
        buf[..n].copy_from_slice(&data);
        let table = self.table_for(pid);
        let of = table.get_mut(fd).ok_or(BlockError::NotFound)?;
        of.offset += n as u64;
        Ok(n as u64)
    }

    /// write 系统调用：将 buf 写入 fd，返回写入字节数。
    pub fn write(
        &mut self,
        pid: u64,
        extfs: &mut ExtFs,
        fd: u64,
        buf: &[u8],
    ) -> Result<u64, BlockError> {
        let of = self
            .table_for(pid)
            .get(fd)
            .ok_or(BlockError::NotFound)?;
        if !of.access.can_write() {
            return Err(BlockError::IoError("not writable".into()));
        }
        let written = extfs.file_write(of.ino, of.offset, buf).map_err(|e| e)?;
        let table = self.table_for(pid);
        let of = table.get_mut(fd).ok_or(BlockError::NotFound)?;
        of.offset += written as u64;
        let _ = extfs.writeback_inode(of.ino);
        Ok(written)
    }

    /// close 系统调用。
    pub fn close(&mut self, pid: u64, fd: u64) -> Result<(), BlockError> {
        self.table_for(pid).close(fd)
    }

    /// llseek 系统调用：whence 0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END。
    pub fn llseek(
        &mut self,
        pid: u64,
        extfs: &mut ExtFs,
        fd: u64,
        offset: i64,
        whence: u8,
    ) -> Result<u64, BlockError> {
        let of = self
            .table_for(pid)
            .get(fd)
            .ok_or(BlockError::NotFound)?;
        let inode = extfs.inode_get(of.ino).ok_or(BlockError::NotFound)?;
        let new_offset = match whence {
            0 => offset,
            1 => (of.offset as i64).saturating_add(offset),
            2 => (inode.size as i64).saturating_add(offset),
            _ => return Err(BlockError::IoError("invalid whence".into())),
        };
        if new_offset < 0 {
            return Err(BlockError::IoError("negative seek".into()));
        }
        let table = self.table_for(pid);
        let of = table.get_mut(fd).ok_or(BlockError::NotFound)?;
        of.offset = new_offset as u64;
        Ok(of.offset)
    }

    /// 是否有有效的 fd。
    pub fn has_fd(&self, pid: u64, fd: u64) -> bool {
        self.tables
            .get(&pid)
            .and_then(|t| t.get(fd))
            .is_some()
    }

    /// 进程打开的 fd 数量。
    pub fn fd_count(&self, pid: u64) -> usize {
        self.tables.get(&pid).map(|t| t.slots.iter().filter(|s| s.is_some()).count()).unwrap_or(0)
    }
}

// ----- 路径解析辅助（与 extfs 层共享） -----

fn resolve_path(path: &str) -> Vec<String> {
    let mut comps: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => { comps.pop(); }
            name => comps.push(name.to_string()),
        }
    }
    comps
}

fn resolve_parent(extfs: &mut ExtFs, comps: &[String]) -> Result<u64, BlockError> {
    let mut ino = extfs.root_ino();
    for c in &comps[..comps.len() - 1] {
        let next = extfs.dir_lookup(ino, c).ok_or(BlockError::NotFound)?;
        let inode = extfs.inode_get(next).ok_or(BlockError::NotFound)?;
        if inode.kind != InodeKind::Dir {
            return Err(BlockError::IoError("not a directory".into()));
        }
        ino = next;
    }
    Ok(ino)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_open_read_write_close() {
        let mut vfs = Vfs::new();
        let mut extfs = crate::fs::extfs::ExtFs::new(1024, 128);
        extfs.format().unwrap();

        let fd = vfs.open(1, &mut extfs, "/a.txt", Access::Write).unwrap();
        assert!(vfs.has_fd(1, fd));
        let written = vfs.write(1, &mut extfs, fd, b"hello vfs").unwrap();
        assert_eq!(written, 10);

        let fd2 = vfs.open(1, &mut extfs, "/a.txt", Access::Read).unwrap();
        let mut buf = [0u8; 16];
        let n = vfs.read(1, &mut extfs, fd2, &mut buf).unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf[..n], b"hello vfs");

        vfs.close(1, fd).unwrap();
        vfs.close(1, fd2).unwrap();
        assert!(!vfs.has_fd(1, fd));
        assert_eq!(vfs.fd_count(1), 0);
    }

    #[test]
    fn vfs_fd_isolation_per_process() {
        let mut vfs = Vfs::new();
        let mut extfs = crate::fs::extfs::ExtFs::new(1024, 128);
        extfs.format().unwrap();

        let f1 = vfs.open(1, &mut extfs, "/p1.txt", Access::Write).unwrap();
        let f2 = vfs.open(2, &mut extfs, "/p2.txt", Access::Write).unwrap();
        // 两个进程各自独立，fd 号可相同互不干扰
        assert_eq!(vfs.fd_count(1), 1);
        assert_eq!(vfs.fd_count(2), 1);
        vfs.write(1, &mut extfs, f1, b"proc-one").unwrap();
        vfs.write(2, &mut extfs, f2, b"proc-two").unwrap();
        vfs.close(1, f1).unwrap();
        assert!(!vfs.has_fd(1, f1));
        assert!(vfs.has_fd(2, f2));
    }

    #[test]
    fn vfs_llseek_and_read() {
        let mut vfs = Vfs::new();
        let mut extfs = crate::fs::extfs::ExtFs::new(1024, 128);
        extfs.format().unwrap();
        let fd = vfs.open(1, &mut extfs, "/seek.txt", Access::Write).unwrap();
        vfs.write(1, &mut extfs, fd, b"0123456789").unwrap();
        // 读取后 offset 前移
        let mut buf = [0u8; 3];
        vfs.read(1, &mut extfs, fd, &mut buf).unwrap();
        assert_eq!(&buf, b"012");
        // seek 到末尾再读应得 0
        vfs.llseek(1, &mut extfs, fd, 0, 2).unwrap();
        let mut buf2 = [0u8; 4];
        let n = vfs.read(1, &mut extfs, fd, &mut buf2).unwrap();
        assert_eq!(n, 0);
        vfs.close(1, fd).unwrap();
    }

    #[test]
    fn vfs_close_invalid_fd_errors() {
        let mut vfs = Vfs::new();
        let mut extfs = crate::fs::extfs::ExtFs::new(1024, 128);
        extfs.format().unwrap();
        assert_eq!(vfs.close(1, 999).unwrap_err(), BlockError::NotFound);
        assert_eq!(
            vfs.read(1, &mut extfs, 999, &mut [0u8; 4]).unwrap_err(),
            BlockError::NotFound
        );
    }

    #[test]
    fn vfs_open_creates_and_reuses_fd() {
        let mut vfs = Vfs::new();
        let mut extfs = crate::fs::extfs::ExtFs::new(1024, 128);
        extfs.format().unwrap();
        let f1 = vfs.open(1, &mut extfs, "/reuse.txt", Access::ReadWrite).unwrap();
        let f2 = vfs.open(1, &mut extfs, "/reuse.txt", Access::ReadWrite).unwrap();
        // 同一路径打开两次得到不同 fd，指向同一 ino
        assert_ne!(f1, f2);
        assert_eq!(vfs.tables.get(&1).unwrap().get(f1).unwrap().ino, vfs.tables.get(&1).unwrap().get(f2).unwrap().ino);
        vfs.close(1, f1).unwrap();
        // 释放后的 fd 号可被复用
        let f3 = vfs.open(1, &mut extfs, "/reuse2.txt", Access::Read).unwrap();
        assert_eq!(f3, f1);
        vfs.close(1, f2).unwrap();
        vfs.close(1, f3).unwrap();
    }
}
