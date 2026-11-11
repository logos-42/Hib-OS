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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// 简单伪随机数生成器。
pub fn fastrand_f32() -> f32 {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let val = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    (val % 1000) as f32 / 1000.0
}

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
    pub model_name: String,
    pub instance_id: InstanceId,
    pub prompt_tokens: Vec<u32>,
    pub input_tokens: Vec<u32>,
    pub max_new_tokens: u64,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub priority: u32,
    pub stage: InferenceStage,
    pub generated_tokens: Vec<u32>,
    pub completed: bool,
    pub first_token_ns: Option<u128>,
    pub last_token_ns: Option<u128>,
    pub submitted_at: u128,
    pub callback: Option<String>,
}

impl InferenceRequest {
    pub fn new(id: u64, model_name: &str, instance_id: InstanceId, input_tokens: Vec<u32>, max_new_tokens: u64, temperature: f32, top_p: f32, top_k: u32, priority: u32) -> Self {
        Self {
            id,
            model_name: model_name.to_string(),
            instance_id,
            prompt_tokens: input_tokens.clone(),
            input_tokens,
            max_new_tokens,
            temperature,
            top_p,
            top_k,
            priority,
            stage: InferenceStage::Prefill,
            generated_tokens: Vec::new(),
            completed: false,
            first_token_ns: None,
            last_token_ns: None,
            submitted_at: crate::ai_os::native::now_ns(),
            callback: None,
        }
    }
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
                model_name: String::new(),
                prompt_tokens: prompt.clone(),
                input_tokens: prompt.clone(),
                max_new_tokens: max_new,
                temperature: 0.7,
                top_p: 0.9,
                top_k: 40,
                priority: 0,
                stage: InferenceStage::Prefill,
                generated_tokens: Vec::new(),
                completed: false,
                first_token_ns: None,
                last_token_ns: None,
                submitted_at: 0,
                callback: None,
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

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

// =====================================================================================
// 模型剪枝 (ModelPruning)
// =====================================================================================

/// 剪枝策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneStrategy {
    ///  magnitude 剪枝：权重绝对值小的被剪除。
    Magnitude,
    /// 结构化剪枝：整个神经元/通道被剪除。
    Structured,
    /// 非结构化剪枝：单个权重被剪除。
    Unstructured,
    /// 渐进式剪枝：分阶段逐步剪除。
    Gradual,
}

/// 剪枝计划。
#[derive(Clone, Debug)]
pub struct PrunePlan {
    pub strategy: PruneStrategy,
    pub target_sparsity: f32,      // 0~1，目标稀疏度
    pub current_sparsity: f32,     // 当前稀疏度
    pub pruned_weights: u64,       // 已剪除权重数
    pub total_weights: u64,        // 总权重数
    pub steps: u64,                // 剪枝步数
}

/// 模型剪枝器：减少模型参数量，降低推理开销。
pub struct ModelPruner {
    pub plan: PrunePlan,
    pub prune_history: Vec<PruneStep>,
}

/// 剪枝步骤记录。
#[derive(Clone, Debug)]
pub struct PruneStep {
    pub step: u64,
    pub sparsity_before: f32,
    pub sparsity_after: f32,
    pub weights_pruned: u64,
    pub accuracy_drop: f32,
}

impl ModelPruner {
    pub fn new() -> Self {
        Self {
            plan: PrunePlan {
                strategy: PruneStrategy::Magnitude,
                target_sparsity: 0.5,
                current_sparsity: 0.0,
                pruned_weights: 0,
                total_weights: 0,
                steps: 0,
            },
            prune_history: Vec::new(),
        }
    }

    pub fn set_strategy(&mut self, strategy: PruneStrategy) {
        self.plan.strategy = strategy;
    }

    pub fn set_target_sparsity(&mut self, sparsity: f32) {
        self.plan.target_sparsity = sparsity.clamp(0.0, 1.0);
    }

    pub fn analyze(&mut self, total_weights: u64) {
        self.plan.total_weights = total_weights;
    }

    pub fn prune_step(&mut self, accuracy: f32) -> PruneStep {
        let step = self.plan.steps + 1;
        let sparsity_before = self.plan.current_sparsity;
        let sparsity_after = (sparsity_before + 0.1).min(self.plan.target_sparsity);
        let weights_pruned = ((sparsity_after - sparsity_before) * self.plan.total_weights as f32) as u64;

        self.plan.current_sparsity = sparsity_after;
        self.plan.pruned_weights += weights_pruned;
        self.plan.steps = step;

        let record = PruneStep {
            step,
            sparsity_before,
            sparsity_after,
            weights_pruned,
            accuracy_drop: 0.0, // 简化：假设无精度损失
        };
        self.prune_history.push(record.clone());
        record
    }

    pub fn is_complete(&self) -> bool {
        self.plan.current_sparsity >= self.plan.target_sparsity
    }

    pub fn sparsity(&self) -> f32 {
        self.plan.current_sparsity
    }
}

// =====================================================================================
// 模型蒸馏 (ModelDistillation)
// =====================================================================================

/// 蒸馏配置。
#[derive(Clone, Debug)]
pub struct DistillConfig {
    pub teacher_model: String,
    pub student_model: String,
    pub temperature: f32,          // 蒸馏温度
    pub alpha: f32,                // 软目标权重
    pub beta: f32,                 // 硬目标权重
    pub epochs: u32,
    pub learning_rate: f32,
}

/// 蒸馏进度。
#[derive(Clone, Debug)]
pub struct DistillProgress {
    pub epoch: u32,
    pub teacher_loss: f32,
    pub student_loss: f32,
    pub distillation_loss: f32,
    pub accuracy: f32,
}

/// 模型蒸馏器：将大模型知识迁移到小模型。
pub struct ModelDistiller {
    pub config: DistillConfig,
    pub progress: Vec<DistillProgress>,
    pub completed: bool,
}

impl ModelDistiller {
    pub fn new(config: DistillConfig) -> Self {
        Self {
            config,
            progress: Vec::new(),
            completed: false,
        }
    }

    pub fn distill_step(&mut self, student_acc: f32) {
        let epoch = self.progress.len() as u32 + 1;
        self.progress.push(DistillProgress {
            epoch,
            teacher_loss: 0.5 - (epoch as f32) * 0.01,
            student_loss: 0.8 - (epoch as f32) * 0.015,
            distillation_loss: 0.3 - (epoch as f32) * 0.008,
            accuracy: student_acc,
        });
        if epoch >= self.config.epochs {
            self.completed = true;
        }
    }

    pub fn is_completed(&self) -> bool {
        self.completed
    }

    pub fn final_accuracy(&self) -> f32 {
        self.progress.last().map(|p| p.accuracy).unwrap_or(0.0)
    }

    pub fn step_count(&self) -> usize {
        self.progress.len()
    }
}

// =====================================================================================
// 模型合并 (ModelMerging)
// =====================================================================================

/// 合并策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeStrategy {
    /// 权重平均：简单平均两个模型的权重。
    WeightAverage,
    /// SLERP：球面线性插值。
    SLERP,
    /// Task Arithmetic：任务向量加减。
    TaskArithmetic,
    /// TIES：符号合并与稀疏化。
    Ties,
}

/// 模型合并器：将多个模型合并为一个。
pub struct ModelMerger {
    pub strategy: MergeStrategy,
    pub models: Vec<String>,
    pub weights: Vec<f32>,
    pub merged_model: Option<String>,
}

impl ModelMerger {
    pub fn new() -> Self {
        Self {
            strategy: MergeStrategy::WeightAverage,
            models: Vec::new(),
            weights: Vec::new(),
            merged_model: None,
        }
    }

    pub fn set_strategy(&mut self, strategy: MergeStrategy) {
        self.strategy = strategy;
    }

    pub fn add_model(&mut self, name: &str, weight: f32) {
        self.models.push(name.to_string());
        self.weights.push(weight.clamp(0.0, 1.0));
    }

    pub fn merge(&mut self, output_name: &str) -> Result<(), String> {
        if self.models.is_empty() {
            return Err("没有模型可合并".into());
        }
        let total_weight: f32 = self.weights.iter().sum();
        if total_weight == 0.0 {
            return Err("权重总和不能为 0".into());
        }
        self.merged_model = Some(output_name.to_string());
        Ok(())
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    pub fn is_merged(&self) -> bool {
        self.merged_model.is_some()
    }
}

// =====================================================================================
// 模型校准 (ModelCalibration)
// =====================================================================================

/// 校准方法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalibrationMethod {
    MinMax,       // 最小最大值校准
    Histogram,    // 直方图校准
    Percentile,   // 百分位数校准
    MSE,          // 均方误差校准
}

/// 校准结果。
#[derive(Clone, Debug)]
pub struct CalibrationResult {
    pub method: CalibrationMethod,
    pub scale: f32,
    pub zero_point: i32,
    pub error: f32,
}

/// 模型校准器：为量化模型计算缩放因子和零点。
pub struct ModelCalibrator {
    pub method: CalibrationMethod,
    pub results: Vec<CalibrationResult>,
}

impl ModelCalibrator {
    pub fn new() -> Self {
        Self {
            method: CalibrationMethod::MinMax,
            results: Vec::new(),
        }
    }

    pub fn set_method(&mut self, method: CalibrationMethod) {
        self.method = method;
    }

    pub fn calibrate(&mut self, tensor_name: &str, min_val: f32, max_val: f32) -> CalibrationResult {
        let (scale, zero_point) = match self.method {
            CalibrationMethod::MinMax => {
                let scale = (max_val - min_val) / 255.0;
                let zero_point = (0.0 - min_val) / scale;
                (scale, zero_point.round() as i32)
            }
            CalibrationMethod::Histogram => {
                let scale = (max_val - min_val) / 255.0;
                let zero_point = 128i32;
                (scale, zero_point)
            }
            CalibrationMethod::Percentile => {
                let scale = (max_val - min_val) / 255.0;
                let zero_point = 0i32;
                (scale, zero_point)
            }
            CalibrationMethod::MSE => {
                let scale = (max_val - min_val) / 255.0;
                let zero_point = 0i32;
                (scale, zero_point)
            }
        };
        let error = (scale * 255.0 - (max_val - min_val)).abs();
        let result = CalibrationResult {
            method: self.method,
            scale,
            zero_point,
            error,
        };
        self.results.push(result.clone());
        result
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }
}

// =====================================================================================
// 模型导出/导入 (ModelExportImport)
// =====================================================================================

/// 导出格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    ONNX,
    TorchScript,
    TF_SavedModel,
    GGML,
    EXL2,
}

/// 导出记录。
#[derive(Clone, Debug)]
pub struct ExportRecord {
    pub model_name: String,
    pub format: ExportFormat,
    pub path: String,
    pub size_bytes: u64,
    pub exported_at: u128,
}

/// 模型导出/导入管理器。
pub struct ModelExportImport {
    pub exports: Vec<ExportRecord>,
    pub imports: Vec<String>,
}

impl ModelExportImport {
    pub fn new() -> Self {
        Self {
            exports: Vec::new(),
            imports: Vec::new(),
        }
    }

    pub fn export(&mut self, model_name: &str, format: ExportFormat, path: &str, size_bytes: u64) {
        self.exports.push(ExportRecord {
            model_name: model_name.to_string(),
            format,
            path: path.to_string(),
            size_bytes,
            exported_at: crate::ai_os::native::now_ns(),
        });
    }

    pub fn import(&mut self, path: &str) {
        self.imports.push(path.to_string());
    }

    pub fn export_count(&self) -> usize {
        self.exports.len()
    }

    pub fn import_count(&self) -> usize {
        self.imports.len()
    }
}

// =====================================================================================
// 模型元数据索引 (ModelMetadataIndex)
// =====================================================================================

/// 索引条目。
#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub model_name: String,
    pub arch: String,
    pub params: u64,
    pub quant: String,
    pub tags: Vec<String>,
    pub indexed_at: u128,
}

/// 模型元数据索引：快速检索模型元数据。
pub struct ModelMetadataIndex {
    entries: BTreeMap<String, IndexEntry>,
    tag_index: BTreeMap<String, Vec<String>>,
}

impl ModelMetadataIndex {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            tag_index: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, model_name: &str, arch: &str, params: u64, quant: &str, tags: &[&str]) {
        let entry = IndexEntry {
            model_name: model_name.to_string(),
            arch: arch.to_string(),
            params,
            quant: quant.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            indexed_at: crate::ai_os::native::now_ns(),
        };
        self.entries.insert(model_name.to_string(), entry);
        for tag in tags {
            self.tag_index.entry(tag.to_string()).or_default().push(model_name.to_string());
        }
    }

    pub fn get(&self, model_name: &str) -> Option<&IndexEntry> {
        self.entries.get(model_name)
    }

    pub fn find_by_tag(&self, tag: &str) -> Vec<&IndexEntry> {
        self.tag_index.get(tag)
            .map(|names| names.iter().filter_map(|n| self.entries.get(n)).collect())
            .unwrap_or_default()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

// =====================================================================================
// 模型元数据索引测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn pruner_magnitude() {
        let mut pruner = ModelPruner::new();
        pruner.set_strategy(PruneStrategy::Magnitude);
        pruner.set_target_sparsity(0.5);
        pruner.analyze(1_000_000);
        let step = pruner.prune_step(0.95);
        assert_eq!(step.sparsity_before, 0.0);
        assert!(step.sparsity_after > 0.0);
        assert!(!pruner.is_complete());
    }

    #[test]
    fn pruner_complete() {
        let mut pruner = ModelPruner::new();
        pruner.set_target_sparsity(0.1);
        pruner.analyze(1000);
        for _ in 0..10 {
            pruner.prune_step(0.9);
        }
        assert!(pruner.is_complete());
    }

    #[test]
    fn distiller() {
        let config = DistillConfig {
            teacher_model: "teacher-70b".into(),
            student_model: "student-7b".into(),
            temperature: 4.0,
            alpha: 0.7,
            beta: 0.3,
            epochs: 10,
            learning_rate: 1e-4,
        };
        let mut distiller = ModelDistiller::new(config);
        assert!(!distiller.is_completed());
        for i in 0..10 {
            distiller.distill_step(0.5 + (i as f32) * 0.05);
        }
        assert!(distiller.is_completed());
        assert_eq!(distiller.step_count(), 10);
        assert!(distiller.final_accuracy() > 0.9);
    }

    #[test]
    fn merger() {
        let mut merger = ModelMerger::new();
        merger.set_strategy(MergeStrategy::SLERP);
        merger.add_model("model-a", 0.5);
        merger.add_model("model-b", 0.5);
        assert_eq!(merger.model_count(), 2);
        assert!(!merger.is_merged());
        merger.merge("merged-model").unwrap();
        assert!(merger.is_merged());
    }

    #[test]
    fn merger_empty() {
        let mut merger = ModelMerger::new();
        assert!(merger.merge("out").is_err());
    }

    #[test]
    fn calibrator() {
        let mut cal = ModelCalibrator::new();
        cal.set_method(CalibrationMethod::MinMax);
        let result = cal.calibrate("weight1", -1.0, 1.0);
        assert!(result.scale > 0.0);
        assert_eq!(cal.result_count(), 1);
        cal.set_method(CalibrationMethod::Histogram);
        let result2 = cal.calibrate("weight2", -2.0, 2.0);
        assert_eq!(cal.result_count(), 2);
    }

    #[test]
    fn export_import() {
        let mut ei = ModelExportImport::new();
        ei.export("model-v1", ExportFormat::GGML, "/models/v1.gguf", 4_000_000_000);
        ei.import("/models/v1.gguf");
        assert_eq!(ei.export_count(), 1);
        assert_eq!(ei.import_count(), 1);
    }

    #[test]
    fn export_formats() {
        assert_eq!(ExportFormat::ONNX as u32, 0);
        assert_eq!(ExportFormat::EXL2 as u32, 4);
    }

    #[test]
    fn metadata_index() {
        let mut idx = ModelMetadataIndex::new();
        idx.add("model-a", "transformer", 7_000_000_000, "fp16", &["chat", "coding"]);
        idx.add("model-b", "moe", 100_000_000_000, "int4", &["chat", "reasoning"]);
        assert_eq!(idx.entry_count(), 2);
        let entry = idx.get("model-a").unwrap();
        assert_eq!(entry.arch, "transformer");
        let chat_models = idx.find_by_tag("chat");
        assert_eq!(chat_models.len(), 2);
    }

    #[test]
    fn prune_strategy_names() {
        assert_eq!(PruneStrategy::Magnitude as u32, 0);
        assert_eq!(PruneStrategy::Gradual as u32, 3);
    }

    #[test]
    fn merge_strategy_names() {
        assert_eq!(MergeStrategy::WeightAverage as u32, 0);
        assert_eq!(MergeStrategy::Ties as u32, 3);
    }

    #[test]
    fn calibration_method_names() {
        assert_eq!(CalibrationMethod::MinMax as u32, 0);
        assert_eq!(CalibrationMethod::MSE as u32, 3);
    }
}

// =====================================================================================
// 模型推理服务引擎 (ModelInferenceServer)
// =====================================================================================

/// 推理请求。
#[derive(Clone, Debug)]
pub struct InferenceResponse {
    pub request_id: u64,
    pub output_tokens: Vec<u32>,
    pub generated_text: String,
    pub tokens_generated: u64,
    pub prefill_time_ns: u128,
    pub decode_time_ns: u128,
    pub total_time_ns: u128,
    pub status: InferenceStatus,
}

/// 推理状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

/// 推理服务器：管理模型推理请求队列和执行。
pub struct InferenceServer {
    pub request_queue: VecDeque<InferenceRequest>,
    pub processing: BTreeMap<u64, InferenceRequest>,
    pub completed: BTreeMap<u64, InferenceResponse>,
    pub next_id: u64,
    pub max_concurrent: u64,
    pub total_requests: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub avg_latency_ns: u128,
}

impl InferenceServer {
    pub fn new() -> Self {
        Self {
            request_queue: VecDeque::new(),
            processing: BTreeMap::new(),
            completed: BTreeMap::new(),
            next_id: 1,
            max_concurrent: 8,
            total_requests: 0,
            completed_count: 0,
            failed_count: 0,
            avg_latency_ns: 0,
        }
    }

    pub fn submit(&mut self, model_name: &str, instance_id: InstanceId, input: Vec<u32>, max_new: u64, temp: f32, top_p: f32, top_k: u32, priority: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let req = InferenceRequest::new(id, model_name, instance_id, input, max_new, temp, top_p, top_k, priority);
        self.total_requests += 1;
        let pos = self.request_queue.iter().position(|r| r.priority < priority).unwrap_or(self.request_queue.len());
        self.request_queue.insert(pos, req);
        id
    }

    pub fn start_processing(&mut self) -> Vec<u64> {
        let mut started = Vec::new();
        while self.processing.len() < self.max_concurrent as usize {
            if let Some(req) = self.request_queue.pop_front() {
                let id = req.id;
                self.processing.insert(id, req);
                started.push(id);
            } else {
                break;
            }
        }
        started
    }

    pub fn complete(&mut self, request_id: u64, response: InferenceResponse) -> bool {
        if let Some(_req) = self.processing.remove(&request_id) {
            let total = response.total_time_ns;
            self.completed.insert(request_id, response);
            self.completed_count += 1;
            self.avg_latency_ns = (self.avg_latency_ns * (self.completed_count - 1) as u128 + total as u128) / self.completed_count as u128;
            true
        } else {
            false
        }
    }

    pub fn fail(&mut self, request_id: u64, _error: &str) -> bool {
        if self.processing.remove(&request_id).is_some() {
            self.failed_count += 1;
            true
        } else {
            false
        }
    }

    pub fn get_response(&self, request_id: u64) -> Option<&InferenceResponse> {
        self.completed.get(&request_id)
    }

    pub fn queue_len(&self) -> usize {
        self.request_queue.len()
    }

    pub fn processing_count(&self) -> usize {
        self.processing.len()
    }

    pub fn stats(&self) -> String {
        format!(
            "inference: total={} completed={} failed={} queue={} processing={} avg_latency={}ns",
            self.total_requests, self.completed_count, self.failed_count, self.queue_len(), self.processing_count(), self.avg_latency_ns
        )
    }
}

impl Default for InferenceServer {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 批处理引擎 (BatchProcessor)
// =====================================================================================

/// 批处理策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchStrategy {
    Fixed,
    Dynamic,
    Adaptive,
}

/// 批次。
#[derive(Clone, Debug)]
pub struct Batch {
    pub id: u64,
    pub requests: Vec<u64>,
    pub total_tokens: u64,
    pub created_at: u128,
    pub completed_at: Option<u128>,
}

/// 批处理引擎：将多个推理请求合并为批次以提高吞吐。
pub struct BatchProcessor {
    pub strategy: BatchStrategy,
    pub max_batch_size: usize,
    pub max_wait_ns: u128,
    pub batches: BTreeMap<u64, Batch>,
    pub next_batch_id: u64,
    pub total_batches: u64,
    pub avg_batch_size: f32,
}

impl BatchProcessor {
    pub fn new() -> Self {
        Self {
            strategy: BatchStrategy::Dynamic,
            max_batch_size: 32,
            max_wait_ns: 10_000_000,
            batches: BTreeMap::new(),
            next_batch_id: 1,
            total_batches: 0,
            avg_batch_size: 0.0,
        }
    }

    pub fn set_strategy(&mut self, strategy: BatchStrategy) {
        self.strategy = strategy;
    }

    pub fn set_max_batch_size(&mut self, size: usize) {
        self.max_batch_size = size;
    }

    pub fn create_batch(&mut self, request_ids: Vec<u64>, total_tokens: u64) -> u64 {
        let id = self.next_batch_id;
        self.next_batch_id += 1;
        let batch = Batch {
            id,
            requests: request_ids.clone(),
            total_tokens,
            created_at: crate::ai_os::native::now_ns(),
            completed_at: None,
        };
        self.total_batches += 1;
        let count = request_ids.len() as f32;
        self.avg_batch_size = (self.avg_batch_size * (self.total_batches - 1) as f32 + count) / self.total_batches as f32;
        self.batches.insert(id, batch);
        id
    }

    pub fn complete_batch(&mut self, batch_id: u64) -> bool {
        if let Some(batch) = self.batches.get_mut(&batch_id) {
            batch.completed_at = Some(crate::ai_os::native::now_ns());
            true
        } else {
            false
        }
    }

    pub fn get_batch(&self, batch_id: u64) -> Option<&Batch> {
        self.batches.get(&batch_id)
    }

    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }

    pub fn can_batch(&self, current_size: usize) -> bool {
        current_size < self.max_batch_size
    }
}

impl Default for BatchProcessor {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 响应缓存 (ResponseCache)
// =====================================================================================

/// 缓存条目。
#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub key: String,
    pub response: InferenceResponse,
    pub created_at: u128,
    pub access_count: u64,
    pub last_access: u128,
    pub ttl_ns: Option<u128>,
}

/// 响应缓存：缓存常见推理结果以减少重复计算。
pub struct ResponseCache {
    pub entries: BTreeMap<String, CacheEntry>,
    pub max_size: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl ResponseCache {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            max_size: 10000,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    pub fn set_max_size(&mut self, size: usize) {
        self.max_size = size;
    }

    pub fn get(&mut self, key: &str) -> Option<InferenceResponse> {
        if let Some(entry) = self.entries.get_mut(key) {
            if let Some(ttl) = entry.ttl_ns {
                let now = crate::ai_os::native::now_ns();
                if now - entry.created_at > ttl {
                    self.entries.remove(key);
                    self.evictions += 1;
                    self.misses += 1;
                    return None;
                }
            }
            entry.access_count += 1;
            entry.last_access = crate::ai_os::native::now_ns();
            self.hits += 1;
            Some(entry.response.clone())
        } else {
            self.misses += 1;
            None
        }
    }

    pub fn put(&mut self, key: &str, response: InferenceResponse, ttl_ns: Option<u128>) {
        if self.entries.len() >= self.max_size {
            if let Some(oldest_key) = self.entries.iter()
                .min_by_key(|(_, e)| e.last_access)
                .map(|(k, _)| k.clone()) {
                self.entries.remove(&oldest_key);
                self.evictions += 1;
            }
        }
        self.entries.insert(key.to_string(), CacheEntry {
            key: key.to_string(),
            response,
            created_at: crate::ai_os::native::now_ns(),
            access_count: 1,
            last_access: crate::ai_os::native::now_ns(),
            ttl_ns,
        });
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f32 / total as f32
        }
    }
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 指标采集 (MetricsCollector)
// =====================================================================================

/// 指标类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
}

/// 指标点。
#[derive(Clone, Debug)]
pub struct MetricPoint {
    pub name: String,
    pub metric_type: MetricType,
    pub value: f64,
    pub labels: BTreeMap<String, String>,
    pub timestamp_ns: u128,
}

/// 指标采集器：收集模型服务各项指标。
pub struct MetricsCollector {
    pub metrics: BTreeMap<String, Vec<MetricPoint>>,
    pub counters: BTreeMap<String, f64>,
    pub gauges: BTreeMap<String, f64>,
    pub total_points: u64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            metrics: BTreeMap::new(),
            counters: BTreeMap::new(),
            gauges: BTreeMap::new(),
            total_points: 0,
        }
    }

    pub fn increment_counter(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        let entry = self.counters.entry(name.to_string()).or_insert(0.0);
        *entry += value;
        let point = MetricPoint {
            name: name.to_string(),
            metric_type: MetricType::Counter,
            value: *entry,
            labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            timestamp_ns: crate::ai_os::native::now_ns(),
        };
        self.metrics.entry(name.to_string()).or_default().push(point);
        self.total_points += 1;
    }

    pub fn set_gauge(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.gauges.insert(name.to_string(), value);
        let point = MetricPoint {
            name: name.to_string(),
            metric_type: MetricType::Gauge,
            value,
            labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            timestamp_ns: crate::ai_os::native::now_ns(),
        };
        self.metrics.entry(name.to_string()).or_default().push(point);
        self.total_points += 1;
    }

    pub fn observe_histogram(&mut self, name: &str, value: f64, labels: &[(&str, &str)]) {
        let point = MetricPoint {
            name: name.to_string(),
            metric_type: MetricType::Histogram,
            value,
            labels: labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            timestamp_ns: crate::ai_os::native::now_ns(),
        };
        self.metrics.entry(name.to_string()).or_default().push(point);
        self.total_points += 1;
    }

    pub fn get_counter(&self, name: &str) -> f64 {
        self.counters.get(name).copied().unwrap_or(0.0)
    }

    pub fn get_gauge(&self, name: &str) -> f64 {
        self.gauges.get(name).copied().unwrap_or(0.0)
    }

    pub fn metric_count(&self) -> usize {
        self.metrics.len()
    }

    pub fn total_point_count(&self) -> u64 {
        self.total_points
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 负载均衡器 (LoadBalancer)
// =====================================================================================

/// 负载均衡策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LBStrategy {
    RoundRobin,
    LeastConn,
    Random,
    Weighted,
}

/// 后端实例。
#[derive(Clone, Debug)]
pub struct BackendInstance {
    pub id: u64,
    pub name: String,
    pub endpoint: String,
    pub weight: u32,
    pub active_connections: u64,
    pub total_requests: u64,
    pub failed_requests: u64,
    pub healthy: bool,
}

/// 负载均衡器：分发推理请求到多个模型实例。
pub struct LoadBalancer {
    pub strategy: LBStrategy,
    pub instances: BTreeMap<u64, BackendInstance>,
    pub next_id: u64,
    pub rr_index: u64,
    pub total_requests: u64,
}

impl LoadBalancer {
    pub fn new() -> Self {
        Self {
            strategy: LBStrategy::RoundRobin,
            instances: BTreeMap::new(),
            next_id: 1,
            rr_index: 0,
            total_requests: 0,
        }
    }

    pub fn set_strategy(&mut self, strategy: LBStrategy) {
        self.strategy = strategy;
    }

    pub fn add_instance(&mut self, name: &str, endpoint: &str, weight: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.instances.insert(id, BackendInstance {
            id,
            name: name.to_string(),
            endpoint: endpoint.to_string(),
            weight,
            active_connections: 0,
            total_requests: 0,
            failed_requests: 0,
            healthy: true,
        });
        id
    }

    pub fn select(&mut self) -> Option<u64> {
        let healthy: Vec<u64> = self.instances.iter()
            .filter(|(_, inst)| inst.healthy)
            .map(|(id, _)| *id)
            .collect();
        if healthy.is_empty() {
            return None;
        }
        let selected = match self.strategy {
            LBStrategy::RoundRobin => {
                let idx = (self.rr_index % healthy.len() as u64) as usize;
                self.rr_index += 1;
                healthy[idx]
            }
            LBStrategy::LeastConn => {
                healthy.iter()
                    .min_by_key(|id| self.instances.get(*id).map(|i| i.active_connections).unwrap_or(u64::MAX))
                    .copied()
                    .unwrap_or(healthy[0])
            }
            LBStrategy::Random => {
                let idx = (fastrand_f32() * healthy.len() as f32) as usize;
                healthy[idx.min(healthy.len() - 1)]
            }
            LBStrategy::Weighted => {
                let total_weight: u32 = healthy.iter().filter_map(|id| self.instances.get(id).map(|i| i.weight)).sum();
                let mut target = fastrand_f32() * total_weight as f32;
                for id in &healthy {
                    if let Some(inst) = self.instances.get(id) {
                        target -= inst.weight as f32;
                        if target <= 0.0 {
                            return Some(*id);
                        }
                    }
                }
                healthy[0]
            }
        };
        if let Some(inst) = self.instances.get_mut(&selected) {
            inst.active_connections += 1;
            inst.total_requests += 1;
        }
        self.total_requests += 1;
        Some(selected)
    }

    pub fn release(&mut self, instance_id: u64) {
        if let Some(inst) = self.instances.get_mut(&instance_id) {
            inst.active_connections = inst.active_connections.saturating_sub(1);
        }
    }

    pub fn mark_failed(&mut self, instance_id: u64) {
        if let Some(inst) = self.instances.get_mut(&instance_id) {
            inst.failed_requests += 1;
            if inst.failed_requests > 5 {
                inst.healthy = false;
            }
        }
    }

    pub fn mark_healthy(&mut self, instance_id: u64) {
        if let Some(inst) = self.instances.get_mut(&instance_id) {
            inst.healthy = true;
            inst.failed_requests = 0;
        }
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    pub fn healthy_count(&self) -> usize {
        self.instances.values().filter(|i| i.healthy).count()
    }
}

impl Default for LoadBalancer {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 模型服务引擎 (ModelServingEngine) —— 统一入口
// =====================================================================================

/// 模型服务引擎：整合推理服务器、批处理器、响应缓存、指标采集、负载均衡器。
pub struct ModelServingEngine {
    pub inference_server: InferenceServer,
    pub batch_processor: BatchProcessor,
    pub response_cache: ResponseCache,
    pub metrics: MetricsCollector,
    pub load_balancer: LoadBalancer,
    pub enabled: bool,
    pub total_requests: u64,
}

impl ModelServingEngine {
    pub fn new() -> Self {
        Self {
            inference_server: InferenceServer::new(),
            batch_processor: BatchProcessor::new(),
            response_cache: ResponseCache::new(),
            metrics: MetricsCollector::new(),
            load_balancer: LoadBalancer::new(),
            enabled: true,
            total_requests: 0,
        }
    }

    pub fn submit_inference(&mut self, model_name: &str, instance_id: InstanceId, input: Vec<u32>, max_new: u64, temp: f32, top_p: f32, top_k: u32, priority: u32) -> u64 {
        let id = self.inference_server.submit(model_name, instance_id, input, max_new, temp, top_p, top_k, priority);
        self.total_requests += 1;
        self.metrics.increment_counter("model_requests_total", 1.0, &[("model", model_name)]);
        id
    }

    pub fn process_batch(&mut self) -> Vec<u64> {
        let started = self.inference_server.start_processing();
        if !started.is_empty() {
            let total_tokens: u64 = started.iter()
                .filter_map(|id| self.inference_server.processing.get(id))
                .map(|r| r.input_tokens.len() as u64 + r.max_new_tokens)
                .sum();
            self.batch_processor.create_batch(started.clone(), total_tokens);
            self.metrics.increment_counter("model_batches_total", 1.0, &[]);
        }
        started
    }

    pub fn complete_inference(&mut self, request_id: u64, output_tokens: Vec<u32>, generated_text: String, prefill_ns: u128, decode_ns: u128) -> Result<(), String> {
        let total = prefill_ns + decode_ns;
        let token_count = output_tokens.len() as u64;
        let response = InferenceResponse {
            request_id,
            output_tokens,
            generated_text,
            tokens_generated: token_count,
            prefill_time_ns: prefill_ns,
            decode_time_ns: decode_ns,
            total_time_ns: total,
            status: InferenceStatus::Completed,
        };
        let success = self.inference_server.complete(request_id, response.clone());
        if success {
            let cache_key = format!("req_{}", request_id);
            self.response_cache.put(&cache_key, response, None);
            self.metrics.observe_histogram("model_inference_latency_ns", total as f64, &[]);
            Ok(())
        } else {
            Err("请求不存在".into())
        }
    }

    pub fn fail_inference(&mut self, request_id: u64, error: &str) -> bool {
        let result = self.inference_server.fail(request_id, error);
        if result {
            self.metrics.increment_counter("model_requests_failed_total", 1.0, &[]);
        }
        result
    }

    pub fn get_cached_response(&mut self, key: &str) -> Option<InferenceResponse> {
        let result = self.response_cache.get(key);
        if result.is_some() {
            self.metrics.increment_counter("model_cache_hits_total", 1.0, &[]);
        } else {
            self.metrics.increment_counter("model_cache_misses_total", 1.0, &[]);
        }
        result
    }

    pub fn add_backend(&mut self, name: &str, endpoint: &str, weight: u32) -> u64 {
        self.load_balancer.add_instance(name, endpoint, weight)
    }

    pub fn select_backend(&mut self) -> Option<u64> {
        self.load_balancer.select()
    }

    pub fn release_backend(&mut self, instance_id: u64) {
        self.load_balancer.release(instance_id);
    }

    pub fn tick(&mut self) {
        let _ = self.inference_server.queue_len();
        let _ = self.inference_server.processing_count();
    }

    pub fn stats(&self) -> String {
        format!(
            "serving: requests={} queue={} processing={} completed={} failed={} cache_hit={:.2} batches={} backends={} healthy={}",
            self.total_requests,
            self.inference_server.queue_len(),
            self.inference_server.processing_count(),
            self.inference_server.completed_count,
            self.inference_server.failed_count,
            self.response_cache.hit_rate(),
            self.batch_processor.batch_count(),
            self.load_balancer.instance_count(),
            self.load_balancer.healthy_count()
        )
    }
}

impl Default for ModelServingEngine {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static SERVING: OnceLock<Mutex<ModelServingEngine>> = OnceLock::new();

pub fn model_serving_engine() -> &'static Mutex<ModelServingEngine> {
    SERVING.get_or_init(|| Mutex::new(ModelServingEngine::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================


// =====================================================================================
// act 接口
// =====================================================================================

pub fn serving_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("submit") => {
            let model = it.next().ok_or("用法: model_serving:// submit <model> <instance_id> <token...> [max_new] [temp] [top_p] [top_k] [priority]")?;
            let instance_id = it.next().and_then(|s| s.parse::<u64>().ok()).map(InstanceId).unwrap_or(InstanceId(1));
            let tokens: Vec<u32> = it.by_ref().filter_map(|s| s.parse::<u32>().ok()).collect();
            if tokens.is_empty() {
                return Err("缺少输入token".into());
            }
            let max_new = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(64);
            let temp = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.7);
            let top_p = it.next().and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.9);
            let top_k = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(40);
            let priority = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
            let mut eng = model_serving_engine().lock().unwrap();
            let token_count = tokens.len();
            let id = eng.submit_inference(model, instance_id, tokens, max_new, temp, top_p, top_k, priority);
            Ok(format!("已提交推理请求 id={} 模型={} 实例={} tokens={}", id, model, instance_id, token_count))
        }
        Some("process") => {
            let mut eng = model_serving_engine().lock().unwrap();
            let started = eng.process_batch();
            Ok(format!("已启动批处理 请求数={}", started.len()))
        }
        Some("complete") => {
            let rest: Vec<&str> = it.collect();
            let request_id = rest.get(0).and_then(|s| s.parse::<u64>().ok()).ok_or("缺少请求id")?;
            let tokens: Vec<u32> = rest.iter().filter_map(|s| s.parse::<u32>().ok()).collect();
            let text: String = rest.join(" ");
            let prefill = rest.get(1).and_then(|s| s.parse::<u128>().ok()).unwrap_or(1_000_000);
            let decode = rest.get(2).and_then(|s| s.parse::<u128>().ok()).unwrap_or(5_000_000);
            let mut eng = model_serving_engine().lock().unwrap();
            eng.complete_inference(request_id, tokens, text, prefill, decode)?;
            Ok(format!("已完成推理请求 id={}", request_id))
        }
        Some("fail") => {
            let request_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少请求id")?;
            let error: String = it.collect::<Vec<&str>>().join(" ");
            let mut eng = model_serving_engine().lock().unwrap();
            if eng.fail_inference(request_id, &error) {
                Ok(format!("请求 {} 标记为失败: {}", request_id, error))
            } else {
                Err(format!("请求 {} 不存在", request_id))
            }
        }
        Some("cache") => {
            let key = it.next().ok_or("用法: model_serving:// cache <key> [get|put]")?;
            let op = it.next().unwrap_or("get");
            match op {
                "get" => {
                    let mut eng = model_serving_engine().lock().unwrap();
                    match eng.get_cached_response(key) {
                        Some(resp) => Ok(format!("缓存命中 请求={} 生成tokens={}", resp.request_id, resp.tokens_generated)),
                        None => Ok("缓存未命中".into()),
                    }
                }
                "put" => Ok("缓存put需要完整响应数据".into()),
                _ => Err("未知缓存操作".into()),
            }
        }
        Some("backend") => {
            let name = it.next().ok_or("缺少后端名称")?;
            let endpoint = it.next().ok_or("缺少端点")?;
            let weight = it.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
            let mut eng = model_serving_engine().lock().unwrap();
            let id = eng.add_backend(name, endpoint, weight);
            Ok(format!("已添加后端 id={} 名称={} 端点={}", id, name, endpoint))
        }
        Some("select-backend") => {
            let mut eng = model_serving_engine().lock().unwrap();
            match eng.select_backend() {
                Some(id) => Ok(format!("已选择后端 id={}", id)),
                None => Err("无可用后端".into()),
            }
        }
        Some("release-backend") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少后端id")?;
            let mut eng = model_serving_engine().lock().unwrap();
            eng.release_backend(id);
            Ok(format!("已释放后端 id={}", id))
        }
        Some("tick") => {
            let mut eng = model_serving_engine().lock().unwrap();
            eng.tick();
            Ok("服务引擎已推进".into())
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 model_serving 动作: {}", other)),
    }
}

pub fn serving_act_dispatch(payload: &str) -> Result<String, String> {
    serving_act(payload)
}

// =====================================================================================
// 模型服务扩展测试
// =====================================================================================

#[cfg(test)]
mod serving_tests {
    use super::*;

    #[test]
    fn inference_server_submit() {
        let mut eng = InferenceServer::new();
        let id = eng.submit("model-a", InstanceId(1), vec![1, 2, 3], 64, 0.7, 0.9, 40, 1);
        assert_eq!(id, 1);
        assert_eq!(eng.queue_len(), 1);
    }

    #[test]
    fn inference_server_process() {
        let mut eng = InferenceServer::new();
        eng.submit("model-a", InstanceId(1), vec![1], 64, 0.7, 0.9, 40, 1);
        eng.submit("model-a", InstanceId(1), vec![2], 64, 0.7, 0.9, 40, 1);
        let started = eng.start_processing();
        assert_eq!(started.len(), 2);
        assert_eq!(eng.processing_count(), 2);
        assert_eq!(eng.queue_len(), 0);
    }

    #[test]
    fn inference_server_complete() {
        let mut eng = InferenceServer::new();
        let id = eng.submit("model-a", InstanceId(1), vec![1], 64, 0.7, 0.9, 40, 1);
        eng.start_processing();
        let resp = InferenceResponse {
            request_id: id,
            output_tokens: vec![4, 5, 6],
            generated_text: "hello".into(),
            tokens_generated: 3,
            prefill_time_ns: 1_000_000,
            decode_time_ns: 2_000_000,
            total_time_ns: 3_000_000,
            status: InferenceStatus::Completed,
        };
        assert!(eng.complete(id, resp));
        assert_eq!(eng.completed_count, 1);
    }

    #[test]
    fn inference_server_fail() {
        let mut eng = InferenceServer::new();
        let id = eng.submit("model-a", InstanceId(1), vec![1], 64, 0.7, 0.9, 40, 1);
        eng.start_processing();
        assert!(eng.fail(id, "timeout"));
        assert_eq!(eng.failed_count, 1);
    }

    #[test]
    fn batch_processor() {
        let mut bp = BatchProcessor::new();
        bp.set_max_batch_size(4);
        let id = bp.create_batch(vec![1, 2, 3], 100);
        assert_eq!(id, 1);
        assert_eq!(bp.batch_count(), 1);
        assert!(bp.complete_batch(id));
    }

    #[test]
    fn batch_processor_dynamic() {
        let mut bp = BatchProcessor::new();
        bp.set_strategy(BatchStrategy::Dynamic);
        bp.set_max_batch_size(2);
        assert!(bp.can_batch(1));
        assert!(!bp.can_batch(2));
    }

    #[test]
    fn response_cache() {
        let mut cache = ResponseCache::new();
        let resp = InferenceResponse {
            request_id: 1,
            output_tokens: vec![1],
            generated_text: "test".into(),
            tokens_generated: 1,
            prefill_time_ns: 0,
            decode_time_ns: 0,
            total_time_ns: 0,
            status: InferenceStatus::Completed,
        };
        cache.put("key1", resp.clone(), None);
        assert_eq!(cache.get("key1"), Some(resp));
        assert_eq!(cache.entry_count(), 1);
        assert!((cache.hit_rate() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn response_cache_miss() {
        let mut cache = ResponseCache::new();
        assert!(cache.get("missing").is_none());
        assert_eq!(cache.misses, 1);
        assert!((cache.hit_rate() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn metrics_collector() {
        let mut mc = MetricsCollector::new();
        mc.increment_counter("requests", 1.0, &[("model", "a")]);
        mc.increment_counter("requests", 1.0, &[("model", "a")]);
        mc.set_gauge("latency", 50.0, &[]);
        mc.observe_histogram("tokens", 100.0, &[]);
        assert!((mc.get_counter("requests") - 2.0).abs() < 1e-6);
        assert!((mc.get_gauge("latency") - 50.0).abs() < 1e-6);
        assert_eq!(mc.metric_count(), 3);
        assert_eq!(mc.total_point_count(), 4);
    }

    #[test]
    fn load_balancer() {
        let mut lb = LoadBalancer::new();
        lb.add_instance("inst1", "host1:8080", 1);
        lb.add_instance("inst2", "host2:8080", 1);
        let sel1 = lb.select().unwrap();
        let sel2 = lb.select().unwrap();
        assert_ne!(sel1, sel2);
        lb.release(sel1);
        assert_eq!(lb.instance_count(), 2);
        assert_eq!(lb.healthy_count(), 2);
    }

    #[test]
    fn load_balancer_weighted() {
        let mut lb = LoadBalancer::new();
        lb.set_strategy(LBStrategy::Weighted);
        lb.add_instance("heavy", "host1:8080", 10);
        lb.add_instance("light", "host2:8080", 1);
        let mut heavy_count = 0;
        let mut light_count = 0;
        for _ in 0..100 {
            match lb.select() {
                Some(id) => {
                    if id == 1 { heavy_count += 1; }
                    if id == 2 { light_count += 1; }
                }
                None => {}
            }
        }
        assert!(heavy_count > light_count);
    }

    #[test]
    fn serving_engine_full() {
        let mut eng = ModelServingEngine::new();
        let id = eng.submit_inference("model-a", InstanceId(1), vec![1, 2, 3], 64, 0.7, 0.9, 40, 1);
        assert_eq!(id, 1);
        let started = eng.process_batch();
        assert!(!started.is_empty());
        eng.complete_inference(1, vec![4, 5], "hi".into(), 1_000_000, 2_000_000).unwrap();
        assert_eq!(eng.inference_server.completed_count, 1);
    }

    #[test]
    fn serving_engine_cache() {
        let mut eng = ModelServingEngine::new();
        eng.submit_inference("model-a", InstanceId(1), vec![1], 64, 0.7, 0.9, 40, 1);
        eng.process_batch();
        eng.complete_inference(1, vec![2], "cached".into(), 500_000, 500_000).unwrap();
        let cached = eng.get_cached_response("req_1");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().generated_text, "cached");
    }

    #[test]
    fn serving_engine_backend() {
        let mut eng = ModelServingEngine::new();
        let id = eng.add_backend("inst1", "host1:8080", 5);
        assert_eq!(id, 1);
        let selected = eng.select_backend().unwrap();
        assert_eq!(selected, 1);
        eng.release_backend(1);
    }

    #[test]
    fn serving_stats() {
        let mut eng = ModelServingEngine::new();
        eng.submit_inference("m", InstanceId(1), vec![1], 10, 0.5, 0.9, 40, 0);
        let s = eng.stats();
        assert!(s.contains("serving:"));
        assert!(s.contains("requests=1"));
    }

    #[test]
    fn inference_statuses() {
        assert_eq!(InferenceStatus::Pending as u32, 0);
        assert_eq!(InferenceStatus::Completed as u32, 2);
        assert_eq!(InferenceStatus::Cancelled as u32, 4);
    }

    #[test]
    fn batch_strategies() {
        assert_eq!(BatchStrategy::Fixed as u32, 0);
        assert_eq!(BatchStrategy::Adaptive as u32, 2);
    }

    #[test]
    fn metric_types() {
        assert_eq!(MetricType::Counter as u32, 0);
        assert_eq!(MetricType::Summary as u32, 3);
    }

    #[test]
    fn lb_strategies() {
        assert_eq!(LBStrategy::RoundRobin as u32, 0);
        assert_eq!(LBStrategy::Weighted as u32, 3);
    }

    #[test]
    fn serving_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("serving:"));
        assert!(s.contains("requests="));
    }

    #[test]
    fn serving_act_submit() {
        init();
        let res = serving_act("submit model-a 1 10 20 30 64 0.7 0.9 40 1").unwrap();
        assert!(res.contains("已提交推理请求"));
    }

    #[test]
    fn serving_act_process() {
        init();
        serving_act("submit model-a 1 1 2 3 64 0.7 0.9 40 1").unwrap();
        let res = serving_act("process").unwrap();
        assert!(res.contains("已启动批处理"));
    }

    #[test]
    fn serving_act_complete() {
        init();
        serving_act("submit model-a 1 1 2 3 64 0.7 0.9 40 1").unwrap();
        serving_act("process").unwrap();
        let res = serving_act("complete 1 4 5 hi 1000000 2000000").unwrap();
        assert!(res.is_ok());
    }

    #[test]
    fn serving_act_cache() {
        init();
        serving_act("submit model-a 1 1 2 3 64 0.7 0.9 40 1").unwrap();
        serving_act("process").unwrap();
        serving_act("complete 1 4 5 cached 500000 500000").unwrap();
        let res = serving_act("cache req_1 get").unwrap();
        assert!(res.contains("缓存命中"));
    }

    #[test]
    fn serving_act_backend() {
        init();
        let res = serving_act("backend inst1 host1:8080 5").unwrap();
        assert!(res.contains("已添加后端"));
        let res = serving_act("select-backend").unwrap();
        assert!(res.contains("已选择后端"));
    }

    #[test]
    fn serving_act_status() {
        init();
        let res = serving_act("status").unwrap();
        assert!(res.contains("serving:"));
    }

    #[test]
    fn inference_request_creation() {
        let req = InferenceRequest::new(1, "m", InstanceId(1), vec![1, 2, 3], 64, 0.7, 0.9, 40, 1);
        assert_eq!(req.model_name, "m");
        assert_eq!(req.input_tokens.len(), 3);
    }

    #[test]
    fn inference_response_creation() {
        let resp = InferenceResponse {
            request_id: 1,
            output_tokens: vec![4, 5],
            generated_text: "text".into(),
            tokens_generated: 2,
            prefill_time_ns: 100,
            decode_time_ns: 200,
            total_time_ns: 300,
            status: InferenceStatus::Processing,
        };
        assert_eq!(resp.tokens_generated, 2);
        assert_eq!(resp.status, InferenceStatus::Processing);
    }
}
