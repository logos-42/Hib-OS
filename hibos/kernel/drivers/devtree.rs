//! 设备树 —— `dev://tree` 通道后端（复用 kernel/dev.rs 的 DevTree）。
//!
//! 维护设备层级树，类似 Linux devicetree。

use crate::dev::{devtree, DevNode};

/// 挂接设备到设备树。
pub fn attach_devtree(name: &str) -> Result<String, String> {
    devtree().lock().unwrap().attach(name)
}

/// 从设备树卸下设备。
pub fn detach_devtree(name: &str) -> Result<String, String> {
    devtree().lock().unwrap().detach(name)
}

/// 列出设备树所有节点（供 `sense dev://tree` 使用）。
pub fn devtree_list() -> String {
    devtree().lock().unwrap().list()
}

/// 设备树节点快照。
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub name: String,
    pub major: u32,
    pub minor: u32,
    pub kind: String,
    pub driver: String,
    pub state: String,
}

impl TreeNode {
    pub fn from(dev: &DevNode) -> Self {
        TreeNode { name: dev.name.clone(), major: dev.major, minor: dev.minor, kind: dev.kind.clone(), driver: dev.driver.clone(), state: dev.state.clone() }
    }
}

/// 列出所有树节点。
pub fn tree_nodes() -> Vec<TreeNode> {
    devtree().lock().unwrap().nodes.iter().map(TreeNode::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtree_list_nonempty() {
        let list = devtree_list();
        assert!(!list.is_empty());
        assert!(list.contains("gpu0"));
        assert!(list.contains("timer"));
    }

    #[test]
    fn test_attach_devtree() {
        // 先 detach 再 attach，验证状态迁移
        let _ = detach_devtree("tty");
        let r = attach_devtree("tty");
        assert!(r.is_ok());
        let list = devtree_list();
        assert!(list.contains("tty"));
    }

    #[test]
    fn test_detach_devtree() {
        let r = detach_devtree("console");
        assert!(r.is_ok());
        let list = devtree_list();
        // detached 设备仍在列表中，只是 state 变为 detached
        assert!(list.contains("console"));
        let nodes = tree_nodes();
        let c = nodes.iter().find(|n| n.name == "console").unwrap();
        assert_eq!(c.state, "detached");
    }
}
