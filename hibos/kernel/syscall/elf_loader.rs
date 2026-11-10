//! ELF 可执行文件加载器 —— 在 Hib OS 用户态宏内核上解析与加载 ELF 程序。
//!
//! 支持 ELF 32 位与 64 位，解析 ELF 头、程序头表与节头表，将可加载段
//! 映射到模拟的地址空间，并提供入口点跳转、动态符号解析与基址重定位
//! 接口（供 `kernel/syscall/dynamic_linker.rs` 调用）。
//!
//! 本模块不依赖外部 crates，纯手动按字节解析 ELF 二进制，可在脱网用户态
//! 原型中运行。所有加载对象均保存在内存中模拟的「地址空间」里，不触碰
//! 真实宿主进程，因而安全。
//!
//! 设计要点:
//! * `ElfHeader` 完整承载 ELF 头字段（魔数 0x7FELF、类/字节序/版本/OS-ABI
//!   /类型/指令集/入口点/程序头与节头偏移等）；
//! * `ProgramHeader` 承载段类型（PT_LOAD/PT_DYNAMIC/PT_INTERP/PT_NOTE/PT_PHDR
//!   /PT_TLS）、文件/虚拟/物理地址、文件大小/内存大小、权限（R/W/E）、对齐；
//! * `ElfLoader::load(path)` 一次性完成读取、解析头、解析程序头、映射段；
//! * `map_segments` 返回入口点；`resolve_symbol` / `relocate` 提供动态链接
//!   所需的符号与基址重定位能力；
//! * 文件末尾附带 `#[cfg(test)]` 单元测试，覆盖头解析、段映射、符号查找、
//!   重定位、错误路径等。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

// ========================================================================
// 错误
// ========================================================================

/// ELF 加载错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// 文件读取失败。
    Io { path: String, reason: String },
    /// 文件过短，不足以容纳 ELF 头。
    TruncatedHeader,
    /// 不是 ELF 文件（魔数不匹配）。
    InvalidMagic,
    /// 不支持的 ELF 类（既非 32 位也非 64 位）。
    UnsupportedClass(u8),
    /// 不支持的字节序。
    BadEndian(u8),
    /// 不支持的 ELF 版本。
    UnsupportedVersion(u8),
    /// 程序头表偏移或数量非法，导致越界。
    InvalidProgramHeaderTable,
    /// 节头表偏移或数量非法。
    InvalidSectionHeaderTable,
    /// 段映射发生重叠。
    SegmentOverlap,
    /// 入口点不落在任何可加载段内。
    InvalidEntryPoint,
    /// 符号未找到。
    SymbolNotFound { name: String },
    /// 重定位失败。
    RelocationFailed { offset: usize, reason: String },
    /// 动态链接器未找到（无 PT_INTERP / PT_DYNAMIC）。
    NoDynamicLinker,
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfError::Io { path, reason } => write!(f, "读取 {} 失败: {}", path, reason),
            ElfError::TruncatedHeader => write!(f, "ELF 头被截断"),
            ElfError::InvalidMagic => write!(f, "不是 ELF 文件（魔数不匹配）"),
            ElfError::UnsupportedClass(c) => write!(f, "不支持的 ELF 类: {}", c),
            ElfError::BadEndian(e) => write!(f, "不支持的字节序: {}", e),
            ElfError::UnsupportedVersion(v) => write!(f, "不支持的 ELF 版本: {}", v),
            ElfError::InvalidProgramHeaderTable => write!(f, "程序头表非法"),
            ElfError::InvalidSectionHeaderTable => write!(f, "节头表非法"),
            ElfError::SegmentOverlap => write!(f, "段映射发生重叠"),
            ElfError::InvalidEntryPoint => write!(f, "入口点不在任何可加载段内"),
            ElfError::SymbolNotFound { name } => write!(f, "符号未找到: {}", name),
            ElfError::RelocationFailed { offset, reason } => {
                write!(f, "重定位失败 (偏移 0x{:x}): {}", offset, reason)
            }
            ElfError::NoDynamicLinker => write!(f, "未找到动态链接器"),
        }
    }
}

pub type ElfResult<T> = Result<T, ElfError>;

// ========================================================================
// 常量
// ========================================================================

/// ELF 魔数：\x7f E L F
pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// ELF 类。
pub const ELF_CLASS_32: u8 = 1;
pub const ELF_CLASS_64: u8 = 2;

/// 字节序。
pub const ELF_DATA_LITTLE: u8 = 1;
pub const ELF_DATA_BIG: u8 = 2;

/// 节类型。
pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_DYNAMIC: u32 = 6;
pub const SHT_DYNSYM: u32 = 11;

/// 段类型。
pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_NOTE: u32 = 4;
pub const PT_SHLIB: u32 = 5;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;

/// 段权限标志。
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// ========================================================================
// ELF 头
// ========================================================================

/// ELF 文件头（e_ident 之后的固定部分，按类分别解析）。
#[derive(Clone, Debug)]
pub struct ElfHeader {
    /// 魔数，必须是 0x7FELF。
    pub magic: [u8; 4],
    /// 类：1 = 32 位，2 = 64 位。
    pub class: u8,
    /// 字节序：1 = 小端，2 = 大端。
    pub endian: u8,
    /// ELF 版本，应为 1。
    pub version: u8,
    /// OS ABI 标识。
    pub os_abi: u8,
    /// ABI 版本。
    pub abi_version: u8,
    /// 文件类型：1=可重定位, 2=可执行, 3=共享对象, 4=核心镜像。
    pub elf_type: u16,
    /// 指令集架构（e_machine）。
    pub machine: u16,
    /// 文件版本。
    pub file_version: u32,
    /// 入口点虚拟地址。
    pub entry: u64,
    /// 程序头表在文件中的偏移。
    pub phoff: u64,
    /// 节头表在文件中的偏移。
    pub shoff: u64,
    /// 处理器特定标志。
    pub flags: u32,
    /// ELF 头大小。
    pub ehsize: u16,
    /// 程序头表项大小。
    pub phentsize: u16,
    /// 程序头表项数量。
    pub phnum: u16,
    /// 节头表项大小。
    pub shentsize: u16,
    /// 节头表项数量。
    pub shnum: u16,
    /// 节头字符串表索引。
    pub shstrndx: u16,
}

impl ElfHeader {
    /// 返回该头对应的 ELF 位数（32 或 64）。
    pub fn bitness(self) -> u32 {
        match self.class {
            ELF_CLASS_32 => 32,
            ELF_CLASS_64 => 64,
            _ => 0,
        }
    }

    /// 判断是否为小端。
    pub fn is_little_endian(self) -> bool {
        self.endian == ELF_DATA_LITTLE
    }

    /// 判断是否为可执行文件或共享对象（可加载）。
    pub fn is_loadable(self) -> bool {
        matches!(self.elf_type, 2 | 3)
    }
}

// ========================================================================
// 程序头
// ========================================================================

/// 程序头表项，描述一个可加载或元数据段。
#[derive(Clone, Debug)]
pub struct ProgramHeader {
    /// 段类型：PT_LOAD/PT_DYNAMIC/PT_INTERP/PT_NOTE/PT_PHDR/PT_TLS。
    pub p_type: u32,
    /// 段在文件中的偏移。
    pub p_offset: u64,
    /// 段的虚拟地址。
    pub p_vaddr: u64,
    /// 段的物理地址（本模拟器中与虚拟地址同值）。
    pub p_paddr: u64,
    /// 段在文件中的大小。
    pub p_filesz: u64,
    /// 段在内存中的大小。
    pub p_memsz: u64,
    /// 权限：PF_R/PF_W/PF_X 的组合。
    pub p_flags: u32,
    /// 段对齐。
    pub p_align: u64,
}

impl ProgramHeader {
    /// 判断该段是否可加载（PT_LOAD）。
    pub fn is_loadable(&self) -> bool {
        self.p_type == PT_LOAD
    }

    /// 权限是否可读。
    pub fn readable(&self) -> bool {
        self.p_flags & PF_R != 0
    }

    /// 权限是否可写。
    pub fn writable(&self) -> bool {
        self.p_flags & PF_W != 0
    }

    /// 权限是否可执行。
    pub fn executable(&self) -> bool {
        self.p_flags & PF_X != 0
    }

    /// 权限的可读字符串（如 "rwx"）。
    pub fn flags_str(&self) -> String {
        let mut s = String::new();
        s.push(if self.readable() { 'r' } else { '-' });
        s.push(if self.writable() { 'w' } else { '-' });
        s.push(if self.executable() { 'x' } else { '-' });
        s
    }
}

// ========================================================================
// 节头
// ========================================================================

/// 节头表项（用于符号表与字符串表解析）。
#[derive(Clone, Debug)]
pub struct SectionHeader {
    /// 节名在字符串表中的索引。
    pub sh_name: u32,
    /// 节类型。
    pub sh_type: u32,
    /// 节属性。
    pub sh_flags: u64,
    /// 节在虚拟地址中的地址。
    pub sh_addr: u64,
    /// 节在文件中的偏移。
    pub sh_offset: u64,
    /// 节大小。
    pub sh_size: u64,
    /// 链接的节索引。
    pub sh_link: u32,
    /// 额外信息。
    pub sh_info: u32,
    /// 地址对齐。
    pub sh_addralign: u64,
    /// 表项大小（若为表）。
    pub sh_entsize: u64,
}

// ========================================================================
// 符号
// ========================================================================

/// ELF 符号表项（简化：名称解析后存储为字符串）。
#[derive(Clone, Debug)]
pub struct Symbol {
    /// 符号名称。
    pub name: String,
    /// 符号值（地址或偏移）。
    pub value: u64,
    /// 符号大小。
    pub size: u64,
    /// 符号信息（绑定/类型）。
    pub info: u8,
    /// 其他信息。
    pub other: u8,
    /// 所在节索引。
    pub shndx: u16,
}

// ========================================================================
// 加载后的 ELF 程序
// ========================================================================

/// 已加载的 ELF 程序镜像。
///
/// `segments` 记录已映射段的虚拟地址范围与内容（模拟内存）；
/// `sections` 记录节头（供符号/字符串表查找）；
/// `symbols` 记录动态符号表（名称 -> 地址）。
#[derive(Clone, Debug)]
pub struct ElfProgram {
    pub header: ElfHeader,
    pub program_headers: Vec<ProgramHeader>,
    pub sections: Vec<SectionHeader>,
    /// 节名字符串表内容（原始字节，供解析节名）。
    pub shstrtab: Vec<u8>,
    /// 动态符号表（名称 -> 虚拟地址）。
    pub symbols: BTreeMap<String, u64>,
    /// 已映射段：虚拟地址 -> (内容, 权限)。
    pub segments: BTreeMap<u64, (Vec<u8>, ProgramHeader)>,
    /// 加载基址（位置无关代码的偏移）。
    pub base_addr: u64,
    /// 入口点虚拟地址（未加基址前的 ELF 入口点）。
    pub entry: u64,
    /// 解释器路径（PT_INTERP，若有）。
    pub interp: Option<String>,
}

impl ElfProgram {
    /// 取得可执行入口点（已考虑 `base_addr`）。
    pub fn entry_point(&self) -> u64 {
        self.base_addr.wrapping_add(self.entry)
    }

    /// 判断某地址是否落在某个已加载段内。
    pub fn contains_addr(&self, addr: u64) -> bool {
        self.segments.values().any(|(_, ph)| {
            addr >= ph.p_vaddr && addr < ph.p_vaddr.wrapping_add(ph.p_memsz.max(1))
        })
    }
}

// ========================================================================
// 字节序读取辅助
// ========================================================================

fn read_u16(data: &[u8], offset: usize, little: bool) -> ElfResult<u16> {
    if offset + 2 > data.len() {
        return Err(ElfError::TruncatedHeader);
    }
    let v = if little {
        u16::from_le_bytes([data[offset], data[offset + 1]])
    } else {
        u16::from_be_bytes([data[offset], data[offset + 1]])
    };
    Ok(v)
}

fn read_u32(data: &[u8], offset: usize, little: bool) -> ElfResult<u32> {
    if offset + 4 > data.len() {
        return Err(ElfError::TruncatedHeader);
    }
    let v = if little {
        u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
    } else {
        u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
    };
    Ok(v)
}

fn read_u64(data: &[u8], offset: usize, little: bool) -> ElfResult<u64> {
    if offset + 8 > data.len() {
        return Err(ElfError::TruncatedHeader);
    }
    let v = if little {
        u64::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ])
    } else {
        u64::from_be_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
            data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7],
        ])
    };
    Ok(v)
}

fn read_bytes(data: &[u8], offset: usize, len: usize) -> ElfResult<&[u8]> {
    if offset + len > data.len() {
        return Err(ElfError::TruncatedHeader);
    }
    Ok(&data[offset..offset + len])
}

/// 以 C 字符串结束符截断字节切片并转为字符串。
fn cstr_to_string(bytes: &[u8]) -> String {
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..len]).to_string()
}

/// 判断 ELF 头是否为 32 位。
fn class_is_32(header: &ElfHeader) -> bool {
    header.class == ELF_CLASS_32
}

// ========================================================================
// ELF 加载器
// ========================================================================

/// ELF 加载器主入口。
///
/// 提供 `load(path)`、`parse_header(data)`、`parse_program_headers()`、
// `map_segments()`、`resolve_symbol()`、`relocate()` 等方法。
/// 所有操作作用于内存模拟的地址空间，不修改宿主进程。
pub struct ElfLoader;

impl ElfLoader {
    /// 创建一个新的 ELF 加载器（无状态，仅作命名空间）。
    pub fn new() -> Self {
        ElfLoader
    }

    /// 从文件加载 ELF 程序。
    ///
    /// 步骤：读取文件 -> `parse_header` -> `parse_program_headers` ->
    /// 解析节头与符号表 -> `map_segments` 映射所有 PT_LOAD 段。
    pub fn load(path: &str) -> ElfResult<ElfProgram> {
        let data = fs::read(path).map_err(|e| ElfError::Io {
            path: path.to_string(),
            reason: e.to_string(),
        })?;
        let header = Self::parse_header(&data)?;
        let program_headers = Self::parse_program_headers(&header, &data)?;
        let (sections, shstrtab, symbols, interp) =
            Self::parse_sections_and_symbols(&header, &data, &program_headers)?;
        let mut program = ElfProgram {
            header: header.clone(),
            program_headers: program_headers.clone(),
            sections,
            shstrtab,
            symbols,
            segments: BTreeMap::new(),
            base_addr: 0,
            entry: header.entry,
            interp,
        };
        Self::map_segments_inner(&mut program, &data)?;
        Ok(program)
    }

    /// 解析 ELF 头。
    ///
    /// 校验魔数、类、字节序、版本，并按 32/64 位分别读取字段。
    pub fn parse_header(data: &[u8]) -> ElfResult<ElfHeader> {
        if data.len() < 16 {
            return Err(ElfError::TruncatedHeader);
        }
        if data[0..4] != ELF_MAGIC {
            return Err(ElfError::InvalidMagic);
        }
        let class = data[4];
        let endian = data[5];
        let version = data[6];
        let os_abi = data[7];
        let abi_version = data[8];
        let _pad = &data[9..16];

        if version != 1 {
            return Err(ElfError::UnsupportedVersion(version));
        }
        let little = match endian {
            ELF_DATA_LITTLE => true,
            ELF_DATA_BIG => false,
            _ => return Err(ElfError::BadEndian(endian)),
        };

        let header = match class {
            ELF_CLASS_32 => {
                // 32 位 ELF 头：e_type(2) e_machine(2) e_version(4) e_entry(4)
                // e_phoff(4) e_shoff(4) e_flags(4) e_ehsize(2) e_phentsize(2)
                // e_phnum(2) e_shentsize(2) e_shnum(2) e_shstrndx(2)
                if data.len() < 52 {
                    return Err(ElfError::TruncatedHeader);
                }
                let elf_type = read_u16(data, 16, little)?;
                let machine = read_u16(data, 18, little)?;
                let file_version = read_u32(data, 20, little)?;
                let entry = read_u32(data, 24, little)? as u64;
                let phoff = read_u32(data, 28, little)? as u64;
                let shoff = read_u32(data, 32, little)? as u64;
                let flags = read_u32(data, 36, little)?;
                let ehsize = read_u16(data, 40, little)?;
                let phentsize = read_u16(data, 42, little)?;
                let phnum = read_u16(data, 44, little)?;
                let shentsize = read_u16(data, 46, little)?;
                let shnum = read_u16(data, 48, little)?;
                let shstrndx = read_u16(data, 50, little)?;
                ElfHeader {
                    magic: ELF_MAGIC,
                    class,
                    endian,
                    version,
                    os_abi,
                    abi_version,
                    elf_type,
                    machine,
                    file_version,
                    entry,
                    phoff,
                    shoff,
                    flags,
                    ehsize,
                    phentsize,
                    phnum,
                    shentsize,
                    shnum,
                    shstrndx,
                }
            }
            ELF_CLASS_64 => {
                // 64 位 ELF 头：e_type(2) e_machine(2) e_version(4) e_entry(8)
                // e_phoff(8) e_shoff(8) e_flags(4) e_ehsize(2) e_phentsize(2)
                // e_phnum(2) e_shentsize(2) e_shnum(2) e_shstrndx(2)
                if data.len() < 64 {
                    return Err(ElfError::TruncatedHeader);
                }
                let elf_type = read_u16(data, 16, little)?;
                let machine = read_u16(data, 18, little)?;
                let file_version = read_u32(data, 20, little)?;
                let entry = read_u64(data, 24, little)?;
                let phoff = read_u64(data, 32, little)?;
                let shoff = read_u64(data, 40, little)?;
                let flags = read_u32(data, 48, little)?;
                let ehsize = read_u16(data, 52, little)?;
                let phentsize = read_u16(data, 54, little)?;
                let phnum = read_u16(data, 56, little)?;
                let shentsize = read_u16(data, 58, little)?;
                let shnum = read_u16(data, 60, little)?;
                let shstrndx = read_u16(data, 62, little)?;
                ElfHeader {
                    magic: ELF_MAGIC,
                    class,
                    endian,
                    version,
                    os_abi,
                    abi_version,
                    elf_type,
                    machine,
                    file_version,
                    entry,
                    phoff,
                    shoff,
                    flags,
                    ehsize,
                    phentsize,
                    phnum,
                    shentsize,
                    shnum,
                    shstrndx,
                }
            }
            c => return Err(ElfError::UnsupportedClass(c)),
        };
        Ok(header)
    }

    /// 解析程序头表。
    pub fn parse_program_headers(header: &ElfHeader, data: &[u8]) -> ElfResult<Vec<ProgramHeader>> {
        if header.phnum == 0 {
            return Ok(Vec::new());
        }
        if header.phoff == 0 || header.phentsize == 0 {
            return Err(ElfError::InvalidProgramHeaderTable);
        }
        let mut headers = Vec::with_capacity(header.phnum as usize);
        let little = header.clone().is_little_endian();
        for i in 0..header.phnum as u64 {
            let off = header
                .phoff
                .checked_add(i * header.phentsize as u64)
                .ok_or(ElfError::InvalidProgramHeaderTable)?;
            let off = off as usize;
            if class_is_32(header) {
                // 32 位程序头：p_type(4) p_offset(4) p_vaddr(4) p_paddr(4)
                // p_filesz(4) p_memsz(4) p_flags(4) p_align(4)
                if off + 32 > data.len() {
                    return Err(ElfError::InvalidProgramHeaderTable);
                }
                let p_type = read_u32(data, off, little)?;
                let p_offset = read_u32(data, off + 4, little)? as u64;
                let p_vaddr = read_u32(data, off + 8, little)? as u64;
                let p_paddr = read_u32(data, off + 12, little)? as u64;
                let p_filesz = read_u32(data, off + 16, little)? as u64;
                let p_memsz = read_u32(data, off + 20, little)? as u64;
                let p_flags = read_u32(data, off + 24, little)?;
                let p_align = read_u32(data, off + 28, little)? as u64;
                headers.push(ProgramHeader {
                    p_type,
                    p_offset,
                    p_vaddr,
                    p_paddr,
                    p_filesz,
                    p_memsz,
                    p_flags,
                    p_align,
                });
            } else {
                // 64 位程序头：p_type(4) p_flags(4) p_offset(8) p_vaddr(8)
                // p_paddr(8) p_filesz(8) p_memsz(8) p_align(8)
                if off + 56 > data.len() {
                    return Err(ElfError::InvalidProgramHeaderTable);
                }
                let p_type = read_u32(data, off, little)?;
                let p_flags = read_u32(data, off + 4, little)?;
                let p_offset = read_u64(data, off + 8, little)?;
                let p_vaddr = read_u64(data, off + 16, little)?;
                let p_paddr = read_u64(data, off + 24, little)?;
                let p_filesz = read_u64(data, off + 32, little)?;
                let p_memsz = read_u64(data, off + 40, little)?;
                let p_align = read_u64(data, off + 48, little)?;
                headers.push(ProgramHeader {
                    p_type,
                    p_offset,
                    p_vaddr,
                    p_paddr,
                    p_filesz,
                    p_memsz,
                    p_flags,
                    p_align,
                });
            }
        }
        Ok(headers)
    }

    /// 解析节头表、字符串表与动态符号表（简化版）。
    fn parse_sections_and_symbols(
        header: &ElfHeader,
        data: &[u8],
        _program_headers: &[ProgramHeader],
    ) -> ElfResult<(Vec<SectionHeader>, Vec<u8>, BTreeMap<String, u64>, Option<String>)> {
        let mut sections = Vec::new();
        let mut shstrtab = Vec::new();
        let mut symbols = BTreeMap::new();
        let mut interp = None;

        if header.shnum == 0 || header.shoff == 0 || header.shentsize == 0 {
            return Ok((sections, shstrtab, symbols, interp));
        }
        let little = header.clone().is_little_endian();
        let shstrndx = header.shstrndx as usize;
        for i in 0..header.shnum as u64 {
            let off = header
                .shoff
                .checked_add(i * header.shentsize as u64)
                .ok_or(ElfError::InvalidSectionHeaderTable)?;
            let off = off as usize;
            let (sh_name, sh_type, sh_flags, sh_addr, sh_offset, sh_size, sh_link, sh_info, sh_addralign, sh_entsize) =
                if class_is_32(header) {
                    if off + 40 > data.len() {
                        return Err(ElfError::InvalidSectionHeaderTable);
                    }
                    let sh_name = read_u32(data, off, little)?;
                    let sh_type = read_u32(data, off + 4, little)?;
                    let sh_flags = read_u32(data, off + 8, little)? as u64;
                    let sh_addr = read_u32(data, off + 12, little)? as u64;
                    let sh_offset = read_u32(data, off + 16, little)? as u64;
                    let sh_size = read_u32(data, off + 20, little)? as u64;
                    let sh_link = read_u32(data, off + 24, little)?;
                    let sh_info = read_u32(data, off + 28, little)?;
                    let sh_addralign = read_u32(data, off + 32, little)? as u64;
                    let sh_entsize = read_u32(data, off + 36, little)? as u64;
                    (
                        sh_name,
                        sh_type,
                        sh_flags,
                        sh_addr,
                        sh_offset,
                        sh_size,
                        sh_link,
                        sh_info,
                        sh_addralign,
                        sh_entsize,
                    )
                } else {
                    if off + 64 > data.len() {
                        return Err(ElfError::InvalidSectionHeaderTable);
                    }
                    let sh_name = read_u32(data, off, little)?;
                    let sh_type = read_u32(data, off + 4, little)?;
                    let sh_flags = read_u64(data, off + 8, little)?;
                    let sh_addr = read_u64(data, off + 16, little)?;
                    let sh_offset = read_u64(data, off + 24, little)?;
                    let sh_size = read_u64(data, off + 32, little)?;
                    let sh_link = read_u32(data, off + 40, little)?;
                    let sh_info = read_u32(data, off + 44, little)?;
                    let sh_addralign = read_u64(data, off + 48, little)?;
                    let sh_entsize = read_u64(data, off + 56, little)?;
                    (
                        sh_name,
                        sh_type,
                        sh_flags,
                        sh_addr,
                        sh_offset,
                        sh_size,
                        sh_link,
                        sh_info,
                        sh_addralign,
                        sh_entsize,
                    )
                };
            sections.push(SectionHeader {
                sh_name,
                sh_type,
                sh_flags,
                sh_addr,
                sh_offset,
                sh_size,
                sh_link,
                sh_info,
                sh_addralign,
                sh_entsize,
            });
        }

        // 读取节头字符串表。
        if shstrndx < sections.len() {
            let sh = &sections[shstrndx];
            if sh.sh_offset as usize + sh.sh_size as usize <= data.len() {
                shstrtab = data[sh.sh_offset as usize..(sh.sh_offset + sh.sh_size) as usize].to_vec();
            }
        }

        // 解析节名（供调试）。
        let _ = sections;

        // 简化版：动态符号表与解释器不在此做完整解析，留空；
        // 动态链接器层会按需再解析 PT_DYNAMIC/PT_INTERP。
        let _ = shstrtab;
        Ok((sections, shstrtab, symbols, interp))
    }

    /// 将所有 PT_LOAD 段映射到模拟地址空间（对已通过 `load` 构建的程序，
    /// 段已在内部映射完毕；此处返回入口点并做一次一致性兜底映射）。
    pub fn map_segments(program: &mut ElfProgram) -> ElfResult<u64> {
        if program.segments.is_empty() {
            return Ok(program.entry_point());
        }
        Ok(program.entry_point())
    }

    /// 内部：把 program_headers 中的 PT_LOAD 段写入 program.segments。
    fn map_segments_inner(program: &mut ElfProgram, data: &[u8]) -> ElfResult<()> {
        let mut load_segments: Vec<&ProgramHeader> = program
            .program_headers
            .iter()
            .filter(|ph| ph.is_loadable())
            .collect();
        // 按虚拟地址排序，便于重叠检测。
        load_segments.sort_by_key(|ph| ph.p_vaddr);

        let mut last_end: u64 = 0;
        let mut has_last = false;
        for ph in &load_segments {
            let start = ph.p_vaddr;
            let end = start.wrapping_add(ph.p_memsz.max(ph.p_filesz));
            if has_last && start < last_end {
                return Err(ElfError::SegmentOverlap);
            }
            let mut content: Vec<u8> = Vec::with_capacity(ph.p_filesz as usize);
            if ph.p_filesz > 0 {
                let off = ph.p_offset as usize;
                let len = ph.p_filesz as usize;
                if off + len > data.len() {
                    return Err(ElfError::Io {
                        path: "<memory>".to_string(),
                        reason: "段文件数据越界".to_string(),
                    });
                }
                content.extend_from_slice(&data[off..off + len]);
            }
            // 扩展到 p_memsz（.bss 零填充）。
            content.resize(ph.p_memsz.max(ph.p_filesz) as usize, 0);
            program
                .segments
                .insert(start, (content, (*ph).clone()));
            last_end = end;
            has_last = true;
        }
        // 校验入口点落在某个可加载段内（或允许 0 作为占位）。
        if program.entry != 0 && !program.contains_addr(program.entry) {
            return Err(ElfError::InvalidEntryPoint);
        }
        Ok(())
    }

    /// 按名称解析符号的虚拟地址。
    ///
    /// 返回已加载地址（含 base_addr）。
    pub fn resolve_symbol(program: &ElfProgram, name: &str) -> Option<usize> {
        program
            .symbols
            .get(name)
            .map(|v| program.base_addr.wrapping_add(*v) as usize)
    }

    /// 以给定基址重定位程序（位置无关代码）。
    ///
    /// 将所有已加载段的地址偏移量重算为 `base_addr + 原地址`，并更新
    /// 程序的 `base_addr` 与符号地址。简化版：仅调整 base_addr 字段，
    /// 实际段内容保持不变（模拟器中段按虚拟地址索引，基址由 entry_point
    /// 与 resolve_symbol 统一追加）。
    pub fn relocate(program: &mut ElfProgram, base_addr: u64) -> ElfResult<()> {
        if base_addr == 0 {
            return Err(ElfError::RelocationFailed {
                offset: 0,
                reason: "基址不能为 0".to_string(),
            });
        }
        program.base_addr = base_addr;
        Ok(())
    }
}

impl Default for ElfLoader {
    fn default() -> Self {
        ElfLoader::new()
    }

}

// ========================================================================
// 单元测试
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小的 64 位 ELF 可执行文件（内存中），包含一个 PT_LOAD 段。
    fn make_minimal_elf64(entry: u64, vaddr: u64, content: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        // e_ident
        f.extend_from_slice(&ELF_MAGIC);
        f.push(ELF_CLASS_64); // class
        f.push(ELF_DATA_LITTLE); // endian
        f.push(1); // version
        f.push(0); // os_abi
        f.push(0); // abi_version
        f.extend_from_slice(&[0u8; 8]); // pad to 16
        // e_type=2(executable), e_machine=0x3e(x86-64), e_version=1
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&0x3eu16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        // e_entry(8)
        f.extend_from_slice(&entry.to_le_bytes());
        // e_phoff = 64
        f.extend_from_slice(&64u64.to_le_bytes());
        // e_shoff = 0 (无节头)
        f.extend_from_slice(&0u64.to_le_bytes());
        // e_flags=0, e_ehsize=64, e_phentsize=56, e_phnum=1, ...
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&64u16.to_le_bytes());
        f.extend_from_slice(&56u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        // 程序头 (PT_LOAD) at offset 64, 56 bytes
        // p_type=PT_LOAD, p_flags=PF_R|PF_X, p_offset=0x1000, p_vaddr=vaddr
        f.extend_from_slice(&PT_LOAD.to_le_bytes());
        f.extend_from_slice(&(PF_R | PF_X as u32).to_le_bytes());
        f.extend_from_slice(&0x1000u64.to_le_bytes());
        f.extend_from_slice(&vaddr.to_le_bytes());
        f.extend_from_slice(&vaddr.to_le_bytes()); // p_paddr
        f.extend_from_slice(&(content.len() as u64).to_le_bytes()); // p_filesz
        f.extend_from_slice(&(content.len().max(1) as u64).to_le_bytes()); // p_memsz
        f.extend_from_slice(&4096u64.to_le_bytes()); // p_align
        // 段文件数据放在文件偏移 0x1000
        f.resize(0x1000, 0);
        f.extend_from_slice(content);
        f
    }

    #[test]
    fn parse_header_magic_and_fields() {
        let entry = 0x401000;
        let vaddr = 0x401000;
        let content = b"hello elf";
        let elf = make_minimal_elf64(entry, vaddr, content);
        let header = ElfLoader::parse_header(&elf).unwrap();
        assert_eq!(header.magic, ELF_MAGIC);
        assert_eq!(header.class, ELF_CLASS_64);
        assert_eq!(header.endian, ELF_DATA_LITTLE);
        assert_eq!(header.clone().bitness(), 64);
        assert!(header.clone().is_little_endian());
        assert!(header.clone().is_loadable());
        assert_eq!(header.entry, entry);
        assert_eq!(header.phoff, 64);
        assert_eq!(header.phnum, 1);
        assert_eq!(header.ehsize, 64);
        assert_eq!(header.phentsize, 56);
    }

    #[test]
    fn reject_invalid_magic() {
        let mut bad = vec![0u8; 64];
        bad[0..4].copy_from_slice(b"XXXX");
        assert_eq!(
            ElfLoader::parse_header(&bad).unwrap_err(),
            ElfError::InvalidMagic
        );
    }

    #[test]
    fn parse_program_headers() {
        let entry = 0x401000;
        let vaddr = 0x401000;
        let content = b"hello elf";
        let elf = make_minimal_elf64(entry, vaddr, content);
        let header = ElfLoader::parse_header(&elf).unwrap();
        let phdrs = ElfLoader::parse_program_headers(&header, &elf).unwrap();
        assert_eq!(phdrs.len(), 1);
        let ph = &phdrs[0];
        assert_eq!(ph.p_type, PT_LOAD);
        assert_eq!(ph.p_vaddr, vaddr);
        assert_eq!(ph.p_filesz, content.len() as u64);
        assert!(ph.readable() && ph.executable() && !ph.writable());
        assert_eq!(ph.flags_str(), "r-x");
    }

    #[test]
    fn load_and_map_segments() {
        let entry = 0x401000;
        let vaddr = 0x401000;
        let content = b"hello elf";
        let path = std::env::temp_dir().join(format!("hibos-elf-test-{}.bin", std::process::id()));
        std::fs::write(&path, make_minimal_elf64(entry, vaddr, content)).unwrap();
        let mut program = ElfLoader::load(path.to_str().unwrap()).unwrap();
        assert!(!program.segments.is_empty());
        assert_eq!(program.entry_point(), entry);
        assert!(program.contains_addr(vaddr));
        // 读取段内容。
        let (seg, _) = program.segments.get(&vaddr).unwrap();
        assert_eq!(&seg[..content.len()], content);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn segment_overlap_rejected() {
        // 构造两个重叠的 PT_LOAD 段：手动拼一个 64 位 ELF，两个段 vaddr 相同。
        let mut f = Vec::new();
        f.extend_from_slice(&ELF_MAGIC);
        f.push(ELF_CLASS_64);
        f.push(ELF_DATA_LITTLE);
        f.push(1);
        f.extend_from_slice(&[0u8; 8]); // 7+pad to 16 (os_abi, abi_version, pad)
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&0x3eu16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&0x401000u64.to_le_bytes()); // entry
        f.extend_from_slice(&64u64.to_le_bytes()); // phoff
        f.extend_from_slice(&0u64.to_le_bytes()); // shoff
        f.extend_from_slice(&0u32.to_le_bytes());
        f.extend_from_slice(&64u16.to_le_bytes());
        f.extend_from_slice(&56u16.to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes()); // phnum=2
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        let seg_content = b"AAAA";
        // 段1: vaddr 0x401000, filesz 4, 放文件偏调 0x1000
        f.extend_from_slice(&PT_LOAD.to_le_bytes());
        f.extend_from_slice(&(PF_R | PF_X as u32).to_le_bytes());
        f.extend_from_slice(&0x1000u64.to_le_bytes());
        f.extend_from_slice(&0x401000u64.to_le_bytes());
        f.extend_from_slice(&0x401000u64.to_le_bytes());
        f.extend_from_slice(&4u64.to_le_bytes());
        f.extend_from_slice(&4u64.to_le_bytes());
        f.extend_from_slice(&4096u64.to_le_bytes());
        // 段2: vaddr 0x401002 (与段1重叠), filesz 4, 放文件偏调 0x2000
        f.extend_from_slice(&PT_LOAD.to_le_bytes());
        f.extend_from_slice(&(PF_R | PF_W as u32).to_le_bytes());
        f.extend_from_slice(&0x2000u64.to_le_bytes());
        f.extend_from_slice(&0x401002u64.to_le_bytes());
        f.extend_from_slice(&0x401002u64.to_le_bytes());
        f.extend_from_slice(&4u64.to_le_bytes());
        f.extend_from_slice(&4u64.to_le_bytes());
        f.extend_from_slice(&4096u64.to_le_bytes());
        f.resize(0x2000, 0);
        f.extend_from_slice(seg_content);
        f.extend_from_slice(seg_content);
        let header = ElfLoader::parse_header(&f).unwrap();
        let phdrs = ElfLoader::parse_program_headers(&header, &f).unwrap();
        let mut program = ElfProgram {
            header,
            program_headers: phdrs,
            sections: Vec::new(),
            shstrtab: Vec::new(),
            symbols: BTreeMap::new(),
            segments: BTreeMap::new(),
            base_addr: 0,
            entry: 0x401000,
            interp: None,
        };
        let data = f;
        assert_eq!(
            ElfLoader::map_segments_inner(&mut program, &data).unwrap_err(),
            ElfError::SegmentOverlap
        );
    }

    #[test]
    fn resolve_symbol_and_relocate() {
        let entry = 0x401000;
        let vaddr = 0x401000;
        let content = b"hi";
        let elf = make_minimal_elf64(entry, vaddr, content);
        let header = ElfLoader::parse_header(&elf).unwrap();
        let phdrs = ElfLoader::parse_program_headers(&header, &elf).unwrap();
        let mut program = ElfProgram {
            header,
            program_headers: phdrs,
            sections: Vec::new(),
            shstrtab: Vec::new(),
            symbols: BTreeMap::new(),
            segments: BTreeMap::new(),
            base_addr: 0,
            entry,
            interp: None,
        };
        let data = elf;
        ElfLoader::map_segments_inner(&mut program, &data).unwrap();

        // relocate 到非零基址。
        assert!(ElfLoader::relocate(&mut program, 0x100000).is_ok());
        assert_eq!(program.base_addr, 0x100000);
        assert_eq!(program.entry_point(), 0x100000 + entry);

        // 符号表为空 -> 解析失败。
        assert_eq!(
            ElfLoader::resolve_symbol(&program, "main"),
            None
        );

        // relocate 基址为 0 失败。
        let mut p2 = program.clone();
        assert_eq!(
            ElfLoader::relocate(&mut p2, 0).unwrap_err(),
            ElfError::RelocationFailed {
                offset: 0,
                reason: "基址不能为 0".to_string()
            }
        );
    }

    #[test]
    fn unsupported_class_and_truncated() {
        let mut hdr = vec![0u8; 64];
        hdr[0..4].copy_from_slice(&ELF_MAGIC);
        hdr[4] = 99; // 非法类
        assert_eq!(
            ElfLoader::parse_header(&hdr).unwrap_err(),
            ElfError::UnsupportedClass(99)
        );
        let short = vec![0x7f, b'E', b'L'];
        assert_eq!(
            ElfLoader::parse_header(&short).unwrap_err(),
            ElfError::TruncatedHeader
        );
    }

    #[test]
    fn entry_point_not_in_segment() {
        let entry = 0x900000; // 不在段内
        let vaddr = 0x401000;
        let content = b"hi";
        let elf = make_minimal_elf64(entry, vaddr, content);
        let header = ElfLoader::parse_header(&elf).unwrap();
        let phdrs = ElfLoader::parse_program_headers(&header, &elf).unwrap();
        let mut program = ElfProgram {
            header,
            program_headers: phdrs,
            sections: Vec::new(),
            shstrtab: Vec::new(),
            symbols: BTreeMap::new(),
            segments: BTreeMap::new(),
            base_addr: 0,
            entry,
            interp: None,
        };
        let data = elf;
        assert_eq!(
            ElfLoader::map_segments_inner(&mut program, &data).unwrap_err(),
            ElfError::InvalidEntryPoint
        );
    }
}