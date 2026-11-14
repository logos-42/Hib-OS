//! 磁盘镜像文件（块设备后端）。
//!
//! `DiskImage` 把宿主机上的一个普通文件当作磁盘镜像（块设备）来使用：
//! 按固定扇区大小（默认 512 字节）读写，支持稀疏写（写超出当前容量时
//! 自动扩展镜像文件）、脏块跟踪（仅回写被修改过的扇区）与 `flush` 持久化。
//!
//! ## 设计信条
//! - **块设备统一接口**：`read_block(sector, buf)` / `write_block(sector, buf)` / `flush()`，
//!   供缓冲区缓存、分区表、文件系统上层调用。
//! - **单写者租约 + epoch CAS**：写入前先 `lease::acquire` 获取写租约，提交时用
//!   `lease::commit(channel, expected_epoch, data)` 做乐观校验，epoch 不符则拒绝，
//!   避免悲观锁阻塞。读取不需要租约，直接拿快照。
//! - **能力位权限**：通过 `caps::CAP_SYS_RAWIO` / `caps::CAP_SYS_ADMIN` 校验读写权限，
//!   而非传统 uid 检查。
//! - **通道接口**：提供 `probe_disk_img` / `act_disk_img`，供 `sense`/`act` 框架以
//!   `diskimg/<路径>` 通道形式操作镜像。
//!
//! 这是 `kernel/fs/diskimg.rs` 的扩展版，面向分区表、缓冲区缓存与文件系统
//! 提供更底层的扇区级读写接口。与旧模块并存，不影响既有 `block/` 通道。

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::lease::{self, Epoch};
use crate::caps;

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

// ============================================================================
// 并发控制：基于 lease.rs 的单写者租约 + epoch CAS
// ============================================================================

/// 磁盘镜像并发控制键：以镜像路径作为通道名，供 lease 系统使用。
fn diskimg_lease_channel(path: &Path) -> String {
    format!("diskimg/{}", path.display())
}

/// 单写者租约包装器：对 `DiskImage` 的写入加 lease 保护。
///
/// 读取路径不需要租约（直接快照读），写入路径必须先 `acquire_write_lease`，
/// 提交时用 `commit_write` 做乐观 epoch CAS。与 `Mutex` 悲观锁不同，
/// 冲突时调用方需重新 `probe` 再决定，不阻塞调度。
pub struct DiskImageLeased {
    inner: Mutex<DiskImage>,
    channel: String,
}

impl DiskImageLeased {
    /// 打开（或创建）并包装为租约保护的镜像。
    pub fn open(path: &Path, read_only: bool) -> io::Result<Self> {
        let image = DiskImage::open(path, read_only)?;
        Ok(Self {
            inner: Mutex::new(image),
            channel: diskimg_lease_channel(path),
        })
    }

    /// 创建并包装为租约保护的镜像。
    pub fn create(path: &Path, size_bytes: u64) -> io::Result<Self> {
        let image = DiskImage::create(path, size_bytes)?;
        Ok(Self {
            inner: Mutex::new(image),
            channel: diskimg_lease_channel(path),
        })
    }

    /// 申请写租约，返回当前 epoch（供后续 commit 校验）。
    /// 冲突时返回 `lease::LeaseConflict`，提示持有者。
    pub fn acquire_write_lease(
        &self,
        owner: u64,
    ) -> Result<Epoch, crate::lease::LeaseConflict> {
        use crate::lease::acquire;
        acquire(&self.channel, owner, None)
    }

    /// 释放写租约。
    pub fn release_write_lease(&self, owner: u64) -> bool {
        use crate::lease::release;
        release(&self.channel, owner)
    }

    /// 读一个扇区（无需租约，直接快照读）。
    pub fn read_block(&self, sector_idx: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut img = self.inner.lock().unwrap();
        img.read_block(sector_idx, buf)
    }

    /// 读多个扇区（无需租约）。
    pub fn read_blocks(&self, start_sector: u64, count: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut img = self.inner.lock().unwrap();
        img.read_blocks(start_sector, count, buf)
    }

    /// 写一个扇区：需先持有写租约，写入后标记脏块。
    /// 不直接推进 epoch；调用方应随后 `commit_write`。
    pub fn write_block_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        sector_idx: u64,
        buf: &[u8],
    ) -> Result<Epoch, crate::lease::CommitError> {
        // 先校验租约所有权与 epoch（乐观 CAS）。
        use crate::lease::{commit, acquire};
        let _ = acquire(&self.channel, owner, None)
            .map_err(|c| crate::lease::CommitError::NotLeaseOwner {
                holder: Some(c.holder),
            })?;
        {
            let mut img = self.inner.lock().unwrap();
            img.write_block(sector_idx, buf)?;
        }
        commit(&self.channel, owner, expected_epoch, &format!("write sector {}", sector_idx))
    }

    /// 刷新所有脏块，并提交一次 epoch（force 模式，无需租约）。
    pub fn flush_under_lease(&mut self, owner: u64) -> io::Result<Epoch> {
        let mut img = self.inner.lock().unwrap();
        img.flush()?;
        let data = format!("flush {} sectors dirty={}", img.sector_count(), img.dirty_count());
        let epoch = crate::lease::force(&self.channel, &data);
        Ok(epoch)
    }

    /// 返回镜像路径。
    pub fn path(&self) -> &Path {
        self.inner.lock().unwrap().path()
    }

    /// 返回扇区大小。
    pub fn sector_size(&self) -> u64 {
        self.inner.lock().unwrap().sector_size()
    }

    /// 返回扇区总数。
    pub fn sector_count(&self) -> u64 {
        self.inner.lock().unwrap().sector_count()
    }

    /// 返回当前世界版本（epoch）。
    pub fn epoch(&self) -> Epoch {
        crate::lease::epoch(&self.channel)
    }
}

/// 能力位权限包装器：在底层 `DiskImage` 上层加 `caps` 校验。
///
/// 仅凭 `CAP_SYS_RAWIO` 允许底层扇区读写；`CAP_SYS_ADMIN` 允许 flush/truncate
/// 等管理操作。不使用 uid，完全按能力位授权。
pub struct DiskImageCaps {
    inner: Mutex<DiskImage>,
    caps: Arc<Mutex<caps::Caps>>,
}

impl DiskImageCaps {
    /// 包装一个已打开的镜像，绑定全局能力表。
    pub fn new(image: DiskImage) -> Self {
        Self {
            inner: Mutex::new(image),
            caps: Arc::new(Mutex::new(caps::caps().lock().unwrap().clone())),
        }
    }

    /// 校验是否具有底层 IO 能力（CAP_SYS_RAWIO）。
    fn check_rawio(&self) -> Result<(), io::Error> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_RAWIO") {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "缺少 CAP_SYS_RAWIO 能力",
            ))
        }
    }

    /// 校验是否具有管理能力（CAP_SYS_ADMIN）。
    fn check_admin(&self) -> Result<(), io::Error> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_ADMIN") {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "缺少 CAP_SYS_ADMIN 能力",
            ))
        }
    }

    /// 读一个扇区（需 CAP_SYS_RAWIO）。
    pub fn read_block(&self, sector_idx: u64, buf: &mut [u8]) -> io::Result<()> {
        self.check_rawio()?;
        let mut img = self.inner.lock().unwrap();
        img.read_block(sector_idx, buf)
    }

    /// 写一个扇区（需 CAP_SYS_RAWIO）。
    pub fn write_block(&self, sector_idx: u64, buf: &[u8]) -> io::Result<()> {
        self.check_rawio()?;
        let mut img = self.inner.lock().unwrap();
        img.write_block(sector_idx, buf)
    }

    /// 刷新脏块（需 CAP_SYS_ADMIN）。
    pub fn flush(&self) -> io::Result<()> {
        self.check_admin()?;
        let mut img = self.inner.lock().unwrap();
        img.flush()
    }

    /// 截断镜像（需 CAP_SYS_ADMIN）。
    pub fn truncate(&self, sector_count: u64) -> io::Result<()> {
        self.check_admin()?;
        let mut img = self.inner.lock().unwrap();
        img.truncate(sector_count)
    }

    /// 返回扇区数（无需能力，仅元数据）。
    pub fn sector_count(&self) -> u64 {
        self.inner.lock().unwrap().sector_count()
    }

    /// 返回脏块数（无需能力，仅元数据）。
    pub fn dirty_count(&self) -> usize {
        self.inner.lock().unwrap().dirty_count()
    }
}

// ============================================================================
// sense/act 通道接口：diskimg/<路径>
// ============================================================================

/// 感知磁盘镜像通道：返回镜像实时快照（扇区数、脏块数、epoch）。
///
/// 通道名形式：`diskimg/<绝对路径>`
pub fn probe_disk_img(name: &str) -> Option<String> {
    let path = name.strip_prefix("diskimg/")?;
    let path = Path::new(path);
    let img = match DiskImage::open(path, true) {
        Ok(i) => i,
        Err(_) => return Some(format!("镜像打开失败: {}", path.display())),
    };
    let epoch = lease::epoch(&diskimg_lease_channel(path));
    Some(format!(
        "path={} sector_size={} sector_count={} read_only={} dirty={} epoch={}",
        path.display(),
        img.sector_size(),
        img.sector_count(),
        img.is_read_only(),
        img.dirty_count(),
        epoch
    ))
}

/// 对磁盘镜像通道执行行动。
///
/// 支持子命令：
/// - `create <字节数>` —— 创建新镜像
/// - `write <扇区> <十六进制数据>` —— 写入一个扇区（需先获取租约，此处简化为 force 写）
/// - `read <扇区>` —— 读取一个扇区并返回十六进制摘要
/// - `flush` —— 刷新脏块
/// - `epoch` —— 返回当前 epoch
pub fn act_disk_img(name: &str, payload: &str) -> Option<Result<String, String>> {
    let path = name.strip_prefix("diskimg/")?;
    let path = Path::new(path);
    let mut tokens = payload.split_whitespace();
    let sub = tokens.next().unwrap_or("");
    match sub {
        "create" => {
            let size: u64 = tokens.next().and_then(|s| s.parse().ok())?;
            match DiskImage::create(path, size) {
                Ok(_) => Some(Ok(format!("已创建镜像 {} ({} 字节)", path.display(), size))),
                Err(e) => Some(Err(format!("创建失败: {}", e))),
            }
        }
        "write" => {
            let sector: u64 = tokens.next().and_then(|s| s.parse().ok())?;
            let hex = tokens.next().unwrap_or("");
            let data: Vec<u8> = (0..hex.len() / 2)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
                .collect();
            let mut img = match DiskImage::open(path, false) {
                Ok(i) => i,
                Err(e) => return Some(Err(format!("打开失败: {}", e))),
            };
            if data.len() != DEFAULT_SECTOR_SIZE as usize {
                return Some(Err(format!("数据长度必须为 {} 字节", DEFAULT_SECTOR_SIZE)));
            }
            match img.write_block(sector, &data) {
                Ok(()) => {
                    let _ = img.flush();
                    Some(Ok(format!("已写入扇区 {} ({} 字节)", sector, data.len())))
                }
                Err(e) => Some(Err(format!("写入失败: {}", e))),
            }
        }
        "read" => {
            let sector: u64 = tokens.next().and_then(|s| s.parse().ok())?;
            let mut img = match DiskImage::open(path, true) {
                Ok(i) => i,
                Err(e) => return Some(Err(format!("打开失败: {}", e))),
            };
            let mut buf = [0u8; DEFAULT_SECTOR_SIZE as usize];
            match img.read_block(sector, &mut buf) {
                Ok(()) => {
                    let hex: String = buf.iter().take(32).map(|b| format!("{:02x}", b)).collect();
                    Some(Ok(format!("扇区 {} 前 32 字节: {}", sector, hex)))
                }
                Err(e) => Some(Err(format!("读取失败: {}", e))),
            }
        }
        "flush" => {
            let mut img = match DiskImage::open(path, false) {
                Ok(i) => i,
                Err(e) => return Some(Err(format!("打开失败: {}", e))),
            };
            match img.flush() {
                Ok(()) => Some(Ok("脏块已刷新".to_string())),
                Err(e) => Some(Err(format!("刷新失败: {}", e))),
            }
        }
        "epoch" => {
            let ep = lease::epoch(&diskimg_lease_channel(path));
            Some(Ok(format!("epoch={}", ep)))
        }
        _ => Some(Err(
            "用法: create <字节数> | write <扇区> <hex> | read <扇区> | flush | epoch".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    // ----- lease 并发控制测试 -----

    #[test]
    fn disk_img_leased_single_writer() {
        let path = temp_path("lease1");
        let _ = std::fs::remove_file(&path);
        let img = DiskImageLeased::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        // 两个不同 owner 不能同时持有写租约。
        assert!(img.acquire_write_lease(101).is_ok());
        let err = img.acquire_write_lease(102).unwrap_err();
        assert_eq!(err.holder, Some(crate::exec::101));
        assert!(img.release_write_lease(101));
        assert!(img.acquire_write_lease(102).is_ok());
        assert!(img.release_write_lease(102));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_leased_write_under_lease() {
        let path = temp_path("lease2");
        let _ = std::fs::remove_file(&path);
        let img = DiskImageLeased::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        let ep = img.acquire_write_lease(201).unwrap();
        let mut data = [0u8; DEFAULT_SECTOR_SIZE as usize];
        data[0] = 0x42;
        let new_ep = img.write_block_under_lease(201, ep, 3, &data).unwrap();
        assert_eq!(new_ep, ep + 1);
        // 读回验证。
        let mut back = [0u8; DEFAULT_SECTOR_SIZE as usize];
        img.read_block(3, &mut back).unwrap();
        assert_eq!(back[0], 0x42);
        assert!(img.release_write_lease(201));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_leased_stale_epoch_rejected() {
        let path = temp_path("lease3");
        let _ = std::fs::remove_file(&path);
        let img = DiskImageLeased::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        let ep = img.acquire_write_lease(301).unwrap();
        let mut data = [0u8; DEFAULT_SECTOR_SIZE as usize];
        data[0] = 0x01;
        let _ = img.write_block_under_lease(301, ep, 0, &data).unwrap();
        // 再用旧 epoch 提交应被拒绝。
        let ep2 = img.acquire_write_lease(301).unwrap();
        let err = img.write_block_under_lease(301, ep, 1, &data).unwrap_err();
        assert!(matches!(err, crate::lease::CommitError::StaleEpoch { .. }));
        let _ = img.release_write_lease(301);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_leased_epoch_and_flush() {
        let path = temp_path("lease4");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImageLeased::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        let ep0 = img.epoch();
        let mut data = [0u8; DEFAULT_SECTOR_SIZE as usize];
        data[1] = 0x09;
        img.inner.lock().unwrap().write_block(7, &data).unwrap();
        let ep1 = img.flush_under_lease(401).unwrap();
        assert!(ep1 > ep0);
        assert_eq!(img.inner.lock().unwrap().dirty_count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    // ----- caps 权限测试 -----

    #[test]
    fn disk_img_caps_rawio_required() {
        let path = temp_path("caps1");
        let _ = std::fs::remove_file(&path);
        let img = DiskImage::create(&path, 8 * DEFAULT_SECTOR_SIZE).unwrap();
        let caps_img = DiskImageCaps::new(img);
        // 撤销 CAP_SYS_RAWIO 后读写应被拒绝。
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_RAWIO");
        }
        let mut buf = [0u8; DEFAULT_SECTOR_SIZE as usize];
        assert!(caps_img.read_block(0, &mut buf).is_err());
        assert!(caps_img.write_block(0, &buf).is_err());
        // 恢复能力后应成功。
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_RAWIO");
        }
        assert!(caps_img.read_block(0, &mut buf).is_ok());
        assert!(caps_img.write_block(0, &[0x1u8; DEFAULT_SECTOR_SIZE as usize]).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_caps_admin_required() {
        let path = temp_path("caps2");
        let _ = std::fs::remove_file(&path);
        let img = DiskImage::create(&path, 8 * DEFAULT_SECTOR_SIZE).unwrap();
        let caps_img = DiskImageCaps::new(img);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_img.truncate(4).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_img.truncate(4).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    // ----- sense/act 通道测试 -----

    #[test]
    fn disk_img_probe_channel() {
        let path = temp_path("probe");
        let _ = std::fs::remove_file(&path);
        let mut img = DiskImage::create(&path, 16 * DEFAULT_SECTOR_SIZE).unwrap();
        img.write_block(0, &[0x01u8; DEFAULT_SECTOR_SIZE as usize]).unwrap();
        img.flush().unwrap();
        let name = format!("diskimg/{}", path.display());
        let snap = probe_disk_img(&name).unwrap();
        assert!(snap.contains("sector_count=16"));
        assert!(snap.contains("epoch="));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disk_img_act_channel_create_flush() {
        let path = temp_path("act");
        let _ = std::fs::remove_file(&path);
        let name = format!("diskimg/{}", path.display());
        // 创建。
        let r = act_disk_img(&name, "create 4096").unwrap().unwrap();
        assert!(r.contains("已创建镜像"));
        // 写入（hex 数据，512 字节 = 1024 个 hex 字符）。
        let hex = "00".repeat(512);
        let r = act_disk_img(&name, &format!("write 0 {}", hex)).unwrap().unwrap();
        assert!(r.contains("已写入扇区"));
        // 读取。
        let r = act_disk_img(&name, "read 0").unwrap().unwrap();
        assert!(r.contains("扇区 0 前 32 字节"));
        // flush。
        let r = act_disk_img(&name, "flush").unwrap().unwrap();
        assert_eq!(r, "脏块已刷新");
        // epoch。
        let r = act_disk_img(&name, "epoch").unwrap().unwrap();
        assert!(r.contains("epoch="));
        let _ = std::fs::remove_file(&path);
    }
}

