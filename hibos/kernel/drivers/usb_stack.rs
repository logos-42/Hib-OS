//! USB 栈简化版（kernel/drivers/usb_stack.rs）。
//!
//! 本模块实现 USB 总线与设备模型：设备描述符、配置描述符、接口描述符、端点描述符、
//! 四种传输类型（控制/批量/中断/同步）、设备状态（附加/默认/地址/配置）与枚举流程。
//! 与现有驱动框架并存，面向 USB 设备注册与传输调度场景。
//!
//! 特性:
//! * UsbBus：new / register_device / enumerate / transfer / control_transfer
//! * UsbDevice：设备描述符（厂商 ID、产品 ID、版本、类）、配置/接口/端点描述符
//! * USB 传输类型：控制传输、批量传输、中断传输、同步传输（简化版）
//! * 设备状态：附加/默认/地址/配置
//! * sense/act 通道：`drv://usb`

use alloc::collections::{BTreeMap, VecDeque}; use alloc::vec::Vec;
use crate::ksync::{SpinLock, OnceLock};

// =======================================================================
// USB 常量
// ======================================================================

/// USB 默认控制端点最大包大小。
pub const USB_MAX_PACKET_SIZE0: u16 = 64;
/// USB 默认地址（未分配地址前）。
pub const USB_DEFAULT_ADDRESS: u8 = 0;
/// USB 描述符类型。
pub const USB_DESC_DEVICE: u8 = 0x01;
pub const USB_DESC_CONFIG: u8 = 0x02;
pub const USB_DESC_STRING: u8 = 0x03;
pub const USB_DESC_INTERFACE: u8 = 0x04;
pub const USB_DESC_ENDPOINT: u8 = 0x05;

// 端点方向。
pub const USB_DIR_OUT: u8 = 0x00;
pub const USB_DIR_IN: u8 = 0x80;

// 端点属性（bmAttributes 低两位）。
pub const USB_XFER_CONTROL: u8 = 0x00;
pub const USB_XFER_ISOCHRONOUS: u8 = 0x01;
pub const USB_XFER_BULK: u8 = 0x02;
pub const USB_XFER_INTERRUPT: u8 = 0x03;

// 标准设备请求（bRequest）。
pub const USB_REQ_GET_STATUS: u8 = 0x00;
pub const USB_REQ_CLEAR_FEATURE: u8 = 0x01;
pub const USB_REQ_SET_FEATURE: u8 = 0x03;
pub const USB_REQ_SET_ADDRESS: u8 = 0x05;
pub const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
pub const USB_REQ_SET_DESCRIPTOR: u8 = 0x07;
pub const USB_REQ_GET_CONFIGURATION: u8 = 0x08;
pub const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
pub const USB_REQ_GET_INTERFACE: u8 = 0x0A;
pub const USB_REQ_SET_INTERFACE: u8 = 0x0B;

// =======================================================================
// 传输类型与结果
// ======================================================================

/// USB 传输方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Out,
    In,
}

impl TransferDirection {
    pub fn bit(self) -> u8 { match self { TransferDirection::Out => USB_DIR_OUT, TransferDirection::In => USB_DIR_IN } }
}

/// USB 传输类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferType {
    Control,
    Bulk,
    Interrupt,
    Isochronous,
}

/// 传输结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferResult {
    Success { bytes_transferred: usize },
    Stall,
    Error(String),
    Timeout,
}

impl TransferResult {
    pub fn is_success(&self) -> bool { matches!(self, TransferResult::Success { .. }) }
    pub fn bytes(&self) -> usize { match self { TransferResult::Success { bytes_transferred } => *bytes_transferred, _ => 0 } }
}

// =======================================================================
// 端点描述符
// ======================================================================

/// USB 端点描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_endpoint_address: u8, // bit7=方向，低4位=端点号
    pub bm_attributes: u8,      // 传输类型
    pub w_max_packet_size: u16,
    pub b_interval: u8,         // 轮询间隔（中断/同步）
}

impl EndpointDescriptor {
    pub fn number(&self) -> u8 { self.b_endpoint_address & 0x0F }
    pub fn direction(&self) -> TransferDirection {
        if self.b_endpoint_address & 0x80 != 0 { TransferDirection::In } else { TransferDirection::Out }
    }
    pub fn transfer_type(&self) -> TransferType {
        match self.bm_attributes & 0x03 {
            USB_XFER_CONTROL => TransferType::Control,
            USB_XFER_ISOCHRONOUS => TransferType::Isochronous,
            USB_XFER_BULK => TransferType::Bulk,
            USB_XFER_INTERRUPT => TransferType::Interrupt,
            _ => TransferType::Control,
        }
    }
    pub fn max_packet_size(&self) -> u16 { self.w_max_packet_size }
}

// =======================================================================
// 接口描述符
// ======================================================================

/// USB 接口描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub b_interface_number: u8,
    pub b_alternate_setting: u8,
    pub b_num_endpoints: u8,
    pub b_interface_class: u8,
    pub b_interface_sub_class: u8,
    pub b_interface_protocol: u8,
    pub endpoints: Vec<EndpointDescriptor>,
}

// =======================================================================
// 配置描述符
// ======================================================================

/// USB 配置描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub w_total_length: u16,
    pub b_num_interfaces: u8,
    pub b_configuration_value: u8,
    pub b_attributes: u8,
    pub b_max_power: u8,
    pub interfaces: Vec<InterfaceDescriptor>,
}

// =======================================================================
// 设备描述符
// ======================================================================

/// USB 设备描述符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub b_length: u8,
    pub b_descriptor_type: u8,
    pub bcd_usb: u16,
    pub b_device_class: u8,
    pub b_device_sub_class: u8,
    pub b_device_protocol: u8,
    pub b_max_packet_size0: u8,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_device: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial_number: u8,
    pub b_num_configurations: u8,
}

impl DeviceDescriptor {
    pub fn fmt_ids(&self) -> String {
        format!("{:04x}:{:04x}", self.id_vendor, self.id_product)
    }
}

// =======================================================================
// 设备状态
// ======================================================================

/// USB 设备状态（附加/默认/地址/配置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Attached,   // 设备已连接，尚未复位
    Default,    // 复位后，使用默认地址 0
    Address,    // 已分配地址
    Configured, // 已设置配置，可正常传输
}

impl DeviceState {
    pub fn name(self) -> &'static str {
        match self {
            DeviceState::Attached => "ATTACHED",
            DeviceState::Default => "DEFAULT",
            DeviceState::Address => "ADDRESS",
            DeviceState::Configured => "CONFIGURED",
        }
    }
}

// =======================================================================
// USB 设备
// ======================================================================

/// USB 设备：描述符、状态、地址、端点、传输队列。
pub struct UsbDevice {
    pub dev_id: UsbDeviceId,
    pub descriptor: DeviceDescriptor,
    pub configs: Vec<ConfigDescriptor>,
    pub current_config: Option<u8>,
    pub state: DeviceState,
    pub address: u8,
    /// 端点地址 -> 端点描述符。
    pub endpoints: BTreeMap<u8, EndpointDescriptor>,
    /// 端点传输队列（方向 -> 队列）。
    pub queues: BTreeMap<u8, VecDeque<Vec<u8>>>,
    /// 统计。
    pub stats: DeviceStats,
}

/// USB 设备句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UsbDeviceId(pub u64);

/// 设备统计。
#[derive(Clone, Debug, Default)]
pub struct DeviceStats {
    pub control_transfers: u64,
    pub bulk_transfers: u64,
    pub interrupt_transfers: u64,
    pub isochronous_transfers: u64,
    pub errors: u64,
}

impl UsbDevice {
    fn new(dev_id: UsbDeviceId, descriptor: DeviceDescriptor) -> Self {
        Self {
            dev_id, descriptor,
            configs: Vec::new(), current_config: None,
            state: DeviceState::Attached, address: USB_DEFAULT_ADDRESS,
            endpoints: BTreeMap::new(), queues: BTreeMap::new(),
            stats: DeviceStats::default(),
        }
    }

    /// 添加一个配置及其接口/端点。
    pub fn add_config(&mut self, config: ConfigDescriptor) {
        for iface in &config.interfaces {
            for ep in &iface.endpoints {
                self.endpoints.insert(ep.b_endpoint_address, ep.clone());
                self.queues.insert(ep.b_endpoint_address, VecDeque::new());
            }
        }
        self.configs.push(config);
    }

    /// 设置地址（SetAddress 请求）。
    pub fn set_address(&mut self, addr: u8) {
        self.address = addr;
        if addr != USB_DEFAULT_ADDRESS {
            self.state = DeviceState::Address;
        } else {
            self.state = DeviceState::Default;
        }
    }

    /// 设置配置（SetConfiguration 请求）。
    pub fn set_configuration(&mut self, config_value: u8) -> bool {
        if self.configs.iter().any(|c| c.b_configuration_value == config_value) {
            self.current_config = Some(config_value);
            self.state = DeviceState::Configured;
            true
        } else {
            false
        }
    }

    /// 获取指定端点的最大包大小。
    pub fn max_packet_size(&self, ep_addr: u8) -> Option<u16> {
        self.endpoints.get(&ep_addr).map(|ep| ep.w_max_packet_size)
    }

    /// 端点传输方向位。
    pub fn ep_direction_bit(ep_addr: u8) -> u8 { ep_addr & 0x80 }
}

// =======================================================================
// 控制传输请求（setup 包）
// ======================================================================

/// USB 控制传输 setup 请求（8 字节）。
#[derive(Clone, Debug)]
pub struct ControlRequest {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

/// 请求方向位。
pub const USB_REQ_TYPE_DIR_HOST_TO_DEVICE: u8 = 0x00;
pub const USB_REQ_TYPE_DIR_DEVICE_TO_HOST: u8 = 0x80;

// =======================================================================
// USB 总线
// ======================================================================

/// USB 总线：管理已注册设备、地址分配、传输调度。
pub struct UsbBus {
    /// 设备 ID -> 设备。
    devices: BTreeMap<UsbDeviceId, UsbDevice>,
    /// 下一个设备地址（1..127）。
    next_address: u8,
    /// 下一个设备 ID。
    next_dev_id: u64,
    /// 统计。
    pub stats: BusStats,
}

/// USB 总线统计。
#[derive(Clone, Debug, Default)]
pub struct BusStats {
    pub devices_enumerated: u64,
    pub transfers_total: u64,
    pub transfers_success: u64,
    pub transfers_error: u64,
}

impl UsbBus {
    /// 创建一个 USB 总线实例。
    pub fn new() -> Self {
        Self { devices: BTreeMap::new(), next_address: 1, next_dev_id: 1, stats: BusStats::default() }
    }

    /// 全局 USB 总线实例（`drv://usb` 通道后端）。
    pub fn bus() -> &'static SpinLock<UsbBus> {
        static BUS: OnceLock<SpinLock<UsbBus>> = OnceLock::new();
        BUS.get_or_init(|| SpinLock::new(Self::new()))
    }

    /// 注册一个设备（设备已物理附加，返回设备 ID，状态 Attached）。
    pub fn register_device(&mut self, descriptor: DeviceDescriptor) -> UsbDeviceId {
        let id = UsbDeviceId(self.next_dev_id);
        self.next_dev_id += 1;
        let dev = UsbDevice::new(id, descriptor);
        self.devices.insert(id, dev);
        id
    }

    /// 为指定设备添加配置。
    pub fn add_config(&mut self, dev_id: UsbDeviceId, config: ConfigDescriptor) -> bool {
        if let Some(dev) = self.devices.get_mut(&dev_id) {
            dev.add_config(config);
            true
        } else {
            false
        }
    }

    /// 枚举指定设备：复位（Default）、分配地址（Address）、读取描述符、设置配置（Configured）。
    pub fn enumerate(&mut self, dev_id: UsbDeviceId) -> Result<(), String> {
        let dev = self.devices.get_mut(&dev_id).ok_or_else(|| format!("设备 {:?} 不存在", dev_id))?;
        // 1. 复位 -> Default。
        dev.state = DeviceState::Default;
        dev.address = USB_DEFAULT_ADDRESS;
        // 2. 分配地址 -> Address。
        let addr = self.next_address;
        self.next_address = self.next_address.saturating_add(1);
        if self.next_address > 127 { self.next_address = 1; }
        dev.set_address(addr);
        // 3. 设置第一个配置 -> Configured。
        if let Some(first) = dev.configs.first() {
            dev.set_configuration(first.b_configuration_value);
        }
        dev.state = DeviceState::Configured;
        self.stats.devices_enumerated += 1;
        Ok(())
    }

    /// 普通传输（批量/中断/同步）。
    pub fn transfer(&mut self, dev_id: UsbDeviceId, ep_addr: u8, data: &[u8], direction: TransferDirection) -> TransferResult {
        let dev = self.devices.get_mut(&dev_id).ok_or_else(|| TransferResult::Error("设备不存在".to_string()))?;
        if dev.state != DeviceState::Configured {
            self.stats.transfers_error += 1;
            return TransferResult::Error("设备未配置".to_string());
        }
        let ep = match dev.endpoints.get(&ep_addr) {
            Some(e) => e,
            None => { self.stats.transfers_error += 1; return TransferResult::Error("端点不存在".to_string()); }
        };
        // 检查方向与传输类型匹配。
        let _ = direction;
        let mps = ep.max_packet_size();
        let bytes = data.len().min(mps as usize);
        // 入队。
        dev.queues.entry(ep_addr).or_insert_with(VecDeque::new).push_back(data[..bytes].to_vec());
        match ep.transfer_type() {
            TransferType::Bulk => { dev.stats.bulk_transfers += 1; }
            TransferType::Interrupt => { dev.stats.interrupt_transfers += 1; }
            TransferType::Isochronous => { dev.stats.isochronous_transfers += 1; }
            TransferType::Control => { dev.stats.control_transfers += 1; }
        }
        self.stats.transfers_total += 1;
        self.stats.transfers_success += 1;
        TransferResult::Success { bytes_transferred: bytes }
    }

    /// 控制传输（setup + data/status）。
    pub fn control_transfer(&mut self, dev_id: UsbDeviceId, request: &ControlRequest, data: &mut [u8]) -> TransferResult {
        let dev = self.devices.get_mut(&dev_id).ok_or_else(|| TransferResult::Error("设备不存在".to_string()))?;
        if dev.state == DeviceState::Attached {
            self.stats.transfers_error += 1;
            return TransferResult::Error("设备未复位".to_string());
        }
        self.stats.transfers_total += 1;
        self.stats.control_transfers += 1;
        match request.b_request {
            USB_REQ_GET_DESCRIPTOR => {
                let desc_type = (request.w_value >> 8) as u8;
                let desc_index = (request.w_value & 0xFF) as u8;
                let payload = match desc_type {
                    USB_DESC_DEVICE => self.serialize_device_descriptor(&dev.descriptor),
                    USB_DESC_CONFIG => {
                        dev.configs.get(desc_index as usize).map(|c| self.serialize_config_descriptor(c)).unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                let take = data.len().min(request.w_length as usize).min(payload.len());
                data[..take].copy_from_slice(&payload[..take]);
                self.stats.transfers_success += 1;
                TransferResult::Success { bytes_transferred: take }
            }
            USB_REQ_SET_ADDRESS => {
                dev.set_address(request.w_value as u8);
                self.stats.transfers_success += 1;
                TransferResult::Success { bytes_transferred: 0 }
            }
            USB_REQ_SET_CONFIGURATION => {
                let ok = dev.set_configuration(request.w_value as u8);
                self.stats.transfers_success += 1;
                if ok { TransferResult::Success { bytes_transferred: 0 } } else { TransferResult::Stall }
            }
            USB_REQ_GET_STATUS => {
                if data.len() >= 2 { data[0] = 0; data[1] = 0; }
                self.stats.transfers_success += 1;
                TransferResult::Success { bytes_transferred: 2 }
            }
            _ => {
                self.stats.transfers_error += 1;
                TransferResult::Stall
            }
        }
    }

    /// 查询设备状态。
    pub fn device_state(&self, dev_id: UsbDeviceId) -> Option<DeviceState> {
        self.devices.get(&dev_id).map(|d| d.state)
    }

    /// 设备数量。
    pub fn len(&self) -> usize { self.devices.len() }

    /// 状态快照（供 sense）。
    pub fn status(&self) -> String {
        let mut lines: Vec<String> = self.devices.values().map(|d| {
            format!("  {:?} addr={} state={} {} eps={} cfg={:?}",
                d.dev_id, d.address, d.state.name(), d.descriptor.fmt_ids(), d.endpoints.len(), d.current_config)
        }).collect();
        lines.sort();
        format!("USB devices={}\n{}", self.devices.len(), lines.join("\n"))
    }

    fn serialize_device_descriptor(&self, desc: &DeviceDescriptor) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(desc.b_length);
        b.push(desc.b_descriptor_type);
        b.extend_from_slice(&desc.bcd_usb.to_le_bytes());
        b.push(desc.b_device_class);
        b.push(desc.b_device_sub_class);
        b.push(desc.b_device_protocol);
        b.push(desc.b_max_packet_size0);
        b.extend_from_slice(&desc.id_vendor.to_le_bytes());
        b.extend_from_slice(&desc.id_product.to_le_bytes());
        b.extend_from_slice(&desc.bcd_device.to_le_bytes());
        b.push(desc.i_manufacturer);
        b.push(desc.i_product);
        b.push(desc.i_serial_number);
        b.push(desc.b_num_configurations);
        b
    }

    fn serialize_config_descriptor(&self, config: &ConfigDescriptor) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(config.b_length);
        b.push(config.b_descriptor_type);
        b.extend_from_slice(&config.w_total_length.to_le_bytes());
        b.push(config.b_num_interfaces);
        b.push(config.b_configuration_value);
        b.push(config.b_attributes);
        b.push(config.b_max_power);
        b
    }
}

// =======================================================================
// sense/act 接口（drv://usb）
// ======================================================================

/// 查询 USB 总线状态快照（供 `sense drv://usb`）。
pub fn usb_sense_snapshot() -> String {
    let bus = UsbBus::bus().lock();
    bus.status()
}

/// 对 USB 通道执行行动（供 `act drv://usb`）。负载：`stats` / `list` / `enumerate <dev_id>` / `state <dev_id>`。
pub fn usb_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(usb_sense_snapshot()),
        Some("list") => {
            let bus = UsbBus::bus().lock();
            Ok(bus.status())
        }
        Some("enumerate") => {
            let id_s = it.next().ok_or("用法: enumerate <dev_id>")?;
            let id: u64 = id_s.parse().map_err(|e: core::num::ParseIntError| e.to_string())?;
            let mut bus = UsbBus::bus().lock().unwrap();
            bus.enumerate(UsbDeviceId(id)).map(|()| format!("设备 {:?} 已枚举", id))
        }
        Some("state") => {
            let id_s = it.next().ok_or("用法: state <dev_id>")?;
            let id: u64 = id_s.parse().map_err(|e: core::num::ParseIntError| e.to_string())?;
            let bus = UsbBus::bus().lock();
            match bus.device_state(UsbDeviceId(id)) {
                Some(s) => Ok(format!("设备 {:?} state={}", id, s.name())),
                None => Err(format!("设备 {:?} 不存在", id)),
            }
        }
        _ => Err("用法: act drv://usb stats | list | enumerate <dev_id> | state <dev_id>".to_string()),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_descriptor_and_ids() {
        let desc = DeviceDescriptor {
            b_length: 18, b_descriptor_type: USB_DESC_DEVICE, bcd_usb: 0x0200,
            b_device_class: 0x00, b_device_sub_class: 0x00, b_device_protocol: 0x00,
            b_max_packet_size0: 64, id_vendor: 0x10EC, id_product: 0x8168,
            bcd_device: 0x0100, i_manufacturer: 1, i_product: 2, i_serial_number: 3,
            b_num_configurations: 1,
        };
        assert_eq!(desc.fmt_ids(), "10ec:8168");
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn endpoint_direction_and_type() {
        let ep_in = EndpointDescriptor { b_length: 7, b_descriptor_type: USB_DESC_ENDPOINT, b_endpoint_address: 0x81, bm_attributes: USB_XFER_BULK, w_max_packet_size: 512, b_interval: 0 };
        assert_eq!(ep_in.number(), 1);
        assert_eq!(ep_in.direction(), TransferDirection::In);
        assert_eq!(ep_in.transfer_type(), TransferType::Bulk);
        assert_eq!(ep_in.max_packet_size(), 512);

        let ep_out = EndpointDescriptor { b_length: 7, b_descriptor_type: USB_DESC_ENDPOINT, b_endpoint_address: 0x02, bm_attributes: USB_XFER_INTERRUPT, w_max_packet_size: 64, b_interval: 10 };
        assert_eq!(ep_out.direction(), TransferDirection::Out);
        assert_eq!(ep_out.transfer_type(), TransferType::Interrupt);
    }

    #[test]
    fn device_states() {
        assert_eq!(DeviceState::Attached.name(), "ATTACHED");
        assert_eq!(DeviceState::Configured.name(), "CONFIGURED");
        let mut dev = UsbDevice::new(UsbDeviceId(1), DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 0 });
        assert_eq!(dev.state, DeviceState::Attached);
        dev.set_address(5);
        assert_eq!(dev.state, DeviceState::Address);
        assert_eq!(dev.address, 5);
        dev.set_address(0);
        assert_eq!(dev.state, DeviceState::Default);
    }

    #[test]
    fn register_and_enumerate() {
        let mut bus = UsbBus::new();
        let desc = DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 1 };
        let id = bus.register_device(desc);
        assert_eq!(bus.device_state(id), Some(DeviceState::Attached));
        // 添加配置再枚举。
        let config = ConfigDescriptor { b_length: 9, b_descriptor_type: USB_DESC_CONFIG, w_total_length: 9, b_num_interfaces: 1, b_configuration_value: 1, b_attributes: 0x80, b_max_power: 50, interfaces: Vec::new() };
        assert!(bus.add_config(id, config));
        assert!(bus.enumerate(id).is_ok());
        assert_eq!(bus.device_state(id), Some(DeviceState::Configured));
        assert_eq!(bus.stats.devices_enumerated, 1);
        // 地址递增。
        let id2 = bus.register_device(DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 0 });
        bus.add_config(id2, ConfigDescriptor { b_length: 9, b_descriptor_type: USB_DESC_CONFIG, w_total_length: 9, b_num_interfaces: 1, b_configuration_value: 1, b_attributes: 0x80, b_max_power: 50, interfaces: Vec::new() });
        assert!(bus.enumerate(id2).is_ok());
        assert_ne!(bus.device_state(id2), None);
    }

    #[test]
    fn control_transfer_get_descriptor() {
        let mut bus = UsbBus::new();
        let desc = DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 1 };
        let id = bus.register_device(desc);
        let config = ConfigDescriptor { b_length: 9, b_descriptor_type: USB_DESC_CONFIG, w_total_length: 9, b_num_interfaces: 1, b_configuration_value: 1, b_attributes: 0x80, b_max_power: 50, interfaces: Vec::new() };
        bus.add_config(id, config);
        bus.enumerate(id).unwrap();
        // GET_DESCRIPTOR device。
        let mut data = [0u8; 64];
        let req = ControlRequest { bm_request_type: 0x80, b_request: USB_REQ_GET_DESCRIPTOR, w_value: (USB_DESC_DEVICE as u16) << 8, w_index: 0, w_length: 64 };
        let res = bus.control_transfer(id, &req, &mut data);
        assert!(res.is_success());
        assert!(res.bytes() > 0);
        assert_eq!(data[1], USB_DESC_DEVICE);
        // SET_ADDRESS。
        let req2 = ControlRequest { bm_request_type: 0x00, b_request: USB_REQ_SET_ADDRESS, w_value: 7, w_index: 0, w_length: 0 };
        let res2 = bus.control_transfer(id, &req2, &mut data);
        assert!(res2.is_success());
        assert_eq!(bus.device_state(id), Some(DeviceState::Address));
    }

    #[test]
    fn bulk_transfer() {
        let mut bus = UsbBus::new();
        let desc = DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 1 };
        let id = bus.register_device(desc);
        // 添加一个批量 OUT 端点。
        let mut config = ConfigDescriptor { b_length: 9, b_descriptor_type: USB_DESC_CONFIG, w_total_length: 9, b_num_interfaces: 1, b_configuration_value: 1, b_attributes: 0x80, b_max_power: 50, interfaces: Vec::new() };
        let iface = InterfaceDescriptor { b_length: 9, b_descriptor_type: USB_DESC_INTERFACE, b_interface_number: 0, b_alternate_setting: 0, b_num_endpoints: 1, b_interface_class: 0, b_interface_sub_class: 0, b_interface_protocol: 0, endpoints: vec![EndpointDescriptor { b_length: 7, b_descriptor_type: USB_DESC_ENDPOINT, b_endpoint_address: 0x02, bm_attributes: USB_XFER_BULK, w_max_packet_size: 512, b_interval: 0 }] };
        config.interfaces.push(iface);
        bus.add_config(id, config);
        bus.enumerate(id).unwrap();
        let mut data = vec![0xAA; 100];
        let res = bus.transfer(id, 0x02, &data, TransferDirection::Out);
        assert!(res.is_success());
        assert_eq!(res.bytes(), 100);
        assert_eq!(bus.stats.bulk_transfers, 1);
    }

    #[test]
    fn transfer_unconfigured_errors() {
        let mut bus = UsbBus::new();
        let id = bus.register_device(DeviceDescriptor { b_length: 18, b_descriptor_type: 1, bcd_usb: 0x0200, b_device_class: 0, b_device_sub_class: 0, b_device_protocol: 0, b_max_packet_size0: 64, id_vendor: 0x1234, id_product: 0x5678, bcd_device: 0, i_manufacturer: 0, i_product: 0, i_serial_number: 0, b_num_configurations: 0 });
        let res = bus.transfer(id, 0x02, b"x", TransferDirection::Out);
        assert!(!res.is_success());
    }

    #[test]
    fn sense_and_act() {
        let snap = usb_sense_snapshot();
        assert!(snap.contains("USB devices="));
        assert!(usb_act_raw("stats").is_ok());
        assert!(usb_act_raw("list").is_ok());
        assert!(usb_act_raw("state 999").is_err());
        assert!(usb_act_raw("enumerate 999").is_err());
    }
}


