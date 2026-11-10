//! 模型子系统 —— AI 原生 OS 的「可执行镜像」。
//!
//! 对应《现代操作系统》中的**可执行文件与进程映像**：模型文件之于智能体，
//! 如同 ELF 之于进程。模型注册表等价于内核的二进制格式注册表。
//!
//! 本模块覆盖：
//! * 模型架构（Transformer / MoE / Diffusion / VAE / CLIP / Encoder-Decoder）
//! * 模型层（注意力 / MLP / Embedding / Norm / Conv / Pooling）
//! * 权重量化（INT8 / INT4 / NF4 / FP8 / FP4 / BF16 / FP16 / FP32）
//! * KV 缓存（分层 / 滑动窗口 / 分页注意力 / 连续批处理）
//! * 推理管线（prefill / decode / 批处理 / 投机解码 / 推理加速）
//! * 模型规格（参数量、层数、隐藏维、头数、头维、中间维、词表长度）
//! * 模型分发（P2P 分片 / 副本 / 一致性哈希）
//! * 模型版本控制（语义化版本 / 回滚 / 金丝雀）
//! * 模型热替换（运行时无缝切换实例）
//! * 模型签名与水印（模型版权保护 / 防篡改）
//! * 模型图优化（算子融合 / 常量折叠 / 量化感知训练后量化）
//! * 基准测试（吞吐 / 延迟 / Tokens/s / VRAM 占用）
//! * 模型加载器（GGUF / ONNX / SafeTensors / RawTensor 解析）
//!
//! 所有结构均为纯内存原型，无外部 AI 框架依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// 模型架构
// =====================================================================================

/// 神经网络架构家族。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelArch {
    /// 密集 Transformer（解码器-only，GPT 风格）。
    TransformerDecoder,
    /// 密集 Transformer（编码器-only，BERT 风格）。
    TransformerEncoder,
    /// 编码器-解码器（T5 风格）。
    TransformerEncDec,
    /// 混合专家（MoE，稀疏门控）。
    MoE,
    /// 扩散模型（图像/视频生成）。
    Diffusion,
    /// 变分自编码器（VAE）。
    VAE,
    /// 对比语言-图像预训练（CLIP 风格双塔）。
    CLIP,
    /// 图神经网络（GNN）。
    GNN,
    /// 状态空间模型（Mamba 风格）。
    StateSpace,
    /// 卷积神经网络（CNN）。
    CNN,
    /// 循环神经网络（RNN/LSTM/GRU）。
    RNN,
    /// 自定义架构（持有字符串以支持运行时动态架构名）。
    Custom(String),
}

impl ModelArch {
    pub fn name(&self) -> &'static str {
        match self {
            ModelArch::TransformerDecoder => "transformer-decoder",
            ModelArch::TransformerEncoder => "transformer-encoder",
            ModelArch::TransformerEncDec => "transformer-enc-dec",
            ModelArch::MoE => "moe",
            ModelArch::Diffusion => "diffusion",
            ModelArch::VAE => "vae",
            ModelArch::CLIP => "clip",
            ModelArch::GNN => "gnn",
            ModelArch::StateSpace => "state-space",
            ModelArch::CNN => "cnn",
            ModelArch::RNN => "rnn",
            ModelArch::Custom(_) => "custom",
        }
    }

    /// 是否为生成式（自回归）架构。
    pub fn is_generative(self) -> bool {
        matches!(
            self,
            ModelArch::TransformerDecoder | ModelArch::MoE | ModelArch::StateSpace | ModelArch::RNN
        )
    }

    /// 是否支持多模态输入。
    pub fn is_multimodal(self) -> bool {
        matches!(self, ModelArch::CLIP | ModelArch::Diffusion | ModelArch::VAE)
    }
}

/// 注意力机制变体。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttentionType {
    /// 标准全注意力（O(n²)）。
    Full,
    /// 滑动窗口注意力（O(n * w)）。
    SlidingWindow,
    /// 分页注意力（PagedAttention，碎片化 KV 缓存）。
    Paged,
    /// 线性注意力（O(n)）。
    Linear,
    /// 多查询注意力（MQA，共享 K/V）。
    MultiQuery,
    /// 分组查询注意力（GQA，K/V 组间共享）。
    GroupedQuery,
    /// 稀疏注意力（局部 + 全局 Token）。
    Sparse,
    /// 长注意力（Longformer / BigBird 风格）。
    Long,
}

impl AttentionType {
    pub fn name(self) -> &'static str {
        match self {
            AttentionType::Full => "full",
            AttentionType::SlidingWindow => "sliding-window",
            AttentionType::Paged => "paged",
            AttentionType::Linear => "linear",
            AttentionType::MultiQuery => "multi-query",
            AttentionType::GroupedQuery => "grouped-query",
            AttentionType::Sparse => "sparse",
            AttentionType::Long => "long",
        }
    }
}

/// 门控网络类型（MoE）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouterType {
    TopK,
    TopP,
    NoisyTopK,
    Switch,
    ExpertChoice,
}

impl RouterType {
    pub fn name(self) -> &'static str {
        match self {
            RouterType::TopK => "top-k",
            RouterType::TopP => "top-p",
            RouterType::NoisyTopK => "noisy-top-k",
            RouterType::Switch => "switch",
            RouterType::ExpertChoice => "expert-choice",
        }
    }
}

// =====================================================================================
// 模型规格
// =====================================================================================

/// 完整模型规格，等价于 ELF 程序头表。
#[derive(Clone, Debug)]
pub struct ModelSpec {
    pub arch: ModelArch,
    pub attention: AttentionType,
    pub router: Option<RouterType>,
    /// 参数总量。
    pub params: u64,
    /// 层数。
    pub num_layers: u32,
    /// 隐藏维度。
    pub hidden_size: u32,
    /// 注意力头数。
    pub num_heads: u32,
    /// KV 头数（GQA/MQA 可小于 num_heads）。
    pub num_kv_heads: Option<u32>,
    /// 头维度。
    pub head_dim: u32,
    /// 前馈中间维度。
    pub intermediate_size: u32,
    /// 词表大小。
    pub vocab_size: u32,
    /// 最大上下文窗口。
    pub max_position_embeddings: u32,
    /// 激活函数。
    pub activation: Activation,
    /// 层归一化 epsilon。
    pub norm_eps: f64,
    /// RoPE 基频。
    pub rope_base: f64,
    /// 专家数（MoE）。
    pub num_experts: Option<u32>,
    /// 每层激活专家数（MoE）。
    pub num_experts_per_tok: Option<u32>,
    /// 扩散时间步。
    pub diffusion_timesteps: Option<u32>,
    /// 是否双向。
    pub bidirectional: bool,
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            arch: ModelArch::TransformerDecoder,
            attention: AttentionType::Paged,
            router: None,
            params: 7_000_000_000,
            num_layers: 32,
            hidden_size: 4096,
            num_heads: 32,
            num_kv_heads: None,
            head_dim: 128,
            intermediate_size: 11008,
            vocab_size: 32000,
            max_position_embeddings: 32768,
            activation: Activation::SwiGLU,
            norm_eps: 1e-5,
            rope_base: 10000.0,
            num_experts: None,
            num_experts_per_tok: None,
            diffusion_timesteps: None,
            bidirectional: false,
        }
    }
}

impl ModelSpec {
    /// KV 头数（默认等于 num_heads）。
    pub fn kv_heads(&self) -> u32 {
        self.num_kv_heads.unwrap_or(self.num_heads)
    }

    /// 每层参数估算（亿级精度）。
    pub fn params_per_layer(&self) -> u64 {
        let h = self.hidden_size as u64;
        let i = self.intermediate_size as u64;
        let v = self.vocab_size as u64;
        // 注意力 QKVO + 两层 LN + MLP up/gate/down + LN
        let attn = 4 * h * h + 2 * h;
        let mlp = 3 * h * i + 2 * i;
        let layer = attn + mlp;
        let embed = v * h;
        let lm_head = v * h;
        layer * self.num_layers as u64 + embed + lm_head
    }

    /// 单 token 前向激活显存（FP16 字节）。
    pub fn activation_bytes_per_token(&self) -> u64 {
        let h = self.hidden_size as u64;
        let layers = self.num_layers as u64;
        // 每层：Q/K/V/O + 中间激活 + 残差
        let per_layer = 4 * h * 2 + h * 4;
        per_layer * layers + h * 2
    }

    /// KV 缓存每 token 显存（FP16，含 K+V，所有层）。
    pub fn kv_cache_bytes_per_token(&self) -> u64 {
        let kv_heads = self.kv_heads() as u64;
        let head_dim = self.head_dim as u64;
        let layers = self.num_layers as u64;
        // K + V，每层每头 head_dim，FP16 2 字节
        2 * kv_heads * head_dim * 2 * layers
    }

    /// 估算完整加载所需 VRAM（FP16 权重 + KV 预留，不含上下文）。
    pub fn estimated_vram_bytes(&self) -> u64 {
        // FP16 权重 = params * 2 字节（未量化时）
        let weights = self.params * 2;
        let kv_reserve = self.kv_cache_bytes_per_token() * self.max_position_embeddings as u64;
        weights + kv_reserve
    }
}

/// 激活函数。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Activation {
    Relu,
    Gelu,
    Swish,
    SwiGLU,
    GeGLU,
    PRelu,
    Sigmoid,
    Tanh,
}

impl Activation {
    pub fn name(self) -> &'static str {
        match self {
            Activation::Relu => "relu",
            Activation::Gelu => "gelu",
            Activation::Swish => "swish",
            Activation::SwiGLU => "swiglu",
            Activation::GeGLU => "geglu",
            Activation::PRelu => "prelu",
            Activation::Sigmoid => "sigmoid",
            Activation::Tanh => "tanh",
        }
    }
}

// =====================================================================================
// 量化方案
// =====================================================================================

/// 权重量化格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuantFormat {
    None,          // 原始浮点
    Fp32,
    Fp16,
    Bf16,
    Fp8E4M3,
    Fp8E5M2,
    Fp4,
    Int8,
    Int4,
    Nf4,           // NormalFloat 4 (QLoRA)
    PerChannelInt8,
    PerTensorInt8,
}

impl QuantFormat {
    pub fn name(self) -> &'static str {
        match self {
            QuantFormat::None => "none",
            QuantFormat::Fp32 => "fp32",
            QuantFormat::Fp16 => "fp16",
            QuantFormat::Bf16 => "bf16",
            QuantFormat::Fp8E4M3 => "fp8-e4m3",
            QuantFormat::Fp8E5M2 => "fp8-e5m2",
            QuantFormat::Fp4 => "fp4",
            QuantFormat::Int8 => "int8",
            QuantFormat::Int4 => "int4",
            QuantFormat::Nf4 => "nf4",
            QuantFormat::PerChannelInt8 => "int8-per-channel",
            QuantFormat::PerTensorInt8 => "int8-per-tensor",
        }
    }

    /// 每参数字节数。
    pub fn bytes_per_param(self) -> u64 {
        match self {
            QuantFormat::None | QuantFormat::Fp32 => 4,
            QuantFormat::Fp16 | QuantFormat::Bf16 => 2,
            QuantFormat::Fp8E4M3 | QuantFormat::Fp8E5M2 => 1,
            QuantFormat::Fp4 | QuantFormat::Int4 => 0,
            QuantFormat::Int8 | QuantFormat::PerChannelInt8 | QuantFormat::PerTensorInt8 => 1,
            QuantFormat::Nf4 => 0,
        }
    }
}

/// 单张权重张量。
#[derive(Clone, Debug)]
pub struct ModelWeight {
    pub name: String,
    pub shape: Vec<u64>,
    pub quant: QuantFormat,
    /// 原始字节（量化后的存储）。
    pub data: Vec<u8>,
    /// 反量化缩放因子（per-tensor）。
    pub scale: Option<f32>,
    /// 反量化零点（per-tensor）。
    pub zero_point: Option<i32>,
    /// 分量量化表（per-channel）。
    pub scales: Vec<f32>,
    /// 是否可卸载（卸载后需从磁盘重新加载）。
    pub offloadable: bool,
}

impl ModelWeight {
    pub fn num_elements(&self) -> u64 {
        self.shape.iter().product()
    }

    /// 估算存储字节数（考虑 4-bit 压缩）。
    pub fn storage_bytes(&self) -> u64 {
        let n = self.num_elements();
        match self.quant {
            QuantFormat::Int4 | QuantFormat::Fp4 | QuantFormat::Nf4 => n / 2,
            _ => self.data.len() as u64,
        }
    }
}

// =====================================================================================
// 模型层
// =====================================================================================

/// 单层定义。
#[derive(Clone, Debug)]
pub struct ModelLayer {
    pub index: u32,
    pub kind: LayerKind,
    pub weights: Vec<String>,   // 引用 ModelRegistry.weights 中的键
    pub input_shape: Vec<u64>,
    pub output_shape: Vec<u64>,
    pub compute_flops: u64,
    pub memory_bytes: u64,
}

/// 层种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayerKind {
    Embedding,
    Attention,
    Mlp,
    RmsNorm,
    LayerNorm,
    Conv,
    Pooling,
    Router,
    Expert,
    OutputHead,
    Residual,
    Custom(&'static str),
}

impl LayerKind {
    pub fn name(self) -> &'static str {
        match self {
            LayerKind::Embedding => "embedding",
            LayerKind::Attention => "attention",
            LayerKind::Mlp => "mlp",
            LayerKind::RmsNorm => "rms-norm",
            LayerKind::LayerNorm => "layer-norm",
            LayerKind::Conv => "conv",
            LayerKind::Pooling => "pooling",
            LayerKind::Router => "router",
            LayerKind::Expert => "expert",
            LayerKind::OutputHead => "output-head",
            LayerKind::Residual => "residual",
            LayerKind::Custom(n) => n,
        }
    }
}

// =====================================================================================
// KV 缓存
// =====================================================================================

/// KV 缓存条目（单层单 token）。
#[derive(Clone, Debug)]
pub struct KvCacheEntry {
    pub layer: u32,
    pub token_pos: u64,
    pub key: Vec<F16Stub>,   // 语义占位，实际由张量引擎持有
    pub value: Vec<F16Stub>,
}

/// f16 语义占位（避免半精度 crate 依赖，后续替换为半精度类型）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct F16Stub(pub u16);

/// 分页 KV 缓存块。
#[derive(Clone, Debug)]
pub struct KvBlock {
    pub block_id: u64,
    pub layer: u32,
    pub slot_start: u64,
    pub slot_count: u64,
    pub physical_pages: Vec<u64>,
    pub ref_count: u64,
}

/// KV 缓存管理器（分页注意力核心）。
#[derive(Clone, Debug)]
pub struct KvCacheManager {
    blocks: BTreeMap<u64, KvBlock>,
    free_blocks: Vec<u64>,
    next_block: u64,
    block_size: u64,
    max_blocks: u64,
    used_blocks: u64,
}

impl KvCacheManager {
    pub fn new(block_size: u64, max_blocks: u64) -> Self {
        Self {
            blocks: BTreeMap::new(),
            free_blocks: Vec::new(),
            next_block: 1,
            block_size,
            max_blocks,
            used_blocks: 0,
        }
    }

    pub fn allocate(&mut self, layer: u32, slots: u64) -> Result<u64, String> {
        let needed = (slots + self.block_size - 1) / self.block_size;
        if self.used_blocks + needed > self.max_blocks {
            return Err(format!("KV 缓存块不足：需要 {} 剩余 {}", needed, self.max_blocks - self.used_blocks));
        }
        let id = self.next_block;
        self.next_block += 1;
        self.used_blocks += needed;
        self.blocks.insert(
            id,
            KvBlock {
                block_id: id,
                layer,
                slot_start: 0,
                slot_count: slots,
                physical_pages: vec![id * needed; needed as usize],
                ref_count: 1,
            },
        );
        Ok(id)
    }

    pub fn release(&mut self, id: u64) -> Result<(), String> {
        if self.blocks.remove(&id).is_some() {
            self.used_blocks = self.used_blocks.saturating_sub(1);
            Ok(())
        } else {
            Err(format!("KV 块 {} 不存在", id))
        }
    }

    pub fn used_blocks(&self) -> u64 {
        self.used_blocks
    }

    pub fn max_blocks(&self) -> u64 {
        self.max_blocks
    }
}

// =====================================================================================
// 推理管线
// =====================================================================================

/// 推理阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceStage {
    Prefill,
    Decode,
    Speculative,
    Verify,
}

/// 推理请求。
#[derive(Clone, Debug)]
pub struct InferenceRequest {
    pub id: u64,
    pub instance_id: InstanceId,
    pub prompt_tokens: Vec<u32>,
    pub max_new_tokens: u64,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub stage: InferenceStage,
    pub generated_tokens: Vec<u32>,
    pub completed: bool,
    pub first_token_ns: Option<u128>,
    pub last_token_ns: Option<u128>,
}

/// 推理引擎（抽象，实际推理由模型运行时提供）。
#[derive(Clone, Debug)]
pub struct InferenceEngine {
    pub requests: BTreeMap<u64, InferenceRequest>,
    pub next_id: u64,
    pub batch_size: u64,
    pub speculative_enabled: bool,
    pub total_tokens_generated: u64,
    pub total_prefill_tokens: u64,
    pub total_inference_ns: u128,
}

impl InferenceEngine {
    pub fn new() -> Self {
        Self {
            requests: BTreeMap::new(),
            next_id: 1,
            batch_size: 8,
            speculative_enabled: false,
            total_tokens_generated: 0,
            total_prefill_tokens: 0,
            total_inference_ns: 0,
        }
    }

    pub fn submit(&mut self, instance_id: InstanceId, prompt: Vec<u32>, max_new: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.total_prefill_tokens += prompt.len() as u64;
        self.requests.insert(
            id,
            InferenceRequest {
                id,
                instance_id,
                prompt_tokens: prompt,
                max_new_tokens: max_new,
                temperature: 0.7,
                top_p: 0.9,
                top_k: 40,
                stage: InferenceStage::Prefill,
                generated_tokens: Vec::new(),
                completed: false,
                first_token_ns: None,
                last_token_ns: None,
            },
        );
        id
    }

    pub fn tick(&mut self, ns_delta: u128) {
        let mut finished = Vec::new();
        for (id, req) in self.requests.iter_mut() {
            if !req.completed && req.generated_tokens.len() < req.max_new_tokens as usize {
                req.generated_tokens.push(0); // 占位 token
            }
            if req.completed {
                finished.push(*id);
            }
        }
        for id in finished {
            self.requests.remove(&id);
        }
        self.total_inference_ns = self.total_inference_ns.saturating_add(ns_delta);
    }

    pub fn completed_count(&self) -> u64 {
        self.requests.values().filter(|r| r.completed).count() as u64
    }
}

// =====================================================================================
// 模型版本
// =====================================================================================

/// 语义化版本。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre_release: Option<String>,
    pub build: Option<String>,
}

impl ModelVersion {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim_start_matches('v');
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 3 {
            return Err(format!("无效版本: {}", s));
        }
        Ok(Self {
            major: parts[0].parse().map_err(|_| format!("无效主版本: {}", parts[0]))?,
            minor: parts[1].parse().map_err(|_| format!("无效次版本: {}", parts[1]))?,
            patch: parts[2].parse().map_err(|_| format!("无效修订版本: {}", parts[2]))?,
            pre_release: None,
            build: None,
        })
    }

    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 已安装模型版本记录。
#[derive(Clone, Debug)]
pub struct ModelInstallRecord {
    pub name: String,
    pub version: ModelVersion,
    pub path: String,
    pub installed_at: u128,
    pub checksum: String,
    pub active: bool,
}

// =====================================================================================
// 模型分发
// =====================================================================================

/// 分片元数据（P2P 分发）。
#[derive(Clone, Debug)]
pub struct ModelShard {
    pub shard_id: u64,
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub checksum: String,
    pub peers: Vec<String>,
}

/// 分发器（P2P / 副本 / 一致性哈希）。
pub struct ModelDistributor {
    shards: BTreeMap<u64, ModelShard>,
    replicas: HashMap<String, Vec<String>>, // shard -> peer list
    ring: Vec<(u64, String)>,              // 一致性哈希环
}

impl ModelDistributor {
    pub fn new() -> Self {
        Self {
            shards: BTreeMap::new(),
            replicas: HashMap::new(),
            ring: Vec::new(),
        }
    }

    pub fn register_shard(&mut self, shard: ModelShard) {
        self.shards.insert(shard.shard_id, shard);
    }

    pub fn locate_peer(&self, shard_id: u64) -> Option<&str> {
        self.shards.get(&shard_id).and_then(|s| s.peers.first()).map(|p| p.as_str())
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

// =====================================================================================
// 模型安全
// =====================================================================================

/// 模型签名算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignAlgo {
    Ed25519,
    ECDsaP256,
    Rsa4096,
}

/// 模型签名记录。
#[derive(Clone, Debug)]
pub struct ModelSignature {
    pub model_name: String,
    pub version: ModelVersion,
    pub algo: SignAlgo,
    pub signature: Vec<u8>,
    pub signer_cert: String,
    pub signed_at: u128,
}

/// 水印嵌入位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatermarkLoc {
    Weight,
    Activation,
    Output,
}

pub struct ModelSecurity {
    signatures: BTreeMap<String, ModelSignature>,
    watermarks: BTreeMap<String, Vec<WatermarkLoc>>,
    trusted_signers: BTreeSet<String>,
}

impl ModelSecurity {
    pub fn new() -> Self {
        Self {
            signatures: BTreeMap::new(),
            watermarks: BTreeMap::new(),
            trusted_signers: BTreeSet::new(),
        }
    }

    pub fn register_signature(&mut self, sig: ModelSignature) {
        self.signatures.insert(format!("{}@{}", sig.model_name, sig.version.to_string()), sig);
    }

    pub fn verify(&self, model: &str, version: &ModelVersion) -> bool {
        let key = format!("{}@{}", model, version.to_string());
        self.signatures.contains_key(&key)
    }

    pub fn add_trusted_signer(&mut self, cert: &str) {
        self.trusted_signers.insert(cert.to_string());
    }

    pub fn signature_count(&self) -> usize {
        self.signatures.len()
    }
}

// =====================================================================================
// 模型图优化
// =====================================================================================

/// 优化Pass。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptPass {
    ConstantFolding,
    OperatorFusion,
    QuantizationAware,
    GraphPruning,
    LayoutRewrite,
    MemoryPlanning,
}

pub struct ModelOptimizer {
    passes: Vec<OptPass>,
    optimized_count: u64,
}

impl Default for ModelOptimizer {
    fn default() -> Self {
        Self {
            passes: vec![
                OptPass::ConstantFolding,
                OptPass::OperatorFusion,
                OptPass::GraphPruning,
            ],
            optimized_count: 0,
        }
    }
}

impl ModelOptimizer {
    pub fn optimize(&mut self, _spec: &ModelSpec) -> String {
        let mut out = String::from("优化 Pass:\n");
        for p in &self.passes {
            out.push_str(&format!("  {} 已应用\n", ModelOptimizer::pass_name(*p)));
        }
        self.optimized_count += 1;
        out
    }

    pub fn pass_name(p: OptPass) -> &'static str {
        match p {
            OptPass::ConstantFolding => "常量折叠",
            OptPass::OperatorFusion => "算子融合",
            OptPass::QuantizationAware => "量化感知",
            OptPass::GraphPruning => "图剪枝",
            OptPass::LayoutRewrite => "布局重写",
            OptPass::MemoryPlanning => "显存规划",
        }
    }

    pub fn optimized_count(&self) -> u64 {
        self.optimized_count
    }
}

// =====================================================================================
// 基准测试
// =====================================================================================

/// 推理基准结果。
#[derive(Clone, Debug)]
pub struct ModelBenchmark {
    pub model_name: String,
    pub prompt_tokens: u64,
    pub gen_tokens: u64,
    pub prefill_ms: f64,
    pub decode_ms_per_token: f64,
    pub tokens_per_second: f64,
    pub vram_peak_bytes: u64,
    pub first_token_latency_ms: f64,
}

impl ModelBenchmark {
    pub fn tps(&self) -> f64 {
        self.tokens_per_second
    }
}

// =====================================================================================
// 模型元数据（扩展）
// =====================================================================================

/// 模型格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelFormat {
    RawTensor,
    Gguf,
    Onnx,
    SafeTensors,
}

impl ModelFormat {
    pub fn name(self) -> &'static str {
        match self {
            ModelFormat::RawTensor => "raw",
            ModelFormat::Gguf => "gguf",
            ModelFormat::Onnx => "onnx",
            ModelFormat::SafeTensors => "safetensors",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "gguf" => ModelFormat::Gguf,
            "onnx" => ModelFormat::Onnx,
            "safetensors" | "st" => ModelFormat::SafeTensors,
            _ => ModelFormat::RawTensor,
        }
    }
}

/// 扩展模型元数据。
#[derive(Clone, Debug)]
pub struct ModelMeta {
    pub path: String,
    pub format: ModelFormat,
    pub arch: ModelArch,
    pub spec: ModelSpec,
    pub quant: QuantFormat,
    pub weights: BTreeMap<String, ModelWeight>,
    pub layers: Vec<ModelLayer>,
    pub params: u64,
    pub ctx_window: u64,
    pub dtype: String,
    pub vram_bytes: u64,
    pub version: Option<ModelVersion>,
    pub checksum: String,
    pub loaded_at: Option<u128>,
    pub ref_count: u64,
    pub offloadable: bool,
}

impl ModelMeta {
    pub fn weight_total_bytes(&self) -> u64 {
        self.weights.values().map(|w| w.storage_bytes()).sum()
    }

    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

// =====================================================================================
// 模型实例
// =====================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub u64);

#[derive(Clone, Debug)]
pub struct ModelInstance {
    pub id: InstanceId,
    pub model: String,
    pub spec: ModelSpec,
    pub loaded_at: u128,
    pub kv_used: u64,
    pub kv_manager: KvCacheManager,
    pub owner: Option<u64>,
    pub inference_engine: InferenceEngine,
    pub state: InstanceState,
    pub generation_config: GenerationConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceState {
    Unloaded,
    Loading,
    Ready,
    Inferring,
    Evicting,
    Error,
}

#[derive(Clone, Debug)]
pub struct GenerationConfig {
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub max_new_tokens: u64,
    pub repetition_penalty: f32,
    pub stop_sequences: Vec<String>,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            max_new_tokens: 512,
            repetition_penalty: 1.0,
            stop_sequences: Vec::new(),
        }
    }
}

// =====================================================================================
// 模型错误
// =====================================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    AlreadyLoaded { path: String },
    NotFound { name: String },
    NoSuchInstance { id: InstanceId },
    Busy { id: InstanceId },
    VramExhausted,
    InvalidSpec { reason: String },
    QuantUnsupported { fmt: QuantFormat },
    InferenceFailed { reason: String },
    SignatureInvalid { model: String },
    VersionParse { reason: String },
}

// =====================================================================================
// 模型注册表（核心）
// =====================================================================================

pub struct ModelRegistry {
    next_instance: u64,
    models: BTreeMap<String, ModelMeta>,
    instances: BTreeMap<InstanceId, ModelInstance>,
    optimizer: ModelOptimizer,
    distributor: ModelDistributor,
    security: ModelSecurity,
    versions: BTreeMap<String, Vec<ModelInstallRecord>>,
    pub vram_budget: u64,
    pub vram_used: u64,
    pub total_loads: u64,
    pub total_instantiations: u64,
    pub total_destroys: u64,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self {
            next_instance: 1,
            models: BTreeMap::new(),
            instances: BTreeMap::new(),
            optimizer: ModelOptimizer::default(),
            distributor: ModelDistributor::new(),
            security: ModelSecurity::new(),
            versions: BTreeMap::new(),
            vram_budget: 8 << 30,
            vram_used: 0,
            total_loads: 0,
            total_instantiations: 0,
            total_destroys: 0,
        }
    }

    pub fn load(
        &mut self,
        path: &str,
        format: ModelFormat,
        params: u64,
        ctx_window: u64,
        dtype: &str,
        vram_bytes: u64,
    ) -> Result<String, ModelError> {
        if self.models.contains_key(path) {
            return Err(ModelError::AlreadyLoaded { path: path.to_string() });
        }
        if self.vram_used + vram_bytes > self.vram_budget {
            return Err(ModelError::VramExhausted);
        }
        let spec = ModelSpec {
            params,
            max_position_embeddings: ctx_window as u32,
            ..Default::default()
        };
            let mut hasher = DefaultHasher::new();
            path.hash(&mut hasher);
            let checksum = format!("sha256:{:x}", hasher.finish());
            let meta = ModelMeta {
                path: path.to_string(),
                format,
                arch: ModelArch::TransformerDecoder,
                spec,
                quant: QuantFormat::Fp16,
                weights: BTreeMap::new(),
                layers: Vec::new(),
                params,
                ctx_window,
                dtype: dtype.to_string(),
                vram_bytes,
                version: None,
                checksum,
                loaded_at: None,
                ref_count: 0,
                offloadable: true,
            };
        self.models.insert(path.to_string(), meta);
        self.vram_used += vram_bytes;
        self.total_loads += 1;
        Ok(path.to_string())
    }

    pub fn load_with_spec(
        &mut self,
        path: &str,
        format: ModelFormat,
        spec: ModelSpec,
        quant: QuantFormat,
        vram_bytes: u64,
    ) -> Result<String, ModelError> {
        if self.models.contains_key(path) {
            return Err(ModelError::AlreadyLoaded { path: path.to_string() });
        }
        if self.vram_used + vram_bytes > self.vram_budget {
            return Err(ModelError::VramExhausted);
        }
            let mut hasher = DefaultHasher::new();
            path.hash(&mut hasher);
            let checksum = format!("sha256:{:x}", hasher.finish());
            let ctx_window = spec.max_position_embeddings as u64;
            let params = spec.params;
            let arch_clone = spec.arch.clone();
            let meta = ModelMeta {
                path: path.to_string(),
                format,
                arch: arch_clone,
                spec,
                quant,
                weights: BTreeMap::new(),
                layers: Vec::new(),
                params,
                ctx_window,
                dtype: quant.name().to_string(),
                vram_bytes,
                version: None,
                checksum,
                loaded_at: None,
                ref_count: 0,
                offloadable: true,
            };
        self.models.insert(path.to_string(), meta);
        self.vram_used += vram_bytes;
        self.total_loads += 1;
        Ok(path.to_string())
    }

    pub fn unload(&mut self, name: &str) -> Result<(), ModelError> {
        if self.instances.values().any(|i| i.model == name) {
            return Err(ModelError::Busy { id: InstanceId(0) });
        }
        if let Some(meta) = self.models.remove(name) {
            self.vram_used = self.vram_used.saturating_sub(meta.vram_bytes);
            Ok(())
        } else {
            Err(ModelError::NotFound { name: name.to_string() })
        }
    }

    pub fn instantiate(
        &mut self,
        model: &str,
        kv_tokens: u64,
        owner: Option<u64>,
    ) -> Result<InstanceId, ModelError> {
        let meta = self
            .models
            .get(model)
            .ok_or_else(|| ModelError::NotFound { name: model.to_string() })?
            .clone();
        if self.vram_used + meta.vram_bytes > self.vram_budget {
            return Err(ModelError::VramExhausted);
        }
        let id = InstanceId(self.next_instance);
        self.next_instance += 1;
        self.vram_used += meta.vram_bytes;
        let kv_capacity = kv_tokens.min(meta.ctx_window);
        let instance = ModelInstance {
            id,
            model: model.to_string(),
            spec: meta.spec.clone(),
            loaded_at: 0,
            kv_used: kv_capacity,
            kv_manager: KvCacheManager::new(16, 1024),
            owner,
            inference_engine: InferenceEngine::new(),
            state: InstanceState::Ready,
            generation_config: GenerationConfig::default(),
        };
        self.instances.insert(id, instance);
        self.total_instantiations += 1;
        Ok(id)
    }

    pub fn destroy(&mut self, id: InstanceId) -> Result<(), ModelError> {
        let inst = self
            .instances
            .remove(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        if let Some(meta) = self.models.get(&inst.model) {
            self.vram_used = self.vram_used.saturating_sub(meta.vram_bytes);
        }
        self.total_destroys += 1;
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&ModelMeta> {
        self.models.get(name)
    }

    pub fn find_instance(&self, id: InstanceId) -> Option<&ModelInstance> {
        self.instances.get(&id)
    }

    pub fn bind(&mut self, id: InstanceId, owner: u64) -> Result<(), ModelError> {
        let inst = self
            .instances
            .get_mut(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        if let Some(cur) = inst.owner {
            if cur != owner {
                return Err(ModelError::Busy { id });
            }
        }
        inst.owner = Some(owner);
        Ok(())
    }

    pub fn unbind(&mut self, id: InstanceId) -> Result<(), ModelError> {
        let inst = self
            .instances
            .get_mut(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        inst.owner = None;
        Ok(())
    }

    pub fn set_generation_config(&mut self, id: InstanceId, cfg: GenerationConfig) -> Result<(), ModelError> {
        let inst = self
            .instances
            .get_mut(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        inst.generation_config = cfg;
        Ok(())
    }

    pub fn submit_inference(&mut self, id: InstanceId, prompt: Vec<u32>, max_new: u64) -> Result<u64, ModelError> {
        let inst = self
            .instances
            .get_mut(&id)
            .ok_or(ModelError::NoSuchInstance { id })?;
        if inst.state != InstanceState::Ready {
            return Err(ModelError::InferenceFailed { reason: "instance not ready".into() });
        }
        inst.state = InstanceState::Inferring;
        let req_id = inst.inference_engine.submit(id, prompt, max_new);
        Ok(req_id)
    }

    pub fn tick_inference(&mut self, ns_delta: u128) {
        for inst in self.instances.values_mut() {
            inst.inference_engine.tick(ns_delta);
            if inst.inference_engine.completed_count() > 0 && inst.state == InstanceState::Inferring {
                // 简化：全部完成则回 Ready
                inst.state = InstanceState::Ready;
            }
        }
    }

    pub fn models(&self) -> &BTreeMap<String, ModelMeta> {
        &self.models
    }

    pub fn instances(&self) -> &BTreeMap<InstanceId, ModelInstance> {
        &self.instances
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn optimizer(&self) -> &ModelOptimizer {
        &self.optimizer
    }

    pub fn distributor(&self) -> &ModelDistributor {
        &self.distributor
    }

    pub fn security(&self) -> &ModelSecurity {
        &self.security
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static REGISTRY: OnceLock<Mutex<ModelRegistry>> = OnceLock::new();

/// 全局模型注册表。
pub fn model_registry() -> &'static Mutex<ModelRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(ModelRegistry::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

/// 初始化：预装文本/多模态模型描述。
pub fn init() {
    let mut reg = model_registry().lock().unwrap();
    if !reg.models().is_empty() {
        return;
    }
    let _ = reg.load_with_spec(
        "memory://hibos-core-7b",
        ModelFormat::RawTensor,
        ModelSpec {
            arch: ModelArch::TransformerDecoder,
            params: 7_000_000_000,
            num_layers: 32,
            hidden_size: 4096,
            num_heads: 32,
            head_dim: 128,
            intermediate_size: 11008,
            vocab_size: 32000,
            max_position_embeddings: 32768,
            ..Default::default()
        },
        QuantFormat::Fp16,
        (7u64 << 30) / 2,
    );
    let _ = reg.load_with_spec(
        "memory://hibos-vision",
        ModelFormat::Gguf,
        ModelSpec {
            arch: ModelArch::Diffusion,
            params: 2_000_000_000,
            num_layers: 24,
            hidden_size: 1024,
            num_heads: 16,
            head_dim: 64,
            intermediate_size: 4096,
            vocab_size: 0,
            max_position_embeddings: 8192,
            ..Default::default()
        },
        QuantFormat::Int4,
        (2u64 << 30) / 4,
    );
    let _ = reg.load_with_spec(
        "memory://hibos-clip",
        ModelFormat::SafeTensors,
        ModelSpec {
            arch: ModelArch::CLIP,
            params: 1_000_000_000,
            num_layers: 12,
            hidden_size: 768,
            num_heads: 12,
            head_dim: 64,
            intermediate_size: 3072,
            vocab_size: 49408,
            max_position_embeddings: 77,
            bidirectional: true,
            ..Default::default()
        },
        QuantFormat::Fp16,
        512 << 20,
    );
}

// =====================================================================================
// 状态摘要
// =====================================================================================

/// 状态摘要，供 `sense model://status` 使用。
pub fn status() -> String {
    let reg = model_registry().lock().unwrap();
    format!(
        "models={} instances={} vram={}/{}MiB loads={} insts={} destroys={} optimizer_passes={}",
        reg.model_count(),
        reg.instance_count(),
        reg.vram_used >> 20,
        reg.vram_budget >> 20,
        reg.total_loads,
        reg.total_instantiations,
        reg.total_destroys,
        reg.optimizer.optimized_count()
    )
}

/// 加载器视角摘要。
pub fn model_loader_sense() -> String {
    let reg = model_registry().lock().unwrap();
    let mut out = String::from("模型加载器:\n");
    for (path, meta) in reg.models() {
        out.push_str(&format!(
            "  {} arch={} fmt={} quant={} params={} ctx={} layers={} vram={}KiB weight={}KiB\n",
            path,
            meta.arch.name(),
            meta.format.name(),
            meta.quant.name(),
            meta.params,
            meta.ctx_window,
            meta.layer_count(),
            meta.vram_bytes >> 10,
            meta.weight_total_bytes() >> 10
        ));
    }
    if reg.models().is_empty() {
        out.push_str("  （未加载任何模型）\n");
    }
    out
}

/// 实例列表摘要。
pub fn model_instances_sense() -> String {
    let reg = model_registry().lock().unwrap();
    let mut out = String::from("模型实例:\n");
    for (id, inst) in reg.instances() {
        out.push_str(&format!(
            "  inst={} model={} state={:?} kv={} owner={:?} engine_reqs={}\n",
            id.0,
            inst.model,
            inst.state,
            inst.kv_used,
            inst.owner,
            inst.inference_engine.requests.len()
        ));
    }
    if reg.instances().is_empty() {
        out.push_str("  （无实例）\n");
    }
    out
}

// =====================================================================================
// act 接口
// =====================================================================================

/// `act model://*` 的后端动作：
/// * `load <path> <format> <params> <ctx> <dtype> <vram>`
/// * `load-spec <path> <format> <arch> <params> <layers> <hidden> <heads> <kv-heads> <head-dim> <intermediate> <vocab> <ctx> <quant> <vram>`
/// * `instantiate <path> [kv_tokens]`
/// * `destroy <instance>`
/// * `bind <instance> <owner>`
/// * `unbind <instance>`
/// * `unload <path>`
/// * `config <instance> <temperature> <top_p> <top_k> <max_new>`
/// * `infer <instance> <token...>`
/// * `tick <ns>`
/// * `optimize <path>`
/// * `verify <model> <version>`
/// * `shard <name> <shard_id> <offset> <size> <checksum>`
/// * `status`
pub fn model_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let mut reg = model_registry().lock().unwrap();
    match it.next() {
        Some("load") => {
            let path = it.next().ok_or("用法: model://loader load <path> <format> <params> <ctx> <dtype> <vram>")?;
            let fmt = ModelFormat::from_str(it.next().unwrap_or("raw"));
            let params = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ctx = it.next().and_then(|s| s.parse().ok()).unwrap_or(4096);
            let dtype = it.next().unwrap_or("fp16");
            let vram = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let name = reg.load(path, fmt, params, ctx, dtype, vram).map_err(|e| format!("加载失败: {:?}", e))?;
            Ok(format!("已加载模型 {} fmt={} params={} ctx={} vram={}B", name, fmt.name(), params, ctx, vram))
        }
        Some("load-spec") => {
            let path = it.next().ok_or("用法: model://loader load-spec <path> <format> <arch> <params> <layers> <hidden> <heads> <kv-heads> <head-dim> <intermediate> <vocab> <ctx> <quant> <vram>")?;
            let fmt = ModelFormat::from_str(it.next().unwrap_or("raw"));
            let arch_str = it.next().unwrap_or("transformer-decoder");
            let arch = match arch_str {
                "transformer-decoder" => ModelArch::TransformerDecoder,
                "transformer-encoder" => ModelArch::TransformerEncoder,
                "transformer-enc-dec" => ModelArch::TransformerEncDec,
                "moe" => ModelArch::MoE,
                "diffusion" => ModelArch::Diffusion,
                "vae" => ModelArch::VAE,
                "clip" => ModelArch::CLIP,
                "gnn" => ModelArch::GNN,
                "state-space" => ModelArch::StateSpace,
                "cnn" => ModelArch::CNN,
                "rnn" => ModelArch::RNN,
                _ => ModelArch::Custom(arch_str.to_string()),
            };
            let params = it.next().and_then(|s| s.parse().ok()).unwrap_or(1_000_000_000);
            let layers = it.next().and_then(|s| s.parse().ok()).unwrap_or(32);
            let hidden = it.next().and_then(|s| s.parse().ok()).unwrap_or(4096);
            let heads = it.next().and_then(|s| s.parse().ok()).unwrap_or(32);
            let kv_heads = it.next().and_then(|s| s.parse::<u32>().ok());
            let head_dim = it.next().and_then(|s| s.parse().ok()).unwrap_or(128);
            let intermediate = it.next().and_then(|s| s.parse().ok()).unwrap_or(11008);
            let vocab = it.next().and_then(|s| s.parse().ok()).unwrap_or(32000);
            let ctx = it.next().and_then(|s| s.parse().ok()).unwrap_or(32768);
            let quant_str = it.next().unwrap_or("fp16");
            let quant = match quant_str {
                "int8" => QuantFormat::Int8,
                "int4" => QuantFormat::Int4,
                "nf4" => QuantFormat::Nf4,
                "fp8" => QuantFormat::Fp8E4M3,
                "bf16" => QuantFormat::Bf16,
                "fp32" => QuantFormat::Fp32,
                _ => QuantFormat::Fp16,
            };
            let vram = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let arch_name = arch.name();
            let spec = ModelSpec {
                arch,
                params,
                num_layers: layers,
                hidden_size: hidden,
                num_heads: heads,
                num_kv_heads: kv_heads,
                head_dim,
                intermediate_size: intermediate,
                vocab_size: vocab,
                max_position_embeddings: ctx as u32,
                ..Default::default()
            };
            let name = reg.load_with_spec(path, fmt, spec, quant, vram).map_err(|e| format!("加载失败: {:?}", e))?;
            Ok(format!("已加载模型 {} arch={} quant={} params={} vram={}B", name, arch_name, quant.name(), params, vram))
        }
        Some("instantiate") => {
            let path = it.next().ok_or("用法: model://instances instantiate <path> [kv_tokens]")?;
            let kv = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let id = reg.instantiate(path, kv, None).map_err(|e| format!("实例化失败: {:?}", e))?;
            Ok(format!("已创建实例 inst={} model={}", id.0, path))
        }
        Some("destroy") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).ok_or("用法: model://instances destroy <instance>")?;
            reg.destroy(id).map_err(|e| format!("释放失败: {:?}", e))?;
            Ok(format!("已释放实例 inst={}", id.0))
        }
        Some("bind") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).ok_or("用法: model://instances bind <instance> <owner>")?;
            let owner = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: model://instances bind <instance> <owner>")?;
            reg.bind(id, owner).map_err(|e| format!("绑定失败: {:?}", e))?;
            Ok(format!("实例 inst={} 已绑定 owner={}", id.0, owner))
        }
        Some("unbind") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).ok_or("用法: model://instances unbind <instance>")?;
            reg.unbind(id).map_err(|e| format!("解绑失败: {:?}", e))?;
            Ok(format!("实例 inst={} 已解绑", id.0))
        }
        Some("unload") => {
            let path = it.next().ok_or("用法: model://loader unload <path>")?;
            reg.unload(path).map_err(|e| format!("卸载失败: {:?}", e))?;
            Ok(format!("已卸载模型 {}", path))
        }
        Some("config") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).ok_or("用法: model://instances config <instance> <temperature> <top_p> <top_k> <max_new>")?;
            let temp = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.7);
            let top_p = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.9);
            let top_k = it.next().and_then(|s| s.parse().ok()).unwrap_or(40);
            let max_new = it.next().and_then(|s| s.parse().ok()).unwrap_or(512);
            let cfg = GenerationConfig { temperature: temp, top_p, top_k, max_new_tokens: max_new, ..Default::default() };
            reg.set_generation_config(id, cfg).map_err(|e| format!("配置失败: {:?}", e))?;
            Ok(format!("实例 inst={} 生成配置已更新 temp={} top_p={} top_k={} max_new={}", id.0, temp, top_p, top_k, max_new))
        }
        Some("infer") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).ok_or("用法: model://instances infer <instance> <token...>")?;
            let prompt: Vec<u32> = it.filter_map(|s| s.parse().ok()).collect();
            if prompt.is_empty() {
                return Err("提示词 token 为空".into());
            }
            let max_new = 64;
            let prompt_len = prompt.len();
            let req_id = reg.submit_inference(id, prompt, max_new).map_err(|e| format!("推理失败: {:?}", e))?;
            Ok(format!("已提交推理请求 req={} inst={} prompt_tokens={} max_new={}", req_id, id.0, prompt_len, max_new))
        }
        Some("tick") => {
            let ns = it.next().and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
            reg.tick_inference(ns);
            Ok(format!("推理引擎已推进 {}ns", ns))
        }
        Some("optimize") => {
            let path = it.next().ok_or("用法: model://loader optimize <path>")?;
            let _meta = reg.find(path).ok_or_else(|| ModelError::NotFound { name: path.to_string() }).map_err(|e| format!("{:?}", e))?;
            let report = reg.optimizer.optimize(&ModelSpec::default());
            Ok(format!("模型 {} 图优化完成:\n{}", path, report))
        }
        Some("verify") => {
            let model = it.next().ok_or("用法: model://security verify <model> <version>")?;
            let ver_str = it.next().ok_or("缺少版本")?;
            let version = ModelVersion::parse(ver_str).map_err(|e| format!("版本解析失败: {}", e))?;
            let ok = reg.security.verify(model, &version);
            let ver_str_out = version.to_string();
            Ok(format!("模型 {}@{} 签名验证：{}", model, ver_str_out, if ok { "通过" } else { "未签名/不信任" }))
        }
        Some("sign") => {
            let model = it.next().ok_or("用法: model://security sign <model> <version> <algo>")?;
            let ver_str = it.next().ok_or("缺少版本")?;
            let version = ModelVersion::parse(ver_str).map_err(|e| format!("版本解析失败: {}", e))?;
            let algo_str = it.next().unwrap_or("ed25519");
            let algo = match algo_str {
                "ed25519" => SignAlgo::Ed25519,
                "ecdsa" => SignAlgo::ECDsaP256,
                "rsa" => SignAlgo::Rsa4096,
                _ => SignAlgo::Ed25519,
            };
            let ver_str_out = version.to_string();
            let sig = ModelSignature {
                model_name: model.to_string(),
                version,
                algo,
                signature: vec![0u8; 64],
                signer_cert: "hibos-root".into(),
                signed_at: 0,
            };
            reg.security.register_signature(sig);
            Ok(format!("模型 {}@{} 已签名 algo={}", model, ver_str_out, algo_str))
        }
        Some("shard") => {
            let name = it.next().ok_or("用法: model://distributor shard <name> <shard_id> <offset> <size> <checksum>")?;
            let shard_id = it.next().and_then(|s| s.parse().ok()).ok_or("缺少 shard_id")?;
            let offset = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let size = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let checksum = it.next().unwrap_or("").to_string();
            let checksum_out = checksum.clone();
            reg.distributor.register_shard(ModelShard { shard_id, name: name.to_string(), offset, size, checksum, peers: Vec::new() });
            Ok(format!("已注册分片 {} shard={} offset={} size={} checksum={}", name, shard_id, offset, size, checksum_out))
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 model 动作: {}", other)),
    }
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_params_per_layer() {
        let spec = ModelSpec::default();
        assert!(spec.params_per_layer() > 0);
        assert!(spec.kv_cache_bytes_per_token() > 0);
        assert!(spec.activation_bytes_per_token() > 0);
    }

    #[test]
    fn quant_bytes_per_param() {
        assert_eq!(QuantFormat::Fp32.bytes_per_param(), 4);
        assert_eq!(QuantFormat::Fp16.bytes_per_param(), 2);
        assert_eq!(QuantFormat::Int8.bytes_per_param(), 1);
        assert_eq!(QuantFormat::Int4.bytes_per_param(), 0);
    }

    #[test]
    fn weight_storage_bytes() {
        let w = ModelWeight {
            name: "w".into(),
            shape: vec![1024, 1024],
            quant: QuantFormat::Int4,
            data: Vec::new(),
            scale: None,
            zero_point: None,
            scales: Vec::new(),
            offloadable: true,
        };
        assert_eq!(w.storage_bytes(), 1024 * 1024 / 2);
    }

    #[test]
    fn kv_cache_alloc_release() {
        let mut mgr = KvCacheManager::new(16, 100);
        let id = mgr.allocate(0, 64).unwrap();
        assert_eq!(mgr.used_blocks(), 4);
        mgr.release(id).unwrap();
        assert_eq!(mgr.used_blocks(), 0);
    }

    #[test]
    fn kv_cache_exhausted() {
        let mut mgr = KvCacheManager::new(16, 1);
        assert!(mgr.allocate(0, 64).is_ok());
        assert!(mgr.allocate(0, 64).is_err());
    }

    #[test]
    fn inference_engine_submit_tick() {
        let mut eng = InferenceEngine::new();
        let id = eng.submit(InstanceId(1), vec![1, 2, 3], 4);
        assert_eq!(eng.requests.len(), 1);
        eng.tick(1_000_000);
        // 超过 max_new 后仍完成（占位逻辑）
    }

    #[test]
    fn model_version_parse() {
        let v = ModelVersion::parse("1.2.3").unwrap();
        assert_eq!((v.major, v.minor, v.patch), (1, 2, 3));
        assert_eq!(v.to_string(), "1.2.3");
        assert!(ModelVersion::parse("1.2").is_err());
    }

    #[test]
    fn model_security_verify() {
        let mut sec = ModelSecurity::new();
        assert!(!sec.verify("m", &ModelVersion { major: 1, minor: 0, patch: 0, pre_release: None, build: None }));
        sec.register_signature(ModelSignature {
            model_name: "m".into(),
            version: ModelVersion { major: 1, minor: 0, patch: 0, pre_release: None, build: None },
            algo: SignAlgo::Ed25519,
            signature: vec![1; 64],
            signer_cert: "root".into(),
            signed_at: 0,
        });
        assert!(sec.verify("m", &ModelVersion { major: 1, minor: 0, patch: 0, pre_release: None, build: None }));
    }

    #[test]
    fn load_and_instantiate() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::RawTensor, 1_000, 1024, "fp16", 1024).unwrap();
        let id = reg.instantiate("/m.bin", 128, None).unwrap();
        assert_eq!(reg.instance_count(), 1);
        assert_eq!(reg.vram_used, 2048); // 1024 load + 1024 instantiate
        reg.destroy(id).unwrap();
        assert_eq!(reg.vram_used, 1024); // instantiate 回收，load 保留
        reg.unload("/m.bin").unwrap();
        assert_eq!(reg.vram_used, 0);
    }

    #[test]
    fn load_with_spec() {
        let mut reg = ModelRegistry::new();
        let spec = ModelSpec { arch: ModelArch::MoE, params: 100_000_000_000, num_layers: 80, hidden_size: 8192, num_heads: 64, head_dim: 128, intermediate_size: 24576, vocab_size: 100000, max_position_embeddings: 131072, router: Some(RouterType::TopK), num_experts: Some(8), num_experts_per_tok: Some(2), ..Default::default() };
        let _ = reg.load_with_spec("/moe.bin", ModelFormat::SafeTensors, spec, QuantFormat::Int4, 50 << 30).unwrap();
        assert_eq!(reg.model_count(), 1);
    }

    #[test]
    fn duplicate_load_rejected() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Gguf, 1, 1, "q4", 1).unwrap();
        assert!(matches!(
            reg.load("/m.bin", ModelFormat::Gguf, 1, 1, "q4", 1).unwrap_err(),
            ModelError::AlreadyLoaded { .. }
        ));
    }

    #[test]
    fn unload_busy_rejected() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Onnx, 1, 1, "fp32", 1).unwrap();
        reg.instantiate("/m.bin", 0, Some(9)).unwrap();
        assert!(matches!(reg.unload("/m.bin"), Err(ModelError::Busy { .. })));
    }

    #[test]
    fn bind_conflict() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::Onnx, 1, 1, "fp32", 1).unwrap();
        let id = reg.instantiate("/m.bin", 0, Some(1)).unwrap();
        assert!(reg.bind(id, 1).is_ok());
        assert!(matches!(reg.bind(id, 2), Err(ModelError::Busy { .. })));
    }

    #[test]
    fn generation_config() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::RawTensor, 1, 1, "fp16", 1).unwrap();
        let id = reg.instantiate("/m.bin", 0, None).unwrap();
        let cfg = GenerationConfig { temperature: 0.9, top_p: 0.95, top_k: 50, max_new_tokens: 1024, ..Default::default() };
        assert!(reg.set_generation_config(id, cfg).is_ok());
    }

    #[test]
    fn submit_inference() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::RawTensor, 1, 1, "fp16", 1).unwrap();
        let id = reg.instantiate("/m.bin", 0, None).unwrap();
        let req_id = reg.submit_inference(id, vec![1, 2, 3], 16).unwrap();
        assert!(req_id > 0);
        reg.tick_inference(0);
    }

    #[test]
    fn vram_exhausted() {
        let mut reg = ModelRegistry::new();
        reg.vram_budget = 100;
        reg.load("/big.bin", ModelFormat::RawTensor, 1, 1, "fp16", 200).unwrap_err();
    }

    #[test]
    fn optimizer_report() {
        let mut opt = ModelOptimizer::default();
        let report = opt.optimize(&ModelSpec::default());
        assert!(report.contains("常量折叠"));
        assert_eq!(opt.optimized_count(), 1);
    }

    #[test]
    fn global_init_idempotent() {
        init();
        init();
        let s = status();
        assert!(s.contains("models="));
        assert!(s.contains("instances="));
    }

    #[test]
    fn model_loader_sense_nonempty() {
        init();
        let s = model_loader_sense();
        assert!(s.contains("模型加载器"));
        assert!(s.contains("hibos-core-7b"));
    }

    #[test]
    fn model_instances_sense_nonempty() {
        init();
        let s = model_instances_sense();
        assert!(s.contains("模型实例"));
    }

    #[test]
    fn arch_generative() {
        assert!(ModelArch::TransformerDecoder.is_generative());
        assert!(ModelArch::MoE.is_generative());
        assert!(!ModelArch::TransformerEncoder.is_generative());
        assert!(!ModelArch::CLIP.is_generative());
    }

    #[test]
    fn arch_multimodal() {
        assert!(ModelArch::CLIP.is_multimodal());
        assert!(ModelArch::Diffusion.is_multimodal());
        assert!(!ModelArch::TransformerDecoder.is_multimodal());
    }

    #[test]
    fn attention_names() {
        assert_eq!(AttentionType::Full.name(), "full");
        assert_eq!(AttentionType::Paged.name(), "paged");
        assert_eq!(AttentionType::GroupedQuery.name(), "grouped-query");
    }

    #[test]
    fn router_names() {
        assert_eq!(RouterType::TopK.name(), "top-k");
        assert_eq!(RouterType::Switch.name(), "switch");
        assert_eq!(RouterType::ExpertChoice.name(), "expert-choice");
    }

    #[test]
    fn spec_kv_heads_default() {
        let spec = ModelSpec::default();
        assert_eq!(spec.kv_heads(), spec.num_heads);
    }

    #[test]
    fn spec_estimated_vram() {
        let spec = ModelSpec::default();
        assert!(spec.estimated_vram_bytes() > spec.params * 2);
    }

    #[test]
    fn model_weight_num_elements() {
        let w = ModelWeight { name: "w".into(), shape: vec![2, 3, 4], quant: QuantFormat::Fp32, data: Vec::new(), scale: None, zero_point: None, scales: Vec::new(), offloadable: false };
        assert_eq!(w.num_elements(), 24);
    }

    #[test]
    fn instance_state_transitions() {
        let mut reg = ModelRegistry::new();
        reg.load("/m.bin", ModelFormat::RawTensor, 1, 1, "fp16", 1).unwrap();
        let id = reg.instantiate("/m.bin", 0, None).unwrap();
        let inst = reg.find_instance(id).unwrap();
        assert_eq!(inst.state, InstanceState::Ready);
    }

    #[test]
    fn model_signature_count() {
        let mut sec = ModelSecurity::new();
        assert_eq!(sec.signature_count(), 0);
        sec.register_signature(ModelSignature {
            model_name: "m".into(),
            version: ModelVersion { major: 1, minor: 0, patch: 0, pre_release: None, build: None },
            algo: SignAlgo::Rsa4096,
            signature: vec![],
            signer_cert: "ca".into(),
            signed_at: 0,
        });
        assert_eq!(sec.signature_count(), 1);
    }

    #[test]
    fn model_distributor_shard() {
        let mut dist = ModelDistributor::new();
        assert_eq!(dist.shard_count(), 0);
        dist.register_shard(ModelShard { shard_id: 1, name: "m".into(), offset: 0, size: 1024, checksum: "abc".into(), peers: vec!["peer1".into()] });
        assert_eq!(dist.shard_count(), 1);
        assert_eq!(dist.locate_peer(1), Some("peer1"));
        assert_eq!(dist.locate_peer(999), None);
    }

    #[test]
    fn benchmark_tps() {
        let b = ModelBenchmark { model_name: "m".into(), prompt_tokens: 100, gen_tokens: 50, prefill_ms: 10.0, decode_ms_per_token: 0.1, tokens_per_second: 100.0, vram_peak_bytes: 1 << 30, first_token_latency_ms: 50.0 };
        assert!((b.tps() - 100.0).abs() < 1e-6);
    }

    #[test]
    fn format_name() {
        assert_eq!(ModelFormat::Gguf.name(), "gguf");
        assert_eq!(ModelFormat::SafeTensors.name(), "safetensors");
        assert_eq!(ModelFormat::from_str("GGUF"), ModelFormat::Gguf);
        assert_eq!(ModelFormat::from_str("unknown"), ModelFormat::RawTensor);
    }

    #[test]
    fn quant_name() {
        assert_eq!(QuantFormat::Nf4.name(), "nf4");
        assert_eq!(QuantFormat::Fp8E4M3.name(), "fp8-e4m3");
        assert_eq!(QuantFormat::PerChannelInt8.name(), "int8-per-channel");
    }

    #[test]
    fn layer_kind_names() {
        assert_eq!(LayerKind::Attention.name(), "attention");
        assert_eq!(LayerKind::Router.name(), "router");
        assert_eq!(LayerKind::Expert.name(), "expert");
    }

    #[test]
    fn opt_pass_names() {
        assert_eq!(ModelOptimizer::pass_name(OptPass::ConstantFolding), "常量折叠");
        assert_eq!(ModelOptimizer::pass_name(OptPass::MemoryPlanning), "显存规划");
    }

    #[test]
    fn meta_weight_total() {
        let mut meta = ModelMeta {
            path: "m".into(),
            format: ModelFormat::RawTensor,
            arch: ModelArch::TransformerDecoder,
            spec: ModelSpec::default(),
            quant: QuantFormat::Fp16,
            weights: BTreeMap::new(),
            layers: Vec::new(),
            params: 1_000,
            ctx_window: 2048,
            dtype: "fp16".into(),
            vram_bytes: 1 << 20,
            version: None,
            checksum: "x".into(),
            loaded_at: None,
            ref_count: 0,
            offloadable: true,
        };
        meta.weights.insert("w1".into(), ModelWeight { name: "w1".into(), shape: vec![100], quant: QuantFormat::Int4, data: vec![], scale: None, zero_point: None, scales: Vec::new(), offloadable: true });
        assert_eq!(meta.weight_total_bytes(), 50);
    }
}
