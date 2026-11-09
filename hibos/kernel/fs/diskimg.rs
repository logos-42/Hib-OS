//! 磁盘映像读写与持久化。
//!
//! `DiskImage` 封装宿主机上的一个普通文件作为块设备：按固定块大小
//! 读写，自动扩展文件长度，并支持 `flush` 将页缓存回写到底层文件。
//! 磁盘映像即文件系统的持久化载体：启动时加载，运行时回写。

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 标准块大小（4 KiB）。
pub const BLOCK_SIZE: usize = 4096;

/// 磁盘映像文件：宿主机文件上的块设备视图。
pub struct DiskImage {
    path: PathBuf,
    file: File,
    block_size: usize,
    block_count: u64,
    read_only: bool,
}

impl DiskImage {
    /// 打开已有映像文件；不存在则报错。
    pub fn open(path: &Path, read_only: bool) -> io::Result<Self> {
        let mut opts = OpenOptions::new();
        opts.read(true);
        if !read_only {
            opts.write(true).create(true);
        }
        let file = opts.open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            block_size: BLOCK_SIZE,
            block_count: len.div_ceil(BLOCK_SIZE as u64),
            read_only,
        })
    }

    /// 创建（或截断）一个 `blocks` 块的新映像文件。
    pub fn create(path: &Path, blocks: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let total = (blocks as u64) * (BLOCK_SIZE as u64);
        file.set_len(total)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            block_size: BLOCK_SIZE,
            block_count: blocks,
            read_only: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// 读取一个块到 `buf`（`buf.len()` 必须等于 block_size）。
    pub fn read_block_at(&mut self, block: u64, buf: &mut [u8]) -> io::Result<()> {
        if buf.len() != self.block_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal block size",
            ));
        }
        if block >= self.block_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "block index out of range",
            ));
        }
        self.file
            .seek(SeekFrom::Start(block * (self.block_size as u64)))?;
        self.file.read_exact(buf)?;
        Ok(())
    }

    /// 写入一个块（`buf.len()` 必须等于 block_size）。写超出当前 `block_count`
    /// 时自动扩展映像文件。
    pub fn write_block_at(&mut self, block: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read only"));
        }
        if buf.len() != self.block_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal block size",
            ));
        }
        let needed = (block + 1) as u64;
        if needed > self.block_count {
            let total = needed * (self.block_size as u64);
            self.file.set_len(total)?;
            self.block_count = needed;
        }
        self.file
            .seek(SeekFrom::Start(block * (self.block_size as u64)))?;
        self.file.write_all(buf)?;
        Ok(())
    }

    /// 将缓冲区写回磁盘文件。
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// 重新从文件长度计算块数（外部修改文件后调用）。
    pub fn recount(&mut self) -> io::Result<()> {
        let len = self.file.metadata()?.len();
        self.block_count = len.div_ceil(BLOCK_SIZE as u64);
        Ok(())
    }
}

impl std::fmt::Debug for DiskImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskImage")
            .field("path", &self.path)
            .field("block_size", &self.block_size)
            .field("block_count", &self.block_count)
            .field("read_only", &self.read_only)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hibos_diskimg_{}_{}.img", name, std::process::id()))
    }

    #[test]
    fn diskimg_create_and_roundtrip() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 16).unwrap();
        assert_eq!(img.block_count(), 16);
        assert_eq!(img.block_size(), BLOCK_SIZE);

        let mut data = [0x5Au8; BLOCK_SIZE];
        data[123] = 0x7F;
        img.write_block_at(5, &data).unwrap();

        let mut back = [0u8; BLOCK_SIZE];
        img.read_block_at(5, &mut back).unwrap();
        assert_eq!(back, data);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diskimg_persists_across_reopen() {
        let path = temp_path("persist");
        let _ = std::fs::remove_file(&path);
        {
            let mut img = DiskImage::create(&path, 8).unwrap();
            let mut data = [0u8; BLOCK_SIZE];
            data[0] = 0xA5;
            img.write_block_at(0, &data).unwrap();
            img.flush().unwrap();
        }
        let mut img = DiskImage::open(&path, false).unwrap();
        assert_eq!(img.block_count(), 8);
        let mut back = [0u8; BLOCK_SIZE];
        img.read_block_at(0, &mut back).unwrap();
        assert_eq!(back[0], 0xA5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diskimg_extends_on_write_beyond_count() {
        let path = temp_path("extend");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 4).unwrap();
        assert_eq!(img.block_count(), 4);
        let mut data = [0u8; BLOCK_SIZE];
        data[999] = 0x11;
        // 写入第 10 块（当前只有 4 块），应自动扩展
        img.write_block_at(10, &data).unwrap();
        assert!(img.block_count() >= 11);
        let mut back = [0u8; BLOCK_SIZE];
        img.read_block_at(10, &mut back).unwrap();
        assert_eq!(back[999], 0x11);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diskimg_read_only_rejects_write() {
        let path = temp_path("readonly");
        let _ = std::fs::remove_file(&path);
        {
            let mut img = DiskImage::create(&path, 4).unwrap();
            let data = [0u8; BLOCK_SIZE];
            img.write_block_at(0, &data).unwrap();
            img.flush().unwrap();
        }
        let mut img = DiskImage::open(&path, true).unwrap();
        assert!(img.is_read_only());
        let data = [0xFFu8; BLOCK_SIZE];
        assert!(img.write_block_at(0, &data).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn diskimg_flush_and_recount() {
        let path = temp_path("flush");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 4).unwrap();
        img.write_block_at(3, &[0x1u8; BLOCK_SIZE]).unwrap();
        img.flush().unwrap();
        img.recount().unwrap();
        assert_eq!(img.block_count(), 4);
        let _ = std::fs::remove_file(&path);
    }
}
