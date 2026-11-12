//! 磁盘映像文件 —— fs/diskimg 子模块。
//!
//! 提供对磁盘映像文件的块级读写，供 `FileBlockDevice` 使用。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// 磁盘映像文件句柄。
pub struct DiskImage {
    file: File,
    block_count: u64,
    block_size: u64,
    read_only: bool,
}

impl DiskImage {
    /// 打开现有磁盘映像文件。
    pub fn open(path: &Path, read_only: bool) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(!read_only)
            .open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();
        let block_size = 4096u64;
        let block_count = file_size.div_ceil(block_size);
        Ok(Self {
            file,
            block_count,
            block_size,
            read_only,
        })
    }

    /// 创建新的磁盘映像文件（预分配指定块数）。
    pub fn create(path: &Path, blocks: u64) -> io::Result<Self> {
        let block_size = 4096u64;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let total_bytes = blocks * block_size;
        file.set_len(total_bytes)?;
        file.sync_all()?;
        Ok(Self {
            file,
            block_count: blocks,
            block_size,
            read_only: false,
        })
    }

    /// 块数量。
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// 块大小（字节）。
    pub fn block_size(&self) -> u64 {
        self.block_size
    }

    /// 读取一个块。
    pub fn read_block_at(&mut self, block: u64, buf: &mut [u8]) -> io::Result<()> {
        let offset = block * self.block_size;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(buf)?;
        Ok(())
    }

    /// 写入一个块。
    pub fn write_block_at(&mut self, block: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
        }
        let offset = block * self.block_size;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buf)?;
        Ok(())
    }

    /// 刷新到磁盘。
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_path() -> PathBuf {
        std::env::temp_dir().join(format!("hibos_diskimg_test_{}.img", std::process::id()))
    }

    #[test]
    fn create_and_read_write_block() {
        let path = tmp_path();
        let _ = fs::remove_file(&path);
        {
            let mut img = DiskImage::create(&path, 16).unwrap();
            assert_eq!(img.block_count(), 16);
            let mut data = [0u8; 4096];
            data[0] = 0xAA;
            data[100] = 0xBB;
            img.write_block_at(3, &data).unwrap();
            img.flush().unwrap();
        }
        {
            let mut img = DiskImage::open(&path, false).unwrap();
            assert_eq!(img.block_count(), 16);
            let mut back = [0u8; 4096];
            img.read_block_at(3, &mut back).unwrap();
            assert_eq!(back[0], 0xAA);
            assert_eq!(back[100], 0xBB);
        }
        let _ = fs::remove_file(&path);
    }
}
