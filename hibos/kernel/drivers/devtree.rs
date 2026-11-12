//! 设备树 —— P1-2 子模块。

use alloc::collections::BTreeMap; use alloc::string::String; use alloc::vec::Vec;
use crate::ksync::{SpinLock, OnceLock};

/// 设备树节点。
#[derive(Clone, Debug)]
pub struct DeviceNode {
    pub name: String,
    pub path: String,
    pub properties: BTreeMap<String, String>,
    pub children: Vec<String>, // 子节点路径
}

/// 设备树。
pub struct DeviceTree {
    nodes: BTreeMap<String, DeviceNode>,
}

static DEV_TREE: OnceLock<SpinLock<DeviceTree>> = OnceLock::new();

fn dev_tree() -> &'static SpinLock<DeviceTree> {
    DEV_TREE.get_or_init(|| SpinLock::new(DeviceTree {
        nodes: BTreeMap::new(),
    }))
}

/// 插入设备节点。
pub fn devtree_insert(name: &str, path: &str, properties: BTreeMap<String, String>) -> Result<(), String> {
    let mut g = dev_tree().lock();
    if g.nodes.contains_key(path) {
        return Err(format!("节点已存在: {}", path));
    }
    let node = DeviceNode {
        name: name.to_string(),
        path: path.to_string(),
        properties,
        children: Vec::new(),
    };
    // 更新父节点的 children
    if let Some(parent_path) = path.rsplit_once('/').map(|(p, _)| p.to_string()) {
        if let Some(parent) = g.nodes.get_mut(&parent_path) {
            parent.children.push(path.to_string());
        }
    }
    g.nodes.insert(path.to_string(), node);
    Ok(())
}

/// 查找设备节点。
pub fn devtree_lookup(path: &str) -> Option<DeviceNode> {
    let g = dev_tree().lock().unwrap();
    g.nodes.get(path).cloned()
}

/// 列出所有节点路径。
pub fn devtree_paths() -> Vec<String> {
    let g = dev_tree().lock().unwrap();
    g.nodes.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn insert_and_lookup() {
        let mut props = BTreeMap::new();
        props.insert("compatible".to_string(), "nvme".to_string());
        assert!(devtree_insert("nvme0", "/pci/nvme0", props).is_ok());
        let node = devtree_lookup("/pci/nvme0").unwrap();
        assert_eq!(node.name, "nvme0");
    }

    #[test]
    fn insert_duplicate() {
        let props = BTreeMap::new();
        assert!(devtree_insert("x", "/a", props.clone()).is_ok());
        assert!(devtree_insert("y", "/a", props).is_err());
    }
}
