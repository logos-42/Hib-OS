//! 设备注册表与驱动绑定 —— `dev://drivers` 通道后端。
//!
//! 全局设备表，设备可绑定/解绑驱动，驱动支持热插拔。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// 驱动接口：每个驱动实现 init/probe/remove 三个方法。
pub trait Driver: Send + Sync {
    fn name(&self) -> &str;
    fn init(&self) -> bool;
    fn probe(&self, device: &str) -> bool;
    fn remove(&self, device: &str) -> bool;
}

/// 设备表项。
#[derive(Clone, Debug)]
pub struct DeviceEntry {
    pub name: String,
    pub driver: Option<String>,
    pub state: String,
    pub pci_addr: Option<String>,
}

impl DeviceEntry {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), driver: None, state: "unbound".to_string(), pci_addr: None }
    }
}

/// 设备注册表。
pub struct DeviceRegistry {
    devices: BTreeMap<String, DeviceEntry>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self { devices: BTreeMap::new() }
    }

    pub fn register(&mut self, name: &str) {
        self.devices.entry(name.to_string()).or_insert_with(|| DeviceEntry::new(name));
    }

    pub fn bind_driver(&mut self, device: &str, driver: &str) -> bool {
        match self.devices.get_mut(device) {
            Some(e) => {
                e.driver = Some(driver.to_string());
                e.state = "bound".to_string();
                true
            }
            None => false,
        }
    }

    pub fn unbind_driver(&mut self, device: &str) -> bool {
        match self.devices.get_mut(device) {
            Some(e) => {
                e.driver = None;
                e.state = "unbound".to_string();
                true
            }
            None => false,
        }
    }

    pub fn list(&self) -> Vec<DeviceEntry> {
        self.devices.values().cloned().collect()
    }

    pub fn find(&self, name: &str) -> Option<DeviceEntry> {
        self.devices.get(name).cloned()
    }
}

static DEVICE_REGISTRY: OnceLock<Mutex<DeviceRegistry>> = OnceLock::new();

fn device_registry() -> &'static Mutex<DeviceRegistry> {
    DEVICE_REGISTRY.get_or_init(|| Mutex::new(DeviceRegistry::new()))
}

/// 注册一个设备。
pub fn register_device(name: &str) {
    device_registry().lock().unwrap().register(name);
}

/// 将驱动绑定到设备。
pub fn bind_driver(device: &str, driver: &str) -> bool {
    device_registry().lock().unwrap().bind_driver(device, driver)
}

/// 解绑设备的驱动。
pub fn unbind_driver(device: &str) -> bool {
    device_registry().lock().unwrap().unbind_driver(device)
}

/// 列出所有设备的快照字符串（供 `sense dev://drivers` 使用）。
pub fn drivers_list() -> String {
    let reg = device_registry().lock().unwrap();
    if reg.list().is_empty() {
        return "（无设备）".to_string();
    }
    reg.list()
        .iter()
        .map(|e| {
            format!(
                "{} driver={:?} state={} pci={}",
                e.name,
                e.driver,
                e.state,
                e.pci_addr.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 执行一次驱动探针（不持久化绑定，仅返回探测结果）。
pub fn probe_driver<T: Driver>(driver: &T, device: &str) -> bool {
    driver.probe(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullDriver;
    impl Driver for NullDriver {
        fn name(&self) -> &str {
            "null"
        }
        fn init(&self) -> bool {
            true
        }
        fn probe(&self, _device: &str) -> bool {
            true
        }
        fn remove(&self, _device: &str) -> bool {
            true
        }
    }

    #[test]
    fn register_and_list() {
        register_device("disk0");
        let list = drivers_list();
        assert!(list.contains("disk0"));
        assert!(list.contains("state=unbound"));
        assert!(list.contains("driver=None"));
    }

    #[test]
    fn bind_and_unbind_driver() {
        register_device("net0");
        assert!(bind_driver("net0", "e1000"));
        let e = device_registry().lock().unwrap().find("net0").unwrap();
        assert_eq!(e.driver.as_deref(), Some("e1000"));
        assert_eq!(e.state, "bound");

        assert!(unbind_driver("net0"));
        let e = device_registry().lock().unwrap().find("net0").unwrap();
        assert_eq!(e.driver, None);
        assert_eq!(e.state, "unbound");

        // 绑定/解绑不存在的设备返回 false
        assert!(!bind_driver("nope", "e1000"));
        assert!(!unbind_driver("nope"));
    }

    #[test]
    fn driver_init_probe_remove() {
        let d = NullDriver;
        assert!(d.init());
        assert!(d.probe("disk0"));
        assert!(d.remove("disk0"));
        assert_eq!(d.name(), "null");
        // probe_driver 封装
        assert!(probe_driver(&d, "net0"));
        assert!(!probe_driver(&d, "net0") == false);
    }
}
