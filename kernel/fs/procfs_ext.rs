//! procfs 扩展 —— /proc/[pid]/ 详细信息与系统统计。
//!
//! 为 Hib OS 用户态宏内核提供 /proc 伪文件系统的扩展条目，模拟 Linux
//! /proc 接口：每个进程的 cmdline、status、fd、maps、environ、stat、statm，
//! 以及系统级的 meminfo、cpuinfo、stat、uptime、loadavg、version、modules。
//!
//! 与 `kernel/fs/procfs.rs`（若存在）并行，由主智能体统一集成到 VFS/挂载层。
//! 本模块纯 Rust 实现，不依赖外部 crate，所有进程/系统状态保存在内存
//! 中模拟的内核对象里，可在脱网用户态环境下独立编译与测试。
//!
//! 设计信条:
//! * 伪文件即通道：每个 /proc 条目对应一个只读「文件」，内容由进程/系统
//!   状态动态生成，不落盘；
//! * 与 capability 模型配合：/proc/[pid] 的读取受调用方能力（kernel/caps.rs）
//!   校验，非 uid 硬编码；
//! * 租约友好：读取可结合 kernel/lease.rs 的 epoch 作为快照版本（本模块
//!   提供 snapshot 版本号字段）；
//! * 条目与查询分离：`proc_entry(path) -> Option<String>` 生成文件内容；
//!   `sys_stats() -> SysStats` 返回结构化系统统计（供 sense/act 查询）。
//!
//! 并发: 内部进程表与统计使用 Mutex 保护，读取返回 String/结构体副本，
//! 读写不长期阻塞。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ========================================================================
// 错误
// ========================================================================

/// procfs 错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcfsError {
    /// 进程不存在。
    NotFound { pid: u32 },
    /// 条目不存在。
    EntryNotFound { path: String },
    /// 权限不足（能力不足）。
    PermissionDenied { cap: String },
}

impl std::fmt::Display for ProcfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcfsError::NotFound { pid } => write!(f, "进程不存在: pid={}", pid),
            ProcfsError::EntryNotFound { path } => write!(f, "proc 条目不存在: {}", path),
            ProcfsError::PermissionDenied { cap } => {
                write!(f, "权限不足，需要能力: {}", cap)
            }
        }
    }
}

pub type ProcfsResult<T> = Result<T, ProcfsError>;

// ========================================================================
// 进程模拟状态
// ========================================================================

/// 进程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    Running,
    Sleeping,
    Zombie,
    Dead,
}

impl std::fmt::Display for ProcState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcState::Running => write!(f, "R"),
            ProcState::Sleeping => write!(f, "S"),
            ProcState::Zombie => write!(f, "Z"),
            ProcState::Dead => write!(f, "D"),
        }
    }
}

/// 打开的文件描述符（模拟）。
#[derive(Clone, Debug)]
pub struct ProcFd {
    pub fd: i32,
    pub path: String,
    pub flags: u32,
}

/// 进程完整信息（模拟 PCB 的可见部分）。
#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub gid: u32,
    pub state: ProcState,
    pub cwd: PathBuf,
    pub cmdline: Vec<String>,        // argv
    pub environ: Vec<String>,        // 环境变量 "KEY=VAL"
    pub fds: BTreeMap<i32, ProcFd>,  // fd -> 条目
    /// 虚拟内存区域：起始地址 -> (结束地址, 权限, 来源文件)
    pub vmas: BTreeMap<u64, (u64, String, String)>,
    pub start_time_ns: u64,
    pub utime_ns: u64, // 用户态 CPU 时间
    pub stime_ns: u64, // 内核态 CPU 时间
    pub rss_pages: u64, // 常驻内存页数
    pub vsz_pages: u64, // 虚拟内存页数
}

impl ProcInfo {
    pub fn new(pid: u32, ppid: u32, uid: u32, gid: u32) -> Self {
        Self {
            pid,
            ppid,
            uid,
            gid,
            state: ProcState::Running,
            cwd: PathBuf::from("/"),
            cmdline: Vec::new(),
            environ: Vec::new(),
            fds: BTreeMap::new(),
            vmas: BTreeMap::new(),
            start_time_ns: now_ns(),
            utime_ns: 0,
            stime_ns: 0,
            rss_pages: 0,
            vsz_pages: 0,
        }
    }

    /// 设置命令行（argv）。
    pub fn set_cmdline(&mut self, argv: Vec<String>) {
        self.cmdline = argv;
    }

    /// 设置环境变量。
    pub fn set_environ(&mut self, env: Vec<String>) {
        self.environ = env;
    }

    /// 添加一个打开的 fd。
    pub fn add_fd(&mut self, fd: i32, path: String, flags: u32) {
        self.fds.insert(fd, ProcFd { fd, path, flags });
    }

    /// 添加一个虚拟内存区域。
    pub fn add_vma(&mut self, start: u64, end: u64, perms: String, source: String) {
        self.vmas.insert(start, (end, perms, source));
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

// ========================================================================
// 系统统计
// ========================================================================

/// 系统级统计信息（供 /proc/meminfo、/proc/stat、/proc/uptime 等）。
#[derive(Clone, Debug, Default)]
pub struct SysStats {
    pub uptime_ns: u64,
    pub boot_time_ns: u64,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub mem_available_kb: u64,
    pub mem_buffers_kb: u64,
    pub mem_cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub cpu_user_ns: u64,
    pub cpu_nice_ns: u64,
    pub cpu_system_ns: u64,
    pub cpu_idle_ns: u64,
    pub cpu_iowait_ns: u64,
    pub cpu_irq_ns: u64,
    pub cpu_softirq_ns: u64,
    pub processes_created: u64,
    pub processes_running: u64,
    pub processes_blocked: u64,
    pub context_switches: u64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub page_size_bytes: u64,
    pub os_name: String,
    pub os_version: String,
}

impl SysStats {
    pub fn new() -> Self {
        Self {
            os_name: "Hib OS".to_string(),
            os_version: "1.99.1".to_string(),
            page_size_bytes: 4096,
            boot_time_ns: now_ns(),
            ..Default::default()
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        (now_ns().saturating_sub(self.boot_time_ns)) / 1_000_000_000
    }
}

// ========================================================================
// procfs 扩展主体
// ========================================================================

/// procfs 扩展：维护进程表与系统统计，生成 /proc/[pid]/* 与系统级伪文件内容。
pub struct ProcFsExt {
    /// pid -> 进程信息。
    procs: BTreeMap<u32, ProcInfo>,
    /// 下一个可分配 pid。
    next_pid: u32,
    /// 系统统计。
    stats: SysStats,
}

impl ProcFsExt {
    /// 创建一个新的 procfs 扩展实例。
    pub fn new() -> Self {
        Self {
            procs: BTreeMap::new(),
            next_pid: 1,
            stats: SysStats::new(),
        }
    }

    /// 注册一个进程（返回分配的 pid）。
    pub fn register_process(&mut self, ppid: u32, uid: u32, gid: u32) -> u32 {
        let pid = self.next_pid;
        self.next_pid += 1;
        let proc = ProcInfo::new(pid, ppid, uid, gid);
        self.stats.processes_created += 1;
        self.stats.processes_running += 1;
        self.procs.insert(pid, proc);
        pid
    }

    /// 注销一个进程（进程退出）。
    pub fn unregister_process(&mut self, pid: u32) -> bool {
        if self.procs.remove(&pid).is_some() {
            self.stats.processes_running = self.stats.processes_running.saturating_sub(1);
            true
        } else {
            false
        }
    }

    /// 取得进程引用（只读）。
    pub fn proc(&self, pid: u32) -> Option<&ProcInfo> {
        self.procs.get(&pid)
    }

    /// 取得可变进程引用。
    pub fn proc_mut(&mut self, pid: u32) -> Option<&mut ProcInfo> {
        self.procs.get_mut(&pid)
    }

    /// 取得全部 pid（升序）。
    pub fn pids(&self) -> Vec<u32> {
        self.procs.keys().copied().collect()
    }

    /// 进程总数。
    pub fn process_count(&self) -> usize {
        self.procs.len()
    }

    /// 取得进程引用（只读），不存在则返回 NotFound。
    fn proc_ok(&self, pid: u32) -> ProcfsResult<&ProcInfo> {
        self.procs
            .get(&pid)
            .ok_or_else(|| ProcfsError::NotFound { pid })
    }

    /// 取得系统统计快照。
    pub fn stats(&self) -> SysStats {
        self.stats.clone()
    }

    /// 设置系统统计字段（供外部调度器/内存周期性更新）。
    pub fn set_stats<F>(&mut self, f: F)
    where
        F: FnOnce(&mut SysStats),
    {
        f(&mut self.stats);
    }

    // ------------------------------------------------------------------
    // /proc/[pid]/* 条目内容生成
    // ------------------------------------------------------------------

    /// 生成 /proc/[pid]/cmdline 内容（参数以 NUL 分隔，末尾 NUL）。
    pub fn proc_cmdline(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut out = String::new();
        for (i, arg) in proc.cmdline.iter().enumerate() {
            if i > 0 {
                out.push('\0');
            }
            out.push_str(arg);
        }
        out.push('\0');
        Ok(out)
    }

    /// 生成 /proc/[pid]/status 内容（模拟 Linux /proc/[pid]/status 格式）。
    pub fn proc_status(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut out = String::new();
        out.push_str(&format!("Name: hibos-{}\n", pid));
        out.push_str(&format!("State: {} ({})\n", proc.state as u8, proc.state));
        out.push_str(&format!("Pid: {}\n", proc.pid));
        out.push_str(&format!("PPid: {}\n", proc.ppid));
        out.push_str(&format!("Uid: {} {} {} {}\n", proc.uid, proc.uid, proc.uid, proc.uid));
        out.push_str(&format!("Gid: {} {} {} {}\n", proc.gid, proc.gid, proc.gid, proc.gid));
        out.push_str(&format!("Cwd: {}\n", proc.cwd.display()));
        out.push_str(&format!("StartTime: {}\n", proc.start_time_ns));
        out.push_str(&format!("Utime: {}\n", proc.utime_ns));
        out.push_str(&format!("Stime: {}\n", proc.stime_ns));
        out.push_str(&format!("RssPages: {}\n", proc.rss_pages));
        out.push_str(&format!("VszPages: {}\n", proc.vsz_pages));
        out.push_str(&format!("FdCount: {}\n", proc.fds.len()));
        Ok(out)
    }

    /// 生成 /proc/[pid]/fd 目录列表（每行一个 fd 号）。
    pub fn proc_fd_list(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut fds: Vec<i32> = proc.fds.keys().copied().collect();
        fds.sort();
        let mut out = String::new();
        for fd in fds {
            out.push_str(&format!("{}\n", fd));
        }
        Ok(out)
    }

    /// 生成 /proc/[pid]/fdinfo/<fd> 链接目标（伪符号链接，返回路径）。
    pub fn proc_fd_link(&self, pid: u32, fd: i32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let entry = proc
            .fds
            .get(&fd)
            .ok_or_else(|| ProcfsError::EntryNotFound {
                path: format!("/proc/{}/fd/{}", pid, fd),
            })?;
        Ok(entry.path.clone())
    }

    /// 生成 /proc/[pid]/maps 内容（虚拟内存区域列表）。
    pub fn proc_maps(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut out = String::new();
        for (start, (end, perms, source)) in &proc.vmas {
            out.push_str(&format!(
                "{:x}-{:x} {} {}\n",
                start, end, perms, source
            ));
        }
        if out.is_empty() {
            // 没有 VMA 时输出一行占位，便于测试。
            out.push_str("00000000-00001000 r-xp [vvar]\n");
        }
        Ok(out)
    }

    /// 生成 /proc/[pid]/environ 内容（环境变量以 NUL 分隔）。
    pub fn proc_environ(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut out = String::new();
        for (i, kv) in proc.environ.iter().enumerate() {
            if i > 0 {
                out.push('\0');
            }
            out.push_str(kv);
        }
        out.push('\0');
        Ok(out)
    }

    /// 生成 /proc/[pid]/stat 内容（模拟 Linux 格式的一行）。
    pub fn proc_stat(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        let mut out = String::new();
        out.push_str(&format!("{} ({}) {}", proc.pid, format!("hibos-{}", proc.pid), proc.state));
        // 余下字段为空格分隔的数字（与 Linux /proc/[pid]/stat 对齐；此处简化，
        // 未实现的字段填 0，避免格式占位符与参数数量不匹配）。
        let rest: Vec<u64> = vec![
            proc.ppid as u64, 0, 0, 0, 0, // ppid, pgrp, session, tty_nr, tty_pgrp
            0, // flags
            0, 0, 0, 0, // minflt, cminflt, majflt, cmajflt
            proc.utime_ns,
            proc.stime_ns,
            0, 0, // cutime, cstime
            0, 0, 0, 0, // priority, nice, num_threads, itrealvalue
            proc.start_time_ns,
            proc.vsz_pages * 4096,
            proc.rss_pages * 4096,
            proc.rss_pages,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        for v in rest {
            out.push(' ');
            out.push_str(&v.to_string());
        }
        out.push('\n');
        Ok(out)
    }

    /// 生成 /proc/[pid]/statm 内容（内存页统计：size resident shared ...）。
    pub fn proc_statm(&self, pid: u32) -> ProcfsResult<String> {
        let proc = self.proc_ok(pid)?;
        Ok(format!(
            "{} {} {} {} {} {} {}",
            proc.vsz_pages,
            proc.rss_pages,
            proc.rss_pages, // shared
            0,              // text
            0,              // lib
            0,              // data
            0,              // dt
        ))
    }

    /// 生成 /proc/[pid]/wchan 内容（等待通道地址，简化为 0）。
    pub fn proc_wchan(&self, pid: u32) -> ProcfsResult<String> {
        let _ = self.proc_ok(pid)?;
        Ok("0".to_string())
    }

    // ------------------------------------------------------------------
    // 系统级 /proc 条目
    // ------------------------------------------------------------------

    /// 生成 /proc/meminfo 内容。
    pub fn proc_meminfo(&self) -> String {
        let s = &self.stats;
        format!(
            "MemTotal: {}\tkB\nMemFree: {}\tkB\nMemAvailable: {}\tkB\nBuffers: {}\tkB\nCached: {}\tkB\nSwapTotal: {}\tkB\nSwapFree: {}\tkB\n",
            s.mem_total_kb,
            s.mem_free_kb,
            s.mem_available_kb,
            s.mem_buffers_kb,
            s.mem_cached_kb,
            s.swap_total_kb,
            s.swap_free_kb
        )
    }

    /// 生成 /proc/stat 内容（CPU 累计时间与中断）。
    pub fn proc_cpu_stat(&self) -> String {
        let s = &self.stats;
        let total_user = s.cpu_user_ns + s.cpu_nice_ns + s.cpu_system_ns + s.cpu_idle_ns;
        format!(
            "cpu  {} {} {} {} {} {} {} {}\nintr {}\nctxt {}\nprocesses {}\nprocs_running {}\nprocs_blocked {}\n",
            total_user,
            s.cpu_user_ns,
            s.cpu_nice_ns,
            s.cpu_system_ns,
            s.cpu_idle_ns,
            s.cpu_iowait_ns,
            s.cpu_irq_ns,
            s.cpu_softirq_ns,
            0,
            s.context_switches,
            s.processes_created,
            s.processes_running,
            s.processes_blocked
        )
    }

    /// 生成 /proc/uptime 内容（秒，空闲秒）。
    pub fn proc_uptime(&self) -> String {
        let up = self.stats.uptime_secs();
        format!("{} {:.2}\n", up, up as f64)
    }

    /// 生成 /proc/loadavg 内容（1/5/15 分钟负载）。
    pub fn proc_loadavg(&self) -> String {
        let s = &self.stats;
        format!(
            "{:.2} {:.2} {:.2} {}/{} {}\n",
            s.load1, s.load5, s.load15,
            s.processes_running,
            self.procs.len(),
            1
        )
    }

    /// 生成 /proc/version 内容。
    pub fn proc_version(&self) -> String {
        format!(
            "{} version {}\n",
            self.stats.os_name, self.stats.os_version
        )
    }

    /// 生成 /proc/modules 内容（已加载模块列表，本模拟器为空）。
    pub fn proc_modules(&self) -> String {
        String::new()
    }

    /// 解析 /proc 路径，返回条目内容。
    ///
    /// 支持:
    /// * /proc/[pid]/cmdline|status|fd|fdinfo/<n>|maps|environ|stat|statm|wchan
    /// * /proc/meminfo|stat|uptime|loadavg|version|modules
    /// * /proc/pids（全部 pid 列表）
    pub fn read_entry(&self, path: &str) -> ProcfsResult<String> {
        // 按 '/' 或 '\\' 分割并过滤空分量，兼容 Unix 与 Windows 路径。
        let comps: Vec<&str> = path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();
        if comps.is_empty() || comps[0] != "proc" {
            return Err(ProcfsError::EntryNotFound { path: path.to_string() });
        }
        match comps.get(1).copied() {
            Some("meminfo") => Ok(self.proc_meminfo()),
            Some("stat") => Ok(self.proc_cpu_stat()),
            Some("uptime") => Ok(self.proc_uptime()),
            Some("loadavg") => Ok(self.proc_loadavg()),
            Some("version") => Ok(self.proc_version()),
            Some("modules") => Ok(self.proc_modules()),
            Some("pids") => {
                let mut out = String::new();
                for pid in self.pids() {
                    out.push_str(&format!("{}\n", pid));
                }
                Ok(out)
            }
            Some(pid_str) => {
                let pid: u32 = pid_str
                    .parse()
                    .map_err(|_| ProcfsError::EntryNotFound { path: path.to_string() })?;
                let rest = &comps[2..];
                match rest.first().copied() {
                    Some("cmdline") => self.proc_cmdline(pid),
                    Some("status") => self.proc_status(pid),
                    Some("fd") => match rest.get(1) {
                        None => self.proc_fd_list(pid),
                        Some(_fd_str) => {
                            // /proc/<pid>/fd/<n> 或 /proc/<pid>/fdinfo/<n>
                            let fd: i32 = rest
                                .get(1)
                                .ok_or_else(|| ProcfsError::EntryNotFound {
                                    path: path.to_string(),
                                })?
                                .parse()
                                .map_err(|_| ProcfsError::EntryNotFound {
                                    path: path.to_string(),
                                })?;
                            match rest.get(2).copied() {
                                Some("info") => self.proc_fd_link(pid, fd),
                                _ => self.proc_fd_link(pid, fd),
                            }
                        }
                    },
                    Some("maps") => self.proc_maps(pid),
                    Some("environ") => self.proc_environ(pid),
                    Some("stat") => self.proc_stat(pid),
                    Some("statm") => self.proc_statm(pid),
                    Some("wchan") => self.proc_wchan(pid),
                    _ => Err(ProcfsError::EntryNotFound { path: path.to_string() }),
                }
            }
            None => Err(ProcfsError::EntryNotFound { path: path.to_string() }),
        }
    }

    /// 能力感知读取：在读取 /proc/[pid] 条目前校验调用方能力。
    ///
    /// `cap` 为调用方持有的能力（kernel/caps.rs 风格，如 "CAP_SYS_PTRACE"）；
    /// 读取其他进程的详细信息需要 CAP_SYS_PTRACE，读取自身则无需。
    pub fn read_entry_with_cap(
        &self,
        caller_pid: u32,
        caller_caps: &[String],
        path: &str,
    ) -> ProcfsResult<String> {
        // 若路径涉及 /proc/[pid] 且 pid 不是调用方自身，则要求 CAP_SYS_PTRACE。
        let target_pid = extract_target_pid(path);
        if let Some(tpid) = target_pid {
            if tpid != caller_pid && !caller_caps.iter().any(|c| c == "CAP_SYS_PTRACE") {
                return Err(ProcfsError::PermissionDenied {
                    cap: "CAP_SYS_PTRACE".to_string(),
                });
            }
        }
        self.read_entry(path)
    }

    /// 取得结构化系统统计（供 sense/act 查询，不走伪文件字符串）。
    pub fn sys_stats(&self) -> SysStats {
        self.stats.clone()
    }
}

impl Default for ProcFsExt {
    fn default() -> Self {
        Self::new()
    }
}

/// 从路径中提取 /proc/[pid] 的 pid（若有）。按 '/' 或 '\\' 分割，兼容平台。
fn extract_target_pid(path: &str) -> Option<u32> {
    let parts: Vec<&str> = path
        .split(|c| c == '/' || c == '\\')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() || parts[0] != "proc" {
        return None;
    }
    parts.get(1).and_then(|s| s.parse::<u32>().ok())
}

// ========================================================================
// 全局单例（供 sense/act 与其他模块使用）
// ========================================================================

static PROC_FS_EXT: OnceLock<Mutex<ProcFsExt>> = OnceLock::new();

/// 取得全局 procfs 扩展单例。
pub fn procfs_ext() -> &'static Mutex<ProcFsExt> {
    PROC_FS_EXT.get_or_init(|| Mutex::new(ProcFsExt::new()))
}

/// `proc/sysinfo` 通道的 probe：返回系统统计快照文本。
pub fn procfs_ext_probe(_name: &str) -> Option<String> {
    let g = procfs_ext().lock().unwrap();
    let s = g.sys_stats();
    Some(format!(
        "uptime={}s mem_total={}kB mem_free={}kB cpu_user={}ns cpu_idle={}ns procs={} ctx_switches={}",
        s.uptime_secs(),
        s.mem_total_kb,
        s.mem_free_kb,
        s.cpu_user_ns,
        s.cpu_idle_ns,
        s.processes_running,
        s.context_switches
    ))
}

/// `proc/sysinfo` 通道的 act：执行 procfs 操作。
///
/// 支持的负载:
/// * `read <路径>` —— 读取 /proc 条目内容
/// * `spawn <ppid> <uid> <gid>` —— 注册模拟进程
/// * `exit <pid>` —— 注销模拟进程
/// * `stats` —— 返回系统统计文本
pub fn procfs_ext_act(_name: &str, payload: &str) -> Option<Result<String, String>> {
    let mut g = procfs_ext().lock().unwrap();
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("read") => {
            let path = match it.next() {
                Some(p) => p,
                None => return Some(Err("用法: read <路径>".to_string())),
            };
            Some(match g.read_entry(path) {
                Ok(content) => Ok(content),
                Err(e) => Err(e.to_string()),
            })
        }
        Some("spawn") => {
            let ppid = match it.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => return Some(Err("用法: spawn <ppid> <uid> <gid>".to_string())),
            };
            let uid = match it.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => return Some(Err("缺少 uid".to_string())),
            };
            let gid = match it.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => return Some(Err("缺少 gid".to_string())),
            };
            let pid = g.register_process(ppid, uid, gid);
            Some(Ok(format!("进程已注册 pid={}", pid)))
        }
        Some("exit") => {
            let pid = match it.next().and_then(|s| s.parse::<u32>().ok()) {
                Some(v) => v,
                None => return Some(Err("用法: exit <pid>".to_string())),
            };
            Some(if g.unregister_process(pid) {
                Ok(format!("进程已退出 pid={}", pid))
            } else {
                Err(format!("进程不存在 pid={}", pid))
            })
        }
        Some("stats") => {
            let s = g.sys_stats();
            Some(Ok(format!(
                "uptime={}s procs={} running={} mem={}kB/{}kB load={:.2}",
                s.uptime_secs(),
                s.processes_running,
                s.processes_running,
                s.mem_free_kb,
                s.mem_total_kb,
                s.load1
            )))
        }
        _ => Some(Err(
            "用法: read|spawn|exit|stats <参数>".to_string(),
        )),
    }
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> ProcFsExt {
        ProcFsExt::new()
    }

    #[test]
    fn register_and_pids() {
        let mut p = fs();
        let pid1 = p.register_process(0, 0, 0);
        let pid2 = p.register_process(pid1, 1000, 1000);
        assert!(pid1 > 0);
        assert!(pid2 > pid1);
        let pids = p.pids();
        assert!(pids.contains(&pid1));
        assert!(pids.contains(&pid2));
        assert_eq!(p.process_count(), 2);
    }

    #[test]
    fn unregister_process() {
        let mut p = fs();
        let pid = p.register_process(0, 0, 0);
        assert!(p.unregister_process(pid));
        assert!(!p.unregister_process(pid)); // 重复
        assert_eq!(p.process_count(), 0);
    }

    #[test]
    fn proc_status_and_cmdline() {
        let mut p = fs();
        let pid = p.register_process(0, 0, 0);
        p.proc_mut(pid).unwrap().set_cmdline(vec!["hibos-init".to_string(), "--demo".to_string()]);
        let status = p.proc_status(pid).unwrap();
        assert!(status.contains(&format!("Pid: {}", pid)));
        assert!(status.contains("State:"));
        assert!(status.contains("Uid:"));
        let cmdline = p.proc_cmdline(pid).unwrap();
        assert_eq!(cmdline, "hibos-init\0--demo\0");
    }

    #[test]
    fn proc_fd_and_maps() {
        let mut p = fs();
        let pid = p.register_process(0, 0, 0);
        p.proc_mut(pid)
            .unwrap()
            .add_fd(3, "/tmp/test".to_string(), 0);
        p.proc_mut(pid)
            .unwrap()
            .add_vma(0x400000, 0x401000, "r-xp".to_string(), "/bin/sh".to_string());
        let fd_list = p.proc_fd_list(pid).unwrap();
        assert!(fd_list.contains("3"));
        let link = p.proc_fd_link(pid, 3).unwrap();
        assert_eq!(link, "/tmp/test");
        assert!(p.proc_fd_link(pid, 99).is_err());
        let maps = p.proc_maps(pid).unwrap();
        assert!(maps.contains("400000-401000"));
        assert!(maps.contains("r-xp"));
    }

    #[test]
    fn proc_environ_and_stat_and_statm() {
        let mut p = fs();
        let pid = p.register_process(0, 0, 0);
        p.proc_mut(pid)
            .unwrap()
            .set_environ(vec!["PATH=/bin".to_string(), "HOME=/root".to_string()]);
        let env = p.proc_environ(pid).unwrap();
        assert!(env.contains("PATH=/bin"));
        assert!(env.contains("HOME=/root"));
        let stat = p.proc_stat(pid).unwrap();
        assert!(stat.starts_with(&format!("{} (hibos-{})", pid, pid)));
        let statm = p.proc_statm(pid).unwrap();
        assert!(!statm.is_empty());
        let wchan = p.proc_wchan(pid).unwrap();
        assert_eq!(wchan, "0");
    }

    #[test]
    fn proc_meminfo_uptime_loadavg_version() {
        let p = fs();
        let meminfo = p.proc_meminfo();
        assert!(meminfo.contains("MemTotal:"));
        assert!(meminfo.contains("MemFree:"));
        let uptime = p.proc_uptime();
        assert!(uptime.contains('.'));
        let loadavg = p.proc_loadavg();
        assert!(!loadavg.is_empty());
        let version = p.proc_version();
        assert!(version.contains("Hib OS"));
        let modules = p.proc_modules();
        assert!(modules.is_empty()); // 模拟器无模块
    }

    #[test]
    fn proc_cpu_stat() {
        let p = fs();
        let stat = p.proc_cpu_stat();
        assert!(stat.starts_with("cpu "));
        assert!(stat.contains("ctxt"));
        assert!(stat.contains("processes"));
    }

    #[test]
    fn read_entry_system_paths() {
        let p = fs();
        assert!(p.read_entry("/proc/meminfo").is_ok());
        assert!(p.read_entry("/proc/stat").is_ok());
        assert!(p.read_entry("/proc/uptime").is_ok());
        assert!(p.read_entry("/proc/loadavg").is_ok());
        assert!(p.read_entry("/proc/version").is_ok());
        assert!(p.read_entry("/proc/modules").is_ok());
        assert!(p.read_entry("/proc/pids").is_ok());
        assert!(p.read_entry("/proc/nope").is_err());
        assert!(p.read_entry("/proc/1/cmdline").is_err()); // pid 不存在
    }

    #[test]
    fn read_entry_pid_paths() {
        let mut p = fs();
        let pid = p.register_process(0, 0, 0);
        assert!(p.read_entry(&format!("/proc/{}/status", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/cmdline", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/fd", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/maps", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/environ", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/stat", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/statm", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/wchan", pid)).is_ok());
        assert!(p.read_entry(&format!("/proc/{}/fd/3", pid)).is_err()); // fd 3 不存在
    }

    #[test]
    fn capability_guard() {
        let mut p = fs();
        let target = p.register_process(0, 0, 0);
        let caller = p.register_process(0, 1000, 1000);
        // 读取他人进程且无 CAP_SYS_PTRACE -> 拒绝。
        let path = format!("/proc/{}/status", target);
        assert!(matches!(
            p.read_entry_with_cap(caller, &[], &path).unwrap_err(),
            ProcfsError::PermissionDenied { .. }
        ));
        // 带 CAP_SYS_PTRACE -> 允许。
        assert!(p.read_entry_with_cap(caller, &["CAP_SYS_PTRACE".to_string()], &path).is_ok());
        // 读取自身 -> 无需能力。
        let own = format!("/proc/{}/status", caller);
        assert!(p.read_entry_with_cap(caller, &[], &own).is_ok());
    }

    #[test]
    fn sys_stats_snapshot() {
        let mut p = fs();
        p.register_process(0, 0, 0);
        let s = p.sys_stats();
        assert_eq!(s.processes_created, 1);
        assert_eq!(s.processes_running, 1);
        assert!(!s.os_name.is_empty());
    }

    #[test]
    fn procfs_ext_probe_and_act() {
        let out = procfs_ext_act("proc/sysinfo", "spawn 0 0 0").unwrap().unwrap();
        assert!(out.contains("pid="));
        let stats_out = procfs_ext_act("proc/sysinfo", "stats").unwrap().unwrap();
        assert!(stats_out.contains("uptime="));
        let read_out = procfs_ext_act(
            "proc/sysinfo",
            "read /proc/version",
        )
        .unwrap()
        .unwrap();
        assert!(read_out.contains("Hib OS"));
        // probe 返回系统快照。
        let probe = procfs_ext_probe("proc/sysinfo").unwrap();
        assert!(probe.contains("uptime="));
        // 清理：尝试退出刚注册的进程（全局单例中 pid 未知，允许成功或失败）。
        let _ = procfs_ext_act("proc/sysinfo", "exit 0");
    }
}
