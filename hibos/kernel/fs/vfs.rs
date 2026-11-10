//! 文件描述符表与系统调用层（open/read/write/close/llseek）。
//!
//! 每进程独立 fd 表；open/read/write/close/llseek 与简单文件系统
//! 配合，为 `fs://` 通道提供 POSIX 风格的文件访问。

use std::collections::BTreeMap;

use super::inode::{alloc_inode, get_inode, get_inode_mut, InodeType};
use super::dentry::{dentry_lookup, dentry_create, dentry_delete, ROOT_INO};
use super::fd_table::{fd_alloc, fd_close, fd_count, fd_get, Access, OpenFile, FdTable, fd_registry};

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
    pub fn open(&mut self, pid: u64, path: &str, access: Access) -> Result<u64, String> {
        let comps = resolve_path(path);
        if comps.is_empty() {
            return Err("空路径".to_string());
        }
        let name = comps.last().unwrap();
        let parent = resolve_parent(&comps)?;
        let ino = match dentry_lookup(parent, name) {
            Some(ino) => ino,
            None => {
                let ino = alloc_inode(InodeType::File);
                dentry_create(parent, name, InodeType::File).map_err(|e| format!("{}", e))?;
                ino
            }
        };
        let _ = access;
        let fd = fd_alloc(pid, ino, access);
        Ok(fd)
    }

    /// read 系统调用：从 fd 读入 buf，返回读取字节数。
    pub fn read(&mut self, pid: u64, fd: u64, buf: &mut [u8]) -> Result<u64, String> {
        let of = fd_get(pid, fd).ok_or("fd 未找到")?;
        if !of.access.can_read() {
            return Err("不可读".to_string());
        }
        let inode = get_inode(of.ino).ok_or("inode 未找到")?;
        let data = &inode.data;
        let start = of.offset as usize;
        let end = (start + buf.len()).min(data.len());
        let n = end - start;
        buf[..n].copy_from_slice(&data[start..end]);
        // 更新 offset 需要可变访问
        {
            let mut g = fd_registry().lock().unwrap();
            if let Some(table) = g.tables.get_mut(&pid) {
                if let Some(of) = table.get_mut(fd) {
                    of.offset += n as u64;
                }
            }
        }
        Ok(n as u64)
    }

    /// write 系统调用：将 buf 写入 fd，返回写入字节数。
    pub fn write(&mut self, pid: u64, fd: u64, buf: &[u8]) -> Result<u64, String> {
        let of = fd_get(pid, fd).ok_or("fd 未找到")?;
        if !of.access.can_write() {
            return Err("不可写".to_string());
        }
        let mut g = fd_registry().lock().unwrap();
        let table = g.tables.get_mut(&pid).ok_or("进程表未找到")?;
        let of = table.get_mut(fd).ok_or("fd 未找到")?;
        let pos = of.offset as usize;
        let needed = pos + buf.len();
        let inode = get_inode_mut(of.ino).ok_or("inode 未找到")?;
        if needed > inode.data.len() {
            inode.data.resize(needed, 0);
        }
        inode.data[pos..needed].copy_from_slice(buf);
        inode.size = needed as u64;
        of.offset += buf.len() as u64;
        let written = buf.len();
        drop(g);
        Ok(written as u64)
    }

    /// close 系统调用。
    pub fn close(&mut self, pid: u64, fd: u64) -> Result<(), String> {
        if fd_close(pid, fd) {
            Ok(())
        } else {
            Err("fd 未找到".to_string())
        }
    }

    /// llseek 系统调用：whence 0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END。
    pub fn llseek(&mut self, pid: u64, fd: u64, offset: i64, whence: u8) -> Result<u64, String> {
        let of = fd_get(pid, fd).ok_or("fd 未找到")?;
        let inode = get_inode(of.ino).ok_or("inode 未找到")?;
        let new_offset = match whence {
            0 => offset,
            1 => (of.offset as i64).saturating_add(offset),
            2 => (inode.size as i64).saturating_add(offset),
            _ => return Err("无效 whence".to_string()),
        };
        if new_offset < 0 {
            return Err("负偏移".to_string());
        }
        let mut g = fd_registry().lock().unwrap();
        let table = g.tables.get_mut(&pid).ok_or("进程表未找到")?;
        let of = table.get_mut(fd).ok_or("fd 未找到")?;
        of.offset = new_offset as u64;
        drop(g);
        Ok(of.offset)
    }

    /// 是否有有效的 fd。
    pub fn has_fd(&self, pid: u64, fd: u64) -> bool {
        let g = fd_registry().lock().unwrap();
        g.tables.get(&pid).and_then(|t| t.get(fd)).is_some()
    }

    /// 进程打开的 fd 数量。
    pub fn fd_count(&self, pid: u64) -> usize {
        fd_count(pid)
    }
}

// ----- 路径解析辅助 -----

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

fn resolve_parent(comps: &[String]) -> Result<u64, String> {
    let mut ino = ROOT_INO;
    for c in &comps[..comps.len() - 1] {
        let next = dentry_lookup(ino, c).ok_or("父目录未找到")?;
        let inode = get_inode(next).ok_or("inode 未找到")?;
        if !inode.is_dir() {
            return Err("不是目录".to_string());
        }
        ino = next;
    }
    Ok(ino)
}

/// 全局 VFS 单例。
pub fn vfs() -> Vfs {
    Vfs::new()
}

/// vfs_open 便捷函数。
pub fn vfs_open(pid: u64, path: &str, access: Access) -> Result<u64, String> {
    let mut vfs = Vfs::new();
    vfs.open(pid, path, access)
}

/// vfs_read 便捷函数。
pub fn vfs_read(pid: u64, fd: u64, buf: &mut [u8]) -> Result<u64, String> {
    let mut vfs = Vfs::new();
    vfs.read(pid, fd, buf)
}

/// vfs_write 便捷函数。
pub fn vfs_write(pid: u64, fd: u64, buf: &[u8]) -> Result<u64, String> {
    let mut vfs = Vfs::new();
    vfs.write(pid, fd, buf)
}

/// vfs_close 便捷函数。
pub fn vfs_close(pid: u64, fd: u64) -> Result<(), String> {
    let mut vfs = Vfs::new();
    vfs.close(pid, fd)
}

/// vfs_unlink 便捷函数。
pub fn vfs_unlink(pid: u64, path: &str) -> Result<(), String> {
    let _ = pid;
    let comps = resolve_path(path);
    if comps.is_empty() {
        return Err("空路径".to_string());
    }
    let name = comps.last().unwrap();
    let parent = resolve_parent(&comps)?;
    dentry_lookup(parent, name).ok_or("文件未找到")?;
    remove_entry(parent, name)
}

/// vfs_stats 便捷函数。
pub fn vfs_stats() -> String {
    format!("vfs 就绪，fd 表项数={}", fd_registry().lock().unwrap().tables.len())
}

fn remove_entry(parent_ino: u64, name: &str) -> Result<(), String> {
    let _ = parent_ino;
    dentry_delete(parent_ino, name).then_some(()).ok_or("删除失败".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::inode::alloc_inode;
    use crate::fs::dentry::dentry_create;

    #[test]
    fn vfs_open_read_write_close() {
        let mut vfs = Vfs::new();
        // 预创建根目录条目
        let _ = dentry_create(ROOT_INO, "a.txt", InodeType::File);
        let ino = alloc_inode(InodeType::File);
        let _ = dentry_create(ROOT_INO, "a.txt", InodeType::File); // 会失败因为已存在，忽略

        let fd = vfs.open(1, "/a.txt", Access::Write);
        // 注意：open 会尝试 dentry_lookup，如果不存在则创建
        // 由于上面已经创建过，这里应该能找到
        let fd = match vfs.open(1, "/a.txt", Access::Write) {
            Ok(f) => f,
            Err(_) => {
                // 如果失败，创建新文件
                let ino = alloc_inode(InodeType::File);
                let _ = dentry_create(ROOT_INO, "a.txt", InodeType::File);
                vfs.open(1, "/a.txt", Access::Write).unwrap()
            }
        };
        assert!(vfs.has_fd(1, fd));
        let written = vfs.write(1, fd, b"hello vfs").unwrap();
        assert_eq!(written, 10);

        let fd2 = vfs.open(1, "/a.txt", Access::Read).unwrap();
        let mut buf = [0u8; 16];
        let n = vfs.read(1, fd2, &mut buf).unwrap();
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
        let _ = dentry_create(ROOT_INO, "p1.txt", InodeType::File);
        let _ = dentry_create(ROOT_INO, "p2.txt", InodeType::File);

        let f1 = vfs.open(1, "/p1.txt", Access::Write).unwrap();
        let f2 = vfs.open(2, "/p2.txt", Access::Write).unwrap();
        assert_eq!(vfs.fd_count(1), 1);
        assert_eq!(vfs.fd_count(2), 1);
        vfs.write(1, f1, b"proc-one").unwrap();
        vfs.write(2, f2, b"proc-two").unwrap();
        vfs.close(1, f1).unwrap();
        assert!(!vfs.has_fd(1, f1));
        assert!(vfs.has_fd(2, f2));
    }

    #[test]
    fn vfs_llseek_and_read() {
        let mut vfs = Vfs::new();
        let ino = alloc_inode(InodeType::File);
        let _ = dentry_create(ROOT_INO, "seek.txt", InodeType::File);
        let fd = vfs.open(1, "/seek.txt", Access::Write).unwrap();
        vfs.write(1, fd, b"0123456789").unwrap();
        let mut buf = [0u8; 3];
        vfs.read(1, fd, &mut buf).unwrap();
        assert_eq!(&buf, b"012");
        vfs.llseek(1, fd, 0, 2).unwrap();
        let mut buf2 = [0u8; 4];
        let n = vfs.read(1, fd, &mut buf2).unwrap();
        assert_eq!(n, 0);
        vfs.close(1, fd).unwrap();
    }

    #[test]
    fn vfs_close_invalid_fd_errors() {
        let mut vfs = Vfs::new();
        assert_eq!(vfs.close(1, 999).unwrap_err(), "fd 未找到".to_string());
        assert_eq!(
            vfs.read(1, 999, &mut [0u8; 4]).unwrap_err(),
            "fd 未找到".to_string()
        );
    }
}
