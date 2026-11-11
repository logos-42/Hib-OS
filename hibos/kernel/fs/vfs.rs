//! 文件描述符表与系统调用层（open/read/write/close/llseek）。
//!
//! 每进程独立 fd 表；open/read/write/close/llseek 与简单文件系统
//! 配合，为 `fs://` 通道提供 POSIX 风格的文件访问。

use std::collections::BTreeMap;

use super::dentry::{dentry_create, dentry_create_typed, dentry_delete, dentry_list, dentry_lookup, ROOT_INO};
use super::fd_table::{fd_alloc, fd_close, fd_count, fd_get, fd_registry, Access, FdTable};
use super::inode::{alloc_inode, free_inode, get_inode, get_inode_mut, InodeType};

/// 全局 VFS：按进程 ID 维护独立 fd 表。
pub struct Vfs {
    tables: BTreeMap<u64, FdTable>,
}

impl Vfs {
    pub fn new() -> Self {
        Self { tables: BTreeMap::new() }
    }

    /// open 系统调用：解析路径到 inode（不存在则创建文件），分配 fd。
    pub fn open(&mut self, pid: u64, path: &str, access: Access) -> Result<u64, String> {
        let comps = resolve_path(path);
        if comps.is_empty() {
            return Err("空路径".to_string());
        }
        let name = comps.last().unwrap().clone();
        let parent = resolve_parent(&comps)?;
        let ino = match dentry_lookup(parent, &name) {
            Some(ino) => ino,
            None => dentry_create(parent, &name, 0)?,
        };
        Ok(fd_alloc(pid, ino, access))
    }

    /// read 系统调用：从 fd 读入 buf，返回读取字节数。
    pub fn read(&mut self, pid: u64, fd: u64, buf: &mut [u8]) -> Result<u64, String> {
        let of = fd_get(pid, fd).ok_or("fd 未找到")?;
        if !of.access.can_read() {
            return Err("不可读".to_string());
        }
        let inode = get_inode(of.ino).ok_or("inode 未找到")?;
        let start = of.offset as usize;
        let end = (start + buf.len()).min(inode.data.len());
        let n = end.saturating_sub(start);
        if start <= inode.data.len() && n > 0 {
            buf[..n].copy_from_slice(&inode.data[start..end]);
        }
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
        let pos = of.offset as usize;
        let needed = pos + buf.len();
        get_inode_mut(of.ino, |inode| {
            if needed > inode.data.len() {
                inode.data.resize(needed, 0);
            }
            inode.data[pos..needed].copy_from_slice(buf);
            inode.size = needed as u64;
        })
        .ok_or("inode 未找到")?;
        {
            let mut g = fd_registry().lock().unwrap();
            if let Some(table) = g.tables.get_mut(&pid) {
                if let Some(of) = table.get_mut(fd) {
                    of.offset += buf.len() as u64;
                }
            }
        }
        Ok(buf.len() as u64)
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

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

// ----- 路径解析辅助 -----

fn resolve_path(path: &str) -> Vec<String> {
    let mut comps: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                comps.pop();
            }
            name => comps.push(name.to_string()),
        }
    }
    comps
}

fn resolve_parent(comps: &[String]) -> Result<u64, String> {
    let mut ino = ROOT_INO;
    if comps.len() <= 1 {
        return Ok(ino);
    }
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

/// 全局 VFS 单例（每次调用构造新实例，fd 状态在全局注册表中）。
pub fn vfs() -> Vfs {
    Vfs::new()
}

/// vfs_open 便捷函数。
pub fn vfs_open(pid: u64, path: &str, access: Access) -> Result<u64, String> {
    Vfs::new().open(pid, path, access)
}

/// vfs_read 便捷函数。
pub fn vfs_read(pid: u64, fd: u64, buf: &mut [u8]) -> Result<u64, String> {
    Vfs::new().read(pid, fd, buf)
}

/// vfs_write 便捷函数。
pub fn vfs_write(pid: u64, fd: u64, buf: &[u8]) -> Result<u64, String> {
    Vfs::new().write(pid, fd, buf)
}

/// vfs_close 便捷函数。
pub fn vfs_close(pid: u64, fd: u64) -> Result<(), String> {
    Vfs::new().close(pid, fd)
}

/// vfs_unlink 便捷函数。
pub fn vfs_unlink(_pid: u64, path: &str) -> Result<(), String> {
    let comps = resolve_path(path);
    if comps.is_empty() {
        return Err("空路径".to_string());
    }
    let name = comps.last().unwrap().clone();
    let parent = resolve_parent(&comps)?;
    let ino = dentry_lookup(parent, &name).ok_or("文件未找到")?;
    if dentry_delete(parent, &name) {
        let _ = ino;
        Ok(())
    } else {
        Err("删除失败".to_string())
    }
}

/// vfs_stats 便捷函数。
pub fn vfs_stats() -> String {
    let n = fd_registry().lock().unwrap().tables.len();
    format!("vfs 就绪，fd 表项数={}", n)
}

// =====================================================================
// 高层文件操作 —— `fs://vfs` 通道的落地实现
//
// 低层 open/read/write/close 要求调用方自己持有 fd 与偏移；通道层
// （`sense` / `act`）只关心"路径 → 内容"。以下函数全部复用上面的
// POSIX 实现与同一份 inode/dentry 表，不引入第二套状态。
// =====================================================================

/// 解析绝对路径到 inode 号（不分配 fd，不修改任何状态）。
///
/// 中间路径分量必须是目录；最后一个分量可以是文件或目录。
/// 空路径（`/`、`.`、`//`）解析为根目录。
pub fn resolve_inode(path: &str) -> Result<u64, String> {
    let comps = resolve_path(path);
    if comps.is_empty() {
        return Ok(ROOT_INO);
    }
    let mut ino = ROOT_INO;
    for (i, c) in comps.iter().enumerate() {
        let next = dentry_lookup(ino, c).ok_or_else(|| format!("未找到: {}", path))?;
        if i + 1 < comps.len() {
            let inode = get_inode(next).ok_or("inode 未找到")?;
            if !inode.is_dir() {
                return Err(format!("不是目录: {}", c));
            }
        }
        ino = next;
    }
    Ok(ino)
}

/// 一次性写入整个文件：不存在则创建，存在则从 0 偏移覆盖并截断。
///
/// 与 shell 的 `write` 语义一致（覆盖而非追加），但走真实 inode。
pub fn write_file(pid: u64, path: &str, data: &[u8]) -> Result<u64, String> {
    let mut v = Vfs::new();
    let fd = v.open(pid, path, Access::ReadWrite)?;
    let ino = fd_get(pid, fd).ok_or("fd 未找到")?.ino;
    // 截断：清空旧内容并把偏移归零，避免残留尾部字节。
    get_inode_mut(ino, |inode| {
        if inode.is_dir() {
            return;
        }
        inode.data.clear();
        inode.size = 0;
    })
    .ok_or("inode 未找到")?;
    v.llseek(pid, fd, 0, 0)?;
    let written = v.write(pid, fd, data);
    let _ = v.close(pid, fd);
    written
}

/// 读取整个文件内容。目录返回错误。
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let ino = resolve_inode(path)?;
    let inode = get_inode(ino).ok_or("inode 未找到")?;
    if inode.is_dir() {
        return Err(format!("是目录: {}", path));
    }
    Ok(inode.data)
}

/// 读取整个文件并解码为 UTF-8 文本。
pub fn read_file_text(path: &str) -> Result<String, String> {
    let data = read_file(path)?;
    String::from_utf8(data).map_err(|e| format!("非 UTF-8 内容: {}", e))
}

/// 列出目录内容：`(名字, inode 号, 是否目录)`，按名字排序。
pub fn list_dir(path: &str) -> Result<Vec<(String, u64, bool)>, String> {
    let ino = resolve_inode(path)?;
    let inode = get_inode(ino).ok_or("inode 未找到")?;
    if !inode.is_dir() {
        return Err(format!("不是目录: {}", path));
    }
    let mut out: Vec<(String, u64, bool)> = dentry_list(ino)
        .into_iter()
        .map(|(name, child)| {
            let is_dir = get_inode(child).map(|i| i.is_dir()).unwrap_or(false);
            (name, child, is_dir)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// 递归创建目录（等价 `mkdir -p`），返回最终 inode 号。
pub fn make_dir(path: &str) -> Result<u64, String> {
    let comps = resolve_path(path);
    if comps.is_empty() {
        return Ok(ROOT_INO);
    }
    let mut ino = ROOT_INO;
    for c in &comps {
        match dentry_lookup(ino, c) {
            Some(next) => {
                let inode = get_inode(next).ok_or("inode 未找到")?;
                if !inode.is_dir() {
                    return Err(format!("不是目录: {}", c));
                }
                ino = next;
            }
            None => {
                ino = dentry_create_typed(ino, c, InodeType::Dir)?;
            }
        }
    }
    Ok(ino)
}

/// 删除文件或空目录。非空目录拒绝删除（无 `rm -r` 语义）。
pub fn remove_path(path: &str) -> Result<(), String> {
    let comps = resolve_path(path);
    if comps.is_empty() {
        return Err("不能删除根目录".to_string());
    }
    let name = comps.last().unwrap().clone();
    let parent = resolve_parent(&comps)?;
    let ino = dentry_lookup(parent, &name).ok_or_else(|| format!("未找到: {}", path))?;
    let inode = get_inode(ino).ok_or("inode 未找到")?;
    if inode.is_dir() && !dentry_list(ino).is_empty() {
        return Err(format!("目录非空: {}", path));
    }
    if !dentry_delete(parent, &name) {
        return Err("删除失败".to_string());
    }
    free_inode(ino);
    Ok(())
}

/// 路径的元数据快照，形如 `ino=2 type=file size=10 mode=644 uid=0 gid=0`。
pub fn stat_path(path: &str) -> Result<String, String> {
    let ino = resolve_inode(path)?;
    let inode = get_inode(ino).ok_or("inode 未找到")?;
    let kind = match inode.kind {
        InodeType::File => "file",
        InodeType::Dir => "dir",
        InodeType::Symlink => "symlink",
    };
    Ok(format!(
        "ino={} type={} size={} mode={:o} uid={} gid={}",
        inode.ino, kind, inode.size, inode.mode, inode.uid, inode.gid
    ))
}

/// `fs://vfs` 的感知快照：inode 总数、根目录条目数、fd 表规模。
pub fn fs_sense_snapshot() -> String {
    let inodes = super::inode::inode_count();
    let root_entries = dentry_list(ROOT_INO).len();
    let tables = fd_registry().lock().unwrap().tables.len();
    let mut out = format!(
        "vfs inodes={} root_entries={} fd_tables={}\n",
        inodes, root_entries, tables
    );
    for (name, ino, is_dir) in list_dir("/").unwrap_or_default() {
        out.push_str(&format!(
            "  {}{} ino={}\n",
            name,
            if is_dir { "/" } else { "" },
            ino
        ));
    }
    out
}

/// `fs://vfs` 的行动分发。
///
/// 用法：
/// * `write <路径> <内容>` —— 覆盖写入文本
/// * `read <路径>`          —— 读取文本
/// * `ls [路径]`            —— 列目录
/// * `mkdir <路径>`         —— 递归建目录
/// * `rm <路径>`            —— 删除文件或空目录
/// * `stat <路径>`          —— 元数据
pub fn fs_act_dispatch(payload: &str) -> Result<String, String> {
    let payload = payload.trim();
    let (cmd, rest) = match payload.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (payload, ""),
    };
    match cmd {
        "write" => {
            let (path, content) = rest
                .split_once(char::is_whitespace)
                .ok_or("用法: write <路径> <内容>")?;
            let n = write_file(0, path, content.as_bytes())?;
            Ok(format!("已写入 {} 字节 -> {}", n, path))
        }
        "read" => {
            if rest.is_empty() {
                return Err("用法: read <路径>".to_string());
            }
            Ok(read_file_text(rest)?)
        }
        "ls" => {
            let path = if rest.is_empty() { "/" } else { rest };
            let entries = list_dir(path)?;
            let mut out = format!("{}: {} 项\n", path, entries.len());
            for (name, ino, is_dir) in entries {
                out.push_str(&format!(
                    "  {}{} ino={}\n",
                    name,
                    if is_dir { "/" } else { "" },
                    ino
                ));
            }
            Ok(out)
        }
        "mkdir" => {
            if rest.is_empty() {
                return Err("用法: mkdir <路径>".to_string());
            }
            let ino = make_dir(rest)?;
            Ok(format!("已创建目录 {} ino={}", rest, ino))
        }
        "rm" => {
            if rest.is_empty() {
                return Err("用法: rm <路径>".to_string());
            }
            remove_path(rest)?;
            Ok(format!("已删除 {}", rest))
        }
        "stat" => {
            if rest.is_empty() {
                return Err("用法: stat <路径>".to_string());
            }
            Ok(format!("{} {}", rest, stat_path(rest)?))
        }
        other => Err(format!("未知操作: {}（write/read/ls/mkdir/rm/stat）", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vfs_open_read_write_close() {
        let mut v = Vfs::new();
        let fd = v.open(1, "/a.txt", Access::Write).unwrap();
        assert!(v.has_fd(1, fd));
        let written = v.write(1, fd, b"hello vfs").unwrap();
        assert_eq!(written, 10);

        let fd2 = v.open(1, "/a.txt", Access::Read).unwrap();
        let mut buf = [0u8; 16];
        let n = v.read(1, fd2, &mut buf).unwrap();
        assert_eq!(n, 10);
        assert_eq!(&buf[..n as usize], b"hello vfs");

        v.close(1, fd).unwrap();
        v.close(1, fd2).unwrap();
        assert!(!v.has_fd(1, fd));
        assert_eq!(v.fd_count(1), 0);
    }

    #[test]
    fn vfs_fd_isolation_per_process() {
        let mut v = Vfs::new();
        let f1 = v.open(101, "/p1.txt", Access::Write).unwrap();
        let f2 = v.open(102, "/p2.txt", Access::Write).unwrap();
        assert_eq!(v.fd_count(101), 1);
        assert_eq!(v.fd_count(102), 1);
        v.write(101, f1, b"proc-one").unwrap();
        v.write(102, f2, b"proc-two").unwrap();
        v.close(101, f1).unwrap();
        assert!(!v.has_fd(101, f1));
        assert!(v.has_fd(102, f2));
    }

    #[test]
    fn vfs_llseek_and_read() {
        let mut v = Vfs::new();
        let fd = v.open(201, "/seek.txt", Access::ReadWrite).unwrap();
        v.write(201, fd, b"0123456789").unwrap();
        v.llseek(201, fd, 0, 0).unwrap();
        let mut buf = [0u8; 3];
        v.read(201, fd, &mut buf).unwrap();
        assert_eq!(&buf, b"012");
        v.llseek(201, fd, 0, 2).unwrap();
        let mut buf2 = [0u8; 4];
        let n = v.read(201, fd, &mut buf2).unwrap();
        assert_eq!(n, 0);
        v.close(201, fd).unwrap();
    }

    #[test]
    fn vfs_close_invalid_fd_errors() {
        let mut v = Vfs::new();
        assert_eq!(v.close(1, 999).unwrap_err(), "fd 未找到".to_string());
        assert_eq!(v.read(1, 999, &mut [0u8; 4]).unwrap_err(), "fd 未找到".to_string());
    }
}
