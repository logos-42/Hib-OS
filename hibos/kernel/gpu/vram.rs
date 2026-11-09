//! VRAM (显存) 分区管理。
//!
//! 每个 AI Action Token 拥有独立的 VRAM 分区，通过 IOMMU 域实现硬件级隔离。
//! 支持加密、压缩与共享分区等高级特性。

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// VRAM 分区类型。
#[derive(Clone, Debug, PartialEq)]
pub enum VramPartitionType {
    /// 固定大小分区。
    Fixed { start: u64, size: u64 },
    /// 动态分区。
    Dynamic { min_size: u64, max_size: u64 },
    /// 共享区域 (多个 Token 可访问)。
    Shared { regions: Vec<(u64, u64)> },
}

/// VRAM 分区句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VramPartitionId(pub u64);

/// IOMMU 域 ID —— 代表一个硬件隔离域。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IommuDomainId(pub u64);

/// GPU 物理页框。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuPage {
    /// 物理页号 (Page Frame Number)。
    pub pfn: u64,
    /// 是否锁定 (防止换出)。
    pub locked: bool,
    /// 是否已映射到 IOMMU 域。
    pub mapped: bool,
}

/// VRAM 保护标志 (位集)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VramFlags(u32);

impl VramFlags {
    pub const READ_ONLY: u32 = 1 << 0;
    pub const WRITE_THROUGH: u32 = 1 << 1;
    pub const WRITE_BACK: u32 = 1 << 2;
    pub const CACHED: u32 = 1 << 3;
    pub const ENCRYPTED: u32 = 1 << 4;
    pub const COMPRESSED: u32 = 1 << 5;
    pub const SHARED: u32 = 1 << 6;
    pub const SHARED_READ_ONLY: u32 = 1 << 7;
    pub const SHARED_WRITE: u32 = 1 << 8;
    pub const ENCRYPTED_COMPRESSED: u32 = 1 << 9;
    pub const OWNER_ONLY: u32 = 1 << 10;
    pub const NO_WRITE: u32 = 1 << 11;
    pub const NO_READ: u32 = 1 << 12;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    pub fn insert(&mut self, flag: u32) {
        self.0 |= flag;
    }
}

/// VRAM 分区 (完整对象)。
#[derive(Clone, Debug)]
pub struct VramPartition {
    pub part_type: VramPartitionType,
    pub iommu_domain: IommuDomainId,
    pub size: u64,
    pub usage: u64,
    pub pages: Vec<GpuPage>,
    pub flags: VramFlags,
}

/// 压缩统计。
#[derive(Clone, Debug, Default)]
pub struct CompressionStats {
    pub original_size: u64,
    pub compressed_size: u64,
    pub total_compressions: u64,
    pub total_saved: u64,
}

/// VRAM 错误类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VramError {
    InsufficientVram { requested: u64, available: u64 },
    NotFound,
    PartitionHasActiveMappings,
    OutOfPages,
    AccessViolation,
    EncryptionError,
    CompressionError,
    SharedViolation,
}

impl std::fmt::Display for VramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VramError::InsufficientVram {
                requested,
                available,
            } => write!(
                f,
                "VRAM 不足: 请求 {} 字节, 可用 {} 字节",
                requested, available
            ),
            VramError::NotFound => write!(f, "分区不存在"),
            VramError::PartitionHasActiveMappings => write!(f, "分区仍有活跃映射"),
            VramError::OutOfPages => write!(f, "页框耗尽"),
            VramError::AccessViolation => write!(f, "访问越权"),
            VramError::EncryptionError => write!(f, "加密失败"),
            VramError::CompressionError => write!(f, "压缩失败"),
            VramError::SharedViolation => write!(f, "共享分区冲突"),
        }
    }
}

/// VRAM 管理器。
pub struct VramManager {
    partitions: BTreeMap<VramPartitionId, VramPartition>,
    next_partition_id: u64,
    total_vram: u64,
    reserved: u64,
    encryption_key: Option<[u8; 32]>,
    compression_stats: CompressionStats,
    shared_partitions: BTreeMap<VramPartitionId, HashSet<VramPartitionId>>,
    /// GPU 写租约：分区 ID -> 持有写租约的 exec-id（单写者，与 lease.rs 语义一致）。
    gpu_write_leases: BTreeMap<VramPartitionId, u64>,
    gpu_lease_epochs: BTreeMap<VramPartitionId, u64>,
}

impl VramManager {
    /// 默认显存容量: 16 GiB。
    pub const DEFAULT_TOTAL_VRAM: u64 = 16 * 1024 * 1024 * 1024;

    pub fn new(total_vram: u64) -> Self {
        Self {
            partitions: BTreeMap::new(),
            next_partition_id: 1,
            total_vram,
            reserved: 0,
            encryption_key: None,
            compression_stats: CompressionStats::default(),
            shared_partitions: BTreeMap::new(),
            gpu_write_leases: BTreeMap::new(),
            gpu_lease_epochs: BTreeMap::new(),
        }
    }

    /// 总容量。
    pub fn total_vram(&self) -> u64 {
        self.total_vram
    }

    /// 当前可用容量。
    pub fn available(&self) -> u64 {
        self.total_vram.saturating_sub(self.reserved)
    }

    /// 预留容量 (不做具体映射)。
    pub fn reserve(&mut self, bytes: u64) -> Result<(), VramError> {
        if bytes > self.available() {
            return Err(VramError::InsufficientVram {
                requested: bytes,
                available: self.available(),
            });
        }
        self.reserved += bytes;
        Ok(())
    }

    /// 释放预留容量。
    pub fn release(&mut self, bytes: u64) {
        self.reserved = self.reserved.saturating_sub(bytes);
    }

    /// 设置加密密钥。
    pub fn set_encryption_key(&mut self, key: [u8; 32]) {
        self.encryption_key = Some(key);
    }

    /// 读取加密密钥。
    pub fn get_encryption_key(&self) -> Option<[u8; 32]> {
        self.encryption_key
    }

    /// 分配 VRAM 分区。
    pub fn allocate_partition(
        &mut self,
        part_type: VramPartitionType,
        size: u64,
        flags: VramFlags,
    ) -> Result<VramPartition, VramError> {
        let used: u64 = self.partitions.values().map(|p| p.size).sum();
        let available = self.total_vram.saturating_sub(used);
        if size > available {
            return Err(VramError::InsufficientVram {
                requested: size,
                available,
            });
        }

        let domain = IommuDomainId(self.next_partition_id);
        let id = VramPartitionId(self.next_partition_id);
        self.next_partition_id += 1;

        let partition = VramPartition {
            part_type,
            iommu_domain: domain,
            size,
            usage: 0,
            pages: Vec::new(),
            flags,
        };

        if flags.contains(VramFlags::COMPRESSED) {
            self.compression_stats.total_compressions += 1;
        }

        self.partitions.insert(id, partition.clone());
        Ok(partition)
    }

    /// 释放分区。
    pub fn deallocate_partition(&mut self, id: VramPartitionId) -> Result<(), VramError> {
        let partition = self.partitions.remove(&id).ok_or(VramError::NotFound)?;
        if !partition.pages.is_empty() {
            self.partitions.insert(id, partition);
            return Err(VramError::PartitionHasActiveMappings);
        }
        self.shared_partitions.remove(&id);
        for peers in self.shared_partitions.values_mut() {
            peers.remove(&id);
        }
        Ok(())
    }

    /// 查询分区。
    pub fn get_partition(&self, id: VramPartitionId) -> Option<&VramPartition> {
        self.partitions.get(&id)
    }

    /// 是否还有足够空间。
    pub fn check_space(&self, required: u64) -> bool {
        self.available() >= required
    }

    /// 获取某个分区的 GPU 写租约（单写者）。
    pub fn acquire_gpu_lease(&mut self, partition_id: VramPartitionId, owner: u64) -> Result<u64, VramError> {
        let partition = self.partitions.get(&partition_id).ok_or(VramError::NotFound)?;
        let _ = partition; // 确保分区存在
        if let Some(&current_holder) = self.gpu_write_leases.get(&partition_id) {
            if current_holder == owner {
                // 同一 owner 可重入，返回当前 epoch。
                return Ok(*self.gpu_lease_epochs.get(&partition_id).unwrap_or(&0));
            }
            return Err(VramError::AccessViolation);
        }
        let epoch = self.gpu_lease_epochs.entry(partition_id).or_insert(0);
        *epoch += 1;
        self.gpu_write_leases.insert(partition_id, owner);
        Ok(*epoch)
    }

    /// 释放某个分区的 GPU 写租约。
    pub fn release_gpu_lease(&mut self, partition_id: VramPartitionId, owner: u64) -> bool {
        if let Some(&current_holder) = self.gpu_write_leases.get(&partition_id) {
            if current_holder == owner {
                self.gpu_write_leases.remove(&partition_id);
                return true;
            }
        }
        false
    }

    /// 查询某个分区当前的 GPU 写租约持有者与 epoch。
    pub fn query_gpu_lease(&self, partition_id: VramPartitionId) -> Option<(u64, u64)> {
        self.gpu_write_leases.get(&partition_id).map(|&h| (h, *self.gpu_lease_epochs.get(&partition_id).unwrap_or(&0)))
    }

    /// 向某个分区提交帧数据（要求持有该分区的写租约）。
    pub fn commit_gpu(
        &mut self,
        partition_id: VramPartitionId,
        owner: u64,
        expected_epoch: u64,
        data: &[u8],
    ) -> Result<u64, VramError> {
        let _partition = self.partitions.get(&partition_id).ok_or(VramError::NotFound)?;
        // 租约检查：只有持租者才能提交。
        match self.gpu_write_leases.get(&partition_id) {
            Some(&h) if h == owner => {}
            _ => return Err(VramError::AccessViolation),
        }
        // Epoch 检查（乐观 CAS）。
        let current_epoch = *self.gpu_lease_epochs.get(&partition_id).unwrap_or(&0);
        if current_epoch != expected_epoch {
            return Err(VramError::AccessViolation); // 简化：epoch 不符视为冲突
        }
        let new_epoch = current_epoch + 1;
        self.gpu_lease_epochs.insert(partition_id, new_epoch);
        let _ = self.write_partition(partition_id, data);
        Ok(new_epoch)
    }

    /// 读取分区内容 (零填充占位)。
    pub fn read_partition(&self, id: VramPartitionId, size: usize) -> Result<Vec<u8>, VramError> {
        let partition = self.partitions.get(&id).ok_or(VramError::NotFound)?;
        let len = size.min(partition.size as usize);
        Ok(vec![0u8; len])
    }

    /// 写入分区内容 (带可选加密)。
    pub fn write_partition(&mut self, id: VramPartitionId, data: &[u8]) -> Result<(), VramError> {
        let partition = self.partitions.get_mut(&id).ok_or(VramError::NotFound)?;
        let mut buf = data.to_vec();
        if partition.flags.contains(VramFlags::ENCRYPTED) {
            for (i, byte) in buf.iter_mut().enumerate() {
                if let Some(key) = self.encryption_key {
                    *byte ^= key[i % key.len()];
                }
            }
        }
        partition.usage = buf.len().min(partition.size as usize) as u64;
        Ok(())
    }

    /// 建立共享关系。
    pub fn set_shared(
        &mut self,
        a: VramPartitionId,
        b: VramPartitionId,
    ) -> Result<(), VramError> {
        let pa = self.partitions.get(&a).ok_or(VramError::NotFound)?;
        let pb = self.partitions.get(&b).ok_or(VramError::NotFound)?;
        if !pa.flags.contains(VramFlags::SHARED) || !pb.flags.contains(VramFlags::SHARED) {
            return Err(VramError::SharedViolation);
        }
        self.shared_partitions.entry(a).or_default().insert(b);
        self.shared_partitions.entry(b).or_default().insert(a);
        Ok(())
    }

    /// 查询共享对端。
    pub fn get_shared_partitions(&self, id: VramPartitionId) -> Option<HashSet<VramPartitionId>> {
        self.shared_partitions.get(&id).cloned()
    }

    /// 压缩统计快照。
    pub fn compression_stats(&self) -> &CompressionStats {
        &self.compression_stats
    }

    /// 分区数量。
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }
}

static VRAM_MANAGER: OnceLock<Mutex<VramManager>> = OnceLock::new();

/// 全局 VRAM 管理器。
pub fn vram() -> &'static Mutex<VramManager> {
    VRAM_MANAGER.get_or_init(|| Mutex::new(VramManager::new(VramManager::DEFAULT_TOTAL_VRAM)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(size: u64) -> VramPartitionType {
        VramPartitionType::Fixed { start: 0, size }
    }

    #[test]
    fn allocate_and_deallocate() {
        let mut m = VramManager::new(1024);
        let p = m
            .allocate_partition(fixed(512), 512, VramFlags::empty())
            .unwrap();
        assert_eq!(m.partition_count(), 1);
        let id = VramPartitionId(p.iommu_domain.0);
        assert!(m.get_partition(id).is_some());
        m.deallocate_partition(id).unwrap();
        assert_eq!(m.partition_count(), 0);
    }

    #[test]
    fn insufficient_vram_rejected() {
        let mut m = VramManager::new(100);
        let r = m.allocate_partition(fixed(200), 200, VramFlags::empty());
        assert!(matches!(r, Err(VramError::InsufficientVram { .. })));
    }

    #[test]
    fn reserve_and_release() {
        let mut m = VramManager::new(1000);
        assert_eq!(m.available(), 1000);
        m.reserve(400).unwrap();
        assert_eq!(m.available(), 600);
        assert!(m.reserve(700).is_err());
        m.release(400);
        assert_eq!(m.available(), 1000);
    }

    #[test]
    fn shared_partitions_tracked() {
        let mut m = VramManager::new(4096);
        let mut flags = VramFlags::empty();
        flags.insert(VramFlags::SHARED);
        let a = m.allocate_partition(fixed(1024), 1024, flags).unwrap();
        let b = m.allocate_partition(fixed(1024), 1024, flags).unwrap();
        let ia = VramPartitionId(a.iommu_domain.0);
        let ib = VramPartitionId(b.iommu_domain.0);
        m.set_shared(ia, ib).unwrap();
        assert!(m.get_shared_partitions(ia).unwrap().contains(&ib));
        assert!(m.get_shared_partitions(ib).unwrap().contains(&ia));
    }

    #[test]
    fn encryption_key_and_write() {
        let mut m = VramManager::new(1024);
        m.set_encryption_key([7u8; 32]);
        assert!(m.get_encryption_key().is_some());
        let mut flags = VramFlags::empty();
        flags.insert(VramFlags::ENCRYPTED);
        let p = m.allocate_partition(fixed(64), 64, flags).unwrap();
        let id = VramPartitionId(p.iommu_domain.0);
        m.write_partition(id, &[1, 2, 3, 4]).unwrap();
        assert_eq!(m.get_partition(id).unwrap().usage, 4);
        assert_eq!(m.read_partition(id, 8).unwrap().len(), 8);
    }

    #[test]
    fn shared_flag_required() {
        let mut m = VramManager::new(4096);
        let a = m.allocate_partition(fixed(256), 256, VramFlags::empty()).unwrap();
        let b = m.allocate_partition(fixed(256), 256, VramFlags::empty()).unwrap();
        let r = m.set_shared(
            VramPartitionId(a.iommu_domain.0),
            VramPartitionId(b.iommu_domain.0),
        );
        assert_eq!(r, Err(VramError::SharedViolation));
    }
}

