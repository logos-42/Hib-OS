//! 形式化验证辅助工具 (轻量版)。

/// 验证结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationResult {
    Pass,
    Fail { message: String },
    Inconclusive,
}

impl VerificationResult {
    pub fn is_pass(&self) -> bool {
        matches!(self, VerificationResult::Pass)
    }
}

/// 验证引擎。
pub struct VerificationEngine;

impl VerificationEngine {
    /// 校验能力闭包: 请求的能力必须被持有能力覆盖。
    pub fn verify_capability_closure(
        token_caps: crate::gpu::GpuCapabilities,
        requested_caps: crate::gpu::GpuCapabilities,
    ) -> VerificationResult {
        if token_caps.covers(requested_caps) {
            VerificationResult::Pass
        } else {
            VerificationResult::Fail {
                message: "能力闭包不成立: 请求能力超出持有范围".to_string(),
            }
        }
    }

    /// 校验 IOMMU 隔离: 每个 Token 必须拥有唯一域。
    pub fn verify_iommu_isolation(
        domains: &std::collections::BTreeMap<crate::ai_token::TokenId, u64>,
    ) -> VerificationResult {
        let mut seen = std::collections::HashSet::new();
        for (_, d) in domains {
            if !seen.insert(*d) {
                return VerificationResult::Fail {
                    message: "IOMMU 域重复, 隔离被破坏".to_string(),
                };
            }
        }
        VerificationResult::Pass
    }
}

/// 初始化验证子系统 (极简启动，不在终端打印)。
pub fn init() {
    // 形式化验证框架已就绪；启动阶段保持静默。
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_token::TokenId;
    use crate::gpu::{GpuCapability, GpuCapabilities};

    #[test]
    fn capability_closure() {
        let have = GpuCapabilities::from_caps(&[
            GpuCapability::Compute,
            GpuCapability::Tensor,
        ]);
        let need = GpuCapabilities::from_caps(&[GpuCapability::Compute]);
        assert!(VerificationEngine::verify_capability_closure(have, need).is_pass());
        assert!(!VerificationEngine::verify_capability_closure(need, have).is_pass());
    }

    #[test]
    fn iommu_isolation() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(TokenId(1), 1u64);
        map.insert(TokenId(2), 2u64);
        assert!(VerificationEngine::verify_iommu_isolation(&map).is_pass());

        map.insert(TokenId(3), 1u64);
        assert!(!VerificationEngine::verify_iommu_isolation(&map).is_pass());
    }

    #[test]
    fn empty_isolation_passes() {
        let map = std::collections::BTreeMap::new();
        assert!(VerificationEngine::verify_iommu_isolation(&map).is_pass());
    }
}

