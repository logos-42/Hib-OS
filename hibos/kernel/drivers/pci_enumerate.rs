//! PCI 配置空间扫描（kernel/drivers/pci_enumerate.rs）。
//!
//! 本模块实现 PCI 总线枚举与配置空间读写：扫描总线/设备/功能、读取 256 字节
//! 标准配置空间头与扩展能力、BAR（基地址寄存器）大小探测、中断行、命令/状态寄存器。
//! 与现有 `kernel/drivers/pci.rs` 轻量枚举并存，面向更完整的 PCI 配置空间模型。
//!
//! 特性:
//! * PciBus：scan_bus / read_config / write_config / device_by_class
//! * PciDevice：厂商 ID、设备 ID、类代码、子类、编程接口、BAR 空间、中断行、命令/状态寄存器
//! * PCI 配置空间读写（256 字节标准头 + 扩展能力）
//! * 设备枚举（总线/设备/功能）、BAR 大小探测
//! * sense/act 通道：`drv://pci`

use alloc::collections::BTreeMap; use alloc::vec::Vec; use alloc::boxed::Box; use alloc::string::String; use alloc::string::ToString;
use crate::ksync::{SpinLock, OnceLock};

// =======================================================================
// PCI 常量
// ======================================================================

/// PCI 配置空间标准头大小（字节）。
pub const PCI_CONFIG_SPACE_SIZE: usize = 256;
/// PCI 厂商 ID 无效（无设备）。
pub const PCI_VENDOR_ID_NONE: u16 = 0xFFFF;

/// PCI 基类代码。
pub const PCI_CLASS_UNCLASSIFIED: u8 = 0x00;
pub const PCI_CLASS_STORAGE: u8 = 0x01;
pub const PCI_CLASS_NETWORK: u8 = 0x02;
pub const PCI_CLASS_DISPLAY: u8 = 0x03;
pub const PCI_CLASS_MULTIMEDIA: u8 = 0x04;
pub const PCI_CLASS_BRIDGE: u8 = 0x06;
pub const PCI_CLASS_SERIAL_BUS: u8 = 0x0C;

// 配置空间偏移（字节）。
const PCI_OFFSET_VENDOR_ID: usize = 0x00;
const PCI_OFFSET_DEVICE_ID: usize = 0x02;
const PCI_OFFSET_COMMAND: usize = 0x04;
const PCI_OFFSET_STATUS: usize = 0x06;
const PCI_OFFSET_REVISION_ID: usize = 0x08;
const PCI_OFFSET_PROG_IF: usize = 0x09;
const PCI_OFFSET_SUBCLASS: usize = 0x0A;
const PCI_OFFSET_CLASS_CODE: usize = 0x0B;
const PCI_OFFSET_HEADER_TYPE: usize = 0x0E;
const PCI_OFFSET_BAR0: usize = 0x10;
const PCI_OFFSET_BAR_COUNT: usize = 6;
const PCI_OFFSET_BAR_SPACING: usize = 4;
const PCI_OFFSET_CAPABILITY_PTR: usize = 0x34;
const PCI_OFFSET_INTERRUPT_LINE: usize = 0x3C;

// PCI 命令寄存器位。
const PCI_COMMAND_IO_SPACE: u16 = 0x0001;
const PCI_COMMAND_MEMORY_SPACE: u16 = 0x0002;
const PCI_COMMAND_BUS_MASTER: u16 = 0x0004;

// BAR 位。
const BAR_TYPE_MASK: u64 = 0x1;
const BAR_TYPE_IO: u64 = 0x1;
const BAR_TYPE_MEM_32: u64 = 0x0;
const BAR_TYPE_MEM_64: u64 = 0x2; // bit 1:2 = 10b

// =======================================================================
// BAR 描述符
// ======================================================================

/// BAR 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarType {
    Io,
    Memory32,
    Memory64,
    Unknown,
}

/// BAR 描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub index: u8,
    pub bar_type: BarType,
    /// 配置空间中读出的原始值。
    pub raw: u32,
    /// 探测出的大小（字节），0 表示未探测/不可探测。
    pub size: u64,
    /// 探测出的基址。
    pub base: u64,
    /// 是否可预取（仅内存 BAR）。
    pub prefetchable: bool,
}

// =======================================================================
// PCI 设备
// ======================================================================

/// PCI 设备描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision_id: u8,
    pub irq_line: u8,
    pub command: u16,
    pub status: u16,
    pub bars: [Bar; PCI_OFFSET_BAR_COUNT],
    /// 能力链首指针（配置空间偏移）。
    pub cap_ptr: u8,
    /// 是否多功能设备。
    pub multi_function: bool,
}

impl PciDevice {
    /// BDF 字符串：<bus>:<device>.<function>。
    pub fn bdf(&self) -> String {
        format!("{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }

    /// 通道名：`drv://pci/<bus>:<device>.<function>`。
    pub fn channel(&self) -> String {
        format!("drv://pci/{}", self.bdf())
    }

    /// 设备 ID 字符串：<vendor>:<device>。
    pub fn id_str(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor_id, self.device_id)
    }

    /// 是否有效设备（厂商 ID 非 0xFFFF）。
    pub fn is_valid(&self) -> bool { self.vendor_id != PCI_VENDOR_ID_NONE }

    /// 是否启用总线主控。
    pub fn bus_master_enabled(&self) -> bool { self.command & PCI_COMMAND_BUS_MASTER != 0 }

    /// 是否启用 I/O 空间。
    pub fn io_space_enabled(&self) -> bool { self.command & PCI_COMMAND_IO_SPACE != 0 }

    /// 查找指定索引的 BAR。
    pub fn bar(&self, index: u8) -> Option<&Bar> {
        if index < PCI_OFFSET_BAR_COUNT as u8 { Some(&self.bars[index as usize]) } else { None }
    }
}

// =======================================================================
// PCI 配置空间读写后端
// ======================================================================

/// PCI 配置空间读写后端（原型：模拟读写；真实硬件接入时替换为 0xCF8/0xCFC 端口或 MMIO ECAM）。
pub trait PciConfigOps: Send + Sync {
    /// 读取一个 32-bit 配置字。
    fn read_config(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32;
    /// 写入一个 32-bit 配置字。
    fn write_config(&mut self, bus: u8, device: u8, function: u8, offset: u8, val: u32);
}

/// 默认模拟配置空间：未写入的项读回 0xFFFFFFFF（模拟真实硬件无设备时的 0xFFFF）。
#[derive(Clone, Debug, Default)]
pub struct PciDefaultConfig {
    /// 缓存的配置字：(bus, device, function, offset_aligned) -> u32。
    pub space: BTreeMap<(u8, u8, u8, u8), u32>,
}

impl PciDefaultConfig {
    pub fn new() -> Self { Self { space: BTreeMap::new() } }

    /// 注册一个模拟设备的配置空间字（offset 必须 4 字节对齐）。
    pub fn insert(&mut self, bus: u8, device: u8, function: u8, offset: u8, val: u32) {
        self.space.insert((bus, device, function, offset & 0xFC), val);
    }
}

impl PciConfigOps for PciDefaultConfig {
    fn read_config(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        let o = offset & 0xFC;
        self.space.get(&(bus, device, function, o)).copied().unwrap_or(0xFFFFFFFF)
    }
    fn write_config(&mut self, bus: u8, device: u8, function: u8, offset: u8, val: u32) {
        let o = offset & 0xFC;
        self.space.insert((bus, device, function, o), val);
    }
}

// =======================================================================
// PCI 总线
// ======================================================================

/// PCI 总线：维护已枚举的设备，提供配置空间读写与枚举。
pub struct PciBus {
    /// 配置空间读写后端。
    pub config: Box<dyn PciConfigOps>,
    /// 已枚举设备：BDF -> PciDevice。
    pub devices: BTreeMap<String, PciDevice>,
}

impl PciBus {
    /// 创建一个使用默认模拟配置空间的 PCI 总线。
    pub fn new() -> Self {
        Self { config: Box::new(PciDefaultConfig::new()), devices: BTreeMap::new() }
    }

    /// 使用指定配置空间后端创建。
    pub fn with_config(config: Box<dyn PciConfigOps>) -> Self {
        Self { config, devices: BTreeMap::new() }
    }

    /// 全局 PCI 总线实例（`drv://pci` 通道后端）。
    pub fn bus() -> &'static SpinLock<PciBus> {
        static BUS: OnceLock<SpinLock<PciBus>> = OnceLock::new();
        BUS.get_or_init(|| SpinLock::new(Self::new()))
    }

    // ------ 配置空间读写 ------

    /// 读取指定设备配置空间一个 32-bit 字（offset 必须 4 字节对齐，0..256）。
    pub fn read_config(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        if offset as usize >= PCI_CONFIG_SPACE_SIZE { return 0xFFFFFFFF; }
        self.config.read_config(bus, device, function, offset & 0xFC)
    }

    /// 写入指定设备配置空间一个 32-bit 字。
    pub fn write_config(&mut self, bus: u8, device: u8, function: u8, offset: u8, val: u32) {
        if offset as usize >= PCI_CONFIG_SPACE_SIZE { return; }
        self.config.write_config(bus, device, function, offset & 0xFC, val);
    }

    /// 读取 16-bit 字（little-endian：offset 字节起始的低 16 位）。
    pub fn read_config16(&self, bus: u8, device: u8, function: u8, offset: u8) -> u16 {
        let v = self.read_config(bus, device, function, offset & 0xFC);
        let shift = ((offset as u32 & 0x2) * 8) as u32;
        ((v >> shift) & 0xFFFF) as u16
    }

    /// 读取 8-bit 字节。
    pub fn read_config8(&self, bus: u8, device: u8, function: u8, offset: u8) -> u8 {
        let v = self.read_config(bus, device, function, offset & 0xFC);
        let shift = ((offset as u32 & 0x3) * 8) as u32;
        ((v >> shift) & 0xFF) as u8
    }

    // ------ 设备探测 ------

    /// 探测指定 BDF 是否存在设备（读厂商 ID != 0xFFFF）。
    fn device_present(&self, bus: u8, device: u8, function: u8) -> bool {
        self.read_config16(bus, device, function, PCI_OFFSET_VENDOR_ID as u8) != PCI_VENDOR_ID_NONE
    }

    /// 探测一个 BAR 的大小：向 BAR 写全 1 再读回，掩码出可变位。
    fn probe_bar_size(&mut self, bus: u8, device: u8, function: u8, bar_offset: usize) -> u64 {
        let offset = (bar_offset as u8) & 0xFC;
        let orig = self.read_config(bus, device, function, offset);
        self.write_config(bus, device, function, offset, 0xFFFFFFFF);
        let mask = self.read_config(bus, device, function, offset);
        self.write_config(bus, device, function, offset, orig); // 恢复
        let mut m = mask as u64;
        if (m & BAR_TYPE_MASK) == BAR_TYPE_IO {
            m &= !1u64; // IO 地址掩码忽略类型位
        } else {
            m &= !0xFu64; // 内存地址掩码忽略低 4 位
        }
        if m == 0 { return 0; }
        // 最低 1 位的位置即对齐大小。
        let size = (!m).wrapping_add(1);
        size
    }

    /// 解析一个 BAR。
    fn parse_bar(&mut self, bus: u8, device: u8, function: u8, index: u8, bar_offset: usize) -> Bar {
        let raw = self.read_config(bus, device, function, bar_offset as u8);
        let bar_type = if (raw as u64 & BAR_TYPE_MASK) == BAR_TYPE_IO {
            BarType::Io
        } else {
            let mem_type = (raw as u64 >> 1) & 0x3;
            match mem_type {
                0 => BarType::Memory32,
                2 => BarType::Memory64,
                _ => BarType::Unknown,
            }
        };
        let prefetchable = (raw as u64 >> 3) & 0x1 == 1;
        let size = self.probe_bar_size(bus, device, function, bar_offset);
        let base = if bar_type == BarType::Io { raw as u64 & !1u64 } else { raw as u64 & !0xFu64 };
        Bar { index, bar_type, raw, size, base, prefetchable }
    }

    /// 读取配置空间标准头，构造 PciDevice。
    fn read_device_header(&mut self, bus: u8, device: u8, function: u8) -> Option<PciDevice> {
        let vendor_id = self.read_config16(bus, device, function, PCI_OFFSET_VENDOR_ID as u8);
        if vendor_id == PCI_VENDOR_ID_NONE { return None; }
        let device_id = self.read_config16(bus, device, function, PCI_OFFSET_DEVICE_ID as u8);
        let command = self.read_config16(bus, device, function, PCI_OFFSET_COMMAND as u8);
        let status = self.read_config16(bus, device, function, PCI_OFFSET_STATUS as u8);
        let revision_id = self.read_config8(bus, device, function, PCI_OFFSET_REVISION_ID as u8);
        let prog_if = self.read_config8(bus, device, function, PCI_OFFSET_PROG_IF as u8);
        let subclass = self.read_config8(bus, device, function, PCI_OFFSET_SUBCLASS as u8);
        let class_code = self.read_config8(bus, device, function, PCI_OFFSET_CLASS_CODE as u8);
        let header_type = self.read_config8(bus, device, function, PCI_OFFSET_HEADER_TYPE as u8);
        let multi_function = header_type & 0x80 != 0;
        let cap_ptr = self.read_config8(bus, device, function, PCI_OFFSET_CAPABILITY_PTR as u8);
        let irq_line = self.read_config8(bus, device, function, PCI_OFFSET_INTERRUPT_LINE as u8);

        // 读取 6 个 BAR（64 位 BAR 占 2 个槽位）。
        let mut bars: [Bar; PCI_OFFSET_BAR_COUNT] = [
            Bar { index: 0, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 1, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 2, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 3, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 4, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 5, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
        ];
        let mut i = 0;
        let mut idx = 0;
        while i < PCI_OFFSET_BAR_COUNT && idx < PCI_OFFSET_BAR_COUNT {
            let bar_offset = PCI_OFFSET_BAR0 + i * PCI_OFFSET_BAR_SPACING;
            let bar = self.parse_bar(bus, device, function, idx as u8, bar_offset);
            bars[idx] = bar;
            idx += 1;
            i += 1;
            if bar.bar_type == BarType::Memory64 { i += 1; }
        }

        Some(PciDevice {
            bus, device, function, vendor_id, device_id, class_code, subclass, prog_if, revision_id,
            irq_line, command, status, bars, cap_ptr, multi_function,
        })
    }

    // ------ 枚举 ------

    /// 扫描 PCI 总线：遍历 bus/device/function，读取配置空间，发现设备。
    pub fn scan_bus(&mut self) -> Vec<PciDevice> {
        let mut found = Vec::new();
        for bus in 0..=255u8 {
            for device in 0..=31u8 {
                if !self.device_present(bus, device, 0) {
                    continue;
                }
                if let Some(dev) = self.read_device_header(bus, device, 0) {
                    let mf = dev.multi_function;
                    let key = dev.channel();
                    self.devices.insert(key.clone(), dev.clone());
                    found.push(dev);
                    // 若多功能，继续探测 function 1..7。
                    if mf {
                        for function in 1..=7u8 {
                            if self.device_present(bus, device, function) {
                                if let Some(devf) = self.read_device_header(bus, device, function) {
                                    let key = devf.channel();
                                    self.devices.insert(key.clone(), devf.clone());
                                    found.push(devf);
                                }
                            }
                        }
                    }
                }
            }
        }
        found
    }

    /// 按基类代码查找设备。
    pub fn device_by_class(&self, class_code: u8) -> Vec<PciDevice> {
        self.devices.values().filter(|d| d.class_code == class_code).cloned().collect()
    }

    /// 按供应商/设备 ID 查找。
    pub fn device_by_id(&self, vendor_id: u16, device_id: u16) -> Vec<PciDevice> {
        self.devices.values().filter(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned().collect()
    }

    /// 查找指定 BDF 的设备。
    pub fn find(&self, bdf: &str) -> Option<PciDevice> {
        self.devices.get(bdf).cloned()
    }

    /// 启用设备的总线主控与内存空间（写命令寄存器）。
    pub fn enable_bus_master(&mut self, bus: u8, device: u8, function: u8) -> bool {
        if !self.device_present(bus, device, function) { return false; }
        let cmd = self.read_config16(bus, device, function, PCI_OFFSET_COMMAND as u8);
        let new_cmd = cmd | PCI_COMMAND_BUS_MASTER | PCI_COMMAND_MEMORY_SPACE | PCI_COMMAND_IO_SPACE;
        self.write_config(bus, device, function, PCI_OFFSET_COMMAND as u8, new_cmd);
        true
    }

    /// 状态快照字符串（供 sense）。
    pub fn status(&self) -> String {
        let mut lines: Vec<String> = self.devices.values().map(|d| {
            format!("  {} {} class={:02x} subclass={:02x} prog_if={:02x} irq={} bars={}",
                d.bdf(), d.id_str(), d.class_code, d.subclass, d.prog_if, d.irq_line,
                d.bars.iter().filter(|b| b.raw != 0).count())
        }).collect();
        lines.sort();
        format!("PCI devices={}\n{}", self.devices.len(), lines.join("\n"))
    }
}

// =======================================================================
// sense/act 接口（drv://pci）
// ======================================================================

/// 查询 PCI 总线状态快照（供 `sense drv://pci`）。
pub fn pci_sense_snapshot() -> String {
    let bus = PciBus::bus().lock();
    bus.status()
}

/// 对 PCI 通道执行行动（供 `act drv://pci`）。负载：`scan` / `list` / `class <代码>` / `bdf <bdf>`。
pub fn pci_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("scan") => {
            let mut bus = PciBus::bus().lock();
            let found = bus.scan_bus();
            Ok(format!("PCI 扫描完成，发现 {} 个设备", found.len()))
        }
        Some("list") => {
            let bus = PciBus::bus().lock();
            Ok(bus.status())
        }
        Some("class") => {
            let code_s = it.next().ok_or("用法: class <16 进制类代码>")?;
            let code = u8::from_str_radix(code_s, 16).map_err(|e| e.to_string())?;
            let bus = PciBus::bus().lock();
            let devs = bus.device_by_class(code);
            Ok(format!("类 {:02x} 设备数={}", code, devs.len()))
        }
        Some("bdf") => {
            let bdf = it.next().ok_or("用法: bdf <bus:device.function>")?;
            let bus = PciBus::bus().lock();
            match bus.find(bdf) {
                Some(d) => Ok(format!("{} {} command=0x{:04x} status=0x{:04x} irq={}", d.bdf(), d.id_str(), d.command, d.status, d.irq_line)),
                None => Err(format!("未找到设备 {}", bdf)),
            }
        }
        _ => Err("用法: act drv://pci scan | list | class <代码> | bdf <bdf>".to_string()),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 u32：低 16 位 = vendor_id，高 16 位 = device_id（PCI offset 0x00 布局）。
    fn vd_u32(vendor: u16, device: u16) -> u32 {
        (device as u32) << 16 | vendor as u32
    }

    #[test]
    fn default_config_read_absent() {
        let bus = PciBus::new();
        // 无设备时读回 0xFFFFFFFF（模拟真实硬件 0xFFFF）。
        assert_eq!(bus.read_config(0, 0, 0, 256), 0xFFFFFFFF);
        assert_eq!(bus.read_config(0, 0, 0, 255), 0xFFFFFFFF); // 对齐到 252，未写入
    }

    #[test]
    fn config_read_write_roundtrip() {
        let mut config = PciDefaultConfig::new();
        // offset 0x00: vendor=0x8086, device=0x15F2
        config.insert(0, 5, 0, 0x00, vd_u32(0x8086, 0x15F2));
        // offset 0x04: command=0x0007, status=0x00A0
        config.insert(0, 5, 0, 0x04, 0x00A0_0007);
        let bus = PciBus::with_config(Box::new(config));
        assert_eq!(bus.read_config16(0, 5, 0, 0x00), 0x8086); // vendor
        assert_eq!(bus.read_config16(0, 5, 0, 0x02), 0x15F2); // device
        assert_eq!(bus.read_config16(0, 5, 0, 0x04), 0x0007); // command
        assert_eq!(bus.read_config16(0, 5, 0, 0x06), 0x00A0); // status
    }

    #[test]
    fn write_then_read() {
        let mut bus = PciBus::with_config(Box::new(PciDefaultConfig::new()));
        bus.write_config(0, 5, 0, 0x04, 0x0007_0001);
        assert_eq!(bus.read_config16(0, 5, 0, 0x04), 0x0001);
        assert_eq!(bus.read_config16(0, 5, 0, 0x06), 0x0007);
    }

    #[test]
    fn device_present_and_read_header() {
        let mut config = PciDefaultConfig::new();
        // Realtek 网卡：vendor=0x10EC, device=0x8168, class=0x02(网络)
        config.insert(0, 3, 0, 0x00, vd_u32(0x10EC, 0x8168));
        config.insert(0, 3, 0, 0x04, 0x00A0_0007);
        config.insert(0, 3, 0, 0x08, 0x0200_0010); // class=0x02 @ byte 0x0B, revision=0x10 @ byte 0x08
        config.insert(0, 3, 0, 0x0E, 0x0000_0000); // header type 0 (single function)
        config.insert(0, 3, 0, 0x34, 0x0000_0040); // cap_ptr=0x40
        config.insert(0, 3, 0, 0x3C, 0x0000_0014); // irq_line=0x14
        let mut bus = PciBus::with_config(Box::new(config));
        assert!(bus.device_present(0, 3, 0));
        let dev = bus.read_device_header(0, 3, 0).unwrap();
        assert_eq!(dev.bdf(), "00:03.0");
        assert_eq!(dev.vendor_id, 0x10EC);
        assert_eq!(dev.device_id, 0x8168);
        assert_eq!(dev.class_code, PCI_CLASS_NETWORK);
        assert_eq!(dev.irq_line, 0x14);
        assert!(!dev.multi_function);
        assert_eq!(dev.command, 0x0007);
    }

    #[test]
    fn scan_bus_populates() {
        let mut config = PciDefaultConfig::new();
        // 设备 A：显示器 vendor=0x8086 device=0x5916 class=0x03
        config.insert(0, 2, 0, 0x00, vd_u32(0x8086, 0x5916));
        config.insert(0, 2, 0, 0x08, 0x0300_0000); // class=0x03
        config.insert(0, 2, 0, 0x0E, 0x0000_0000);
        // 设备 B：网卡 vendor=0x10EC device=0x8168 class=0x02
        config.insert(0, 4, 0, 0x00, vd_u32(0x10EC, 0x8168));
        config.insert(0, 4, 0, 0x08, 0x0200_0000); // class=0x02
        config.insert(0, 4, 0, 0x0E, 0x0000_0000);
        let mut bus = PciBus::with_config(Box::new(config));
        let found = bus.scan_bus();
        assert_eq!(found.len(), 2);
        assert_eq!(bus.len(), 2);
        assert_eq!(bus.device_by_id(0x10EC, 0x8168).len(), 1);
        assert_eq!(bus.device_by_id(0x8086, 0x5916).len(), 1);
        assert_eq!(bus.device_by_class(PCI_CLASS_NETWORK).len(), 1);
        assert_eq!(bus.device_by_class(PCI_CLASS_DISPLAY).len(), 1);
        // 扫描后再次扫描不重复（已插入 devices，device_present 仍真，但 read_device_header 会重复插入覆盖，len 保持 2）。
        let _ = bus.scan_bus();
        assert_eq!(bus.len(), 2);
    }

    #[test]
    fn bar_type_detection() {
        let mut config = PciDefaultConfig::new();
        config.insert(0, 1, 0, 0x00, vd_u32(0x1234, 0x5678));
        config.insert(0, 1, 0, 0x0B, 0x0200_0000); // class network (byte 0x0B = 0x02)
        config.insert(0, 1, 0, 0x0E, 0x0000_0000);
        // BAR0 = 内存 32 位，原值 0x1000_0000（低 4 位为 0）。
        config.insert(0, 1, 0, 0x10, 0x1000_0000);
        let mut bus = PciBus::with_config(Box::new(config));
        let dev = bus.read_device_header(0, 1, 0).unwrap();
        let bar0 = dev.bar(0).unwrap();
        assert_eq!(bar0.bar_type, BarType::Memory32);
        assert!(!bar0.prefetchable);
        assert_eq!(bar0.raw, 0x1000_0000);
    }

    #[test]
    fn enable_bus_master() {
        let mut config = PciDefaultConfig::new();
        config.insert(0, 1, 0, 0x00, vd_u32(0x1234, 0x5678));
        config.insert(0, 1, 0, 0x04, 0x0000_0000); // command=0
        config.insert(0, 1, 0, 0x0B, 0x0200_0000);
        config.insert(0, 1, 0, 0x0E, 0x0000_0000);
        let mut bus = PciBus::with_config(Box::new(config));
        assert!(bus.enable_bus_master(0, 1, 0));
        let dev = bus.read_device_header(0, 1, 0).unwrap();
        assert!(dev.bus_master_enabled());
        assert!(dev.io_space_enabled());
    }

    #[test]
    fn bdf_and_channel() {
        let bars = [
            Bar { index: 0, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 1, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 2, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 3, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 4, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
            Bar { index: 5, bar_type: BarType::Unknown, raw: 0, size: 0, base: 0, prefetchable: false },
        ];
        let dev = PciDevice { bus: 0, device: 5, function: 1, vendor_id: 0x8086, device_id: 0x15F2, class_code: 0x03, subclass: 0x00, prog_if: 0x00, revision_id: 0x00, irq_line: 16, command: 0, status: 0, bars, cap_ptr: 0, multi_function: true };
        assert_eq!(dev.bdf(), "00:05.1");
        assert_eq!(dev.channel(), "drv://pci/00:05.1");
        assert_eq!(dev.id_str(), "8086:15f2");
        assert!(dev.is_valid());
        let inv = PciDevice { vendor_id: PCI_VENDOR_ID_NONE, ..dev };
        assert!(!inv.is_valid());
    }

    #[test]
    fn find_missing() {
        let bus = PciBus::new();
        assert!(bus.find("00:03.0").is_none());
    }

    #[test]
    fn sense_and_act() {
        let snap = pci_sense_snapshot();
        assert!(snap.contains("PCI devices="));
        assert!(pci_act_raw("scan").is_ok());
        assert!(pci_act_raw("list").is_ok());
        assert!(pci_act_raw("class 02").is_ok());
        assert!(pci_act_raw("bdf 00:03.0").is_err()); // 默认无设备
    }
}
