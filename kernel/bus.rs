//! 设备总线 —— AI 原生的「热插拔 / 驱动绑定」视图。
//!
//! `dev/tree` 管的是**设备节点**（/dev 下的字符块设备、主次设备号），
//! `dev/bus` 管的是**总线枚举**（PCI 地址、厂商/设备号、哪个驱动接管）。
//! 两者互补，共同构成设备管理能力。
//!
//! * `sense dev/bus`
//! * `act dev/bus bind <地址> <驱动>` / `unbind <地址>`
//! * `act dev/bus rescan`（模拟总线重新扫描）
//! * `act dev/bus add <地址> <vendor:device> <类别> [驱动]` / `remove <地址>`

use std::sync::{Mutex, OnceLock};

/// 总线上的一个设备。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusDevice {
    /// PCI 地址，形如 `00:02.0`。
    pub addr: String,
    pub vendor: u16,
    pub device: u16,
    /// 设备类别：display / network / storage / system / console ...
    pub class: String,
    /// 已绑定驱动；None 表示未绑定。
    pub driver: Option<String>,
}

impl BusDevice {
    /// 单行渲染：`00:02.0 10de:2204 display driver=gpu`。
    pub fn render(&self) -> String {
        format!(
            "{:<8} {:04x}:{:04x} {:<8} driver={}",
            self.addr,
            self.vendor,
            self.device,
            self.class,
            self.driver.as_deref().unwrap_or("(none)")
        )
    }
}

/// 总线枚举表。
pub struct Bus {
    devices: Vec<BusDevice>,
    /// 扫描代号，每次 `rescan` 自增。
    pub generation: u64,
}

impl Bus {
    fn new() -> Self {
        let mk = |addr: &str, vendor: u16, device: u16, class: &str, driver: Option<&str>| {
            BusDevice {
                addr: addr.to_string(),
                vendor,
                device,
                class: class.to_string(),
                driver: driver.map(|d| d.to_string()),
            }
        };
        Self {
            devices: vec![
                mk("00:00.0", 0x8086, 0x1237, "system", Some("iommu")),
                mk("00:02.0", 0x10de, 0x2204, "display", Some("gpu")),
                mk("00:03.0", 0x1af4, 0x1000, "network", Some("virtio-net")),
                mk("00:04.0", 0x1af4, 0x1001, "storage", Some("vfs")),
                // 出厂未绑定驱动，供 `bind` 演示。
                mk("00:05.0", 0x1af4, 0x1003, "console", None),
            ],
            generation: 1,
        }
    }

    /// 全部设备渲染为多行文本。
    pub fn list(&self) -> String {
        self.devices
            .iter()
            .map(BusDevice::render)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn len(&self) -> usize {
        self.devices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 未绑定驱动的设备数量。
    pub fn unbound(&self) -> usize {
        self.devices.iter().filter(|d| d.driver.is_none()).count()
    }

    fn find_mut(&mut self, addr: &str) -> Result<&mut BusDevice, String> {
        self.devices
            .iter_mut()
            .find(|d| d.addr == addr)
            .ok_or_else(|| format!("总线上无设备 {}", addr))
    }

    /// 绑定驱动。
    pub fn bind(&mut self, addr: &str, driver: &str) -> Result<String, String> {
        if driver.trim().is_empty() {
            return Err("驱动名不能为空".to_string());
        }
        let dev = self.find_mut(addr)?;
        if let Some(cur) = &dev.driver {
            return Err(format!("设备 {} 已绑定驱动 {}", addr, cur));
        }
        dev.driver = Some(driver.to_string());
        Ok(format!("设备 {} 已绑定驱动 {}", addr, driver))
    }

    /// 解绑驱动。
    pub fn unbind(&mut self, addr: &str) -> Result<String, String> {
        let dev = self.find_mut(addr)?;
        match dev.driver.take() {
            Some(old) => Ok(format!("设备 {} 已解绑驱动 {}", addr, old)),
            None => Err(format!("设备 {} 未绑定驱动", addr)),
        }
    }

    /// 重新扫描总线（模拟热插拔后的枚举）。
    pub fn rescan(&mut self) -> Result<String, String> {
        self.generation += 1;
        Ok(format!(
            "总线扫描第 {} 轮: 设备 {} 个, 未绑定驱动 {} 个",
            self.generation,
            self.devices.len(),
            self.unbound()
        ))
    }

    /// 添加设备（如热插入的网卡 / 显卡）。
    pub fn add(
        &mut self,
        addr: &str,
        id: &str,
        class: &str,
        driver: Option<&str>,
    ) -> Result<String, String> {
        let (vendor, device) = parse_id(id)?;
        if self.devices.iter().any(|d| d.addr == addr) {
            return Err(format!("总线地址 {} 已被占用", addr));
        }
        self.devices.push(BusDevice {
            addr: addr.to_string(),
            vendor,
            device,
            class: class.to_string(),
            driver: driver.map(|d| d.to_string()),
        });
        Ok(format!(
            "已热插入设备 {} {:04x}:{:04x} {}",
            addr, vendor, device, class
        ))
    }

    /// 移除设备（热拔出）。
    pub fn remove(&mut self, addr: &str) -> Result<String, String> {
        let pos = self
            .devices
            .iter()
            .position(|d| d.addr == addr)
            .ok_or_else(|| format!("总线上无设备 {}", addr))?;
        self.devices.remove(pos);
        Ok(format!("已热拔出设备 {}", addr))
    }
}

/// 解析 `vendor:device` 十六进制标识。
fn parse_id(s: &str) -> Result<(u16, u16), String> {
    let (v, d) = s
        .split_once(':')
        .ok_or_else(|| format!("设备标识应为 <vendor:device> 十六进制: {}", s))?;
    let vendor = u16::from_str_radix(v.trim_start_matches("0x"), 16)
        .map_err(|_| format!("无效厂商号: {}", v))?;
    let device = u16::from_str_radix(d.trim_start_matches("0x"), 16)
        .map_err(|_| format!("无效设备号: {}", d))?;
    Ok((vendor, device))
}

static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();

/// 全局设备总线（`dev/bus` 的实体）。
pub fn bus() -> &'static Mutex<Bus> {
    BUS.get_or_init(|| Mutex::new(Bus::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus_new() -> Bus {
        Bus::new()
    }

    #[test]
    fn defaults_have_unbound_device() {
        let b = bus_new();
        assert_eq!(b.len(), 5);
        assert_eq!(b.unbound(), 1);
        let out = b.list();
        assert!(
            out.lines()
                .any(|l| l.contains("10de:2204") && l.contains("driver=gpu")),
            "{}",
            out
        );
        assert!(
            out.lines()
                .any(|l| l.contains("1af4:1003") && l.contains("driver=(none)")),
            "{}",
            out
        );
    }

    #[test]
    fn bind_and_unbind() {
        let mut b = bus_new();
        assert!(b.bind("00:05.0", "console").is_ok());
        assert_eq!(b.unbound(), 0);
        assert!(b.bind("00:05.0", "again").is_err());
        assert!(b.bind("00:99.0", "x").is_err());
        assert!(b.bind("00:02.0", "").is_err());
        assert!(b.unbind("00:05.0").is_ok());
        assert!(b.unbind("00:05.0").is_err());
    }

    #[test]
    fn rescan_bumps_generation() {
        let mut b = bus_new();
        assert_eq!(b.generation, 1);
        let r = b.rescan().unwrap();
        assert!(r.contains("第 2 轮"), "{}", r);
        assert!(r.contains("未绑定驱动 1 个"), "{}", r);
        assert_eq!(b.generation, 2);
    }

    #[test]
    fn hotplug_add_remove() {
        let mut b = bus_new();
        assert!(b.add("00:06.0", "10de:1eb8", "display", None).is_ok());
        assert_eq!(b.len(), 6);
        assert!(b.add("00:06.0", "10de:1eb8", "display", None).is_err());
        assert!(b.add("00:07.0", "bad", "x", None).is_err());
        assert!(b.remove("00:06.0").is_ok());
        assert!(b.remove("00:06.0").is_err());
    }

    #[test]
    fn id_parsing() {
        assert_eq!(parse_id("10de:2204").unwrap(), (0x10de, 0x2204));
        assert_eq!(parse_id("0x10de:0x2204").unwrap(), (0x10de, 0x2204));
        assert!(parse_id("10de").is_err());
        assert!(parse_id("zz:2204").is_err());
    }
}
