//! 挂载点管理 —— P0-3 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 挂载点：源路径 -> 目标路径。
#[derive(Clone, Debug)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub fs_type: String,
    pub readonly: bool,
}

/// 全局挂载表。
struct MountTable {
    mounts: BTreeMap<String, Mount>,
}

static MOUNT_TABLE: OnceLock<Mutex<MountTable>> = OnceLock::new();

fn mount_table() -> &'static Mutex<MountTable> {
    MOUNT_TABLE.get_or_init(|| Mutex::new(MountTable {
        mounts: BTreeMap::new(),
    }))
}

/// 挂载文件系统。
pub fn mount(source: &str, target: &str, fs_type: &str, readonly: bool) -> Result<(), String> {
    let mut g = mount_table().lock().unwrap();
    if g.mounts.contains_key(target) {
        return Err(format!("挂载点已存在: {}", target));
    }
    g.mounts.insert(
        target.to_string(),
        Mount {
            source: source.to_string(),
            target: target.to_string(),
            fs_type: fs_type.to_string(),
            readonly,
        },
    );
    Ok(())
}

/// 卸载文件系统。
pub fn umount(target: &str) -> bool {
    let mut g = mount_table().lock().unwrap();
    g.mounts.remove(target).is_some()
}

/// 查询挂载点。
pub fn lookup_mount(target: &str) -> Option<Mount> {
    let g = mount_table().lock().unwrap();
    g.mounts.get(target).cloned()
}

/// 列出所有挂载点。
pub fn list_mounts() -> Vec<Mount> {
    let g = mount_table().lock().unwrap();
    g.mounts.values().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_and_umount() {
        assert!(mount("/dev/sda1", "/mnt/data", "ext4", false).is_ok());
        assert!(lookup_mount("/mnt/data").is_some());
        assert!(umount("/mnt/data"));
        assert!(lookup_mount("/mnt/data").is_none());
    }

    #[test]
    fn mount_duplicate_target() {
        mount("/dev/sda1", "/mnt", "ext4", false).unwrap();
        assert!(mount("/dev/sdb1", "/mnt", "ext4", false).is_err());
    }

    #[test]
    fn list_mounts_works() {
        mount("/dev/sda1", "/mnt/a", "ext4", false).unwrap();
        mount("/dev/sdb1", "/mnt/b", "xfs", true).unwrap();
        let mounts = list_mounts();
        assert!(mounts.len() >= 2);
    }
}
