//! 设备模型 —— AI 原生的「硬件感官」。
//!
//! 主次设备号 / 字符块设备 / 驱动状态，统一为可感知的 `dev/tree` 通道。

use std::sync::{Mutex, OnceLock};

pub struct DevNode {
    pub name: String,
    pub major: u32,
    pub minor: u32,
    pub kind: String,
    pub driver: String,
    pub state: String,
}

pub struct DevTree {
    pub nodes: Vec<DevNode>,
}

impl DevTree {
    fn new() -> Self {
        let mk = |name: &str, major: u32, minor: u32, kind: &str, driver: &str| DevNode {
            name: name.to_string(),
            major,
            minor,
            kind: kind.to_string(),
            driver: driver.to_string(),
            state: "attached".to_string(),
        };
        Self {
            nodes: vec![
                mk("gpu0", 195, 0, "char", "gpu"),
                mk("disk", 8, 0, "block", "vfs"),
                mk("null", 1, 3, "char", "misc"),
                mk("zero", 1, 5, "char", "misc"),
                mk("tty", 4, 0, "char", "console"),
                mk("console", 5, 1, "char", "console"),
                mk("timer", 0, 0, "char", "clock"),
                mk("iommu0", 0, 1, "char", "iommu"),
            ],
        }
    }

    pub fn list(&self) -> String {
        self.nodes
            .iter()
            .map(|n| {
                format!(
                    "{:<8} major={} minor={} kind={} driver={} {}",
                    n.name, n.major, n.minor, n.kind, n.driver, n.state
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn attach(&mut self, name: &str) -> Result<String, String> {
        match self.nodes.iter_mut().find(|n| n.name == name) {
            Some(n) => {
                n.state = "attached".to_string();
                Ok(format!("设备 {} 已接入", name))
            }
            None => Err(format!("设备 {} 不存在", name)),
        }
    }

    pub fn detach(&mut self, name: &str) -> Result<String, String> {
        match self.nodes.iter_mut().find(|n| n.name == name) {
            Some(n) => {
                n.state = "detached".to_string();
                Ok(format!("设备 {} 已卸下", name))
            }
            None => Err(format!("设备 {} 不存在", name)),
        }
    }
}

static DEVTREE: OnceLock<Mutex<DevTree>> = OnceLock::new();

pub fn devtree() -> &'static Mutex<DevTree> {
    DEVTREE.get_or_init(|| Mutex::new(DevTree::new()))
}
