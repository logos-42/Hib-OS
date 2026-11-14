//! Hib OS —— GPU 为中心的宏内核 (原型)。
//!
//! 五大支柱:
//! 1. GPU Primacy —— 调度器与内存管理器天生具备 GPU 感知
//! 2. AI Consciousness —— 概念图驱动语义调度
//! 3. Action Processes —— 带语义的计算任务单元
//! 4. Modularity —— 热插拔可扩展模块
//! 5. Unified Perception —— 上下文/行动/文件/智能体/桌面/路径/IO/网关/接口/硬件/文本/轨迹
//!    都是同构的感知通道，统一走 sense/act 抽象

// 在宿主 std 环境下编译，子模块用 `alloc::` 需要先导入 alloc crate。
extern crate alloc;

pub mod agent;
pub mod ai_os;
pub mod ai_token;
pub mod belief;
pub mod browser;
pub mod bus;
pub mod caps;
pub mod cell;
pub mod cgroup;
pub mod channels;
pub mod desktop;
pub mod dev;
pub mod domain;
pub mod env;
pub mod exec;
pub mod fsmeta;
pub mod fw;
pub mod gateway;
pub mod gpu;
pub mod gui;
pub mod hibernate;
pub mod init;
pub mod inspiration;
pub mod ipc;
pub mod judgment;
pub mod lease;
pub mod limits;
pub mod line_editor;
pub mod mem;
pub mod modules;
pub mod monitor;
pub mod motive;
pub mod mount;
pub mod net;
pub mod netif;
pub mod pkg;
pub mod plugin;
pub mod power;
pub mod proc;
pub mod proc2;
pub mod procfs;
pub mod protection;
pub mod realm;
pub mod scheduler;
pub mod scheduler_domains;
pub mod security;
pub mod fiber;
pub mod ksync;
pub mod sense;
pub mod socket;
pub mod sync;
pub mod sysctl;
pub mod mutex2;
pub mod time;
pub mod tty;
pub mod user;
pub mod llm;
pub mod debug;
pub mod shell;
pub mod syscall;
pub mod syscall_handler;
pub mod verification;
pub mod vfs;
pub mod loader;
pub mod version;
pub mod irq;
pub mod mm;
pub mod file_channel;
pub mod worktree;

// —— Subagent-A 新增：内核核心与通道系统扩充 ——
pub mod channel_ext;
pub mod capability;
pub mod event;
pub mod permission;
pub mod shell_parser;
pub mod buddy_alloc;
pub mod slab_alloc;
pub mod page_cache;
pub mod swap_sim;
pub mod page_table_sim;
pub mod multicore_sched;
pub mod async_runtime;
pub mod signal;
pub mod ipc_ext;

// —— P0 路线图：内核基础设施 ——
pub mod process;       // P0-2 进程隔离与 PCB
pub mod fs;            // P0-3 文件系统 VFS
pub mod block;         // P0-4 块设备层
pub mod simplefs;      // P0-4 简单文件系统

// —— P1 路线图：重要组件 ——
pub mod drivers;       // P1-2 设备驱动框架
pub mod smp;           // P1-3 多核 SMP 支持
pub mod graphics;      // P1-4 图形输出（GPU Primacy 基础）
pub mod syscall_entry; // P1-5 系统调用入口（sense/act）

use ai_token::{ActionProcess, AIConsciousness, ComputeGraph, GraphOpType, TokenCreateRequest};
use scheduler::{CpuPriority, GpuScheduler};
use syscall::{AiSystemCall, AiSystemCallResult};
use syscall_handler::SyscallHandler;

#[allow(dead_code)]
fn main() {
    use std::io::IsTerminal;

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--demo") => kernel_main(),
        Some("--shell") => shell::Shell::new().run(),
        // 支持 `cargo run --features gui -- gui run` 直接打开真实窗口。
        Some("gui") => {
            let line = args.join(" ");
            let mut sh = shell::Shell::new();
            sh.dispatch(&line);
        }
        Some("--help") | Some("-h") => print_usage(),
        Some(other) => {
            eprintln!("未知参数: {} (使用 --help 查看用法)", other);
            print_usage();
        }
        None => {
            // 有 TTY 时进入交互 shell，否则跑一遍演示 (适合无终端环境)。
            if std::io::stdin().is_terminal() {
                shell::Shell::new().run();
            } else {
                kernel_main();
            }
        }
    }
}

#[allow(dead_code)]
fn print_usage() {
    println!("Hib OS —— GPU-Centric Macro-Kernel Prototype");
    println!("用法:");
    println!("  hibos            自动选择 (TTY→shell, 否则→demo)");
    println!("  hibos --demo     运行一次完整启动/调度演示");
    println!("  hibos --shell    启动交互式 syscall shell");
    println!("  hibos --help     显示本帮助");
}

/// 初始化全部内核子系统。
pub fn boot_subsystems() {
    gpu::init();
    ai_token::init();
    ai_os::init();
    monitor::init();
    scheduler::init();
    syscall::init();
    verification::init();
    capability::caps_impl::register_all_capabilities();
    // GUI 订阅 EventBus：Shell / Agent 通过能力层产生的状态变化会实时推送到界面。
    gui::init_event_subscription();
}

/// 内核入口: 初始化各子系统并运行一次演示调度。
pub fn kernel_main() {
    println!("==================================================");
    println!("  Hib OS  —— GPU-Centric Macro-Kernel (prototype)");
    println!("==================================================");

    // ---------- 1. 子系统初始化 ----------
    boot_subsystems();
    let mut module_manager = modules::KernelModuleManager::new();

    // ---------- 2. AI 意识本体 (概念图) ----------
    let mut consciousness = AIConsciousness::new();
    consciousness.add_common_concepts();
    consciousness.add_extended_concepts();
    println!(
        "[ai] 概念本体已加载: {} 个概念",
        consciousness.concept_count()
    );

    // ---------- 3. 调度器 (语义感知) ----------
    let mut scheduler = GpuScheduler::new(consciousness.clone());
    println!(
        "[sched] 已初始化, 时间片 = {} ns",
        scheduler::TIME_SLICE_NS
    );

    // ---------- 4. Token 创建演示 ----------
    let req = TokenCreateRequest::new(
        "llama-13b",
        &[gpu::GpuCapability::Compute, gpu::GpuCapability::Tensor],
        2 * 1024 * 1024 * 1024,
        CpuPriority::High,
    );
    match ai_token::create_token_external(&req) {
        Ok(token) => {
            println!(
                "[token] 已创建 {} model={} caps=0x{:02x}",
                token.token_id,
                token.model_id,
                token.capabilities.bits()
            );
            let concept = consciousness.search_by_name("Transformer_Attention_Base");
            scheduler.enqueue(
                token.token_id,
                token.priority,
                concept.map(|c| c.id),
            );
        }
        Err(e) => println!("[token] 创建失败: {}", e),
    }

    // ---------- 5. 行动进程演示 ----------
    let concept_id = consciousness
        .search_by_name("Transformer_Attention_Base")
        .map(|c| c.id)
        .expect("内置概念应当存在");

    let mut graph = ComputeGraph::new();
    let a = graph.add_node(GraphOpType::MatMul);
    let b = graph.add_node(GraphOpType::Attention);
    graph.add_edge(a, b, 4 * 1024);

    match ActionProcess::new(&consciousness, concept_id, graph, 512 * 1024 * 1024) {
        Ok(mut process) => {
            match process.submit(&mut scheduler) {
                Ok(()) => println!(
                    "[proc] 提交成功 {} state={} priority={}",
                    process.pid,
                    process.state.name(),
                    process.priority.name()
                ),
                Err(e) => println!("[proc] 提交失败: {}", e),
            }
            let result = process.complete(Some(vec![0u8; 16]));
            println!(
                "[proc] 完成 success={} elapsed={}ns",
                result.success, result.execution_time_ns
            );
        }
        Err(e) => println!("[proc] 创建失败: {}", e),
    }

    // ---------- 6. 系统调用演示 ----------
    let mut handler = SyscallHandler::new(consciousness);
    let policy_call = AiSystemCall::QuerySchedulePolicy { concept_id };
    let AiSystemCallResult {
        success,
        schedule_policy,
        ..
    } = handler.handle(policy_call);
    println!(
        "[syscall] QuerySchedulePolicy success={} policy={:?}",
        success, schedule_policy
    );

    let load_call = AiSystemCall::SubmitLoad {
        token_id: ai_token::TokenId(1),
        program_id: 42,
        data_ptr: 0x1000,
    };
    println!("[syscall] {} -> {}", load_call.name(), handler.handle(load_call).success);

    // ---------- 7. 模块加载演示 ----------
    let entry = modules::ModuleEntry {
        name: "demo-gpu-driver".to_string(),
        version: modules::Version::new(
            crate::version::VERSION_MAJOR,
            crate::version::VERSION_MINOR,
            crate::version::VERSION_PATCH,
        ),
        init: || Ok(()),
        cleanup: || {},
    };
    if let Err(e) = module_manager.load_module(entry, modules::ModuleProtocol::GpuDriver) {
        println!("[module] 加载失败: {}", e);
    }
    println!("[module] 已加载模块数: {}", module_manager.loaded_count());
    let _ = module_manager.unload_module("demo-gpu-driver");

    // ---------- 8. 调度器状态 ----------
    let state = scheduler.get_state();
    println!(
        "[sched] 排队数={} 运行中={:?} 就绪数={}",
        state.enqueued_count, state.running_token, scheduler.ready_count()
    );
    println!("[sched] 下一个: {:?}", scheduler.pick_next());

    // ---------- 9. 系统统计 ----------
    let stats = monitor::get_stats();
    println!(
        "[monitor] tokens={} completed={} failed={} uptime={}ns",
        stats.total_tokens, stats.total_tasks_completed, stats.total_tasks_failed, stats.system_uptime_ns
    );

    println!("==================================================");
    println!("  Hib OS 启动序列完成");
    println!("==================================================");
}
