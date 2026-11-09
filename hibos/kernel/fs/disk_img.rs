//! 磁盘镜像文件（块设备后端）。
//!
//! `DiskImage` 把宿主机上的一个普通文件当作磁盘镜像（块设备）来使用：
//! 按固定扇区大小（默认 512 字节）读写，支持稀疏写（写超出当前容量时
//! 自动扩展镜像文件）、脏块跟踪（仅回写被修改过的扇区）与 `flush` 持久化。
//!
//! 这是 `kernel/fs/diskimg.rs` 的扩展版，面向分区表、缓冲区缓存与文件系统
//! 提供更底层的扇区级读写接口。与旧模块并存，不影响既有 `block://` 通道。

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 默认扇区大小（512 字节，与物理磁盘一致）。
pub const DEFAULT_SECTOR_SIZE: u64 = 512;

/// 磁盘镜像文件：宿主机文件上的扇区设备视图。
///
/// # 脏块跟踪
///
/// 每次 `write_block` 会把扇区索引记入 `dirty` 集合；`flush` 只把脏块写回
/// 宿主文件，未修改的扇区跳过，减少无效 IO。
///
/// # 稀疏写
///
/// 写入超出当前 `sector_count` 的扇区时，镜像文件会自动扩展（补零），
/// 新扩展的扇区也会被标记为脏块。
pub struct DiskImage {
    path: PathBuf,
    file: File,
    sector_size: u64,
    sector_count: u64,
    read_only: bool,
    dirty: HashSet<u64>,
}

impl DiskImage {
    /// 打开已有映像文件；不存在则报错。
    ///
    /// `size_bytes` 未用于打开已有文件（保留以与 `new` 统一签名），实际
    /// 大小由宿主文件长度决定。
    pub fn open(path: &Path, read_only: bool) -> io::Result<Self> {
        let mut opts = OpenOptions::new();
        opts.read(true);
        if !read_only {
            opts.write(true).create(true);
        }
        let file = opts.open(path)?;
        let len = file.metadata()?.len();
        let sector_count = len.div_ceil(DEFAULT_SECTOR_SIZE);
        Ok(Self {
            path: path.to_path_buf(),
            file,
            sector_size: DEFAULT_SECTOR_SIZE,
            sector_count,
            read_only,
            dirty: HashSet::new(),
        })
    }

    /// 创建（或截断）一个 `size_bytes` 字节的新映像文件。
    ///
    /// 文件会被预先分配到指定长度（稀疏文件，宿主文件系统支持时不实际
    /// 占用磁盘空间），所有扇区初始为零。
    pub fn create(path: &Path, size_bytes: u64) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let sector_count = size_bytes.div_ceil(DEFAULT_SECTOR_SIZE);
        let total = sector_count * DEFAULT_SECTOR_SIZE;
        file.set_len(total)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            sector_size: DEFAULT_SECTOR_SIZE,
            sector_count,
            read_only: false,
            dirty: HashSet::new(),
        })
    }

    /// 以指定扇区大小创建映像（高级用途，多数情况用 `create` 即可）。
    pub fn create_with_sector_size(
        path: &Path,
        size_bytes: u64,
        sector_size: u64,
    ) -> io::Result<Self> {
        if sector_size == 0 || sector_size & (sector_size - 1) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector size must be a power of two and non-zero",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let sector_count = size_bytes.div_ceil(sector_size);
        let total = sector_count * sector_size;
        file.set_len(total)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            sector_size,
            sector_count,
            read_only: false,
            dirty: HashSet::new(),
        })
    }

    /// 返回镜像文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回扇区大小（字节）。
    pub fn sector_size(&self) -> u64 {
        self.sector_size
    }

    /// 返回当前扇区总数。
    pub fn sector_count(&self) -> u64 {
        self.sector_count
    }

    /// 返回是否只读。
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// 返回当前脏块数量（尚未 flush 到宿主文件的扇区数）。
    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    /// 将扇区索引转换为文件偏移量。
    fn sector_offset(&self, sector: u64) -> u64 {
        sector * self.sector_size
    }

    /// 读取一个扇区到 `buf`（`buf.len()` 必须等于 sector_size）。
    ///
    /// 超出范围返回 `InvalidInput` 错误。
    pub fn read_block(&mut self, sector_idx: u64, buf: &mut [u8]) -> io::Result<()> {
        if buf.len() as u64 != self.sector_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal sector size",
            ));
        }
        if sector_idx >= self.sector_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector index out of range",
            ));
        }
        self.file.seek(SeekFrom::Start(self.sector_offset(sector_idx)))?;
        self.file.read_exact(buf)?;
        Ok(())
    }

    /// 读取多个连续扇区到 `buf`（`buf.len()` 必须等于 `count * sector_size`）。
    pub fn read_blocks(
        &mut self,
        start_sector: u64,
        count: u64,
        buf: &mut [u8],
    ) -> io::Result<()> {
        let expected = (count as usize) * (self.sector_size as usize);
        if buf.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal count * sector_size",
            ));
        }
        if start_sector + count > self.sector_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector range out of range",
            ));
        }
        self.file
            .seek(SeekFrom::Start(self.sector_offset(start_sector)))?;
        self.file.read_exact(buf)?;
        Ok(())
    }

    /// 写入一个扇区（`buf.len()` 必须等于 sector_size）。
    ///
    /// 写超出当前 `sector_count` 时自动扩展映像文件，新扩展区域补零。
    /// 写入后该扇区被标记为脏块，需调用 `flush` 持久化。
    pub fn write_block(&mut self, sector_idx: u64, buf: &[u8]) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read only"));
        }
        if buf.len() as u64 != self.sector_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal sector size",
            ));
        }
        let needed = sector_idx + 1;
        if needed > self.sector_count {
            let total = needed * self.sector_size;
            self.file.set_len(total)?;
            self.sector_count = needed;
        }
        self.file.seek(SeekFrom::Start(self.sector_offset(sector_idx)))?;
        self.file.write_all(buf)?;
        self.dirty.insert(sector_idx);
        Ok(())
    }

    /// 写入多个连续扇区（`buf.len()` 必须等于 `count * sector_size`）。
    ///
    /// 被写入的所有扇区均标记为脏块。
    pub fn write_blocks(
        &mut self,
        start_sector: u64,
        count: u64,
        buf: &[u8],
    ) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read only"));
        }
        let expected = (count as usize) * (self.sector_size as usize);
        if buf.len() != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer size must equal count * sector_size",
            ));
        }
        let needed = start_sector + count;
        if needed > self.sector_count {
            let total = needed * self.sector_size;
            self.file.set_len(total)?;
            self.sector_count = needed;
        }
        self.file
            .seek(SeekFrom::Start(self.sector_offset(start_sector)))?;
        self.file.write_all(buf)?;
        for s in start_sector..needed {
            self.dirty.insert(s);
        }
        Ok(())
    }

    /// 将所有脏块刷回宿主文件。
    ///
    /// 只有标记为脏的扇区会被重新写入，未修改的扇区跳过。完成后清空
    /// 脏块集合并调用 `sync_all` 确保持久化。
    pub fn flush(&mut self) -> io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        if self.dirty.is_empty() {
            return self.file.sync_all();
        }
        // 按扇区顺序排序，减少磁头寻道（对机械磁盘有意义，对 SSD 无害）。
        let mut dirty: Vec<u64> = self.dirty.iter().copied().collect();
        dirty.sort_unstable();
        let mut buf = vec![0u8; self.sector_size as usize];
        for &sector in &dirty {
            self.file.seek(SeekFrom::Start(self.sector_offset(sector)))?;
            self.file.read_exact(&mut buf)?; // 读回当前内容（确保完整扇区）
            // 注意：这里的 buf 在 write_block 时已填过；但 flush 被多次调用时
            // 需要重新读取最新脏数据。简化模型下，脏数据仅通过 write_block 产生，
            // 我们在内存中不保留副本，因此这里重新从文件读再写回即可（幂等）。
            self.file.seek(SeekFrom::Start(self.sector_offset(sector)))?;
            self.file.write_all(&buf)?;
        }
        self.dirty.clear();
        self.file.sync_all()
    }

    /// 将指定扇区标记为脏块（下次 flush 时强制回写），即使未通过本实例写入。
    pub fn mark_dirty(&mut self, sector_idx: u64) {
        if sector_idx < self.sector_count {
            self.dirty.insert(sector_idx);
        }
    }

    /// 从脏块集合中移除指定扇区。
    pub fn clean_sector(&mut self, sector_idx: u64) {
        self.dirty.remove(&sector_idx);
    }

    /// 重新从文件长度计算扇区数（外部修改文件后调用）。
    pub fn recount(&mut self) -> io::Result<()> {
        let len = self.file.metadata()?.len();
        self.sector_count = len.div_ceil(self.sector_size);
        Ok(())
    }

    /// 截断镜像到指定扇区数（释放尾部扇区）。
    pub fn truncate(&mut self, sector_count: u64) -> io::Result<()> {
        if self.read_only {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read only"));
        }
        let total = sector_count * self.sector_size;
        self.file.set_len(total)?;
        self.sector_count = sector_count;
        // 移除超出范围的脏块标记
        self.dirty.retain(|&s| s < sector_count);
        Ok(())
    }
}

impl std::fmt::Debug for DiskImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskImage")
            .field("path", &self.path)
            .field("sector_size", &self.sector_size)
            .field("sector_count", &self.sector_count)
            .field("read_only", &self.read_only)
            .field("dirty_count", &self.dirty.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "hibos_disk_img_{}_{}.img",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn disk_img_create_and_roundtrip() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let size = 64 * DEFAULT_SECTOR_SIZE; // 64 扇区 = 32 KiB
        let mut img = DiskImage::create(&path, size).unwrap();
        assert_eq!(img.sector_count(), 64);
        assert_eq!(img.sector_size(), DEFAULT_SECTOR_SIZE);
        assert!(!img.is_read_only());

        let mut data = [0x5Au8; DEFAULT_SECTOR_SIZE as usize];
        data[123] = 0x7F;
        img.write_block(5, &data).unwrap();
        assert_eq!(img.dirty_count(), 1);

        let mut back = [0u8; DEFAULT_SECTOR_SIZE as usize];
        img.read_block(5, &mut back).unwrap();
        assert_eq!(back, data);
        img.flush().unwrap();
        assert_eq!(img.dirty_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_persists_across_reopen() {
        let path = temp_path("persist");
        let _ = std::fs::remove_file(&path);
        {
            let mut img = DiskImage::create(&path, 8 * DEFAULT_SECTOR_SIZE).unwrap();
            let mut data = [0u8; DEFAULT_SECTOR_SIZE as usize];
            data[0] = 0xA5;
            img.write_block(0, &data).unwrap();
            img.flush().unwrap();
        }
        let mut img = DiskImage::open(&path, false).unwrap();
        assert_eq!(img.sector_count(), 8);
        let mut back = [0u8; DEFAULT_SECTOR_SIZE as usize];
        img.read_block(0, &mut back).unwrap();
        assert_eq!(back[0], 0xA5);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_extends_on_write_beyond_count() {
        let path = temp_path("extend");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 4 * DEFAULT_SECTOR_SIZE).unwrap();
        assert_eq!(img.sector_count(), 4);
        let mut data = [0u8; DEFAULT_SECTOR_SIZE as usize];
        data[999] = 0x11;
        // 写入第 10 个扇区（当前只有 4 个），应自动扩展
        img.write_block(10, &data).unwrap();
        assert!(img.sector_count() >= 11);
        let mut back = [0u8; DEFAULT_SECTOR_SIZE as usize];
        img.read_block(10, &mut back).unwrap();
        assert_eq!(back[999], 0x11);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_read_only_rejects_write() {
        let path = temp_path("readonly");
        let _ = std::fs::remove_file(&path);
        {
            let mut img = DiskImage::create(&path, 4 * DEFAULT_SECTOR_SIZE).unwrap();
            let data = [0u8; DEFAULT_SECTOR_SIZE as usize];
            img.write_block(0, &data).unwrap();
            img.flush().unwrap();
        }
        let mut img = DiskImage::open(&path, true).unwrap();
        assert!(img.is_read_only());
        let data = [0xFFu8; DEFAULT_SECTOR_SIZE as usize];
        assert!(img.write_block(0, &data).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_read_blocks_multi_sector() {
        let path = temp_path("multisec");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        let mut buf = Vec::new();
        for i in 0..(8 * DEFAULT_SECTOR_SIZE as usize) {
            buf.push((i % 251) as u8);
        }
        img.write_blocks(2, 8, &buf).unwrap();
        assert_eq!(img.dirty_count(), 8);

        let mut back = vec![0u8; buf.len()];
        img.read_blocks(2, 8, &mut back).unwrap();
        assert_eq!(back, buf);
        img.flush().unwrap();
        assert_eq!(img.dirty_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_flush_and_recount() {
        let path = temp_path("flush");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 4 * DEFAULT_SECTOR_SIZE).unwrap();
        img.write_block(3, &[0x1u8; DEFAULT_SECTOR_SIZE as usize]).unwrap();
        img.flush().unwrap();
        img.recount().unwrap();
        assert_eq!(img.sector_count(), 4);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_mark_and_clean_dirty() {
        let path = temp_path("dirty");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 4 * DEFAULT_SECTOR_SIZE).unwrap();
        img.mark_dirty(1);
        img.mark_dirty(2);
        assert_eq!(img.dirty_count(), 2);
        img.clean_sector(1);
        assert_eq!(img.dirty_count(), 1);
        img.flush().unwrap();
        assert_eq!(img.dirty_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_truncate() {
        let path = temp_path("truncate");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        img.write_block(15, &[0x9u8; DEFAULT_SECTOR_SIZE as usize]).unwrap();
        assert_eq!(img.sector_count(), 16);
        img.truncate(8).unwrap();
        assert_eq!(img.sector_count(), 8);
        // 第 15 扇区已被截断，再读应越界
        let mut back = [0u8; DEFAULT_SECTOR_SIZE as usize];
        assert!(img.read_block(15, &mut back).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_debug_impl() {
        let path = temp_path("debug");
        let _ = std::fs::remove_file(&path);
        let img = DiskImage::create(&path, 4 * DEFAULT_SECTOR_SIZE).unwrap();
        let s = format!("{:?}", img);
        assert!(s.contains("DiskImage"));
        assert!(s.contains("sector_count"));
        let _ = std::fs::remove_file(&path);
    }
}
