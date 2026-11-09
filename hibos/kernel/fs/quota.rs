//! 磁盘配额（用户/组配额、块和 inode 限制）。
//!
//! 提供 `QuotaManager`：按用户 ID 设置块/inode 上限，检查与扣费
//!（charge/release），支持宽限期与警告阈值。项目配额（目录级）简化版。
//!
//! 与 `ext2_like.rs` / `vfs.rs` 配合，在文件创建/块分配前调用
//! `check_block_quota` / `check_inode_quota` 进行准入控制。

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 用户/组 ID。
pub type Uid = u64;
/// 项目 ID（目录级配额）。
pub type ProjId = u64;

/// 配额信息。
#[derive(Clone, Debug)]
pub struct QuotaInfo {
    pub uid: Uid,
    /// 已用块数（512 字节单位）。
    pub used_blocks: u64,
    /// 已用 inode 数。
    pub used_inodes: u64,
    /// 块限制（0 表示不限制）。
    pub block_limit: u64,
    /// inode 限制（0 表示不限制）。
    pub inode_limit: u64,
    /// 块宽限期（秒）。
    pub block_grace: u64,
    /// inode 宽限期（秒）。
    pub inode_grace: u64,
    /// 块警告阈值（百分比，0-100）。
    pub block_warn_pct: u8,
    /// inode 警告阈值（百分比，0-100）。
    pub inode_warn_pct: u8,
}

impl QuotaInfo {
    pub fn new(uid: Uid) -> Self {
        Self {
            uid,
            used_blocks: 0,
            used_inodes: 0,
            block_limit: 0,
            inode_limit: 0,
            block_grace: 0,
            inode_grace: 0,
            block_warn_pct: 80,
            inode_warn_pct: 80,
        }
    }

    /// 块使用率（0-100）。
    pub fn block_pct(&self) -> u8 {
        if self.block_limit == 0 {
            0
        } else {
            (self.used_blocks * 100 / self.block_limit).min(100) as u8
        }
    }

    /// inode 使用率（0-100）。
    pub fn inode_pct(&self) -> u8 {
        if self.inode_limit == 0 {
            0
        } else {
            (self.used_inodes * 100 / self.inode_limit).min(100) as u8
        }
    }

    /// 是否超过块警告阈值。
    pub fn block_warn(&self) -> bool {
        self.block_limit > 0 && self.block_pct() >= self.block_warn_pct
    }

    /// 是否超过 inode 警告阈值。
    pub fn inode_warn(&self) -> bool {
        self.inode_limit > 0 && self.inode_pct() >= self.inode_warn_pct
    }

    /// 块是否超过限制。
    pub fn block_over_limit(&self) -> bool {
        self.block_limit > 0 && self.used_blocks >= self.block_limit
    }

    /// inode 是否超过限制。
    pub fn inode_over_limit(&self) -> bool {
        self.inode_limit > 0 && self.used_inodes >= self.inode_limit
    }
}

/// 项目配额（目录级）。
#[derive(Clone, Debug)]
pub struct ProjectQuota {
    pub proj_id: ProjId,
    pub used_blocks: u64,
    pub used_inodes: u64,
    pub block_limit: u64,
    pub inode_limit: u64,
}

impl ProjectQuota {
    pub fn new(proj_id: ProjId) -> Self {
        Self {
            proj_id,
            used_blocks: 0,
            used_inodes: 0,
            block_limit: 0,
            inode_limit: 0,
        }
    }
}

/// 配额错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuotaError {
    /// 用户不存在。
    NotFound,
    /// 块超出限制。
    BlockOverLimit,
    /// inode 超出限制。
    InodeOverLimit,
    /// 参数无效。
    Invalid,
    /// 项目不存在。
    ProjectNotFound,
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaError::NotFound => write!(f, "用户配额不存在"),
            QuotaError::BlockOverLimit => write!(f, "块超出配额限制"),
            QuotaError::InodeOverLimit => write!(f, "inode 超出配额限制"),
            QuotaError::Invalid => write!(f, "无效配额参数"),
            QuotaError::ProjectNotFound => write!(f, "项目配额不存在"),
        }
    }
}

/// 配额管理器：维护用户配额表与项目配额表，提供检查与扣费。
pub struct QuotaManager {
    /// 用户 ID -> 配额。
    quotas: Mutex<HashMap<Uid, QuotaInfo>>,
    /// 项目 ID -> 项目配额。
    projects: Mutex<HashMap<ProjId, ProjectQuota>>,
    /// 全局默认块限制（未设置用户时）。
    default_block_limit: u64,
    /// 全局默认 inode 限制。
    default_inode_limit: u64,
}

impl QuotaManager {
    /// 创建配额管理器。
    pub fn new() -> Self {
        Self {
            quotas: Mutex::new(HashMap::new()),
            projects: Mutex::new(HashMap::new()),
            default_block_limit: 0,
            default_inode_limit: 0,
        }
    }

    /// 设置用户配额。
    pub fn set_quota(
        &self,
        uid: Uid,
        block_limit: u64,
        inode_limit: u64,
    ) -> Result<(), QuotaError> {
        if block_limit == 0 && inode_limit == 0 {
            return Err(QuotaError::Invalid);
        }
        let mut quotas = self.quotas.lock().unwrap();
        let info = quotas.entry(uid).or_insert_with(|| QuotaInfo::new(uid));
        info.block_limit = block_limit;
        info.inode_limit = inode_limit;
        Ok(())
    }

    /// 获取用户配额信息。
    pub fn get_quota(&self, uid: Uid) -> Result<QuotaInfo, QuotaError> {
        let quotas = self.quotas.lock().unwrap();
        quotas.get(&uid).cloned().ok_or(QuotaError::NotFound)
    }

    /// 检查块配额是否足够（不扣费）。
    pub fn check_block_quota(&self, uid: Uid, blocks: u64) -> Result<(), QuotaError> {
        if blocks == 0 {
            return Ok(());
        }
        let quotas = self.quotas.lock().unwrap();
        let info = quotas.get(&uid).ok_or(QuotaError::NotFound)?;
        if info.block_limit > 0 && info.used_blocks + blocks > info.block_limit {
            return Err(QuotaError::BlockOverLimit);
        }
        Ok(())
    }

    /// 检查 inode 配额是否足够（不扣费）。
    pub fn check_inode_quota(&self, uid: Uid, count: u64) -> Result<(), QuotaError> {
        if count == 0 {
            return Ok(());
        }
        let quotas = self.quotas.lock().unwrap();
        let info = quotas.get(&uid).ok_or(QuotaError::NotFound)?;
        if info.inode_limit > 0 && info.used_inodes + count > info.inode_limit {
            return Err(QuotaError::InodeOverLimit);
        }
        Ok(())
    }

    /// 扣费：占用块与 inode。若超限返回错误且不修改状态。
    pub fn charge(
        &self,
        uid: Uid,
        blocks: u64,
        inodes: u64,
    ) -> Result<(), QuotaError> {
        self.check_block_quota(uid, blocks)?;
        self.check_inode_quota(uid, inodes)?;
        let mut quotas = self.quotas.lock().unwrap();
        let info = quotas.get_mut(&uid).ok_or(QuotaError::NotFound)?;
        info.used_blocks += blocks;
        info.used_inodes += inodes;
        Ok(())
    }

    /// 释放：归还块与 inode。
    pub fn release(&self, uid: Uid, blocks: u64, inodes: u64) -> Result<(), QuotaError> {
        let mut quotas = self.quotas.lock().unwrap();
        let info = quotas.get_mut(&uid).ok_or(QuotaError::NotFound)?;
        info.used_blocks = info.used_blocks.saturating_sub(blocks);
        info.used_inodes = info.used_inodes.saturating_sub(inodes);
        Ok(())
    }

    /// 设置项目配额（目录级）。
    pub fn set_project_quota(
        &self,
        proj_id: ProjId,
        block_limit: u64,
        inode_limit: u64,
    ) -> Result<(), QuotaError> {
        if block_limit == 0 && inode_limit == 0 {
            return Err(QuotaError::Invalid);
        }
        let mut projects = self.projects.lock().unwrap();
        let pq = projects.entry(proj_id).or_insert_with(|| ProjectQuota::new(proj_id));
        pq.block_limit = block_limit;
        pq.inode_limit = inode_limit;
        Ok(())
    }

    /// 获取项目配额。
    pub fn get_project_quota(&self, proj_id: ProjId) -> Result<ProjectQuota, QuotaError> {
        let projects = self.projects.lock().unwrap();
        projects.get(&proj_id).cloned().ok_or(QuotaError::ProjectNotFound)
    }

    /// 检查项目块配额。
    pub fn check_project_block_quota(&self, proj_id: ProjId, blocks: u64) -> Result<(), QuotaError> {
        if blocks == 0 {
            return Ok(());
        }
        let projects = self.projects.lock().unwrap();
        let pq = projects.get(&proj_id).ok_or(QuotaError::ProjectNotFound)?;
        if pq.block_limit > 0 && pq.used_blocks + blocks > pq.block_limit {
            return Err(QuotaError::BlockOverLimit);
        }
        Ok(())
    }

    /// 扣费项目配额。
    pub fn charge_project(
        &self,
        proj_id: ProjId,
        blocks: u64,
        inodes: u64,
    ) -> Result<(), QuotaError> {
        self.check_project_block_quota(proj_id, blocks)?;
        {
            let projects = self.projects.lock().unwrap();
            let pq = projects.get(&proj_id).ok_or(QuotaError::ProjectNotFound)?;
            if pq.inode_limit > 0 && pq.used_inodes + inodes > pq.inode_limit {
                return Err(QuotaError::InodeOverLimit);
            }
        }
        let mut projects = self.projects.lock().unwrap();
        let pq = projects.get_mut(&proj_id).ok_or(QuotaError::ProjectNotFound)?;
        pq.used_blocks += blocks;
        pq.used_inodes += inodes;
        Ok(())
    }

    /// 释放项目配额。
    pub fn release_project(&self, proj_id: ProjId, blocks: u64, inodes: u64) -> Result<(), QuotaError> {
        let mut projects = self.projects.lock().unwrap();
        let pq = projects.get_mut(&proj_id).ok_or(QuotaError::ProjectNotFound)?;
        pq.used_blocks = pq.used_blocks.saturating_sub(blocks);
        pq.used_inodes = pq.used_inodes.saturating_sub(inodes);
        Ok(())
    }

    /// 设置全局默认限制（用户未配置时作为上限）。
    pub fn set_default_limit(&mut self, block_limit: u64, inode_limit: u64) {
        self.default_block_limit = block_limit;
        self.default_inode_limit = inode_limit;
    }

    /// 获取用户数。
    pub fn user_count(&self) -> usize {
        self.quotas.lock().unwrap().len()
    }

    /// 获取项目数。
    pub fn project_count(&self) -> usize {
        self.projects.lock().unwrap().len()
    }

    /// 列出超过警告阈值的用户。
    pub fn warn_users(&self) -> Vec<Uid> {
        let quotas = self.quotas.lock().unwrap();
        quotas
            .iter()
            .filter(|(_, q)| q.block_warn() || q.inode_warn())
            .map(|(uid, _)| *uid)
            .collect()
    }

    /// 返回当前时间（秒，供宽限期计算）。
    pub fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// 设置用户宽限期。
    pub fn set_grace(&self, uid: Uid, block_grace: u64, inode_grace: u64) -> Result<(), QuotaError> {
        let mut quotas = self.quotas.lock().unwrap();
        let info = quotas.get_mut(&uid).ok_or(QuotaError::NotFound)?;
        info.block_grace = block_grace;
        info.inode_grace = inode_grace;
        Ok(())
    }

    /// 设置警告阈值（百分比）。
    pub fn set_warn_pct(&self, uid: Uid, block_pct: u8, inode_pct: u8) -> Result<(), QuotaError> {
        let mut quotas = self.quotas.lock().unwrap();
        let info = quotas.get_mut(&uid).ok_or(QuotaError::NotFound)?;
        info.block_warn_pct = block_pct;
        info.inode_warn_pct = inode_pct;
        Ok(())
    }
}

impl Default for QuotaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_set_and_get() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 10000, 500).unwrap();
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.uid, 1000);
        assert_eq!(info.block_limit, 10000);
        assert_eq!(info.inode_limit, 500);
        assert_eq!(info.used_blocks, 0);
    }

    #[test]
    fn quota_check_block_ok_and_over() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 100, 500).unwrap();
        assert!(mgr.check_block_quota(1000, 50).is_ok());
        assert_eq!(
            mgr.check_block_quota(1000, 60).unwrap_err(),
            QuotaError::BlockOverLimit
        );
        // 0 块总是通过。
        assert!(mgr.check_block_quota(1000, 0).is_ok());
    }

    #[test]
    fn quota_check_inode_over() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 10000, 10).unwrap();
        assert!(mgr.check_inode_quota(1000, 5).is_ok());
        assert_eq!(
            mgr.check_inode_quota(1000, 6).unwrap_err(),
            QuotaError::InodeOverLimit
        );
    }

    #[test]
    fn quota_charge_release() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 100).unwrap();
        assert!(mgr.charge(1000, 100, 10).is_ok());
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.used_blocks, 100);
        assert_eq!(info.used_inodes, 10);
        assert!(mgr.release(1000, 50, 5).is_ok());
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.used_blocks, 50);
        assert_eq!(info.used_inodes, 5);
    }

    #[test]
    fn quota_charge_over_limit_rejects_no_change() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 100, 100).unwrap();
        assert!(mgr.charge(1000, 60, 10).is_ok());
        assert_eq!(mgr.charge(1000, 50, 50).unwrap_err(), QuotaError::BlockOverLimit);
        let info = mgr.get_quota(1000).unwrap();
        // 第一个 charge 成功，第二个失败，used_blocks 仍为 60。
        assert_eq!(info.used_blocks, 60);
    }

    #[test]
    fn quota_release_saturates_at_zero() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 100).unwrap();
        assert!(mgr.charge(1000, 10, 5).is_ok());
        assert!(mgr.release(1000, 100, 100).is_ok());
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.used_blocks, 0);
        assert_eq!(info.used_inodes, 0);
    }

    #[test]
    fn quota_not_found() {
        let mgr = QuotaManager::new();
        assert_eq!(
            mgr.get_quota(9999).unwrap_err(),
            QuotaError::NotFound
        );
        assert_eq!(
            mgr.check_block_quota(9999, 1).unwrap_err(),
            QuotaError::NotFound
        );
    }

    #[test]
    fn quota_project() {
        let mgr = QuotaManager::new();
        mgr.set_project_quota(1, 5000, 100).unwrap();
        let pq = mgr.get_project_quota(1).unwrap();
        assert_eq!(pq.proj_id, 1);
        assert_eq!(pq.block_limit, 5000);
        assert!(mgr.charge_project(1, 100, 5).is_ok());
        let pq = mgr.get_project_quota(1).unwrap();
        assert_eq!(pq.used_blocks, 100);
        assert_eq!(pq.used_inodes, 5);
        assert_eq!(
            mgr.charge_project(1, 5000, 1).unwrap_err(),
            QuotaError::BlockOverLimit
        );
    }

    #[test]
    fn quota_warn_pct() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 1000).unwrap();
        mgr.charge(1000, 850, 0).unwrap();
        assert!(mgr.get_quota(1000).unwrap().block_warn());
        assert!(mgr.warn_users().contains(&1000));
    }

    #[test]
    fn quota_invalid_limit() {
        let mgr = QuotaManager::new();
        assert_eq!(
            mgr.set_quota(1000, 0, 0).unwrap_err(),
            QuotaError::Invalid
        );
    }

    #[test]
    fn quota_grace_and_warn_pct() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 1000).unwrap();
        mgr.set_grace(1000, 86400, 0).unwrap();
        mgr.set_warn_pct(1000, 50, 50).unwrap();
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.block_grace, 86400);
        assert_eq!(info.block_warn_pct, 50);
    }

    #[test]
    fn quota_user_and_project_count() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1, 100, 10).unwrap();
        mgr.set_quota(2, 100, 10).unwrap();
        assert_eq!(mgr.user_count(), 2);
        mgr.set_project_quota(1, 100, 10).unwrap();
        assert_eq!(mgr.project_count(), 1);
    }

    #[test]
    fn quota_block_pct() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 1000).unwrap();
        mgr.charge(1000, 250, 0).unwrap();
        assert_eq!(mgr.get_quota(1000).unwrap().block_pct(), 25);
    }
}
