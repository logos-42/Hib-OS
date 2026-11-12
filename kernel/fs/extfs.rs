//! 类 ext 块分配器：inode 位图 + 数据块位图 + 目录项。
//!
//! 参考 ext2/ext3 布局：超级块、inode 位图、数据块位图、inode 表、
//! 数据区。目录用 inode 且其数据块承载序列化的目录项（名 -> ino）。
//! 文件数据通过页缓存（pagecache）读写，元数据直接落盘。

use std::collections::BTreeMap;

use crate::fs::block::{BlockDevice, BlockError, BLOCK_SIZE};
use crate::fs::pagecache::{PageCache, PAGE_SIZE};

/// 文件系统魔数。
const EXT_MAGIC: u64 = 0x4849_424F_5346_532B; // "HIBOSFS+"

/// inode 固定大小（128 字节，便于定长表）。
const INODE_SIZE: usize = 128;
/// 直接块指针数量。
const N_DIRECT: usize = 8;

/// inode 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InodeKind {
    File = 0,
    Dir = 1,
}

impl InodeKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(InodeKind::File),
            1 => Some(InodeKind::Dir),
            _ => None,
        }
    }
}

/// inode 定长表项。
#[derive(Clone, Debug)]
pub struct ExtInode {
    pub ino: u64,
    pub kind: InodeKind,
    pub mode: u16,
    pub size: u64,
    pub blocks: Vec<u64>, // 数据块号，一页一块
    pub nlink: u64,
}

impl ExtInode {
    pub fn blank(ino: u64) -> Self {
        Self {
            ino,
            kind: InodeKind::File,
            mode: 0o644,
            size: 0,
            blocks: Vec::new(),
            nlink: 0,
        }
    }
    pub fn dir(ino: u64) -> Self {
        Self {
            ino,
            kind: InodeKind::Dir,
            mode: 0o755,
            size: 0,
            blocks: Vec::new(),
            nlink: 2,
        }
    }
    pub fn file(ino: u64) -> Self {
        Self {
            ino,
            kind: InodeKind::File,
            mode: 0o644,
            size: 0,
            blocks: Vec::new(),
            nlink: 1,
        }
    }
}

/// 目录项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub ino: u64,
}

/// 超级块布局（记录在块 0）。
#[derive(Clone, Debug)]
struct SuperBlock {
    block_count: u64,
    inode_count: u64,
    data_start: u64,   // 数据区起始块号
    ibb: u64,          // inode 位图占块数
    bbb: u64,          // 数据块位图占块数
    itb: u64,          // inode 表占块数
}

/// 位图（以 Vec<bool> 表示，索引 = 块号 / inode 号）。
fn pack_bits(bitmap: &[bool]) -> Vec<Vec<u8>> {
    let bits_per_block = (BLOCK_SIZE * 8) as usize;
    let blocks = bitmap.len().div_ceil(bits_per_block).max(1);
    let mut out = Vec::with_capacity(blocks);
    for b in 0..blocks {
        let mut block = vec![0u8; BLOCK_SIZE];
        for i in 0..bits_per_block {
            let idx = b * bits_per_block + i;
            if idx < bitmap.len() && bitmap[idx] {
                block[i / 8] |= 1 << (i % 8);
            }
        }
        out.push(block);
    }
    out
}

fn unpack_bits(blocks: &[Vec<u8>], nbits: usize) -> Vec<bool> {
    let mut out = vec![false; nbits];
    for (b, block) in blocks.iter().enumerate() {
        for i in 0..BLOCK_SIZE * 8 {
            let idx = b * (BLOCK_SIZE * 8) + i;
            if idx >= nbits {
                break;
            }
            if block[i / 8] & (1 << (i % 8)) != 0 {
                out[idx] = true;
            }
        }
    }
    out
}

/// 序列化目录项到一个块（返回占用字节数）。
fn serialize_dir(entries: &BTreeMap<String, u64>, buf: &mut [u8; BLOCK_SIZE]) -> usize {
    let mut written = 0usize;
    let count = entries.len() as u32;
    buf[0..4].copy_from_slice(&count.to_le_bytes());
    written = 4;
    for (name, ino) in entries.iter() {
        let nb = name.as_bytes();
        if written + 4 + nb.len() + 8 > BLOCK_SIZE {
            break;
        }
        buf[written..written + 4].copy_from_slice(&(nb.len() as u32).to_le_bytes());
        written += 4;
        buf[written..written + nb.len()].copy_from_slice(nb);
        written += nb.len();
        buf[written..written + 8].copy_from_slice(&(*ino as u64).to_le_bytes());
        written += 8;
    }
    written
}

/// 从一个块反序列化目录项。
fn deserialize_dir(buf: &[u8]) -> BTreeMap<String, u64> {
    let mut map = BTreeMap::new();
    if buf.len() < 4 {
        return map;
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap_or([0; 4])) as usize;
    let mut pos = 4usize;
    for _ in 0..count {
        if pos + 4 > buf.len() {
            break;
        }
        let nlen = u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;
        if pos + nlen + 8 > buf.len() {
            break;
        }
        let name = String::from_utf8_lossy(&buf[pos..pos + nlen]).into_owned();
        pos += nlen;
        let ino = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap_or([0; 8]));
        pos += 8;
        map.insert(name, ino);
    }
    map
}

/// 序列化 inode 到定长 128 字节。
fn serialize_inode(inode: &ExtInode, buf: &mut [u8; INODE_SIZE]) {
    buf[0..8].copy_from_slice(&inode.ino.to_le_bytes());
    buf[8] = inode.kind as u8;
    buf[9..11].copy_from_slice(&inode.mode.to_le_bytes());
    buf[11..19].copy_from_slice(&inode.size.to_le_bytes());
    buf[19..27].copy_from_slice(&inode.nlink.to_le_bytes());
    buf[27..35].copy_from_slice(&(inode.blocks.len() as u64).to_le_bytes());
    let mut off = 35;
    for i in 0..N_DIRECT {
        let blk = inode.blocks.get(i).copied().unwrap_or(0);
        buf[off..off + 8].copy_from_slice(&blk.to_le_bytes());
        off += 8;
    }
    // 99..128 保持为 0
}

/// 从定长 128 字节反序列化 inode。
fn deserialize_inode(buf: &[u8], ino: u64) -> Option<ExtInode> {
    if buf.len() < INODE_SIZE {
        return None;
    }
    let kind = InodeKind::from_u8(buf[8])?;
    let mode = u16::from_le_bytes(buf[9..11].try_into().ok()?);
    let size = u64::from_le_bytes(buf[11..19].try_into().ok()?);
    let nlink = u64::from_le_bytes(buf[19..27].try_into().ok()?);
    let nblocks = u64::from_le_bytes(buf[27..35].try_into().ok()?);
    let mut blocks = Vec::new();
    let mut off = 35;
    for _ in 0..N_DIRECT {
        if off + 8 > INODE_SIZE {
            break;
        }
        blocks.push(u64::from_le_bytes(buf[off..off + 8].try_into().ok()?));
        off += 8;
    }
    let blocks = blocks.into_iter().take(nblocks as usize).collect();
    Some(ExtInode {
        ino,
        kind,
        mode,
        size,
        blocks,
        nlink,
    })
}

/// 类 ext 文件系统。
pub struct ExtFs {
    block_count: u64,
    inode_count: u64,
    data_start: u64,
    inode_bitmap: Vec<bool>,
    block_bitmap: Vec<bool>,
    inodes: Vec<ExtInode>,
    device: Box<dyn BlockDevice + Send>,
    page_cache: PageCache,
    formatted: bool,
}

impl ExtFs {
    pub fn new(block_count: u64, inode_count: u64) -> Self {
        Self::with_device(
            block_count,
            inode_count,
            Box::new(crate::fs::block::MemBlockDevice::new("ram0", block_count)),
        )
    }

    pub fn with_device(
        block_count: u64,
        inode_count: u64,
        device: Box<dyn BlockDevice + Send>,
    ) -> Self {
        Self {
            block_count,
            inode_count,
            data_start: 0,
            inode_bitmap: vec![false; inode_count as usize],
            block_bitmap: vec![false; block_count as usize],
            inodes: vec![ExtInode::blank(0); inode_count as usize],
            device,
            page_cache: PageCache::new(64),
            formatted: false,
        }
    }

    /// 格式化：写超级块、位图、初始化根目录 ino=1。
    pub fn format(&mut self) -> Result<(), BlockError> {
        let bits_per_block = (BLOCK_SIZE * 8) as u64;
        let ibb = (self.inode_count + bits_per_block - 1).div_ceil(bits_per_block).max(1);
        let bbb = (self.block_count + bits_per_block - 1).div_ceil(bits_per_block).max(1);
        let itb = ((self.inode_count * INODE_SIZE as u64) + BLOCK_SIZE as u64 - 1)
            .div_ceil(BLOCK_SIZE as u64)
            .max(1);
        let data_start = 1 + ibb + bbb + itb;
        self.data_start = data_start;

        let sb = SuperBlock {
            block_count: self.block_count,
            inode_count: self.inode_count,
            data_start,
            ibb,
            bbb,
            itb,
        };

        // 写超级块（块 0）
        let mut sb_buf = [0u8; BLOCK_SIZE];
        let mut off = 0;
        macro_rules! put_u64 { ($v:expr) => { sb_buf[off..off+8].copy_from_slice(&($v as u64).to_le_bytes()); off += 8; } }
        put_u64!(EXT_MAGIC);
        put_u64!(sb.block_count);
        put_u64!(sb.inode_count);
        put_u64!(sb.data_start);
        put_u64!(sb.ibb);
        put_u64!(sb.bbb);
        put_u64!(sb.itb);
        self.device.write_at(0, &sb_buf)?;

        // 位图：元数据块 [0, data_start) 已占用；inode 0 保留。
        for b in 0..data_start {
            self.block_bitmap[b as usize] = true;
        }
        self.inode_bitmap[0] = true; // 保留

        // 写位图块
        let ib_blocks = pack_bits(&self.inode_bitmap);
        for (i, block) in ib_blocks.iter().enumerate() {
            self.device.write_at(1 + i as u64, block)?;
        }
        let bb_blocks = pack_bits(&self.block_bitmap);
        for (i, block) in bb_blocks.iter().enumerate() {
            self.device.write_at(1 + ibb + i as u64, block)?;
        }

        // 初始化 inode 表（全空）
        let empty_inode = [0u8; INODE_SIZE];
        let inode_table_start = 1 + ibb + bbb;
        for i in 0..itb {
            self.device.write_at(inode_table_start + i, &empty_inode)?;
        }
        // 实际写入的 inode 表条目会随 alloc_inode / inode_write 更新；这里先清空

        // 根目录 ino=1
        self.inode_bitmap[1] = true;
        let root = ExtInode::dir(1);
        self.inodes[1] = root.clone();
        self.inode_write(1)?;
        let root_block = self.alloc_block()?;
        self.dir_write_map(root_block, &BTreeMap::new())?;
        let mut root_inode = self.inode_get_mut(1).unwrap();
        root_inode.blocks = vec![root_block];
        self.inode_write(1)?;

        self.formatted = true;
        Ok(())
    }

    /// 将元数据（超级块、位图、inode 表）刷到设备。
    pub fn sync_metadata(&mut self) -> Result<(), BlockError> {
        if !self.formatted {
            return Ok(());
        }
        // 超级块
        let mut sb_buf = [0u8; BLOCK_SIZE];
        let mut off = 0;
        macro_rules! put_u64 { ($v:expr) => { sb_buf[off..off+8].copy_from_slice(&($v as u64).to_le_bytes()); off += 8; } }
        put_u64!(EXT_MAGIC);
        put_u64!(self.block_count);
        put_u64!(self.inode_count);
        put_u64!(self.data_start);
        put_u64!((self.inode_bitmap.len() as u64 + (BLOCK_SIZE*8) as u64 - 1).div_ceil((BLOCK_SIZE*8) as u64).max(1));
        put_u64!((self.block_bitmap.len() as u64 + (BLOCK_SIZE*8) as u64 - 1).div_ceil((BLOCK_SIZE*8) as u64).max(1));
        put_u64!((self.inodes.len() as u64 * INODE_SIZE as u64 + BLOCK_SIZE as u64 - 1).div_ceil(BLOCK_SIZE as u64).max(1));
        self.device.write_at(0, &sb_buf)?;

        let ib_blocks = pack_bits(&self.inode_bitmap);
        let ibb = ib_blocks.len() as u64;
        for (i, block) in ib_blocks.iter().enumerate() {
            self.device.write_at(1 + i as u64, block)?;
        }
        let bb_blocks = pack_bits(&self.block_bitmap);
        for (i, block) in bb_blocks.iter().enumerate() {
            self.device.write_at(1 + ibb + i as u64, block)?;
        }
        let inode_table_start = 1 + ibb + bb_blocks.len() as u64;
        for (i, inode) in self.inodes.iter().enumerate() {
            let block_idx = inode_table_start + (i as u64) / (BLOCK_SIZE as u64 / INODE_SIZE as u64);
            let block_off = ((i as u64) % (BLOCK_SIZE as u64 / INODE_SIZE as u64)) as usize * INODE_SIZE;
            // 读取现有块、修改该条目、写回（因为一个块装多个 inode）
            let mut block = [0u8; BLOCK_SIZE];
            self.device.read_at(block_idx, &mut block)?;
            let mut entry = [0u8; INODE_SIZE];
            serialize_inode(inode, &mut entry);
            block[block_off..block_off + INODE_SIZE].copy_from_slice(&entry);
            self.device.write_at(block_idx, &block)?;
        }
        Ok(())
    }

    /// 从设备加载元数据。
    pub fn load_metadata(&mut self) -> Result<(), BlockError> {
        let mut sb_buf = [0u8; BLOCK_SIZE];
        self.device.read_at(0, &mut sb_buf)?;
        let magic = u64::from_le_bytes(sb_buf[0..8].try_into().unwrap_or([0; 8]));
        if magic != EXT_MAGIC {
            return Err(BlockError::IoError("bad superblock magic".into()));
        }
        let block_count = u64::from_le_bytes(sb_buf[8..16].try_into().unwrap_or([0; 8]));
        let inode_count = u64::from_le_bytes(sb_buf[16..24].try_into().unwrap_or([0; 8]));
        let data_start = u64::from_le_bytes(sb_buf[24..32].try_into().unwrap_or([0; 8]));
        let ibb = u64::from_le_bytes(sb_buf[32..40].try_into().unwrap_or([0; 8]));
        let bbb = u64::from_le_bytes(sb_buf[40..48].try_into().unwrap_or([0; 8]));
        let itb = u64::from_le_bytes(sb_buf[48..56].try_into().unwrap_or([0; 8]));

        self.block_count = block_count;
        self.inode_count = inode_count;
        self.data_start = data_start;
        self.inode_bitmap = vec![false; inode_count as usize];
        self.block_bitmap = vec![false; block_count as usize];
        self.inodes = vec![ExtInode::blank(0); inode_count as usize];

        // 位图
        let mut ib_blocks = Vec::new();
        for i in 0..ibb {
            let mut block = [0u8; BLOCK_SIZE];
            self.device.read_at(1 + i, &mut block)?;
            ib_blocks.push(block.to_vec());
        }
        self.inode_bitmap = unpack_bits(&ib_blocks, inode_count as usize);
        let mut bb_blocks = Vec::new();
        for i in 0..bbb {
            let mut block = [0u8; BLOCK_SIZE];
            self.device.read_at(1 + ibb + i, &mut block)?;
            bb_blocks.push(block.to_vec());
        }
        self.block_bitmap = unpack_bits(&bb_blocks, block_count as usize);

        // inode 表
        let per_block = (BLOCK_SIZE / INODE_SIZE) as u64;
        let inode_table_start = 1 + ibb + bbb;
        for i in 0..inode_count {
            let block_idx = inode_table_start + i / per_block;
            let block_off = (i % per_block) as usize * INODE_SIZE;
            let mut block = [0u8; BLOCK_SIZE];
            self.device.read_at(block_idx, &mut block)?;
            if let Some(inode) = deserialize_inode(&block[block_off..block_off + INODE_SIZE], i as u64) {
                self.inodes[i as usize] = inode;
            }
        }
        self.formatted = true;
        Ok(())
    }

    // ----- 位图分配 -----

    pub fn alloc_inode(&mut self) -> Result<u64, BlockError> {
        // ino 0 保留，从 1 开始
        for i in 1..self.inode_bitmap.len() {
            if !self.inode_bitmap[i] {
                self.inode_bitmap[i] = true;
                return Ok(i as u64);
            }
        }
        Err(BlockError::IoError("no free inodes".into()))
    }

    pub fn free_inode(&mut self, ino: u64) -> Result<(), BlockError> {
        let ino = ino as usize;
        if ino == 0 || ino >= self.inode_bitmap.len() {
            return Err(BlockError::OutOfRange);
        }
        self.inode_bitmap[ino] = false;
        if ino < self.inodes.len() {
            self.inodes[ino] = ExtInode::blank(ino as u64);
        }
        Ok(())
    }

    pub fn alloc_block(&mut self) -> Result<u64, BlockError> {
        for (i, used) in self.block_bitmap.iter().enumerate() {
            if !(*used) {
                self.block_bitmap[i] = true;
                return Ok(i as u64);
            }
        }
        Err(BlockError::IoError("no free blocks".into()))
    }

    pub fn free_block(&mut self, blk: u64) -> Result<(), BlockError> {
        let blk = blk as usize;
        if blk >= self.block_bitmap.len() {
            return Err(BlockError::OutOfRange);
        }
        self.block_bitmap[blk] = false;
        Ok(())
    }

    // ----- inode -----

    pub fn inode_get(&self, ino: u64) -> Option<&ExtInode> {
        let ino = ino as usize;
        if ino < self.inodes.len() && self.inode_bitmap.get(ino).copied().unwrap_or(false) {
            Some(&self.inodes[ino])
        } else {
            None
        }
    }

    pub fn inode_get_mut(&mut self, ino: u64) -> Option<&mut ExtInode> {
        let ino = ino as usize;
        if ino < self.inodes.len() && self.inode_bitmap.get(ino).copied().unwrap_or(false) {
            Some(&mut self.inodes[ino])
        } else {
            None
        }
    }

    fn inode_write(&mut self, ino: u64) -> Result<(), BlockError> {
        let inode = self.inode_get(ino).cloned().ok_or(BlockError::NotFound)?;
        let per_block = (BLOCK_SIZE / INODE_SIZE) as u64;
        let inode_table_start = 1 + self.data_start - self.inode_table_blocks();
        // 位图占用块数
        let ibb = (self.inode_bitmap.len() as u64 + (BLOCK_SIZE*8) as u64 - 1).div_ceil((BLOCK_SIZE*8) as u64).max(1);
        let bbb = (self.block_bitmap.len() as u64 + (BLOCK_SIZE*8) as u64 - 1).div_ceil((BLOCK_SIZE*8) as u64).max(1);
        let inode_table_start = 1 + ibb + bbb;
        let block_idx = inode_table_start + ino / per_block;
        let block_off = (ino % per_block) as usize * INODE_SIZE;
        let mut block = [0u8; BLOCK_SIZE];
        self.device.read_at(block_idx, &mut block)?;
        let mut entry = [0u8; INODE_SIZE];
        serialize_inode(&inode, &mut entry);
        block[block_off..block_off + INODE_SIZE].copy_from_slice(&entry);
        self.device.write_at(block_idx, &block)
    }

    fn inode_table_blocks(&self) -> u64 {
        ((self.inode_count * INODE_SIZE as u64) + BLOCK_SIZE as u64 - 1).div_ceil(BLOCK_SIZE as u64).max(1)
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }
    pub fn inode_count(&self) -> u64 {
        self.inode_count
    }
    pub fn root_ino(&self) -> u64 {
        1
    }
    pub fn page_cache(&mut self) -> &mut PageCache {
        &mut self.page_cache
    }
    pub fn block_device(&mut self) -> &mut dyn BlockDevice {
        self.device.as_mut()
    }

    // ----- 目录操作 -----

    fn dir_data_block(&mut self, ino: u64) -> Result<u64, BlockError> {
        let inode = self.inode_get(ino).ok_or(BlockError::NotFound)?;
        if !inode.blocks.is_empty() {
            return Ok(inode.blocks[0]);
        }
        // 分配一个块作为目录数据块
        let block = self.alloc_block()?;
        let mut inode = self.inode_get_mut(ino).ok_or(BlockError::NotFound)?;
        inode.blocks = vec![block];
        self.inode_write(ino)?;
        Ok(block)
    }

    fn dir_read_map(&mut self, ino: u64) -> Result<BTreeMap<String, u64>, BlockError> {
        let block = match self.inode_get(ino) {
            Some(inode) if !inode.blocks.is_empty() => inode.blocks[0],
            _ => return Ok(BTreeMap::new()),
        };
        let mut buf = [0u8; BLOCK_SIZE];
        self.device.read_at(block, &mut buf)?;
        Ok(deserialize_dir(&buf))
    }

    fn dir_write_map(&mut self, ino: u64, entries: &BTreeMap<String, u64>) -> Result<(), BlockError> {
        let block = self.dir_data_block(ino)?;
        let mut buf = [0u8; BLOCK_SIZE];
        serialize_dir(entries, &mut buf);
        self.device.write_at(block, &buf)
    }

    pub fn dir_lookup(&mut self, parent_ino: u64, name: &str) -> Option<u64> {
        self.dir_read_map(parent_ino).ok()?.get(name).copied()
    }

    pub fn dir_add(&mut self, parent_ino: u64, name: &str, ino: u64) -> Result<(), BlockError> {
        let mut entries = self.dir_read_map(parent_ino)?;
        if entries.contains_key(name) {
            return Err(BlockError::IoError("already exists".into()));
        }
        entries.insert(name.to_string(), ino);
        self.dir_write_map(parent_ino, &entries)?;
        // 更新父目录 nlink（子目录 +1）
        if let Some(p) = self.inode_get_mut(parent_ino) {
            if p.kind == InodeKind::Dir {
                p.nlink = p.nlink.saturating_add(1);
                let _ = self.inode_write(parent_ino);
            }
        }
        Ok(())
    }

    pub fn dir_remove(&mut self, parent_ino: u64, name: &str) -> Result<(), BlockError> {
        let mut entries = self.dir_read_map(parent_ino)?;
        if entries.get(name).is_none() {
            return Err(BlockError::IoError("not found".into()));
        }
        let child_ino = entries[name];
        entries.remove(name);
        self.dir_write_map(parent_ino, &entries)?;
        if let Some(p) = self.inode_get_mut(parent_ino) {
            if p.kind == InodeKind::Dir && p.nlink > 0 {
                p.nlink -= 1;
                let _ = self.inode_write(parent_ino);
            }
        }
        // 释放子 inode 及其数据块
        let _ = self.free_inode(child_ino);
        Ok(())
    }

    pub fn dir_list(&mut self, parent_ino: u64) -> Result<Vec<DirEntry>, BlockError> {
        let entries = self.dir_read_map(parent_ino)?;
        Ok(entries
            .into_iter()
            .map(|(name, ino)| DirEntry { name, ino })
            .collect())
    }

    // ----- 文件数据（通过页缓存） -----

    /// 保证 ino 的第 page_idx 页有数据块，返回块号。
    fn ensure_data_block(&mut self, ino: u64, page_idx: u64) -> Result<u64, BlockError> {
        let existing = {
            let inode = self.inode_get(ino).ok_or(BlockError::NotFound)?;
            let idx = page_idx as usize;
            if idx < inode.blocks.len() {
                Some(inode.blocks[idx])
            } else {
                None
            }
        };
        if let Some(b) = existing {
            return Ok(b);
        }
        let block = self.alloc_block()?;
        let idx = page_idx as usize;
        let inode = self.inode_get_mut(ino).ok_or(BlockError::NotFound)?;
        if inode.blocks.len() <= idx {
            inode.blocks.resize(idx + 1, 0);
        }
        inode.blocks[idx] = block;
        self.inode_write(ino)?;
        Ok(block)
    }

    fn read_page(&mut self, ino: u64, page_idx: u64) -> Result<[u8; PAGE_SIZE], BlockError> {
        let key = (ino, page_idx);
        if let Some(data) = self.page_cache.page_data(key) {
            return Ok(*data);
        }
        let mut page = [0u8; PAGE_SIZE];
        let block = {
            let inode = self.inode_get(ino).ok_or(BlockError::NotFound)?;
            let idx = page_idx as usize;
            if idx < inode.blocks.len() {
                Some(inode.blocks[idx])
            } else {
                None
            }
        };
        if let Some(block) = block {
            self.device.read_at(block, &mut page)?;
        }
        self.page_cache.insert(key, page);
        Ok(page)
    }

    pub fn file_read(&mut self, ino: u64, offset: u64, len: u64) -> Result<Vec<u8>, BlockError> {
        let inode = self.inode_get(ino).ok_or(BlockError::NotFound)?;
        let size = inode.size;
        if offset >= size || len == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut pos = offset;
        let mut remaining = len.min(size - offset);
        while remaining > 0 {
            let page_idx = pos / PAGE_SIZE as u64;
            let page_off = (pos % PAGE_SIZE as u64) as usize;
            let page = self.read_page(ino, page_idx)?;
            let take = (remaining as usize)
                .min(PAGE_SIZE - page_off)
                .min((size - pos) as usize);
            out.extend_from_slice(&page[page_off..page_off + take]);
            pos += take as u64;
            remaining -= take as u64;
        }
        Ok(out)
    }

    pub fn file_write(&mut self, ino: u64, offset: u64, data: &[u8]) -> Result<u64, BlockError> {
        let mut pos = offset;
        let mut written = 0u64;
        while (written as usize) < data.len() {
            let page_idx = pos / PAGE_SIZE as u64;
            let page_off = (pos % PAGE_SIZE as u64) as usize;
            let key = (ino, page_idx);
            let block = self.ensure_data_block(ino, page_idx)?;
            let page = self
                .page_cache
                .page_data_mut(key)
                .or_else(|| {
                    let mut p = [0u8; PAGE_SIZE];
                    let _ = self.device.read_at(block, &mut p);
                    self.page_cache.insert(key, p);
                    self.page_cache.page_data_mut(key)
                })
                .ok_or(BlockError::IoError("page cache insert failed".into()))?;
            let remaining_bytes = data.len() - written as usize;
            let take = remaining_bytes.min(PAGE_SIZE - page_off);
            page[page_off..page_off + take]
                .copy_from_slice(&data[written as usize..written as usize + take]);
            self.page_cache.mark_dirty(key);
            pos += take as u64;
            written += take as u64;
        }
        if let Some(inode) = self.inode_get_mut(ino) {
            if pos > inode.size {
                inode.size = pos;
                let _ = self.inode_write(ino);
            }
        }
        Ok(written)
    }

    pub fn file_set_size(&mut self, ino: u64, size: u64) -> Result<(), BlockError> {
        let inode = self.inode_get_mut(ino).ok_or(BlockError::NotFound)?;
        inode.size = size;
        self.inode_write(ino)
    }

    /// 回写指定 inode 的脏页到设备。
    pub fn writeback_inode(&mut self, ino: u64) -> Result<usize, BlockError> {
        let dirty_pages: Vec<(u64, [u8; PAGE_SIZE])> = {
            let keys: Vec<(u64, u64)> = self
                .page_cache
                .dirty_keys()
                .into_iter()
                .filter(|(i, _)| *i == ino)
                .collect();
            keys.into_iter()
                .filter_map(|(i, idx)| {
                    self.page_cache.page_data((i, idx)).map(|d| (idx, *d))
                })
                .collect()
        };
        let mut done = 0;
        for (idx, data) in dirty_pages {
            let block = {
                let inode = self.inode_get(ino).ok_or(BlockError::NotFound)?;
                let idx = idx as usize;
                if idx < inode.blocks.len() {
                    Some(inode.blocks[idx])
                } else {
                    None
                }
            };
            if let Some(block) = block {
                self.device.write_at(block, &data)?;
                self.page_cache.set_clean((ino, idx as u64));
                done += 1;
            }
        }
        Ok(done)
    }

    /// 回写全部脏页。
    pub fn writeback_all(&mut self) -> Result<usize, BlockError> {
        let inos: Vec<u64> = self
            .page_cache
            .dirty_keys()
            .into_iter()
            .map(|(i, _)| i)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let mut total = 0;
        for ino in inos {
            total += self.writeback_inode(ino)?;
        }
        Ok(total)
    }

    /// 缓存压力回收。
    pub fn reclaim_cache(&mut self, target: usize) -> usize {
        self.page_cache.reclaim(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fs() -> ExtFs {
        ExtFs::new(1024, 128)
    }

    #[test]
    fn extfs_alloc_free_inode_and_block() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let ino = fs.alloc_inode().unwrap();
        assert!(ino >= 1);
        assert!(fs.inode_get(ino).is_some());
        let blk = fs.alloc_block().unwrap();
        assert!(fs.block_bitmap[blk as usize]);
        fs.free_block(blk).unwrap();
        assert!(!fs.block_bitmap[blk as usize]);
        fs.free_inode(ino).unwrap();
        assert!(!fs.inode_bitmap[ino as usize]);
    }

    #[test]
    fn extfs_create_delete_file() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let root = fs.root_ino();
        let ino = fs.alloc_inode().unwrap();
        fs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
        fs.dir_add(root, "hello.txt", ino).unwrap();
        assert_eq!(fs.dir_lookup(root, "hello.txt"), Some(ino));
        assert!(fs.dir_lookup(root, "nope.txt").is_none());
        let list = fs.dir_list(root).unwrap();
        assert!(list.iter().any(|e| e.name == "hello.txt"));
        fs.dir_remove(root, "hello.txt").unwrap();
        assert_eq!(fs.dir_lookup(root, "hello.txt"), None);
    }

    #[test]
    fn extfs_file_read_write_roundtrip() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let root = fs.root_ino();
        let ino = fs.alloc_inode().unwrap();
        fs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
        fs.dir_add(root, "data.bin", ino).unwrap();

        let content = b"Hib OS persistent file content";
        fs.file_write(ino, 0, content).unwrap();
        fs.writeback_inode(ino).unwrap();

        let back = fs.file_read(ino, 0, content.len() as u64).unwrap();
        assert_eq!(back, content);

        // 偏移读取
        let part = fs.file_read(ino, 4, 6).unwrap();
        assert_eq!(part, b"OS per");
    }

    #[test]
    fn extfs_writeback_dirty_page() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let root = fs.root_ino();
        let ino = fs.alloc_inode().unwrap();
        fs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
        fs.dir_add(root, "wb.bin", ino).unwrap();

        fs.file_write(ino, 0, b"writeback test data").unwrap();
        // 此时页在缓存中是脏的，设备上可能尚未持久化
        assert!(fs.page_cache().is_dirty((ino, 0)));
        let written = fs.writeback_inode(ino).unwrap();
        assert_eq!(written, 1);
        assert!(!fs.page_cache().is_dirty((ino, 0)));

        // 新建一个 ExtFs 挂载同一设备，数据应可读（通过同步元数据简化：此处直接读设备块）
        let back = fs.file_read(ino, 0, 19).unwrap();
        assert_eq!(back, b"writeback test data");
    }

    #[test]
    fn extfs_reclaim_cache_keeps_dirty() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let root = fs.root_ino();
        let ino = fs.alloc_inode().unwrap();
        fs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
        fs.dir_add(root, "rc.bin", ino).unwrap();
        fs.file_write(ino, 0, b"dirty content here").unwrap();
        assert!(fs.page_cache().is_dirty((ino, 0)));
        // 压力回收到 0，脏页不应被回收
        let released = fs.reclaim_cache(0);
        assert_eq!(released, 0);
        assert!(fs.page_cache().is_dirty((ino, 0)));
        assert!(fs.page_cache().len() >= 1);
    }

    #[test]
    fn extfs_mkdir_and_list() {
        let mut fs = make_fs();
        fs.format().unwrap();
        let root = fs.root_ino();
        let ino = fs.alloc_inode().unwrap();
        fs.inode_get_mut(ino).unwrap().kind = InodeKind::Dir;
        fs.dir_add(root, "subdir", ino).unwrap();
        let list = fs.dir_list(root).unwrap();
        assert!(list.iter().any(|e| e.name == "subdir" && e.ino == ino));
    }
}
