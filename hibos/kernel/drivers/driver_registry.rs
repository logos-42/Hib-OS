//! 驱动注册表 —— P1-2 子模块。

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::drivers::{Driver, DriverState};

/// 驱动注册表。
struct DriverRegistryState {
    drivers: BTreeMap<String, Driver>,
}

static DRV_REG: OnceLock<Mutex<DriverRegistryState>> = OnceLock::new();

fn drv_reg() -> &'static Mutex<DriverRegistryState> {
    DRV_REG.get_or_init(|| Mutex::new(DriverRegistryState {
        drivers: BTreeMap::new(),
    }))
}

/// 注册驱动。
pub fn driver_register(driver: Driver) -> Result<(), String> {
    let mut g = drv_reg().lock().unwrap();
    if g.drivers.contains_key(&driver.name) {
        return Err(format!("驱动已存在: {}", driver.name));
    }
    g.drivers.insert(driver.name.clone(), driver);
    Ok(())
}

/// 查找驱动。
pub fn driver_lookup(name: &str) -> Option<Driver> {
    let g = drv_reg().lock().unwrap();
    g.drivers.get(name).cloned()
}

/// 绑定驱动到设备（标记为 Active）。
pub fn driver_bind(name: &str) -> Result<(), String> {
    let mut g = drv_reg().lock().unwrap();
    match g.drivers.get_mut(name) {
        Some(d) => {
            d.state = DriverState::Active;
            Ok(())
        }
        None => Err(format!("驱动不存在: {}", name)),
    }
}

/// 解绑驱动（标记为 Unloaded）。
pub fn driver_unbind(name: &str) -> Result<(), String> {
    let mut g = drv_reg().lock().unwrap();
    match g.drivers.get_mut(name) {
        Some(d) => {
            d.state = DriverState::Unloaded;
            Ok(())
        }
        None => Err(format!("驱动不存在: {}", name)),
    }
}

/// 列出所有驱动。
pub fn driver_list() -> Vec<Driver> {
    let g = drv_reg().lock().unwrap();
    g.drivers.values().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::Driver;

    #[test]
    fn register_and_bind() {
        let d = Driver::new("nvme", "1.0");
        assert!(driver_register(d).is_ok());
        assert!(driver_bind("nvme").is_ok());
        let d = driver_lookup("nvme").unwrap();
        assert_eq!(d.state, DriverState::Active);
        assert!(driver_unbind("nvme").is_ok());
    }

    #[test]
    fn register_duplicate() {
        let d1 = Driver::new("x", "1");
        let d2 = Driver::new("x", "2");
        assert!(driver_register(d1).is_ok());
        assert!(driver_register(d2).is_err());
    }
}
