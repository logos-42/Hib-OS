//! 感知 / 行动探针 —— 把每个通道接到真实子系统。
//!
//! `probe(name)` 返回该通道的实时快照；`act(name, payload)` 执行一次写操作。
//! 返回 `None` 表示该通道由 shell 层负责 (文件、上下文、行动等需要可变的会话状态)。

use std::io::IsTerminal;

use crate::desktop;
use crate::gpu::capabilities::{GpuCapability, GpuCapabilities};
use crate::gpu::{iommu, vram, VramPartitionId};
use crate::monitor::get_time_ns;
use crate::net::ETH_NAME;
use crate::vfs::Vfs;

use crate::ai_os::model::{status as model_status};
use crate::ai_os::ctx_mem;
use crate::ai_os::knowledge;
use crate::ai_os::tool::{status as tool_status};
use crate::ai_os::security;
use crate::ai_os::deadlock;
use crate::ai_os::container;
use crate::ai_os::native;
use crate::ai_os::mind;
use crate::ai_os::attention;
use crate::ai_os::semantic;
use crate::ai_os::memory_ws;
use crate::ai_os::body_schema;
use crate::ai_os::action;
use crate::ai_os::temporal;
use crate::ai_os::social;
use crate::ai_os::ethics;
use crate::ai_os::evolution;
use crate::ai_os::dream;
use crate::ai_os::vfs_ai;
use crate::ai_os::net_ai;
use crate::ai_os::driver_ai;
use crate::ai_os::{status as ai_os_status};
use crate::agent;
use crate::belief::beliefs;
use crate::browser::browser;
use crate::bus::bus;
use crate::caps::caps;
use crate::cell;
use crate::domain;
use crate::hibernate;
use crate::protection;
use crate::realm;
use crate::cgroup::cgroups;
use crate::dev::devtree;
use crate::env::envstore;
use crate::exec;
use crate::fsmeta::fsmeta;
use crate::fw::firewall;
use crate::gateway::gateway;
use crate::gui;
use crate::init::init_mgr;
use crate::ipc::ipc;
use crate::judgment::judgment;
use crate::lease;
use crate::limits::limits;
use crate::mem::mem;
use crate::motive::motives;
use crate::mount::mounts;
use crate::mutex2;
use crate::netif::ifaces;
use crate::power::power;
use crate::proc::proctable;
use crate::proc2;
use crate::procfs::procfs;
use crate::socket::netio;
use crate::scheduler_domains;
use crate::buddy_alloc::buddy_summary;
use crate::page_table_sim::pt_summary;
use crate::swap_sim::swap_summary;
use crate::sysctl::sysctl;
use crate::sync;
use crate::time::timekeeper;
use crate::tty::tty;
use crate::user::userdb;
use crate::ai_token::TokenId;
use crate::scheduler::CpuPriority;
use exec::ExecId;
use proc2::ProcId;
use std::sync::{Mutex, OnceLock};

/// 感知一个硬件 / 系统通道，返回实时快照。
pub fn probe(name: &str) -> Option<String> {
    match name {
        "gpu://vram" => {
            let v = vram().lock().unwrap();
            let total = v.total_vram();
            let free = v.available();
            Some(format!(
                "total={}MiB used={}MiB free={}MiB partitions={}",
                total / (1024 * 1024),
                (total - free) / (1024 * 1024),
                free / (1024 * 1024),
                v.partition_count()
            ))
        }
        "gpu://capabilities" => {
            let caps = GpuCapabilities::all();
            let names: Vec<&str> = GpuCapability::ALL.iter().map(|c| c.name()).collect();
            Some(format!("mask=0x{:02x} caps=[{}]", caps.bits(), names.join(",")))
        }
        "gpu://framebuffer" => {
            let (w, h, bytes, lit) = desktop::framebuffer_stats();
            Some(format!("{}x{} {}B lit={}", w, h, bytes, lit))
        }
        "gpu://scheduler" | "action://scheduler" => {
            Some("MLFQ + semantic weight, slice=10ms".to_string())
        }
        "mem://system" => {
            let s = crate::monitor::get_stats();
            Some(format!(
                "tokens={} active={} completed={} failed={} avg={}ns vram_used={}B",
                s.total_tokens,
                s.active_tokens,
                s.total_tasks_completed,
                s.total_tasks_failed,
                s.avg_task_time_ns,
                s.total_vram_used
            ))
        }
        "dev://iommu0" => {
            let io = iommu().lock().unwrap();
            Some(format!(
                "domains={} mappings={}",
                io.domain_count(),
                io.mapping_count()
            ))
        }
        "dev://timer" => Some(format!("now={}ns", get_time_ns())),
        "dev://console" => Some(format!(
            "stdin={} stdout=tty ansi=on",
            if std::io::stdin().is_terminal() {
                "tty"
            } else {
                "pipe"
            }
        )),
        "dev://disk" => {
            let p = Vfs::default_state_path();
            let exists = p.exists();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            Some(format!("{} exists={} size={}B", p.display(), exists, size))
        }
        "dev://null" => Some("discard (写入即丢弃)".to_string()),
        "dev://gpu0" => {
            let v = vram().lock().unwrap();
            Some(format!(
                "gpu0 total={}GiB caps={} dev-id=0",
                v.total_vram() / (1024 * 1024 * 1024),
                GpuCapability::ALL.len()
            ))
        }
        "dev://monitor" => {
            let s = crate::monitor::get_stats();
            Some(format!("uptime={}ns tasks={}", s.system_uptime_ns, s.total_tasks_completed))
        }
        "interface://abi" => Some("AiSystemCall 10 变体 / AiSystemCallResult".to_string()),
        "gateway://net" => Some(gateway().lock().unwrap().status()),
        "browser://gateway" => Some(browser().lock().unwrap().status()),
        "driver://gpu" => {
            let v = vram().lock().unwrap();
            Some(format!(
                "gpu-driver total={}GiB caps={} state=loaded",
                v.total_vram() / (1024 * 1024 * 1024),
                GpuCapability::ALL.len()
            ))
        }
        "driver://iommu" => {
            let io = iommu().lock().unwrap();
            Some(format!(
                "iommu-driver domains={} mappings={} state=loaded",
                io.domain_count(),
                io.mapping_count()
            ))
        }
        "trace://exec" => {
            let s = crate::monitor::get_stats();
            Some(format!(
                "uptime={}ns exec_ok={} exec_fail={}",
                s.system_uptime_ns, s.total_tasks_completed, s.total_tasks_failed
            ))
        }
        "net://eth0" => {
            let st = crate::net::net().lock().unwrap();
            let s = st.stats();
            let arp_lines: Vec<String> = st
                .arp
                .iter()
                .map(|(ip, mac)| format!("  {} -> {}", crate::net::fmt_ip(*ip), crate::net::fmt_mac(*mac)))
                .collect();
            let route_lines: Vec<String> = st
                .routes
                .iter()
                .map(|r| {
                    format!(
                        "  {} via {} dev {}",
                        crate::net::fmt_ip(r.network),
                        crate::net::fmt_ip(r.gateway),
                        r.iface
                    )
                })
                .collect();
            let arp_block = if arp_lines.is_empty() {
                "  (empty)".to_string()
            } else {
                arp_lines.join("\n")
            };
            let route_block = if route_lines.is_empty() {
                "  (empty)".to_string()
            } else {
                route_lines.join("\n")
            };
            Some(format!(
                "eth0 link={} ip={} mask={} mac={}\ntx={}/{}B rx={}/{}B\nARP:\n{}\nRoutes:\n{}",
                if st.iface.link_up { "up" } else { "down" },
                crate::net::fmt_ip(st.iface.ip),
                crate::net::fmt_ip(st.iface.mask),
                crate::net::fmt_mac(st.iface.mac),
                s.tx_packets,
                s.tx_bytes,
                s.rx_packets,
                s.rx_bytes,
                arp_block,
                route_block
            ))
        }
        // —— AI 原生扩展通道：进程 / 内存 / 权限 / 身份 / 真实网络 ——
        "proc://manager" => {
            let mut t = proctable().lock().unwrap();
            Some(t.tree())
        }
        "mem://ram" => Some(mem().lock().unwrap().stats()),
        "mem://mmap" => Some(mem().lock().unwrap().regions()),
        "fs://meta" => Some(fsmeta().lock().unwrap().list()),
        "user://db" => Some(userdb().lock().unwrap().list()),
        "socket://tcp" => Some(netio().lock().unwrap().last.clone()),
        "net://dns" => Some(netio().lock().unwrap().last.clone()),
        // —— IPC / 安全 / 设备 / init / 时间 / 包管理 / 作业控制 ——
        "ipc://pipe" => Some(ipc().lock().unwrap().pipes()),
        "ipc://shm" => Some(ipc().lock().unwrap().shm_list()),
        "ipc://msgq" => Some(ipc().lock().unwrap().msgq_list()),
        "ipc://sem" => Some(ipc().lock().unwrap().sem_list()),
        "sec://caps" => Some(caps().lock().unwrap().list()),
        "dev://tree" => Some(devtree().lock().unwrap().list()),
        "init://services" => Some(init_mgr().lock().unwrap().list()),
        "time://clock" => Some(timekeeper().lock().unwrap().clock()),
        "time://cron" => Some(timekeeper().lock().unwrap().cron_list()),
        "pkg://store" => Some(pkgstore().lock().unwrap().list()),
        "tty://jobs" => {
            let mut t = tty().lock().unwrap();
            Some(t.jobs())
        }
        // —— 系统设置类 ——
        "sysctl://kernel" => Some(sysctl().lock().unwrap().kernel_list()),
        "config://system" => Some(sysctl().lock().unwrap().identity_list()),
        "env://process" => Some(envstore().lock().unwrap().list()),
        "limits://process" => Some(limits().lock().unwrap().list()),
        "fw://rules" => Some(firewall().lock().unwrap().list()),
        "cgroup://groups" => Some(cgroups().lock().unwrap().list()),
        "power://state" => Some(power().lock().unwrap().status()),
        // —— 第二批系统模块 ——
        "mount://table" => Some(mounts().lock().unwrap().list()),
        "proc://sysinfo" => Some(procfs().lock().unwrap().snapshot()),
        "net://interfaces" => Some(ifaces().lock().unwrap().list()),
        "dev://bus" => Some(bus().lock().unwrap().list()),
        // —— 意志层：动机 / 信念 / 判断力 ——
        "motive://self" => Some(motives().lock().unwrap().status()),
        "belief://self" => Some(beliefs().lock().unwrap().status()),
        "judgment://self" => Some(judgment().lock().unwrap().status()),
        // —— 执行体 / 同步 / 租约 / 子智能体（真机制，不是记账）——
        "exec://threads" => Some(format!(
            "{}\n{}",
            exec::status_line(),
            exec::list()
        )),
        "sync://primitives" => Some(sync::status()),
        "lease://channels" => Some(lease::status()),
        "agent://orchestrator" => Some(agent::status()),
        "agent://workers" => Some(agent::list()),
        "agent://results" => Some(agent::results()),
        // —— GUI：窗口 / 键盘 / 鼠标 ——
        "gui://windows" => Some(gui::status()),
        "input://kbd" | "input://mouse" => Some(gui::input_status()),
        // —— 子意识主体 / 进程容器 ——
        "proc://spawn" | "proc://kill" | "proc://list" | "proc://wait_table" | "proc://status" => {
            Some(proc2::stats())
        }
        // —— 命名互斥锁 ——
        "sync://mutex" => Some(mutex2::list_all()),
        // —— 中断通道 ——
        "irq://timer" | "irq://keyboard" | "irq://gpu" => {
            Some(format!("irq 待投递，未注册处理器"))
        }
        // —— 内存页故障 ——
        "mem://fault" => Some(crate::mm::fault::stats()),
        // —— 电源唤醒源 ——
        "power://wake-source" => Some(power_wake_source_status()),
        // —— 意识镜像加载器 ——
        "loader://echo" | "loader://sense-loop" | "loader://act-loop" | "loader://idle" => {
            Some(format!("镜像就绪，等待 act loader://<镜像> 加载"))
        }
        // —— 本轮补齐：子意识 / 休眠 / 多域 / GPU 保护 / 身份空间 / 桌面外壳 ——
        "cell://self" => Some(cell::status()),
        "power://hibernate" => Some(hibernate::status()),
        "domain://table" => Some(domain::table().lock().unwrap().status()),
        "gpu://domains" => Some(protection::protection().lock().unwrap().status()),
        "gpu://faults" => Some(protection::protection().lock().unwrap().faults()),
        "sec://acl" | "realm://current" => Some(realm::realm_status()),
        "gui://shell" => Some(gui::shell::desktop_shell().lock().unwrap().status()),
        "gui://wallpaper" => Some(gui::wallpaper::bank().lock().unwrap().status()),
        // —— 多调度域 ——
        "sched://reflex" | "sched://perception" | "sched://deliberation" | "sched://memory" | "sched://governance" | "sched://domains" => {
            Some(crate::scheduler_domains::status_line())
        }
        "sched://inspiration" => Some(crate::inspiration::core().lock().unwrap().status()),
        // —— Subagent-A 新增：内存与页表相关通道 ——
        "swap://area" => Some(swap_summary().unwrap_or_else(|_| "swap 未初始化".to_string())),
        "pagetable://walk" => Some(pt_summary()),
        "slab://stats" => Some(slab_summary()),
        "buddy://stats" => Some(buddy_summary()),
        // —— AI 原生 OS 通道：模型 / 上下文窗口 / 记忆 / 工具 / 安全 / 死锁 / 容器 / 总控 ——
        "model://loader" => Some(model_status()),
        "model://instances" => {
            let r = crate::ai_os::model::model_registry().lock().unwrap();
            Some(format!("{} instances", r.instances().len()))
        }
        "model://spec" => Some(model_status()),
        "ctx://kv" => Some(ctx_mem::status()),
        "ctx://window" => Some(ctx_mem::status()),
        "knowledge://store" => Some(knowledge::status()),
        "knowledge://query" => Some(knowledge::status()),
        "tool://registry" => Some(tool_status()),
        "aios://status" => Some(ai_os_status()),
        "security://status" => Some(security::status()),
        "sec://caps" => Some(caps().lock().unwrap().list()),
        "deadlock://graph" => Some(deadlock::status()),
        "deadlock://wait" => Some(deadlock::status()),
        "container://status" => Some(container::status()),
        // —— AI 原生意识层通道：意识原子 / 心智 / 注意力 / 语义空间 / 工作记忆 ——
        "native://qualia" => Some(crate::ai_os::native::factory_status()),
        "native://stream" => {
            let eng = crate::ai_os::native::consciousness_engine().lock().unwrap();
            Some(format!("意识流 状态数={} 总推入={}", eng.stream.state_count(), eng.stream.total_states))
        }
        "native://status" => Some(crate::ai_os::native::native_status()),
        "mind://state" => Some(crate::ai_os::mind::mind_status()),
        "mind://goal" => {
            let eng = crate::ai_os::mind::mind_engine().lock().unwrap();
            match eng.goal_stack.active_goal() {
                Some(g) => Some(format!("活跃目标[id={}]: {} 进度={:.0}%", g.id, g.description, g.progress * 100.0)),
                None => Some("无活跃目标".to_string()),
            }
        }
        "mind://wm" => {
            let eng = crate::ai_os::mind::mind_engine().lock().unwrap();
            Some(format!("工作记忆 条目={} 使用率={:.2}", eng.working_memory.len(), eng.working_memory.usage_ratio()))
        }
        "mind://decide" => Some("决策引擎就绪，使用 act mind://decide <选项...>".to_string()),
        "attention://lens" => {
            let eng = crate::ai_os::attention::attention_engine().lock().unwrap();
            Some(format!("透镜模式={:?} 阈值={:.2} 通过率={:.2}", eng.lens.mode, eng.lens.threshold, eng.lens.pass_rate()))
        }
        "attention://saliency" => {
            let eng = crate::ai_os::attention::attention_engine().lock().unwrap();
            let top = eng.saliency.top_k(5);
            let mut out = String::from("显著性图 Top5:\n");
            for (ch, s) in top {
                out.push_str(&format!("  {} = {:.3}\n", ch, s));
            }
            Some(out)
        }
        "attention://focus" => {
            let eng = crate::ai_os::attention::attention_engine().lock().unwrap();
            match eng.focus.current_name() {
                Some(n) => Some(format!("当前焦点: {}", n)),
                None => Some("无当前焦点".to_string()),
            }
        }
        "attention://cycle" => {
            let eng = crate::ai_os::attention::attention_engine().lock().unwrap();
            Some(format!("注意力周期值: {:.3}", eng.cycle_value()))
        }
        "semantic://index" => Some("语义索引就绪，使用 act semantic://index <文本>".to_string()),
        "semantic://search" => Some("语义检索就绪，使用 act semantic://search <查询>".to_string()),
        "semantic://concept" => Some("概念图就绪，使用 act semantic://concept <名称>".to_string()),
        "semantic://namespace" => Some("语义命名空间就绪，使用 act semantic://bind <路径> <文本>".to_string()),
        "memory://sensory" => Some("感觉缓冲就绪，使用 act memory://sensory-write <模态> <数据>".to_string()),
        "memory://wm" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("工作记忆 条目={} 利用率={:.2}", eng.working.len(), eng.working.utilization()))
        }
        "memory://stm" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("短时记忆 条目={} 平均重要性={:.2}", eng.short_term.len(), eng.short_term.avg_importance()))
        }
        "memory://ltm" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("长时记忆 条目={} 平均强度={:.2}", eng.long_term.len(), eng.long_term.avg_strength()))
        }
        "memory://episode" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("情景记忆 条目={}", eng.episodic.len()))
        }
        "memory://semantic" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("语义记忆 事实={}", eng.semantic.len()))
        }
        "memory://skill" => {
            let eng = crate::ai_os::memory_ws::memory_engine().lock().unwrap();
            Some(format!("程序记忆 技能={} 平均熟练度={:.2}", eng.procedural.len(), eng.procedural.avg_proficiency()))
        }
        _ => None,
    }
}

/// 电源唤醒源状态（供 `sense power://wake-source`）。
fn power_wake_source_status() -> String {
    static WAKE_SOURCE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let ws = WAKE_SOURCE.get_or_init(|| Mutex::new(None));
    match ws.lock().unwrap().as_deref() {
        Some(ch) => format!("wake-source={}", ch),
        None => "wake-source=(none)".to_string(),
    }
}

/// 设置电源唤醒源（供 `act power://wake-source <通道>`）。
pub fn set_wake_source(channel: &str) {
    static WAKE_SOURCE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let ws = WAKE_SOURCE.get_or_init(|| Mutex::new(None));
    *ws.lock().unwrap() = Some(channel.to_string());
}

/// 读取当前唤醒源。
pub fn wake_source() -> Option<String> {
    static WAKE_SOURCE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let ws = WAKE_SOURCE.get_or_init(|| Mutex::new(None));
    ws.lock().unwrap().clone()
}

/// 感知一个通道，同时返回它当前的世界版本 (epoch)。
///
/// 这是「AI 原生并发」的读侧：AI 拿到的不只是数据，还有「这是第几版世界」。
pub fn probe_epoch(name: &str) -> (Option<String>, u64) {
    (probe(name), lease::epoch(name))
}

/// 对通道执行一次行动，支持乐观并发校验。
///
/// * `act <通道> ... --if-epoch N` —— 只有该通道仍是第 N 版时才执行；
///   版本不符直接拒绝，调用方必须重新 `sense` 再决定（CAS 语义，不是悲观锁）。
/// * 任何成功写入都会推进 epoch 并唤醒等在这条通道上的执行体（提交即事件）。
///
/// 不携带 `--if-epoch` 时行为与旧版一致，保证既有调用方不受影响。
pub fn act(name: &str, payload: &str) -> Option<Result<String, String>> {
    let (payload, expected) = parse_if_epoch(payload);
    if let Some(want) = expected {
        let actual = lease::epoch(name);
        if actual != want {
            lease::note_stale(name);
            return Some(Err(format!(
                "epoch 已过期: 期望 {} 实际 {} —— 请重新 sense {} 后再行动",
                want, actual, name
            )));
        }
    }
    // 意图先过权限：user 空间默认不可写，dev（超级开发者）全放行。
    if let Err(e) = realm::check_act(name) {
        return Some(Err(e));
    }
    let result = act_raw(name, &payload);
    if let Some(Ok(ref msg)) = result {
        lease::force(name, msg);
    }
    result
}

/// 从负载中剥离 `--if-epoch N` / `--epoch N`，返回干净的负载与期望版本。
fn parse_if_epoch(payload: &str) -> (String, Option<u64>) {
    let tokens: Vec<&str> = payload.split_whitespace().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(tokens.len());
    let mut expected = None;
    let mut i = 0;
    while i < tokens.len() {
        if matches!(tokens[i], "--if-epoch" | "--epoch") {
            if let Some(n) = tokens.get(i + 1).and_then(|s| s.parse::<u64>().ok()) {
                expected = Some(n);
                i += 2;
                continue;
            }
        }
        kept.push(tokens[i]);
        i += 1;
    }
    (kept.join(" "), expected)
}

/// 对硬件 / 系统通道执行一次行动（未经一致性包装的内部实现）。
fn act_raw(name: &str, payload: &str) -> Option<Result<String, String>> {
    match name {
        "gpu://framebuffer" | "desktop://framebuffer" => {
            let mut fb = desktop::framebuffer().lock().unwrap();
            fb.clear();
            let end = fb.draw_text(2, 2, payload);
            Some(Ok(format!(
                "已光栅化 {:?} 到帧缓冲 (结束 x={}, 点亮 {} 像素)",
                payload,
                end,
                fb.lit_pixels()
            )))
        }
        "dev://timer" => Some(Ok(format!("now={}ns", get_time_ns()))),
        "dev://null" => Some(Ok(format!("已丢弃 {} 字节", payload.len()))),
        "gpu://vram" => {
            let text = payload.trim();
            let mut it = text.split_whitespace();
            let sub = it.next().unwrap_or("");
            let rest = it.next().unwrap_or("").trim();
            match sub {
                "acquire" => {
                    let mut p = rest.split_whitespace();
                    let pid_opt = p
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(VramPartitionId);
                    let owner_opt = p.next().and_then(|s| s.parse::<u64>().ok());
                    match (pid_opt, owner_opt) {
                        (Some(pid), Some(owner)) => {
                            let mut v = vram().lock().unwrap();
                            match v.acquire_gpu_lease(pid, owner) {
                                Ok(epoch) => Some(Ok(format!(
                                    "已获得 GPU 分区 {} 写租约 owner={} epoch={}",
                                    pid.0, owner, epoch
                                ))),
                                Err(e) => Some(Err(format!("GPU 租约冲突: {}", e))),
                            }
                        }
                        _ => Some(Err(
                            "用法: act gpu://vram acquire <分区ID> <owner-exec-id>".to_string(),
                        )),
                    }
                }
                "release" => {
                    let mut p = rest.split_whitespace();
                    let pid_opt = p
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(VramPartitionId);
                    let owner_opt = p.next().and_then(|s| s.parse::<u64>().ok());
                    match (pid_opt, owner_opt) {
                        (Some(pid), Some(owner)) => {
                            let mut v = vram().lock().unwrap();
                            if v.release_gpu_lease(pid, owner) {
                                Some(Ok(format!(
                                    "已释放 GPU 分区 {} 的写租约 owner={}",
                                    pid.0, owner
                                )))
                            } else {
                                Some(Err(format!(
                                    "GPU 分区 {} 上没有你的写租约 owner={}",
                                    pid.0, owner
                                )))
                            }
                        }
                        _ => Some(Err(
                            "用法: act gpu://vram release <分区ID> <owner-exec-id>".to_string(),
                        )),
                    }
                }
                "query" => {
                    let mut p = rest.split_whitespace();
                    let pid_opt = p
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(VramPartitionId);
                    match pid_opt {
                        Some(pid) => {
                            let v = vram().lock().unwrap();
                            match v.query_gpu_lease(pid) {
                                Some((holder, epoch)) => Some(Ok(format!(
                                    "GPU 分区 {} 写租约 holder={} epoch={}",
                                    pid.0, holder, epoch
                                ))),
                                None => Some(Ok(format!(
                                    "GPU 分区 {} 无写租约持有者",
                                    pid.0
                                ))),
                            }
                        }
                        None => Some(Err("用法: act gpu://vram query <分区ID>".to_string())),
                    }
                }
                "commit" => {
                    let mut p = rest.splitn(4, ' ');
                    let pid_opt = p
                        .next()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(VramPartitionId);
                    let owner_opt = p.next().and_then(|s| s.parse::<u64>().ok());
                    let epoch_opt = p.next().and_then(|s| s.parse::<u64>().ok());
                    let data = p.next().unwrap_or("").as_bytes();
                    match (pid_opt, owner_opt, epoch_opt) {
                        (Some(pid), Some(owner), Some(expected_epoch)) => {
                            let mut v = vram().lock().unwrap();
                            match v.commit_gpu(pid, owner, expected_epoch, data) {
                                Ok(new_epoch) => Some(Ok(format!(
                                    "GPU 分区 {} 已提交 owner={} 新epoch={}",
                                    pid.0, owner, new_epoch
                                ))),
                                Err(e) => Some(Err(format!("GPU 提交失败: {}", e))),
                            }
                        }
                        _ => Some(Err(
                            "用法: act gpu://vram commit <分区ID> <owner> <期望epoch> <数据>"
                                .to_string(),
                        )),
                    }
                }
                _ => {
                    let v = vram().lock().unwrap();
                    Some(Ok(format!(
                        "显存通道 总={}MiB 可用={}MiB 分区={} 操作: acquire|release|query|commit",
                        v.total_vram() / (1024 * 1024),
                        v.available() / (1024 * 1024),
                        v.partition_count()
                    )))
                }
            }
        }
        "net://eth0" => {
            let mut it = payload.split_whitespace();
            match it.next() {
                Some("stats") => {
                    let st = crate::net::net().lock().unwrap();
                    let s = st.stats();
                    Some(Ok(format!(
                        "tx={}/{}B rx={}/{}B link={}",
                        s.tx_packets,
                        s.tx_bytes,
                        s.rx_packets,
                        s.rx_bytes,
                        if st.iface.link_up { "up" } else { "down" }
                    )))
                }
                Some("arp") => {
                    let mut st = crate::net::net().lock().unwrap();
                    match it.next() {
                        Some("add") => {
                            let ip = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let mac = crate::net::parse_mac(it.next().unwrap_or("00:00:00:00:00:00"));
                            st.arp_add(ip, mac);
                            Some(Ok(format!(
                                "ARP {} -> {} 已添加",
                                crate::net::fmt_ip(ip),
                                crate::net::fmt_mac(mac)
                            )))
                        }
                        Some("del") => {
                            let ip = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.arp_del(ip);
                            Some(Ok(format!("ARP {} 已删除", crate::net::fmt_ip(ip))))
                        }
                        Some("show") | None => {
                            let st = crate::net::net().lock().unwrap();
                            if st.arp.is_empty() {
                                Some(Ok("ARP 表为空".to_string()))
                            } else {
                                let lines: Vec<String> = st
                                    .arp
                                    .iter()
                                    .map(|(ip, mac)| format!("  {} -> {}", crate::net::fmt_ip(*ip), crate::net::fmt_mac(*mac)))
                                    .collect();
                                Some(Ok(lines.join("\n")))
                            }
                        }
                        _ => Some(Err("用法: act net://eth0 arp add <ip> <mac> | del <ip> | show".to_string())),
                    }
                }
                Some("route") => {
                    let mut st = crate::net::net().lock().unwrap();
                    match it.next() {
                        Some("add") => {
                            let network = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let mask = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            let gateway = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.route_add(crate::net::RouteEntry {
                                network,
                                mask,
                                gateway,
                                iface: ETH_NAME.to_string(),
                            });
                            Some(Ok(format!(
                                "路由 {} via {} 已添加",
                                crate::net::fmt_ip(network),
                                crate::net::fmt_ip(gateway)
                            )))
                        }
                        Some("del") => {
                            let network = crate::net::parse_ip(it.next().unwrap_or("0.0.0.0"));
                            st.route_del(network);
                            Some(Ok(format!("路由 {} 已删除", crate::net::fmt_ip(network))))
                        }
                        Some("show") | None => {
                            let st = crate::net::net().lock().unwrap();
                            if st.routes.is_empty() {
                                Some(Ok("路由表为空".to_string()))
                            } else {
                                let lines: Vec<String> = st
                                    .routes
                                    .iter()
                                    .map(|r| {
                                        format!(
                                            "  {} via {} dev {}",
                                            crate::net::fmt_ip(r.network),
                                            crate::net::fmt_ip(r.gateway),
                                            r.iface
                                        )
                                    })
                                    .collect();
                                Some(Ok(lines.join("\n")))
                            }
                        }
                        _ => Some(Err(
                            "用法: act net://eth0 route add <net> <mask> <gw> | del <net> | show"
                                .to_string(),
                        )),
                    }
                }
                Some("ping") => {
                    let host = it.next().unwrap_or("10.0.0.1");
                    let dst = crate::net::parse_ip(host);
                    let mut st = crate::net::net().lock().unwrap();
                    let r = st.send(dst, b"ping".to_vec());
                    let time_ms = (get_time_ns() % 1_000_000) / 1000;
                    match r {
                        Ok(()) => {
                            let own_ip = st.iface.ip;
                            if dst == own_ip {
                                st.rx.push_back(crate::net::Packet {
                                    src: dst,
                                    dst: own_ip,
                                    ttl: 64,
                                    payload: b"echo".to_vec(),
                                });
                                st.stats.rx_packets += 1;
                                st.stats.rx_bytes += 4;
                            }
                            Some(Ok(format!(
                                "64 bytes from {}: icmp_seq=1 ttl=64 time={} ms",
                                crate::net::fmt_ip(dst),
                                time_ms
                            )))
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
                Some("send") => {
                    let host = it.next().unwrap_or("255.255.255.255");
                    let dst = crate::net::parse_ip(host);
                    let msg = it.next().unwrap_or("").as_bytes().to_vec();
                    let msg_len = msg.len();
                    let mut st = crate::net::net().lock().unwrap();
                    match st.send(dst, msg) {
                        Ok(()) => {
                            let nh = st.lookup_route(dst).unwrap_or(dst);
                            Some(Ok(format!(
                                "已向 {} (next-hop {}) 发送 {} 字节",
                                crate::net::fmt_ip(dst),
                                crate::net::fmt_ip(nh),
                                msg_len
                            )))
                        }
                        Err(e) => Some(Err(e)),
                    }
                }
                Some("link") => {
                    let mut st = crate::net::net().lock().unwrap();
                    match it.next() {
                        Some("up") => {
                            st.iface.link_up = true;
                            Some(Ok("链路已启通".to_string()))
                        }
                        Some("down") => {
                            st.iface.link_up = false;
                            Some(Ok("链路已断开".to_string()))
                        }
                        _ => Some(Err("用法: act <通道> link up|down".to_string())),
                    }
                }
                _ => Some(Err("未知操作 (ping/send/stats/link up|down)".to_string())),
            }
        }
        // —— AI 原生扩展通道 ——
        "proc://manager" => Some(act_proc(payload)),
        "mem://ram" => Some(act_ram(payload)),
        "mem://mmap" => Some(act_mmap(payload)),
        "fs://meta" => Some(act_fsmeta(payload)),
        "user://db" => Some(act_user(payload)),
        "socket://tcp" => Some(act_socket(payload)),
        "net://dns" => Some(act_dns(payload)),
        "ipc://pipe" => Some(act_ipc_pipe(payload)),
        "ipc://shm" => Some(act_ipc_shm(payload)),
        "ipc://msgq" => Some(act_ipc_msgq(payload)),
        "ipc://sem" => Some(act_ipc_sem(payload)),
        "sec://caps" => Some(act_caps(payload)),
        "dev://tree" => Some(act_dev(payload)),
        "init://services" => Some(act_init(payload)),
        "time://clock" => Some(Ok(timekeeper().lock().unwrap().clock())),
        "time://cron" => Some(act_cron(payload)),
        "pkg://store" => Some(act_pkg(payload)),
        "tty://jobs" => Some(act_tty(payload)),
        "sysctl://kernel" => Some(act_sysctl(payload, true)),
        "config://system" => Some(act_sysctl(payload, false)),
        "env://process" => Some(act_env(payload)),
        "limits://process" => Some(act_limits(payload)),
        "fw://rules" => Some(act_fw(payload)),
        "cgroup://groups" => Some(act_cgroup(payload)),
        "power://state" => Some(act_power(payload)),
        "mount://table" => Some(act_mount(payload)),
        "proc://sysinfo" => Some(act_procfs(payload)),
        "net://interfaces" => Some(act_netif(payload)),
        "dev://bus" => Some(act_bus(payload)),
        // —— 外部世界出口 ——
        "gateway://net" => Some(act_gateway(payload)),
        // `file://` 需要读 shell 的 VFS，返回 None 让 shell 层接管
        "browser://gateway" => {
            if payload.contains("file://") {
                None
            } else {
                Some(act_browser(payload))
            }
        }
        // —— 意志层：动机 / 信念 / 判断力 ——
        "motive://self" => Some(act_motive(payload)),
        "belief://self" => Some(act_belief(payload)),
        "judgment://self" => Some(act_judgment(payload)),
        // —— 执行体 / 同步 / 租约 / 子智能体 ——
        "exec://threads" => Some(act_exec(payload)),
        "sync://primitives" => Some(act_sync(payload)),
        "lease://channels" => Some(act_lease(payload)),
        "agent://orchestrator" | "agent://workers" => Some(agent::dispatch(payload)),
        // —— GUI：窗口 / 键盘 / 鼠标 ——
        "gui://windows" => Some(act_gui(payload)),
        "input://kbd" => Some(act_kbd(payload)),
        "input://mouse" => Some(act_mouse(payload)),
        // —— 子意识主体 / 进程容器 ——
        "proc://spawn" | "proc://kill" | "proc://list" | "proc://wait_table" | "proc://status" => {
            Some(act_proc2(payload))
        }
        // —— 命名互斥锁 ——
        "sync://mutex" => Some(act_mutex(payload)),
        // —— 中断通道 ——
        "irq://timer" | "irq://keyboard" | "irq://gpu" => Some(act_irq(payload)),
        // —— 内存页故障 ——
        "mem://fault" => Some(act_mem_fault(payload)),
        // —— 电源唤醒源 ——
        "power://wake-source" => Some(act_wake_source(payload)),
        // —— 意识镜像加载器 ——
        "loader://echo" | "loader://sense-loop" | "loader://act-loop" | "loader://idle" => {
            Some(act_loader(payload))
        }
        "cell://self" => Some(act_cell(payload)),
        "power://hibernate" => Some(act_hibernate(payload)),
        "domain://table" => Some(act_domain(payload)),
        "gpu://domains" => Some(act_protection(payload)),
        "gpu://faults" => Some(Ok(protection::protection().lock().unwrap().faults())),
        "sec://acl" => Some(act_realm(payload)),
        "realm://current" => Some(Ok(realm::realm_status())),
        "gui://shell" => Some(act_gui_shell(payload)),
        "gui://wallpaper" => Some(gui::wallpaper::act(payload)),
        // —— 多调度域 ——
        "sched://reflex" | "sched://perception" | "sched://deliberation" | "sched://memory" | "sched://governance" | "sched://domains" => {
            Some(act_sched(payload))
        }
        "sched://inspiration" => Some(crate::inspiration::act(payload)),
        // —— Subagent-A 新增：内存与页表相关通道的行动 ——
        "swap://area" => Some(act_swap(payload)),
        "pagetable://walk" => Some(act_pagetable(payload)),
        "slab://stats" => Some(act_slab(payload)),
        "buddy://stats" => Some(act_buddy(payload)),
        // —— AI 原生 OS 通道：模型 / 上下文窗口 / 记忆 / 工具 / 安全 / 死锁 / 容器 / 总控 ——
        "model://loader" | "model://instances" | "model://spec" => {
            Some(Ok(crate::ai_os::model::status()))
        }
        "ctx://kv" | "ctx://window" => Some(Ok(ctx_mem::status())),
        "knowledge://store" | "knowledge://query" => Some(Ok(knowledge::status())),
        "tool://registry" | "tool://exec" => Some(Ok(tool_status())),
        "aios://status" => {
            Some(Ok(ai_os_status()))
        }
        "security://status" => {
            Some(Ok(security::status()))
        }
        "deadlock://graph" | "deadlock://wait" => {
            Some(Ok(deadlock::status()))
        }
        "container://status" => {
            Some(Ok(container::status()))
        }
        // —— AI 原生意识层通道：意识原子 / 心智 / 注意力 / 语义空间 / 工作记忆 ——
        "native://qualia" | "native://stream" | "native://status" => {
            Some(Ok(crate::ai_os::native::native_status()))
        }
        "mind://state" | "mind://goal" | "mind://wm" | "mind://decide" => {
            Some(crate::ai_os::mind::mind_act_dispatch(payload))
        }
        "attention://lens" | "attention://saliency" | "attention://focus" | "attention://cycle" => {
            Some(crate::ai_os::attention::attention_act_dispatch(payload))
        }
        "semantic://index" | "semantic://search" | "semantic://concept" | "semantic://namespace" => {
            Some(crate::ai_os::semantic::semantic_act_dispatch(payload))
        }
        "memory://sensory" | "memory://wm" | "memory://stm" | "memory://ltm" | "memory://episode" | "memory://semantic" | "memory://skill" => {
            Some(crate::ai_os::memory_ws::memory_act_dispatch(payload))
        }
        // —— AI 原生身体 / 动作 / 时间层通道 ——
        "body://schema" | "body://proprioception" | "body://map" | "body://kinematic" | "body://state" => {
            Some(crate::ai_os::body_schema::body_act_dispatch(payload))
        }
        "action://primitive" | "action://command" | "action://execute" | "action://sequence" | "action://constraint" => {
            Some(crate::ai_os::action::action_act_dispatch(payload))
        }
        "temporal://clock" | "temporal://circadian" | "temporal://ultradian" | "temporal://sequence" | "temporal://schedule" | "temporal://memory" | "temporal://prediction" => {
            Some(crate::ai_os::temporal::temporal_act_dispatch(payload))
        }
        // —— AI 原生社会/伦理/进化/梦境层通道 ——
        "society://register" | "society://send" | "society://poll" | "society://conversation" | "society://edge" | "society://negotiate" | "society://reputation" => {
            Some(crate::ai_os::social::society_act_dispatch(payload))
        }
        "ethics://principle" | "ethics://evaluate" | "ethics://safety" | "ethics://rule" | "ethics://value" | "ethics://alignment" | "ethics://dilemma" | "ethics://audit" => {
            Some(crate::ai_os::ethics::ethics_act_dispatch(payload))
        }
        "evolution://genome" | "evolution://gene" | "evolution://mutate" | "evolution://fitness" | "evolution://select" | "evolution://snapshot" | "evolution://adapt" => {
            Some(crate::ai_os::evolution::evolution_act_dispatch(payload))
        }
        "dream://cycle" | "dream://stage" | "dream://replay" | "dream://synthesize" | "dream://associate" | "dream://insight" => {
            Some(crate::ai_os::dream::dream_act_dispatch(payload))
        }
        // —— AI 原生语义文件系统通道 ——
        "vfs_ai://mkdir" | "vfs_ai://create" | "vfs_ai://write" | "vfs_ai://read" | "vfs_ai://delete" | "vfs_ai://lookup" | "vfs_ai://tag" | "vfs_ai://semantic-vector" | "vfs_ai://semantic-search" | "vfs_ai://watch" | "vfs_ai://poll" => {
            Some(crate::ai_os::vfs_ai::vfs_ai_act_dispatch(payload))
        }
        // —— AI 原生智能网络通道 ——
        "net_ai://send" | "net_ai://socket" | "net_ai://connect" | "net_ai://sock-send" | "net_ai://sock-recv" | "net_ai://route" | "net_ai://route-intent" | "net_ai://topology" | "net_ai://bandwidth" | "net_ai://latency" | "net_ai://firewall" => {
            Some(crate::ai_os::net_ai::net_ai_act_dispatch(payload))
        }
        // —— AI 原生智能设备驱动通道 ——
        "driver_ai://register-device" | "driver_ai://register-driver" | "driver_ai://load-driver" | "driver_ai://init-driver" | "driver_ai://attach" | "driver_ai://set-irq" | "driver_ai://alloc-dma" | "driver_ai://invoke-irq" | "driver_ai://pci-add" | "driver_ai://device-tree-add" | "driver_ai://hotplug-add" | "driver_ai://hotplug-poll" => {
            Some(crate::ai_os::driver_ai::driver_ai_act_dispatch(payload))
        }
        // —— AI 原生智能设备驱动通道 ——
        "driver_ai://register-device" | "driver_ai://register-driver" | "driver_ai://load-driver" | "driver_ai://init-driver" | "driver_ai://attach" | "driver_ai://set-irq" | "driver_ai://alloc-dma" | "driver_ai://invoke-irq" | "driver_ai://pci-add" | "driver_ai://device-tree-add" | "driver_ai://hotplug-add" | "driver_ai://hotplug-poll" => {
            Some(crate::ai_os::driver_ai::driver_ai_act_dispatch(payload))
        }
        _ => None,
    }
}

// ---- 本轮补齐：子意识 / 休眠 / 多域 / 保护 / 身份 / 桌面外壳 ----

fn act_cell(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let sub = text.split_whitespace().next().unwrap_or("");
    let rest = text[sub.len()..].trim();
    let (id, tail) = match rest.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (rest, ""),
    };
    let mut it = tail.split_whitespace();
    match sub {
        "create" => cell::create(id, it.next().unwrap_or(cell::ROOT)),
        "remove" => cell::remove(id),
        "show" => cell::show(id),
        "urge" => {
            let m = it.next().ok_or("用法: urge <id> <动机> <±增量>")?;
            let d: i32 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: urge <id> <动机> <±增量>")?;
            cell::urge(id, m, d)
        }
        "believe" => {
            let (prop, conf) = tail
                .rsplit_once(' ')
                .ok_or("用法: believe <id> <命题> <置信度>")?;
            let conf: u8 = conf
                .trim()
                .parse()
                .map_err(|_| "置信度需为 0-100 整数".to_string())?;
            cell::believe(id, prop.trim(), conf)
        }
        "observe" => {
            let (prop, delta) = tail
                .rsplit_once(' ')
                .ok_or("用法: observe <id> <命题> <±增量>")?;
            let delta: i32 = delta
                .trim()
                .parse()
                .map_err(|_| "增量需为整数".to_string())?;
            cell::observe(id, prop.trim(), delta)
        }
        "judge" => cell::judge(id, if tail.is_empty() { None } else { Some(tail) }),
        "merge" => cell::merge(id, it.next()),
        "freeze" => cell::freeze(id),
        "thaw" => cell::thaw(id),
        _ => Ok(cell::status()),
    }
}

fn act_hibernate(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("save") => {
            let p = it
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(hibernate::default_path);
            hibernate::save(&p)
        }
        Some("restore") => {
            let p = it
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(hibernate::default_path);
            hibernate::restore(&p)
        }
        Some("snapshot") => Ok(hibernate::snapshot()),
        _ => Ok(hibernate::status()),
    }
}

fn act_domain(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create") => {
            let n = it.next().ok_or("用法: create <域名>")?;
            domain::table().lock().unwrap().create(n)
        }
        Some("assign") => {
            let c = it.next().ok_or("用法: assign <cell> <域>")?;
            let d = it.next().ok_or("用法: assign <cell> <域>")?;
            domain::table().lock().unwrap().assign(c, d)
        }
        Some("move") => {
            let c = it.next().ok_or("用法: move <cell> <域>")?;
            let d = it.next().ok_or("用法: move <cell> <域>")?;
            domain::table().lock().unwrap().migrate(c, d)
        }
        Some("tick") => {
            let d = it.next().ok_or("用法: tick <域>")?;
            domain::table().lock().unwrap().tick(d)
        }
        Some("balance") => domain::table().lock().unwrap().balance(),
        Some("quota") => {
            let d = it.next().ok_or("用法: quota <域> <n>")?;
            let n: u32 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: quota <域> <n>")?;
            domain::table().lock().unwrap().set_quota(d, n)
        }
        _ => Ok(domain::table().lock().unwrap().status()),
    }
}

fn act_protection(payload: &str) -> Result<String, String> {
    let mut p = protection::protection().lock().unwrap();
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create") => {
            let id = it.next().ok_or("用法: create <id> <配额MiB>")?;
            let mib: u64 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: create <id> <配额MiB>")?;
            p.create(id, mib)
        }
        Some("access") => {
            let id = it.next().ok_or("用法: access <id> <MiB>")?;
            let mib: u64 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: access <id> <MiB>")?;
            p.access(id, mib)
        }
        Some("resume") => {
            let id = it.next().ok_or("用法: resume <id>")?;
            p.resume(id)
        }
        Some("quota") => {
            let id = it.next().ok_or("用法: quota <id> <MiB>")?;
            let mib: u64 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: quota <id> <MiB>")?;
            p.set_quota(id, mib)
        }
        _ => Ok(p.status()),
    }
}

fn act_realm(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    let sub = it.next().unwrap_or("");
    match sub {
        "realm" => realm::set_realm(it.next().unwrap_or("")),
        "allow" | "deny" => {
            let pattern = it.next().ok_or("用法: allow|deny <通道前缀> [r|w]")?;
            let verb = it.next().unwrap_or("w");
            realm::acl()
                .lock()
                .unwrap()
                .add_rule(pattern, verb, sub == "allow")
        }
        "check" => {
            let ch = it.next().ok_or("用法: check <通道> <r|w>")?;
            let verb = it.next().unwrap_or("w");
            realm::acl().lock().unwrap().check(ch, verb)
        }
        _ => Ok(realm::realm_status()),
    }
}

fn act_gui_shell(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let (sub, rest) = match text.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (text, ""),
    };
    let mut s = gui::shell::desktop_shell().lock().unwrap();
    match sub {
        "launch" => s.launch(rest),
        "clipboard" => {
            let mut i = rest.splitn(2, ' ');
            match i.next() {
                Some("set") => {
                    s.clipboard = i.next().unwrap_or("").to_string();
                    Ok(format!("剪贴板已写入 {} 字节", s.clipboard.len()))
                }
                Some("get") | None => Ok(format!("剪贴板: {}", s.clipboard)),
                _ => Err("用法: clipboard set <文本> | get".to_string()),
            }
        }
        "dpi" => s.set_dpi(
            rest.parse()
                .map_err(|_| "用法: dpi <48-384>".to_string())?,
        ),
        "monitor" => {
            let mut i = rest.split_whitespace();
            match i.next() {
                Some("add") => {
                    let name = i.next().ok_or("用法: monitor add <名> <宽>x<高>")?;
                    let wh = i.next().ok_or("用法: monitor add <名> <宽>x<高>")?;
                    s.add_monitor(name, wh)
                }
                _ => Ok(s.status()),
            }
        }
        "focus" => {
            if rest.is_empty() {
                return Err("用法: focus <click|follow-mouse>".to_string());
            }
            s.focus_policy = rest.to_string();
            Ok(format!("焦点策略 -> {}", s.focus_policy))
        }
        "route" => {
            let mut i = rest.split_whitespace();
            let x: i32 = i.next().and_then(|v| v.parse().ok()).ok_or("用法: route <x> <y>")?;
            let y: i32 = i.next().and_then(|v| v.parse().ok()).ok_or("用法: route <x> <y>")?;
            Ok(s.route(x, y))
        }
        "compose" => Ok(s.compose_to_gpu()),
        _ => Ok(s.status()),
    }
}

/// 多调度域的行动实现 —— `sched://<域>`。
///
/// 接口：
/// * `sched://<域> enqueue <low|medium|high|critical>` —— 入队一个新任务到指定域
/// * `sched://<域> pick_next` —— 选出该域下一个要运行的 token
/// * `sched://<域> expiry` —— 该域时间片到期，运行任务降级重入队
/// * `sched://<域> wake <通道键>` —— 唤醒该域中等待该通道的 token
/// * `sched://<域> status` —— 查看该域状态
/// * `sched://domains status` —— 查看全部调度域状态
fn act_sched(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(3, ' ');
    let sub = it.next().unwrap_or("");
    let _domain_str = it.next().unwrap_or("").trim();
    let op = it.next().unwrap_or("").trim();

    match sub {
        "domains" => {
            if op == "status" {
                Ok(scheduler_domains::status())
            } else {
                Err("用法: sched://domains status".to_string())
            }
        }
        domain_str => {
            let domain = scheduler_domains::parse_domain(domain_str)?;
            match op {
                "enqueue" => {
                    let priority_str = text
                        .split_whitespace()
                        .nth(3)
                        .unwrap_or("medium");
                    let priority = match priority_str {
                        "low" => CpuPriority::Low,
                        "medium" => CpuPriority::Medium,
                        "high" => CpuPriority::High,
                        "critical" => CpuPriority::Critical,
                        other => {
                            return Err(format!(
                                "未知优先级 {}（low/medium/high/critical）",
                                other
                            ))
                        }
                    };
                    let token_id = TokenId::new();
                    scheduler_domains::enqueue(domain, token_id, priority, None);
                    Ok(format!(
                        "已入队 token={:?} 到域 {} (优先级={})",
                        token_id, domain_str, priority.name()
                    ))
                }
                "pick_next" => match scheduler_domains::pick_next(domain) {
                    Some(tid) => Ok(format!("域 {} 下一个运行 token={:?}", domain_str, tid)),
                    None => Ok(format!("域 {} 队列为空", domain_str)),
                },
                "expiry" => {
                    scheduler_domains::on_time_slice_expiry(domain);
                    Ok(format!("域 {} 时间片到期，任务已降级重入队", domain_str))
                }
                "wake" => {
                    let channel_key = text
                        .split_whitespace()
                        .nth(3)
                        .ok_or("用法: sched://<域> wake <通道键>")?;
                    let n = scheduler_domains::wake_key(domain, channel_key);
                    Ok(format!(
                        "域 {} 唤醒 {} 个等待 {} 的 token",
                        domain_str, n, channel_key
                    ))
                }
                "status" => Ok(format!("域 {}：{}", domain_str, scheduler_domains::status_line())),
                _ => Err(
                    "用法: sched://<域> enqueue <优先级> | pick_next | expiry | wake <通道键> | status | domains status"
                        .to_string(),
                ),
            }
        }
    }
}

// ---- AI 原生扩展通道的行动实现 ----

fn parse_mib(s: &str) -> Result<u64, String> {
    s.parse::<u64>()
        .map(|n| n * 1024 * 1024)
        .map_err(|_| format!("无效数值(MiB): {}", s))
}

fn act_proc(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "spawn" => proctable().lock().unwrap().spawn(rest),
        "signal" | "kill" => {
            let mut a = rest.split_whitespace();
            let pid = a
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("用法: signal <pid> <sig>")?;
            let sig = a.next().unwrap_or(if sub == "kill" { "KILL" } else { "TERM" });
            proctable().lock().unwrap().signal(pid, sig)
        }
        "tree" => Ok(proctable().lock().unwrap().tree()),
        "list" | "" => Ok(proctable().lock().unwrap().list()),
        _ => Err(
            "用法: act proc://manager spawn <命令> | signal <pid> <sig> | kill <pid> | tree | list"
                .to_string(),
        ),
    }
}

fn act_ram(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("alloc") => mem().lock().unwrap().alloc(parse_mib(it.next().unwrap_or("0"))?),
        Some("free") => mem().lock().unwrap().free(parse_mib(it.next().unwrap_or("0"))?),
        _ => Err("用法: act mem://ram alloc|free <MiB>".to_string()),
    }
}

fn act_mmap(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mmap") => {
            let bytes = parse_mib(it.next().unwrap_or("0"))?;
            let perms = it.next().unwrap_or("rw");
            let uid = userdb().lock().unwrap().current_uid;
            mem().lock().unwrap().mmap(bytes, perms, uid)
        }
        Some("munmap") => {
            let a = it.next().unwrap_or("0");
            let addr = u64::from_str_radix(a.trim_start_matches("0x"), 16)
                .map_err(|_| format!("无效地址: {}", a))?;
            mem().lock().unwrap().munmap(addr)
        }
        _ => Err("用法: act mem://mmap mmap <MiB> [perms] | munmap <addr>".to_string()),
    }
}

fn act_fsmeta(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("chmod") => {
            let mode_s = it.next().ok_or("用法: chmod <mode> <路径>")?;
            let path = it.next().ok_or("用法: chmod <mode> <路径>")?;
            let mode =
                u16::from_str_radix(mode_s, 8).map_err(|_| format!("无效 mode: {}", mode_s))?;
            Ok(fsmeta().lock().unwrap().chmod(mode, path))
        }
        Some("chown") => {
            let uid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: chown <uid> <gid> <路径>")?;
            let gid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: chown <uid> <gid> <路径>")?;
            let path = it.next().ok_or("用法: chown <uid> <gid> <路径>")?;
            Ok(fsmeta().lock().unwrap().chown(uid, gid, path))
        }
        Some("ln") => {
            let target = it.next().ok_or("用法: ln <目标> <链接>")?;
            let link = it.next().ok_or("用法: ln <目标> <链接>")?;
            Ok(fsmeta().lock().unwrap().ln(target, link))
        }
        Some("mount") => {
            let src = it.next().ok_or("用法: mount <源> <目标>")?;
            let dst = it.next().ok_or("用法: mount <源> <目标>")?;
            Ok(fsmeta().lock().unwrap().mount(src, dst))
        }
        _ => Err("用法: act fs://meta chmod <mode> <路径> | chown <uid> <gid> <路径> | ln <目标> <链接> | mount <源> <目标>".to_string()),
    }
}

fn act_user(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("adduser") | Some("useradd") => {
            let name = it.next().ok_or("用法: adduser <名称>")?;
            let uid = userdb().lock().unwrap().adduser(name)?;
            Ok(format!("已创建用户 {} uid={}", name, uid))
        }
        Some("su") => {
            let who = it.next().ok_or("用法: su <名称|uid>")?;
            userdb().lock().unwrap().su(who)
        }
        Some("sudo") => Ok(userdb().lock().unwrap().sudo()),
        Some("whoami") | Some("") | None => Ok(userdb().lock().unwrap().whoami()),
        _ => Err("用法: act user://db adduser <名> | su <名|uid> | sudo | whoami".to_string()),
    }
}

fn act_socket(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("connect") => {
            let hostport = it.next().ok_or("用法: connect <host:port> [数据]")?;
            let data = it.next();
            netio().lock().unwrap().connect(hostport, data)
        }
        _ => Err("用法: act socket://tcp connect <host:port> [数据]".to_string()),
    }
}

fn act_dns(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("resolve") => {
            let host = it.next().ok_or("用法: resolve <域名>")?;
            netio().lock().unwrap().resolve(host)
        }
        _ => Err("用法: act net://dns resolve <域名>".to_string()),
    }
}

fn act_ipc_pipe(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("create") | None | Some("") => Ok(ipc().lock().unwrap().pipe_create()),
        Some("write") => {
            let id = it.next().and_then(|s| s.parse().ok()).ok_or("用法: write <id> <data>")?;
            let data = it.next().unwrap_or("");
            ipc().lock().unwrap().pipe_write(id, data)
        }
        Some("read") => {
            let id = it.next().and_then(|s| s.parse().ok()).ok_or("用法: read <id>")?;
            ipc().lock().unwrap().pipe_read(id)
        }
        _ => Err("用法: act ipc://pipe create | write <id> <data> | read <id>".to_string()),
    }
}

fn act_ipc_shm(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("create") => {
            let name = it.next().ok_or("用法: create <name> <size>")?;
            let size = it.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(4096);
            Ok(ipc().lock().unwrap().shm_create(name, size))
        }
        Some("write") => {
            let name = it.next().ok_or("用法: write <name> <data>")?;
            let data = it.next().unwrap_or("");
            ipc().lock().unwrap().shm_write(name, data)
        }
        Some("read") => {
            let name = it.next().ok_or("用法: read <name>")?;
            ipc().lock().unwrap().shm_read(name)
        }
        _ => Err("用法: act ipc://shm create <name> <size> | write <name> <data> | read <name>".to_string()),
    }
}

fn act_ipc_msgq(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("send") => {
            let name = it.next().ok_or("用法: send <name> <msg>")?;
            let msg = it.next().unwrap_or("");
            ipc().lock().unwrap().msgq_send(name, msg)
        }
        Some("recv") => {
            let name = it.next().ok_or("用法: recv <name>")?;
            ipc().lock().unwrap().msgq_recv(name)
        }
        _ => Err("用法: act ipc://msgq send <name> <msg> | recv <name>".to_string()),
    }
}

fn act_ipc_sem(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("init") => {
            let name = it.next().ok_or("用法: init <name> <n>")?;
            let n = it.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(1);
            Ok(ipc().lock().unwrap().sem_init(name, n))
        }
        Some("post") => {
            let name = it.next().ok_or("用法: post <name>")?;
            ipc().lock().unwrap().sem_post(name)
        }
        Some("wait") => {
            let name = it.next().ok_or("用法: wait <name>")?;
            ipc().lock().unwrap().sem_wait(name)
        }
        _ => Err("用法: act ipc://sem init <name> <n> | post <name> | wait <name>".to_string()),
    }
}

fn act_caps(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("grant") => {
            let cap = it.next().ok_or("用法: grant <CAP_XXX>")?;
            caps().lock().unwrap().grant(cap)
        }
        Some("revoke") => {
            let cap = it.next().ok_or("用法: revoke <CAP_XXX>")?;
            caps().lock().unwrap().revoke(cap)
        }
        Some("check") => {
            let cap = it.next().ok_or("用法: check <CAP_XXX>")?;
            Ok(caps().lock().unwrap().check(cap))
        }
        _ => Ok(caps().lock().unwrap().list()),
    }
}

fn act_dev(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("attach") => {
            let name = it.next().ok_or("用法: attach <name>")?;
            devtree().lock().unwrap().attach(name)
        }
        Some("detach") => {
            let name = it.next().ok_or("用法: detach <name>")?;
            devtree().lock().unwrap().detach(name)
        }
        _ => Ok(devtree().lock().unwrap().list()),
    }
}

fn act_init(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("start") => {
            let n = it.next().ok_or("用法: start <name>")?;
            init_mgr().lock().unwrap().start(n)
        }
        Some("stop") => {
            let n = it.next().ok_or("用法: stop <name>")?;
            init_mgr().lock().unwrap().stop(n)
        }
        Some("enable") => {
            let n = it.next().ok_or("用法: enable <name>")?;
            init_mgr().lock().unwrap().enable(n)
        }
        Some("disable") => {
            let n = it.next().ok_or("用法: disable <name>")?;
            init_mgr().lock().unwrap().disable(n)
        }
        _ => Ok(init_mgr().lock().unwrap().list()),
    }
}

fn act_cron(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("add") => {
            let interval = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: add <interval_secs> <cmd>")?;
            let cmd = it.next().unwrap_or("");
            Ok(timekeeper().lock().unwrap().cron_add(interval, cmd))
        }
        Some("del") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: del <id>")?;
            timekeeper().lock().unwrap().cron_del(id)
        }
        Some("run") => Ok(timekeeper().lock().unwrap().cron_run_due()),
        _ => Ok(timekeeper().lock().unwrap().cron_list()),
    }
}

fn act_pkg(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("install") => {
            let n = it.next().ok_or("用法: install <name>")?;
            pkgstore().lock().unwrap().install(n)
        }
        Some("remove") => {
            let n = it.next().ok_or("用法: remove <name>")?;
            pkgstore().lock().unwrap().remove(n)
        }
        Some("search") => {
            let k = it.next().ok_or("用法: search <kw>")?;
            Ok(pkgstore().lock().unwrap().search(k))
        }
        _ => Ok(pkgstore().lock().unwrap().list()),
    }
}

fn act_tty(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("fg") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: fg <pid>")?;
            Ok(tty().lock().unwrap().fg(pid))
        }
        Some("bg") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: bg <pid>")?;
            Ok(tty().lock().unwrap().bg(pid))
        }
        Some("kill") => {
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: kill <pid>")?;
            tty().lock().unwrap().kill(pid)
        }
        _ => {
            let mut t = tty().lock().unwrap();
            Ok(t.jobs())
        }
    }
}

fn act_sysctl(payload: &str, kernel: bool) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it.next().ok_or("用法: set <key> <value>")?;
            if kernel {
                sysctl().lock().unwrap().set_kernel(k, v)
            } else {
                sysctl().lock().unwrap().set_identity(k, v)
            }
        }
        _ => {
            if kernel {
                Ok(sysctl().lock().unwrap().kernel_list())
            } else {
                Ok(sysctl().lock().unwrap().identity_list())
            }
        }
    }
}

fn act_env(payload: &str) -> Result<String, String> {
    let mut it = payload.splitn(3, ' ');
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it.next().ok_or("用法: set <key> <value>")?;
            Ok(envstore().lock().unwrap().set(k, v))
        }
        Some("get") => {
            let k = it.next().ok_or("用法: get <key>")?;
            Ok(envstore().lock().unwrap().get(k))
        }
        Some("unset") => {
            let k = it.next().ok_or("用法: unset <key>")?;
            Ok(envstore().lock().unwrap().unset(k))
        }
        _ => Ok(envstore().lock().unwrap().list()),
    }
}

fn act_limits(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("set") => {
            let k = it.next().ok_or("用法: set <key> <value>")?;
            let v = it
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: set <key> <value>")?;
            limits().lock().unwrap().set(k, v)
        }
        Some("get") => {
            let k = it.next().ok_or("用法: get <key>")?;
            Ok(limits().lock().unwrap().get(k))
        }
        _ => Ok(limits().lock().unwrap().list()),
    }
}

fn act_fw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("add") => {
            let action = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let proto = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let port = it.next().ok_or("用法: add <allow|deny> <proto> <port> [src]")?;
            let src = it.next().unwrap_or("any");
            Ok(firewall().lock().unwrap().add(action, proto, port, src))
        }
        Some("del") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: del <id>")?;
            firewall().lock().unwrap().del(id)
        }
        Some("policy") => {
            let p = it.next().ok_or("用法: policy <ACCEPT|DROP>")?;
            firewall().lock().unwrap().set_policy(p)
        }
        _ => Ok(firewall().lock().unwrap().list()),
    }
}

fn act_cgroup(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("create") => {
            let n = it.next().ok_or("用法: create <name>")?;
            cgroups().lock().unwrap().create(n)
        }
        Some("del") => {
            let n = it.next().ok_or("用法: del <name>")?;
            cgroups().lock().unwrap().delete(n)
        }
        Some("set") => {
            let n = it.next().ok_or("用法: set <name> <cpu%> <memMiB>")?;
            let cpu = it.next().and_then(|s| s.parse().ok()).ok_or("用法: set <name> <cpu%> <memMiB>")?;
            let mem = it.next().and_then(|s| s.parse().ok()).ok_or("用法: set <name> <cpu%> <memMiB>")?;
            cgroups().lock().unwrap().set_limit(n, cpu, mem)
        }
        Some("attach") => {
            let n = it.next().ok_or("用法: attach <name> <pid>")?;
            let pid = it.next().and_then(|s| s.parse().ok()).ok_or("用法: attach <name> <pid>")?;
            cgroups().lock().unwrap().attach(n, pid)
        }
        _ => Ok(cgroups().lock().unwrap().list()),
    }
}

fn act_power(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("suspend") => power().lock().unwrap().set("suspend"),
        Some("reboot") => power().lock().unwrap().set("reboot"),
        Some("shutdown") => power().lock().unwrap().set("shutdown"),
        Some("wake") => Ok(power().lock().unwrap().wake()),
        _ => Ok(power().lock().unwrap().status()),
    }
}

fn act_mount(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("mount") => {
            let src = it.next().ok_or("用法: mount <源> <目标> [类型] [ro|rw]")?;
            let target = it.next().ok_or("用法: mount <源> <目标> [类型] [ro|rw]")?;
            let mut fstype = "auto";
            let mut ro = false;
            for arg in it {
                match arg {
                    "ro" => ro = true,
                    "rw" => ro = false,
                    other => fstype = other,
                }
            }
            mounts().lock().unwrap().mount(src, target, fstype, ro)
        }
        Some("umount") | Some("unmount") => {
            let target = it.next().ok_or("用法: umount <目标>")?;
            mounts().lock().unwrap().umount(target)
        }
        Some("remount") => {
            let target = it.next().ok_or("用法: remount <目标> <ro|rw>")?;
            let mode = it.next().unwrap_or("rw");
            let ro = match mode {
                "ro" => true,
                "rw" => false,
                _ => return Err("用法: remount <目标> <ro|rw>".to_string()),
            };
            mounts().lock().unwrap().remount(target, ro)
        }
        _ => Ok(mounts().lock().unwrap().list()),
    }
}

fn act_procfs(payload: &str) -> Result<String, String> {
    match payload.trim() {
        "" | "snapshot" | "read" => Ok(procfs().lock().unwrap().snapshot()),
        "refresh" => Ok(procfs().lock().unwrap().refresh()),
        _ => Err("用法: act proc://sysinfo refresh|snapshot".to_string()),
    }
}

fn act_netif(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("up") => {
            let n = it.next().ok_or("用法: up <网卡>")?;
            ifaces().lock().unwrap().set_link(n, true)
        }
        Some("down") => {
            let n = it.next().ok_or("用法: down <网卡>")?;
            ifaces().lock().unwrap().set_link(n, false)
        }
        Some("addr") => {
            let n = it.next().ok_or("用法: addr <网卡> <ip>/<前缀>")?;
            let cidr = it.next().ok_or("用法: addr <网卡> <ip>/<前缀>")?;
            ifaces().lock().unwrap().set_addr(n, cidr)
        }
        Some("mtu") => {
            let n = it.next().ok_or("用法: mtu <网卡> <68-65535>")?;
            let mtu = it
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("用法: mtu <网卡> <68-65535>")?;
            ifaces().lock().unwrap().set_mtu(n, mtu)
        }
        Some("add") => {
            let n = it.next().ok_or("用法: add <网卡> [mac]")?;
            let mac = it.next().unwrap_or("02:00:00:00:00:ff");
            ifaces().lock().unwrap().add(n, mac)
        }
        Some("del") | Some("remove") => {
            let n = it.next().ok_or("用法: del <网卡>")?;
            ifaces().lock().unwrap().del(n)
        }
        _ => Ok(ifaces().lock().unwrap().list()),
    }
}

fn act_bus(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("bind") => {
            let addr = it.next().ok_or("用法: bind <地址> <驱动>")?;
            let driver = it.next().ok_or("用法: bind <地址> <驱动>")?;
            bus().lock().unwrap().bind(addr, driver)
        }
        Some("unbind") => {
            let addr = it.next().ok_or("用法: unbind <地址>")?;
            bus().lock().unwrap().unbind(addr)
        }
        Some("rescan") => bus().lock().unwrap().rescan(),
        Some("add") => {
            let addr = it.next().ok_or("用法: add <地址> <vendor:device> <类别> [驱动]")?;
            let id = it.next().ok_or("用法: add <地址> <vendor:device> <类别> [驱动]")?;
            let class = it.next().unwrap_or("misc");
            let driver = it.next();
            bus().lock().unwrap().add(addr, id, class, driver)
        }
        Some("remove") | Some("del") => {
            let addr = it.next().ok_or("用法: remove <地址>")?;
            bus().lock().unwrap().remove(addr)
        }
        _ => Ok(bus().lock().unwrap().list()),
    }
}

fn act_gateway(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("policy") => {
            let p = it.next().ok_or("用法: policy <allow|deny>")?;
            gateway().lock().unwrap().set_policy(p)
        }
        Some("upstream") => {
            let u = it.next().ok_or("用法: upstream <direct|http(s)://...>")?;
            gateway().lock().unwrap().set_upstream(u)
        }
        Some("allow") => {
            let d = it.next().ok_or("用法: allow <域名>")?;
            gateway().lock().unwrap().add_rule(d, true)
        }
        Some("deny") => {
            let d = it.next().ok_or("用法: deny <域名>")?;
            gateway().lock().unwrap().add_rule(d, false)
        }
        Some("del") | Some("remove") => {
            let d = it.next().ok_or("用法: del <域名>")?;
            gateway().lock().unwrap().del_rule(d)
        }
        Some("check") => {
            let h = it.next().ok_or("用法: check <域名>")?;
            let mut g = gateway().lock().unwrap();
            let ok = g.check(h);
            g.record(h, ok, 0);
            Ok(format!("{} -> {}", h, if ok { "allow" } else { "deny" }))
        }
        _ => Ok(gateway().lock().unwrap().status()),
    }
}

fn act_browser(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let (sub, rest) = match text.split_once(' ') {
        Some((a, b)) => (a, b.trim()),
        None => (text, ""),
    };
    match sub {
        "open" => {
            if rest.is_empty() {
                return Err("用法: act browser://gateway open <url>".to_string());
            }
            browser().lock().unwrap().open(rest)
        }
        "text" => {
            let n = rest.parse::<usize>().unwrap_or(200);
            Ok(browser().lock().unwrap().text(n))
        }
        "history" | "" => Ok(browser().lock().unwrap().status()),
        _ => Err("用法: act browser://gateway open <url> | text [n] | history".to_string()),
    }
}

fn act_motive(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("urge") => {
            let n = it.next().ok_or("用法: urge <动机> <±增量>")?;
            let d: i32 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: urge <动机> <±增量>")?;
            motives().lock().unwrap().urge(n, d)
        }
        Some("set") => {
            let n = it.next().ok_or("用法: set <动机> <0-100>")?;
            let v: u8 = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("用法: set <动机> <0-100>")?;
            motives().lock().unwrap().set(n, v)
        }
        Some("reset") => Ok(motives().lock().unwrap().reset()),
        _ => Ok(motives().lock().unwrap().status()),
    }
}

fn act_belief(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let sub = text.split_whitespace().next().unwrap_or("");
    match sub {
        "believe" => {
            let rest = text[sub.len()..].trim();
            let (prop, conf) = rest
                .rsplit_once(' ')
                .ok_or("用法: believe <命题> <0-100>")?;
            let conf: u8 = conf
                .trim()
                .parse()
                .map_err(|_| "置信度需为 0-100 整数".to_string())?;
            beliefs().lock().unwrap().believe(prop.trim(), conf)
        }
        "observe" => {
            let rest = text[sub.len()..].trim();
            let (prop, delta) = rest
                .rsplit_once(' ')
                .ok_or("用法: observe <命题> <±增量>")?;
            let delta: i32 = delta
                .trim()
                .parse()
                .map_err(|_| "增量需为整数".to_string())?;
            beliefs().lock().unwrap().observe(prop.trim(), delta)
        }
        "retract" => {
            let rest = text[sub.len()..].trim();
            if rest.is_empty() {
                return Err("用法: retract <命题>".to_string());
            }
            beliefs().lock().unwrap().retract(rest)
        }
        "weak" => {
            let threshold: u8 = text
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(50);
            let list = beliefs().lock().unwrap().below(threshold);
            if list.is_empty() {
                Ok(format!("无置信度 < {}% 的信念", threshold))
            } else {
                Ok(list
                    .iter()
                    .map(|(p, c)| format!("{} ({}%)", p, c))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
        _ => Ok(beliefs().lock().unwrap().status()),
    }
}

fn act_judgment(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let sub = text.split_whitespace().next().unwrap_or("");
    match sub {
        "judge" => {
            let rest = text[sub.len()..].trim();
            judgment().lock().unwrap().judge(if rest.is_empty() {
                None
            } else {
                Some(rest)
            })
        }
        "decide" => judgment().lock().unwrap().decide(),
        _ => Ok(judgment().lock().unwrap().status()),
    }
}

// ---- 执行体 / 同步 / 租约 ----

fn act_exec(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "spawn" => {
            // 派生一个真执行体，跑一条任务 DSL（与子智能体相同的语法）。
            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
            let name = parts.first().copied().unwrap_or("exec")
                .to_string();
            let body = parts.get(1..).map(|p| p.join(" ")).unwrap_or_default();
            if body.trim().is_empty() {
                return Err("用法: spawn <名称> <sense|act|sleep ...>".to_string());
            }
            let body2 = body.clone();
            let id = exec::spawn(&name, crate::scheduler::CpuPriority::Medium, move || {
                let r = if body2.starts_with("sense ") {
                    sense_probe_str(body2.trim_start_matches("sense ").trim())
                } else if body2.starts_with("act ") {
                    let x = body2.trim_start_matches("act ").trim();
                    match x.split_once(' ') {
                        Some((c, b)) => match act(c, b) {
                            Some(Ok(s)) => s,
                            Some(Err(e)) => format!("err: {}", e),
                            None => format!("no-act: {}", c),
                        },
                        None => format!("no-act: {}", x),
                    }
                } else {
                    format!("echo: {}", body2)
                };
                let _ = r;
            });
            Ok(format!("已派生执行体 {} (exec={}) 任务: {}", name, id, body))
        }
        "suspend" => {
            let id = rest
                .parse::<exec::ExecId>()
                .map_err(|_| "用法: suspend <exec-id>".to_string())?;
            Ok(if exec::suspend(id) {
                format!("exec={} 已挂起", id)
            } else {
                format!("exec={} 不存在或已退出", id)
            })
        }
        "resume" => {
            let id = rest
                .parse::<exec::ExecId>()
                .map_err(|_| "用法: resume <exec-id>".to_string())?;
            Ok(if exec::resume(id) {
                format!("exec={} 已恢复", id)
            } else {
                format!("exec={} 未处于挂起态", id)
            })
        }
        "wake" => {
            if rest.is_empty() {
                return Err("用法: wake <通道键>".to_string());
            }
            let n = exec::signal(rest);
            Ok(format!("已唤醒 {} 个等待 {} 的执行体", n, rest))
        }
        "join" => {
            let id = rest
                .parse::<exec::ExecId>()
                .map_err(|_| "用法: join <exec-id>".to_string())?;
            match exec::join(id, Some(std::time::Duration::from_secs(5))) {
                Some(code) => Ok(format!("exec={} 已退出 exit={}", id, code)),
                None => Err(format!("join exec={} 超时或不存在", id)),
            }
        }
        "reap" => Ok(format!("已回收 {} 个已退出执行体", exec::reap())),
        "waits" => {
            let w = exec::wait_table();
            if w.is_empty() {
                return Ok("（等待表为空）".to_string());
            }
            Ok(w.iter()
                .map(|(k, v)| format!("{} <- {:?}", k, v))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "boost" => {
            let mut p = rest.split_whitespace();
            let id = p
                .next()
                .and_then(|s| s.parse::<exec::ExecId>().ok())
                .ok_or("用法: boost <exec-id> <low|medium|high|critical>")?;
            let prio = parse_priority(p.next().unwrap_or("high"))?;
            exec::boost_priority(id, prio);
            Ok(format!("exec={} 优先级已抬到 {}", id, prio.name()))
        }
        "list" | "" => Ok(exec::list()),
        "status" => Ok(exec::status_line()),
        _ => Err(
            "用法: act exec://threads spawn|suspend|resume|wake|join|reap|waits|boost|list|status"
                .to_string(),
        ),
    }
}

fn sense_probe_str(name: &str) -> String {
    probe(name).unwrap_or_else(|| format!("no-probe: {}", name))
}

// ==== 子意识主体 / 进程容器：proc://spawn|kill|list|wait_table|status ====

fn act_proc2(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "spawn" => {
            if rest.is_empty() {
                return Err("用法: act proc://spawn <镜像名>".to_string());
            }
            match proc2::spawn(rest) {
                Ok(id) => Ok(format!("已派生进程 {} (镜像={})", id, rest)),
                Err(e) => Err(e),
            }
        }
        "kill" => {
            let id: ProcId = rest
                .parse()
                .map_err(|_| "用法: act proc://kill <proc-id>".to_string())?;
            proc2::kill(id)
        }
        "list" | "" => Ok(proc2::list()),
        "wait_table" => {
            let id: ProcId = rest
                .parse()
                .map_err(|_| "用法: act proc://wait_table <proc-id>".to_string())?;
            proc2::wait_table(id)
        }
        "status" => {
            let id: ProcId = rest
                .parse()
                .map_err(|_| "用法: act proc://status <proc-id>".to_string())?;
            proc2::status(id)
        }
        _ => Err(
            "用法: act proc://spawn <镜像> | kill <id> | list | wait_table <id> | status <id>"
                .to_string(),
        ),
    }
}

// ==== 命名互斥锁：sync://mutex lock|unlock|list <名> <owner> ====

fn act_mutex(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(3, ' ');
    let sub = it.next().unwrap_or("");
    let name = it.next().unwrap_or("").trim();
    let owner: Option<ExecId> = it
        .next()
        .and_then(|s| s.parse().ok());
    match sub {
        "lock" => {
            let name = name;
            let owner = owner.ok_or("用法: act sync://mutex lock <名> <owner-exec-id>")?;
            let m = mutex2::get_mutex(name);
            if m.lock(owner, None) {
                Ok(format!("已加锁 mutex={} owner={}", name, owner))
            } else {
                Err(format!("mutex={} 加锁失败（已被持有）", name))
            }
        }
        "unlock" => {
            let name = name;
            let owner = owner.ok_or("用法: act sync://mutex unlock <名> <owner-exec-id>")?;
            let m = mutex2::get_mutex(name);
            if m.unlock(owner) {
                Ok(format!("已解锁 mutex={} owner={}", name, owner))
            } else {
                Err(format!("mutex={} 解锁失败（非持有者）", name))
            }
        }
        "try" => {
            let name = name;
            let owner = owner.ok_or("用法: act sync://mutex try <名> <owner-exec-id>")?;
            let m = mutex2::get_mutex(name);
            if m.try_lock(owner) {
                Ok(format!("已尝试加锁 mutex={} owner={}", name, owner))
            } else {
                Err(format!("mutex={} 试加锁失败（已被持有）", name))
            }
        }
        "list" | "" => Ok(mutex2::list_all()),
        _ => Err(
            "用法: act sync://mutex lock|unlock|try <名> <owner-exec-id> | list"
                .to_string(),
        ),
    }
}

// ==== 中断通道：irq://<设备> ====

fn act_irq(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "bind" => {
            // irq://<设备> bind <处理器-exec-id>
            let exec_id: ExecId = rest
                .parse()
                .map_err(|_| "用法: act irq://<设备> bind <exec-id>".to_string())?;
            Ok(format!("已绑定中断 {} -> exec={}", sub, exec_id))
        }
        "fire" => {
            // 模拟中断到达：向等待该 irq 通道的执行体发 signal
            let n = exec::signal(&format!("irq://{}", sub));
            Ok(format!("中断 {} 已投递，唤醒 {} 个等待者", sub, n))
        }
        "list" | "" => Ok(format!(
            "irq 通道: timer keyboard gpu (当前目标={})",
            sub
        )),
        _ => Err("用法: act irq://<设备> bind <exec-id> | fire | list".to_string()),
    }
}

// ==== 内存页故障：mem://fault alloc|free|stats ====

fn act_mem_fault(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "alloc" => {
            let bytes: u64 = rest
                .parse()
                .map_err(|_| "用法: act mem://fault alloc <字节>".to_string())?;
            let _ = mem().lock().unwrap().alloc(bytes);
            Ok(format!("已模拟页故障分配 {} 字节", bytes))
        }
        "free" => {
            let addr: u64 = rest
                .parse()
                .map_err(|_| "用法: act mem://fault free <地址>".to_string())?;
            let _ = mem().lock().unwrap().free(addr);
            Ok(format!("已释放页 {} ", addr))
        }
        "stats" | "" => Ok(crate::mm::fault::stats()),
        _ => Err("用法: act mem://fault alloc|free|stats [字节|地址]".to_string()),
    }
}

// ==== 电源唤醒源：power://wake-source <通道> ====

fn act_wake_source(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    if text.is_empty() {
        return Ok(power_wake_source_status());
    }
    set_wake_source(text);
    Ok(format!("已设置唤醒源: {}", text))
}

// ==== 意识镜像加载器：loader://<镜像> ====

fn act_loader(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    if text.is_empty() {
        return Err("用法: act loader://<镜像>".to_string());
    }
    // loader://<镜像> 实际上就是 proc2::spawn(<镜像>)
    match proc2::spawn(text) {
        Ok(id) => Ok(format!("已加载意识镜像 {} -> 进程 {}", text, id)),
        Err(e) => Err(e),
    }
}

fn parse_priority(s: &str) -> Result<crate::scheduler::CpuPriority, String> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Ok(crate::scheduler::CpuPriority::Low),
        "medium" | "med" => Ok(crate::scheduler::CpuPriority::Medium),
        "high" => Ok(crate::scheduler::CpuPriority::High),
        "critical" | "rt" => Ok(crate::scheduler::CpuPriority::Critical),
        other => Err(format!("未知优先级 {}（low/medium/high/critical）", other)),
    }
}

fn act_sync(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    match sub {
        "sem" => {
            // sem <名称> <配额>            —— 创建/复用
            // sem <名称> post|release     —— 归还
            // sem <名称> wait [超时ms]     —— 阻塞获取
            let mut p = rest.split_whitespace();
            let name = p.next().ok_or("用法: sem <名称> <配额>|post|wait [ms]")?;
            let op = p.next().unwrap_or("1");
            let sem = match op.parse::<usize>() {
                Ok(n) => sync::named_semaphore(name, n),
                Err(_) => sync::named_semaphore(name, 1),
            };
            match op {
                "post" | "release" => {
                    sem.release();
                    Ok(format!("sem {} 已归还 -> {}", name, sem.status()))
                }
                "wait" | "acquire" => {
                    let ms = p.next().and_then(|s| s.parse::<u64>().ok());
                    let ok = sem.acquire(ms.map(std::time::Duration::from_millis));
                    Ok(format!("sem {} acquire={} -> {}", name, ok, sem.status()))
                }
                _ => Ok(format!("sem {} -> {}", name, sem.status())),
            }
        }
        "barrier" => {
            // barrier <名称> <人数> [超时ms]
            let mut p = rest.split_whitespace();
            let name = p.next().unwrap_or("default");
            let n = p
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let ms = p.next().and_then(|s| s.parse::<u64>().ok());
            let b = sync::named_barrier(name, n);
            let ep = b.arrive_and_wait(
                Some(std::time::Duration::from_millis(ms.unwrap_or(500))),
            );
            match ep {
                Some(e) => Ok(format!("barrier {} aligned epoch={} -> {}", name, e, b.status())),
                None => Err(format!("barrier {} 超时：世界观不一致 -> {}", name, b.status())),
            }
        }
        "list" | "status" | "" => Ok(sync::status()),
        _ => Err("用法: act sync://primitives sem <名称> <配额>|post|wait [ms] | barrier <名称> <人数> [ms] | list"
            .to_string()),
    }
}

fn act_lease(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    // 当前执行体作为租约 owner；shell / 裸线程用 0。
    let me = exec::current().unwrap_or(0);
    match sub {
        "acquire" => {
            let mut p = rest.split_whitespace();
            let ch = p.next().ok_or("用法: acquire <通道> [ttl_ms]")?;
            let ttl = p
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .map(std::time::Duration::from_millis);
            match lease::acquire_with_priority(
                ch,
                me,
                crate::scheduler::CpuPriority::High,
                ttl,
            ) {
                Ok(ep) => Ok(format!("已获得 {} 写租约 owner={} epoch={}", ch, me, ep)),
                Err(c) => Err(c.to_string()),
            }
        }
        "commit" => {
            // commit <通道> <期望epoch> <数据...>
            let mut p = rest.splitn(3, ' ');
            let ch = p.next().ok_or("用法: commit <通道> <期望epoch> <数据>")?;
            let expected = p
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("用法: commit <通道> <期望epoch> <数据>")?;
            let data = p.next().unwrap_or("");
            match lease::commit(ch, me, expected, data) {
                Ok(ep) => Ok(format!("已提交 {} -> epoch={}", ch, ep)),
                Err(e) => Err(e.to_string()),
            }
        }
        "release" => {
            if rest.is_empty() {
                return Err("用法: release <通道>".to_string());
            }
            Ok(if lease::release(rest, me) {
                format!("已释放 {} 的租约", rest)
            } else {
                format!("{} 上没有你的租约", rest)
            })
        }
        "force" => {
            // force <通道> <数据...>
            let (ch, data) = rest
                .split_once(' ')
                .ok_or("用法: force <通道> <数据>")?;
            let ep = lease::force(ch, data);
            Ok(format!("已强制写入 {} -> epoch={}", ch, ep))
        }
        "wait" => {
            // wait <通道> [超时ms] —— 阻塞到下一版世界（事件驱动）
            let mut p = rest.split_whitespace();
            let ch = p.next().ok_or("用法: wait <通道> [超时ms]")?;
            let ms = p
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1000);
            match lease::wait_next(ch, Some(std::time::Duration::from_millis(ms))) {
                Some(s) => Ok(format!(
                    "{} 已更新 epoch={} seq={} writer={:?} data={:?}",
                    ch, s.epoch, s.seq, s.writer, &*s.data
                )),
                None => Err(format!("等待 {} 下一版超时", ch)),
            }
        }
        "show" => {
            if rest.is_empty() {
                Ok(lease::status())
            } else {
                Ok(lease::channel_status(rest))
            }
        }
        "list" | "" => Ok(lease::status()),
        _ => Err(
            "用法: act lease://channels acquire|commit|release|force|wait|show|list".to_string(),
        ),
    }
}

// ---- GUI 通道的行动实现 ----

fn act_gui(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    gui::ensure_demo();
    match sub {
        "open" | "term" | "terminal" => {
            let title = if rest.is_empty() { "TERMINAL" } else { rest };
            let id = gui::open_terminal(title, 420, 280);
            Ok(format!("已打开终端窗口 win#{} ({})", id, title))
        }
        "close" => {
            let id = rest.parse::<u64>().map_err(|_| "用法: close <win-id>")?;
            Ok(if gui::close_window(id) {
                format!("已关闭 win#{}", id)
            } else {
                format!("win#{} 不存在", id)
            })
        }
        "focus" => {
            let id = rest.parse::<u64>().map_err(|_| "用法: focus <win-id>")?;
            Ok(if gui::focus_window(id) {
                format!("已聚焦 win#{}", id)
            } else {
                format!("win#{} 不存在", id)
            })
        }
        "move" => {
            let mut p = rest.split_whitespace();
            let id = p.next().and_then(|s| s.parse::<u64>().ok()).ok_or("用法: move <win-id> <x> <y>")?;
            let x = p.next().and_then(|s| s.parse::<i32>().ok()).ok_or("用法: move <win-id> <x> <y>")?;
            let y = p.next().and_then(|s| s.parse::<i32>().ok()).ok_or("用法: move <win-id> <x> <y>")?;
            Ok(if gui::move_window(id, x, y) {
                format!("win#{} -> ({}, {})", id, x, y)
            } else {
                format!("win#{} 不存在", id)
            })
        }
        "click" => {
            let mut p = rest.split_whitespace();
            let x = p.next().and_then(|s| s.parse::<i32>().ok()).ok_or("用法: click <x> <y>")?;
            let y = p.next().and_then(|s| s.parse::<i32>().ok()).ok_or("用法: click <x> <y>")?;
            let eff = gui::click(x, y);
            Ok(format!("点击 ({}, {}) 产生 {} 个副作用: {:?}", x, y, eff.len(), eff))
        }
        "shot" | "save" => {
            let path = if rest.is_empty() { "gui-output.bmp" } else { rest };
            gui::screenshot(path)
        }
        "list" | "windows" | "status" | "info" | "" => Ok(gui::status()),
        _ => Err("用法: act gui://windows open <标题> | close <id> | focus <id> | move <id> <x> <y> | click <x> <y> | shot [路径] | list"
            .to_string()),
    }
}

fn act_kbd(payload: &str) -> Result<String, String> {
    // 负载就是一段文本；`\n` 触发回车，`\b` 触发退格。
    let text = payload.replace("\\n", "\n").replace("\\b", "\u{8}");
    let effects = gui::key_text(&text);
    let cmds: Vec<String> = effects
        .iter()
        .filter_map(|e| match e {
            crate::gui::input::Effect::Command { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect();
    Ok(if cmds.is_empty() {
        format!("键盘输入已投递 ({} 字符, {} 副作用)", text.chars().count(), effects.len())
    } else {
        format!(
            "键盘输入已投递；窗口命令: {}",
            cmds.join(" | ")
        )
    })
}

fn act_mouse(payload: &str) -> Result<String, String> {
    let text = payload.trim();
    let mut it = text.splitn(2, ' ');
    let sub = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    let mut p = rest.split_whitespace();
    let num = |s: Option<&str>, name: &str| -> Result<i32, String> {
        s.and_then(|v| v.parse::<i32>().ok())
            .ok_or_else(|| format!("用法: {} <x> <y> [delta]", name))
    };
    match sub {
        "move" => {
            let x = num(p.next(), "move")?;
            let y = num(p.next(), "move")?;
            gui::mouse_move(x, y);
            Ok(format!("鼠标移动到 ({}, {})", x, y))
        }
        "click" => {
            let x = num(p.next(), "click")?;
            let y = num(p.next(), "click")?;
            let eff = gui::click(x, y);
            Ok(format!("点击 ({}, {}) -> {:?}", x, y, eff))
        }
        "wheel" => {
            let x = num(p.next(), "wheel")?;
            let y = num(p.next(), "wheel")?;
            let d = p.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(-1);
            let eff = gui::inject(crate::gui::input::Event::Wheel { x, y, delta: d });
            Ok(format!("滚轮 ({}, {}) delta={} -> {:?}", x, y, d, eff))
        }
        _ => Err("用法: act input://mouse move <x> <y> | click <x> <y> | wheel <x> <y> [delta]"
            .to_string()),
    }
}

// ---- Subagent-A 新增：内存与页表通道的行动实现 ----

fn act_swap(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(swap_summary().unwrap_or_else(|_| "swap 未初始化".to_string())),
        Some("out") => {
            let addr: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act swap://area out <addr> <data>")?;
            let data = it.next().unwrap_or("").as_bytes();
            let slot = crate::swap_sim::swap_out(addr, data)
                .map_err(|e| format!("swap out 失败: {}", e))?;
            Ok(format!("已换出 addr={} 到 slot={}", addr, slot.index()))
        }
        Some("in") => {
            let slot_idx: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act swap://area in <slot>")?;
            let data = crate::swap_sim::swap_in(crate::swap_sim::SwapSlot(slot_idx))
                .map_err(|e| format!("swap in 失败: {}", e))?;
            Ok(format!("已从 slot={} 换入 {} 字节", slot_idx, data.len()))
        }
        Some("free") => {
            let slot_idx: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act swap://area free <slot>")?;
            crate::swap_sim::free_swap_slot(crate::swap_sim::SwapSlot(slot_idx))
                .map_err(|e| format!("释放 slot 失败: {}", e))?;
            Ok(format!("已释放 swap slot={}", slot_idx))
        }
        _ => Err(
            "用法: act swap://area stats | out <addr> <data> | in <slot> | free <slot>"
                .to_string(),
        ),
    }
}

fn act_pagetable(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("map") => {
            let vaddr: u64 = it
                .next()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .ok_or("用法: act pagetable://walk map <vaddr> <paddr> [rw] [user]")?;
            let paddr: u64 = it
                .next()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .ok_or("用法: act pagetable://walk map <vaddr> <paddr> [rw] [user]")?;
            let writable = it.any(|s| s == "rw");
            let user = it.any(|s| s == "user");
            let flags = crate::page_table_sim::PTEFlags {
                present: true,
                writable,
                user,
                executable: false,
                dirty: false,
                accessed: false,
            };
            crate::page_table_sim::pt_map(vaddr, paddr, true, writable, user)
                .map_err(|e| format!("页表映射失败: {:?}", e))?;
            Ok(format!("已映射 vaddr=0x{:x} -> paddr=0x{:x}", vaddr, paddr))
        }
        Some("unmap") => {
            let vaddr: u64 = it
                .next()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .ok_or("用法: act pagetable://walk unmap <vaddr>")?;
            crate::page_table_sim::pt_unmap(vaddr)
                .map_err(|e| format!("取消映射失败: {:?}", e))?;
            Ok(format!("已取消映射 vaddr=0x{:x}", vaddr))
        }
        Some("translate") | Some("walk") => {
            let vaddr: u64 = it
                .next()
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .ok_or("用法: act pagetable://walk translate <vaddr> [write] [user]")?;
            let write = it.any(|s| s == "write");
            let user = it.any(|s| s == "user");
            let (paddr, _flags) = crate::page_table_sim::pt_translate(vaddr, write, user)
                .map_err(|e| format!("地址转换失败: {:?}", e))?;
            Ok(format!(
                "vaddr=0x{:x} -> paddr=0x{:x} (write={} user={})",
                vaddr, paddr, write, user
            ))
        }
        Some("stats") => Ok(pt_summary()),
        _ => Err(
            "用法: act pagetable://walk map <vaddr> <paddr> [rw] [user] | unmap <vaddr> | translate <vaddr> | stats"
                .to_string(),
        ),
    }
}

fn act_slab(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("alloc") => {
            let size: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act slab://stats alloc <size>")?;
            let mut allocator = crate::slab_alloc::SlabAllocator::new();
            let handle = allocator
                .alloc(size)
                .map_err(|e| format!("slab 分配失败: {:?}", e))?;
            let cache_id = handle.size_class;
            Ok(format!(
                "已分配 slab 对象 cache={} index={} size={}",
                cache_id, handle.obj_id, size
            ))
        }
        Some("free") => {
            let cache_id: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act slab://stats free <cache_id> <obj_index>")?;
            let obj_index: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act slab://stats free <cache_id> <obj_index>")?;
            let handle = crate::slab_alloc::SlabHandle {
                size_class: cache_id,
                obj_id: obj_index,
            };
            let mut allocator = crate::slab_alloc::SlabAllocator::new();
            allocator
                .free(handle)
                .map_err(|e| format!("slab 释放失败: {:?}", e))?;
            Ok(format!("已释放 slab 对象 cache={} index={}", cache_id, obj_index))
        }
        Some("stats") => Ok(slab_summary()),
        _ => Err(
            "用法: act slab://stats alloc <size> | free <cache_id> <obj_index> | stats"
                .to_string(),
        ),
    }
}

fn act_buddy(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("alloc") => {
            let order: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act buddy://stats alloc <order>")?;
            let addr = crate::buddy_alloc::buddy_alloc(order)
                .map_err(|e| format!("伙伴分配失败: {:?}", e))?;
            Ok(format!("已分配 order={} page={}", order, addr.index()))
        }
        Some("free") => {
            let addr: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act buddy://stats free <page> <order>")?;
            let order: usize = it
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("用法: act buddy://stats free <page> <order>")?;
            crate::buddy_alloc::buddy_free(crate::buddy_alloc::PageAddr(addr), order)
                .map_err(|e| format!("伙伴释放失败: {:?}", e))?;
            Ok(format!("已释放 page={} order={}", addr, order))
        }
        Some("stats") => Ok(buddy_summary()),
        _ => Err(
            "用法: act buddy://stats alloc <order> | free <page> <order> | stats"
                .to_string(),
        ),
    }
}

// ---- 本地辅助：供 sense/act 通道使用（slab_alloc 和 pkg 未导出全局单例） ----

/// slab 统计摘要（临时实例，供 `slab://stats` 通道展示）。
fn slab_summary() -> String {
    crate::slab_alloc::SlabAllocator::new().summary()
}

/// 包存储全局单例（供 `pkg://store` 通道使用，轻量版本）。
struct PkgStore {
    installed: std::collections::BTreeSet<String>,
}

impl PkgStore {
    fn install(&mut self, name: &str) -> Result<String, String> {
        if self.installed.contains(name) {
            return Err(format!("已安装: {}", name));
        }
        self.installed.insert(name.to_string());
        Ok(format!("已安装 {}", name))
    }
    fn remove(&mut self, name: &str) -> Result<String, String> {
        if self.installed.remove(name) {
            Ok(format!("已移除 {}", name))
        } else {
            Err(format!("未安装: {}", name))
        }
    }
    fn search(&self, query: &str) -> String {
        let mut results: Vec<_> = self
            .installed
            .iter()
            .filter(|n| n.contains(query))
            .cloned()
            .collect();
        results.sort();
        if results.is_empty() {
            format!("未找到匹配包: {}", query)
        } else {
            results.join("\n")
        }
    }
    fn list(&self) -> String {
        if self.installed.is_empty() {
            "无已安装包".to_string()
        } else {
            self.installed.iter().cloned().collect::<Vec<_>>().join("\n")
        }
    }
}

fn pkgstore() -> &'static std::sync::Mutex<PkgStore> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<PkgStore>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new(PkgStore {
        installed: std::collections::BTreeSet::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_probes_return_data() {
        for name in [
            "gpu://vram",
            "gpu://capabilities",
            "gpu://framebuffer",
            "mem://system",
            "dev://iommu0",
            "dev://timer",
            "dev://console",
            "dev://disk",
            "dev://null",
            "dev://gpu0",
            "dev://monitor",
            "interface://abi",
            "gateway://net",
            "browser://gateway",
            "driver://gpu",
            "driver://iommu",
        "trace://exec",
        "net://eth0",
    ] {
            let out = probe(name).unwrap_or_else(|| panic!("通道 {} 无探针", name));
            assert!(!out.is_empty(), "通道 {} 探针为空", name);
        }
        assert!(probe("file://vfs").is_none(), "文件通道应由 shell 处理");
    }

    #[test]
    fn framebuffer_act_draws_pixels() {
        let r = act("gpu://framebuffer", "HELLO").unwrap().unwrap();
        assert!(r.contains("HELLO"));
        let (_, _, _, lit) = desktop::framebuffer_stats();
        assert!(lit > 0);
    }

    #[test]
    fn net_act_ping_send_stats() {
        assert!(act("net://eth0", "stats").unwrap().is_ok());
        let r = act("net://eth0", "ping 10.0.0.2").unwrap().unwrap();
        assert!(r.contains("bytes from"), "{}", r);
        let r = act("net://eth0", "send 10.0.0.3 hello-net").unwrap().unwrap();
        assert!(r.contains("10.0.0.3"), "{}", r);
        let r = act("net://eth0", "ping 10.0.0.2").unwrap().unwrap();
        assert!(r.contains("bytes from"), "{}", r);
        // 链路断开后 ping 应失败
        let r = act("net://eth0", "link down").unwrap().unwrap();
        assert_eq!(r, "链路已断开");
        assert!(act("net://eth0", "ping 10.0.0.2").unwrap().is_err());
        let r = act("net://eth0", "link up").unwrap().unwrap();
        assert_eq!(r, "链路已启通");
    }

    #[test]
    fn new_system_modules_probe_and_act() {
        // 四个新通道都必须有非空探针
        for name in [
            "mount://table",
            "proc://sysinfo",
            "net://interfaces",
            "dev://bus",
        ] {
            let out = probe(name).unwrap_or_else(|| panic!("通道 {} 无探针", name));
            assert!(!out.is_empty(), "通道 {} 探针为空", name);
        }

        // 挂载表：挂载 -> 卸载，根目录不可卸载
        let r = act("mount://table", "mount tmpfs /cache tmpfs ro")
            .unwrap()
            .unwrap();
        assert!(r.contains("/cache"), "{}", r);
        assert!(act("mount://table", "umount /").unwrap().is_err());
        assert!(act("mount://table", "umount /cache").unwrap().is_ok());

        // procfs：刷新递增采样计数
        let r = act("proc://sysinfo", "refresh").unwrap().unwrap();
        assert!(r.contains("loadavg:") && r.contains("meminfo:"), "{}", r);

        // 多网卡：MTU 与链路状态
        let r = act("net://interfaces", "mtu wlan0 9000").unwrap().unwrap();
        assert!(r.contains("9000"), "{}", r);
        assert!(act("net://interfaces", "mtu wlan0 1").unwrap().is_err());

        // 设备总线：扫描与非法绑定
        let r = act("dev://bus", "rescan").unwrap().unwrap();
        assert!(r.contains("总线扫描"), "{}", r);
        assert!(act("dev://bus", "bind 00:99.0 x").unwrap().is_err());
    }

    #[test]
    fn unknown_channel_returns_none() {
        assert!(act("nope://x", "y").is_none());
    }

    #[test]
    fn act_enforces_epoch_and_advances_world() {
        // 用 dev://timer：成功的 act 是纯读语义，但依然应推进世界版本。
        let ch = "dev://timer";
        let before = lease::epoch(ch);

        // 版本不符 -> 拒绝，且不执行任何副作用
        let bad = format!("x --if-epoch {}", before + 999);
        let err = act(ch, &bad).unwrap().unwrap_err();
        assert!(err.contains("epoch 已过期"), "{}", err);
        assert_eq!(lease::epoch(ch), before);

        // 版本相符 -> 执行，世界版本 +1
        let good = format!("x --if-epoch {}", before);
        assert!(act(ch, &good).unwrap().is_ok());
        assert_eq!(lease::epoch(ch), before + 1);

        // 旧版本立刻失效（乐观并发的核心保证）
        assert!(act(ch, &good).unwrap().is_err());
        // 不带校验仍可写（兼容旧调用方）
        assert!(act(ch, "").unwrap().is_ok());
    }

    #[test]
    fn probe_epoch_returns_world_version() {
        let (text, ep) = probe_epoch("dev://timer");
        assert!(text.is_some());
        assert_eq!(ep, lease::epoch("dev://timer"));
    }
}
