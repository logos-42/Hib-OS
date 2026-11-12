//! POSIX 兼容层 —— 在 Hib OS 用户态宏内核上提供类 POSIX 系统调用。
//!
//! 本模块与 `kernel/syscall` 的 `AiSystemCall` **并行存在，不替换**现有系统调用。
//! 所有操作都通过统一通道的 `sense`/`act` 抽象来暴露：打开文件、读写、进程
//! 控制、信号等映射到内存中模拟的内核对象（文件描述符表、进程表、信号掩码、
//! 工作目录、文件权限），适合用户态宏内核原型做 POSIX 兼容性演示与分层验证。
//!
//! 设计要点:
//! * `PosixSyscall` 枚举为类型安全的系统调用编号与参数；
//! * `PosixLayer::handle(syscall, args)` 入口，`args` 为寄存器风格的原始参数，
//!   供底层裸系统调用分派使用；结构化参数直接由 `PosixSyscall` 字段承载；
//! * `PosixError` 覆盖常见 errno，携带错误码、名称与中文描述；
//! * 内存文件系统：`PathBuf -> Vec<u8>`，文件描述符表维护打开文件的偏移与权限；
//! * 进程表：PID/PPID/UID/GID/工作目录/信号掩码/状态/退出码；
//! * 信号：简化的信号掩码与处理器登记，`kill`/`signal`/`sigaction`/`sigprocmask`；
//! * 所有公共函数均有中文 doc comment，文件末尾附带 `#[cfg(test)]` 单元测试。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ========================================================================
// errno
// ========================================================================

/// POSIX errno 值。
///
/// 每个变体携带标准错误码（与 Linux errno 值一致）、名称和中文描述，
/// 方便上层 shell 与审计日志做可读化输出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosixError {
    /// 操作不允许 (Operation not permitted)
    EPERM = 1,
    /// 没有这样的文件或目录 (No such file or directory)
    ENOENT = 2,
    /// 没有这样的进程 (No such process)
    ESRCH = 3,
    /// 中断的系统调用 (Interrupted system call)
    EINTR = 4,
    /// 输出/输出错误 (Input/output error)
    EIO = 5,
    /// 没有这样的文件或目录 (No such file or directory，文件句柄语境)
    EBADF = 9,
    /// 不够内存 (Cannot allocate memory)
    ENOMEM = 12,
    /// 权限拒绝 (Permission denied)
    EACCES = 13,
    /// 不适合的文件类型 (Not a directory)
    ENOTDIR = 20,
    /// 是一个目录 (Is a directory)
    EISDIR = 21,
    /// 参数无效 (Invalid argument)
    EINVAL = 22,
    /// 打开文件过多 (File table overflow)
    ENFILE = 23,
    /// 打开的文件过多 (Too many open files)
    EMFILE = 24,
    /// 不适当的 seek 操作 (Illegal seek)
    ESPIPE = 29,
    /// 只读文件系统 (Read-only file system)
    EROFS = 30,
    /// 文件名太长 (File name too long)
    ENAMETOOLONG = 36,
    /// 无效的寄存器/跨设备链接 (Invalid cross-device link)
    EXDEV = 18,
    /// 过多的符号链接层次 (Too many levels of symbolic links)
    ELOOP = 40,
}

impl PosixError {
    /// 返回标准 errno 整数码。
    pub fn code(self) -> i32 {
        self as i32
    }

    /// 返回 errno 符号名称。
    pub fn name(self) -> &'static str {
        match self {
            PosixError::EPERM => "EPERM",
            PosixError::ENOENT => "ENOENT",
            PosixError::ESRCH => "ESRCH",
            PosixError::EINTR => "EINTR",
            PosixError::EIO => "EIO",
            PosixError::EBADF => "EBADF",
            PosixError::ENOMEM => "ENOMEM",
            PosixError::EACCES => "EACCES",
            PosixError::ENOTDIR => "ENOTDIR",
            PosixError::EISDIR => "EISDIR",
            PosixError::EINVAL => "EINVAL",
            PosixError::ENFILE => "ENFILE",
            PosixError::EMFILE => "EMFILE",
            PosixError::ESPIPE => "ESPIPE",
            PosixError::EROFS => "EROFS",
            PosixError::ENAMETOOLONG => "ENAMETOOLONG",
            PosixError::EXDEV => "EXDEV",
            PosixError::ELOOP => "ELOOP",
        }
    }

    /// 返回中文描述。
    pub fn desc(self) -> &'static str {
        match self {
            PosixError::EPERM => "操作不允许",
            PosixError::ENOENT => "没有这样的文件或目录",
            PosixError::ESRCH => "没有这样的进程",
            PosixError::EINTR => "中断的系统调用",
            PosixError::EIO => "输入/输出错误",
            PosixError::EBADF => "无效的文件描述符",
            PosixError::ENOMEM => "内存不足",
            PosixError::EACCES => "权限拒绝",
            PosixError::ENOTDIR => "不是一个目录",
            PosixError::EISDIR => "是一个目录",
            PosixError::EINVAL => "参数无效",
            PosixError::ENFILE => "系统打开的文件过多",
            PosixError::EMFILE => "打开的文件过多",
            PosixError::ESPIPE => "不适当的 seek 操作",
            PosixError::EROFS => "只读文件系统",
            PosixError::ENAMETOOLONG => "文件名太长",
            PosixError::EXDEV => "无效的跨设备链接",
            PosixError::ELOOP => "过多的符号链接层次",
        }
    }
}

impl std::fmt::Display for PosixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({}): {}", self.name(), self.code(), self.desc())
    }
}

/// POSIX 调用结果类型。
pub type PosixResult<T> = Result<T, PosixError>;

// ========================================================================
// POSIX 系统调用枚举
// ========================================================================

/// POSIX 系统调用编号与结构化参数。
///
/// 与 `kernel/syscall::AiSystemCall` 并行，不替换。每个变体对应一个
/// POSIX 接口；`handle` 主要读取枚举字段，`args` 保留给底层寄存器
/// 风格分派使用（两者可互补）。
#[derive(Clone, Debug)]
pub enum PosixSyscall {
    /// 打开文件：Open(path, flags, mode) -> fd
    Open { path: String, flags: i32, mode: u32 },
    /// 读取：Read(fd, buf_len) -> 读取字节数
    Read { fd: i32, buf_len: usize },
    /// 写入：Write(fd, buf_len) -> 写入字节数
    Write { fd: i32, buf_len: usize },
    /// 关闭：Close(fd) -> 0
    Close { fd: i32 },
    /// 取文件状态：Stat(path) -> 编码后的 stat 结果（占位）
    Stat { path: String },
    /// 取文件句柄状态：Fstat(fd) -> 编码后的 stat 结果（占位）
    Fstat { fd: i32 },
    /// 取符号链接本身状态：Lstat(path) -> 编码后的 stat 结果（占位）
    Lstat { path: String },
    /// 创建子进程：Fork() -> 子进程 pid
    Fork,
    /// 执行程序：Exec(path) -> 入口点（占位）
    Exec { path: String },
    /// 等待任一子进程：Wait(pid) -> 退出的子进程 pid
    Wait { pid: i32 },
    /// 等待指定子进程：Waitpid(pid, options) -> 退出的子进程 pid
    Waitpid { pid: i32, options: i32 },
    /// 退出进程：Exit(status)
    Exit { status: i32 },
    /// 发送信号：Kill(pid, sig) -> 0
    Kill { pid: i32, sig: i32 },
    /// 安装信号处理器（简化）：Signal(sig, handler_addr)
    Signal { sig: i32, handler: usize },
    /// 信号动作：Sigaction(sig, action_addr)
    Sigaction { sig: i32, action: usize },
    /// 信号掩码操作：Sigprocmask(how, mask)
    Sigprocmask { how: i32, mask: usize },
    /// 取得进程 ID：Getpid() -> pid
    Getpid,
    /// 取得父进程 ID：Getppid() -> ppid
    Getppid,
    /// 取得用户 ID：Getuid() -> uid
    Getuid,
    /// 取得组 ID：Getgid() -> gid
    Getgid,
    /// 设置用户 ID：Setuid(uid) -> 0
    Setuid { uid: u32 },
    /// 设置组 ID：Setgid(gid) -> 0
    Setgid { gid: u32 },
    /// 改变工作目录：Chdir(path) -> 0
    Chdir { path: String },
    /// 取得工作目录：Getcwd(buf_len) -> 路径长度（占位）
    Getcwd { buf_len: usize },
    /// 创建目录：Mkdir(path, mode) -> 0
    Mkdir { path: String, mode: u32 },
    /// 删除目录：Rmdir(path) -> 0
    Rmdir { path: String },
    /// 删除文件/链接：Unlink(path) -> 0
    Unlink { path: String },
    /// 创建硬链接：Link(old, new) -> 0
    Link { old: String, new: String },
    /// 创建符号链接：Symlink(target, link) -> 0
    Symlink { target: String, link: String },
    /// 读取符号链接：Readlink(path) -> 链接目标长度（占位）
    Readlink { path: String },
    /// 改变文件模式：Chmod(path, mode) -> 0
    Chmod { path: String, mode: u32 },
    /// 改变所有者：Chown(path, uid, gid) -> 0
    Chown { path: String, uid: u32, gid: u32 },
    /// 取/设置文件创建掩码：Umask(mask) -> 旧掩码
    Umask { mask: u32 },
    /// 校验访问权限：Access(path, mode) -> 0
    Access { path: String, mode: i32 },
    /// 复制文件描述符：Dup(fd) -> 新 fd
    Dup { fd: i32 },
    /// 复制并指定文件描述符：Dup2(oldfd, newfd) -> newfd
    Dup2 { oldfd: i32, newfd: i32 },
    /// 创建管道：Pipe() -> 读端 fd（写端紧随其后，占位）
    Pipe,
    /// 文件控制：Fcntl(fd, cmd, arg)
    Fcntl { fd: i32, cmd: i32, arg: usize },
    /// 设备控制：Ioctl(fd, request)
    Ioctl { fd: i32, request: usize },
    /// I/O 复用：Select(nfds, timeout) -> 就绪 fd 数（占位）
    Select { nfds: i32, timeout: usize },
    /// I/O 复用：Poll(nfds, timeout) -> 就绪 fd 数（占位）
    Poll { nfds: i32, timeout: usize },
    /// 取日历时间：Gettimeofday() -> 秒数（占位）
    Gettimeofday,
    /// 设置日历时间：Settimeofday(sec)
    Settimeofday { sec: i64 },
    /// 取进程 CPU 时间：Times() -> 时钟滴答数（占位）
    Times,
    /// 纳秒睡眠：Nanosleep(sec, nsec) -> 0
    Nanosleep { sec: i64, nsec: i64 },
    /// 取时钟时间：ClockGettime(clock_id) -> 纳秒（占位）
    ClockGettime { clock_id: i32 },
    /// 取系统名：Uname() -> 编码后结果（占位）
    Uname,
}

impl PosixSyscall {
    /// 返回系统调用的可读名称（供调试与审计）。
    pub fn name(&self) -> &'static str {
        match self {
            PosixSyscall::Open { .. } => "Open",
            PosixSyscall::Read { .. } => "Read",
            PosixSyscall::Write { .. } => "Write",
            PosixSyscall::Close { .. } => "Close",
            PosixSyscall::Stat { .. } => "Stat",
            PosixSyscall::Fstat { .. } => "Fstat",
            PosixSyscall::Lstat { .. } => "Lstat",
            PosixSyscall::Fork => "Fork",
            PosixSyscall::Exec { .. } => "Exec",
            PosixSyscall::Wait { .. } => "Wait",
            PosixSyscall::Waitpid { .. } => "Waitpid",
            PosixSyscall::Exit { .. } => "Exit",
            PosixSyscall::Kill { .. } => "Kill",
            PosixSyscall::Signal { .. } => "Signal",
            PosixSyscall::Sigaction { .. } => "Sigaction",
            PosixSyscall::Sigprocmask { .. } => "Sigprocmask",
            PosixSyscall::Getpid => "Getpid",
            PosixSyscall::Getppid => "Getppid",
            PosixSyscall::Getuid => "Getuid",
            PosixSyscall::Getgid => "Getgid",
            PosixSyscall::Setuid { .. } => "Setuid",
            PosixSyscall::Setgid { .. } => "Setgid",
            PosixSyscall::Chdir { .. } => "Chdir",
            PosixSyscall::Getcwd { .. } => "Getcwd",
            PosixSyscall::Mkdir { .. } => "Mkdir",
            PosixSyscall::Rmdir { .. } => "Rmdir",
            PosixSyscall::Unlink { .. } => "Unlink",
            PosixSyscall::Link { .. } => "Link",
            PosixSyscall::Symlink { .. } => "Symlink",
            PosixSyscall::Readlink { .. } => "Readlink",
            PosixSyscall::Chmod { .. } => "Chmod",
            PosixSyscall::Chown { .. } => "Chown",
            PosixSyscall::Umask { .. } => "Umask",
            PosixSyscall::Access { .. } => "Access",
            PosixSyscall::Dup { .. } => "Dup",
            PosixSyscall::Dup2 { .. } => "Dup2",
            PosixSyscall::Pipe => "Pipe",
            PosixSyscall::Fcntl { .. } => "Fcntl",
            PosixSyscall::Ioctl { .. } => "Ioctl",
            PosixSyscall::Select { .. } => "Select",
            PosixSyscall::Poll { .. } => "Poll",
            PosixSyscall::Gettimeofday => "Gettimeofday",
            PosixSyscall::Settimeofday { .. } => "Settimeofday",
            PosixSyscall::Times => "Times",
            PosixSyscall::Nanosleep { .. } => "Nanosleep",
            PosixSyscall::ClockGettime { .. } => "ClockGettime",
            PosixSyscall::Uname => "Uname",
        }
    }
}

impl std::fmt::Display for PosixSyscall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ========================================================================
// 内核态模拟对象：文件、描述符、进程、信号
// ========================================================================

/// 打开文件的方式（对应 POSIX open flags 的简化版）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenFlags {
    RDONLY = 0,
    WRONLY = 1,
    RDWR = 2,
    APPEND = 1 << 8,
    CREAT = 1 << 9,
    TRUNC = 1 << 10,
    EXCL = 1 << 11,
}

impl OpenFlags {
    pub fn from_bits(bits: i32) -> Self {
        if bits & Self::RDWR as i32 == Self::RDWR as i32 {
            OpenFlags::RDWR
        } else if bits & Self::WRONLY as i32 == Self::WRONLY as i32 {
            OpenFlags::WRONLY
        } else {
            OpenFlags::RDONLY
        }
    }
    pub fn can_read(self) -> bool {
        matches!(self, OpenFlags::RDONLY | OpenFlags::RDWR)
    }
    pub fn can_write(self) -> bool {
        matches!(self, OpenFlags::WRONLY | OpenFlags::RDWR)
    }
}

/// 文件元数据（模拟 inode 信息）。
#[derive(Clone, Debug)]
pub struct FileMeta {
    pub path: PathBuf,
    pub size: u64,
    pub mode: u32,           // 权限位 (0o755 等)
    pub uid: u32,
    pub gid: u32,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub link_target: Option<PathBuf>,
    pub nlink: u64,          // 硬链接数
}

/// 打开的文件描述符条目。
#[derive(Clone, Debug)]
pub struct FdEntry {
    pub fd: i32,
    pub path: PathBuf,
    pub flags: OpenFlags,
    pub offset: u64,
    pub readable: bool,
    pub writable: bool,
    /// 指向内存文件系统中文件内容的逻辑索引（由 FileTable 维护）。
    pub file_id: u64,
}

/// 进程信息（模拟 PCB 的关键字段）。
#[derive(Clone, Debug)]
pub struct ProcessInfo {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub gid: u32,
    pub cwd: PathBuf,
    pub signal_mask: u32,    // 位掩码，bit i 表示信号 i 是否被屏蔽
    pub state: ProcState,
    pub exit_code: Option<i32>,
}

/// 进程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcState {
    Running,
    Sleeping,
    Zombie,
    Dead,
}

/// 信号处理器登记（简化：信号号 -> 处理器地址；SIG_IGN/SIG_DFL 用 0/1 表示）。
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

// ========================================================================
// 全局状态：内存文件系统 + 文件描述符表 + 进程表
// ========================================================================

/// POSIX 层全部可变状态。
struct PosixState {
    /// 内存文件系统内容：路径 -> (元数据, 字节)。
    fs: BTreeMap<PathBuf, (FileMeta, Vec<u8>)>,
    /// 打开的文件描述符表：fd -> FdEntry。
    fds: BTreeMap<i32, FdEntry>,
    /// 下一个可分配 fd。
    next_fd: i32,
    /// 进程表。
    procs: BTreeMap<i32, ProcessInfo>,
    /// 下一个可分配 pid。
    next_pid: i32,
    /// 当前进程 pid（全局当前上下文，用于单进程原型）。
    current_pid: i32,
    /// 文件创建掩码。
    umask: u32,
    /// 信号处理器表：sig -> handler 地址。
    signal_handlers: BTreeMap<i32, usize>,
}

impl PosixState {
    fn new() -> Self {
        let root = PathBuf::from("/");
        let mut fs = BTreeMap::new();
        fs.insert(
            root.clone(),
            (
                FileMeta {
                    path: root,
                    size: 0,
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                    is_dir: true,
                    is_symlink: false,
                    link_target: None,
                    nlink: 1,
                },
                Vec::new(),
            ),
        );
        let mut procs = BTreeMap::new();
        let init_pid = 1;
        procs.insert(
            init_pid,
            ProcessInfo {
                pid: init_pid,
                ppid: 0,
                uid: 0,
                gid: 0,
                cwd: PathBuf::from("/"),
                signal_mask: 0,
                state: ProcState::Running,
                exit_code: None,
            },
        );
        Self {
            fs,
            fds: BTreeMap::new(),
            next_fd: 3, // 0,1,2 保留给 stdin/stdout/stderr
            procs,
            next_pid: init_pid + 1,
            current_pid: init_pid,
            umask: 0o022,
            signal_handlers: BTreeMap::new(),
        }
    }

    /// 取得（或惰性初始化）全局 POSIX 状态。
    fn state() -> &'static Mutex<PosixState> {
        static STATE: OnceLock<Mutex<PosixState>> = OnceLock::new();
        STATE.get_or_init(|| Mutex::new(PosixState::new()))
    }

    /// 以当前进程上下文执行一个只读操作。
    fn with<R>(f: impl FnOnce(&PosixState) -> R) -> R {
        let g = Self::state().lock().unwrap();
        f(&g)
    }

    /// 以当前进程上下文执行一个可变操作。
    fn with_mut<R>(f: impl FnOnce(&mut PosixState) -> R) -> R {
        let mut g = Self::state().lock().unwrap();
        f(&mut g)
    }
}

// ========================================================================
// 路径解析辅助
// ========================================================================

/// 规范化路径：处理 `.`、`..`、多余斜杠，返回绝对路径。
fn normalize_path(path: &str, cwd: &Path) -> PosixResult<PathBuf> {
    if path.is_empty() {
        return Err(PosixError::ENOENT);
    }
    if path.len() > 4096 {
        return Err(PosixError::ENAMETOOLONG);
    }
    let p = Path::new(path);
    let base = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for comp in base.iter() {
        match comp.to_str() {
            Some("") | Some(".") => {}
            Some("..") => {
                if !parts.is_empty() {
                    parts.pop();
                }
            }
            Some(_) => parts.push(comp),
            None => return Err(PosixError::EINVAL),
        }
    }
    let mut out = PathBuf::new();
    if base.is_absolute() {
        out.push("/");
    }
    for part in parts {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        out.push("/");
    }
    Ok(out)
}

/// 取得当前进程的工作目录。
fn current_cwd() -> PathBuf {
    PosixState::with(|s| s.procs.get(&s.current_pid).map(|p| p.cwd.clone()).unwrap_or_else(|| PathBuf::from("/")))
}

/// 取得当前进程 pid。
fn current_pid() -> i32 {
    PosixState::with(|s| s.current_pid)
}

/// 取得当前进程 uid/gid。
fn current_cred() -> (u32, u32) {
    PosixState::with(|s| {
        let p = s.procs.get(&s.current_pid).unwrap();
        (p.uid, p.gid)
    })
}

// ========================================================================
// PosixLayer：对外主入口
// ========================================================================

/// POSIX 兼容层主入口。
///
/// 提供 `handle(syscall, args)` 分派入口、`init()` / `install()` 初始化。
/// 所有调用都作用于内存中的模拟内核对象，不触碰真实宿主文件系统，
/// 因而可安全在用户态宏内核原型中运行。
pub struct PosixLayer;

impl PosixLayer {
    /// 初始化 POSIX 兼容层（注册根目录与 init 进程，保持静默）。
    pub fn init() {
        // PosixState::new() 在首次锁访问时惰性初始化；此处触发一次以确保就绪。
        let _ = PosixState::state();
    }

    /// 安装 POSIX 系统调用分派（与现有 AiSystemCall 并行，不替换）。
    ///
    /// 在 Hib OS 中，POSIX 调用通过 `sys://posix` 通道的 `act` 入口进入，
    /// 此处仅做标记与一次性初始化。
    pub fn install() {
        Self::init();
    }

    /// 分派一个 POSIX 系统调用，返回结果或 errno。
    ///
    /// `syscall` 承载结构化参数；`args` 为寄存器风格的原始参数数组，
    /// 供与底层裸系统调用入口对齐（本实现主要读取 `syscall` 字段）。
    pub fn handle(syscall: PosixSyscall, _args: &[usize]) -> PosixResult<usize> {
        match syscall {
            // ---------- 文件操作 ----------
            PosixSyscall::Open { path, flags, mode } => Self::sys_open(&path, flags, mode),
            PosixSyscall::Read { fd, buf_len } => Self::sys_read(fd, buf_len),
            PosixSyscall::Write { fd, buf_len } => Self::sys_write(fd, buf_len),
            PosixSyscall::Close { fd } => Self::sys_close(fd),
            PosixSyscall::Stat { path } => Self::sys_stat(&path),
            PosixSyscall::Fstat { fd } => Self::sys_fstat(fd),
            PosixSyscall::Lstat { path } => Self::sys_lstat(&path),
            // ---------- 进程控制 ----------
            PosixSyscall::Fork => Self::sys_fork(),
            PosixSyscall::Exec { path } => Self::sys_exec(&path),
            PosixSyscall::Wait { pid } => Self::sys_wait(pid),
            PosixSyscall::Waitpid { pid, options } => Self::sys_waitpid(pid, options),
            PosixSyscall::Exit { status } => {
                Self::sys_exit(status);
                Ok(0)
            }
            // ---------- 信号 ----------
            PosixSyscall::Kill { pid, sig } => Self::sys_kill(pid, sig),
            PosixSyscall::Signal { sig, handler } => Self::sys_signal(sig, handler),
            PosixSyscall::Sigaction { sig, action } => Self::sys_sigaction(sig, action),
            PosixSyscall::Sigprocmask { how, mask } => Self::sys_sigprocmask(how, mask),
            // ---------- 进程/用户标识 ----------
            PosixSyscall::Getpid => Ok(current_pid() as usize),
            PosixSyscall::Getppid => {
                PosixState::with(|s| {
                    Ok(s.procs.get(&s.current_pid).map(|p| p.ppid as usize).unwrap_or(0))
                })
            }
            PosixSyscall::Getuid => Ok(current_cred().0 as usize),
            PosixSyscall::Getgid => Ok(current_cred().1 as usize),
            PosixSyscall::Setuid { uid } => Self::sys_setuid(uid),
            PosixSyscall::Setgid { gid } => Self::sys_setgid(gid),
            // ---------- 路径与目录 ----------
            PosixSyscall::Chdir { path } => Self::sys_chdir(&path),
            PosixSyscall::Getcwd { buf_len: _ } => Self::sys_getcwd(),
            PosixSyscall::Mkdir { path, mode } => Self::sys_mkdir(&path, mode),
            PosixSyscall::Rmdir { path } => Self::sys_rmdir(&path),
            PosixSyscall::Unlink { path } => Self::sys_unlink(&path),
            PosixSyscall::Link { old, new } => Self::sys_link(&old, &new),
            PosixSyscall::Symlink { target, link } => Self::sys_symlink(&target, &link),
            PosixSyscall::Readlink { path } => Self::sys_readlink(&path),
            // ---------- 权限 ----------
            PosixSyscall::Chmod { path, mode } => Self::sys_chmod(&path, mode),
            PosixSyscall::Chown { path, uid, gid } => Self::sys_chown(&path, uid, gid),
            PosixSyscall::Umask { mask } => Self::sys_umask(mask),
            PosixSyscall::Access { path, mode } => Self::sys_access(&path, mode),
            // ---------- 描述符 ----------
            PosixSyscall::Dup { fd } => Self::sys_dup(fd),
            PosixSyscall::Dup2 { oldfd, newfd } => Self::sys_dup2(oldfd, newfd),
            PosixSyscall::Pipe => Self::sys_pipe(),
            // ---------- 文件控制 / 设备 ----------
            PosixSyscall::Fcntl { fd, cmd, arg } => Self::sys_fcntl(fd, cmd, arg),
            PosixSyscall::Ioctl { fd, request } => Self::sys_ioctl(fd, request),
            // ---------- I/O 复用 ----------
            PosixSyscall::Select { nfds, timeout } => Self::sys_select(nfds, timeout),
            PosixSyscall::Poll { nfds, timeout } => Self::sys_poll(nfds, timeout),
            // ---------- 时间 ----------
            PosixSyscall::Gettimeofday => Self::sys_gettimeofday(),
            PosixSyscall::Settimeofday { sec } => Self::sys_settimeofday(sec),
            PosixSyscall::Times => Self::sys_times(),
            PosixSyscall::Nanosleep { sec, nsec } => Self::sys_nanosleep(sec, nsec),
            PosixSyscall::ClockGettime { clock_id } => Self::sys_clock_gettime(clock_id),
            // ---------- 系统名 ----------
            PosixSyscall::Uname => Self::sys_uname(),
        }
    }

    // ------------------------------------------------------------------
    // 内部系统调用实现
    // ------------------------------------------------------------------

    fn sys_open(path: &str, flags: i32, mode: u32) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        let (uid, gid) = current_cred();
        let oflags = OpenFlags::from_bits(flags);

        PosixState::with_mut(|s| {
            // 若文件不存在且要求 CREAT，则创建。
            let exists = s.fs.contains_key(&np);
            if !exists {
                if oflags as i32 & OpenFlags::CREAT as i32 == 0 {
                    return Err(PosixError::ENOENT);
                }
                // 创建新文件。
                let meta = FileMeta {
                    path: np.clone(),
                    size: 0,
                    mode: (mode & 0o777 & !s.umask),
                    uid,
                    gid,
                    is_dir: false,
                    is_symlink: false,
                    link_target: None,
                    nlink: 1,
                };
                s.fs.insert(np.clone(), (meta, Vec::new()));
            } else {
                // 若同时指定 EXCL 且文件已存在 -> EEXIST（这里用 EINVAL 近似）。
                if oflags as i32 & OpenFlags::EXCL as i32 != 0 {
                    return Err(PosixError::EINVAL);
                }
                // 权限校验。
                let (meta, _) = s.fs.get(&np).unwrap();
                if !Self::may_access(meta, uid, gid, oflags) {
                    return Err(PosixError::EACCES);
                }
                if oflags as i32 & OpenFlags::TRUNC as i32 != 0 {
                    let (_, content) = s.fs.get_mut(&np).unwrap();
                    content.clear();
                    let m = s.fs.get_mut(&np).unwrap();
                    m.0.size = 0;
                }
            }
            // 分配文件描述符。
            let fd = s.next_fd;
            s.next_fd += 1;
            let file_id = hash_path(&np);
            let entry = FdEntry {
                fd,
                path: np.clone(),
                flags: oflags,
                offset: 0,
                readable: oflags.can_read(),
                writable: oflags.can_write(),
                file_id,
            };
            s.fds.insert(fd, entry);
            // 若指定 APPEND，偏移置到文件末尾。
            if oflags as i32 & OpenFlags::APPEND as i32 != 0 {
                if let Some(entry) = s.fds.get_mut(&fd) {
                    if let Some((meta, _)) = s.fs.get(&np) {
                        entry.offset = meta.size;
                    }
                }
            }
            Ok(fd as usize)
        })
    }

    /// 校验当前凭证对文件的访问权限。
    fn may_access(meta: &FileMeta, uid: u32, _gid: u32, flags: OpenFlags) -> bool {
        // root (uid 0) 全放行。
        if uid == 0 {
            return true;
        }
        let mode = meta.mode;
        let (owner_read, owner_write, other_read, other_write) = (
            (mode >> 6) & 1,
            (mode >> 5) & 1,
            mode & 1,
            (mode >> 1) & 1,
        );
        let readable = if uid == meta.uid { owner_read == 1 } else { other_read == 1 };
        let writable = if uid == meta.uid { owner_write == 1 } else { other_write == 1 };
        if flags.can_read() && !readable {
            return false;
        }
        if flags.can_write() && !writable {
            return false;
        }
        true
    }

    fn sys_read(fd: i32, buf_len: usize) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let entry = s.fds.get_mut(&fd).ok_or(PosixError::EBADF)?;
            if !entry.readable {
                return Err(PosixError::EACCES);
            }
            let (meta, content) = s.fs.get_mut(&entry.path).ok_or(PosixError::ENOENT)?;
            let start = entry.offset as usize;
            if start >= content.len() {
                entry.offset = content.len() as u64;
                return Ok(0); // EOF
            }
            let end = (start + buf_len).min(content.len());
            let n = end - start;
            entry.offset = end as u64;
            meta.size = content.len() as u64;
            Ok(n)
        })
    }

    fn sys_write(fd: i32, buf_len: usize) -> PosixResult<usize> {
        // 用户态原型中无实际缓冲区，按 buf_len 视为要写入的字节数（填充零字节模拟）。
        PosixState::with_mut(|s| {
            let entry = s.fds.get_mut(&fd).ok_or(PosixError::EBADF)?;
            if !entry.writable {
                return Err(PosixError::EACCES);
            }
            let (meta, content) = s.fs.get_mut(&entry.path).ok_or(PosixError::ENOENT)?;
            let pos = entry.offset as usize;
            let needed = pos + buf_len;
            if needed > content.len() {
                content.resize(needed, 0);
            }
            entry.offset = needed as u64;
            meta.size = content.len() as u64;
            Ok(buf_len)
        })
    }

    fn sys_close(fd: i32) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            if s.fds.remove(&fd).is_some() {
                Ok(0)
            } else {
                Err(PosixError::EBADF)
            }
        })
    }

    fn sys_stat(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with(|s| {
            let (meta, _) = s.fs.get(&np).ok_or(PosixError::ENOENT)?;
            Ok((((meta.mode as u64) << 32) | (meta.size as u64 & 0xFFFF_FFFF)) as usize)
        })
    }

    fn sys_fstat(fd: i32) -> PosixResult<usize> {
        PosixState::with(|s| {
            let entry = s.fds.get(&fd).ok_or(PosixError::EBADF)?;
            let (meta, _) = s.fs.get(&entry.path).ok_or(PosixError::ENOENT)?;
            Ok((((meta.mode as u64) << 32) | (meta.size as u64 & 0xFFFF_FFFF)) as usize)
        })
    }

    fn sys_lstat(path: &str) -> PosixResult<usize> {
        // 简化版：与 stat 相同（不跟随符号链接，本层不实现完整链接跟随）。
        Self::sys_stat(path)
    }

    /// 将 FileMeta 编码为一个 usize 占位（供 stat/fstat/lstat 返回）。
    fn encode_stat(meta: &FileMeta) -> u64 {
        ((meta.mode as u64) << 32) | (meta.size as u64 & 0xFFFF_FFFF)
    }

    // ---------- 进程控制 ----------

    fn sys_fork() -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let parent = s.current_pid;
            let pid = s.next_pid;
            s.next_pid += 1;
            let parent_info = s
                .procs
                .get(&parent)
                .ok_or(PosixError::ECHILD_())?;
            let child = ProcessInfo {
                pid,
                ppid: parent,
                uid: parent_info.uid,
                gid: parent_info.gid,
                cwd: parent_info.cwd.clone(),
                signal_mask: parent_info.signal_mask,
                state: ProcState::Running,
                exit_code: None,
            };
            s.procs.insert(pid, child);
            Ok(pid as usize)
        })
    }

    fn sys_exec(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with(|s| {
            if s.fs.contains_key(&np) {
                let (_, content) = s.fs.get(&np).unwrap();
                Ok((content.len() + hash_path(&np) as usize) % 0x10000)
            } else {
                Err(PosixError::ENOENT)
            }
        })
    }

    fn sys_wait(pid: i32) -> PosixResult<usize> {
        PosixState::with(|s| {
            if s.procs.contains_key(&pid) {
                Ok(pid as usize)
            } else {
                Err(PosixError::ECHILD_())
            }
        })
    }

    fn sys_waitpid(pid: i32, _options: i32) -> PosixResult<usize> {
        Self::sys_wait(pid)
    }

    fn sys_exit(status: i32) {
        PosixState::with_mut(|s| {
            if let Some(p) = s.procs.get_mut(&s.current_pid) {
                p.state = ProcState::Zombie;
                p.exit_code = Some(status);
            }
        });
        let _ = status;
    }

    // ---------- 信号 ----------

    fn sys_kill(pid: i32, sig: i32) -> PosixResult<usize> {
        if sig < 0 || sig > 64 {
            return Err(PosixError::EINVAL);
        }
        PosixState::with(|s| {
            if s.procs.contains_key(&pid) {
                Ok(0)
            } else {
                Err(PosixError::ESRCH)
            }
        })
    }

    fn sys_signal(sig: i32, handler: usize) -> PosixResult<usize> {
        if sig < 1 || sig > 64 {
            return Err(PosixError::EINVAL);
        }
        PosixState::with_mut(|s| {
            let old = s.signal_handlers.insert(sig, handler);
            Ok(old.unwrap_or(SIG_DFL))
        })
    }

    fn sys_sigaction(sig: i32, action: usize) -> PosixResult<usize> {
        Self::sys_signal(sig, action)
    }

    fn sys_sigprocmask(_how: i32, mask: usize) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let old = s.procs.get(&s.current_pid).map(|p| p.signal_mask).unwrap_or(0);
            if let Some(p) = s.procs.get_mut(&s.current_pid) {
                p.signal_mask = mask as u32;
            }
            Ok(old as usize)
        })
    }

    // ---------- 用户标识 ----------

    fn sys_setuid(uid: u32) -> PosixResult<usize> {
        let (cur_uid, _) = current_cred();
        if cur_uid != 0 && uid != cur_uid {
            return Err(PosixError::EPERM);
        }
        PosixState::with_mut(|s| {
            if let Some(p) = s.procs.get_mut(&s.current_pid) {
                p.uid = uid;
            }
            Ok(0)
        })
    }

    fn sys_setgid(gid: u32) -> PosixResult<usize> {
        let (_, cur_gid) = current_cred();
        if cur_gid != 0 && gid != cur_gid {
            return Err(PosixError::EPERM);
        }
        PosixState::with_mut(|s| {
            if let Some(p) = s.procs.get_mut(&s.current_pid) {
                p.gid = gid;
            }
            Ok(0)
        })
    }

    // ---------- 路径与目录 ----------

    fn sys_chdir(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with_mut(|s| {
            if let Some((meta, _)) = s.fs.get(&np) {
                if !meta.is_dir {
                    return Err(PosixError::ENOTDIR);
                }
            } else {
                return Err(PosixError::ENOENT);
            }
            if let Some(p) = s.procs.get_mut(&s.current_pid) {
                p.cwd = np;
            }
            Ok(0)
        })
    }

    fn sys_getcwd() -> PosixResult<usize> {
        let cwd = current_cwd();
        Ok(cwd.to_string_lossy().len() as usize)
    }

    fn sys_mkdir(path: &str, mode: u32) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        let (uid, gid) = current_cred();
        PosixState::with_mut(|s| {
            if s.fs.contains_key(&np) {
                return Err(PosixError::EEXIST_());
            }
            let meta = FileMeta {
                path: np.clone(),
                size: 0,
                mode: mode & 0o777 & !s.umask,
                uid,
                gid,
                is_dir: true,
                is_symlink: false,
                link_target: None,
                nlink: 2,
            };
            s.fs.insert(np, (meta, Vec::new()));
            Ok(0)
        })
    }

    fn sys_rmdir(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with_mut(|s| {
            if let Some((meta, _)) = s.fs.get(&np) {
                if !meta.is_dir {
                    return Err(PosixError::ENOTDIR);
                }
            } else {
                return Err(PosixError::ENOENT);
            }
            s.fs.remove(&np);
            Ok(0)
        })
    }

    fn sys_unlink(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with_mut(|s| {
            if let Some((meta, _)) = s.fs.get(&np) {
                if meta.is_dir {
                    return Err(PosixError::EISDIR);
                }
            } else {
                return Err(PosixError::ENOENT);
            }
            s.fs.remove(&np);
            Ok(0)
        })
    }

    fn sys_link(old: &str, new: &str) -> PosixResult<usize> {
        let oldp = normalize_path(old, &current_cwd())?;
        let newp = normalize_path(new, &current_cwd())?;
        PosixState::with_mut(|s| {
            if !s.fs.contains_key(&oldp) {
                return Err(PosixError::ENOENT);
            }
            if s.fs.contains_key(&newp) {
                return Err(PosixError::EEXIST_());
            }
            let (meta, content) = s.fs.get(&oldp).unwrap().clone();
            let mut meta = meta;
            meta.path = newp.clone();
            meta.nlink += 1;
            s.fs.insert(newp, (meta, content));
            Ok(0)
        })
    }

    fn sys_symlink(target: &str, link: &str) -> PosixResult<usize> {
        let linkp = normalize_path(link, &current_cwd())?;
        let (uid, gid) = current_cred();
        PosixState::with_mut(|s| {
            if s.fs.contains_key(&linkp) {
                return Err(PosixError::EEXIST_());
            }
            let meta = FileMeta {
                path: linkp.clone(),
                size: target.len() as u64,
                mode: 0o777 & !s.umask,
                uid,
                gid,
                is_dir: false,
                is_symlink: true,
                link_target: Some(PathBuf::from(target)),
                nlink: 1,
            };
            s.fs.insert(linkp, (meta, Vec::new()));
            Ok(0)
        })
    }

    fn sys_readlink(path: &str) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        PosixState::with(|s| {
            let (meta, _) = s.fs.get(&np).ok_or(PosixError::ENOENT)?;
            if let Some(t) = &meta.link_target {
                Ok(t.to_string_lossy().len() as usize)
            } else {
                Err(PosixError::EINVAL)
            }
        })
    }

    // ---------- 权限 ----------

    fn sys_chmod(path: &str, mode: u32) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        let (uid, _) = current_cred();
        PosixState::with_mut(|s| {
            let (meta, _) = s.fs.get_mut(&np).ok_or(PosixError::ENOENT)?;
            if uid != 0 && uid != meta.uid {
                return Err(PosixError::EPERM);
            }
            meta.mode = mode & 0o777;
            Ok(0)
        })
    }

    fn sys_chown(path: &str, uid: u32, gid: u32) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        let (cur_uid, _) = current_cred();
        if cur_uid != 0 {
            return Err(PosixError::EPERM);
        }
        PosixState::with_mut(|s| {
            let (meta, _) = s.fs.get_mut(&np).ok_or(PosixError::ENOENT)?;
            meta.uid = uid;
            meta.gid = gid;
            Ok(0)
        })
    }

    fn sys_umask(mask: u32) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let old = s.umask;
            s.umask = mask & 0o777;
            Ok(old as usize)
        })
    }

    fn sys_access(path: &str, mode: i32) -> PosixResult<usize> {
        let np = normalize_path(path, &current_cwd())?;
        let (uid, gid) = current_cred();
        PosixState::with(|s| {
            let (meta, _) = s.fs.get(&np).ok_or(PosixError::ENOENT)?;
            let flags = OpenFlags::from_bits(mode);
            if Self::may_access(meta, uid, gid, flags) {
                Ok(0)
            } else {
                Err(PosixError::EACCES)
            }
        })
    }

    // ---------- 描述符 ----------

    fn sys_dup(fd: i32) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let entry = s.fds.get(&fd).ok_or(PosixError::EBADF)?.clone();
            let newfd = s.next_fd;
            s.next_fd += 1;
            let mut entry = entry;
            entry.fd = newfd;
            s.fds.insert(newfd, entry);
            Ok(newfd as usize)
        })
    }

    fn sys_dup2(oldfd: i32, newfd: i32) -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            if !s.fds.contains_key(&oldfd) {
                return Err(PosixError::EBADF);
            }
            s.fds.remove(&newfd);
            let entry = s.fds.get(&oldfd).unwrap().clone();
            let mut entry = entry;
            entry.fd = newfd;
            s.fds.insert(newfd, entry);
            Ok(newfd as usize)
        })
    }

    fn sys_pipe() -> PosixResult<usize> {
        PosixState::with_mut(|s| {
            let rd = s.next_fd;
            s.next_fd += 1;
            let wr = s.next_fd;
            s.next_fd += 1;
            let pipe_id = hash_path(Path::new("pipe://"));
            let rd_entry = FdEntry {
                fd: rd,
                path: PathBuf::from("pipe://rd"),
                flags: OpenFlags::RDONLY,
                offset: 0,
                readable: true,
                writable: false,
                file_id: pipe_id,
            };
            let wr_entry = FdEntry {
                fd: wr,
                path: PathBuf::from("pipe://wr"),
                flags: OpenFlags::WRONLY,
                offset: 0,
                readable: false,
                writable: true,
                file_id: pipe_id,
            };
            let pipe_meta = FileMeta {
                path: PathBuf::from("pipe://rd"),
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                is_dir: false,
                is_symlink: false,
                link_target: None,
                nlink: 1,
            };
            s.fs.insert(PathBuf::from("pipe://rd"), (pipe_meta.clone(), Vec::new()));
            let pipe_meta_wr = FileMeta {
                path: PathBuf::from("pipe://wr"),
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                is_dir: false,
                is_symlink: false,
                link_target: None,
                nlink: 1,
            };
            s.fs.insert(PathBuf::from("pipe://wr"), (pipe_meta_wr, Vec::new()));
            s.fds.insert(rd, rd_entry);
            s.fds.insert(wr, wr_entry);
            Ok(rd as usize)
        })
    }

    // ---------- 文件控制 / 设备 ----------

    fn sys_fcntl(_fd: i32, cmd: i32, _arg: usize) -> PosixResult<usize> {
        // 简化：只支持 F_GETFL(3) 返回当前 flags，其余返回 EINVAL。
        if cmd == 3 {
            Ok(0) // 占位：返回 RDONLY
        } else {
            Err(PosixError::EINVAL)
        }
    }

    fn sys_ioctl(_fd: i32, _request: usize) -> PosixResult<usize> {
        Err(PosixError::EINVAL)
    }

    // ---------- I/O 复用 ----------

    fn sys_select(_nfds: i32, _timeout: usize) -> PosixResult<usize> {
        Ok(0) // 占位：无就绪 fd
    }

    fn sys_poll(_nfds: i32, _timeout: usize) -> PosixResult<usize> {
        Ok(0)
    }

    // ---------- 时间 ----------

    fn sys_gettimeofday() -> PosixResult<usize> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as usize)
    }

    fn sys_settimeofday(_sec: i64) -> PosixResult<usize> {
        Err(PosixError::EPERM)
    }

    fn sys_times() -> PosixResult<usize> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as usize)
    }

    fn sys_nanosleep(_sec: i64, _nsec: i64) -> PosixResult<usize> {
        Ok(0)
    }

    fn sys_clock_gettime(_clock_id: i32) -> PosixResult<usize> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as usize)
    }

    fn sys_uname() -> PosixResult<usize> {
        Ok("Hib OS".len() as usize)
    }
}

impl Default for PosixLayer {
    fn default() -> Self {
        PosixLayer
    }
}

// ========================================================================
// errno 扩展变体（私有，供 fork/wait 用，避免污染公共枚举）
// ========================================================================

impl PosixError {
    fn ECHILD_() -> PosixError {
        // ECHILD 未在公共枚举中列出，这里复用 EINVAL 近似；实际产品中应加入 ECHILD。
        PosixError::EINVAL
    }
    fn EEXIST_() -> PosixError {
        PosixError::EINVAL
    }
}

/// 路径的稳定哈希（用于生成 file_id）。
fn hash_path(path: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errno_codes_and_names() {
        assert_eq!(PosixError::ENOENT.code(), 2);
        assert_eq!(PosixError::EPERM.code(), 1);
        assert_eq!(PosixError::EACCES.name(), "EACCES");
        assert_eq!(PosixError::EACCES.desc(), "权限拒绝");
        assert_eq!(format!("{}", PosixError::ENOMEM), "ENOMEM(12): 内存不足");
    }

    #[test]
    fn syscall_name_display() {
        let s = PosixSyscall::Open {
            path: "/tmp/x".into(),
            flags: 0,
            mode: 0o644,
        };
        assert_eq!(s.name(), "Open");
        assert_eq!(format!("{}", PosixSyscall::Getpid), "Getpid");
        assert_eq!(format!("{}", PosixSyscall::Uname), "Uname");
    }

    #[test]
    fn open_read_write_close_roundtrip() {
        PosixLayer::init();
        let fd = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/testfile".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        )
        .unwrap();
        assert!(fd >= 3);
        let written = PosixLayer::handle(PosixSyscall::Write { fd: fd as i32, buf_len: 16 }, &[]).unwrap();
        assert_eq!(written, 16);
        let fd2 = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/testfile".into(),
                flags: OpenFlags::RDONLY as i32,
                mode: 0,
            },
            &[],
        )
        .unwrap();
        let read = PosixLayer::handle(PosixSyscall::Read { fd: fd2 as i32, buf_len: 64 }, &[]).unwrap();
        assert_eq!(read, 16);
        assert!(PosixLayer::handle(PosixSyscall::Close { fd: fd as i32 }, &[]).is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Close { fd: fd2 as i32 }, &[]).is_ok());
        // 关闭后再读应 EBADF。
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Read { fd: fd as i32, buf_len: 4 }, &[]).unwrap_err(),
            PosixError::EBADF
        );
    }

    #[test]
    fn open_nonexistent_without_creat_is_enoent() {
        PosixLayer::init();
        let err = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/no/such/file".into(),
                flags: OpenFlags::RDONLY as i32,
                mode: 0,
            },
            &[],
        )
        .unwrap_err();
        assert_eq!(err, PosixError::ENOENT);
    }

    #[test]
    fn chdir_and_getcwd() {
        PosixLayer::init();
        // 先创建目录。
        assert!(PosixLayer::handle(
            PosixSyscall::Mkdir {
                path: "/subdir".into(),
                mode: 0o755,
            },
            &[],
        )
        .is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Chdir { path: "/subdir".into() }, &[]).is_ok());
        let len = PosixLayer::handle(PosixSyscall::Getcwd { buf_len: 64 }, &[]).unwrap();
        assert_eq!(len, "/subdir".len());
        // 进入不存在的目录应失败。
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Chdir { path: "/nope".into() }, &[]).unwrap_err(),
            PosixError::ENOENT
        );
        // 将 cwd 改回根，避免影响其它测试。
        assert!(PosixLayer::handle(PosixSyscall::Chdir { path: "/".into() }, &[]).is_ok());
    }

    #[test]
    fn mkdir_rmdir_and_unlink() {
        PosixLayer::init();
        assert!(PosixLayer::handle(PosixSyscall::Mkdir { path: "/d1".into(), mode: 0o755 }, &[]).is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Mkdir { path: "/d1".into(), mode: 0o755 }, &[]).is_err()); // 重复
        assert!(PosixLayer::handle(PosixSyscall::Unlink { path: "/d1".into() }, &[]).is_err()); // 目录不可 unlink (EISDIR)
        assert!(PosixLayer::handle(PosixSyscall::Rmdir { path: "/d1".into() }, &[]).is_ok());
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Rmdir { path: "/d1".into() }, &[]).unwrap_err(),
            PosixError::ENOENT
        );
    }

    #[test]
    fn stat_fstat_lstat() {
        PosixLayer::init();
        PosixLayer::handle(
            PosixSyscall::Open {
                path: "/statfile".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        ).unwrap();
        let st = PosixLayer::handle(PosixSyscall::Stat { path: "/statfile".into() }, &[]).unwrap();
        assert!(st > 0);
        // fstat 需要先 open 得 fd。
        let fd = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/statfile".into(),
                flags: OpenFlags::RDONLY as i32,
                mode: 0,
            },
            &[],
        ).unwrap();
        let st2 = PosixLayer::handle(PosixSyscall::Fstat { fd: fd as i32 }, &[]).unwrap();
        assert_eq!(st, st2);
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: fd as i32 }, &[]);
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Stat { path: "/missing".into() }, &[]).unwrap_err(),
            PosixError::ENOENT
        );
        let _ = PosixLayer::handle(PosixSyscall::Lstat { path: "/statfile".into() }, &[]);
    }

    #[test]
    fn fork_exec_wait() {
        PosixLayer::init();
        let pid = PosixLayer::handle(PosixSyscall::Fork, &[]).unwrap() as i32;
        assert!(pid > 1);
        // 当前进程不变（fork 后当前上下文仍是父）。
        assert_eq!(PosixLayer::handle(PosixSyscall::Getpid, &[]).unwrap() as i32, 1);
        // exec 一个存在的文件。
        PosixLayer::handle(
            PosixSyscall::Open {
                path: "/bin/sh".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o755,
            },
            &[],
        ).unwrap();
        let entry = PosixLayer::handle(PosixSyscall::Exec { path: "/bin/sh".into() }, &[]);
        assert!(entry.is_ok());
        // wait 已存在的 pid。
        assert!(PosixLayer::handle(PosixSyscall::Wait { pid }, &[]).is_ok());
        // wait 不存在的 pid -> 失败。
        assert!(PosixLayer::handle(PosixSyscall::Wait { pid: 99999 }, &[]).is_err());
    }

    #[test]
    fn exit_changes_state() {
        PosixLayer::init();
        PosixLayer::handle(PosixSyscall::Exit { status: 42 }, &[]);
        // 退出后再次 Getpid 应仍能读（单进程原型不真正销毁）。
        assert!(PosixLayer::handle(PosixSyscall::Getpid, &[]).is_ok());
    }

    #[test]
    fn signal_sigaction_sigprocmask() {
        PosixLayer::init();
        let old = PosixLayer::handle(PosixSyscall::Signal { sig: 9, handler: 0x1000 }, &[]).unwrap();
        assert_eq!(old, SIG_DFL);
        let old2 = PosixLayer::handle(PosixSyscall::Sigaction { sig: 9, action: 0x2000 }, &[]).unwrap();
        assert_eq!(old2, 0x1000);
        let old_mask = PosixLayer::handle(PosixSyscall::Sigprocmask { how: 0, mask: 0b1010 }, &[]).unwrap();
        assert_eq!(old_mask, 0);
        // 非法信号。
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Signal { sig: 0, handler: 0 }, &[]).unwrap_err(),
            PosixError::EINVAL
        );
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Kill { pid: 99999, sig: 9 }, &[]).unwrap_err(),
            PosixError::ESRCH
        );
    }

    #[test]
    fn uid_gid_setuid_setgid() {
        PosixLayer::init();
        assert_eq!(PosixLayer::handle(PosixSyscall::Getuid, &[]).unwrap() as u32, 0);
        assert_eq!(PosixLayer::handle(PosixSyscall::Getgid, &[]).unwrap() as u32, 0);
        assert!(PosixLayer::handle(PosixSyscall::Setuid { uid: 1000 }, &[]).is_ok());
        assert_eq!(PosixLayer::handle(PosixSyscall::Getuid, &[]).unwrap() as u32, 1000);
        assert!(PosixLayer::handle(PosixSyscall::Setgid { gid: 2000 }, &[]).is_ok());
        assert_eq!(PosixLayer::handle(PosixSyscall::Getgid, &[]).unwrap() as u32, 2000);
    }

    #[test]
    fn chmod_chown_umask_access() {
        PosixLayer::init();
        PosixLayer::handle(
            PosixSyscall::Open {
                path: "/permfile".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        ).unwrap();
        assert!(PosixLayer::handle(PosixSyscall::Chmod { path: "/permfile".into(), mode: 0o755 }, &[]).is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Chown { path: "/permfile".into(), uid: 0, gid: 0 }, &[]).is_ok());
        let old = PosixLayer::handle(PosixSyscall::Umask { mask: 0o022 }, &[]).unwrap();
        assert_eq!(old, 0o022);
        assert!(PosixLayer::handle(PosixSyscall::Access { path: "/permfile".into(), mode: OpenFlags::RDONLY as i32 }, &[]).is_ok());
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Access { path: "/nofile".into(), mode: OpenFlags::RDONLY as i32 }, &[]).unwrap_err(),
            PosixError::ENOENT
        );
    }

    #[test]
    fn dup_dup2_pipe() {
        PosixLayer::init();
        let fd = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/dupf".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        ).unwrap() as i32;
        let dupd = PosixLayer::handle(PosixSyscall::Dup { fd }, &[]).unwrap() as i32;
        assert_ne!(dupd, fd);
        // 关闭原 fd，dup 的 fd 应仍可读。
        assert!(PosixLayer::handle(PosixSyscall::Close { fd }, &[]).is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Read { fd: dupd, buf_len: 4 }, &[]).is_ok());
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: dupd }, &[]);
        // dup2。
        let fd2 = PosixLayer::handle(
            PosixSyscall::Open {
                path: "/dupf2".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        ).unwrap() as i32;
        assert!(PosixLayer::handle(PosixSyscall::Dup2 { oldfd: fd2, newfd: 50 }, &[]).is_ok());
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: fd2 }, &[]);
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: 50 }, &[]);
        // pipe。
        let rd = PosixLayer::handle(PosixSyscall::Pipe, &[]).unwrap() as i32;
        assert!(rd >= 3);
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: rd }, &[]);
        // 关闭写端（rd+1）。
        let _ = PosixLayer::handle(PosixSyscall::Close { fd: rd + 1 }, &[]);
    }

    #[test]
    fn fcntl_ioctl_select_poll_time() {
        PosixLayer::init();
        // fcntl F_GETFL(3)。
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Fcntl { fd: 999, cmd: 3, arg: 0 }, &[]).unwrap_err(),
            PosixError::EBADF
        );
        assert_eq!(
            PosixLayer::handle(PosixSyscall::Ioctl { fd: 1, request: 0 }, &[]).unwrap_err(),
            PosixError::EINVAL
        );
        assert_eq!(PosixLayer::handle(PosixSyscall::Select { nfds: 0, timeout: 0 }, &[]).unwrap(), 0);
        assert_eq!(PosixLayer::handle(PosixSyscall::Poll { nfds: 0, timeout: 0 }, &[]).unwrap(), 0);
        let now = PosixLayer::handle(PosixSyscall::Gettimeofday, &[]).unwrap();
        assert!(now > 0);
        let ns = PosixLayer::handle(PosixSyscall::ClockGettime { clock_id: 0 }, &[]).unwrap();
        assert!(ns > 0);
        assert_eq!(PosixLayer::handle(PosixSyscall::Nanosleep { sec: 0, nsec: 0 }, &[]).unwrap(), 0);
        assert_eq!(PosixLayer::handle(PosixSyscall::Times, &[]).unwrap() > 0, true);
        assert_eq!(PosixLayer::handle(PosixSyscall::Uname, &[]).unwrap(), "Hib OS".len());
    }

    #[test]
    fn link_symlink_readlink() {
        PosixLayer::init();
        PosixLayer::handle(
            PosixSyscall::Open {
                path: "/linktarget".into(),
                flags: OpenFlags::CREAT as i32 | OpenFlags::WRONLY as i32,
                mode: 0o644,
            },
            &[],
        ).unwrap();
        assert!(PosixLayer::handle(PosixSyscall::Link { old: "/linktarget".into(), new: "/linkhard".into() }, &[]).is_ok());
        assert!(PosixLayer::handle(PosixSyscall::Symlink { target: "/linktarget".into(), link: "/linksoft".into() }, &[]).is_ok());
        let n = PosixLayer::handle(PosixSyscall::Readlink { path: "/linksoft".into() }, &[]).unwrap();
        assert_eq!(n, "/linktarget".len());
        // 重复 symlink 应失败（EEXIST 近似 EINVAL）。
        assert!(PosixLayer::handle(PosixSyscall::Symlink { target: "/linktarget".into(), link: "/linksoft".into() }, &[]).is_err());
    }
}

