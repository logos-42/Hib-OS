//! 分区表解析（MBR / GPT 简化版）。
//!
//! 把磁盘镜像（`DiskImage` 或任意实现块读写的设备）上的主引导记录
//! 与 GUID 分区表解析成 `Partition` 列表，供上层文件系统挂载使用。
//!
//! ## MBR 布局（512 字节）
//! - [0, 446)   引导代码区（446 字节）
//! - [446, 510) 四个分区表项，每项 16 字节
//! - [510, 512) 签名 0x55AA（小端存储为 0xAA55）
//!
//! ## GPT 简化版（LBA 1 开始）
//! - LBA 1：GPT 头（签名、头大小、头 CRC、当前 LBA、备份 LBA、分区入口
//!   开始 LBA、分区入口数量、分区入口大小、分区入口数组 CRC）
//! - LBA 2..：分区入口数组（每项 128 字节：类型 GUID、唯一 GUID、起始 LBA、
//!   结束 LBA、属性、名称 36 字符 UTF-16LE）
//!
//! CRC32 用于校验 GPT 头与分区入口数组。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::fs::disk_img::DiskImage;

/// 分区类型标识符（MBR 用 1 字节 ID，GPT 用 16 字节 GUID，此处统一用 u128）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartType(pub u128);

impl PartType {
    /// 常见 MBR 分区类型：空
    pub const EMPTY: PartType = PartType(0x00);
    /// 常见 MBR 分区类型：FAT12
    pub const FAT12: PartType = PartType(0x01);
    /// 常见 MBR 分区类型：FAT16 (<32M)
    pub const FAT16: PartType = PartType(0x04);
    /// 常见 MBR 分区类型：扩展分区
    pub const EXTENDED: PartType = PartType(0x05);
    /// 常见 MBR 分区类型：FAT32
    pub const FAT32: PartType = PartType(0x0B);
    /// 常见 MBR 分区类型：NTFS / exFAT
    pub const NTFS: PartType = PartType(0x07);
    /// 常见 MBR 分区类型：Linux
    pub const LINUX: PartType = PartType(0x83);
    /// 常见 MBR 分区类型：Linux swap
    pub const LINUX_SWAP: PartType = PartType(0x82);
    /// 常见 GPT 分区类型：未用
    pub const GPT_UNUSED: PartType = PartType(0);
    /// 常见 GPT 分区类型：EFI 系统分区
    pub const GPT_EFI: PartType = PartType(
        0xC12A7328_F81F_11D2_BA4B_00A0C93EC93B,
    );
    /// 常见 GPT 分区类型：Linux 文件系统数据
    pub const GPT_LINUX_DATA: PartType = PartType(
        0x0FC63DAF_8483_4772_8E79_3D69D8477DE4,
    );
}

impl std::fmt::Display for PartType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032X}", self.0)
    }
}

/// 单个分区的元信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    /// 分区索引（MBR 中为 0..4，GPT 中为入口数组下标）。
    pub index: u32,
    /// 起始扇区（LBA）。
    pub start_sector: u64,
    /// 扇区数量。
    pub sector_count: u64,
    /// 分区类型（MBR ID 或 GPT 类型 GUID）。
    pub part_type: PartType,
    /// 分区唯一 GUID（GPT 有效，MBR 为 0）。
    pub guid: u128,
    /// 分区名称（GPT UTF-16 名称，MBR 为空）。
    pub name: String,
    /// 是否可引导（MBR 引导标志 0x80）。
    pub bootable: bool,
}

impl Partition {
    /// 返回分区结束扇区（不含，即 start + count）。
    pub fn end_sector(&self) -> u64 {
        self.start_sector.saturating_add(self.sector_count)
    }

    /// 返回分区大小（字节），按 512 字节/扇区计算。
    pub fn size_bytes(&self) -> u64 {
        self.sector_count * 512
    }

    /// 判断该分区是否为空（未使用）。
    pub fn is_empty(&self) -> bool {
        self.sector_count == 0 && self.part_type == PartType::EMPTY
    }
}

/// 分区表解析结果：持有全部已解析分区及表类型。
#[derive(Clone, Debug, Default)]
pub struct PartitionTable {
    /// 表类型：Mbr 或 Gpt。
    pub table_type: TableType,
    /// 已解析的分区（按索引排序，非空项）。
    pub partitions: Vec<Partition>,
    /// 磁盘 GUID（GPT 磁盘唯一标识，MBR 为 0）。
    pub disk_guid: u128,
    /// LBA 0 是否为有效 MBR（签名 0xAA55）。
    pub mbr_valid: bool,
    /// GPT 头是否校验通过。
    pub gpt_valid: bool,
}

/// 分区表类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableType {
    Unknown,
    Mbr,
    Gpt,
}

/// 分区表错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartitionError {
    /// 设备读写错误。
    IoError(String),
    /// 扇区超出设备范围。
    OutOfRange,
    /// MBR 签名不匹配。
    BadMbrSignature,
    /// GPT 签名不匹配。
    BadGptSignature,
    /// GPT CRC 校验失败。
    BadCrc,
    /// 分区表项损坏。
    CorruptEntry,
}

impl std::fmt::Display for PartitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionError::IoError(e) => write!(f, "分区表 IO 错误: {}", e),
            PartitionError::OutOfRange => write!(f, "扇区超出设备范围"),
            PartitionError::BadMbrSignature => write!(f, "MBR 签名无效 (非 0xAA55)"),
            PartitionError::BadGptSignature => write!(f, "GPT 签名无效"),
            PartitionError::BadCrc => write!(f, "GPT CRC 校验失败"),
            PartitionError::CorruptEntry => write!(f, "分区表项损坏"),
        }
    }
}

/// 从 `DiskImage` 解析分区表。
///
/// 策略：先读 LBA 0，判断 MBR 签名；若签名有效则解析 MBR 四个分区项；
/// 随后读 LBA 1 判断 GPT 头签名 "EFI PART"，若有效且 CRC 通过则优先
/// 使用 GPT 分区表（GPT 优先于 MBR，符合 UEFI 规范）。
pub fn parse(disk: &mut DiskImage) -> Result<PartitionTable, PartitionError> {
    let sector_size = disk.sector_size();
    if sector_size < 512 {
        return Err(PartitionError::IoError(
            "扇区大小小于 512 字节，无法解析分区表".into(),
        ));
    }

    // 读 LBA 0。
    let mut lba0 = vec![0u8; 512];
    disk.read_block(0, &mut lba0).map_err(|e| {
        PartitionError::IoError(format!("读 LBA 0 失败: {}", e))
    })?;

    let mbr_valid = u16::from_le_bytes(lba0[510..512].try_into().unwrap_or([0; 2]))
        == 0xAA55;

    let mut table = PartitionTable {
        table_type: TableType::Unknown,
        partitions: Vec::new(),
        disk_guid: 0,
        mbr_valid,
        gpt_valid: false,
    };

    // 先尝试 GPT（LBA 1）。
    if disk.sector_count() > 1 {
        let mut lba1 = vec![0u8; 512];
        if disk.read_block(1, &mut lba1).is_ok() {
            if let Ok(gpt) = parse_gpt_header(&lba1, disk) {
                table.gpt_valid = true;
                // 读取分区入口数组
                let entry_start = gpt.entry_start_lba;
                let entry_count = gpt.entry_count as usize;
                let entry_size = gpt.entry_size as usize;
                let entries_bytes = entry_count * entry_size;
                let entries_start_sector = entry_start;
                let sectors_needed = (entries_bytes as u64 + sector_size - 1) / sector_size;
                let mut entries_buf = vec![0u8; (sectors_needed * sector_size) as usize];
                disk.read_blocks(entries_start_sector, sectors_needed, &mut entries_buf)
                    .map_err(|e| PartitionError::IoError(format!("读 GPT 分区入口数组失败: {}", e)))?;
                let entries_slice = &entries_buf[..entries_bytes];
                let entries_crc = crc32(entries_slice);
                if entries_crc != gpt.entry_array_crc {
                    return Err(PartitionError::BadCrc);
                }
                let mut parts = Vec::new();
                for i in 0..entry_count {
                    let off = i * entry_size;
                    if off + 128 > entries_slice.len() {
                        break;
                    }
                    if let Some(p) = parse_gpt_entry(&entries_slice[off..off + 128], i as u32) {
                        parts.push(p);
                    }
                }
                table.table_type = TableType::Gpt;
                table.disk_guid = gpt.disk_guid;
                table.partitions = parts;
                return Ok(table);
            }
        }
    }

    // 回退到 MBR。
    if mbr_valid {
        let mut parts = parse_mbr(&lba0)?;
        table.table_type = TableType::Mbr;
        table.disk_guid = mbr_disk_signature(&lba0);
        table.partitions = parts;
        return Ok(table);
    }

    Ok(table)
}

/// 解析 GPT 头（LBA 1 的前 92 字节关键字段，其余按规范简化）。
struct GptHeader {
    signature: [u8; 8],
    revision: u32,
    header_size: u32,
    header_crc: u32,
    reserved: u32,
    current_lba: u64,
    backup_lba: u64,
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: u128,
    entry_start_lba: u64,
    entry_count: u32,
    entry_size: u32,
    entry_array_crc: u32,
}

/// GPT 头签名 "EFI PART"（ASCII）。
const GPT_SIGNATURE: &[u8; 8] = b"EFI PART";

fn parse_gpt_header(buf: &[u8], disk: &DiskImage) -> Result<GptHeader, PartitionError> {
    if buf.len() < 92 {
        return Err(PartitionError::IoError("GPT 头缓冲区过小".into()));
    }
    if &buf[0..8] != GPT_SIGNATURE {
        return Err(PartitionError::BadGptSignature);
    }
    let header_crc = u32::from_le_bytes(buf[16..20].try_into().unwrap_or([0; 4]));
    // 校验头 CRC：将 CRC 字段置零后计算。
    let mut header_buf = buf[..92].to_vec();
    header_buf[16..20].copy_from_slice(&[0u8; 4]);
    let computed = crc32(&header_buf);
    if computed != header_crc {
        return Err(PartitionError::BadCrc);
    }
    let revision = u32::from_le_bytes(buf[8..12].try_into().unwrap_or([0; 4]));
    let header_size = u32::from_le_bytes(buf[12..16].try_into().unwrap_or([0; 4]));
    let current_lba = u64::from_le_bytes(buf[24..32].try_into().unwrap_or([0; 8]));
    let backup_lba = u64::from_le_bytes(buf[32..40].try_into().unwrap_or([0; 8]));
    let first_usable_lba = u64::from_le_bytes(buf[40..48].try_into().unwrap_or([0; 8]));
    let last_usable_lba = u64::from_le_bytes(buf[48..56].try_into().unwrap_or([0; 8]));
    let disk_guid_bytes = &buf[56..72];
    let disk_guid = u128::from_le_bytes(
        disk_guid_bytes.try_into().unwrap_or([0; 16]),
    );
    let entry_start_lba = u64::from_le_bytes(buf[72..80].try_into().unwrap_or([0; 8]));
    let entry_count = u32::from_le_bytes(buf[80..84].try_into().unwrap_or([0; 4]));
    let entry_size = u32::from_le_bytes(buf[84..88].try_into().unwrap_or([0; 4]));
    let entry_array_crc = u32::from_le_bytes(buf[88..92].try_into().unwrap_or([0; 4]));

    let _ = (revision, header_size, current_lba, backup_lba, first_usable_lba, last_usable_lba);
    Ok(GptHeader {
        signature: *GPT_SIGNATURE,
        revision,
        header_size,
        header_crc,
        reserved: 0,
        current_lba,
        backup_lba,
        first_usable_lba,
        last_usable_lba,
        disk_guid,
        entry_start_lba,
        entry_count,
        entry_size,
        entry_array_crc,
    })
}

/// 解析单个 GPT 分区入口（128 字节）。
fn parse_gpt_entry(buf: &[u8], index: u32) -> Option<Partition> {
    if buf.len() < 128 {
        return None;
    }
    // 类型 GUID（前 16 字节）：全零表示未使用条目。
    let type_guid_bytes = &buf[0..16];
    let type_guid = u128::from_le_bytes(type_guid_bytes.try_into().unwrap_or([0; 16]));
    if type_guid == 0 {
        return None; // 未使用条目
    }
    let guid_bytes = &buf[16..32];
    let guid = u128::from_le_bytes(guid_bytes.try_into().unwrap_or([0; 16]));
    let start_lba = u64::from_le_bytes(buf[32..40].try_into().unwrap_or([0; 8]));
    let end_lba = u64::from_le_bytes(buf[40..48].try_into().unwrap_or([0; 8]));
    let _attributes = u64::from_le_bytes(buf[48..56].try_into().unwrap_or([0; 8]));
    // 名称：36 个字符 UTF-16LE，共 72 字节 [56, 128)。
    let name_bytes = &buf[56..128];
    let name = utf16le_to_string(name_bytes);

    Some(Partition {
        index,
        start_sector: start_lba,
        sector_count: end_lba.saturating_sub(start_lba).saturating_add(1),
        part_type: PartType(type_guid),
        guid,
        name,
        bootable: false, // GPT 不使用 MBR 引导标志
    })
}

/// 将 UTF-16LE 字节序列转换为字符串（以空字符终止）。
fn utf16le_to_string(bytes: &[u8]) -> String {
    let mut chars = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let lo = bytes[i] as u16;
        let hi = (bytes[i + 1] as u16) << 8;
        let code = lo | hi;
        if code == 0 {
            break;
        }
        chars.push(char::from_u32(code as u32).unwrap_or('\u{FFFD}'));
        i += 2;
    }
    chars.into_iter().collect()
}

/// 解析 MBR 四个分区表项（每项 16 字节，偏移 446 开始）。
fn parse_mbr(buf: &[u8]) -> Result<Vec<Partition>, PartitionError> {
    let mut parts = Vec::new();
    for i in 0..4 {
        let off = 446 + i * 16;
        if off + 16 > buf.len() {
            break;
        }
        let entry = &buf[off..off + 16];
        let bootable = entry[0] == 0x80;
        let part_type_id = entry[4];
        // 起始 LBA：偏移 8，4 字节小端。
        let start_lba = u32::from_le_bytes(entry[8..12].try_into().unwrap_or([0; 4])) as u64;
        // 扇区数：偏移 12，4 字节小端。
        let sector_count = u32::from_le_bytes(entry[12..16].try_into().unwrap_or([0; 4])) as u64;
        if sector_count == 0 {
            continue; // 空条目
        }
        parts.push(Partition {
            index: i as u32,
            start_sector: start_lba,
            sector_count,
            part_type: PartType(part_type_id as u128),
            guid: 0,
            name: String::new(),
            bootable,
        });
    }
    Ok(parts)
}

/// 从 MBR 提取磁盘签名（偏移 440，4 字节）。
fn mbr_disk_signature(buf: &[u8]) -> u128 {
    if buf.len() < 444 {
        return 0;
    }
    let sig = u32::from_le_bytes(buf[440..444].try_into().unwrap_or([0; 4]));
    sig as u128
}

/// 分区表查询接口。
impl PartitionTable {
    /// 按名称查找分区（名称匹配 GPT 名称，或生成 "part<N>" 用于 MBR）。
    pub fn partition_by_name(&self, name: &str) -> Option<&Partition> {
        self.partitions.iter().find(|p| {
            if !p.name.is_empty() {
                p.name == name
            } else {
                format!("part{}", p.index) == name
            }
        })
    }

    /// 按索引查找分区。
    pub fn partition_by_index(&self, idx: u32) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.index == idx)
    }

    /// 返回第一个可引导分区。
    pub fn boot_partition(&self) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.bootable)
    }

    /// 返回所有非空分区（按起始扇区排序）。
    pub fn active_partitions(&self) -> Vec<&Partition> {
        let mut v: Vec<&Partition> = self.partitions.iter().filter(|p| !p.is_empty()).collect();
        v.sort_by_key(|p| p.start_sector);
        v
    }

    /// 检查两个分区是否有扇区重叠。
    pub fn overlaps(&self, a: &Partition, b: &Partition) -> bool {
        let a_end = a.end_sector();
        let b_end = b.end_sector();
        a.start_sector < b_end && b.start_sector < a_end
    }

    /// 返回分区表摘要字符串（供 `sense`/`act` 通道调用）。
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("表类型: {:?}\n", self.table_type));
        out.push_str(&format!("MBR 有效: {}, GPT 有效: {}\n", self.mbr_valid, self.gpt_valid));
        out.push_str(&format!("磁盘 GUID: {:032X}\n", self.disk_guid));
        out.push_str(&format!("分区数: {}\n", self.partitions.len()));
        for p in &self.partitions {
            out.push_str(&format!(
                "  [{}] start={} count={} type={:032X} bootable={} name={:?}\n",
                p.index,
                p.start_sector,
                p.sector_count,
                p.part_type.0,
                p.bootable,
                if p.name.is_empty() { "-" } else { &p.name }
            ));
        }
        out
    }
}

// ================= CRC32 =================

/// CRC32 查找表（多项式 0xEDB88320，反向表示）。
static CRC32_TABLE: OnceLock<[u32; 256]> = OnceLock::new();

fn crc32_table() -> &'static [u32; 256] {
    CRC32_TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for i in 0..256u32 {
            let mut crc = i;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
            table[i as usize] = crc;
        }
        table
    })
}

/// 计算数据的 CRC32 校验和。
pub fn crc32(data: &[u8]) -> u32 {
    let table = crc32_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc ^ (byte as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[idx];
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_disk(sectors: u64) -> DiskImage {
        let path = std::env::temp_dir().join(format!(
            "hibos_partition_{}_{}.img",
            std::thread::current().id().as_u64(),
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        DiskImage::create(&path, sectors * 512).unwrap()
    }

    #[test]
    fn crc32_known_vector() {
        // "123456789" 的 CRC32 = 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn mbr_parse_four_partitions() {
        let mut disk = make_disk(1024);
        // 构造一个带 4 个分区的 MBR（LBA 0）。
        let mut lba0 = [0u8; 512];
        // 分区 1：起始 2048，2048 扇区，类型 0x83，可引导
        let mut e1 = [0u8; 16];
        e1[0] = 0x80;
        e1[4] = 0x83;
        e1[8..12].copy_from_slice(&2048u32.to_le_bytes());
        e1[12..16].copy_from_slice(&2048u32.to_le_bytes());
        lba0[446..462].copy_from_slice(&e1);
        // 分区 2：起始 4096，4096 扇区，类型 0x07
        let mut e2 = [0u8; 16];
        e2[4] = 0x07;
        e2[8..12].copy_from_slice(&4096u32.to_le_bytes());
        e2[12..16].copy_from_slice(&4096u32.to_le_bytes());
        lba0[462..478].copy_from_slice(&e2);
        // 签名
        lba0[510] = 0x55;
        lba0[511] = 0xAA;
        disk.write_block(0, &lba0).unwrap();
        disk.flush().unwrap();

        let table = parse(&mut disk).unwrap();
        assert!(table.mbr_valid);
        assert_eq!(table.table_type, TableType::Mbr);
        assert_eq!(table.partitions.len(), 2);
        let p1 = table.partition_by_index(0).unwrap();
        assert_eq!(p1.start_sector, 2048);
        assert_eq!(p1.sector_count, 2048);
        assert!(p1.bootable);
        assert_eq!(p1.part_type, PartType::LINUX);
        let p2 = table.partition_by_index(1).unwrap();
        assert_eq!(p2.start_sector, 4096);
        assert_eq!(p2.sector_count, 4096);
        assert!(!p2.bootable);
        assert_eq!(p2.part_type, PartType::NTFS);
    }

    #[test]
    fn mbr_empty_entries_skipped() {
        let mut disk = make_disk(64);
        let mut lba0 = [0u8; 512];
        lba0[510] = 0x55;
        lba0[511] = 0xAA;
        disk.write_block(0, &lba0).unwrap();
        let table = parse(&mut disk).unwrap();
        assert!(table.mbr_valid);
        assert_eq!(table.partitions.len(), 0);
    }

    #[test]
    fn partition_by_name_and_index() {
        let mut disk = make_disk(1024);
        let mut lba0 = [0u8; 512];
        let mut e1 = [0u8; 16];
        e1[0] = 0x80;
        e1[4] = 0x83;
        e1[8..12].copy_from_slice(&100u32.to_le_bytes());
        e1[12..16].copy_from_slice(&50u32.to_le_bytes());
        lba0[446..462].copy_from_slice(&e1);
        lba0[510] = 0x55;
        lba0[511] = 0xAA;
        disk.write_block(0, &lba0).unwrap();
        let table = parse(&mut disk).unwrap();
        assert_eq!(table.partition_by_index(0).unwrap().start_sector, 100);
        // MBR 没有名称，按 "part0" 匹配
        assert!(table.partition_by_name("part0").is_some());
        assert!(table.partition_by_name("nonexistent").is_none());
    }

    #[test]
    fn partition_end_sector_and_size() {
        let p = Partition {
            index: 0,
            start_sector: 2048,
            sector_count: 4096,
            part_type: PartType::LINUX,
            guid: 0,
            name: String::new(),
            bootable: true,
        };
        assert_eq!(p.end_sector(), 6144);
        assert_eq!(p.size_bytes(), 4096 * 512);
        assert!(!p.is_empty());
        let empty = Partition {
            index: 1,
            start_sector: 0,
            sector_count: 0,
            part_type: PartType::EMPTY,
            guid: 0,
            name: String::new(),
            bootable: false,
        };
        assert!(empty.is_empty());
    }

    #[test]
    fn gpt_header_parse_and_crc() {
        // 构造一个最小合法 GPT 头。
        let mut buf = [0u8; 512];
        buf[0..8].copy_from_slice(b"EFI PART");
        buf[12..16].copy_from_slice(&92u32.to_le_bytes()); // header_size
        buf[24..32].copy_from_slice(&1u64.to_le_bytes());   // current_lba
        buf[32..40].copy_from_slice(&2u64.to_le_bytes());   // backup_lba
        buf[40..48].copy_from_slice(&34u64.to_le_bytes());  // first_usable
        buf[48..56].copy_from_slice(&100u64.to_le_bytes()); // last_usable
        buf[56..72].copy_from_slice(&0x1122334455667788u128.to_le_bytes()); // disk guid
        buf[72..80].copy_from_slice(&2u64.to_le_bytes());   // entry_start_lba
        buf[80..84].copy_from_slice(&128u32.to_le_bytes()); // entry_count
        buf[84..88].copy_from_slice(&128u32.to_le_bytes()); // entry_size
        // 计算头 CRC（CRC 字段置零）。
        let mut hbuf = buf[..92].to_vec();
        hbuf[16..20].copy_from_slice(&[0u8; 4]);
        let crc = crc32(&hbuf);
        buf[16..20].copy_from_slice(&crc.to_le_bytes());

        let mut disk = make_disk(200);
        disk.write_block(1, &buf).unwrap();
        let header = parse_gpt_header(&buf, &disk).unwrap();
        assert_eq!(header.entry_count, 128);
        assert_eq!(header.entry_start_lba, 2);
        assert_eq!(header.disk_guid, 0x1122334455667788);
    }

    #[test]
    fn gpt_partition_entry_parse() {
        let mut entry = [0u8; 128];
        // 类型 GUID：Linux 文件系统数据
        entry[0..16].copy_from_slice(&PartType::GPT_LINUX_DATA.0.to_le_bytes());
        entry[16..32].copy_from_slice(&0xDEADBEEFCAFEF00Du128.to_le_bytes());
        entry[32..40].copy_from_slice(&2048u64.to_le_bytes());
        entry[40..48].copy_from_slice(&4095u64.to_le_bytes());
        // 名称 "rootfs" UTF-16LE
        let name = "rootfs".encode_utf16().collect::<Vec<_>>();
        for (i, c) in name.iter().enumerate() {
            entry[56 + i * 2..56 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        let p = parse_gpt_entry(&entry, 0).unwrap();
        assert_eq!(p.start_sector, 2048);
        assert_eq!(p.sector_count, 4095 - 2048 + 1);
        assert_eq!(p.name, "rootfs");
        assert_eq!(p.part_type, PartType::GPT_LINUX_DATA);
        assert!(!p.bootable);
    }

    #[test]
    fn gpt_unused_entry_skipped() {
        let entry = [0u8; 128]; // 全零：未使用
        assert!(parse_gpt_entry(&entry, 0).is_none());
    }

    #[test]
    fn full_gpt_parse_roundtrip() {
        let mut disk = make_disk(2048);
        // 构造 GPT 头 + 一个分区入口。
        let mut gpt_head = [0u8; 512];
        gpt_head[0..8].copy_from_slice(b"EFI PART");
        gpt_head[12..16].copy_from_slice(&92u32.to_le_bytes());
        gpt_head[24..32].copy_from_slice(&1u64.to_le_bytes());
        gpt_head[32..40].copy_from_slice(&2u64.to_le_bytes());
        gpt_head[40..48].copy_from_slice(&34u64.to_le_bytes());
        gpt_head[48..56].copy_from_slice(&2000u64.to_le_bytes());
        gpt_head[56..72].copy_from_slice(&0xAABBCCDDEEFF0011u128.to_le_bytes());
        gpt_head[72..80].copy_from_slice(&2u64.to_le_bytes()); // 入口从 LBA 2 开始
        gpt_head[80..84].copy_from_slice(&1u32.to_le_bytes()); // 1 个入口
        gpt_head[84..88].copy_from_slice(&128u32.to_le_bytes());
        // 入口数组 CRC（只有一个 128 字节入口）。
        let mut entry = [0u8; 128];
        entry[0..16].copy_from_slice(&PartType::GPT_LINUX_DATA.0.to_le_bytes());
        entry[16..32].copy_from_slice(&0x0123456789ABCDEFu128.to_le_bytes());
        entry[32..40].copy_from_slice(&2048u64.to_le_bytes());
        entry[40..48].copy_from_slice(&6143u64.to_le_bytes());
        let entry_crc = crc32(&entry);
        gpt_head[88..92].copy_from_slice(&entry_crc.to_le_bytes());
        // 头 CRC
        let mut hbuf = gpt_head[..92].to_vec();
        hbuf[16..20].copy_from_slice(&[0u8; 4]);
        let head_crc = crc32(&hbuf);
        gpt_head[16..20].copy_from_slice(&head_crc.to_le_bytes());

        disk.write_block(1, &gpt_head).unwrap();
        disk.write_blocks(2, 1, &entry).unwrap();
        disk.flush().unwrap();

        let table = parse(&mut disk).unwrap();
        assert!(table.gpt_valid);
        assert_eq!(table.table_type, TableType::Gpt);
        assert_eq!(table.partitions.len(), 1);
        let p = table.partitions[0];
        assert_eq!(p.start_sector, 2048);
        assert_eq!(p.sector_count, 6143 - 2048 + 1);
        assert_eq!(p.part_type, PartType::GPT_LINUX_DATA);
    }

    #[test]
    fn partition_table_summary() {
        let mut disk = make_disk(512);
        let mut lba0 = [0u8; 512];
        let mut e1 = [0u8; 16];
        e1[4] = 0x83;
        e1[8..12].copy_from_slice(&2048u32.to_le_bytes());
        e1[12..16].copy_from_slice(&1024u32.to_le_bytes());
        lba0[446..462].copy_from_slice(&e1);
        lba0[510] = 0x55;
        lba0[511] = 0xAA;
        disk.write_block(0, &lba0).unwrap();
        let table = parse(&mut disk).unwrap();
        let s = table.summary();
        assert!(s.contains("Mbr"));
        assert!(s.contains("2048"));
        assert!(s.contains("1024"));
    }

    #[test]
    fn partition_table_overlaps() {
        let p1 = Partition {
            index: 0, start_sector: 100, sector_count: 100,
            part_type: PartType::LINUX, guid: 0, name: String::new(), bootable: false,
        };
        let p2 = Partition {
            index: 1, start_sector: 150, sector_count: 100,
            part_type: PartType::NTFS, guid: 0, name: String::new(), bootable: false,
        };
        let p3 = Partition {
            index: 2, start_sector: 300, sector_count: 50,
            part_type: PartType::FAT32, guid: 0, name: String::new(), bootable: false,
        };
        let table = PartitionTable::default();
        assert!(table.overlaps(&p1, &p2));
        assert!(!table.overlaps(&p1, &p3));
    }

    #[test]
    fn bad_mbr_signature_not_valid() {
        let mut disk = make_disk(64);
        let mut lba0 = [0u8; 512];
        lba0[510] = 0x00; // 无效签名
        lba0[511] = 0x00;
        disk.write_block(0, &lba0).unwrap();
        let table = parse(&mut disk).unwrap();
        assert!(!table.mbr_valid);
        assert_eq!(table.table_type, TableType::Unknown);
    }

    /// 将缓冲区写入磁盘指定扇区（测试辅助）。
    fn disk_write_lba(disk: &mut DiskImage, lba: u64, buf: &[u8]) {
        let mut sector = vec![0u8; 512];
        sector[..buf.len().min(512)].copy_from_slice(&buf[..buf.len().min(512)]);
        disk.write_block(lba, &sector).unwrap();
    }
}
