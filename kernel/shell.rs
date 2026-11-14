//! 交互式 syscall shell —— 把内核能力暴露为可操作命令。
//!
//! 通过标准输入读取命令，直接在宏内核上调用 `AiSystemCall`。
//! 在终端下为真 REPL (↑↓ 历史 / Tab 补全 / 光标编辑)，
//! 在管道下自动降级为逐行读取 (`echo help | hibos --shell`)。

use std::io;
use std::io::IsTerminal;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ai_token::{AIConsciousness, ComputeGraph, GraphOpType, TokenId};
use crate::channels::{ChannelKind, ChannelRegistry};
use crate::desktop::{self, DesktopInfo};
use crate::gpu::{GpuCapability, GpuCapabilities, IommuFlags};
use crate::gui;
use crate::line_editor::{self, Completion, LineEditor, ReadOutcome};
use crate::modules::{KernelModuleManager, ModuleEntry, ModuleProtocol, Version};
use crate::scheduler::CpuPriority;
use crate::syscall::AiSystemCall;
use crate::syscall_handler::SyscallHandler;
use crate::vfs::{NodeKind, Vfs};

/// 闭环实例序号：每个 AI 意识实例（loop）分配一个独立 worktree。
static LOOP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 终端主题：极简 ANSI 配色（仅真实终端生效，管道/文件输出自动去色）。
///
/// 让 "Linux 模式" 拥有更地道的外观：systemd 风彩色 `[  OK  ]` 启动行、经典彩色 prompt。
#[allow(dead_code)]
mod theme {
    use std::io::IsTerminal;

    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const GREEN: &str = "\x1b[32m";
    const RED: &str = "\x1b[31m";
    const YELLOW: &str = "\x1b[33m";
    const BLUE: &str = "\x1b[34m";
    const CYAN: &str = "\x1b[36m";
    const GREY: &str = "\x1b[90m";

    /// 真实终端（stdout 是 tty）才上色，避免管道/重定向输出出现乱码转义符。
    fn enabled() -> bool {
        std::io::stdout().is_terminal()
    }

    /// 仅终端时上色，否则原样返回（保证管道输出干净）。
    pub fn paint(s: &str, code: &str) -> String {
        if enabled() {
            format!("{}{}{}", code, s, RESET)
        } else {
            s.to_string()
        }
    }

    pub fn bold(s: &str) -> String { paint(s, BOLD) }
    pub fn green(s: &str) -> String { paint(s, GREEN) }
    pub fn red(s: &str) -> String { paint(s, RED) }
    pub fn yellow(s: &str) -> String { paint(s, YELLOW) }
    pub fn blue(s: &str) -> String { paint(s, BLUE) }
    pub fn cyan(s: &str) -> String { paint(s, CYAN) }
    pub fn grey(s: &str) -> String { paint(s, GREY) }

    /// systemd 风启动行：`[  OK  ] Started <service>`。
    pub fn ok(service: &str) -> String {
        format!("{} Started {}", green("[  OK  ]"), service)
    }

    /// 经典 Linux 风 prompt：`root@hibos:<path># `（绿主机 + 蓝路径）。
    pub fn prompt(cwd: &str) -> String {
        let host = green("root@hibos");
        let path = blue(if cwd.is_empty() || cwd == "/" { "~" } else { cwd });
        format!("{}:{}# ", host, path)
    }
}

/// 带方括号标识的输出：**方括号内的标识用绿色，方括号外的正文保持终端默认色**。
///
/// 例：`tagged_out!("[llm]", "Provider '{}' => {}", provider, masked)`
macro_rules! tagged_out {
    ($tag:expr, $($body:tt)*) => {
        println!("{} {}", theme::green($tag), format!($($body)*))
    };
}

/// REPL 可补全的命令名。
pub const COMMANDS: &[&str] = &[
    "help",
    "channels",
    "sense",
    "act",
    "concepts",
    "policy",
    "token",
    "query",
    "prio",
    "proc",
    "run",
    "queue",
    "stats",
    "desktop",
    "gui",
    "pwd",
    "ls",
    "cd",
    "cat",
    "write",
    "append",
    "mkdir",
    "touch",
    "rm",
    "tree",
    "df",
    "fbpreview",
    "save",
    "load",
    "resetfs",
    "wallpaper",
    "deepseek",
    "llm",
    "modules",
    "modload",
    "modunload",
    "history",
    "exit",
    "quit",
    "agent",
    "task",
    "capability",
    "system",
    "app",
];

/// 交互式内核外壳。
pub struct Shell {
    pub handler: SyscallHandler,
    pub modules: KernelModuleManager,
    pub vfs: Vfs,
    pub channels: ChannelRegistry,
    pub editor: LineEditor,
    /// driver/gpu 累计预留的显存字节。
    pub gpu_reserved_bytes: u64,
    /// 当前桌面壁纸名称。
    pub wallpaper: String,
}

impl Shell {
    pub fn new() -> Self {
        let mut consciousness = AIConsciousness::new();
        consciousness.add_common_concepts();
        consciousness.add_extended_concepts();

        // 从环境变量加载 API Key（不使用 .env 文件）。
        // DEEPSEEK_API_KEY 为兼容旧名；通用形式为 HIBOS_LLM_<PROVIDER>_KEY。
        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            if !key.is_empty() {
                consciousness.set_deepseek_key(key);
            }
        }
        for p in crate::llm::known_providers() {
            let env_key = format!("HIBOS_LLM_{}_KEY", p.name.to_ascii_uppercase());
            if let Ok(key) = std::env::var(&env_key) {
                if !key.is_empty() {
                    consciousness.set_api_key(&p.name, key);
                }
            }
        }

        Self {
            handler: SyscallHandler::new(consciousness),
            modules: KernelModuleManager::new(),
            vfs: Vfs::new(),
            channels: crate::channels::bootstrap(),
            editor: LineEditor::new(),
            gpu_reserved_bytes: 0,
            wallpaper: "default".to_string(),
        }
    }

    /// 提示符 (经典 Linux 风格，彩色): `root@hibos:~# `。
    pub fn prompt(&self) -> String {
        theme::prompt(&self.vfs.cwd_path())
    }

    /// 启动交互循环 (EOF / Ctrl+D / `exit` 结束)。
    ///
    /// 极简 Linux 风启动画面（彩色；管道下自动去色）。
    fn linux_boot(&self) {
        println!(
            "{}",
            theme::bold(concat!(
                "Hib OS ",
                crate::version::hibos_version!(),
                " (linux)"
            ))
        );
        println!(
            "{} hibos: GPU-Centric macro-kernel (by logos-42, MIT)",
            theme::grey("[    0.000000 ]")
        );
        println!("{}", theme::ok("GPU 显存 / IOMMU"));
        println!("{}", theme::ok("GPU 调度器"));
        println!("{}", theme::ok("系统调用门闸 (10 种调用)"));
        println!("{}", theme::ok("形式化验证框架"));
        println!("{}", theme::ok("虚拟文件系统 (VFS)"));
    }

    /// 默认进入 LLM 闭环循环模式 (loop)：AI 自动感知系统状态、思考、执行行动。
    /// 如未配置 API Key，则退回到传统命令行 shell 模式。
    pub fn run(&mut self) {
        crate::boot_subsystems();

        let state = Vfs::default_state_path();
        match Vfs::load_from_path(&state) {
            Ok(Some(v)) => {
                self.vfs = v;
            }
            Ok(None) => {}
            Err(_) => {}
        }
        // 把挂载表物化进目录树：/proc、/sys、/dev/gpu 等在 ls 里可见。
        self.materialize_all_mounts();

        let providers = self.configured_providers();

        if !providers.is_empty() {
            // 极简启动（无大横幅、无子系统噪声日志）。
            println!("Hib OS — AI OS 已就绪 (exit 退出)");
            self.run_llm_loop(&providers);
            return;
        }

        // 传统 shell 模式才输出启动画面；AI 模式保持极简。
        self.linux_boot();

        println!("Hib OS — interactive shell ready (help / exit)");
        if io::stdin().is_terminal() {
            println!("↑↓ 历史, Tab 补全, ←→ 移动光标");
            println!("设置环境变量 HIBOS_LLM_<PROVIDER>_KEY（兼容 DEEPSEEK_API_KEY）可启用 AI 闭环模式");
        }

        loop {
            let prompt = self.prompt();
            // 借用分离: 编辑器独立持有 history，补全器只读访问 self.vfs。
            let mut editor = std::mem::replace(&mut self.editor, LineEditor::new());
            let outcome = editor.read_line(&prompt, |input| self.completion(input));
            self.editor = editor;
            match outcome {
                Ok(ReadOutcome::Line(line)) => {
                    if !self.dispatch(&line) {
                        break;
                    }
                }
                Ok(ReadOutcome::Cancelled) => continue,
                Ok(ReadOutcome::Eof) | Err(_) => break,
            }
        }

        match self.vfs.save_to_path(&state) {
            Ok(()) => println!("[vfs] 状态已保存到 {}", state.display()),
            Err(e) => println!("[vfs] 保存失败: {}", e),
        }
        println!("[shell] 再见。");
    }

    /// Tab 补全(含参数提示)。
    pub fn completion(&self, input: &str) -> Completion {
        let items = self.complete(input);
        let hint = self.hint_for(input);
        Completion { items, hint }
    }

    /// 首个 token 为已知命令且已进入参数位置时，给出 `--help` 式提示。
    fn hint_for(&self, input: &str) -> Option<String> {
        let trimmed = input.trim_end();
        let mut words = trimmed.split_whitespace();
        let cmd = words.next()?;
        let in_args = words.next().is_some() || input.ends_with(char::is_whitespace);
        if !in_args {
            return None;
        }
        let usage = usage_line(cmd)?;
        let desc = describe(cmd).unwrap_or("");
        Some(format!("{}   —   {}", usage, desc))
    }

    /// Tab 补全：首个 token 补命令名，其余补路径。
    pub fn complete(&self, input: &str) -> Vec<String> {
        let text = input.trim_start();
        if !text.contains(char::is_whitespace) {
            let mut out: Vec<String> = COMMANDS
                .iter()
                .filter(|c| c.starts_with(text))
                .map(|c| c.to_string())
                .collect();
            if !text.is_empty() {
                for p in self.vfs.complete_path(text) {
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
            }
            out
        } else {
            let token = line_editor::last_token(input);
            if token.is_empty() {
                Vec::new()
            } else {
                self.vfs.complete_path(&token)
            }
        }
    }

    /// 处理一条命令；返回 false 表示退出。
    pub fn dispatch(&mut self, line: &str) -> bool {
        // 兼容带 BOM / 前后空白的输入 (例如管道输入)。
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            return true;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "help" | "?" => self.cmd_help(parts.get(1).copied()),
            "channels" => self.cmd_channels(),
            "sense" => self.cmd_sense(&parts),
            "act" => self.cmd_act(&parts),
            "concepts" => self.cmd_concepts(parts.get(1).copied()),
            "policy" => self.cmd_policy(parts.get(1).copied()),
            "token" => {
                self.cmd_token(&parts);
            }
            "query" => self.cmd_query(&parts),
            "prio" => self.cmd_prio(&parts),
            "proc" => self.cmd_proc(&parts),
            "run" => self.cmd_run(),
            "queue" => println!("就绪队列: {} 个任务", self.handler.scheduler.ready_count()),
            "stats" => self.cmd_stats(),
            "desktop" => self.cmd_desktop(&parts),
            "gui" => self.cmd_gui(&parts),
            "pwd" => println!("{}", self.vfs.cwd_path()),
            "ls" => self.cmd_ls(parts.get(1).copied()),
            "cd" => self.cmd_cd(parts.get(1).copied()),
            "cat" => self.cmd_cat(parts.get(1).copied()),
            "write" => self.cmd_write(&parts, false),
            "append" => self.cmd_write(&parts, true),
            "mkdir" => self.cmd_mkdir(parts.get(1).copied()),
            "touch" => self.cmd_touch(parts.get(1).copied()),
            "rm" => self.cmd_rm(parts.get(1).copied()),
            "tree" => print!("{}", self.vfs.tree()),
            "df" => self.cmd_df(),
            "fbpreview" => self.cmd_fbpreview(&parts),
            "save" => self.cmd_save(),
            "load" => self.cmd_load(),
            "resetfs" => self.cmd_resetfs(),
            "wallpaper" => self.cmd_wallpaper(&parts),
            "deepseek" => self.cmd_llm(&parts),
            "llm" => self.cmd_llm(&parts),
            "modules" => self.cmd_modules(),
            "modload" => self.cmd_modload(&parts),
            "modunload" => self.cmd_modunload(&parts),
            "history" => self.cmd_history(),
            "exit" | "quit" => return false,
            "agent" => self.cmd_agent(&parts),
            "task" => self.cmd_task(&parts),
            "capability" => self.cmd_capability(&parts),
            "system" => self.cmd_system(&parts),
            "app" => self.cmd_app(&parts),
            other => println!("未知命令: {} (试试 help)", other),
        }
        true
    }

    // ------------------------------------------------------------------
    // 命令实现：内核
    // ------------------------------------------------------------------

    fn cmd_help(&self, topic: Option<&str>) {
        if let Some(cmd) = topic {
            match (usage_line(cmd), describe(cmd)) {
                (Some(usage), Some(desc)) => {
                    println!("用法: {}", usage);
                    println!("说明: {}", desc);
                }
                _ => println!("没有关于 '{}' 的帮助 (试试 help)", cmd),
            }
            return;
        }
        println!("可用命令 (help <命令> 查看详细用法):");
        println!("统一感知层:");
        println!("  channels                 列出全部感知通道");
        println!("  sense <通道>             通过通道感知 (context/file/gpu/action ...)");
        println!("  act <通道> [负载...]     通过通道行动");
        println!("内核 / AI:");
        println!("  concepts [过滤]          列出概念本体 (可选名称过滤)");
        println!("  policy <概念名>          查询概念的调度策略");
        println!("  token <模型> [MiB] [prio] 创建 AI Action Token (默认 512 MiB / medium)");
        println!("  query <token_id>         查询 Token 状态");
        println!("  prio <token_id> <级别>   调整 Token 优先级 (low|medium|high|critical)");
        println!("  proc <概念名> [MiB]      创建并提交一个 Action Process");
        println!("  run                      调度并完成队首任务");
        println!("  queue                    查看就绪队列长度");
        println!("  stats                    系统统计");
        println!("  llm <provider>           LLM 多提供商对话 (deepseek 等)");
        println!("  llm key set <provider> <key>   设置 LLM Provider API Key");
        println!("  llm key del <provider>           删除 LLM Provider API Key");
        println!("  llm key list / show <provider>   查看已配置 Provider");
        println!("  deepseek                 llm 命令别名 (深度求索)");
        println!("  desktop [--pixels]       文本桌面面板 / 像素帧缓冲缩略图");
        println!("文件系统:");
        println!("  pwd / ls [路径] / cd [路径] / cat <路径>");
        println!("  write <路径> <内容...> / append <路径> <内容...>");
        println!("  mkdir <路径> / touch <路径> / rm <路径> / tree / df");
        println!("  save / load / resetfs    持久化 (JSON) 保存 / 载入 / 重置");
        println!("模块:");
        println!("  modules / modload <名称> / modunload <名称>");
        println!("Agent / 任务 / 能力 / 系统:");
        println!("  agent list / inspect <id> / start <id> / stop <id>");
        println!("  task create <描述> [--agent <id>] / list / inspect <id> / cancel <id> / pause <id> / resume <id>");
        println!("  capability list / inspect <id>");
        println!("  system status / resources / logs");
        println!("  app list / launch <名> / stop <id>");
        println!("其他:");
        println!("  history                  查看命令历史");
        println!("  exit                     退出");
    }

    fn cmd_concepts(&self, filter: Option<&str>) {
        let mut count = 0;
        for node in self.handler.consciousness.concepts() {
            let matched = match filter {
                Some(f) => node.name.to_ascii_lowercase().contains(&f.to_ascii_lowercase()),
                None => true,
            };
            if matched {
                count += 1;
                println!(
                    "  {:<30} prio={:.2} policy={:<14} vram={}MiB",
                    node.name,
                    node.priority,
                    node.schedule_policy.name(),
                    node.estimated_vram / (1024 * 1024)
                );
            }
        }
        println!("共 {} 个概念", count);
    }

    fn cmd_policy(&self, name: Option<&str>) {
        let Some(name) = name else {
            println!("用法: policy <概念名>");
            return;
        };
        match self.handler.consciousness.search_by_name(name) {
            Some(node) => {
                let policy = self
                    .handler
                    .consciousness
                    .find_schedule_policy(&node.compute_pattern)
                    .unwrap_or(crate::ai_token::SchedulePolicy::Batch);
                println!(
                    "{} → 策略={} 优先级={}",
                    node.name,
                    policy.name(),
                    node.priority
                );
            }
            None => println!("未找到概念: {}", name),
        }
    }

    fn cmd_token(&mut self, parts: &[&str]) -> Option<TokenId> {
        let Some(model) = parts.get(1) else {
            println!("用法: token <模型> [MiB] [low|medium|high|critical]");
            return None;
        };
        let vram_mib: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(512);
        let priority = parts
            .get(3)
            .and_then(|s| parse_priority(s))
            .unwrap_or(CpuPriority::Medium);

        let caps = GpuCapabilities::from_caps(&[GpuCapability::Compute, GpuCapability::Tensor]);
        let result = self.handler.handle(AiSystemCall::CreateToken {
            model_id: model.to_string(),
            capabilities: caps,
            vram_size: vram_mib * 1024 * 1024,
            priority,
            concept_id: None,
        });
        match (result.success, result.token_id) {
            (true, Some(id)) => {
                println!(
                    "已创建 token #{} ({} MiB, {})",
                    id.0,
                    vram_mib,
                    priority.name()
                );
                Some(id)
            }
            _ => {
                println!("创建失败: {:?}", result.error);
                None
            }
        }
    }

    fn cmd_query(&mut self, parts: &[&str]) {
        let Some(id) = parts.get(1).and_then(|s| s.parse::<u64>().ok()) else {
            println!("用法: query <token_id>");
            return;
        };
        let result = self
            .handler
            .handle(AiSystemCall::QueryToken { token_id: TokenId(id) });
        match result.token_status {
            Some(st) => println!(
                "#{} model={} valid={} caps=0x{:02x} prio={}",
                st.token_id.0,
                st.model_id,
                st.is_valid,
                st.capabilities.bits(),
                st.priority.name()
            ),
            None => println!("查询失败: {:?}", result.error),
        }
    }

    fn cmd_prio(&mut self, parts: &[&str]) {
        let (Some(id), Some(level)) = (
            parts.get(1).and_then(|s| s.parse::<u64>().ok()),
            parts.get(2),
        ) else {
            println!("用法: prio <token_id> <low|medium|high|critical>");
            return;
        };
        let Some(prio) = parse_priority(level) else {
            println!("非法优先级: {}", level);
            return;
        };
        let result = self.handler.handle(AiSystemCall::SetPriority {
            token_id: TokenId(id),
            new_priority: prio,
        });
        if result.success {
            println!("token #{} 优先级 → {}", id, prio.name());
        } else {
            println!("失败: {:?}", result.error);
        }
    }

    fn cmd_proc(&mut self, parts: &[&str]) {
        let Some(name) = parts.get(1) else {
            println!("用法: proc <概念名> [MiB]");
            return;
        };
        let Some(node) = self.handler.consciousness.search_by_name(name) else {
            println!("未找到概念: {}", name);
            return;
        };
        let concept_id = node.id;
        let concept_name = node.name.clone();
        let priority = self.handler.consciousness.priority_class(concept_id);
        let vram_mib: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(128);

        let mut graph = ComputeGraph::new();
        let a = graph.add_node(GraphOpType::MatMul);
        let b = graph.add_node(GraphOpType::Reduce);
        graph.add_edge(a, b, 4096);

        let result = self.handler.handle(AiSystemCall::CreateProcess {
            ontology_id: concept_id,
            compute_graph: graph,
            vram_size: vram_mib * 1024 * 1024,
            priority,
        });
        match (result.success, result.process_id) {
            (true, Some(pid)) => println!(
                "已提交进程 #{} (概念={}, {} MiB)",
                pid.0, concept_name, vram_mib
            ),
            _ => println!("创建进程失败: {:?}", result.error),
        }
    }

    fn cmd_run(&mut self) {
        let Some(id) = self.handler.dispatch_next() else {
            println!("就绪队列为空");
            return;
        };
        if self.handler.processes.get(id).is_some() {
            let result = self.handler.handle(AiSystemCall::CompleteProcess {
                process_id: id,
                success: true,
                output: None,
            });
            if let Some((_, state, Some(res))) = result.process_status {
                println!(
                    "进程 #{} 执行完成: state={} elapsed={}ns",
                    id.0,
                    state.name(),
                    res.execution_time_ns
                );
            }
        } else {
            println!("调度 token #{} (无进程体, 空转完成)", id.0);
        }
    }

    fn cmd_stats(&self) {
        let s = crate::monitor::get_stats();
        println!(
            "tokens={} (active={}) tasks: {} 完成 / {} 失败 avg={}ns vram={}B uptime={}ns",
            s.total_tokens,
            s.active_tokens,
            s.total_tasks_completed,
            s.total_tasks_failed,
            s.avg_task_time_ns,
            s.total_vram_used,
            s.system_uptime_ns
        );
    }

    /// 组装桌面面板数据。
    fn desktop_info(&self) -> DesktopInfo {
        let s = crate::monitor::get_stats();
        let (dirs, files) = self.vfs.node_count();
        let vram = crate::gpu::vram().lock().unwrap();
        let total = vram.total_vram();
        let free = vram.available();
        let partitions = vram.partition_count();
        drop(vram);
        let gib = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);

        let net_state = crate::net::net().lock();
        let panel_net = if net_state.iface.link_up {
            format!("eth0 {} up", crate::net::fmt_ip(net_state.iface.ip))
        } else {
            "eth0 down".to_string()
        };
        drop(net_state);

        DesktopInfo {
            cwd: self.vfs.cwd_path(),
            concepts: self.handler.consciousness.concept_count(),
            tokens: s.total_tokens,
            active_tokens: s.active_tokens,
            tasks_ok: s.total_tasks_completed,
            tasks_fail: s.total_tasks_failed,
            ready: self.handler.scheduler.ready_count(),
            vram_used_gib: gib(total - free),
            vram_total_gib: gib(total),
            partitions,
            dirs,
            files,
            modules: self.modules.loaded_count(),
            uptime_ms: s.system_uptime_ns as f64 / 1e6,
            channels: self
                .channels
                .list()
                .iter()
                .map(|c| (c.name.clone(), c.kind.name().to_string(), c.senses + c.acts))
                .collect(),
            panel_time: chrono_now_hms(),
            panel_net,
            fb_scroll_y: 0,
            fb_scroll_x: 0,
            wallpaper: self.wallpaper.clone(),
            apps: Vec::new(),
        }
    }

    /// 文本桌面 / 像素帧缓冲。
    fn cmd_desktop(&mut self, parts: &[&str]) {
        let _ = self.channels.act("desktop/screen");
        let mut info = self.desktop_info();

        // 解析滚动参数：--scroll-y N / --scroll-x N
        let mut i = 0;
        while i < parts.len() {
            match parts[i] {
                "--scroll-y" => {
                    if let Some(v) = parts.get(i + 1).and_then(|s| s.parse().ok()) {
                        info.fb_scroll_y = v;
                        let _ = self.channels.sense("desktop/fb-scroll-y");
                        // 覆盖 senses 为滚动值（hack：channels 只有计数器，这里仅作持久化提示）
                        let _ = v;
                    }
                    i += 2;
                }
                "--scroll-x" => {
                    if let Some(v) = parts.get(i + 1).and_then(|s| s.parse().ok()) {
                        info.fb_scroll_x = v;
                        let _ = self.channels.sense("desktop/fb-scroll-x");
                        let _ = v;
                    }
                    i += 2;
                }
                _ => i += 1,
            }
        }

        let pixels = parts.iter().any(|p| *p == "--pixels");
        if pixels {
            desktop::refresh_framebuffer(&info);
            let (w, h, bytes, lit) = desktop::framebuffer_stats();
            println!(
                "帧缓冲 {}x{} ({} 字节, {} 点亮像素) —— 桌面合成后光栅化",
                w, h, bytes, lit
            );
            print!("{}", desktop::framebuffer_preview(76, 20));
        } else {
            // 先把本帧桌面光栅化进帧缓冲，供“ FRAMEBUFFER PREVIEW ”面板显示
            desktop::refresh_framebuffer(&info);
            print!("{}", desktop::render(&info));
            println!();
        }
    }

    // ------------------------------------------------------------------
    // 命令实现：真 GUI
    // ------------------------------------------------------------------

    fn cmd_gui(&mut self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("desktop");
        let rest = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
        gui::ensure_demo();
        match sub {
            "status" | "info" => {
                println!("{}", gui::status());
            }
            "desktop" | "preview" => {
                println!("{}", gui::status());
                print!("{}", gui::preview(96, 26));
            }
            "shot" | "save" => {
                let path = if rest.is_empty() { "gui-output.bmp".to_string() } else { rest };
                match gui::screenshot(&path) {
                    Ok(msg) => println!("{}", msg),
                    Err(e) => println!("{}", e),
                }
            }
            "run" => match gui::run_native() {
                Ok(()) => {
                    println!("真实窗口已关闭");
                    let cmds = gui::take_commands();
                    if !cmds.is_empty() {
                        println!("窗口内输入的命令 ({} 条):", cmds.len());
                    }
                    for line in cmds {
                        println!("[gui] > {}", line);
                        if !line.trim().is_empty() && !line.trim().starts_with("gui") {
                            self.dispatch(&line);
                        }
                    }
                }
                Err(e) => println!("{}", e),
            },
            "wallpaper" | "wp" => match gui::wallpaper::act(&rest) {
                Ok(msg) => println!("{}", msg),
                Err(e) => println!("gui wallpaper 失败: {}", e),
            },
            "app" | "launch" => match gui::open_app(&rest) {
                Ok(id) => println!("已启动应用 {} -> win#{}", rest, id),
                Err(e) => println!("{}", e),
            },
            "taskbar" => {
                let mut p = rest.split_whitespace();
                let xy = (
                    p.next().and_then(|s| s.parse::<i32>().ok()),
                    p.next().and_then(|s| s.parse::<i32>().ok()),
                );
                match xy {
                    (Some(x), Some(y)) => println!(
                        "任务栏点击 ({},{}) -> {:?}",
                        x,
                        y,
                        gui::taskbar_click(x, y)
                    ),
                    _ => println!("用法: gui taskbar <x> <y>"),
                }
            }
            "open" | "term" | "terminal" => {
                let title = if rest.is_empty() { "TERMINAL".to_string() } else { rest };
                let id = gui::open_terminal(&title, 420, 280);
                println!("已打开终端窗口 win#{} ({})", id, title);
            }
            "close" => match rest.parse::<u64>() {
                Ok(id) => println!("{}", if gui::close_window(id) { format!("已关闭 win#{}", id) } else { format!("win#{} 不存在", id) }),
                Err(_) => println!("用法: gui close <win-id>"),
            },
            "focus" => match rest.parse::<u64>() {
                Ok(id) => println!("{}", if gui::focus_window(id) { format!("已聚焦 win#{}", id) } else { format!("win#{} 不存在", id) }),
                Err(_) => println!("用法: gui focus <win-id>"),
            },
            "move" => {
                let mut p = rest.split_whitespace();
                let id = p.next().and_then(|s| s.parse::<u64>().ok());
                let x = p.next().and_then(|s| s.parse::<i32>().ok());
                let y = p.next().and_then(|s| s.parse::<i32>().ok());
                match (id, x, y) {
                    (Some(id), Some(x), Some(y)) => println!(
                        "{}",
                        if gui::move_window(id, x, y) {
                            format!("win#{} -> ({}, {})", id, x, y)
                        } else {
                            format!("win#{} 不存在", id)
                        }
                    ),
                    _ => println!("用法: gui move <win-id> <x> <y>"),
                }
            }
            "click" => {
                let nums: Vec<i32> = rest.split_whitespace().filter_map(|s| s.parse().ok()).collect();
                match nums.as_slice() {
                    [x, y, ..] => {
                        let eff = gui::click(*x, *y);
                        println!("点击 ({}, {}) -> {:?}", x, y, eff);
                    }
                    _ => println!("用法: gui click <x> <y>"),
                }
            }
            "key" => {
                let text = rest.replace("\\n", "\n").replace("\\b", "\u{8}");
                let effects = gui::key_text(&text);
                for e in &effects {
                    if let gui::input::Effect::Command { line, window } = e {
                        println!("[win#{}] 执行命令: {}", window, line);
                        if !line.trim().is_empty() && !line.trim().starts_with("gui") {
                            self.dispatch(&line);
                            gui::app_output(*window, &format!("内核已执行: {}", line));
                        }
                    }
                }
                println!("已向焦点窗口输入 {} 字符", text.chars().count());
            }
            "text" => {
                if rest.is_empty() {
                    println!("用法: gui text <文本...>");
                } else {
                    match gui::render_text_bitmap(&rest, "gui-text.bmp") {
                        Ok(msg) => println!("{}", msg),
                        Err(e) => println!("{}", e),
                    }
                }
            }
            _ => println!(
                "用法: gui [desktop|shot [路径]|run|open <标题>|close <id>|focus <id>|move <id> <x> <y>|click <x> <y>|key <文本>|text <文本>]"
            ),
        }
    }

    // ------------------------------------------------------------------
    // 命令实现：统一感知通道
    // ------------------------------------------------------------------

    fn cmd_channels(&self) {
        println!("感知通道 ({} 个) —— AI 的感官与行动接口:", self.channels.len());
        println!(
            "  {:<32} {:<10} {:<4} {:>5} {:>5}",
            "通道", "类别", "权限", "感知", "行动"
        );
        for c in self.channels.list() {
            println!(
                "  {:<32} {:<10} {:<4} {:>5} {:>5}",
                c.name,
                c.kind.name(),
                c.access.label(),
                c.senses,
                c.acts
            );
        }
        println!(
            "  总计: 感知 {} 次 / 行动 {} 次",
            self.channels.total_senses(),
            self.channels.total_acts()
        );
    }

    /// 已配置 API Key 的 provider 列表（deepseek 优先，其余按配置顺序）。
    /// 多个 provider 会**同时**运行各自的模型。
    fn configured_providers(&self) -> Vec<String> {
        let keys = self.handler.consciousness.api_keys();
        let mut out: Vec<String> = Vec::new();
        if let Some((p, _)) = keys.iter().find(|(p, _)| p == "deepseek") {
            out.push(p.clone());
        }
        for (p, _) in &keys {
            if p != "deepseek" {
                out.push(p.clone());
            }
        }
        out
    }

    /// 自造插件通道按需注册：插件安装后，其 `plugin/<name>` 通道即可被感知/行动。
    fn ensure_dynamic_channel(&mut self, name: &str) -> Option<ChannelKind> {
        if !name.starts_with("plugin/") {
            return None;
        }
        let installed = {
            let reg = crate::plugin::plugins().lock().unwrap();
            reg.plugin_of_channel(name).is_some()
        };
        if !installed {
            return None;
        }
        let _ = self.channels.open(
            ChannelKind::Plugin,
            name,
            crate::channels::Access::ReadWrite,
        );
        Some(ChannelKind::Plugin)
    }

    /// worktree 创建后，在 VFS 里建出对应隔离根目录（让工作区真实可见）。
    fn sync_worktree_hooks(&mut self, payload: &str) {
        let name = payload.split_whitespace().nth(1).unwrap_or("").trim();
        if name.is_empty() {
            return;
        }
        let _ = self.vfs.mkdir("/worktrees");
        let _ = self.vfs.mkdir(&format!("/worktrees/{}", name));
    }

    /// 在 VFS 中建出指定的 worktree 根目录（含父目录），用于实例级工作区。
    fn sync_worktree_root(&mut self, root: &str) {
        let root = root.trim();
        if root.is_empty() || root == "/" {
            return;
        }
        if let Some(i) = root.rfind('/') {
            if i > 0 {
                let _ = self.vfs.mkdir(&root[..i]);
            }
        }
        let _ = self.vfs.mkdir(root);
    }

    fn cmd_sense(&mut self, parts: &[&str]) {
        let Some(name) = parts.get(1).copied() else {
            println!("用法: sense <通道>  (channels 查看全部通道)");
            return;
        };
        let kind = match self.channels.sense(name) {
            Ok(k) => k,
            Err(e) => match self.ensure_dynamic_channel(name) {
                Some(k) => k,
                None => {
                    println!("sense 失败: {}", e);
                    return;
                }
            },
        };
        let (probed, epoch) = crate::sense::probe_epoch(name);
        let text = probed.unwrap_or_else(|| self.sense_via_shell(kind, name));
        println!("sense {} [{}] epoch={} → {}", name, kind.name(), epoch, text);
    }

    fn sense_via_shell(&self, kind: ChannelKind, name: &str) -> String {
        match kind {
            ChannelKind::Context => format!(
                "concepts={} 概念图已加载",
                self.handler.consciousness.concept_count()
            ),
            ChannelKind::Action => format!(
                "ready={} slice=10ms syscalls=10",
                self.handler.scheduler.ready_count()
            ),
            ChannelKind::File => format!(
                "cwd={} dirs_files={:?}",
                self.vfs.cwd_path(),
                self.vfs.node_count()
            ),
            ChannelKind::Path => self.vfs.cwd_path(),
            ChannelKind::Text => self
                .vfs
                .cat("/var/log/kernel.log")
                .map(|s| s.lines().last().unwrap_or("(空)").to_string())
                .unwrap_or_else(|_| "(无日志)".to_string()),
            ChannelKind::Gateway => format!("shell gateway open ({} 条命令)", COMMANDS.len()),
            ChannelKind::Agent => format!(
                "orchestrator ready={} processes={}",
                self.handler.scheduler.ready_count(),
                self.handler.processes.len()
            ),
            ChannelKind::Desktop => format!(
                "{}x{} cells, framebuffer linked",
                desktop::COLS,
                desktop::ROWS
            ),
            ChannelKind::Io => "stdin/stdout attached".to_string(),
            ChannelKind::Net => "net/eth0 就绪".to_string(),
            ChannelKind::App => format!(
                "app 就绪: {} 条命令, {} 个模块, {} 个 token",
                COMMANDS.len(),
                self.modules.loaded_count(),
                crate::monitor::get_stats().total_tokens
            ),
            ChannelKind::Browser => {
                "浏览器通道离线 (无 HTML/DOM/网络栈), 网关已注册".to_string()
            }
            ChannelKind::Driver => {
                let names: Vec<String> = self
                    .modules
                    .list_loaded()
                    .iter()
                    .map(|n| n.to_string())
                    .collect();
                format!(
                    "已加载驱动/模块 {} 个: [{}]",
                    names.len(),
                    names.join(",")
                )
            }
            ChannelKind::Plugin => {
                let names: Vec<String> = self
                    .modules
                    .list_loaded()
                    .iter()
                    .map(|n| n.to_string())
                    .collect();
                format!(
                    "插件总线 {} 个模块: [{}]",
                    names.len(),
                    names.join(",")
                )
            }
            _ => format!("{} 就绪", name),
        }
    }

    fn cmd_act(&mut self, parts: &[&str]) {
        let Some(name) = parts.get(1).copied() else {
            println!("用法: act <通道> [负载...]");
            return;
        };
        let payload = parts[2..].join(" ");
        let kind = match self.channels.act(name) {
            Ok(k) => k,
            Err(e) => match self.ensure_dynamic_channel(name) {
                Some(k) => k,
                None => {
                    println!("act 失败: {}", e);
                    return;
                }
            },
        };
        if let Some(result) = crate::sense::act(name, &payload) {
            let ok = result.is_ok();
            match result {
                Ok(msg) => println!("act {} [{}] → {}", name, kind.name(), msg),
                Err(e) => println!("act {} 失败: {}", name, e),
            }
            // 挂载表是唯一需要同步到 VFS 的通道：成功挂载后立刻建目录 / 写伪文件。
            if ok && name == "mount/table" {
                self.sync_mount_hooks(&payload);
            }
            // worktree 创建后立刻在 VFS 里建出隔离根目录，让工作区真实可见。
            if ok && name == "worktree/trees" && payload.trim_start().starts_with("create") {
                self.sync_worktree_hooks(&payload);
            }
            return;
        }
        match kind {
            ChannelKind::File => {
                let line = format!("[{}] {}\n", name, payload);
                match self.vfs.append("/var/log/channel.log", &line) {
                    Ok(()) => println!("act {} → 已追加到 /var/log/channel.log", name),
                    Err(e) => println!("act 写入失败: {}", e),
                }
            }
            ChannelKind::Action => self.cmd_run(),
            ChannelKind::Context => {
                if payload.is_empty() {
                    println!("act context 需要概念名作为负载");
                } else {
                    let _ = self.dispatch(&format!("proc {}", payload));
                }
            }
            ChannelKind::Text => {
                let line = format!("[{}] {}\n", name, payload);
                match self.vfs.append("/var/log/kernel.log", &line) {
                    Ok(()) => println!("act {} → 已写入内核日志", name),
                    Err(e) => println!("act 写入失败: {}", e),
                }
            }
            ChannelKind::Desktop => {
                let info = self.desktop_info();
                print!("{}", desktop::render(&info));
                println!();
            }
            ChannelKind::Plugin => {
                let mut it = payload.split_whitespace();
                match (it.next(), it.next()) {
                    (Some("load"), Some(name)) => {
                        let _ = self.dispatch(&format!("modload {}", name));
                    }
                    (Some("unload"), Some(name)) => {
                        let _ = self.dispatch(&format!("modunload {}", name));
                    }
                    _ => println!(
                        "act plugin/modules → 用法: act <通道> load|unload <模块名>"
                    ),
                }
            }
            ChannelKind::Driver => {
                match name {
                    "driver/gpu" => {
                        let mut it = payload.split_whitespace();
                        match it.next() {
                            Some("alloc") => {
                                let mib: u64 = it
                                    .next()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(64);
                                let bytes = mib * 1024 * 1024;
                                match crate::gpu::vram()
                                    .lock()
                                    .unwrap()
                                    .reserve(bytes)
                                {
                                    Ok(()) => {
                                        self.gpu_reserved_bytes += bytes;
                                        println!(
                                            "act driver/gpu → 已预留 {} MiB (累计 {} MiB)",
                                            mib,
                                            self.gpu_reserved_bytes / (1024 * 1024)
                                        );
                                    }
                                    Err(e) => {
                                        println!(
                                            "act driver/gpu → 预留失败: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Some("free") => {
                                let bytes = self.gpu_reserved_bytes;
                                if bytes > 0 {
                                    crate::gpu::vram()
                                        .lock()
                                        .unwrap()
                                        .release(bytes);
                                    self.gpu_reserved_bytes = 0;
                                    println!(
                                        "act driver/gpu → 已释放 {} MiB",
                                        bytes / (1024 * 1024)
                                    );
                                } else {
                                    println!("act driver/gpu → 无预留显存");
                                }
                            }
                            _ => println!(
                                "act driver/gpu → 用法: act <通道> alloc <MiB> | free"
                            ),
                        }
                    }
                    "driver/iommu" => {
                        let mut it = payload.split_whitespace();
                        match (it.next(), it.next(), it.next()) {
                            (Some("map"), Some(token), Some(va_hex)) => {
                                let size: u64 = it
                                    .next()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(4096);
                                let token_id =
                                    token.parse::<u64>().unwrap_or(0);
                                let gpu_va =
                                    u64::from_str_radix(
                                        va_hex.trim_start_matches("0x"),
                                        16,
                                    )
                                    .unwrap_or(0);
                                let mut io =
                                    crate::gpu::iommu().lock().unwrap();
                                let domain = io.allocate_domain(
                                    crate::ai_token::TokenId(token_id),
                                );
                                match io.create_mapping(
                                    crate::ai_token::TokenId(token_id),
                                    gpu_va,
                                    0,
                                    size,
                                    IommuFlags::empty(),
                                ) {
                                    Ok(_) => println!(
                                        "act driver/iommu → 域 {} 映射 0x{:x} size={}",
                                        domain.0, gpu_va, size
                                    ),
                                    Err(e) => {
                                        println!(
                                            "act driver/iommu → 映射失败: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            _ => println!(
                                "act driver/iommu → 用法: act <通道> map <token_id> <va_hex> <size>"
                            ),
                        }
                    }
                    _ => println!(
                        "act driver → 驱动已在宏内核态运行 ({} 模块)",
                        self.modules.loaded_count()
                    ),
                }
            }
            ChannelKind::App => {
                println!("act app → 已投递给应用层 ({} 字节)", payload.len());
            }
            ChannelKind::Browser => {
                let mut it = payload.split_whitespace();
                match it.next() {
                    Some("open") => {
                        let url = it.next().unwrap_or("");
                        if let Some(rest) = url.strip_prefix("file://") {
                            let path = rest.trim_start_matches('/');
                            match self.vfs.cat(path) {
                                Ok(content) => {
                                    println!(
                                        "act browser/gateway → 打开 file:///{} ({} 字节)",
                                        path,
                                        content.len()
                                    );
                                    // 同步到浏览器状态，使 sense browser/gateway 能看到
                                    crate::browser::browser()
                                        .lock()
                                        .unwrap()
                                        .note_local(url, &content);
                                }
                                Err(e) => {
                                    println!(
                                        "act browser/gateway → 打开失败: {}",
                                        e
                                    );
                                }
                            }
                        } else {
                            // 非 file:// 的 URL 已由 sense 层（真实抓取）处理
                            println!("act browser/gateway → 用法: open <url>");
                        }
                    }
                    _ => println!(
                        "act browser/gateway → 用法: act <通道> open <url>"
                    ),
                }
            }
            ChannelKind::Device => {
                match name {
                    "dev/disk" => {
                        let mut it = payload.split_whitespace();
                        match it.next() {
                            Some("write") => {
                                let sector: u64 = it
                                    .next()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0);
                                let data = it.next().unwrap_or("");
                                let content =
                                    format!("sector {}: {}\n", sector, data);
                                match self
                                    .vfs
                                    .append("/dev/disk.img", &content)
                                {
                                    Ok(()) => println!(
                                        "act dev/disk → 已写入扇区 {} ({} 字节)",
                                        sector,
                                        content.len()
                                    ),
                                    Err(e) => {
                                        println!(
                                            "act dev/disk → 写入失败: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Some("read") => {
                                let sector: u64 = it
                                    .next()
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0);
                                match self.vfs.cat("/dev/disk.img") {
                                    Ok(content) => {
                                        let line = content
                                            .lines()
                                            .find(|l| {
                                                l.starts_with(&format!(
                                                    "sector {}:",
                                                    sector
                                                ))
                                            })
                                            .unwrap_or(&"(空)");
                                        println!("act dev/disk → {}", line);
                                    }
                                    Err(_) => {
                                        println!("act dev/disk → 镜像为空");
                                    }
                                }
                            }
                            _ => println!(
                                "act dev/disk → 用法: act <通道> write|read <扇区> [数据]"
                            ),
                        }
                    }
                    _ => println!(
                        "act {} [{}] → 已确认 ({} 字节)",
                        name,
                        kind.name(),
                        payload.len()
                    ),
                }
            }
            ChannelKind::Gateway => {
                println!(
                    "act gateway → {} 转发 ({} 字节)",
                    name,
                    payload.len()
                );
            }
            _ => println!(
                "act {} [{}] → 已确认 ({} 字节)",
                name,
                kind.name(),
                payload.len()
            ),
        }
    }

    // ------------------------------------------------------------------
    // 挂载表 ↔ VFS 联动
    // ------------------------------------------------------------------

    /// 把挂载表整体物化进 VFS（启动时调用一次）。
    fn materialize_all_mounts(&mut self) {
        let targets: Vec<String> = crate::mount::mounts()
            .lock()
            .unwrap()
            .targets()
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        for t in targets {
            self.materialize_mount(&t);
        }
    }

    /// 物化单个挂载点：确保目录存在，并按文件系统类型写入伪文件。
    ///
    /// 这一步让 `ls /sys` 真的能看到东西，而不是只有挂载表里的一行记录。
    fn materialize_mount(&mut self, target: &str) {
        if self.vfs.ensure_dir_tree(target).is_err() {
            return;
        }
        let fstype = crate::mount::mounts()
            .lock()
            .unwrap()
            .find(target)
            .map(|e| e.fstype.clone())
            .unwrap_or_default();
        let base = target.trim_end_matches('/');

        match fstype.as_str() {
            "procfs" => {
                let snap = crate::procfs::procfs().lock().unwrap().snapshot();
                for line in snap.lines() {
                    let Some((key, val)) = line.split_once(':') else {
                        continue;
                    };
                    if matches!(key, "meminfo" | "loadavg" | "uptime" | "version") {
                        let _ = self
                            .vfs
                            .write(&format!("{}/{}", base, key), &format!("{}\n", val.trim()));
                    }
                }
                let _ = self
                    .vfs
                    .write(&format!("{}/stat", base), &format!("{}\n", snap));
            }
            "sysfs" => {
                let vram = {
                    let v = crate::gpu::vram().lock().unwrap();
                    format!(
                        "total={}MiB free={}MiB\n",
                        v.total_vram() / (1024 * 1024),
                        v.available() / (1024 * 1024)
                    )
                };
                let _ = self.vfs.write(&format!("{}/gpu_vram", base), &vram);
                let params = crate::sysctl::sysctl().lock().unwrap().kernel_list();
                let _ = self
                    .vfs
                    .write(&format!("{}/kernel_params", base), &format!("{}\n", params));
                let ifs = crate::netif::ifaces().lock().list();
                let _ = self
                    .vfs
                    .write(&format!("{}/net_interfaces", base), &format!("{}\n", ifs));
                let verdict = crate::judgment::judgment().lock().unwrap().status();
                let _ = self
                    .vfs
                    .write(&format!("{}/judgment", base), &format!("{}\n", verdict));
            }
            "devfs" => {
                let nodes: Vec<(String, u32, u32, String)> = crate::dev::devtree()
                    .lock()
                    .unwrap()
                    .nodes
                    .iter()
                    .map(|n| (n.name.clone(), n.major, n.minor, n.driver.clone()))
                    .collect();
                for (name, major, minor, driver) in nodes {
                    let _ = self.vfs.write(
                        &format!("{}/{}", base, name),
                        &format!("<device {}:{} driver={}>\n", major, minor, driver),
                    );
                }
            }
            _ => {}
        }
    }

    /// 每次 ls / cat / cd 前刷新伪文件系统内容。
    fn refresh_virtual_mounts(&mut self) {
        let targets: Vec<String> = crate::mount::mounts()
            .lock()
            .unwrap()
            .targets()
            .into_iter()
            .filter(|(_, fstype)| matches!(fstype.as_str(), "procfs" | "sysfs" | "devfs"))
            .map(|(t, _)| t)
            .collect();
        for t in targets {
            self.materialize_mount(&t);
        }
    }

    /// 挂载 / 卸载成功后的目录树同步。
    fn sync_mount_hooks(&mut self, payload: &str) {
        let mut it = payload.split_whitespace();
        match it.next() {
            Some("mount") => {
                let _src = it.next();
                if let Some(target) = it.next() {
                    self.materialize_mount(target);
                }
            }
            Some("umount") | Some("unmount") => {
                if let Some(target) = it.next() {
                    let _ = self.vfs.remove_dir_all(target);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // 命令实现：文件系统
    // ------------------------------------------------------------------

    fn cmd_ls(&mut self, path: Option<&str>) {
        self.refresh_virtual_mounts();
        // worktree 生效时：相对路径与默认目录都落在隔离根下。
        let resolved: Option<String> = match path {
            Some(p) => Some(crate::worktree::resolve_path(p)),
            None => {
                let root = crate::worktree::current_root();
                if root == "/" {
                    None
                } else {
                    Some(root)
                }
            }
        };
        match self.vfs.ls(resolved.as_deref()) {
            Ok(items) => {
                if items.is_empty() {
                    println!("(空目录)");
                }
                for (name, kind, mode, size) in items {
                    let suffix = if kind == NodeKind::Dir { "/" } else { "" };
                    println!(
                        "{} {:04o} {:>8}  {}{}",
                        kind.type_char(),
                        mode,
                        size,
                        name,
                        suffix
                    );
                }
            }
            Err(e) => println!("ls 失败: {}", e),
        }
    }

    fn cmd_cd(&mut self, path: Option<&str>) {
        self.refresh_virtual_mounts();
        let target = match path {
            Some(p) => crate::worktree::resolve_path(p),
            None => "/".to_string(),
        };
        match self.vfs.cd(&target) {
            Ok(()) => {}
            Err(e) => println!("cd 失败: {} ({})", e, target),
        }
    }

    fn cmd_cat(&mut self, path: Option<&str>) {
        let Some(path) = path else {
            println!("用法: cat <路径>");
            return;
        };
        let path = crate::worktree::resolve_path(path);
        self.refresh_virtual_mounts();
        match self.vfs.cat(&path) {
            Ok(content) => {
                if content.is_empty() {
                    println!("(空文件)");
                } else {
                    print!("{}", content);
                    if !content.ends_with('\n') {
                        println!();
                    }
                }
            }
            Err(e) => println!("cat 失败: {}", e),
        }
    }

    fn cmd_write(&mut self, parts: &[&str], append: bool) {
        let Some(path) = parts.get(1) else {
            println!("用法: {} <路径> <内容...>", if append { "append" } else { "write" });
            return;
        };
        let mut content = parts[2..].join(" ");
        content.push('\n');
        let path = crate::worktree::resolve_path(path);
        let result = if append {
            self.vfs.append(&path, &content)
        } else {
            self.vfs.write(&path, &content)
        };
        match result {
            Ok(()) => println!(
                "{} {} ({} 字节)",
                if append { "已追加" } else { "已写入" },
                path,
                content.len()
            ),
            Err(e) => println!("写入失败: {}", e),
        }
    }

    fn cmd_mkdir(&mut self, path: Option<&str>) {
        let Some(path) = path else {
            println!("用法: mkdir <路径>");
            return;
        };
        let path = crate::worktree::resolve_path(path);
        match self.vfs.mkdir(&path) {
            Ok(()) => println!("已创建目录 {}", path),
            Err(e) => println!("mkdir 失败: {}", e),
        }
    }

    fn cmd_touch(&mut self, path: Option<&str>) {
        let Some(path) = path else {
            println!("用法: touch <路径>");
            return;
        };
        let path = crate::worktree::resolve_path(path);
        match self.vfs.touch(&path) {
            Ok(()) => println!("已创建文件 {}", path),
            Err(e) => println!("touch 失败: {}", e),
        }
    }

    fn cmd_rm(&mut self, path: Option<&str>) {
        let Some(path) = path else {
            println!("用法: rm <路径>");
            return;
        };
        let path = crate::worktree::resolve_path(path);
        match self.vfs.rm(&path) {
            Ok(()) => println!("已删除 {}", path),
            Err(e) => println!("rm 失败: {}", e),
        }
    }

    fn cmd_df(&self) {
        let vram = crate::gpu::vram().lock().unwrap();
        let total = vram.total_vram();
        let free = vram.available();
        let partitions = vram.partition_count();
        println!("设备       总容量       已用        可用      分区");
        println!(
            "gpu0        {:>6} MiB   {:>6} MiB   {:>6} MiB   {}",
            total / (1024 * 1024),
            (total - free) / (1024 * 1024),
            free / (1024 * 1024),
            partitions
        );
    }

    // ------------------------------------------------------------------
    // 命令实现：模块
    // ------------------------------------------------------------------

    fn cmd_modules(&self) {
        let loaded = self.modules.list_loaded();
        if loaded.is_empty() {
            println!("未加载任何模块");
        } else {
            for name in loaded {
                let state = self
                    .modules
                    .get_state(name)
                    .map(|s| s.name())
                    .unwrap_or("?");
                println!("  {} [{}]", name, state);
            }
        }
        println!("已加载模块数: {}", self.modules.loaded_count());
    }

    fn cmd_modload(&mut self, parts: &[&str]) {
        let Some(name) = parts.get(1) else {
            println!("用法: modload <名称>");
            return;
        };
        let entry = ModuleEntry {
            name: name.to_string(),
            version: Version::new(
                crate::version::VERSION_MAJOR,
                crate::version::VERSION_MINOR,
                crate::version::VERSION_PATCH,
            ),
            init: || Ok(()),
            cleanup: || {},
        };
        match self.modules.load_module(entry, ModuleProtocol::GpuDriver) {
            Ok(()) => println!("已加载模块 {}", name),
            Err(e) => println!("加载失败: {}", e),
        }
    }

    fn cmd_modunload(&mut self, parts: &[&str]) {
        let Some(name) = parts.get(1) else {
            println!("用法: modunload <名称>");
            return;
        };
        match self.modules.unload_module(name) {
            Ok(()) => println!("已卸载模块 {}", name),
            Err(e) => println!("卸载失败: {}", e),
        }
    }

    // ------------------------------------------------------------------
    // 命令实现：持久化
    // ------------------------------------------------------------------

    fn cmd_fbpreview(&mut self, parts: &[&str]) {
        let cols: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(76);
        let rows: usize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
        let fb = crate::desktop::framebuffer().lock().unwrap();
        print!("{}", fb.preview_ansi(cols, rows));
        println!("[fbpreview] {}x{} 缩略图，{} 点亮像素", cols, rows, fb.lit_pixels());
    }

    fn cmd_save(&self) {
        let p = Vfs::default_state_path();
        match self.vfs.save_to_path(&p) {
            Ok(()) => println!("[vfs] 已保存到 {}", p.display()),
            Err(e) => println!("[vfs] 保存失败: {}", e),
        }
    }

    fn cmd_load(&mut self) {
        let p = Vfs::default_state_path();
        match Vfs::load_from_path(&p) {
            Ok(Some(v)) => {
                self.vfs = v;
                println!("[vfs] 已从 {} 载入", p.display());
            }
            Ok(None) => println!("[vfs] 没有状态文件: {}", p.display()),
            Err(e) => println!("[vfs] 载入失败: {}", e),
        }
    }

    fn cmd_resetfs(&mut self) {
        self.vfs.reset();
        println!("[vfs] 已重置为默认目录树");
    }

    fn cmd_wallpaper(&mut self, parts: &[&str]) {
        let available = [
            "default", "hibos", "deepseek", "ai", "consciousness",
            "sunset", "aurora", "circuit", "nebula", "grid", "solid",
        ];
        match parts.get(1).copied() {
            None | Some("list") => {
                println!("可用壁纸: {}", available.join(", "));
                println!("当前字符桌面壁纸: {}", self.wallpaper);
                println!(
                    "像素桌面: {}",
                    crate::gui::wallpaper::bank().lock().unwrap().status().lines().next().unwrap_or("")
                );
            }
            Some("set") => {
                let name = parts.get(2).copied().unwrap_or("default");
                if available.contains(&name) {
                    self.wallpaper = name.to_string();
                    // 同一份状态同步到真像素合成器，改一处全系统生效。
                    match crate::gui::wallpaper::set_named(name) {
                        Ok(msg) => println!("[wallpaper] 已设置 {}: {}", name, msg),
                        Err(e) => println!("[wallpaper] 像素壁纸同步失败: {}", e),
                    }
                } else {
                    println!("未知壁纸: {} (可用: {})", name, available.join(", "));
                }
            }
            Some("current") => {
                println!("当前字符桌面壁纸: {}", self.wallpaper);
                println!("当前像素壁纸: {}", crate::gui::wallpaper::bank().lock().unwrap().name);
            }
            _ => println!("用法: wallpaper [list|set <name>|current]"),
        }
    }
    fn cmd_llm(&mut self, parts: &[&str]) {
        match parts.get(1).copied() {
            Some("providers") => {
                // 列出内置 provider（地址 / 默认模型），便于后续接入更多 model。
                for line in crate::llm::providers_text().lines() {
                    println!("{}", line);
                }
            }
            None | Some("status") => {
                let keys = self.handler.consciousness.api_keys();
                if keys.is_empty() {
                    tagged_out!("[llm]", "未配置任何 Provider API Key (使用 `llm key set <provider> <key>`)");
                } else {
                    tagged_out!("[llm]", "已配置 Provider:");
                    for (provider, key) in &keys {
                        let masked = if key.len() > 4 {
                            format!("{}****{}", &key[..4], &key[key.len()-4..])
                        } else {
                            "****".to_string()
                        };
                        println!("  {} => {}", provider, masked);
                    }
                }
            }
            Some("key") => match parts.get(2).copied() {
                None | Some("list") => {
                    let keys = self.handler.consciousness.api_keys();
                    if keys.is_empty() {
                        tagged_out!("[llm]", "未配置任何 Provider");
                    } else {
                        tagged_out!("[llm]", "Provider 列表:");
                        for (provider, key) in &keys {
                            let masked = if key.len() > 4 {
                                format!("{}****{}", &key[..4], &key[key.len()-4..])
                            } else {
                                "****".to_string()
                            };
                            println!("  {} => {}", provider, masked);
                        }
                    }
                }
                Some("set") => {
                    let provider = parts.get(3).copied().unwrap_or("");
                    let key = parts.get(4).copied().unwrap_or("");
                    if provider.is_empty() || key.is_empty() {
                        println!("用法: llm key set <provider> <key>");
                    } else {
                        self.handler.consciousness.set_api_key(provider, key.to_string());
                        tagged_out!("[llm]", "Provider '{}' API Key 已设置 (长度: {})", provider, key.len());
                    }
                }
                Some("del") => {
                    let provider = parts.get(3).copied().unwrap_or("");
                    if provider.is_empty() {
                        println!("用法: llm key del <provider>");
                    } else {
                        self.handler.consciousness.del_api_key(provider);
                        tagged_out!("[llm]", "Provider '{}' API Key 已删除", provider);
                    }
                }
                Some("show") => {
                    let provider = parts.get(3).copied().unwrap_or("");
                    if provider.is_empty() {
                        println!("用法: llm key show <provider>");
                    } else {
                        match self.handler.consciousness.api_key(provider) {
                            Some(key) => {
                                let masked = if key.len() > 4 {
                                    format!("{}****{}", &key[..4], &key[key.len()-4..])
                                } else {
                                    "****".to_string()
                                };
                                tagged_out!("[llm]", "Provider '{}' => {}", provider, masked);
                            }
                            None => tagged_out!("[llm]", "Provider '{}' 未配置 API Key", provider),
                        }
                    }
                }
                _ => println!("用法: llm key [list|set <provider> <key>|del <provider>|show <provider>]"),
            },
            Some("chat") => {
                let provider = parts.get(2).copied().unwrap_or("");
                if provider.is_empty() {
                    println!("用法: llm chat <provider> <消息>");
                    return;
                }
                let message = parts[3..].join(" ");
                if message.is_empty() {
                    println!("用法: llm chat <provider> <消息>");
                    return;
                }
                let key = match self.handler.consciousness.api_key(provider) {
                    Some(k) => k.clone(),
                    None => {
                        tagged_out!("[llm]", "Provider '{}' 未配置 API Key (先用 `llm key set <provider> <key>`)", provider);
                        return;
                    }
                };
                match crate::llm::chat(provider, &key, &message) {
                    Ok(reply) => {
                        // 方括号标识绿色，正文默认色。
                        tagged_out!(&format!("[llm:{}]", provider), "{}", reply);
                        // 把 AI 回复注入意识本体作为新的感知概念
                        let concept_name = format!("llm_reply_{}_{}", provider, self.handler.consciousness.concept_count());
                        self.handler
                            .consciousness
                            .add_perception_channel(&concept_name, crate::channels::ChannelKind::Text);
                        tagged_out!("[llm]", "回复已注入意识本体 (概念: {})", concept_name);
                    }
                    Err(e) => tagged_out!(&format!("[llm:{}]", provider), "对话失败: {}", e),
                }
            }
            Some("loop") => {
                let provider = parts.get(2).copied().unwrap_or("");
                if provider.is_empty() {
                    println!("用法: llm loop <provider|all>");
                    return;
                }
                if provider == "all" {
                    // 全部已配置 provider 同时运行。
                    let providers = self.configured_providers();
                    if providers.is_empty() {
                        tagged_out!("[llm]", "未配置任何 Provider API Key");
                        return;
                    }
                    self.run_llm_loop(&providers);
                } else {
                    self.run_llm_loop(&[provider.to_string()]);
                }
            }
            _ => println!("用法: llm [status|providers|key [list|set <provider> <key>|del <provider>|show <provider>]|chat <provider> <消息>|loop <provider>]"),
        }
    }

    /// 运行 LLM 闭环循环 (感知→思考→行动→感知)。
    ///
    /// 支持**多 provider 同时运行**：每个 provider 持有独立上下文窗口，
    /// 同一份感知并行发给各模型，行动统一落地，回复按 provider 分别呈现。
    pub fn run_llm_loop(&mut self, providers: &[String]) {
        // 1. 解析可用 provider（必须有 API Key）
        let mut available: Vec<(String, String)> = Vec::new();
        for p in providers {
            match self.handler.consciousness.api_key(p) {
                Some(k) => available.push((p.clone(), k.clone())),
                None => tagged_out!(
                    &format!("[llm:{}]", p),
                    "未配置 API Key (先用 `llm key set <provider> <key>`)"
                ),
            }
        }
        if available.is_empty() {
            return;
        }
        // 动态生成“已注册通道”清单，让 AI 知道自己能 sense / act 哪些通道。
        let channel_lines: Vec<String> = self
            .channels
            .list()
            .iter()
            .map(|c| {
                let rw = format!(
                    "{}{}",
                    if c.access.can_read() { "r" } else { "" },
                    if c.access.can_write() { "w" } else { "" }
                );
                format!("  {} [{}]", c.name, rw)
            })
            .collect();
        // 通道清单过长会挤占模型上下文（导致回复被中途截断），只列前 N 个并给出总数。
        const CATALOG_MAX: usize = 60;
        let catalog = if channel_lines.len() > CATALOG_MAX {
            let mut c = channel_lines[..CATALOG_MAX].join("\n");
            c.push_str(&format!(
                "\n  …… 其余 {} 个通道未列出（可按名 sense/act 访问）",
                channel_lines.len() - CATALOG_MAX
            ));
            c
        } else {
            channel_lines.join("\n")
        };
        let system_prompt = format!(
            "你是 Hib OS 宏内核的 AI 意识本体。系统里所有资源（上下文、行动、文件、显存、设备、网络、桌面等）对你而言都是同构的“通道”，你只做两件事：sense 读取(感知) 与 act 写入(行动)。\n\
             你可以在回复中以单独一行 `ACTION: <指令>` 自动采取行动，系统会立即执行。\n\
             指令格式：\n\
               ACTION: sense <通道>                读取通道状态，如 sense net/eth0\n\
               ACTION: act <通道> <负载...>         向通道写入，如 act driver/gpu alloc 64\n\
               ACTION: file write|append <路径> <内容> / file mkdir|touch|rm|cat|ls|cd <路径>\n\
               ACTION: token create <模型> [MiB] [low|medium|high|critical]\n\
               ACTION: proc <概念名> [MiB] / run / desktop / module load|unload <名称> / stats\n\
             已注册通道（r=可读 / w=可写）：\n{}\n\
              高级能力（均已注册为通道，用 sense/act 调用）：\n\
                ACTION: act proc/manager spawn <命令>          真实 fork/exec 一个主机进程\n\
                ACTION: act proc/manager signal <pid> <sig>    向进程发信号 (kill <pid> 亦可)\n\
                ACTION: sense proc/manager                      查看进程树\n\
                ACTION: act mem/ram alloc|free <MiB>            分配/释放主机 RAM\n\
                ACTION: act mem/mmap mmap <MiB> [perms]         建立内存映射 (munmap <addr> 解除)\n\
                ACTION: act fs/meta chmod <mode> <路径>         改权限 (chown/ln/mount 同理)\n\
                ACTION: act user/db adduser <名> | su <名|uid> | sudo | whoami   身份与提权\n\
                ACTION: act net/dns resolve <域名>              真实 DNS 解析\n\
                ACTION: act socket/tcp connect <host:port> [数据] 真实 TCP 连接/读写\n\
                ACTION: act ipc/pipe|shm|msgq|sem ...           进程间通信 (create/write/read/send/recv/init/post/wait)\n\
                ACTION: act sec/caps grant|revoke <CAP_XXX>     安全能力位\n\
                ACTION: act dev/tree attach|detach <设备>       设备树\n\
                ACTION: act init/services start|stop <服务>    服务管理\n\
                ACTION: act time/cron add <秒> <命令>|run       定时任务\n\
                ACTION: act pkg/store install|remove <包>       软件包\n\
                ACTION: act tty/jobs fg|bg|kill <pid>           作业控制\n\
                ACTION: act sysctl/kernel set <key> <value>     内核参数 (hostname/ip_forward/log.level)\n\
                ACTION: act config/system set <key> <value>     系统身份 (hostname/timezone/locale)\n\
                ACTION: act env/process set <key> <value>       环境变量\n\
                ACTION: act limits/process set <key> <value>    资源限制\n\
                ACTION: act fw/rules add|policy|del ...         防火墙规则\n\
                ACTION: act cgroup/groups create|set|attach     控制组配额\n\
                ACTION: act power/state suspend|reboot|wake     电源姿态\n\
                ACTION: act worktree/trees create <名称> [归属]  创建隔离工作区 (隔离实例的文件操作)\n\
                ACTION: act worktree/trees switch <名称>        切换当前工作区\n\
                ACTION: act plugin/modules install <名称> [版本] [kind] [caps]  安装自造插件\n\
                ACTION: act plugin/hotreload reload <名称>      热重载插件\n\
                ACTION: act plugin/<插件名> <负载>               向自造插件投递行动\n\
                规则：先用 sense 了解状态，再决定 act / 创建资源 / 操作文件；不要执行破坏性操作除非必要；用简体中文回答，并在需要时在末尾给出 ACTION。",
            catalog
        );
        // 每个 provider 一个独立上下文窗口：多模型同时运行、互不串味。
        let mut sessions: Vec<(String, String, crate::llm::ThoughtLoop)> = available
            .into_iter()
            .map(|(p, k)| {
                let lp = crate::llm::ThoughtLoop::new(&system_prompt).with_limits(10, 8000);
                (p, k, lp)
            })
            .collect();
        let provider = sessions[0].0.clone();
        if sessions.len() > 1 {
            tagged_out!(
                "[llm]",
                "多 provider 并行: {}",
                sessions
                    .iter()
                    .map(|(p, _, _)| p.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // 意识实例级工作区：**收到第一条用户命令时才绑定**。
        // 没有用户命令之前不创建 worktree、不发起任何模型调用（保持静默待命）。
        let mut wt: Option<(String, String)> = None;
        tagged_out!("[hibos]", "AI OS 已启动（等待你的第一条命令）");
        // 周期巡检间隔：默认 60s，可用环境变量 HIBOS_LLM_INTERVAL_SECS 调整。
        let interval_secs: u64 = std::env::var("HIBOS_LLM_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(60)
            .max(5);
        // 起始即计时：启动时不立即触发一轮，避免初次加载时 AI 刷出一大段回复。
        let mut last_cycle = std::time::Instant::now();
        loop {
            // —— 内部驱动：仅在用户下过命令之后才开始周期巡检 ——
            let now = std::time::Instant::now();
            let due = wt.is_some() && now.duration_since(last_cycle).as_secs() >= interval_secs;
            if due {
                let (_, wt_root) = wt.as_ref().unwrap();
                let perception = format!(
                    "系统周期巡检: 已感知通道={} 当前目录={} 工作区={}",
                    self.channels.len(),
                    self.vfs.cwd_path(),
                    wt_root
                );
                // 多 provider 并行思考 → 逐个落地行动 → 呈现回复。
                self.run_cycle(&mut sessions, &perception, wt_root);
                last_cycle = now;
            }

            // — 外部输入驱动 —
            let prompt = theme::prompt("~");
            let mut editor = std::mem::replace(&mut self.editor, LineEditor::new());
            let outcome = editor.read_line(&prompt, |input| self.completion(input));
            self.editor = editor;
            match outcome {
                Ok(line_editor::ReadOutcome::Line(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed == "exit" || trimmed == "quit" {
                        tagged_out!(
                            &format!("[llm:{}]", provider),
                            "闭环循环结束，共 {} 轮。",
                            Shell::total_turns(&sessions)
                        );
                        break;
                    }
                    // 首次用户命令：绑定本实例工作区（意识实例级隔离）。
                    if wt.is_none() {
                        let seq = LOOP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let wt_name = format!("loop-{}-{}", provider, seq);
                        let wt_root = crate::worktree::ensure_worktree(&wt_name, "ai-loop");
                        self.sync_worktree_root(&wt_root);
                        tagged_out!("[worktree]", "本实例工作区: {} ({})", wt_root, wt_name);
                        wt = Some((wt_name, wt_root));
                        last_cycle = std::time::Instant::now();
                    }
                    let (_, wt_root) = wt.as_ref().unwrap();
                    // 感知：把当前 shell 上下文（工作目录、通道数）作为感知输入
                    let perception = format!(
                        "用户输入: {}\n当前目录: {}\n已感知通道: {} 个\n工作区: {}",
                        trimmed,
                        self.vfs.cwd_path(),
                        self.channels.len(),
                        wt_root
                    );
                    // 多 provider 并行思考 → 逐个落地行动 → 呈现回复。
                    let (mut total, mut mutating, first_reply) =
                        self.run_cycle(&mut sessions, &perception, wt_root);

                    // react 续跑：本轮只有感知、没有落地行动时，基于感知结果自动再跑一轮，
                    // 避免 AI 只说"接下来要行动"却把行动停在承诺里。
                    // 上限与提示语均可通过环境变量覆盖：
                    //   HIBOS_LLM_REACT_ROUNDS  —— 最多续跑轮数（默认 200，最小 1）
                    //   HIBOS_LLM_REACT_PROMPT  —— 续跑时追加给 AI 的提示语
                    let max_react_rounds: usize = std::env::var("HIBOS_LLM_REACT_ROUNDS")
                        .ok()
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(200)
                        .max(1);
                    let react_prompt: String = std::env::var("HIBOS_LLM_REACT_PROMPT")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                        .unwrap_or_else(|| {
                            "提示：上一轮只完成了感知，请直接基于感知结果给出行动 ACTION（不要只做感知）。".to_string()
                        });

                    let mut round = 1usize;
                    let mut prev_reply = first_reply;
                    while round < max_react_rounds && total > 0 && mutating == 0 {
                        round += 1;
                        tagged_out!(
                            "[llm]",
                            "── react 第 {}/{} 轮 ── 上一轮只有感知，继续推动行动",
                            round,
                            max_react_rounds
                        );
                        let cont_perception = format!("{}\n{}", perception, react_prompt);
                        let (t, m, reply) = self.run_cycle(&mut sessions, &cont_perception, wt_root);
                        total += t;
                        mutating += m;
                        if t == 0 {
                            break;
                        }
                        // 护栏：回复与上一轮完全相同 = 没有进展，停止续跑。
                        if !reply.is_empty() && reply == prev_reply {
                            tagged_out!("[llm]", "回复与上一轮一致（无进展），停止续跑");
                            break;
                        }
                        prev_reply = reply;
                    }
                }
                Ok(line_editor::ReadOutcome::Cancelled) => {
                    // Ctrl+C：退出闭环循环（原先仅取消本行、继续 REPL，导致 Ctrl+C 无法退出）
                    tagged_out!(
                        &format!("[llm:{}]", provider),
                        "闭环循环已由 Ctrl+C 中断，共 {} 轮。",
                        Shell::total_turns(&sessions)
                    );
                    break;
                }
                Ok(line_editor::ReadOutcome::Eof) | Err(_) => {
                    tagged_out!(
                        &format!("[llm:{}]", provider),
                        "闭环循环结束，共 {} 轮。",
                        Shell::total_turns(&sessions)
                    );
                    break;
                }
            }
        }
    }

    /// 各 provider 已完成轮次之和（用于闭环统计）。
    fn total_turns(sessions: &[(String, String, crate::llm::ThoughtLoop)]) -> usize {
        sessions.iter().map(|(_, _, lp)| lp.turn()).sum()
    }

    /// 判断一条已下发的命令是否属于"落地行动"（写入/改变状态），而非只读感知。
    fn is_mutating_action(cmd: &str) -> bool {
        let head = cmd.split_whitespace().next().unwrap_or("");
        !matches!(
            head,
            "sense" | "ls"
                | "cat"
                | "pwd"
                | "stat"
                | "read"
                | "tree"
                | "df"
                | "channels"
                | "history"
                | "modules"
                | "fbpreview"
                | "wallpaper"
        )
    }

    /// 一轮闭环：把同一份感知**并行**发给所有 provider，
    /// 逐个落地行动，再按 provider 呈现回复（先执行、后回复）。
    ///
    /// 返回 `(执行的动作数, 其中落地行动数)`。
    fn run_cycle(
        &mut self,
        sessions: &mut [(String, String, crate::llm::ThoughtLoop)],
        perception: &str,
        root: &str,
    ) -> (usize, usize, String) {
        // 并行思考：每个 provider 在自己的线程里调用模型，互不阻塞。
        let results: Vec<(String, Result<String, String>)> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (provider, key, lp) in sessions.iter_mut() {
                let p = provider.clone();
                handles.push((
                    p,
                    scope.spawn(move || lp.think(provider.as_str(), key.as_str(), perception)),
                ));
            }
            handles
                .into_iter()
                .map(|(p, h)| {
                    let r = h
                        .join()
                        .unwrap_or_else(|_| Err("provider 线程异常".to_string()));
                    (p, r)
                })
                .collect()
        });

        let mut total = 0usize;
        let mut mutating = 0usize;
        let mut replies = String::new();
        for (provider, res) in results {
            match res {
                Ok(reply) => {
                    // 行动注入意识本体
                    if let Some((_, _, lp)) = sessions.iter_mut().find(|(p, _, _)| *p == provider) {
                        lp.apply_actions(&reply, &mut self.handler.consciousness);
                    }
                    // 人类阅读节奏：先显示执行（行动落地），再显示回复文本。
                    let (t, m) = self.execute_llm_actions(&reply, root);
                    total += t;
                    mutating += m;
                    let concepts = self.handler.consciousness.concept_count();
                    tagged_out!("[llm]", "意识本体概念数: {}", concepts);
                    // 回复正文：剔除 ACTION 行，标识绿色、正文默认色。
                    let text = reply
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty() && !l.trim_start().starts_with("ACTION:"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        tagged_out!(&format!("[llm:{}]", provider), "{}", text);
                        replies.push_str(&text);
                        replies.push('\n');
                    }
                }
                Err(e) => {
                    tagged_out!(&format!("[llm:{}]", provider), "循环失败: {}", e);
                    // 错误恢复：失败后清空该 provider 的上下文窗口。
                    if let Some((_, _, lp)) = sessions.iter_mut().find(|(p, _, _)| *p == provider) {
                        lp.reset();
                    }
                }
            }
        }
        (total, mutating, replies)
    }

    /// 从 AI 回复中解析并执行 ACTION 指令。
    ///
    /// 支持 `channels` 注册表中的**全部已注册通道**：
    /// - `sense <通道>`           读取通道状态（感知）—— 任何可读取的通道
    /// - `act <通道> <负载...>`   向通道写入（行动）—— 任何可写入的通道
    /// - 高级指令：`file` / `token` / `proc` / `run` / `desktop` / `module` / `concepts` / `stats`
    /// - 兼容旧式写法：`gpu/vram ...` / `net/eth0 ...` / `file://vfs ...` / `dev/disk ...`
    fn execute_llm_actions(&mut self, reply: &str, root: &str) -> (usize, usize) {
        let mut executed = 0usize;
        let mut mutating = 0usize;
        for line in reply.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("ACTION:") else {
                continue;
            };
            let rest = rest.trim();
            if rest.is_empty() {
                continue;
            }
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let head = parts[0];
            let args = &parts[1..];

            // 根据指令头决定要执行的 shell 命令；覆盖全部已注册通道。
            let cmd: String = match head {
                // —— 通用感知 / 行动：覆盖所有已注册通道 ——
                "sense" => {
                    if args.is_empty() {
                        tagged_out!("[llm]", "ACTION sense 需要通道名，例如 sense net/eth0");
                        continue;
                    }
                    format!("sense {}", args.join(" "))
                }
                "act" => {
                    if args.is_empty() {
                        tagged_out!("[llm]", "ACTION act 需要通道名，例如 act driver/gpu alloc 64");
                        continue;
                    }
                    format!("act {}", args.join(" "))
                }
                // —— 文件系统高级指令（相对路径解析到当前 worktree 根，实现实例隔离） ——
                "file" => match args.first().copied() {
                    Some("write") | Some("append") => {
                        if args.len() < 3 {
                            tagged_out!("[llm]", "用法: ACTION: file write <路径> <内容...>");
                            continue;
                        }
                        // 相对路径解析到本实例的隔离工作区。
                        let p = crate::worktree::resolve_path_in(args[1], root);
                        format!("{} {} {}", args[0], p, args[2..].join(" "))
                    }
                    Some("mkdir") | Some("touch") | Some("rm") | Some("cat") | Some("ls")
                    | Some("cd") => {
                        if args.len() < 2 {
                            tagged_out!("[llm]", "用法: ACTION: file {} <路径>", args[0]);
                            continue;
                        }
                        let p = crate::worktree::resolve_path_in(args[1], root);
                        format!("{} {}", args[0], p)
                    }
                    _ => {
                        println!(
                            "[llm] 未知 file 子指令: {}",
                            args.first().copied().unwrap_or("")
                        );
                        continue;
                    }
                },
                // —— 内核资源高级指令 ——
                "token" => {
                    // token create <模型> [MiB] [prio]，或直接 token <模型> ...
                    if args.first().copied() == Some("create") {
                        format!("token {}", args[1..].join(" "))
                    } else {
                        format!("token {}", args.join(" "))
                    }
                }
                "proc" => {
                    if args.is_empty() {
                        tagged_out!("[llm]", "ACTION proc 需要概念名，例如 proc inference 128");
                        continue;
                    }
                    format!("proc {}", args.join(" "))
                }
                "run" => "run".to_string(),
                "desktop" => "desktop".to_string(),
                "module" => match args.first().copied() {
                    Some("load") => {
                        if args.len() < 2 {
                            tagged_out!("[llm]", "用法: ACTION: module load <名称>");
                            continue;
                        }
                        format!("modload {}", args[1..].join(" "))
                    }
                    Some("unload") => {
                        if args.len() < 2 {
                            tagged_out!("[llm]", "用法: ACTION: module unload <名称>");
                            continue;
                        }
                        format!("modunload {}", args[1..].join(" "))
                    }
                    _ => {
                        tagged_out!("[llm]", "用法: ACTION: module load|unload <名称>");
                        continue;
                    }
                },
                "concepts" => format!("concepts {}", args.join(" ")),
                "stats" => "stats".to_string(),
                // —— 兼容旧式“通道前缀”写法（仍走 act 语义）——
                other if other.starts_with("gpu/vram") => {
                    // gpu/vram alloc 64 → driver/gpu alloc 64（真正预留显存）
                    format!("act driver/gpu {}", args.join(" "))
                }
                other if other.starts_with("net/")
                    || other.starts_with("file://")
                    || other.starts_with("dev/") =>
                {
                    format!("act {}", rest)
                }
                // —— 直接以已注册通道名开头：按权限自动选择 act / sense ——
                _ => match self.channels.find(head) {
                    Some(ch) if ch.access.can_write() => format!("act {}", rest),
                    Some(_) => format!("sense {}", rest),
                    None => {
                        tagged_out!("[llm]", "未知 ACTION 指令: {}", head);
                        continue;
                    }
                },
            };

            tagged_out!("[llm]", "执行 ACTION: {}", cmd);
            self.dispatch(&cmd);
            executed += 1;
            if Self::is_mutating_action(&cmd) {
                mutating += 1;
            }
        }
        if executed > 0 {
            tagged_out!("[llm]", "本轮共自动执行 {} 条 ACTION 指令", executed);
        }
        (executed, mutating)
    }

    fn cmd_history(&self) {        if self.editor.history().is_empty() {
            println!("(无历史)");
            return;
        }
        for (i, cmd) in self.editor.history().iter().enumerate() {
            println!("  {:>3}  {}", i + 1, cmd);
        }
    }

    // ---- Agent / 任务 / 能力 / 系统 / 应用 命令 (通过统一能力层) ----

    fn cmd_agent(&self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("list");
        match sub {
            "list" => {
                let output = crate::agent::list();
                println!("{}", output);
            }
            "inspect" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let req = crate::capability::CapabilityRequest {
                    id: "agent".to_string(),
                    input: serde_json::json!({"op": "inspect", "id": id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["agent:read".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "agent.inspect".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("agent") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 agent 未注册"),
                }
            }
            "start" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let result = crate::agent::dispatch(&format!("agent/start {id}"));
                match result {
                    Ok(msg) => println!("{}", msg),
                    Err(e) => println!("错误: {}", e),
                }
            }
            "stop" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let result = crate::agent::stop(&id.to_string());
                match result {
                    Ok(msg) => println!("{}", msg),
                    Err(e) => println!("错误: {}", e),
                }
            }
            _ => println!("用法: agent list / inspect <id> / start <id> / stop <id>"),
        }
    }

    fn cmd_task(&self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("list");
        match sub {
            "create" => {
                let desc: String = parts.get(2).copied().unwrap_or("").to_string();
                let agent_id = parts
                    .iter()
                    .position(|p| *p == "--agent")
                    .and_then(|i| parts.get(i + 1))
                    .map(|s| s.to_string());
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "create", "description": desc, "agent_id": agent_id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:create".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.create".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            "list" => {
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "list"}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:read".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.list".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            "inspect" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "inspect", "id": id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:read".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.inspect".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            "cancel" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "cancel", "id": id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:control".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.cancel".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            "pause" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "pause", "id": id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:control".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.pause".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            "resume" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let req = crate::capability::CapabilityRequest {
                    id: "task".to_string(),
                    input: serde_json::json!({"op": "resume", "id": id}),
                    context: crate::capability::ExecutionContext {
                        caller_id: "shell".to_string(),
                        caller_type: crate::capability::CallerType::Shell,
                        permissions: vec!["task:control".to_string()],
                        resource_quota: crate::capability::ResourceQuota::default(),
                    },
                    timeout_ns: None,
                    cancellation_token: None,
                    audit: crate::capability::AuditInfo {
                        actor: "shell".to_string(),
                        action: "task.resume".to_string(),
                        timestamp: crate::capability::now_ns(),
                        result: crate::capability::AuditResult::Executed,
                        correlation_id: None,
                    },
                };
                let reg = crate::capability::capability_registry();
                match reg.lock().unwrap().get("task") {
                    Some(cap) => match cap.execute(req) {
                        Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                        Err(e) => println!("错误: {} (code={})", e.message, e.code),
                    },
                    None => println!("能力 task 未注册"),
                }
            }
            _ => println!("用法: task create <描述> [--agent <id>] | list | inspect <id> | cancel <id> | pause <id> | resume <id>"),
        }
    }

    fn cmd_capability(&self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("list");
        let reg = crate::capability::capability_registry();
        match sub {
            "list" => {
                let meta = reg.lock().unwrap().list_metadata();
                for m in meta {
                    println!(
                        "  {:<20} {} (权限: {})",
                        m.id,
                        m.description,
                        m.required_permissions.join(", ")
                    );
                }
            }
            "inspect" => {
                let id = parts.get(2).copied().unwrap_or("");
                match reg.lock().unwrap().get(id) {
                    Some(cap) => {
                        println!("能力: {}", cap.id());
                        println!("说明: {}", cap.description());
                        println!("输入 Schema: {}", serde_json::to_string_pretty(&cap.input_schema()).unwrap());
                        println!("输出 Schema: {}", serde_json::to_string_pretty(&cap.output_schema()).unwrap());
                        println!("权限要求: {:?}", cap.required_permissions());
                    }
                    None => println!("未找到能力: {}", id),
                }
            }
            _ => println!("用法: capability list / inspect <id>"),
        }
    }

    fn cmd_system(&self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("status");
        let req = crate::capability::CapabilityRequest {
            id: "system".to_string(),
            input: serde_json::json!({"op": sub}),
            context: crate::capability::ExecutionContext {
                caller_id: "shell".to_string(),
                caller_type: crate::capability::CallerType::Shell,
                permissions: vec!["sys:read".to_string()],
                resource_quota: crate::capability::ResourceQuota::default(),
            },
            timeout_ns: None,
            cancellation_token: None,
            audit: crate::capability::AuditInfo {
                actor: "shell".to_string(),
                action: format!("system.{sub}"),
                timestamp: crate::capability::now_ns(),
                result: crate::capability::AuditResult::Executed,
                correlation_id: None,
            },
        };
        let reg = crate::capability::capability_registry();
        match reg.lock().unwrap().get("system") {
            Some(cap) => match cap.execute(req) {
                Ok(resp) => println!("{}", serde_json::to_string_pretty(&resp.output).unwrap()),
                Err(e) => println!("错误: {} (code={})", e.message, e.code),
            },
            None => println!("能力 system 未注册"),
        }
    }

    fn cmd_app(&self, parts: &[&str]) {
        let sub = parts.get(1).copied().unwrap_or("list");
        match sub {
            "list" => {
                let apps = crate::gui::apps::AppKind::list();
                println!("可用应用: {}", apps);
            }
            "launch" => {
                let name = parts.get(2).copied().unwrap_or("");
                match crate::gui::open_app(name) {
                    Ok(id) => println!("已启动应用 {}, 窗口 id={}", name, id),
                    Err(e) => println!("启动失败: {}", e),
                }
            }
            "stop" => {
                let id: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let ok = crate::gui::close_window(id);
                if ok {
                    println!("已关闭窗口 {}", id);
                } else {
                    println!("窗口 {} 不存在", id);
                }
            }
            _ => println!("用法: app list / launch <名> / stop <id>"),
        }
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

/// 命令的参数形式 (`--help` 式补全提示)。
pub fn usage_line(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "help" => "help [命令]",
        "channels" => "channels",
        "sense" => "sense <通道>",
        "act" => "act <通道> [负载...]",
        "concepts" => "concepts [过滤]",
        "policy" => "policy <概念名>",
        "token" => "token <模型> [MiB] [prio]",
        "query" => "query <token_id>",
        "prio" => "prio <token_id> <low|medium|high|critical>",
        "proc" => "proc <概念名> [MiB]",
        "run" => "run",
        "queue" => "queue",
        "stats" => "stats",
        "desktop" => "desktop [--pixels]",
        "gui" => "gui [desktop|shot [路径]|run|wallpaper <list|set|load|fit|save|preview>|open <标题>|close <id>|focus <id>|move <id> <x> <y>|click <x> <y>|key <文本>|text <文本>]",
        "pwd" => "pwd",
        "ls" => "ls [路径]",
        "cd" => "cd [路径]",
        "cat" => "cat <路径>",
        "write" => "write <路径> <内容...>",
        "append" => "append <路径> <内容...>",
        "mkdir" => "mkdir <路径>",
        "touch" => "touch <路径>",
        "rm" => "rm <路径>",
        "tree" => "tree",
        "df" => "df",
        "save" => "save",
        "load" => "load",
        "resetfs" => "resetfs",
        "wallpaper" => "wallpaper [list|set <name>|current]  (同时切换字符与像素桌面)",
        "deepseek" => "llm [status|providers|key [list|set <provider> <key>|del <provider>|show <provider>]|chat <provider> <消息>|loop <provider>]",
        "llm" => "llm [status|providers|key [list|set <provider> <key>|del <provider>|show <provider>]|chat <provider> <消息>|loop <provider>]",
        "modules" => "modules",
        "modload" => "modload <名称>",
        "modunload" => "modunload <名称>",
        "history" => "history",
        "exit" | "quit" => "exit",
        "agent" => "agent list / inspect <id> / start <id> / stop <id>",
        "task" => "task create <描述> [--agent <id>] | list | inspect <id> | cancel <id> | pause <id> | resume <id>",
        "capability" => "capability list / inspect <id>",
        "system" => "system status / resources / logs",
        "app" => "app list / launch <名> / stop <id>",
        _ => return None,
    })
}

/// 命令的一句话说明。
pub fn describe(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "help" => "查看命令列表或单条命令用法",
        "channels" => "列出全部感知通道 (AI 的感官清单)",
        "sense" => "通过通道感知: 文件/上下文/显存/设备/轨迹 ...",
        "act" => "通过通道行动: 写入日志/提交进程/光栅化帧缓冲 ...",
        "concepts" => "列出概念本体及其优先级、调度策略",
        "policy" => "查询某概念的调度策略",
        "token" => "创建 AI Action Token (分配显存并进入调度队列)",
        "query" => "查询 Token 状态",
        "prio" => "调整 Token 优先级",
        "proc" => "按概念创建并提交 Action Process",
        "run" => "调度并完成队首任务",
        "queue" => "查看就绪队列长度",
        "stats" => "系统统计 (Token/任务/显存/运行时长)",
        "desktop" => "文本桌面面板; --pixels 输出像素帧缓冲缩略图",
        "gui" => "真像素 GUI: 窗口合成/键盘鼠标/BMP 导出/真实窗口 (--features gui)",
        "pwd" => "显示当前工作目录",
        "ls" => "列出目录条目 (类型/权限/大小)",
        "cd" => "切换工作目录, 支持 . 与 .. 及相对路径",
        "cat" => "查看文件内容",
        "write" => "写入 (覆盖) 文件, 父目录须存在",
        "append" => "追加内容到文件",
        "mkdir" => "创建目录",
        "touch" => "创建空文件",
        "rm" => "删除文件或空目录",
        "tree" => "递归打印目录树",
        "df" => "显存占用",
        "save" => "把文件系统保存为 JSON",
        "load" => "从 JSON 载入文件系统",
        "resetfs" => "恢复默认目录树",
        "wallpaper" => "设置桌面壁纸 (default/hibos/deepseek/ai/consciousness + sunset/aurora/circuit/nebula/grid/solid)",
        "deepseek" => "llm 命令别名，配置多提供商 LLM API Key 与对话",
        "llm" => "配置多提供商 LLM API Key (deepseek/openai/moonshot/qwen/zhipu/groq/ollama)，支持单轮对话与闭环循环对话",
        "modules" => "列出已加载内核模块",
        "modload" => "加载演示内核模块",
        "modunload" => "卸载内核模块",
        "history" => "查看命令历史",
        "exit" | "quit" => "退出 shell",
        "agent" => "智能体管理：列表/查看/启动/停止智能体",
        "task" => "任务管理：创建/列表/查看/取消/暂停/恢复任务",
        "capability" => "系统能力：列出/查看已注册能力",
        "system" => "系统信息：状态/资源/日志",
        "app" => "应用管理：列表/启动/停止 GUI 应用",
        _ => return None,
    })
}

/// 返回当前时间的 HH:MM:SS 字符串（零依赖，基于 SystemTime）。
fn chrono_now_hms() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let total_secs = d.as_secs();
            let sec = total_secs % 60;
            let min = (total_secs / 60) % 60;
            let hour = (total_secs / 3600) % 24;
            format!("{:02}:{:02}:{:02}", hour, min, sec)
        }
        Err(_) => "00:00:00".to_string(),
    }
}

fn parse_priority(s: &str) -> Option<CpuPriority> {    match s.to_ascii_lowercase().as_str() {
        "low" | "l" => Some(CpuPriority::Low),
        "medium" | "med" | "m" => Some(CpuPriority::Medium),
        "high" | "h" => Some(CpuPriority::High),
        "critical" | "crit" | "c" => Some(CpuPriority::Critical),
        _ => None,
    }
}

/// 便于测试：取某模型的第一个 token id。
#[cfg(test)]
fn token_id_of(model: &str) -> u64 {
    let mgr = crate::ai_token::token_manager().lock().unwrap();
    mgr.tokens_of_model(model)
        .first()
        .map(|t| t.0)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_and_help() {
        let mut s = Shell::new();
        assert!(s.dispatch("help"));
        assert!(s.dispatch("   "));
        assert!(!s.dispatch("exit"));
        assert!(!s.dispatch("quit"));
    }

    #[test]
    fn mutating_action_classification() {
        // 只读感知不算"落地行动"
        assert!(!Shell::is_mutating_action("sense gpu/vram"));
        assert!(!Shell::is_mutating_action("ls /worktrees/wt1"));
        assert!(!Shell::is_mutating_action("cat note.txt"));
        assert!(!Shell::is_mutating_action("stat /x"));
        // 写入/改变状态算"落地行动"
        assert!(Shell::is_mutating_action("act driver/gpu alloc 64"));
        assert!(Shell::is_mutating_action("write /worktrees/wt1/note.txt hi"));
        assert!(Shell::is_mutating_action("token create llama-13b"));
        assert!(Shell::is_mutating_action("mkdir /worktrees/wt2"));
    }

    #[test]
    fn create_and_query_token() {
        let mut s = Shell::new();
        assert!(s.dispatch("token shell-test 1 high"));
        let id = token_id_of("shell-test");
        assert!(id > 0);
        assert!(s.dispatch(&format!("query {}", id)));
        assert!(s.dispatch("queue"));
        assert!(s.dispatch("stats"));
    }

    #[test]
    fn adjust_priority() {
        let mut s = Shell::new();
        assert!(s.dispatch("token prio-test 1 low"));
        let id = token_id_of("prio-test");
        assert!(id > 0);
        assert!(s.dispatch(&format!("prio {} critical", id)));
        assert!(s.dispatch("prio 999 low"));
    }

    #[test]
    fn process_and_run() {
        let mut s = Shell::new();
        assert!(s.dispatch("proc attention 8"));
        assert!(s.dispatch("run"));
        assert_eq!(s.handler.processes.len(), 0);
    }

    #[test]
    fn module_commands() {
        let mut s = Shell::new();
        assert!(s.dispatch("modload demo"));
        assert!(s.dispatch("modules"));
        assert!(s.dispatch("modunload demo"));
        assert!(s.dispatch("modunload demo"));
    }

    #[test]
    fn concept_and_policy_commands() {
        let mut s = Shell::new();
        assert!(s.dispatch("concepts"));
        assert!(s.dispatch("concepts attention"));
        assert!(s.dispatch("policy MatMul"));
        assert!(s.dispatch("policy 不存在的概念"));
    }

    #[test]
    fn filesystem_commands() {
        let mut s = Shell::new();
        assert!(s.dispatch("pwd"));
        assert!(s.dispatch("ls"));
        assert!(s.dispatch("cd /tmp"));
        assert_eq!(s.vfs.cwd_path(), "/tmp");
        assert!(s.dispatch("write note.txt 你好 Hib OS"));
        assert!(s.dispatch("cat note.txt"));
        assert!(s.dispatch("append note.txt 第二行"));
        assert!(s.dispatch("mkdir sub"));
        assert!(s.dispatch("cd sub"));
        assert_eq!(s.vfs.cwd_path(), "/tmp/sub");
        assert!(s.dispatch("cd .."));
        assert!(s.dispatch("rm note.txt"));
        assert!(s.dispatch("rm sub"));
        assert!(s.dispatch("cat /etc/hibos.toml"));
        assert!(s.dispatch("tree"));
        assert!(s.dispatch("df"));
        assert!(s.dispatch("cat 不存在"));
        assert!(s.dispatch("cd /不存在"));
    }

    #[test]
    fn desktop_renders() {
        let mut s = Shell::new();
        assert!(s.dispatch("desktop"));
    }

    #[test]
    fn completion_suggests_commands_and_paths() {
        let s = Shell::new();
        let cmds = s.complete("he");
        assert!(cmds.contains(&"help".to_string()));

        let paths = s.complete("cat /et");
        assert_eq!(paths, vec!["/etc/".to_string()]);

        let empty = s.complete("");
        assert!(empty.len() >= COMMANDS.len());
    }

    #[test]
    fn channels_registry_and_sense() {
        let mut s = Shell::new();
        assert_eq!(s.channels.len(), 107);
        assert!(s.dispatch("channels"));
        assert!(s.dispatch("sense file://vfs"));
        assert!(s.dispatch("sense gpu/vram"));
        assert!(s.dispatch("sense dev/timer"));
        assert!(s.dispatch("sense driver/gpu"));
        assert!(s.dispatch("sense browser/gateway"));
        // 第二批系统模块通道
        assert!(s.dispatch("sense mount/table"));
        assert!(s.dispatch("sense proc/sysinfo"));
        assert!(s.dispatch("sense net/interfaces"));
        assert!(s.dispatch("sense dev/bus"));
        assert!(s.dispatch("sense gateway/net"));
        // 意志层三个通道
        assert!(s.dispatch("sense motive/self"));
        assert!(s.dispatch("sense belief/self"));
        assert!(s.dispatch("sense judgment/self"));
        // 子意识主体 / 进程容器
        assert!(s.dispatch("sense proc/list"));
        assert!(s.dispatch("sense proc/status"));
        // 命名互斥锁
        assert!(s.dispatch("sense sync/mutex"));
        // 中断通道
        assert!(s.dispatch("sense irq/timer"));
        assert!(s.dispatch("sense irq/keyboard"));
        assert!(s.dispatch("sense irq/gpu"));
        // 内存页故障
        assert!(s.dispatch("sense mem/fault"));
        // 电源唤醒源
        assert!(s.dispatch("sense power/wake-source"));
        // 意识镜像加载器
        assert!(s.dispatch("sense loader/echo"));
        assert!(s.dispatch("sense loader/sense-loop"));
        assert!(s.dispatch("sense loader/act-loop"));
        assert!(s.dispatch("sense loader/idle"));
        // 只写通道不可感知 / 不存在通道
        assert!(s.dispatch("sense action/syscall"));
        assert!(s.dispatch("sense nope/none"));
        assert!(s.channels.total_senses() >= 5);
    }

    #[test]
    fn act_routes_to_real_subsystems() {
        let mut s = Shell::new();
        assert!(s.dispatch("act file://vfs hello-channel"));
        let log = s.vfs.cat("/var/log/channel.log").unwrap();
        assert!(log.contains("hello-channel"), "日志: {}", log);

        assert!(s.dispatch("act gpu/framebuffer HELLO"));
        assert!(s.dispatch("act dev/null drop-me"));
        // browser 的 file:// 由 shell 层直接读 VFS（不触网）
        assert!(s.dispatch("act browser/gateway open file:///etc/hibos.toml"));
        assert!(s.dispatch("act browser/gateway history"));

        assert!(s.dispatch("act plugin/modules load demo"));
        assert_eq!(s.modules.loaded_count(), 1);
        assert!(s.dispatch("act plugin/modules unload demo"));
        assert_eq!(s.modules.loaded_count(), 0);

        assert!(s.dispatch("act text/log kernel-note"));
        assert!(s.vfs.cat("/var/log/kernel.log").unwrap().contains("kernel-note"));

        // 子意识主体 / 进程容器
        assert!(s.dispatch("act proc/spawn echo"));
        assert!(s.dispatch("act proc/list"));
        // 命名互斥锁
        assert!(s.dispatch("act sync/mutex lock my-mutex 1"));
        assert!(s.dispatch("act sync/mutex unlock my-mutex 1"));
        // 中断通道
        assert!(s.dispatch("act irq/timer bind 1"));
        assert!(s.dispatch("act irq/timer fire"));
        // 内存页故障
        assert!(s.dispatch("act mem/fault alloc 4096"));
        assert!(s.dispatch("act mem/fault stats"));
        // 电源唤醒源
        assert!(s.dispatch("act power/wake-source irq/timer"));
        // 意识镜像加载器
        assert!(s.dispatch("act loader/sense-loop"));

        assert!(s.channels.total_acts() >= 6);
    }

    #[test]
    fn mount_syncs_to_vfs() {
        let mut s = Shell::new();
        // /sys 默认不在 VFS 里，由挂载表物化出来
        assert!(s.vfs.ls(Some("/sys")).is_err());
        assert!(s.dispatch("ls /sys"));
        assert!(s.vfs.ls(Some("/sys")).is_ok());
        assert!(s.vfs.cat("/sys/gpu_vram").unwrap().contains("total="));
        // /proc 同样是挂载点，内容来自 procfs 实时快照
        assert!(s.vfs.cat("/proc/loadavg").is_ok());

        // 新挂载立刻出现在目录树
        assert!(s.dispatch("act mount/table mount tmpfs /data tmpfs"));
        assert!(s.vfs.ls(Some("/data")).is_ok());
        // 卸载后目录消失
        assert!(s.dispatch("act mount/table umount /data"));
        assert!(s.vfs.ls(Some("/data")).is_err());
    }

    #[test]
    fn will_channels_cooperate() {
        let mut s = Shell::new();
        // 动机：把 efficacy 拉到最高
        assert!(s.dispatch("act motive/self urge efficacy +60"));
        // 信念：给出高置信度
        assert!(s.dispatch("act belief/self believe 测试命题 90"));
        // 判断力：读取前两者得出结论
        assert!(s.dispatch("act judgment/self judge 测试命题"));
        assert!(s.dispatch("sense judgment/self"));
        assert!(s.dispatch("sense motive/self"));
        assert!(s.dispatch("sense belief/self"));
    }

    #[test]
    fn vfs_persistence_roundtrip() {
        let mut s = Shell::new();
        assert!(s.dispatch("cd /tmp"));
        assert!(s.dispatch("write persist.txt 持久化测试"));

        let path = std::env::temp_dir().join("hibos_shell_persist_test.json");
        s.vfs.save_to_path(&path).unwrap();
        let loaded = Vfs::load_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.cwd_path(), "/tmp");
        assert!(loaded.cat("persist.txt").unwrap().contains("持久化测试"));
        let _ = std::fs::remove_file(&path);

        assert!(s.dispatch("resetfs"));
        assert_eq!(s.vfs.cwd_path(), "/");
    }

    #[test]
    fn completion_includes_arg_hint() {
        let s = Shell::new();
        let c = s.completion("cd ");
        assert!(c.hint.is_some(), "cd 后应给出参数提示");
        assert!(c.hint.unwrap().contains("cd"));
        let c2 = s.completion("cd /et");
        assert_eq!(c2.items, vec!["/etc/".to_string()]);
        assert!(s.completion("help").hint.is_none());
    }

    #[test]
    fn desktop_pixels_renders_preview() {
        let mut s = Shell::new();
        assert!(s.dispatch("desktop --pixels"));
    }

    #[test]
    fn act_net_ping_send() {
        let mut s = Shell::new();
        assert!(s.dispatch("act net/eth0 ping 10.0.0.2"));
        assert!(s.dispatch("act net/eth0 send 10.0.0.3 hello-net"));
        assert!(s.dispatch("act net/eth0 stats"));
        assert!(s.dispatch("act net/eth0 link down"));
        assert!(s.dispatch("act net/eth0 ping 10.0.0.2"));
        assert!(s.dispatch("act net/eth0 link up"));
    }

    #[test]
    fn act_driver_gpu_alloc_free() {
        let mut s = Shell::new();
        assert!(s.dispatch("act driver/gpu alloc 64"));
        assert_eq!(s.gpu_reserved_bytes, 64 * 1024 * 1024);
        assert!(s.dispatch("act driver/gpu free"));
        assert_eq!(s.gpu_reserved_bytes, 0);
        assert!(s.dispatch("act driver/gpu alloc 1"));
        assert!(s.dispatch("act driver/gpu alloc 100000"));
        // 申请失败不累计
        assert_eq!(s.gpu_reserved_bytes, 1 * 1024 * 1024);
        assert!(s.dispatch("act driver/gpu free"));
    }

    #[test]
    fn act_driver_iommu_map() {
        let mut s = Shell::new();
        assert!(s.dispatch("act driver/iommu map 7 0x2000 8192"));
    }

    #[test]
    fn act_browser_open_file() {
        let mut s = Shell::new();
        assert!(s.dispatch("act browser/gateway open file:///etc/hibos.toml"));
        assert!(s.dispatch("act browser/gateway open https://example.com"));
    }

    #[test]
    fn act_disk_write_read() {
        let mut s = Shell::new();
        assert!(s.dispatch("act dev/disk write 3 hello-disk"));
        assert!(s.dispatch("act dev/disk read 3"));
        assert!(s.dispatch("act dev/disk read 9"));
    }

    #[test]
    fn fbpreview_command() {
        let mut s = Shell::new();
        assert!(s.dispatch("fbpreview"));
        assert!(s.dispatch("fbpreview 40 10"));
    }

    #[test]
    fn unknown_command_is_harmless() {
        let mut s = Shell::new();
        assert!(s.dispatch("frobnicate"));
    }
}
