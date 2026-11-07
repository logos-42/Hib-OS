//! IOMMU 集成 —— GPU 设备的 DMA 映射与硬件隔离。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::ai_token::TokenId;
use crate::gpu::vram::IommuDomainId;

/// IOMMU 访问标志 (位集)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IommuFlags(u32);

impl IommuFlags {
    pub const READ: u32 = 1 << 0;
    pub const WRITE: u32 = 1 << 1;
    pub const READ_WRITE: u32 = 1 << 2;
    pub const WRITE_BACK: u32 = 1 << 3;
    pub const NON_CACHED: u32 = 1 << 4;
    pub const ENCRYPTED: u32 = 1 << 5;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// IOMMU 映射条目。
#[derive(Clone, Debug)]
pub struct IommuEntry {
    pub gpu_va: u64,
    pub pfn: u64,
    pub size: u64,
    pub owner_token: TokenId,
    pub flags: IommuFlags,
    pub valid: bool,
}

/// IOMMU 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IommuError {
    TokenNotFound,
    InvalidRange,
    AccessViolation,
}

impl std::fmt::Display for IommuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IommuError::TokenNotFound => write!(f, "IOMMU: Token 不存在"),
            IommuError::InvalidRange => write!(f, "IOMMU: 非法地址范围"),
            IommuError::AccessViolation => write!(f, "IOMMU: 访问越权"),
        }
    }
}

/// IOMMU 管理器。
pub struct IommuManager {
    token_domains: BTreeMap<TokenId, IommuDomainId>,
    token_mappings: BTreeMap<TokenId, Vec<IommuEntry>>,
    next_domain_id: u64,
}

impl IommuManager {
    pub fn new() -> Self {
        Self {
            token_domains: BTreeMap::new(),
            token_mappings: BTreeMap::new(),
            next_domain_id: 1,
        }
    }

    /// 为 Token 分配 IOMMU 域。
    pub fn allocate_domain(&mut self, token_id: TokenId) -> IommuDomainId {
        let domain = IommuDomainId(self.next_domain_id);
        self.next_domain_id += 1;
        self.token_domains.insert(token_id, domain);
        domain
    }

    /// 查询 Token 的域。
    pub fn get_domain(&self, token_id: TokenId) -> Option<IommuDomainId> {
        self.token_domains.get(&token_id).copied()
    }

    /// 创建 DMA 映射。
    pub fn create_mapping(
        &mut self,
        token_id: TokenId,
        gpu_va: u64,
        pfn: u64,
        size: u64,
        flags: IommuFlags,
    ) -> Result<IommuEntry, IommuError> {
        if !self.token_domains.contains_key(&token_id) {
            return Err(IommuError::TokenNotFound);
        }
        if size == 0 {
            return Err(IommuError::InvalidRange);
        }
        let entry = IommuEntry {
            gpu_va,
            pfn,
            size,
            owner_token: token_id,
            flags,
            valid: true,
        };
        self.token_mappings
            .entry(token_id)
            .or_default()
            .push(entry.clone());
        Ok(entry)
    }

    /// 撤销 Token 的全部映射。
    pub fn revoke_mappings(&mut self, token_id: TokenId) -> Vec<IommuEntry> {
        self.token_mappings.remove(&token_id).unwrap_or_default()
    }

    /// 查询 Token 的全部映射。
    pub fn get_mappings(&self, token_id: TokenId) -> Option<&Vec<IommuEntry>> {
        self.token_mappings.get(&token_id)
    }

    /// 校验地址是否属于 Token 的域。
    pub fn validate_address(&self, token_id: TokenId, gpu_va: u64) -> bool {
        match self.token_domains.get(&token_id) {
            Some(_) => self
                .token_mappings
                .get(&token_id)
                .map(|entries| {
                    entries
                        .iter()
                        .any(|e| e.valid && gpu_va >= e.gpu_va && gpu_va < e.gpu_va + e.size)
                })
                .unwrap_or(false),
            None => false,
        }
    }
}

impl Default for IommuManager {
    fn default() -> Self {
        Self::new()
    }
}

impl IommuManager {
    /// 已分配的域数量。
    pub fn domain_count(&self) -> usize {
        self.token_domains.len()
    }

    /// 已建立的映射总数。
    pub fn mapping_count(&self) -> usize {
        self.token_mappings.values().map(|v| v.len()).sum()
    }
}

/// 全局 IOMMU 管理器 (感知通道 `dev://iommu0` 的实体)。
pub fn iommu() -> &'static Mutex<IommuManager> {
    static IOMMU: OnceLock<Mutex<IommuManager>> = OnceLock::new();
    IOMMU.get_or_init(|| Mutex::new(IommuManager::new()))
}

/// IOMMU 硬件配置快照。
pub struct IommuHardwareConfig {
    pub enabled: bool,
    pub active_domains: u64,
    pub total_domains: u64,
    pub total_mappings: u64,
}

impl IommuHardwareConfig {
    pub fn new() -> Self {
        Self {
            enabled: true,
            active_domains: 0,
            total_domains: 4096,
            total_mappings: 0,
        }
    }

    pub fn check_address_in_domain(&self, _domain: IommuDomainId, _gpu_va: u64) -> bool {
        self.enabled
    }
}

impl Default for IommuHardwareConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_and_mapping() {
        let mut m = IommuManager::new();
        let t = TokenId::new();
        let d = m.allocate_domain(t);
        assert_eq!(m.get_domain(t), Some(d));

        let entry = m
            .create_mapping(t, 0x1000, 1, 0x1000, IommuFlags::from_bits(IommuFlags::READ))
            .unwrap();
        assert!(entry.valid);
        assert!(m.validate_address(t, 0x1800));
        assert!(!m.validate_address(t, 0x5000));
        assert_eq!(m.revoke_mappings(t).len(), 1);
        assert!(!m.validate_address(t, 0x1000));
    }

    #[test]
    fn mapping_requires_domain() {
        let mut m = IommuManager::new();
        let t = TokenId::new();
        let r = m.create_mapping(t, 0, 0, 16, IommuFlags::empty());
        assert_eq!(r.unwrap_err(), IommuError::TokenNotFound);
    }

    #[test]
    fn zero_size_rejected() {
        let mut m = IommuManager::new();
        let t = TokenId::new();
        m.allocate_domain(t);
        let r = m.create_mapping(t, 0, 0, 0, IommuFlags::empty());
        assert_eq!(r.unwrap_err(), IommuError::InvalidRange);
    }
}

