//! 系统调用接口 —— AI 意识本体与 GPU 内核之间的交互边界。
//!
//! 本目录下还并行存在 POSIX 兼容层、ELF 加载器与动态链接器，
//! 它们与 `AiSystemCall` 并行存在，不替换现有系统调用。

use crate::ai_token::{
    ComputeGraph, ConceptId, ErrorCode, ProcessResult, ProcessState, SchedulePolicy,
    TokenId, TokenStatus,
};
use crate::gpu::GpuCapabilities;
use crate::scheduler::CpuPriority;

pub mod posix_layer;
pub mod elf_loader;
pub mod dynamic_linker;

/// 系统调用编号与参数。
#[derive(Clone, Debug)]
pub enum AiSystemCall {
    /// 创建新的 AI Action Token
    CreateToken {
        model_id: String,
        capabilities: GpuCapabilities,
        vram_size: u64,
        priority: CpuPriority,
        concept_id: Option<ConceptId>,
    },
    /// 创建行动进程
    CreateProcess {
        ontology_id: ConceptId,
        compute_graph: ComputeGraph,
        vram_size: usize,
        priority: CpuPriority,
    },
    /// 修改 Token 能力
    UpdateCaps {
        token_id: TokenId,
        new_caps: GpuCapabilities,
    },
    /// 设置优先级
    SetPriority {
        token_id: TokenId,
        new_priority: CpuPriority,
    },
    /// 查询 Token
    QueryToken { token_id: TokenId },
    /// 查询进程
    QueryProcess { process_id: TokenId },
    /// 提交负载
    SubmitLoad {
        token_id: TokenId,
        program_id: u64,
        data_ptr: u64,
    },
    /// 完成进程
    CompleteProcess {
        process_id: TokenId,
        success: bool,
        output: Option<Vec<u8>>,
    },
    /// 失败进程
    FailProcess {
        process_id: TokenId,
        error: ErrorCode,
    },
    /// 查询概念调度策略
    QuerySchedulePolicy { concept_id: ConceptId },
}

impl AiSystemCall {
    pub fn name(&self) -> &'static str {
        match self {
            AiSystemCall::CreateToken { .. } => "CreateToken",
            AiSystemCall::CreateProcess { .. } => "CreateProcess",
            AiSystemCall::UpdateCaps { .. } => "UpdateCaps",
            AiSystemCall::SetPriority { .. } => "SetPriority",
            AiSystemCall::QueryToken { .. } => "QueryToken",
            AiSystemCall::QueryProcess { .. } => "QueryProcess",
            AiSystemCall::SubmitLoad { .. } => "SubmitLoad",
            AiSystemCall::CompleteProcess { .. } => "CompleteProcess",
            AiSystemCall::FailProcess { .. } => "FailProcess",
            AiSystemCall::QuerySchedulePolicy { .. } => "QuerySchedulePolicy",
        }
    }
}

impl std::fmt::Display for AiSystemCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// 系统调用结果。
#[derive(Clone, Debug, Default)]
pub struct AiSystemCallResult {
    pub success: bool,
    pub token_id: Option<TokenId>,
    pub process_id: Option<TokenId>,
    pub error: Option<AiSystemCallError>,
    pub token_status: Option<TokenStatus>,
    pub process_status: Option<(TokenId, ProcessState, Option<ProcessResult>)>,
    pub schedule_policy: Option<SchedulePolicy>,
}

impl AiSystemCallResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            ..Default::default()
        }
    }

    pub fn err(e: AiSystemCallError) -> Self {
        Self {
            success: false,
            error: Some(e),
            ..Default::default()
        }
    }
}

/// 系统调用错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiSystemCallError {
    TokenNotFound,
    TokenExpired,
    InsufficientCapabilities,
    InsufficientVram,
    AccessDenied,
    InvalidProgram,
    SchedulerBusy,
    ProcessNotFound,
    ProcessAlreadyRunning,
    InvalidProcessState,
    ConceptNotFound,
    PolicyLookupFailed,
}

impl std::fmt::Display for AiSystemCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AiSystemCallError::TokenNotFound => "TokenNotFound",
            AiSystemCallError::TokenExpired => "TokenExpired",
            AiSystemCallError::InsufficientCapabilities => "InsufficientCapabilities",
            AiSystemCallError::InsufficientVram => "InsufficientVram",
            AiSystemCallError::AccessDenied => "AccessDenied",
            AiSystemCallError::InvalidProgram => "InvalidProgram",
            AiSystemCallError::SchedulerBusy => "SchedulerBusy",
            AiSystemCallError::ProcessNotFound => "ProcessNotFound",
            AiSystemCallError::ProcessAlreadyRunning => "ProcessAlreadyRunning",
            AiSystemCallError::InvalidProcessState => "InvalidProcessState",
            AiSystemCallError::ConceptNotFound => "ConceptNotFound",
            AiSystemCallError::PolicyLookupFailed => "PolicyLookupFailed",
        };
        write!(f, "{}", s)
    }
}

impl From<crate::ai_token::TokenError> for AiSystemCallError {
    fn from(e: crate::ai_token::TokenError) -> Self {
        use crate::ai_token::TokenError as T;
        match e {
            T::NotFound => AiSystemCallError::TokenNotFound,
            T::Expired => AiSystemCallError::TokenExpired,
            T::InsufficientCapabilities => AiSystemCallError::InsufficientCapabilities,
            T::InsufficientVram => AiSystemCallError::InsufficientVram,
            T::AccessDenied => AiSystemCallError::AccessDenied,
            _ => AiSystemCallError::InvalidProcessState,
        }
    }
}

/// 初始化系统调用子系统 (极简启动，不在终端打印)。
pub fn init() {
    // 系统调用门闸已就绪；启动阶段保持静默。
}
