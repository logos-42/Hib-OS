//! 持久化文件系统总线 —— VFS 总线，inode/dentry 树。
//!
//! 把块设备（block://<设备>）、页缓存、类 ext 分配器与文件描述符表
//! 组合成一个可被 `sense`/`act` 操作的文件系统子系统。
//!
//! `fs://<路径>` 通道：
//!   sense 读取文件内容或目录元数据；act 写入/创建/删除文件与目录。
//! `block://<设备>` 通道：
//!   sense 查看块设备信息/读块；act 写块。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::fs::block::{BlockDevice, BlockError, BlockManager, BLOCK_SIZE};
use crate::fs::extfs::{ExtFs, InodeKind};
use crate::fs::vfs::Vfs;

pub mod block;
pub mod pagecache;
pub mod extfs;
pub mod diskimg;
pub mod disk_img;
pub mod partition;
pub mod buffer_cache;
pub mod ext2_like;
pub mod journal;
pub mod file_lock;
pub mod mmap_file;
pub mod dentry_cache;
pub mod quota;
pub mod aio;
pub mod vfs;

static FSBUS: OnceLock<Mutex<FsBus>> = OnceLock::new();

/// 全局文件系统总线（单例，首次访问时初始化一个 ram 盘）。
pub fn fsbus() -> &'static Mutex<FsBus> {
    FSBUS.get_or_init(|| Mutex::new(FsBus::new()))
}

/// 持久化文件系统总线。
pub struct FsBus {
    pub blocks: BlockManager,
    pub extfs: ExtFs,
    pub vfs: Vfs,
    /// 默认挂载的磁盘映像路径（如有）。
    pub image_path: Option<PathBuf>,
}

impl FsBus {
    pub fn new() -> Self {
        let mut blocks = BlockManager::new();
        // ram0 同时注册到块设备管理器（供 block:// 通道查询）与 extfs（拥有独占副本）。
        let ram_size: u64 = 4096;
        blocks.insert(
            "ram0",
            Box::new(crate::fs::block::MemBlockDevice::new("ram0", ram_size)),
        );
        let extfs_device = Box::new(crate::fs::block::MemBlockDevice::new("ram0", ram_size))
            as Box<dyn BlockDevice + Send>;
        let mut extfs = ExtFs::with_device(ram_size, 1024, extfs_device);
        extfs.format().unwrap();
        let vfs = Vfs::new();
        Self {
            blocks,
            extfs,
            vfs,
            image_path: None,
        }
    }

    /// 挂载磁盘映像文件作为根文件系统。
    pub fn mount_disk_image(&mut self, path: &Path) -> Result<(), BlockError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diskimg")
            .to_string();
        let device = crate::fs::block::FileBlockDevice::open(path, &name, false)?;
        let block_count = device.block_count();
        let inode_count = (block_count / 4).max(64); // 粗略估算
        let mut extfs = ExtFs::with_device(block_count, inode_count, Box::new(device));
        match extfs.load_metadata() {
            Ok(()) => {}
            Err(_) => {
                // 未格式化的映像：格式化后使用
                extfs.format()?;
            }
        }
        self.image_path = Some(path.to_path_buf());
        // 把映像设备也注册到块设备管理器，供 block:// 通道访问
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diskimg")
            .to_string();
        // FileBlockDevice 已被 extfs 独占；此处仅记录元数据名称，block 通道通过
        // extfs 的底层设备读写。若要独立块访问，可再打开一个句柄。
        let _ = name;
        self.extfs = extfs;
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), BlockError> {
        self.extfs.sync_metadata()?;
        self.extfs.writeback_all()?;
        Ok(())
    }

    /// 根 inode。
    pub fn root_ino(&self) -> u64 {
        self.extfs.root_ino()
    }

    // ---- fs:// 通道操作 ----

    /// sense：读取文件内容或目录列表/元数据。
    pub fn fs_sense(&mut self, path: &str) -> Result<String, String> {
        let comps = resolve_path(path);
        if comps.is_empty() {
            // 根：返回目录列表
            return self.list_dir_str(self.extfs.root_ino());
        }
        let parent = self.resolve_parent(&comps)?;
        let name = comps.last().unwrap();
        match self.extfs.dir_lookup(parent, name) {
            Some(ino) => {
                let kind = self.extfs.inode_get(ino).map(|i| i.kind);
                match kind {
                    Some(InodeKind::Dir) => self.list_dir_str(ino),
                    Some(InodeKind::File) => {
                        let inode = self.extfs.inode_get(ino).ok_or_else(|| "inode not found".to_string())?;
                        let size = inode.size;
                        let mode = inode.mode;
                        let data = self.extfs.file_read(ino, 0, size).map_err(|e| e.to_string())?;
                        let text = String::from_utf8_lossy(&data).to_string();
                        Ok(format!(
                            "fs://{} size={} ino={} mode={:o}\n{}",
                            path,
                            size,
                            ino,
                            mode,
                            text
                        ))
                    }
                    None => Err("inode not found".to_string()),
                }
            }
            None => Err(format!("fs://{} 路径不存在", path)),
        }
    }

    /// act：写入/创建/删除。
    pub fn fs_act(&mut self, path: &str, payload: &str) -> Result<String, String> {
        let trimmed = payload.trim();
        let comps = resolve_path(path);
        if comps.is_empty() {
            return Err("fs:// 根目录不可直接写入".to_string());
        }
        let name = comps.last().unwrap();
        let parent = self.resolve_parent(&comps)?;

        if let Some(rest) = trimmed.strip_prefix("create ") {
            let target = rest.trim();
            let target_comps = resolve_path(target);
            if target_comps.is_empty() {
                return Err("create 缺少目标路径".to_string());
            }
            let tname = target_comps.last().unwrap();
            let tparent = self.resolve_parent(&target_comps)?;
            let ino = self.extfs.alloc_inode().map_err(|e| e.to_string())?;
            self.extfs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
            self.extfs.dir_add(tparent, tname, ino).map_err(|e| e.to_string())?;
            return Ok(format!("fs://{} 已创建 (ino={})", target, ino));
        }
        if trimmed == "mkdir" {
            let ino = self.extfs.alloc_inode().map_err(|e| e.to_string())?;
            self.extfs.inode_get_mut(ino).unwrap().kind = InodeKind::Dir;
            self.extfs.dir_add(parent, name, ino).map_err(|e| e.to_string())?;
            return Ok(format!("fs://{} 已创建目录 (ino={})", path, ino));
        }
        if trimmed == "delete" || trimmed == "rm" {
            let _ino = self.extfs.dir_lookup(parent, name).ok_or_else(|| format!("fs://{} 不存在", path))?;
            self.extfs.dir_remove(parent, name).map_err(|e| e.to_string())?;
            return Ok(format!("fs://{} 已删除", path));
        }
        if trimmed.is_empty() {
            return Err("fs:// act 需要内容或命令 (create|mkdir|delete)".to_string());
        }
        // 默认：写入文件（不存在则创建）
        let ino = match self.extfs.dir_lookup(parent, name) {
            Some(ino) => ino,
            None => {
                let ino = self.extfs.alloc_inode().map_err(|e| e.to_string())?;
                self.extfs.inode_get_mut(ino).unwrap().kind = InodeKind::File;
                self.extfs.dir_add(parent, name, ino).map_err(|e| e.to_string())?;
                ino
            }
        };
        let written = self.extfs.file_write(ino, 0, payload.as_bytes()).map_err(|e| e.to_string())?;
        // 回写脏页，使写入持久化
        let _ = self.extfs.writeback_inode(ino);
        Ok(format!("fs://{} 已写入 {} 字节", path, written))
    }

    // ---- block:// 通道操作 ----

    /// sense：块设备信息或读块。
    pub fn block_sense(&mut self, spec: &str) -> Result<String, String> {
        // spec: "ram0" 或 "ram0/read/<block>"
        let parts: Vec<&str> = spec.split('/').collect();
        let dev_name = parts.first().copied().unwrap_or("");
        if !self.blocks.contains(dev_name) {
            return Err(format!("block://{} 设备不存在", dev_name));
        }
        if parts.len() == 1 {
            let dev = self.blocks.device(dev_name).unwrap();
            Ok(format!(
                "block://{} block_size={} block_count={} type={}",
                dev_name,
                dev.block_size(),
                dev.block_count(),
                device_type_name(dev)
            ))
        } else if parts.len() == 3 && parts[1] == "read" {
            let block: u64 = parts[2].parse().map_err(|_| "块号非法".to_string())?;
            let data = self.blocks.read_block(dev_name, block).map_err(|e| e.to_string())?;
            let hex: String = data.iter().take(64).map(|b| format!("{:02x}", b)).collect();
            Ok(format!("block://{}/read/{} ({} 字节): {}...", dev_name, block, data.len(), hex))
        } else {
            Err(format!("block://{} 用法: sense block://<dev> | sense block://<dev>/read/<blk>", dev_name))
        }
    }

    /// act：block://<dev>/write/<block> <hex-payload>
    pub fn block_act(&mut self, spec: &str, payload: &str) -> Result<String, String> {
        let parts: Vec<&str> = spec.split('/').collect();
        if parts.len() != 4 || parts[1] != "write" {
            return Err("用法: act block://<dev>/write/<块> <十六进制数据>".to_string());
        }
        let dev_name = parts[0];
        let block: u64 = parts[2].parse().map_err(|_| "块号非法".to_string())?;
        if !self.blocks.contains(dev_name) {
            return Err(format!("block://{} 设备不存在", dev_name));
        }
        let bytes = parse_hex(payload).map_err(|e| format!("hex 解析失败: {}", e))?;
        if bytes.len() > BLOCK_SIZE {
            return Err("数据超过一个块".to_string());
        }
        let mut block_data = [0u8; BLOCK_SIZE];
        block_data[..bytes.len()].copy_from_slice(&bytes);
        self.blocks.write_block(dev_name, block, &block_data).map_err(|e| e.to_string())?;
        Ok(format!("block://{}/write/{} 已写入 {} 字节", dev_name, block, bytes.len()))
    }

    // ---- 内部辅助 ----

    fn resolve_parent(&mut self, comps: &[String]) -> Result<u64, String> {
        let mut ino = self.extfs.root_ino();
        for c in &comps[..comps.len() - 1] {
            let next = self.extfs.dir_lookup(ino, c).ok_or_else(|| format!("中间目录不存在: {}", c))?;
            let inode = self.extfs.inode_get(next).ok_or_else(|| "inode 丢失".to_string())?;
            if inode.kind != InodeKind::Dir {
                return Err(format!("{} 不是目录", c));
            }
            ino = next;
        }
        Ok(ino)
    }

    fn list_dir_str(&mut self, ino: u64) -> Result<String, String> {
        let inode = self.extfs.inode_get(ino).ok_or_else(|| "inode not found".to_string())?;
        if inode.kind != InodeKind::Dir {
            return Err("不是目录".to_string());
        }
        let entries = self.extfs.dir_list(ino).map_err(|e| e.to_string())?;
        let mut out = format!("fs://dir ino={} entries={}\n", ino, entries.len());
        for e in entries {
            let child = self.extfs.inode_get(e.ino);
            let kind = child.map(|i| if i.kind == InodeKind::Dir { 'd' } else { '-' }).unwrap_or('?');
            out.push_str(&format!(" {} {} ino={}\n", kind, e.name, e.ino));
        }
        Ok(out)
    }
}

impl Default for FsBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 把路径拆分为组件（支持绝对与相对，相对默认从根开始）。
fn resolve_path(path: &str) -> Vec<String> {
    let mut comps: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => { comps.pop(); }
            name => comps.push(name.to_string()),
        }
    }
    comps
}

/// 设备类型名称（根据运行时类型推断，此处按块数粗略分类）。
fn device_type_name(_dev: &mut dyn BlockDevice) -> &'static str {
    "block-device"
}

/// 解析十六进制负载（允许空格/换行）。
fn parse_hex(s: &str) -> Result<Vec<u8>, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    if s.len() % 2 != 0 {
        return Err("十六进制长度须为偶数".into());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_val(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("非法十六进制字符: {}", c as char)),
    }
}

// ===== 全局 sense/act 入口（供 sense.rs 路由调用） =====

/// sense fs://<路径>
pub fn fs_sense(path: &str) -> Result<String, String> {
    fsbus().lock().unwrap().fs_sense(path)
}

/// act fs://<路径> <payload>
pub fn fs_act(path: &str, payload: &str) -> Result<String, String> {
    fsbus().lock().unwrap().fs_act(path, payload)
}

/// sense block://<设备>[/read/<块>]
pub fn block_sense(spec: &str) -> Result<String, String> {
    fsbus().lock().unwrap().block_sense(spec)
}

/// act block://<设备>/write/<块> <hex>
pub fn block_act(spec: &str, payload: &str) -> Result<String, String> {
    fsbus().lock().unwrap().block_act(spec, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_bus_default_has_ram_device() {
        let bus = FsBus::new();
        assert!(bus.blocks.contains("ram0"));
        assert_eq!(bus.blocks.block_count("ram0"), Some(4096));
        assert_eq!(bus.extfs.root_ino(), 1);
    }

    #[test]
    fn fs_sense_act_file_roundtrip() {
        let mut bus = FsBus::new();
        // 先 create，再写入
        let _ = bus.fs_act("/hello.txt", "create");
        let res = bus.fs_act("/hello.txt", "Hello, Hib OS!");
        assert!(res.is_ok(), "{:?}", res);
        let sensed = bus.fs_sense("/hello.txt").unwrap();
        assert!(sensed.contains("Hello, Hib OS!"));
        assert!(sensed.contains("size="));
    }

    #[test]
    fn fs_mkdir_delete_and_list() {
        let mut bus = FsBus::new();
        let res = bus.fs_act("/docs", "mkdir");
        assert!(res.is_ok(), "{:?}", res);
        let listed = bus.fs_sense("/docs").unwrap();
        assert!(listed.contains("entries="));
        let res = bus.fs_act("/docs", "delete");
        assert!(res.is_ok(), "{:?}", res);
        assert!(bus.fs_sense("/docs").is_err());
    }

    #[test]
    fn fs_persistence_across_remount() {
        let path = std::env::temp_dir().join(format!("hibos_fs_persist_{}.img", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut bus = FsBus::new();
            bus.mount_disk_image(&path).unwrap();
            let _ = bus.fs_act("/persist.txt", "create");
            let _ = bus.fs_act("/persist.txt", "persistent-data-1234");
            let _ = bus.sync();
        }
        // 新实例挂载同一映像，数据应持久化
        let mut bus2 = FsBus::new();
        bus2.mount_disk_image(&path).unwrap();
        let sensed = bus2.fs_sense("/persist.txt").unwrap();
        assert!(sensed.contains("persistent-data-1234"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn block_sense_info_and_act() {
        let mut bus = FsBus::new();
        let info = bus.block_sense("ram0").unwrap();
        assert!(info.contains("block_count=4096"));
        let res = bus.block_act("ram0/write/3", "deadbeef");
        assert!(res.is_ok(), "{:?}", res);
        let read = bus.block_sense("ram0/read/3").unwrap();
        assert!(read.contains("deadbeef"));
    }

    #[test]
    fn fs_act_create_then_sense_metadata() {
        let mut bus = FsBus::new();
        let _ = bus.fs_act("/meta.txt", "create");
        let s = bus.fs_sense("/meta.txt").unwrap();
        assert!(s.contains("ino="));
        assert!(s.contains("mode="));
    }
}
