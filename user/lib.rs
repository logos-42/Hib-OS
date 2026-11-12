/// Hib OS 用户态库
///
/// 该库提供了与内核交互的 API，供本地 AI 模型和工具使用。
/// 通过系统调用与 Hib OS 内核通信。

pub mod api;

/// 版本信息
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Hib OS 系统信息
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// 内核版本
    pub kernel_version: String,
    /// 操作系统名称
    pub os_name: String,
    /// GPU 架构
    pub gpu_architecture: String,
    /// 活跃的 AI Token 数量
    pub active_token_count: usize,
    /// 总 VRAM 容量 (字节)
    pub total_vram: u64,
}

/// 连接到 Hib OS 内核的错误
#[derive(Debug, thiserror::Error)]
pub enum HibOsError {
    #[error("Token 不存在: {0}")]
    TokenNotFound(String),
    
    #[error("权限不足: {0}")]
    InsufficientCapabilities(String),
    
    #[error("VRAM 不足: {0}")]
    InsufficientVram(String),
    
    #[error("调度器忙碌")]
    SchedulerBusy,
    
    #[error("系统调用失败: {0}")]
    SyscallFailed(String),
}

/// 到 Hib OS 内核的结果类型
pub type HibOsResult<T> = Result<T, HibOsError>;

/// 初始化用户态库与内核的连接
/// 
/// 这应该在使用任何 Hib OS 功能之前调用。
/// 实际实现中，这将建立与内核的 IPC 通道。
pub fn init() -> HibOsResult<()> {
    // 实际实现中，这将验证内核是否运行
    // 并初始化通信通道
    Ok(())
}

/// 获取系统信息
pub fn get_system_info() -> HibOsResult<SystemInfo> {
    // 实际实现中，这将通过系统调用向内核请求信息
    Ok(SystemInfo {
        kernel_version: env!("CARGO_PKG_VERSION").to_string(),
        os_name: "Hib OS".to_string(),
        gpu_architecture: "GPU-Centric Microkernel".to_string(),
        active_token_count: 0, // 将从内核查询
        total_vram: 0, // 将从内核查询
    })
}

/// 创建 AI Action Token
pub fn create_token(
    model_id: &str,
    capabilities: &[u64], // GpuCapability 位
    vram_size: u64,
    priority: u8, // CpuPriority 0-255
) -> HibOsResult<u64> {
    // 实际实现中，这将通过系统调用向内核请求创建 Token
    // 返回创建的 Token ID
    Ok(0) // 占位符
}

/// 提交 GPU 负载
pub fn submit_load(token_id: u64, program_data: &[u8]) -> HibOsResult<()> {
    // 实际实现中，这将通过系统调用向内核提交 GPU 负载
    Ok(()) // 占位符
}

/// 查询 Token 状态
pub fn query_token_status(token_id: u64) -> HibOsResult<api::TokenStatusResponse> {
    // 实际实现中，这将通过系统调用向内核查询 Token 状态
    Ok(api::TokenStatusResponse {
        token_id,
        is_valid: false,
        capabilities: 0,
        vram_allocated: 0,
        priority: 0,
        usage_stats: api::UsageStats::default(),
        model_id: "unknown".to_string(),
    })
}

/// 释放 AI Action Token
pub fn destroy_token(token_id: u64) -> HibOsResult<()> {
    // 实际实现中，这将通过系统调用向内核销毁 Token
    Ok(()) // 占位符
}

/// 更新 Token 能力
pub fn update_token_caps(token_id: u64, capabilities: &[u64]) -> HibOsResult<()> {
    // 实际实现中，这将通过系统调用向内核更新 Token 能力
    Ok(()) // 占位符
}

/// 设置 Token 优先级
pub fn set_token_priority(token_id: u64, priority: u8) -> HibOsResult<()> {
    // 实际实现中，这将通过系统调用向内核设置 Token 优先级
    Ok(()) // 占位符
}

/// VRAM 重新分配
pub fn reallocate_vram(token_id: u64, new_vram_size: u64) -> HibOsResult<()> {
    // 实际实现中，这将通过系统调用向内核重新分配 VRAM
    Ok(()) // 占位符
}

/// Token 状态响应
pub mod api {
    /// Token 状态信息
    #[derive(Debug, Clone)]
    pub struct TokenStatusResponse {
        /// Token ID
        pub token_id: u64,
        /// 是否有效
        pub is_valid: bool,
        /// 已授权的能力位
        pub capabilities: u64,
        /// 已分配的 VRAM (字节)
        pub vram_allocated: u64,
        /// 当前优先级 (0-255)
        pub priority: u8,
        /// 使用统计
        pub usage_stats: UsageStats,
        /// 模型标识
        pub model_id: String,
    }
    
    /// 使用统计
    #[derive(Debug, Clone, Default)]
    pub struct UsageStats {
        /// 总运行时间 (纳秒)
        pub total_run_time_ns: u64,
        /// 当前运行时间 (纳秒)
        pub current_run_time_ns: u64,
        /// 已完成的任务数
        pub tasks_completed: u64,
        /// 任务失败数
        pub tasks_failed: u64,
        /// 平均执行时间 (纳秒)
        pub avg_exec_time_ns: u64,
        /// 最后一次活动时间戳
        pub last_activity: u64,
    }
}