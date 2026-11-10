//! AI Action Token / Action Process / AI Consciousness 本体。
//!
//! - `AiActionToken`: 每个 AI 模型的能力凭证
//! - `AIConsciousness`: 概念图，驱动语义调度
//! - `ActionProcess`: 带语义的计算任务 (核心调度单元)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::channels::ChannelKind;
use crate::gpu::{GpuCapabilities, GpuCapability, VramPartitionId};
use crate::scheduler::{CpuPriority, GpuScheduler, SchedulerError};

// ============================================================================
// TokenId
// ============================================================================

static NEXT_TOKEN_ID: AtomicU64 = AtomicU64::new(1);

/// Token / 进程 ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenId(pub u64);

impl TokenId {
    /// 分配一个新的唯一 ID。
    pub fn new() -> Self {
        TokenId(NEXT_TOKEN_ID.fetch_add(1, Ordering::SeqCst))
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Default for TokenId {
    fn default() -> Self {
        TokenId::new()
    }
}

impl std::fmt::Display for TokenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Token#{}", self.0)
    }
}

// ============================================================================
// 概念图基础类型
// ============================================================================

/// 概念稳定标识 (128 位)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConceptId(pub [u8; 16]);

impl ConceptId {
    /// 由名称确定性派生 (FNV 风格双散列)。
    pub fn from_name(name: &str) -> Self {
        let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
        let mut h2: u64 = 0x8422_2325_cbf2_9ce4;
        for (i, b) in name.bytes().enumerate() {
            h1 ^= b as u64;
            h1 = h1.wrapping_mul(0x100_0000_01b3);
            h2 = h2
                .rotate_left(7)
                ^ (b as u64).wrapping_mul(31u64.wrapping_add(i as u64));
        }
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&h1.to_be_bytes());
        out[8..].copy_from_slice(&h2.to_be_bytes());
        ConceptId(out)
    }

    /// 十六进制短表示。
    pub fn short_hex(&self) -> String {
        self.0[..4].iter().map(|b| format!("{:02x}", b)).collect()
    }
}

/// 计算模式 —— 归一化的计算描述。
#[derive(Clone, Debug, PartialEq)]
pub enum ComputePattern {
    MatrixMultiply { m: usize, n: usize, k: usize },
    Attention { heads: usize, seq_len: usize },
    Convolution { channels: usize, kernel: usize },
    RandomAccess { pattern: String },
    Custom(String),
}

impl ComputePattern {
    /// 计算模式的类别 (用于语义匹配)。
    pub fn kind(&self) -> &'static str {
        match self {
            ComputePattern::MatrixMultiply { .. } => "matmul",
            ComputePattern::Attention { .. } => "attention",
            ComputePattern::Convolution { .. } => "conv",
            ComputePattern::RandomAccess { .. } => "random_access",
            ComputePattern::Custom(_) => "custom",
        }
    }

    pub fn same_kind(&self, other: &Self) -> bool {
        match (self, other) {
            // 自定义模式必须内容完全相同才算同类，避免误匹配。
            (ComputePattern::Custom(a), ComputePattern::Custom(b)) => a == b,
            _ => self.kind() == other.kind(),
        }
    }
}

/// 调度策略。
#[derive(Clone, Debug, PartialEq)]
pub enum SchedulePolicy {
    /// 硬实时 —— 死亡截止时间
    RealTime,
    /// 软实时 —— 平均截止时间
    SoftRealTime,
    /// 批处理 —— 尽最大努力
    Batch,
    /// 能源保护 —— 低功耗
    EnergySaving,
    /// 高吞吐 —— 最大化利用率
    HighThroughput,
    /// 自定义
    Custom(String),
}

impl SchedulePolicy {
    pub fn name(&self) -> &str {
        match self {
            SchedulePolicy::RealTime => "RealTime",
            SchedulePolicy::SoftRealTime => "SoftRealTime",
            SchedulePolicy::Batch => "Batch",
            SchedulePolicy::EnergySaving => "EnergySaving",
            SchedulePolicy::HighThroughput => "HighThroughput",
            SchedulePolicy::Custom(s) => s,
        }
    }
}

/// 概念节点。
#[derive(Clone, Debug)]
pub struct ConceptNode {
    pub id: ConceptId,
    pub name: String,
    pub compute_pattern: ComputePattern,
    pub schedule_policy: SchedulePolicy,
    pub dependencies: Vec<ConceptId>,
    pub estimated_vram: usize,
    pub priority: f32,
}

/// AI 意识本体 —— 概念图管理器。
#[derive(Clone, Default)]
pub struct AIConsciousness {
    nodes: BTreeMap<ConceptId, ConceptNode>,
    name_index: BTreeMap<String, ConceptId>,
    /// LLM API Key 表：provider -> key
    api_keys: BTreeMap<String, String>,
}

impl AIConsciousness {
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加概念节点。
    pub fn add_concept(&mut self, node: ConceptNode) {
        self.name_index.insert(node.name.clone(), node.id);
        self.nodes.insert(node.id, node);
    }

    /// 便捷添加 (由名称派生 ID)。
    pub fn add_named(
        &mut self,
        name: &str,
        pattern: ComputePattern,
        policy: SchedulePolicy,
        estimated_vram: usize,
        priority: f32,
    ) {
        let id = ConceptId::from_name(name);
        self.add_concept(ConceptNode {
            id,
            name: name.to_string(),
            compute_pattern: pattern,
            schedule_policy: policy,
            dependencies: Vec::new(),
            estimated_vram,
            priority,
        });
    }

    pub fn exists(&self, id: ConceptId) -> bool {
        self.nodes.contains_key(&id)
    }

    pub fn get(&self, id: ConceptId) -> Option<&ConceptNode> {
        self.nodes.get(&id)
    }

    /// 概念优先级 (0.0 - 1.0)。
    pub fn get_priority(&self, id: ConceptId) -> f32 {
        self.nodes.get(&id).map(|n| n.priority).unwrap_or(0.5)
    }

    /// 概念映射到调度器优先级等级。
    pub fn priority_class(&self, id: ConceptId) -> CpuPriority {
        let p = self.get_priority(id);
        if p >= 0.90 {
            CpuPriority::Critical
        } else if p >= 0.80 {
            CpuPriority::High
        } else if p >= 0.60 {
            CpuPriority::Medium
        } else {
            CpuPriority::Low
        }
    }

    pub fn concept_count(&self) -> usize {
        self.nodes.len()
    }

    /// 设置某个 LLM Provider 的 API Key（替换已存在的 key）。
    pub fn set_api_key(&mut self, provider: &str, key: String) {
        self.api_keys.insert(provider.to_string(), key);
    }

    /// 删除某个 LLM Provider 的 API Key。
    pub fn del_api_key(&mut self, provider: &str) {
        self.api_keys.remove(provider);
    }

    /// 获取某个 LLM Provider 的 API Key。
    pub fn api_key(&self, provider: &str) -> Option<&String> {
        self.api_keys.get(provider)
    }

    /// 列出所有已配置的 LLM Provider。
    pub fn api_keys(&self) -> Vec<(String, String)> {
        self.api_keys.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// 便捷：设置 DeepSeek API Key（保留旧命令兼容）。
    pub fn set_deepseek_key(&mut self, key: String) {
        self.api_keys.insert("deepseek".to_string(), key);
    }

    /// 便捷：获取 DeepSeek API Key（保留旧命令兼容）。
    pub fn deepseek_key(&self) -> Option<&String> {
        self.api_keys.get("deepseek")
    }

    /// 便捷：删除 DeepSeek API Key（保留旧命令兼容）。
    pub fn del_deepseek_key(&mut self) {
        self.api_keys.remove("deepseek");
    }

    /// 让意识本体感知一个通道，将其注册为概念节点，使 AI 能“看到”该通道。
    pub fn add_perception_channel(&mut self, name: &str, kind: ChannelKind) {
        let id = ConceptId::from_name(name);
        let pattern = match kind {
            ChannelKind::Context => ComputePattern::Custom(format!("context_{}", name)),
            ChannelKind::Action => ComputePattern::Custom(format!("action_{}", name)),
            ChannelKind::File => ComputePattern::Custom(format!("file_{}", name)),
            ChannelKind::Agent => ComputePattern::Custom(format!("agent_{}", name)),
            ChannelKind::Desktop => ComputePattern::Custom(format!("desktop_{}", name)),
            ChannelKind::Path => ComputePattern::Custom(format!("path_{}", name)),
            ChannelKind::Io => ComputePattern::Custom(format!("io_{}", name)),
            ChannelKind::Gateway => ComputePattern::Custom(format!("gateway_{}", name)),
            ChannelKind::Interface => ComputePattern::Custom(format!("interface_{}", name)),
            ChannelKind::Gpu => ComputePattern::Custom(format!("gpu_{}", name)),
            ChannelKind::Device => ComputePattern::Custom(format!("device_{}", name)),
            ChannelKind::Text => ComputePattern::Custom(format!("text_{}", name)),
            ChannelKind::Trace => ComputePattern::Custom(format!("trace_{}", name)),
            ChannelKind::App => ComputePattern::Custom(format!("app_{}", name)),
            ChannelKind::Browser => ComputePattern::Custom(format!("browser_{}", name)),
            ChannelKind::Driver => ComputePattern::Custom(format!("driver_{}", name)),
            ChannelKind::Plugin => ComputePattern::Custom(format!("plugin_{}", name)),
            ChannelKind::Net => ComputePattern::Custom(format!("net_{}", name)),
            ChannelKind::Memory => ComputePattern::Custom(format!("mem_{}", name)),
            ChannelKind::Process => ComputePattern::Custom(format!("process_{}", name)),
            ChannelKind::User => ComputePattern::Custom(format!("user_{}", name)),
            ChannelKind::Will => ComputePattern::Custom(format!("will_{}", name)),
            ChannelKind::Swap => ComputePattern::Custom(format!("swap_{}", name)),
            ChannelKind::PageTable => ComputePattern::Custom(format!("pagetable_{}", name)),
            ChannelKind::Slab => ComputePattern::Custom(format!("slab_{}", name)),
            ChannelKind::Buddy => ComputePattern::Custom(format!("buddy_{}", name)),
        };
        let policy = match kind {
            ChannelKind::Gpu => SchedulePolicy::HighThroughput,
            ChannelKind::Action => SchedulePolicy::RealTime,
            ChannelKind::Context => SchedulePolicy::SoftRealTime,
            ChannelKind::Net => SchedulePolicy::Batch,
            _ => SchedulePolicy::EnergySaving,
        };
        let priority = match kind {
            ChannelKind::Action => 0.95,
            ChannelKind::Gpu => 0.90,
            ChannelKind::Context => 0.85,
            ChannelKind::Net => 0.70,
            _ => 0.50,
        };
        self.add_concept(ConceptNode {
            id,
            name: name.to_string(),
            compute_pattern: pattern,
            schedule_policy: policy,
            dependencies: Vec::new(),
            estimated_vram: 0,
            priority,
        });
    }

    pub fn concepts(&self) -> impl Iterator<Item = &ConceptNode> {
        self.nodes.values()
    }

    /// 按名称子串搜索。
    pub fn search_by_name(&self, needle: &str) -> Option<&ConceptNode> {
        let needle = needle.to_ascii_lowercase();
        self.nodes
            .values()
            .find(|n| n.name.to_ascii_lowercase().contains(&needle))
    }

    /// 查找调度策略: 先精确匹配，再同类语义匹配。
    pub fn find_schedule_policy(&self, pattern: &ComputePattern) -> Option<SchedulePolicy> {
        if let Some(node) = self.nodes.values().find(|n| &n.compute_pattern == pattern) {
            return Some(node.schedule_policy.clone());
        }
        self.nodes
            .values()
            .filter(|n| n.compute_pattern.same_kind(pattern))
            .max_by(|a, b| {
                a.priority
                    .partial_cmp(&b.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|n| n.schedule_policy.clone())
    }

    /// 常用概念 (10 个)。
    pub fn add_common_concepts(&mut self) {
        use ComputePattern::*;
        use SchedulePolicy::*;

        let table: &[(&str, ComputePattern, SchedulePolicy, usize, f32)] = &[
            (
                "Transformer_Attention_Base",
                Attention { heads: 8, seq_len: 512 },
                SoftRealTime,
                512 * 1024 * 1024,
                0.95,
            ),
            (
                "Transformer_Large_Batch",
                Attention { heads: 16, seq_len: 2048 },
                HighThroughput,
                2 * 1024 * 1024 * 1024,
                0.90,
            ),
            (
                "Transformer_Decoding",
                ComputePattern::Custom("autoregressive_decoding".into()),
                EnergySaving,
                256 * 1024 * 1024,
                0.85,
            ),
            (
                "Conv2D_ImageNet",
                Convolution { channels: 64, kernel: 3 },
                EnergySaving,
                128 * 1024 * 1024,
                0.88,
            ),
            (
                "LSTM_Training",
                ComputePattern::Custom("lstm_backprop".into()),
                SoftRealTime,
                384 * 1024 * 1024,
                0.87,
            ),
            (
                "RL_Policy_Gradient",
                ComputePattern::Custom("policy_gradient".into()),
                SoftRealTime,
                512 * 1024 * 1024,
                0.92,
            ),
            (
                "QLearning_Batch",
                ComputePattern::Custom("q_learning".into()),
                Batch,
                256 * 1024 * 1024,
                0.85,
            ),
            (
                "Vector_Search",
                RandomAccess { pattern: "ANN".into() },
                Batch,
                512 * 1024 * 1024,
                0.78,
            ),
            (
                "MatMul_Benchmark",
                MatrixMultiply { m: 1024, n: 1024, k: 1024 },
                HighThroughput,
                1024 * 1024 * 1024,
                0.93,
            ),
            (
                "Language_Model_NextToken",
                ComputePattern::Custom("next_token_prediction".into()),
                SoftRealTime,
                2 * 1024 * 1024 * 1024,
                0.94,
            ),
        ];

        for (name, pattern, policy, vram, prio) in table {
            self.add_named(name, pattern.clone(), policy.clone(), *vram, *prio);
        }
    }

    /// 扩展概念 (11 个)。
    pub fn add_extended_concepts(&mut self) {
        use ComputePattern::*;
        use SchedulePolicy::*;

        let table: &[(&str, ComputePattern, SchedulePolicy, usize, f32)] = &[
            (
                "Conv3D_Medical",
                Convolution { channels: 32, kernel: 3 },
                HighThroughput,
                256 * 1024 * 1024,
                0.82,
            ),
            (
                "Deconv2D_Generator",
                ComputePattern::Custom("transpose_conv".into()),
                HighThroughput,
                192 * 1024 * 1024,
                0.80,
            ),
            (
                "GRU_Inference",
                ComputePattern::Custom("gru_forward".into()),
                EnergySaving,
                128 * 1024 * 1024,
                0.83,
            ),
            (
                "Matrix_Factorization",
                ComputePattern::Custom("svd".into()),
                EnergySaving,
                384 * 1024 * 1024,
                0.80,
            ),
            (
                "Object_Detection",
                ComputePattern::Custom("detection".into()),
                SoftRealTime,
                768 * 1024 * 1024,
                0.91,
            ),
            (
                "Image_Segmentation",
                ComputePattern::Custom("segmentation".into()),
                EnergySaving,
                640 * 1024 * 1024,
                0.86,
            ),
            (
                "Speech_Recognition",
                ComputePattern::Custom("ctc_decode".into()),
                SoftRealTime,
                320 * 1024 * 1024,
                0.89,
            ),
            (
                "Diffusion_Sampling",
                ComputePattern::Custom("diffusion".into()),
                HighThroughput,
                3 * 1024 * 1024 * 1024,
                0.90,
            ),
            (
                "Embedding_Lookup",
                RandomAccess { pattern: "gather".into() },
                Batch,
                512 * 1024 * 1024,
                0.75,
            ),
            (
                "Graph_Neural_Net",
                ComputePattern::Custom("gnn_message_passing".into()),
                SoftRealTime,
                448 * 1024 * 1024,
                0.84,
            ),
            (
                "Realtime_Video_Encode",
                ComputePattern::Custom("h264_encode".into()),
                RealTime,
                96 * 1024 * 1024,
                0.97,
            ),
        ];

        for (name, pattern, policy, vram, prio) in table {
            self.add_named(name, pattern.clone(), policy.clone(), *vram, *prio);
        }
    }
}

// ============================================================================
// Token 数据结构
// ============================================================================

/// 使用统计。
#[derive(Clone, Debug, Default)]
pub struct UsageStats {
    pub total_run_time_ns: u64,
    pub current_run_time_ns: u64,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub avg_exec_time_ns: u64,
    pub last_activity: u64,
}

/// 有效期。
#[derive(Clone, Debug)]
pub struct ValidityPeriod {
    pub permanent: bool,
    pub created_at: u64,
    pub expires_at: u64,
}

impl Default for ValidityPeriod {
    fn default() -> Self {
        Self {
            permanent: true,
            created_at: 0,
            expires_at: 0,
        }
    }
}

impl ValidityPeriod {
    pub fn permanent() -> Self {
        Self::default()
    }

    pub fn is_valid_at(&self, now: u64) -> bool {
        self.permanent || self.expires_at == 0 || now <= self.expires_at
    }
}

/// 每个 AI 模型的能力凭证。
#[derive(Clone, Debug)]
pub struct AiActionToken {
    pub token_id: TokenId,
    pub capabilities: GpuCapabilities,
    pub vram_partition: VramPartitionId,
    pub priority: CpuPriority,
    pub model_id: String,
    pub usage_stats: UsageStats,
    pub validity: ValidityPeriod,
}

/// Token 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    NotFound,
    Expired,
    InsufficientCapabilities,
    InsufficientVram,
    InvalidRequest,
    AccessDenied,
    OutOfRange,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TokenError::NotFound => "Token 不存在",
            TokenError::Expired => "Token 已过期",
            TokenError::InsufficientCapabilities => "缺少所需能力",
            TokenError::InsufficientVram => "VRAM 不足",
            TokenError::InvalidRequest => "无效请求",
            TokenError::AccessDenied => "访问被拒绝",
            TokenError::OutOfRange => "数值越界",
        };
        write!(f, "{}", s)
    }
}

/// Token 状态查询结果。
#[derive(Clone, Debug)]
pub struct TokenStatus {
    pub token_id: TokenId,
    pub is_valid: bool,
    pub capabilities: GpuCapabilities,
    pub vram_allocated: u64,
    pub priority: CpuPriority,
    pub usage_stats: UsageStats,
    pub model_id: String,
}

/// Token 创建请求。
#[derive(Clone, Debug)]
pub struct TokenCreateRequest {
    pub model_id: String,
    pub requested_caps: GpuCapabilities,
    pub requested_vram: u64,
    pub priority: CpuPriority,
}

impl TokenCreateRequest {
    pub fn new(
        model_id: &str,
        caps: &[GpuCapability],
        vram: u64,
        priority: CpuPriority,
    ) -> Self {
        Self {
            model_id: model_id.to_string(),
            requested_caps: GpuCapabilities::from_caps(caps),
            requested_vram: vram,
            priority,
        }
    }
}

/// Token 更新请求。
#[derive(Clone, Debug)]
pub struct TokenUpdateRequest {
    pub token_id: TokenId,
    pub new_caps: Option<GpuCapabilities>,
    pub new_priority: Option<CpuPriority>,
    pub new_vram_size: Option<u64>,
}

/// Token 管理器 —— 全局注册表。
pub struct AiTokenManager {
    tokens: BTreeMap<TokenId, AiActionToken>,
    model_tokens: BTreeMap<String, Vec<TokenId>>,
    total_vram_allocated: u64,
}

impl AiTokenManager {
    pub fn new() -> Self {
        Self {
            tokens: BTreeMap::new(),
            model_tokens: BTreeMap::new(),
            total_vram_allocated: 0,
        }
    }

    /// 创建 Token。
    pub fn create_token(&mut self, req: &TokenCreateRequest) -> Result<AiActionToken, TokenError> {
        if req.requested_vram == 0 || req.model_id.is_empty() {
            return Err(TokenError::InvalidRequest);
        }
        if req.requested_caps.is_empty() || !req.requested_caps.contains(GpuCapability::Compute) {
            return Err(TokenError::InsufficientCapabilities);
        }

        let mut vram = crate::gpu::vram::vram().lock().unwrap();
        if !vram.check_space(req.requested_vram) {
            return Err(TokenError::InsufficientVram);
        }
        let partition = vram
            .allocate_partition(
                crate::gpu::vram::VramPartitionType::Fixed {
                    start: 0,
                    size: req.requested_vram,
                },
                req.requested_vram,
                crate::gpu::vram::VramFlags::empty(),
            )
            .map_err(|_| TokenError::InsufficientVram)?;
        let vram_partition = crate::gpu::vram::VramPartitionId(partition.iommu_domain.0);
        drop(vram);

        let token = AiActionToken {
            token_id: TokenId::new(),
            capabilities: req.requested_caps,
            vram_partition,
            priority: req.priority,
            model_id: req.model_id.clone(),
            usage_stats: UsageStats::default(),
            validity: ValidityPeriod::permanent(),
        };

        self.tokens.insert(token.token_id, token.clone());
        self.model_tokens
            .entry(req.model_id.clone())
            .or_default()
            .push(token.token_id);
        self.total_vram_allocated += req.requested_vram;

        crate::monitor::record_token_created();
        println!(
            "[token] 创建 {} model={} vram={}B caps=0x{:02x}",
            token.token_id,
            token.model_id,
            req.requested_vram,
            token.capabilities.bits()
        );
        Ok(token)
    }

    /// 销毁 Token。
    pub fn destroy_token(&mut self, id: TokenId) -> Result<(), TokenError> {
        let token = self.tokens.remove(&id).ok_or(TokenError::NotFound)?;
        if let Some(list) = self.model_tokens.get_mut(&token.model_id) {
            list.retain(|t| *t != id);
            if list.is_empty() {
                self.model_tokens.remove(&token.model_id);
            }
        }
        let mut vram = crate::gpu::vram::vram().lock().unwrap();
        let _ = vram.deallocate_partition(crate::gpu::vram::VramPartitionId(
            token.vram_partition.0,
        ));
        drop(vram);

        crate::monitor::record_token_destroyed();
        println!("[token] 销毁 {} (model={})", id, token.model_id);
        Ok(())
    }

    /// 更新能力。
    pub fn update_caps(&mut self, id: TokenId, new_caps: GpuCapabilities) -> Result<(), TokenError> {
        if !new_caps.contains(GpuCapability::Compute) {
            return Err(TokenError::InsufficientCapabilities);
        }
        let token = self.tokens.get_mut(&id).ok_or(TokenError::NotFound)?;
        token.capabilities = new_caps;
        Ok(())
    }

    /// 更新优先级。
    pub fn update_priority(&mut self, id: TokenId, p: CpuPriority) -> Result<(), TokenError> {
        let token = self.tokens.get_mut(&id).ok_or(TokenError::NotFound)?;
        token.priority = p;
        Ok(())
    }

    /// 查询状态。
    pub fn query_status(&self, id: TokenId) -> Result<TokenStatus, TokenError> {
        let token = self.tokens.get(&id).ok_or(TokenError::NotFound)?;
        Ok(TokenStatus {
            token_id: token.token_id,
            is_valid: token.validity.is_valid_at(crate::monitor::get_time_ns()),
            capabilities: token.capabilities,
            vram_allocated: token.usage_stats.current_run_time_ns,
            priority: token.priority,
            usage_stats: token.usage_stats.clone(),
            model_id: token.model_id.clone(),
        })
    }

    /// 某模型下的全部 Token。
    pub fn tokens_of_model(&self, model_id: &str) -> Vec<TokenId> {
        self.model_tokens.get(model_id).cloned().unwrap_or_default()
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn total_vram_allocated(&self) -> u64 {
        self.total_vram_allocated
    }
}

impl Default for AiTokenManager {
    fn default() -> Self {
        Self::new()
    }
}

static TOKEN_MANAGER: OnceLock<Mutex<AiTokenManager>> = OnceLock::new();

/// 全局 Token 管理器。
pub fn token_manager() -> &'static Mutex<AiTokenManager> {
    TOKEN_MANAGER.get_or_init(|| Mutex::new(AiTokenManager::new()))
}

/// 初始化 Token 子系统。
pub fn init() {
    let _ = token_manager();
}

// ---- 便利函数 ----

pub fn create_token_external(req: &TokenCreateRequest) -> Result<AiActionToken, TokenError> {
    token_manager().lock().unwrap().create_token(req)
}

pub fn destroy_token_external(id: TokenId) -> Result<(), TokenError> {
    token_manager().lock().unwrap().destroy_token(id)
}

pub fn update_caps_external(id: TokenId, caps: GpuCapabilities) -> Result<(), TokenError> {
    token_manager().lock().unwrap().update_caps(id, caps)
}

pub fn update_priority_external(id: TokenId, p: CpuPriority) -> Result<(), TokenError> {
    token_manager().lock().unwrap().update_priority(id, p)
}

pub fn query_token_status_external(id: TokenId) -> Result<TokenStatus, TokenError> {
    token_manager().lock().unwrap().query_status(id)
}

// ============================================================================
// Action Process (核心调度单元)
// ============================================================================

/// 行动进程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Ready,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    Preempted,
}

impl ProcessState {
    pub fn name(self) -> &'static str {
        match self {
            ProcessState::Created => "Created",
            ProcessState::Ready => "Ready",
            ProcessState::Running => "Running",
            ProcessState::Waiting => "Waiting",
            ProcessState::Completed => "Completed",
            ProcessState::Failed => "Failed",
            ProcessState::Cancelled => "Cancelled",
            ProcessState::Preempted => "Preempted",
        }
    }
}

/// 图节点 ID。
pub type GraphNodeId = u32;

/// 图算子类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphOpType {
    MatMul,
    Gemm,
    Attention,
    Conv2d,
    Deconv2d,
    Reduce,
    Custom(String),
}

/// 数据类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataType {
    F32,
    F16,
    Bf16,
    I8,
    I16,
    U8,
}

/// 张量布局。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TensorLayout {
    Nhwc,
    Nchw,
    Hw,
    Scalar,
}

/// 张量描述。
#[derive(Clone, Debug)]
pub struct TensorDesc {
    pub dtype: DataType,
    pub shape: Vec<usize>,
    pub layout: TensorLayout,
}

/// 图节点。
#[derive(Clone, Debug)]
pub struct GraphNode {
    pub id: GraphNodeId,
    pub op_type: GraphOpType,
}

/// 图边 (数据依赖)。
#[derive(Clone, Debug)]
pub struct GraphEdge {
    pub from: GraphNodeId,
    pub to: GraphNodeId,
    pub data_size: usize,
}

/// 计算图。
#[derive(Clone, Debug, Default)]
pub struct ComputeGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub inputs: Vec<TensorDesc>,
    pub outputs: Vec<TensorDesc>,
}

impl ComputeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, op_type: GraphOpType) -> GraphNodeId {
        let id = self.nodes.len() as GraphNodeId;
        self.nodes.push(GraphNode { id, op_type });
        id
    }

    pub fn add_edge(&mut self, from: GraphNodeId, to: GraphNodeId, data_size: usize) {
        self.edges.push(GraphEdge {
            from,
            to,
            data_size,
        });
    }

    /// 图中全部算子类型。
    pub fn op_types(&self) -> Vec<GraphOpType> {
        self.nodes.iter().map(|n| n.op_type.clone()).collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// GPU 事件句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuEventHandle(pub u32);

/// VRAM 资源令牌。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VramToken {
    pub concept_id: ConceptId,
    pub size: usize,
}

impl VramToken {
    /// 预留 VRAM。
    pub fn allocate(concept_id: ConceptId, size: usize) -> Result<Self, String> {
        let mut vram = crate::gpu::vram::vram().lock().unwrap();
        vram.reserve(size as u64)
            .map_err(|e| format!("VRAM 分配失败: {}", e))?;
        Ok(Self { concept_id, size })
    }
}

/// 资源句柄集合。
#[derive(Clone, Copy, Debug)]
pub struct ResourceHandles {
    pub gpu_event: GpuEventHandle,
    pub vram_partition: VramToken,
}

/// 错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    OutOfVram,
    Timeout,
    GpuError,
    AccessViolation,
    OutOfResources,
}

impl ErrorCode {
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::OutOfVram => "OutOfVram",
            ErrorCode::Timeout => "Timeout",
            ErrorCode::GpuError => "GpuError",
            ErrorCode::AccessViolation => "AccessViolation",
            ErrorCode::OutOfResources => "OutOfResources",
        }
    }
}

/// 进程执行结果。
#[derive(Clone, Debug)]
pub struct ProcessResult {
    pub success: bool,
    pub output_data: Option<Vec<u8>>,
    pub execution_time_ns: u64,
    pub vram_usage: usize,
    pub error_code: Option<ErrorCode>,
}

/// 行动进程 —— Hib OS 的核心调度单元。
#[derive(Clone, Debug)]
pub struct ActionProcess {
    pub pid: TokenId,
    pub ontology_id: ConceptId,
    pub compute_graph: ComputeGraph,
    pub vram_token: VramToken,
    pub state: ProcessState,
    pub priority: CpuPriority,
    pub deadline: Option<Duration>,
    pub resources: ResourceHandles,
    pub created_at: Instant,
}

impl ActionProcess {
    /// 创建进程。
    pub fn new(
        consciousness: &AIConsciousness,
        ontology_id: ConceptId,
        compute_graph: ComputeGraph,
        vram_size: usize,
    ) -> Result<Self, String> {
        if !consciousness.exists(ontology_id) {
            return Err(format!("未知概念: {}", ontology_id.short_hex()));
        }
        let vram_token = VramToken::allocate(ontology_id, vram_size)?;
        let priority = consciousness.priority_class(ontology_id);
        Ok(Self {
            pid: TokenId::new(),
            ontology_id,
            compute_graph,
            vram_token,
            state: ProcessState::Created,
            priority,
            deadline: None,
            resources: ResourceHandles {
                gpu_event: GpuEventHandle(0),
                vram_partition: vram_token,
            },
            created_at: Instant::now(),
        })
    }

    /// 设置截止时间。
    pub fn with_deadline(mut self, d: Duration) -> Self {
        self.deadline = Some(d);
        self
    }

    /// 由计算图推断计算模式。
    pub fn compute_pattern(&self) -> ComputePattern {
        match self.compute_graph.op_types().first() {
            Some(GraphOpType::Attention) => ComputePattern::Attention {
                heads: 8,
                seq_len: 512,
            },
            Some(GraphOpType::MatMul) | Some(GraphOpType::Gemm) => {
                ComputePattern::MatrixMultiply {
                    m: 128,
                    n: 128,
                    k: 64,
                }
            }
            Some(GraphOpType::Conv2d) | Some(GraphOpType::Deconv2d) => {
                ComputePattern::Convolution {
                    channels: 64,
                    kernel: 3,
                }
            }
            Some(GraphOpType::Custom(name)) => ComputePattern::Custom(name.clone()),
            _ => ComputePattern::Custom("unknown".to_string()),
        }
    }

    /// 提交到调度器。
    pub fn submit(&mut self, scheduler: &mut GpuScheduler) -> Result<(), SchedulerError> {
        if self.state != ProcessState::Created && self.state != ProcessState::Preempted {
            return Err(SchedulerError::InvalidProcessState(format!(
                "进程处于 {} 状态, 无法提交",
                self.state.name()
            )));
        }
        let pattern = self.compute_pattern();
        let _policy = scheduler
            .ai_consciousness
            .find_schedule_policy(&pattern)
            .unwrap_or(SchedulePolicy::Batch);

        self.state = ProcessState::Ready;
        self.priority = scheduler.ai_consciousness.priority_class(self.ontology_id);
        scheduler.enqueue(self.pid, self.priority, Some(self.ontology_id));
        Ok(())
    }

    /// 实时任务紧急提交。
    pub fn emergency_submit(
        &mut self,
        scheduler: &mut GpuScheduler,
    ) -> Result<(), SchedulerError> {
        self.priority = CpuPriority::Critical;
        self.submit(scheduler)
    }

    /// 是否超时。
    pub fn is_timeout(&self) -> bool {
        match self.deadline {
            Some(d) => self.created_at.elapsed() > d,
            None => false,
        }
    }

    /// 标记完成。
    pub fn complete(mut self, output: Option<Vec<u8>>) -> ProcessResult {
        self.state = ProcessState::Completed;
        let elapsed = self.created_at.elapsed().as_nanos() as u64;
        crate::gpu::vram::vram()
            .lock()
            .unwrap()
            .release(self.vram_token.size as u64);
        crate::monitor::record_task_completed(elapsed);
        ProcessResult {
            success: true,
            output_data: output,
            execution_time_ns: elapsed,
            vram_usage: self.vram_token.size,
            error_code: None,
        }
    }

    /// 标记失败。
    pub fn fail(mut self, error: ErrorCode) -> ProcessResult {
        self.state = ProcessState::Failed;
        let elapsed = self.created_at.elapsed().as_nanos() as u64;
        crate::gpu::vram::vram()
            .lock()
            .unwrap()
            .release(self.vram_token.size as u64);
        crate::monitor::record_task_failed();
        ProcessResult {
            success: false,
            output_data: None,
            execution_time_ns: elapsed,
            vram_usage: self.vram_token.size,
            error_code: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consciousness() -> AIConsciousness {
        let mut c = AIConsciousness::new();
        c.add_common_concepts();
        c.add_extended_concepts();
        c
    }

    #[test]
    fn concept_graph_has_21_nodes() {
        let c = consciousness();
        assert_eq!(c.concept_count(), 21);
        assert!(c.search_by_name("attention").is_some());
        assert!(c.search_by_name("Object_Detection").is_some());
        assert!(c.search_by_name("不存在的概念").is_none());
    }

    #[test]
    fn exact_policy_lookup() {
        let c = consciousness();
        let node = c.search_by_name("MatMul_Benchmark").unwrap().clone();
        assert_eq!(
            c.find_schedule_policy(&node.compute_pattern),
            Some(SchedulePolicy::HighThroughput)
        );
    }

    #[test]
    fn semantic_fallback_policy() {
        let c = consciousness();
        // 无法精确匹配，但同类 attention → 取最高优先级节点策略
        let p = c.find_schedule_policy(&ComputePattern::Attention {
            heads: 99,
            seq_len: 7,
        });
        assert_eq!(p, Some(SchedulePolicy::SoftRealTime));
        // 完全未知类别
        let unknown = c.find_schedule_policy(&ComputePattern::Custom("nope".into()));
        assert_eq!(unknown, None);
    }

    #[test]
    fn priority_class_buckets() {
        let c = consciousness();
        let critical = c.search_by_name("Realtime_Video_Encode").unwrap().id;
        let high = c.search_by_name("Matrix_Factorization").unwrap().id;
        assert_eq!(c.priority_class(critical), CpuPriority::Critical);
        assert_eq!(c.priority_class(high), CpuPriority::High);
        // 未知概念回落到 0.5 → Low
        assert_eq!(
            c.priority_class(ConceptId::from_name("unknown-x")),
            CpuPriority::Low
        );
    }

    #[test]
    fn concept_id_is_deterministic() {
        assert_eq!(
            ConceptId::from_name("Transformer_Attention_Base"),
            ConceptId::from_name("Transformer_Attention_Base")
        );
        assert_ne!(
            ConceptId::from_name("a"),
            ConceptId::from_name("b")
        );
    }

    #[test]
    fn token_lifecycle() {
        let mut mgr = AiTokenManager::new();
        let req = TokenCreateRequest::new(
            "unit-test-model",
            &[GpuCapability::Compute, GpuCapability::Tensor],
            1024 * 1024,
            CpuPriority::Medium,
        );
        let token = mgr.create_token(&req).unwrap();
        assert_eq!(mgr.token_count(), 1);
        assert!(token.capabilities.contains(GpuCapability::Tensor));

        let status = mgr.query_status(token.token_id).unwrap();
        assert!(status.is_valid);
        assert_eq!(status.model_id, "unit-test-model");
        assert_eq!(mgr.tokens_of_model("unit-test-model"), vec![token.token_id]);

        mgr.destroy_token(token.token_id).unwrap();
        assert_eq!(mgr.token_count(), 0);
        assert_eq!(
            mgr.query_status(token.token_id).unwrap_err(),
            TokenError::NotFound
        );
    }

    #[test]
    fn invalid_token_requests() {
        let mut mgr = AiTokenManager::new();
        let empty_model = TokenCreateRequest::new(
            "",
            &[GpuCapability::Compute],
            1024,
            CpuPriority::Low,
        );
        assert_eq!(
            mgr.create_token(&empty_model).unwrap_err(),
            TokenError::InvalidRequest
        );

        let no_compute = TokenCreateRequest::new(
            "m",
            &[GpuCapability::Tensor],
            1024,
            CpuPriority::Low,
        );
        assert_eq!(
            mgr.create_token(&no_compute).unwrap_err(),
            TokenError::InsufficientCapabilities
        );
    }

    #[test]
    fn token_update_caps_and_priority() {
        let mut mgr = AiTokenManager::new();
        let req = TokenCreateRequest::new(
            "update-model",
            &[GpuCapability::Compute],
            4096,
            CpuPriority::Low,
        );
        let t = mgr.create_token(&req).unwrap();

        let caps = GpuCapabilities::from_caps(&[GpuCapability::Compute, GpuCapability::Media]);
        mgr.update_caps(t.token_id, caps).unwrap();
        assert!(mgr
            .query_status(t.token_id)
            .unwrap()
            .capabilities
            .contains(GpuCapability::Media));

        mgr.update_priority(t.token_id, CpuPriority::Critical).unwrap();
        assert_eq!(
            mgr.query_status(t.token_id).unwrap().priority,
            CpuPriority::Critical
        );
        mgr.destroy_token(t.token_id).unwrap();
    }

    #[test]
    fn action_process_lifecycle() {
        let c = consciousness();
        let concept = c.search_by_name("MatMul_Benchmark").unwrap().id;

        let mut graph = ComputeGraph::new();
        let a = graph.add_node(GraphOpType::MatMul);
        let b = graph.add_node(GraphOpType::Reduce);
        graph.add_edge(a, b, 1024);

        let mut process = ActionProcess::new(&c, concept, graph, 4096).unwrap();
        assert_eq!(process.state, ProcessState::Created);
        assert_eq!(process.compute_pattern().kind(), "matmul");

        let mut scheduler = crate::scheduler::GpuScheduler::new(c.clone());
        process.submit(&mut scheduler).unwrap();
        assert_eq!(process.state, ProcessState::Ready);
        assert_eq!(scheduler.ready_count(), 1);

        let result = process.complete(Some(vec![1, 2, 3]));
        assert!(result.success);
        assert_eq!(result.output_data.unwrap().len(), 3);
    }

    #[test]
    fn action_process_unknown_concept_rejected() {
        let c = AIConsciousness::new();
        let r = ActionProcess::new(&c, ConceptId([0u8; 16]), ComputeGraph::new(), 16);
        assert!(r.is_err());
    }

    #[test]
    fn double_submit_rejected() {
        let c = consciousness();
        let concept = c.search_by_name("QLearning_Batch").unwrap().id;
        let mut process = ActionProcess::new(&c, concept, ComputeGraph::new(), 64).unwrap();
        let mut scheduler = crate::scheduler::GpuScheduler::new(c.clone());
        process.submit(&mut scheduler).unwrap();
        assert!(process.submit(&mut scheduler).is_err());
    }
}

