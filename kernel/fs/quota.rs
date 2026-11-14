//! 磁盘配额（用户/组配额、块和 inode 限制）。
//!
//! 提供 `QuotaManager`：按用户 ID 设置块/inode 上限，检查与扣费
//!（charge/release），支持宽限期与警告阈值。项目配额（目录级）简化版。
//!
//! 与 `ext2_like.rs` / `vfs.rs` 配合，在文件创建/块分配前调用
//! `check_block_quota` / `check_inode_quota` 进行准入控制。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::lease::{self, Epoch};
use crate::caps;

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
    quotas: Mutex<BTreeMap<Uid, QuotaInfo>>,
    /// 项目 ID -> 项目配额。
    projects: Mutex<BTreeMap<ProjId, ProjectQuota>>,
    /// 全局默认块限制（未设置用户时）。
    default_block_limit: u64,
    /// 全局默认 inode 限制。
    default_inode_limit: u64,
}

impl QuotaManager {
    /// 创建配额管理器。
    pub fn new() -> Self {
        Self {
            quotas: Mutex::new(BTreeMap::new()),
            projects: Mutex::new(BTreeMap::new()),
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

// ============================================================================
// 并发控制：基于 lease.rs 的单写者租约 + epoch CAS
// ============================================================================

/// 基于 lease 的配额管理器包装器：对配额设置/扣费/释放加单写者租约，
/// 避免多线程并发修改同一用户配额时发生竞争。
pub struct QuotaManagerLeased {
    inner: QuotaManager,
    channel: String,
}

impl QuotaManagerLeased {
    /// 创建租约保护的配额管理器。
    pub fn new(channel: &str) -> Self {
        Self {
            inner: QuotaManager::new(),
            channel: channel.to_string(),
        }
    }

    /// 申请配额修改租约，返回当前 epoch。
    pub fn acquire_quota_lease(&self, owner: u64) -> Result<Epoch, crate::lease::LeaseConflict> {
        lease::acquire(&self.channel, owner, None)
    }

    /// 释放配额修改租约。
    pub fn release_quota_lease(&self, owner: u64) -> bool {
        lease::release(&self.channel, owner)
    }

    /// 查询用户配额（只读，无需租约）。
    pub fn get_quota(&self, uid: Uid) -> Result<QuotaInfo, QuotaError> {
        self.inner.get_quota(uid)
    }

    /// 在租约保护下设置用户配额，然后 commit 推进 epoch。
    pub fn set_quota_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        uid: Uid,
        block_limit: u64,
        inode_limit: u64,
    ) -> Result<Epoch, QuotaError> {
        let _ = self.acquire_quota_lease(owner)
            .map_err(|c| QuotaError::Invalid)?;
        self.inner.set_quota(uid, block_limit, inode_limit)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("quota set uid={} blocks={} inodes={}", uid, block_limit, inode_limit),
        )
        .map_err(|e| QuotaError::Invalid)?;
        Ok(new_ep)
    }

    /// 在租约保护下扣费，然后 commit 推进 epoch。
    pub fn charge_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        uid: Uid,
        blocks: u64,
        inodes: u64,
    ) -> Result<Epoch, QuotaError> {
        let _ = self.acquire_quota_lease(owner)
            .map_err(|c| QuotaError::Invalid)?;
        self.inner.charge(uid, blocks, inodes)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("quota charge uid={} blocks={} inodes={}", uid, blocks, inodes),
        )
        .map_err(|e| QuotaError::Invalid)?;
        Ok(new_ep)
    }

    /// 在租约保护下释放配额，然后 commit 推进 epoch。
    pub fn release_under_lease(
        &self,
        owner: u64,
        expected_epoch: Epoch,
        uid: Uid,
        blocks: u64,
        inodes: u64,
    ) -> Result<Epoch, QuotaError> {
        let _ = self.acquire_quota_lease(owner)
            .map_err(|c| QuotaError::Invalid)?;
        self.inner.release(uid, blocks, inodes)?;
        let new_ep = lease::commit(
            &self.channel,
            owner,
            expected_epoch,
            &format!("quota release uid={} blocks={} inodes={}", uid, blocks, inodes),
        )
        .map_err(|e| QuotaError::Invalid)?;
        Ok(new_ep)
    }

    /// 返回用户数。
    pub fn user_count(&self) -> usize {
        self.inner.user_count()
    }

    /// 返回项目数。
    pub fn project_count(&self) -> usize {
        self.inner.project_count()
    }

    /// 强制刷新（force 模式，绕过租约）。
    pub fn force_set_default(&self, owner: u64, block_limit: u64, inode_limit: u64) -> Epoch {
        lease::force(
            &self.channel,
            &format!("quota force_default blocks={} inodes={} owner={}", block_limit, inode_limit, owner),
        )
    }

    /// 返回当前 epoch。
    pub fn epoch(&self) -> Epoch {
        lease::epoch(&self.channel)
    }
}

/// 能力位权限包装器：在配额操作上加 `caps` 校验。
///
/// `CAP_SYS_ADMIN` 允许设置/修改配额；查询无需能力。
pub struct QuotaManagerCaps {
    inner: QuotaManager,
    caps: Arc<Mutex<caps::Caps>>,
}

impl QuotaManagerCaps {
    /// 包装一个已有配额管理器，绑定全局能力表。
    pub fn new(mgr: QuotaManager) -> Self {
        Self {
            inner: mgr,
            caps: Arc::new(Mutex::new(caps::caps().lock().unwrap().clone())),
        }
    }

    fn check_admin(&self) -> Result<(), String> {
        let c = self.caps.lock().unwrap();
        if c.has("CAP_SYS_ADMIN") {
            Ok(())
        } else {
            Err("缺少 CAP_SYS_ADMIN 能力".to_string())
        }
    }

    /// 设置用户配额（需 CAP_SYS_ADMIN）。
    pub fn set_quota(
        &self,
        uid: Uid,
        block_limit: u64,
        inode_limit: u64,
    ) -> Result<(), String> {
        self.check_admin()?;
        self.inner.set_quota(uid, block_limit, inode_limit).map_err(|e| e.to_string())
    }

    /// 扣费（需 CAP_SYS_ADMIN）。
    pub fn charge(
        &self,
        uid: Uid,
        blocks: u64,
        inodes: u64,
    ) -> Result<(), String> {
        self.check_admin()?;
        self.inner.charge(uid, blocks, inodes).map_err(|e| e.to_string())
    }

    /// 释放（需 CAP_SYS_ADMIN）。
    pub fn release(
        &self,
        uid: Uid,
        blocks: u64,
        inodes: u64,
    ) -> Result<(), String> {
        self.check_admin()?;
        self.inner.release(uid, blocks, inodes).map_err(|e| e.to_string())
    }

    /// 查询配额（只读，无需能力）。
    pub fn get_quota(&self, uid: Uid) -> Result<QuotaInfo, String> {
        self.inner.get_quota(uid).map_err(|e| e.to_string())
    }

    /// 返回用户数（无需能力）。
    pub fn user_count(&self) -> usize {
        self.inner.user_count()
    }
}

// ============================================================================
// sense/act 通道接口：quota/<设备名>
// ============================================================================

/// 感知磁盘配额通道：返回配额管理器状态摘要。
///
/// 通道名形式：`quota/<设备名>`
pub fn probe_quota(name: &str) -> Option<String> {
    let channel = format!("quota/{}", name);
    let epoch = lease::epoch(&channel);
    Some(format!(
        "channel={} type=quota_manager epoch={} ops=set_quota|charge|release|get_quota|warn_users",
        name, epoch
    ))
}

/// 对磁盘配额通道执行行动。
///
/// 支持子命令：
/// - `status` —— 返回配额状态摘要（占位）
/// - `epoch` —— 返回当前 epoch
pub fn act_quota(name: &str, payload: &str) -> Option<Result<String, String>> {
    let mut tokens = payload.split_whitespace();
    let sub = tokens.next().unwrap_or("");
    let channel = format!("quota/{}", name);
    match sub {
        "status" => Some(Ok(format!(
            "quota/{} status=active epoch={}",
            name,
            lease::epoch(&channel)
        ))),
        "epoch" => Some(Ok(format!("epoch={}", lease::epoch(&channel)))),
        _ => Some(Err("用法: status | epoch".to_string())),
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

    // ----- lease 并发控制测试 -----

    #[test]
    fn quota_leased_single_writer() {
        let mgr = QuotaManagerLeased::new("quota/test-lease");
        assert!(mgr.acquire_quota_lease(101).is_ok());
        let err = mgr.acquire_quota_lease(102).unwrap_err();
        assert_eq!(err.holder, crate::exec::101);
        assert!(mgr.release_quota_lease(101));
        assert!(mgr.acquire_quota_lease(102).is_ok());
        assert!(mgr.release_quota_lease(102));
    }

    #[test]
    fn quota_leased_set_charge_commit_epoch() {
        let mgr = QuotaManagerLeased::new("quota/test-lease2");
        let ep0 = mgr.epoch();
        let ep1 = mgr.acquire_quota_lease(201).unwrap();
        assert_eq!(ep1, ep0);
        let new_ep = mgr.set_quota_under_lease(201, ep0, 1000, 10000, 500).unwrap();
        assert_eq!(new_ep, ep0 + 1);
        let new_ep2 = mgr.charge_under_lease(201, new_ep, 1000, 100, 10).unwrap();
        assert_eq!(new_ep2, new_ep + 1);
        let info = mgr.get_quota(1000).unwrap();
        assert_eq!(info.used_blocks, 100);
        assert!(mgr.release_quota_lease(201));
    }

    #[test]
    fn quota_leased_stale_epoch_rejected() {
        let mgr = QuotaManagerLeased::new("quota/test-lease3");
        let ep = mgr.acquire_quota_lease(301).unwrap();
        let _ = mgr.set_quota_under_lease(301, ep, 2000, 5000, 100).unwrap();
        // 再用旧 epoch 设置应被拒绝。
        let ep2 = mgr.acquire_quota_lease(301).unwrap();
        let err = mgr.set_quota_under_lease(301, ep, 3000, 6000, 200).unwrap_err();
        assert!(matches!(err, QuotaError::Invalid));
        let _ = mgr.release_quota_lease(301);
    }

    #[test]
    fn quota_leased_force_set_default() {
        let mgr = QuotaManagerLeased::new("quota/test-lease4");
        let ep0 = mgr.epoch();
        let ep1 = mgr.force_set_default(401, 100000, 1000);
        assert!(ep1 > ep0);
    }

    // ----- caps 权限测试 -----

    #[test]
    fn quota_caps_admin_required() {
        let mgr = QuotaManager::new();
        let caps_mgr = QuotaManagerCaps::new(mgr);
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.revoke("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.set_quota(1000, 1000, 100).is_err());
        {
            let mut c = caps::caps().lock().unwrap();
            let _ = c.grant("CAP_SYS_ADMIN");
        }
        assert!(caps_mgr.set_quota(1000, 1000, 100).is_ok());
    }

    #[test]
    fn quota_caps_get_quota_no_cap() {
        let mgr = QuotaManager::new();
        mgr.set_quota(1000, 1000, 100).unwrap();
        let caps_mgr = QuotaManagerCaps::new(mgr);
        // 查询不需要能力。
        assert!(caps_mgr.get_quota(1000).is_ok());
    }

    // ----- sense/act 通道测试 -----

    #[test]
    fn quota_probe_channel() {
        let snap = probe_quota("quota/ram0").unwrap();
        assert!(snap.contains("quota/ram0"));
        assert!(snap.contains("epoch="));
        assert!(snap.contains("ops="));
    }

    #[test]
    fn quota_act_channel_status_epoch() {
        let r = act_quota("quota/ram0", "status").unwrap().unwrap();
        assert!(r.contains("status=active"));
        let r = act_quota("quota/ram0", "epoch").unwrap().unwrap();
        assert!(r.contains("epoch="));
    }
}


