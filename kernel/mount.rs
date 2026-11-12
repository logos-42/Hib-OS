//! 挂载表 —— AI 原生的「文件系统骨架」。
//!
//! 类比 Linux 的 `/proc/mounts`：每一行描述「哪个源被挂到哪个目标、什么类型、
//! 只读还是可写」。这里不实现真实块设备，只把挂载关系建模成可感知 / 可设置的
//! 通道 `mount://table`。
//!
//! * `sense mount://table`            —— 列出全部挂载点
//! * `act mount://table mount <源> <目标> [类型] [ro|rw]`
//! * `act mount://table umount <目标>`
//! * `act mount://table remount <目标> <ro|rw>`

use std::sync::{Mutex, OnceLock};

/// 一个挂载点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    /// 挂载源（设备名 / 伪文件系统名）。
    pub src: String,
    /// 挂载目标（挂载点路径）。
    pub target: String,
    /// 文件系统类型。
    pub fstype: String,
    /// 是否只读。
    pub ro: bool,
    /// 额外挂载选项。
    pub options: Vec<String>,
}

impl MountEntry {
    /// 单行渲染：`/ <- /dev/disk (hibfs) rw opts=[]`。
    pub fn render(&self) -> String {
        format!(
            "{:<10} <- {:<12} ({}) {} opts=[{}]",
            self.target,
            self.src,
            self.fstype,
            if self.ro { "ro" } else { "rw" },
            self.options.join(",")
        )
    }
}

/// 挂载表。
pub struct MountTable {
    entries: Vec<MountEntry>,
}

impl MountTable {
    /// 构造初始挂载表：根文件系统 + tmpfs + procfs + sysfs + devfs。
    fn new() -> Self {
        let mk = |src: &str, target: &str, fstype: &str, ro: bool| MountEntry {
            src: src.to_string(),
            target: target.to_string(),
            fstype: fstype.to_string(),
            ro,
            options: Vec::new(),
        };
        Self {
            entries: vec![
                mk("/dev/disk", "/", "hibfs", false),
                mk("tmpfs", "/tmp", "tmpfs", false),
                mk("proc", "/proc", "procfs", false),
                mk("sysfs", "/sys", "sysfs", false),
                mk("gpu0", "/dev/gpu", "devfs", false),
            ],
        }
    }

    /// 全部挂载点渲染为多行文本（按挂载顺序）。
    pub fn list(&self) -> String {
        self.entries
            .iter()
            .map(MountEntry::render)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 按挂载点查找。
    pub fn find(&self, target: &str) -> Option<&MountEntry> {
        self.entries.iter().find(|e| e.target == target)
    }

    /// 全部挂载点副本（供 VFS 联动遍历）。
    pub fn targets(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|e| (e.target.clone(), e.fstype.clone()))
            .collect()
    }

    /// 挂载点数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn find_mut(&mut self, target: &str) -> Option<&mut MountEntry> {
        self.entries.iter_mut().find(|e| e.target == target)
    }

    /// 新增挂载；目标已被占用则报错。
    pub fn mount(
        &mut self,
        src: &str,
        target: &str,
        fstype: &str,
        ro: bool,
    ) -> Result<String, String> {
        if !target.starts_with('/') {
            return Err(format!("挂载点必须是绝对路径: {}", target));
        }
        if self.entries.iter().any(|e| e.target == target) {
            return Err(format!("{} 已被挂载", target));
        }
        let entry = MountEntry {
            src: src.to_string(),
            target: target.to_string(),
            fstype: fstype.to_string(),
            ro,
            options: Vec::new(),
        };
        let line = entry.render();
        self.entries.push(entry);
        Ok(format!("已挂载 {}", line))
    }

    /// 卸载；根目录不允许卸载。
    pub fn umount(&mut self, target: &str) -> Result<String, String> {
        if target == "/" {
            return Err("不能卸载根文件系统 /".to_string());
        }
        let pos = self
            .entries
            .iter()
            .position(|e| e.target == target)
            .ok_or_else(|| format!("{} 未被挂载", target))?;
        let entry = self.entries.remove(pos);
        Ok(format!("已卸载 {} ({})", entry.target, entry.src))
    }

    /// 重新挂载以切换只读 / 读写。
    pub fn remount(&mut self, target: &str, ro: bool) -> Result<String, String> {
        let entry = self
            .find_mut(target)
            .ok_or_else(|| format!("{} 未被挂载", target))?;
        entry.ro = ro;
        Ok(format!(
            "已重新挂载 {} -> {}",
            target,
            if ro { "ro" } else { "rw" }
        ))
    }
}

static MOUNTS: OnceLock<Mutex<MountTable>> = OnceLock::new();

/// 全局挂载表（`mount://table` 的实体）。
pub fn mounts() -> &'static Mutex<MountTable> {
    MOUNTS.get_or_init(|| Mutex::new(MountTable::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> MountTable {
        MountTable::new()
    }

    #[test]
    fn defaults_cover_root_proc_sys() {
        let t = table();
        assert_eq!(t.len(), 5);
        let out = t.list();
        assert!(
            out.lines()
                .any(|l| l.contains("/dev/disk") && l.contains("(hibfs) rw")),
            "{}",
            out
        );
        assert!(out.contains("(procfs)"), "{}", out);
        assert!(out.contains("(sysfs)"), "{}", out);
    }

    #[test]
    fn mount_then_duplicate_rejected() {
        let mut t = table();
        let r = t.mount("tmpfs", "/data", "tmpfs", false).unwrap();
        assert!(r.contains("/data"), "{}", r);
        assert_eq!(t.len(), 6);
        assert!(t.mount("tmpfs", "/data", "tmpfs", false).is_err());
    }

    #[test]
    fn mount_requires_absolute_target() {
        let mut t = table();
        assert!(t.mount("tmpfs", "data", "tmpfs", false).is_err());
    }

    #[test]
    fn umount_root_rejected_and_missing_reported() {
        let mut t = table();
        assert!(t.umount("/").is_err());
        assert!(t.umount("/nope").is_err());
        t.mount("tmpfs", "/data", "tmpfs", false).unwrap();
        let r = t.umount("/data").unwrap();
        assert!(r.contains("/data"), "{}", r);
        assert_eq!(t.len(), 5);
        assert!(t.umount("/data").is_err());
    }

    #[test]
    fn remount_flips_readonly_flag() {
        let mut t = table();
        t.remount("/", true).unwrap();
        assert!(t.list().contains("(hibfs) ro"), "{}", t.list());
        t.remount("/", false).unwrap();
        assert!(t.list().contains("(hibfs) rw"), "{}", t.list());
        assert!(t.remount("/nope", true).is_err());
    }
}
