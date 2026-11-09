//! 调试器 —— 进程级断点、单步、寄存器与内存访问。
//!
//! 为 Hib OS 用户态宏内核提供模拟的进程调试能力。所有状态（寄存器、
//! 内存、断点）均保存在内存中模拟的「目标地址空间」里，不干预真实宿主
//! 进程，因而可安全在用户态宏内核原型中运行。
//!
//! 与 `kernel/syscall`、`kernel/security` 等并行存在，不替换。
//!
//! 设计要点:
//! * `Debugger`：`new(target_pid)`、`attach(pid)`、`detach()`、`breakpoint(addr)`、
//!   `remove_breakpoint(addr)`、`single_step()`、`continue_exec()`、`read_regs()`、
//!   `write_reg(name, val)`、`read_mem(addr, buf)`、`write_mem(addr, data)`、
//!   `backtrace() -> Vec<Frame>`、`disassemble(addr, n) -> Vec<Instruction>`；
//! * `Regs`：通用寄存器（RAX/RBX/RCX/RDX/RSI/RDI/RBP/RSP/R8~R15/RIP/RFLAGS）、
//!   段寄存器与控制寄存器简化版；
//! * `Breakpoint`：地址、原始字节、启用状态、命中次数、条件；
//! * 断点采用 INT3 陷阱（0xCC）简化版，单步采用 TF 标志简化版；
//! * 回溯沿 RBP 链回溯栈帧；反汇编为基于操作码的简化伪汇编。
//!
//! 文件末尾附带 `#[cfg(test)]` 单元测试，覆盖附着/脱离、断点设置与命中、
//! 单步、寄存器读写、内存读写、回溯与反汇编等。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

// ========================================================================
// 错误
// ========================================================================

/// 调试器错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DebugError {
    /// 未附着到目标进程。
    NotAttached,
    /// 目标进程未在运行/单步状态。
    NotRunning,
    /// 目标进程已附着。
    AlreadyAttached { pid: u32 },
    /// 断点不存在。
    BreakpointNotFound { addr: usize },
    /// 断点已存在。
    BreakpointAlreadyExists { addr: usize },
    /// 内存访问越界或被拒绝。
    MemoryAccessDenied { addr: usize },
    /// 无效寄存器名。
    InvalidRegister { name: String },
    /// 无效地址。
    InvalidAddress { addr: usize },
    /// 已脱离。
    Detached { pid: u32 },
}

impl std::fmt::Display for DebugError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebugError::NotAttached => write!(f, "调试器未附着到目标进程"),
            DebugError::NotRunning => write!(f, "目标进程未在运行"),
            DebugError::AlreadyAttached { pid } => write!(f, "已附着到进程 {}", pid),
            DebugError::BreakpointNotFound { addr } => {
                write!(f, "断点不存在: 0x{:x}", addr)
            }
            DebugError::BreakpointAlreadyExists { addr } => {
                write!(f, "断点已存在: 0x{:x}", addr)
            }
            DebugError::MemoryAccessDenied { addr } => {
                write!(f, "内存访问被拒绝: 0x{:x}", addr)
            }
            DebugError::InvalidRegister { name } => write!(f, "无效寄存器: {}", name),
            DebugError::InvalidAddress { addr } => write!(f, "无效地址: 0x{:x}", addr),
            DebugError::Detached { pid } => write!(f, "已从进程 {} 脱离", pid),
        }
    }
}

pub type DebugResult<T> = Result<T, DebugError>;

// ========================================================================
// 寄存器
// ========================================================================

/// 通用与控制寄存器简化版（x86-64 风格）。
#[derive(Clone, Debug, Default)]
pub struct Regs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    /// 段寄存器。
    pub cs: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub fs: u16,
    pub gs: u16,
    /// 控制寄存器简化版。
    pub cr0: u64,
    pub cr3: u64,
}

impl Regs {
    /// 按名读取寄存器值。
    pub fn read(&self, name: &str) -> Option<u64> {
        match name {
            "rax" => Some(self.rax),
            "rbx" => Some(self.rbx),
            "rcx" => Some(self.rcx),
            "rdx" => Some(self.rdx),
            "rsi" => Some(self.rsi),
            "rdi" => Some(self.rdi),
            "rbp" => Some(self.rbp),
            "rsp" => Some(self.rsp),
            "r8" => Some(self.r8),
            "r9" => Some(self.r9),
            "r10" => Some(self.r10),
            "r11" => Some(self.r11),
            "r12" => Some(self.r12),
            "r13" => Some(self.r13),
            "r14" => Some(self.r14),
            "r15" => Some(self.r15),
            "rip" => Some(self.rip),
            "rflags" => Some(self.rflags),
            "cr0" => Some(self.cr0),
            "cr3" => Some(self.cr3),
            "cs" => Some(self.cs as u64),
            "ds" => Some(self.ds as u64),
            "es" => Some(self.es as u64),
            "ss" => Some(self.ss as u64),
            "fs" => Some(self.fs as u64),
            "gs" => Some(self.gs as u64),
            _ => None,
        }
    }

    /// 按名写入寄存器值。
    pub fn write(&mut self, name: &str, val: u64) -> bool {
        match name {
            "rax" => { self.rax = val; true }
            "rbx" => { self.rbx = val; true }
            "rcx" => { self.rcx = val; true }
            "rdx" => { self.rdx = val; true }
            "rsi" => { self.rsi = val; true }
            "rdi" => { self.rdi = val; true }
            "rbp" => { self.rbp = val; true }
            "rsp" => { self.rsp = val; true }
            "r8" => { self.r8 = val; true }
            "r9" => { self.r9 = val; true }
            "r10" => { self.r10 = val; true }
            "r11" => { self.r11 = val; true }
            "r12" => { self.r12 = val; true }
            "r13" => { self.r13 = val; true }
            "r14" => { self.r14 = val; true }
            "r15" => { self.r15 = val; true }
            "rip" => { self.rip = val; true }
            "rflags" => { self.rflags = val; true }
            "cr0" => { self.cr0 = val; true }
            "cr3" => { self.cr3 = val; true }
            "cs" => { self.cs = val as u16; true }
            "ds" => { self.ds = val as u16; true }
            "es" => { self.es = val as u16; true }
            "ss" => { self.ss = val as u16; true }
            "fs" => { self.fs = val as u16; true }
            "gs" => { self.gs = val as u16; true }
            _ => false,
        }
    }

    /// 寄存器名称列表。
    pub fn names() -> Vec<&'static str> {
        vec![
            "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9",
            "r10", "r11", "r12", "r13", "r14", "r15", "rip", "rflags", "cr0",
            "cr3", "cs", "ds", "es", "ss", "fs", "gs",
        ]
    }
}

// ========================================================================
// 断点
// ========================================================================

/// 断点。
#[derive(Clone, Debug)]
pub struct Breakpoint {
    /// 断点地址。
    pub addr: usize,
    /// 被 INT3 替换前的原始字节。
    pub original_bytes: Vec<u8>,
    /// 是否启用。
    pub enabled: bool,
    /// 命中次数。
    pub hit_count: u64,
    /// 可选条件表达式（字符串，简化版始终为真）。
    pub condition: Option<String>,
}

impl Breakpoint {
    pub fn new(addr: usize) -> Self {
        Self {
            addr,
            original_bytes: Vec::new(),
            enabled: true,
            hit_count: 0,
            condition: None,
        }
    }

    pub fn is_hit(&self) -> bool {
        self.enabled && self.hit_count > 0
    }
}

/// 单条（简化）反汇编指令。
#[derive(Clone, Debug)]
pub struct Instruction {
    pub address: usize,
    pub bytes: Vec<u8>,
    pub mnemonic: String,
    pub operands: String,
}

/// 栈帧（用于回溯）。
#[derive(Clone, Debug)]
pub struct Frame {
    pub ip: usize,
    pub bp: usize,
    pub sp: usize,
    /// 函数名与偏移（简化版：根据地址生成占位符）。
    pub function: String,
}

// ========================================================================
// 调试器
// ========================================================================

const INT3: u8 = 0xCC;

/// 调试器主入口。
///
/// 所有操作作用于内存模拟的目标地址空间，不修改宿主进程。
pub struct Debugger {
    pub target_pid: u32,
    attached: bool,
    running: bool,
    regs: Regs,
    /// 模拟目标地址空间：虚拟地址 -> 字节。
    memory: BTreeMap<usize, Vec<u8>>,
    breakpoints: BTreeMap<usize, Breakpoint>,
    single_step: bool,
    last_stop_reason: Option<String>,
}

impl Debugger {
    /// 创建一个针对 `target_pid` 的调试器（尚未附着）。
    pub fn new(target_pid: u32) -> Self {
        Debugger {
            target_pid,
            attached: false,
            running: false,
            regs: Regs::default(),
            memory: BTreeMap::new(),
            breakpoints: BTreeMap::new(),
            single_step: false,
            last_stop_reason: None,
        }
    }

    /// 附着到目标进程，并初始化寄存器与模拟地址空间。
    pub fn attach(&mut self, pid: u32) -> DebugResult<()> {
        if self.attached {
            return Err(DebugError::AlreadyAttached { pid: self.target_pid });
        }
        self.target_pid = pid;
        self.attached = true;
        self.running = false;
        self.single_step = false;
        self.last_stop_reason = None;
        // 初始化模拟寄存器。
        self.regs.rip = 0x1000;
        self.regs.rsp = 0x7fff_0000;
        self.regs.rbp = 0x7fff_0000;
        self.regs.rflags = 0x2;
        self.regs.cs = 0x33;
        self.regs.ss = 0x2b;
        self.regs.cr0 = 0x80000011;
        self.regs.cr3 = 0x1000;
        Ok(())
    }

    /// 从目标进程脱离，清除单步与断点命中状态（断点本身保留）。
    pub fn detach(&mut self) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        let pid = self.target_pid;
        self.attached = false;
        self.running = false;
        self.single_step = false;
        self.last_stop_reason = None;
        Ok(())
    }

    pub fn is_attached(&self) -> bool {
        self.attached
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// 在 `addr` 设置断点（将首字节替换为 INT3）。
    pub fn breakpoint(&mut self, addr: usize) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        if self.breakpoints.contains_key(&addr) {
            return Err(DebugError::BreakpointAlreadyExists { addr });
        }
        let original = self.read_byte(addr);
        let mut bp = Breakpoint::new(addr);
        bp.original_bytes.push(original);
        // 写入 INT3。
        self.write_byte(addr, INT3)?;
        self.breakpoints.insert(addr, bp);
        Ok(())
    }

    /// 移除 `addr` 处的断点，恢复原始字节。
    pub fn remove_breakpoint(&mut self, addr: usize) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        let bp = self
            .breakpoints
            .remove(&addr)
            .ok_or_else(|| DebugError::BreakpointNotFound { addr })?;
        if !bp.original_bytes.is_empty() {
            self.write_byte(addr, bp.original_bytes[0])?;
        }
        Ok(())
    }

    /// 启用/禁用断点。
    pub fn set_breakpoint_enabled(&mut self, addr: usize, enabled: bool) -> DebugResult<()> {
        let bp = self
            .breakpoints
            .get_mut(&addr)
            .ok_or_else(|| DebugError::BreakpointNotFound { addr })?;
        bp.enabled = enabled;
        Ok(())
    }

    /// 单步执行一条指令（设置 TF 标志，推进 RIP 一个指令长度）。
    pub fn single_step(&mut self) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        self.single_step = true;
        self.running = true;
        // 执行一条模拟指令。
        let _ = self.execute_instruction();
        self.single_step = false;
        self.running = false;
        self.last_stop_reason = Some("single step".to_string());
        Ok(())
    }

    /// 继续执行，直到遇到断点或单步结束。
    pub fn continue_exec(&mut self) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        self.running = true;
        const MAX_STEPS: usize = 10_000;
        for _ in 0..MAX_STEPS {
            // 检查当前 RIP 是否在启用的断点上。
            if let Some(bp) = self.breakpoints.get_mut(&(self.regs.rip as usize)) {
                if bp.enabled {
                    bp.hit_count += 1;
                    self.running = false;
                    self.last_stop_reason = Some(format!("breakpoint at 0x{:x}", bp.addr));
                    return Ok(());
                }
            }
            let stepped = self.execute_instruction();
            if self.single_step {
                self.single_step = false;
                self.running = false;
                self.last_stop_reason = Some("single step".to_string());
                return Ok(());
            }
            if !stepped {
                self.running = false;
                self.last_stop_reason = Some("no more instructions".to_string());
                return Ok(());
            }
        }
        self.running = false;
        self.last_stop_reason = Some("step limit reached".to_string());
        Ok(())
    }

    /// 执行一条模拟指令：按 RIP 读取操作码，推进 RIP，模拟对寄存器/内存的极简影响。
    fn execute_instruction(&mut self) -> bool {
        let rip = self.regs.rip as usize;
        let opcode = self.read_byte(rip);
        // 指令长度简化：0xCC=1, 0x90=1, 0x48 开头=3~7，其余=1。
        let len = match opcode {
            0xCC => 1,
            0x90 => 1,
            0x48 => 4,
            _ => 1,
        };
        let next_rip = rip.checked_add(len).and_then(|v| v.checked_shr(0)).unwrap_or(rip);
        // 极简语义模拟。
        match opcode {
            0x48 => {
                // 模拟 "mov rax, imm"：把接下来的 8 字节作为立即数放入 rax。
                if next_rip + 8 <= self.memory_range() {
                    let imm = self.read_u64(next_rip);
                    self.regs.rax = imm;
                }
            }
            0x90 => {} // nop
            0xCC => {} // int3 (breakpoint, handled outside)
            _ => {}
        }
        self.regs.rip = next_rip as u64;
        true
    }

    fn memory_range(&self) -> usize {
        self.memory
            .iter()
            .next_back()
            .map(|(addr, bytes)| addr + bytes.len())
            .unwrap_or(0)
    }

    /// 读取一个字节（未映射地址返回 0）。
    fn read_byte(&self, addr: usize) -> u8 {
        self.memory
            .get(&addr)
            .and_then(|b| b.first().copied())
            .unwrap_or(0)
    }

    /// 写入一个字节；若地址有断点且写入的不是 INT3，则保留 INT3。
    fn write_byte(&mut self, addr: usize, val: u8) -> DebugResult<()> {
        if self.breakpoints.contains_key(&addr) && val != INT3 {
            // 断点位置只允许写 INT3，其余写入被忽略以保护断点。
            return Ok(());
        }
        self.memory.entry(addr).or_default().push(val);
        // 保持每个地址只一个字节：这里简化为直接覆盖首字节。
        if let Some(buf) = self.memory.get_mut(&addr) {
            if !buf.is_empty() {
                buf[0] = val;
            }
        }
        Ok(())
    }

    /// 读取寄存器快照。
    pub fn read_regs(&self) -> Regs {
        self.regs.clone()
    }

    /// 按名写入寄存器。
    pub fn write_reg(&mut self, name: &str, val: u64) -> DebugResult<()> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        if self.regs.write(name, val) {
            Ok(())
        } else {
            Err(DebugError::InvalidRegister { name: name.to_string() })
        }
    }

    /// 读取内存：从 `addr` 开始向 `buf` 填充，返回实际读取字节数。
    pub fn read_mem(&self, addr: usize, buf: &mut [u8]) -> DebugResult<usize> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        let mut read = 0usize;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.read_byte(addr + i);
            read += 1;
        }
        Ok(read)
    }

    /// 写入内存：将 `data` 写入 `addr` 开始处，返回实际写入字节数。
    pub fn write_mem(&mut self, addr: usize, data: &[u8]) -> DebugResult<usize> {
        if !self.attached {
            return Err(DebugError::NotAttached);
        }
        // 存储为一块连续字节。
        let mut block: Vec<u8> = data.to_vec();
        self.memory.insert(addr, block);
        Ok(data.len())
    }

    /// 回溯：沿 RBP 链回溯栈帧，最多 64 帧。
    pub fn backtrace(&self) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut rip = self.regs.rip;
        let mut rbp = self.regs.rbp;
        let mut rsp = self.regs.rsp;
        for _ in 0..64 {
            if rbp == 0 {
                break;
            }
            frames.push(Frame {
                ip: rip as usize,
                bp: rbp as usize,
                sp: rsp as usize,
                function: format!("func+0x{:x}", rip),
            });
            // 从 [rbp] 读取上一帧 rbp，从 [rbp+8] 读取返回地址。
            let saved_rbp = self.read_u64(rbp as usize);
            let saved_rip = self.read_u64(rbp as usize + 8);
            if saved_rbp == 0 || saved_rbp == rbp {
                break;
            }
            rip = saved_rip;
            rbp = saved_rbp;
            rsp = rbp.wrapping_add(8);
        }
        frames
    }

    /// 从地址读取 u64（小端）。
    fn read_u64(&self, addr: usize) -> u64 {
        let mut bytes = [0u8; 8];
        for i in 0..8 {
            bytes[i] = self.read_byte(addr + i);
        }
        u64::from_le_bytes(bytes)
    }

    /// 反汇编 `n` 条指令（简化版伪汇编）。
    pub fn disassemble(&self, mut addr: usize, n: usize) -> Vec<Instruction> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let opcode = self.read_byte(addr);
            let (mnemonic, operands, len) = match opcode {
                0x00 => ("add".to_string(), "rax, rbx".to_string(), 2),
                0x48 => ("mov".to_string(), "rax, imm64".to_string(), 12),
                0x89 => ("mov".to_string(), "[rax], ebx".to_string(), 2),
                0x90 => ("nop".to_string(), "".to_string(), 1),
                0xCC => ("int3".to_string(), "".to_string(), 1),
                0xEB => ("jmp".to_string(), "rel8".to_string(), 2),
                0xE8 => ("call".to_string(), "rel32".to_string(), 5),
                0xC3 => ("ret".to_string(), "".to_string(), 1),
                0xFF => ("call".to_string(), "[rax]".to_string(), 2),
                _ => (
                    format!("db 0x{:02x}", opcode),
                    "".to_string(),
                    1,
                ),
            };
            let bytes: Vec<u8> = (0..len).map(|i| self.read_byte(addr + i)).collect();
            out.push(Instruction {
                address: addr,
                bytes,
                mnemonic,
                operands,
            });
            addr = addr.checked_add(len).unwrap_or(addr);
        }
        out
    }

    /// 取得上次停止原因。
    pub fn stop_reason(&self) -> Option<&str> {
        self.last_stop_reason.as_deref()
    }

    /// 取得全部断点。
    pub fn breakpoints(&self) -> Vec<&Breakpoint> {
        self.breakpoints.values().collect()
    }
}

impl Default for Debugger {
    fn default() -> Self {
        Debugger::new(0)
    }
}

// ========================================================================
// 全局单例访问（可选，供通道接入使用）
// ========================================================================

static DEBUGGER: OnceLock<Mutex<Debugger>> = OnceLock::new();

/// 取得全局调试器单例。
pub fn debugger() -> &'static Mutex<Debugger> {
    DEBUGGER.get_or_init(|| Mutex::new(Debugger::new(0)))
}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_detach_and_states() {
        let mut dbg = Debugger::new(999);
        assert!(!dbg.is_attached());
        assert!(dbg.attach(999).is_ok());
        assert!(dbg.is_attached());
        assert!(!dbg.is_running());
        assert!(dbg.attach(1000).is_err()); // 重复附着
        assert!(dbg.detach().is_ok());
        assert!(!dbg.is_attached());
        assert_eq!(dbg.detach().unwrap_err(), DebugError::NotAttached);
    }

    #[test]
    fn read_write_regs() {
        let mut dbg = Debugger::new(1);
        dbg.attach(1).unwrap();
        assert_eq!(dbg.read_regs().rip, 0x1000);
        assert!(dbg.write_reg("rax", 0x42).is_ok());
        assert_eq!(dbg.read_regs().rax, 0x42);
        assert!(dbg.write_reg("rip", 0x2000).is_ok());
        assert_eq!(dbg.read_regs().rip, 0x2000);
        assert_eq!(dbg.write_reg("nope", 1).unwrap_err(), DebugError::InvalidRegister { name: "nope".to_string() });
    }

    #[test]
    fn breakpoint_set_hit_remove() {
        let mut dbg = Debugger::new(2);
        dbg.attach(2).unwrap();
        // 在 0x1000 处设断点。
        assert!(dbg.breakpoint(0x1000).is_ok());
        assert!(dbg.breakpoint(0x1000).is_err()); // 重复
        // 断点命中：把 rip 设到断点，continue 应停。
        dbg.write_reg("rip", 0x1000).unwrap();
        assert!(dbg.continue_exec().is_ok());
        assert!(!dbg.is_running());
        assert_eq!(
            dbg.stop_reason(),
            Some("breakpoint at 0x1000")
        );
        assert_eq!(dbg.breakpoints()[0].hit_count, 1);
        // 移除断点。
        assert!(dbg.remove_breakpoint(0x1000).is_ok());
        assert!(dbg.remove_breakpoint(0x1000).is_err()); // 不存在
        assert!(dbg.breakpoints().is_empty());
    }

    #[test]
    fn single_step() {
        let mut dbg = Debugger::new(3);
        dbg.attach(3).unwrap();
        let rip_before = dbg.read_regs().rip;
        assert!(dbg.single_step().is_ok());
        assert!(!dbg.is_running());
        assert_eq!(dbg.stop_reason(), Some("single step"));
        // RIP 应推进。
        assert_ne!(dbg.read_regs().rip, rip_before);
    }

    #[test]
    fn read_write_mem() {
        let mut dbg = Debugger::new(4);
        dbg.attach(4).unwrap();
        let data = b"HibOS debug memory";
        assert_eq!(dbg.write_mem(0x10000, data).unwrap(), data.len());
        let mut buf = [0u8; 16];
        let n = dbg.read_mem(0x10000, &mut buf).unwrap();
        assert_eq!(n, 16);
        assert_eq!(&buf, data);
        // 读取未映射地址应为 0。
        let mut buf2 = [0u8; 4];
        dbg.read_mem(0x90000, &mut buf2).unwrap();
        assert_eq!(buf2, [0, 0, 0, 0]);
    }

    #[test]
    fn write_mem_protects_breakpoint() {
        let mut dbg = Debugger::new(5);
        dbg.attach(5).unwrap();
        dbg.breakpoint(0x2000).unwrap();
        // 在断点地址写非 INT3 字节应被忽略。
        assert!(dbg.write_mem(0x2000, &[0xAA]).is_ok());
        assert_eq!(dbg.read_byte_for_test(0x2000), INT3);
    }

    #[test]
    fn backtrace() {
        let mut dbg = Debugger::new(6);
        dbg.attach(6).unwrap();
        // 伪造栈：rbp=0x8000, [0x8000]=0x7000(上一帧 rbp), [0x8008]=0x4000(返回 ip)。
        dbg.write_mem(0x8000, &0x7000u64.to_le_bytes()).unwrap();
        dbg.write_mem(0x8008, &0x4000u64.to_le_bytes()).unwrap();
        dbg.write_reg("rbp", 0x8000).unwrap();
        dbg.write_reg("rip", 0x5000).unwrap();
        let frames = dbg.backtrace();
        assert!(!frames.is_empty());
        assert_eq!(frames[0].ip, 0x5000);
        assert_eq!(frames[0].bp, 0x8000);
    }

    #[test]
    fn disassemble() {
        let mut dbg = Debugger::new(7);
        dbg.attach(7).unwrap();
        // 写入几条已知操作码。
        dbg.write_mem(0x3000, &[0x90, 0xCC, 0xC3, 0xEB, 0x05]).unwrap();
        let insts = dbg.disassemble(0x3000, 5);
        assert_eq!(insts.len(), 5);
        assert_eq!(insts[0].mnemonic, "nop");
        assert_eq!(insts[1].mnemonic, "int3");
        assert_eq!(insts[2].mnemonic, "ret");
        assert_eq!(insts[3].mnemonic, "jmp");
        assert_eq!(insts[0].address, 0x3000);
    }

    #[test]
    fn not_attached_errors() {
        let mut dbg = Debugger::new(8);
        assert_eq!(dbg.read_regs().rip, 0); // 未附着时为零态
        // 附着后才能 read_mem/write_reg 等。
        assert_eq!(dbg.write_reg("rax", 1).unwrap_err(), DebugError::NotAttached);
        let mut buf = [0u8; 4];
        assert_eq!(dbg.read_mem(0, &mut buf).unwrap_err(), DebugError::NotAttached);
    }

    #[test]
    fn breakpoints_and_regs_names() {
        let names = Regs::names();
        assert!(names.contains(&"rip"));
        assert!(names.contains(&"rax"));
        assert!(names.contains(&"rflags"));
        assert!(!names.is_empty());
    }
}

/// 读取单字节（测试专用，绕过私有方法）。
impl Debugger {
    fn read_byte_for_test(&self, addr: usize) -> u8 {
        self.read_byte(addr)
    }
}
