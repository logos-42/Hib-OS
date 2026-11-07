//! 网卡资产表 —— AI 原生的「网络硬件视图」。
//!
//! `net://eth0` 是**协议栈视角**（发包、ARP、路由），
//! `net://interfaces` 是**网卡资产视角**（有几张卡、各自 IP/掩码/MTU/链路状态）。
//! 两者通过 eth0 联动：在这里改 eth0 的地址，协议栈立刻跟着变。
//!
//! * `sense net://interfaces`
//! * `act net://interfaces up|down <网卡>`
//! * `act net://interfaces addr <网卡> <ip>/<前缀>`
//! * `act net://interfaces mtu <网卡> <68-65535>`
//! * `act net://interfaces add <网卡> [mac]` / `del <网卡>`

use std::sync::{Mutex, OnceLock};

use crate::net::{fmt_ip, net, parse_ip, ETH_NAME};

/// 一张网卡。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetIface {
    pub name: String,
    pub mac: String,
    /// 点分十进制 IPv4 地址。
    pub ip: String,
    /// 前缀长度 (0-32)，等价于子网掩码。
    pub prefix: u8,
    pub mtu: u32,
    pub up: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl NetIface {
    /// 单行渲染。
    pub fn render(&self) -> String {
        format!(
            "{:<8} {:<5} ip={}/{} mac={} mtu={} rx={}B tx={}B",
            self.name,
            if self.up { "up" } else { "down" },
            self.ip,
            self.prefix,
            self.mac,
            self.mtu,
            self.rx_bytes,
            self.tx_bytes
        )
    }
}

/// 网卡表。
pub struct IfaceTable {
    ifaces: Vec<NetIface>,
}

impl IfaceTable {
    fn new() -> Self {
        let mk = |name: &str, mac: &str, ip: &str, prefix: u8, mtu: u32, up: bool| NetIface {
            name: name.to_string(),
            mac: mac.to_string(),
            ip: ip.to_string(),
            prefix,
            mtu,
            up,
            rx_bytes: 0,
            tx_bytes: 0,
        };
        Self {
            ifaces: vec![
                mk("lo", "00:00:00:00:00:00", "127.0.0.1", 8, 65536, true),
                mk(ETH_NAME, "02:00:00:00:00:01", "10.0.0.2", 24, 1500, true),
                mk("wlan0", "02:00:00:00:00:02", "0.0.0.0", 0, 1500, false),
            ],
        }
    }

    /// 全部网卡渲染为多行文本。
    pub fn list(&self) -> String {
        self.ifaces
            .iter()
            .map(NetIface::render)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn len(&self) -> usize {
        self.ifaces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ifaces.is_empty()
    }

    fn find_mut(&mut self, name: &str) -> Result<&mut NetIface, String> {
        self.ifaces
            .iter_mut()
            .find(|i| i.name == name)
            .ok_or_else(|| format!("网卡 {} 不存在", name))
    }

    /// 启通 / 断开链路，并同步到协议栈。
    pub fn set_link(&mut self, name: &str, up: bool) -> Result<String, String> {
        let iface = self.find_mut(name)?;
        iface.up = up;
        if name == ETH_NAME {
            net().lock().unwrap().iface.link_up = up;
        }
        Ok(format!(
            "{} 链路{}",
            name,
            if up { "已启通" } else { "已断开" }
        ))
    }

    /// 设置地址（形如 `10.0.0.5/24`）。
    pub fn set_addr(&mut self, name: &str, cidr: &str) -> Result<String, String> {
        let (ip, prefix) = parse_cidr(cidr)?;
        let iface = self.find_mut(name)?;
        iface.ip = fmt_ip(ip);
        iface.prefix = prefix;
        // eth0 与真实协议栈联动，ping/路由立即使用新地址。
        if name == ETH_NAME {
            let mut st = net().lock().unwrap();
            st.iface.ip = ip;
            st.iface.mask = mask_from_prefix(prefix);
        }
        Ok(format!("{} 地址已设为 {}/{}", name, fmt_ip(ip), prefix))
    }

    /// 设置 MTU（68..=65535）。
    pub fn set_mtu(&mut self, name: &str, mtu: u32) -> Result<String, String> {
        if !(68..=65535).contains(&mtu) {
            return Err(format!("MTU 超出范围 (68-65535): {}", mtu));
        }
        let iface = self.find_mut(name)?;
        iface.mtu = mtu;
        if name == ETH_NAME {
            net().lock().unwrap().iface.mtu = mtu as u16;
        }
        Ok(format!("{} MTU 已设为 {}", name, mtu))
    }

    /// 添加网卡（默认 down、0.0.0.0/0）。
    pub fn add(&mut self, name: &str, mac: &str) -> Result<String, String> {
        if self.ifaces.iter().any(|i| i.name == name) {
            return Err(format!("网卡 {} 已存在", name));
        }
        if mac.split(':').count() != 6 {
            return Err(format!("MAC 格式应为 xx:xx:xx:xx:xx:xx: {}", mac));
        }
        self.ifaces.push(NetIface {
            name: name.to_string(),
            mac: mac.to_string(),
            ip: "0.0.0.0".to_string(),
            prefix: 0,
            mtu: 1500,
            up: false,
            rx_bytes: 0,
            tx_bytes: 0,
        });
        Ok(format!("已添加网卡 {} ({})", name, mac))
    }

    /// 删除网卡；lo 不允许删除。
    pub fn del(&mut self, name: &str) -> Result<String, String> {
        if name == "lo" {
            return Err("不能删除回环网卡 lo".to_string());
        }
        let pos = self
            .ifaces
            .iter()
            .position(|i| i.name == name)
            .ok_or_else(|| format!("网卡 {} 不存在", name))?;
        self.ifaces.remove(pos);
        Ok(format!("已删除网卡 {}", name))
    }
}

/// 解析 `ip/prefix`；不合法返回错误。
fn parse_cidr(s: &str) -> Result<([u8; 4], u8), String> {
    let (ip_s, prefix_s) = s
        .split_once('/')
        .ok_or_else(|| format!("地址格式应为 <ip>/<前缀>: {}", s))?;
    let parts: Vec<&str> = ip_s.split('.').collect();
    if parts.len() != 4 {
        return Err(format!("无效 IPv4 地址: {}", ip_s));
    }
    for p in &parts {
        p.parse::<u8>()
            .map_err(|_| format!("无效 IPv4 地址: {}", ip_s))?;
    }
    let prefix: u8 = prefix_s
        .parse()
        .map_err(|_| format!("无效前缀: {}", prefix_s))?;
    if prefix > 32 {
        return Err(format!("前缀超出范围 (0-32): {}", prefix));
    }
    Ok((parse_ip(ip_s), prefix))
}

/// 前缀长度 -> 子网掩码。
fn mask_from_prefix(prefix: u8) -> [u8; 4] {
    let bits = prefix.min(32) as u32;
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    mask.to_be_bytes()
}

static IFACES: OnceLock<Mutex<IfaceTable>> = OnceLock::new();

/// 全局网卡资产表（`net://interfaces` 的实体）。
pub fn ifaces() -> &'static Mutex<IfaceTable> {
    IFACES.get_or_init(|| Mutex::new(IfaceTable::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> IfaceTable {
        IfaceTable::new()
    }

    /// 取出某张网卡的渲染行，避免依赖列宽对齐。
    fn row(list: &str, name: &str) -> String {
        list.lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn defaults_list_three_nics() {
        let t = table();
        assert_eq!(t.len(), 3);
        let out = t.list();
        assert!(out.contains("ip=127.0.0.1/8"), "{}", out);
        assert!(out.contains("ip=10.0.0.2/24"), "{}", out);
        assert!(row(&out, "lo").contains("up"), "{}", out);
        assert!(row(&out, "eth0").contains("up"), "{}", out);
        assert!(row(&out, "wlan0").contains("down"), "{}", out);
    }

    #[test]
    fn link_toggles() {
        let mut t = table();
        let r = t.set_link("wlan0", true).unwrap();
        assert!(r.contains("已启通"), "{}", r);
        assert!(row(&t.list(), "wlan0").contains("up"), "{}", t.list());
        t.set_link("wlan0", false).unwrap();
        assert!(row(&t.list(), "wlan0").contains("down"), "{}", t.list());
        assert!(t.set_link("nope0", true).is_err());
    }

    #[test]
    fn addr_and_mtu_validation() {
        let mut t = table();
        let r = t.set_addr("wlan0", "192.168.1.20/24").unwrap();
        assert!(r.contains("192.168.1.20/24"), "{}", r);
        assert!(t.set_addr("wlan0", "192.168.1.20").is_err());
        assert!(t.set_addr("wlan0", "999.1.1.1/24").is_err());
        assert!(t.set_addr("wlan0", "10.0.0.1/33").is_err());
        assert!(t.set_mtu("wlan0", 9000).is_ok());
        assert!(t.set_mtu("wlan0", 10).is_err());
        assert!(t.set_mtu("wlan0", 70000).is_err());
    }

    #[test]
    fn eth0_addr_syncs_protocol_stack() {
        let mut t = table();
        t.set_addr(ETH_NAME, "10.0.0.9/24").unwrap();
        assert_eq!(net().lock().unwrap().iface.ip, [10, 0, 0, 9]);
        assert_eq!(net().lock().unwrap().iface.mask, [255, 255, 255, 0]);
        t.set_link(ETH_NAME, true).unwrap();
        assert!(net().lock().unwrap().iface.link_up);
    }

    #[test]
    fn add_and_del_guards() {
        let mut t = table();
        assert!(t.add("eth1", "02:00:00:00:00:11").is_ok());
        assert_eq!(t.len(), 4);
        assert!(t.add("eth1", "02:00:00:00:00:11").is_err());
        assert!(t.add("bad", "not-a-mac").is_err());
        assert!(t.del("lo").is_err());
        assert!(t.del("eth1").is_ok());
        assert!(t.del("eth1").is_err());
    }

    #[test]
    fn mask_from_prefix_works() {
        assert_eq!(mask_from_prefix(0), [0, 0, 0, 0]);
        assert_eq!(mask_from_prefix(8), [255, 0, 0, 0]);
        assert_eq!(mask_from_prefix(24), [255, 255, 255, 0]);
        assert_eq!(mask_from_prefix(32), [255, 255, 255, 255]);
    }
}
