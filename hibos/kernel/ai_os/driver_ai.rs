//! AI 原生驱动层 —— AI 原生 OS 的「智能设备驱动」 (AI-Native Driver)
//!
//! 本模块实现 AI 原生 OS 的设备驱动框架，对应传统 OS 中的
//! 「设备驱动 / PCI 枚举 / 中断处理 / DMA / 设备树」概念，但重新映射到 AI 域：
//!
//! * AIDevice (AI 设备) —— 带语义能力的设备抽象，等价于传统设备节点；
//! * AIDriver (AI 驱动) —— 自适应驱动程序，等价于传统设备驱动；
//! * PCIDevice (PCI 设备) —— PCI 总线设备，等价于 PCI 配置空间；
//! * PCIBus (PCI 总线) —— PCI 总线枚举与配置，等价于 PCI 控制器；
//! * IRQLine (中断线) —— 中断请求线，等价于 IRQ 线；
//! * IRQController (中断控制器) —— 中断控制器，等价于 APIC / PIC；
//! * DMARegion (DMA 区域) —— 直接内存访问区域，等价于 DMA 缓冲区；
//! * DeviceTree (设备树) —— 硬件拓扑描述，等价于设备树 (DTB)；
//! * HotPlugManager (热插拔管理器) —— 设备热插拔管理，等价于 udev / hotplug；
//! * DriverAI (AI 原生驱动引擎) —— 统一入口，通过 driver_ai://* 通道暴露。
//!
//! 所有结构均为纯 Rust 原型，无真实硬件依赖，供内核统一核算与调度。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =====================================================================================
// AI 设备 (AIDevice)
// =====================================================================================

/// 设备类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceType {
    Storage,    // 存储
    Network,    // 网络
    Display,    // 显示
    Input,      // 输入
    Audio,      // 音频
    GPU,        // 图形处理
    AIAccelerator, // AI 加速器
    Sensor,     // 传感器
    Unknown,    // 未知
}

/// AI 设备：带语义能力的设备抽象。
#[derive(Clone, Debug)]
pub struct AIDevice {
    pub id: u64,
    pub r#type: DeviceType,
    pub name: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subsystem_id: u16,
    pub revision: u8,
    pub irq: Option<u32>,
    pub dma_region: Option<u64>,
    pub capabilities: BTreeSet<String>,
    pub online: bool,
    pub driver_attached: bool,
    pub created_at: u128,
}

impl AIDevice {
    pub fn new(id: u64, r#type: DeviceType, name: &str, vendor_id: u16, device_id: u16) -> Self {
        Self {
            id,
            r#type,
            name: name.to_string(),
            vendor_id,
            device_id,
            class_code: 0,
            subsystem_id: 0,
            revision: 0,
            irq: None,
            dma_region: None,
            capabilities: BTreeSet::new(),
            online: true,
            driver_attached: false,
            created_at: crate::ai_os::native::now_ns(),
        }
    }

    pub fn add_capability(&mut self, cap: &str) {
        self.capabilities.insert(cap.to_string());
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.contains(cap)
    }

    pub fn attach_driver(&mut self) {
        self.driver_attached = true;
    }

    pub fn detach_driver(&mut self) {
        self.driver_attached = false;
    }

    pub fn set_irq(&mut self, irq: u32) {
        self.irq = Some(irq);
    }

    pub fn set_dma(&mut self, region: u64) {
        self.dma_region = Some(region);
    }
}

// =====================================================================================
// AI 驱动 (AIDriver)
// =====================================================================================

/// 驱动状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverState {
    Unloaded,
    Loading,
    Loaded,
    Initializing,
    Active,
    Error,
}

/// AI 驱动：自适应驱动程序。
#[derive(Clone, Debug)]
pub struct AIDriver {
    pub id: u64,
    pub name: String,
    pub device_type: DeviceType,
    pub vendor_id: u16,
    pub device_id: u16,
    pub state: DriverState,
    pub version: String,
    pub attached_devices: Vec<u64>,
    pub load_count: u64,
    pub error_count: u64,
}

impl AIDriver {
    pub fn new(id: u64, name: &str, device_type: DeviceType, vendor_id: u16, device_id: u16) -> Self {
        Self {
            id,
            name: name.to_string(),
            device_type,
            vendor_id,
            device_id,
            state: DriverState::Unloaded,
            version: "1.0.0".to_string(),
            attached_devices: Vec::new(),
            load_count: 0,
            error_count: 0,
        }
    }

    pub fn load(&mut self) -> Result<(), String> {
        if self.state == DriverState::Loaded || self.state == DriverState::Active {
            return Err("驱动已加载".into());
        }
        self.state = DriverState::Loading;
        self.load_count += 1;
        self.state = DriverState::Loaded;
        Ok(())
    }

    pub fn init(&mut self) -> Result<(), String> {
        if self.state != DriverState::Loaded {
            return Err("驱动未加载".into());
        }
        self.state = DriverState::Initializing;
        self.state = DriverState::Active;
        Ok(())
    }

    pub fn attach(&mut self, device_id: u64) {
        if !self.attached_devices.contains(&device_id) {
            self.attached_devices.push(device_id);
        }
    }

    pub fn detach(&mut self, device_id: u64) {
        self.attached_devices.retain(|&id| id != device_id);
    }

    pub fn record_error(&mut self) {
        self.error_count += 1;
        self.state = DriverState::Error;
    }

    pub fn reset(&mut self) {
        self.state = DriverState::Loaded;
        self.error_count = 0;
    }
}

// =====================================================================================
// PCI 设备 (PCIDevice)
// =====================================================================================

/// PCI 设备：PCI 总线上的设备。
#[derive(Clone, Debug)]
pub struct PCIDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision_id: u8,
    pub irq_pin: u8,
    pub irq_line: u8,
}

/// PCI 地址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PCIBusAddr {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PCIBusAddr {
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Self { bus, device, function }
    }

    pub fn to_u32(&self) -> u32 {
        (self.bus as u32) << 16 | (self.device as u32) << 11 | (self.function as u32) << 8
    }
}

// =====================================================================================
// PCI 总线 (PCIBus)
// =====================================================================================

/// PCI 总线：PCI 设备枚举与配置。
pub struct PCIBus {
    devices: BTreeMap<PCIBusAddr, PCIDevice>,
    pub max_bus: u8,
}

impl PCIBus {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            max_bus: 255,
        }
    }

    pub fn add_device(&mut self, dev: PCIDevice) {
        let addr = PCIBusAddr::new(dev.bus, dev.device, dev.function);
        self.devices.insert(addr, dev);
    }

    pub fn get_device(&self, addr: PCIBusAddr) -> Option<&PCIDevice> {
        self.devices.get(&addr)
    }

    pub fn enumerate(&self) -> Vec<&PCIDevice> {
        self.devices.values().collect()
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn find_by_vendor_device(&self, vendor_id: u16, device_id: u16) -> Vec<&PCIDevice> {
        self.devices.values()
            .filter(|d| d.vendor_id == vendor_id && d.device_id == device_id)
            .collect()
    }
}

// =====================================================================================
// 中断线 (IRQLine)
// =====================================================================================

/// 中断线：中断请求线。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IRQLine(pub u32);

// =====================================================================================
// 中断控制器 (IRQController)
// =====================================================================================

/// 中断处理器。
#[derive(Clone, Debug)]
pub struct IRQHandler {
    pub irq: IRQLine,
    pub handler_id: u64,
    pub owner: String,
    pub enabled: bool,
    pub invoke_count: u64,
}

/// 中断控制器：中断请求线管理。
pub struct IRQController {
    handlers: BTreeMap<IRQLine, IRQHandler>,
    pub next_handler_id: u64,
    pub total_interrupts: u64,
}

impl IRQController {
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
            next_handler_id: 1,
            total_interrupts: 0,
        }
    }

    pub fn register(&mut self, irq: IRQLine, owner: &str) -> u64 {
        let id = self.next_handler_id;
        self.next_handler_id += 1;
        self.handlers.insert(
            irq,
            IRQHandler {
                irq,
                handler_id: id,
                owner: owner.to_string(),
                enabled: true,
                invoke_count: 0,
            },
        );
        id
    }

    pub fn unregister(&mut self, irq: IRQLine) -> bool {
        self.handlers.remove(&irq).is_some()
    }

    pub fn invoke(&mut self, irq: IRQLine) -> bool {
        if let Some(handler) = self.handlers.get_mut(&irq) {
            if handler.enabled {
                handler.invoke_count += 1;
                self.total_interrupts += 1;
                return true;
            }
        }
        false
    }

    pub fn enable(&mut self, irq: IRQLine) -> bool {
        if let Some(handler) = self.handlers.get_mut(&irq) {
            handler.enabled = true;
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, irq: IRQLine) -> bool {
        if let Some(handler) = self.handlers.get_mut(&irq) {
            handler.enabled = false;
            true
        } else {
            false
        }
    }

    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }
}

// =====================================================================================
// DMA 区域 (DMARegion)
// =====================================================================================

/// DMA 区域：直接内存访问区域。
#[derive(Clone, Debug)]
pub struct DMARegion {
    pub id: u64,
    pub paddr: u64,       // 物理地址
    pub size: u64,        // 大小
    pub device_id: u64,   // 所属设备
    pub readonly: bool,
}

/// DMA 区域管理器。
pub struct DMARegionManager {
    regions: BTreeMap<u64, DMARegion>,
    next_id: u64,
    pub total_allocated: u64,
}

impl DMARegionManager {
    pub fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            next_id: 1,
            total_allocated: 0,
        }
    }

    pub fn alloc(&mut self, device_id: u64, size: u64) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let paddr = self.total_allocated;
        self.regions.insert(
            id,
            DMARegion {
                id,
                paddr,
                size,
                device_id,
                readonly: false,
            },
        );
        self.total_allocated += size;
        Ok(id)
    }

    pub fn free(&mut self, id: u64) -> Result<(), String> {
        if let Some(region) = self.regions.remove(&id) {
            self.total_allocated = self.total_allocated.saturating_sub(region.size);
            Ok(())
        } else {
            Err("DMA 区域不存在".into())
        }
    }

    pub fn region_count(&self) -> usize {
        self.regions.len()
    }
}

// =====================================================================================
// 设备树 (DeviceTree)
// =====================================================================================

/// 设备树节点。
#[derive(Clone, Debug)]
pub struct DTNode {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    pub children: Vec<u64>,
    pub parent: Option<u64>,
}

/// 设备树：硬件拓扑描述。
pub struct DeviceTree {
    nodes: BTreeMap<u64, DTNode>,
    next_id: u64,
    pub root_id: Option<u64>,
}

impl DeviceTree {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            next_id: 1,
            root_id: None,
        }
    }

    pub fn add_node(&mut self, name: &str, parent: Option<u64>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if parent.is_none() {
            self.root_id = Some(id);
        }
        if let Some(pid) = parent {
            if let Some(parent_node) = self.nodes.get_mut(&pid) {
                parent_node.children.push(id);
            }
        }
        self.nodes.insert(
            id,
            DTNode {
                name: name.to_string(),
                properties: BTreeMap::new(),
                children: Vec::new(),
                parent,
            },
        );
        id
    }

    pub fn set_property(&mut self, node_id: u64, key: &str, value: &str) -> Result<(), String> {
        let node = self.nodes.get_mut(&node_id).ok_or("节点不存在")?;
        node.properties.insert(key.to_string(), value.to_string());
        Ok(())
    }

    pub fn get_property(&self, node_id: u64, key: &str) -> Option<&String> {
        self.nodes.get(&node_id).and_then(|n| n.properties.get(key))
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// =====================================================================================
// 热插拔管理器 (HotPlugManager)
// =====================================================================================

/// 热插拔事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotPlugEventType {
    Add,
    Remove,
}

/// 热插拔事件。
#[derive(Clone, Debug)]
pub struct HotPlugEvent {
    pub id: u64,
    pub device_id: u64,
    pub r#type: HotPlugEventType,
    pub timestamp_ns: u128,
}

/// 热插拔管理器：设备热插拔管理。
pub struct HotPlugManager {
    events: VecDeque<HotPlugEvent>,
    next_id: u64,
    pub max_events: usize,
    pub hotplug_count: u64,
}

impl HotPlugManager {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            next_id: 1,
            max_events: 100000,
            hotplug_count: 0,
        }
    }

    pub fn add_device(&mut self, device_id: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.events.push_back(HotPlugEvent {
            id,
            device_id,
            r#type: HotPlugEventType::Add,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        self.hotplug_count += 1;
        id
    }

    pub fn remove_device(&mut self, device_id: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.events.push_back(HotPlugEvent {
            id,
            device_id,
            r#type: HotPlugEventType::Remove,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
        self.hotplug_count += 1;
        id
    }

    pub fn poll(&mut self) -> Option<HotPlugEvent> {
        self.events.pop_front()
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

// =====================================================================================
// AI 原生驱动引擎 (DriverAI) —— 统一入口
// =====================================================================================

/// AI 原生驱动引擎：整合 AI 设备、AI 驱动、PCI 总线、中断控制器、DMA 区域、设备树、热插拔管理器。
pub struct DriverAI {
    pub devices: BTreeMap<u64, AIDevice>,
    pub drivers: BTreeMap<u64, AIDriver>,
    pub pci_bus: PCIBus,
    pub irq_controller: IRQController,
    pub dma_manager: DMARegionManager,
    pub device_tree: DeviceTree,
    pub hotplug: HotPlugManager,
    pub next_device_id: u64,
    pub next_driver_id: u64,
    pub enabled: bool,
    pub total_operations: u64,
}

impl DriverAI {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            drivers: BTreeMap::new(),
            pci_bus: PCIBus::new(),
            irq_controller: IRQController::new(),
            dma_manager: DMARegionManager::new(),
            device_tree: DeviceTree::new(),
            hotplug: HotPlugManager::new(),
            next_device_id: 1,
            next_driver_id: 1,
            enabled: true,
            total_operations: 0,
        }
    }

    pub fn register_device(&mut self, r#type: DeviceType, name: &str, vendor_id: u16, device_id: u16) -> u64 {
        let id = self.next_device_id;
        self.next_device_id += 1;
        let device = AIDevice::new(id, r#type, name, vendor_id, device_id);
        self.devices.insert(id, device);
        self.device_tree.add_node(name, None);
        self.total_operations += 1;
        self.hotplug.add_device(id);
        id
    }

    pub fn get_device(&self, id: u64) -> Option<&AIDevice> {
        self.devices.get(&id)
    }

    pub fn register_driver(&mut self, name: &str, device_type: DeviceType, vendor_id: u16, device_id: u16) -> u64 {
        let id = self.next_driver_id;
        self.next_driver_id += 1;
        let driver = AIDriver::new(id, name, device_type, vendor_id, device_id);
        self.drivers.insert(id, driver);
        self.total_operations += 1;
        id
    }

    pub fn load_driver(&mut self, id: u64) -> Result<(), String> {
        let driver = self.drivers.get_mut(&id).ok_or("驱动不存在")?;
        driver.load()?;
        Ok(())
    }

    pub fn init_driver(&mut self, id: u64) -> Result<(), String> {
        let driver = self.drivers.get_mut(&id).ok_or("驱动不存在")?;
        driver.init()?;
        Ok(())
    }

    pub fn attach_driver_to_device(&mut self, driver_id: u64, device_id: u64) -> Result<(), String> {
        let driver = self.drivers.get_mut(&driver_id).ok_or("驱动不存在")?;
        let device = self.devices.get_mut(&device_id).ok_or("设备不存在")?;
        driver.attach(device_id);
        device.attach_driver();
        self.total_operations += 1;
        Ok(())
    }

    pub fn set_device_irq(&mut self, device_id: u64, irq: u32) -> Result<(), String> {
        let device = self.devices.get_mut(&device_id).ok_or("设备不存在")?;
        device.set_irq(irq);
        self.irq_controller.register(IRQLine(irq), &device.name);
        self.total_operations += 1;
        Ok(())
    }

    pub fn allocate_dma(&mut self, device_id: u64, size: u64) -> Result<u64, String> {
        let id = self.dma_manager.alloc(device_id, size)?;
        if let Some(device) = self.devices.get_mut(&device_id) {
            device.set_dma(id);
        }
        self.total_operations += 1;
        Ok(id)
    }

    pub fn invoke_irq(&mut self, irq: u32) -> bool {
        self.irq_controller.invoke(IRQLine(irq))
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn driver_count(&self) -> usize {
        self.drivers.len()
    }

    pub fn pci_device_count(&self) -> usize {
        self.pci_bus.device_count()
    }

    pub fn irq_handler_count(&self) -> usize {
        self.irq_controller.handler_count()
    }

    pub fn dma_region_count(&self) -> usize {
        self.dma_manager.region_count()
    }

    pub fn device_tree_node_count(&self) -> usize {
        self.device_tree.node_count()
    }

    pub fn hotplug_count(&self) -> u64 {
        self.hotplug.hotplug_count
    }
}

impl Default for DriverAI {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================================
// 全局单例
// =====================================================================================

static DRIVER_AI: OnceLock<Mutex<DriverAI>> = OnceLock::new();

pub fn driver_ai() -> &'static Mutex<DriverAI> {
    DRIVER_AI.get_or_init(|| Mutex::new(DriverAI::new()))
}

// =====================================================================================
// 初始化
// =====================================================================================

pub fn init() {
    let _ = driver_ai();
}

// =====================================================================================
// 状态摘要
// =====================================================================================

pub fn status() -> String {
    let eng = driver_ai().lock().unwrap();
    format!(
        "driver_ai: devices={} drivers={} pci_devices={} irq_handlers={} dma_regions={} device_tree_nodes={} hotplug_events={} operations={}",
        eng.device_count(),
        eng.driver_count(),
        eng.pci_device_count(),
        eng.irq_handler_count(),
        eng.dma_region_count(),
        eng.device_tree_node_count(),
        eng.hotplug_count(),
        eng.total_operations
    )
}

pub fn driver_ai_status() -> String {
    status()
}

// =====================================================================================
// act 接口
// =====================================================================================

pub fn driver_ai_act(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("register-device") => {
            let type_str = it.next().unwrap_or("unknown");
            let r#type = match type_str {
                "storage" => DeviceType::Storage,
                "network" => DeviceType::Network,
                "display" => DeviceType::Display,
                "input" => DeviceType::Input,
                "audio" => DeviceType::Audio,
                "gpu" => DeviceType::GPU,
                "ai_accelerator" => DeviceType::AIAccelerator,
                "sensor" => DeviceType::Sensor,
                _ => DeviceType::Unknown,
            };
            let name = it.next().ok_or("缺少设备名")?;
            let vendor_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let device_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let mut eng = driver_ai().lock().unwrap();
            let id = eng.register_device(r#type, name, vendor_id, device_id);
            Ok(format!("已注册设备 id={} 名称={} 类型={:?}", id, name, r#type))
        }
        Some("register-driver") => {
            let name = it.next().ok_or("缺少驱动名")?;
            let type_str = it.next().unwrap_or("unknown");
            let r#type = match type_str {
                "storage" => DeviceType::Storage,
                "network" => DeviceType::Network,
                "display" => DeviceType::Display,
                "input" => DeviceType::Input,
                "audio" => DeviceType::Audio,
                "gpu" => DeviceType::GPU,
                "ai_accelerator" => DeviceType::AIAccelerator,
                "sensor" => DeviceType::Sensor,
                _ => DeviceType::Unknown,
            };
            let vendor_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let device_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let mut eng = driver_ai().lock().unwrap();
            let id = eng.register_driver(name, r#type, vendor_id, device_id);
            Ok(format!("已注册驱动 id={} 名称={} 类型={:?}", id, name, r#type))
        }
        Some("load-driver") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少驱动id")?;
            let mut eng = driver_ai().lock().unwrap();
            eng.load_driver(id)?;
            Ok(format!("已加载驱动 id={}", id))
        }
        Some("init-driver") => {
            let id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少驱动id")?;
            let mut eng = driver_ai().lock().unwrap();
            eng.init_driver(id)?;
            Ok(format!("已初始化驱动 id={}", id))
        }
        Some("attach") => {
            let driver_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少驱动id")?;
            let device_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少设备id")?;
            let mut eng = driver_ai().lock().unwrap();
            eng.attach_driver_to_device(driver_id, device_id)?;
            Ok(format!("已将驱动 {} 附加到设备 {}", driver_id, device_id))
        }
        Some("set-irq") => {
            let device_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少设备id")?;
            let irq = it.next().and_then(|s| s.parse::<u32>().ok()).ok_or("缺少 IRQ")?;
            let mut eng = driver_ai().lock().unwrap();
            eng.set_device_irq(device_id, irq)?;
            Ok(format!("已为设备 {} 设置 IRQ {}", device_id, irq))
        }
        Some("alloc-dma") => {
            let device_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少设备id")?;
            let size = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(4096);
            let mut eng = driver_ai().lock().unwrap();
            let id = eng.allocate_dma(device_id, size)?;
            Ok(format!("已为设备 {} 分配 DMA 区域 id={} 大小={}B", device_id, id, size))
        }
        Some("invoke-irq") => {
            let irq = it.next().and_then(|s| s.parse::<u32>().ok()).ok_or("缺少 IRQ")?;
            let mut eng = driver_ai().lock().unwrap();
            let handled = eng.invoke_irq(irq);
            Ok(format!("IRQ {} 处理结果: {}", irq, if handled { "已处理" } else { "未处理" }))
        }
        Some("pci-add") => {
            let bus = it.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let device = it.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let function = it.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let vendor_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let device_id = it.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
            let class_code = it.next().and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let mut eng = driver_ai().lock().unwrap();
            eng.pci_bus.add_device(PCIDevice {
                bus,
                device,
                function,
                vendor_id,
                device_id,
                class_code,
                subclass: 0,
                prog_if: 0,
                revision_id: 0,
                irq_pin: 0,
                irq_line: 0,
            });
            Ok(format!("已添加 PCI 设备 bus={} device={} function={}", bus, device, function))
        }
        Some("device-tree-add") => {
            let name = it.next().ok_or("缺少节点名")?;
            let mut eng = driver_ai().lock().unwrap();
            let id = eng.device_tree.add_node(name, None);
            Ok(format!("已添加设备树节点 id={} 名称={}", id, name))
        }
        Some("hotplug-add") => {
            let device_id = it.next().and_then(|s| s.parse::<u64>().ok()).ok_or("缺少设备id")?;
            let mut eng = driver_ai().lock().unwrap();
            let id = eng.hotplug.add_device(device_id);
            Ok(format!("已添加热插拔事件 id={} 设备={}", id, device_id))
        }
        Some("hotplug-poll") => {
            let mut eng = driver_ai().lock().unwrap();
            match eng.hotplug.poll() {
                Some(event) => Ok(format!("热插拔事件 id={} 设备={} 类型={:?}", event.id, event.device_id, event.r#type)),
                None => Ok("无热插拔事件".into()),
            }
        }
        Some("status") | None => Ok(status()),
        Some(other) => Err(format!("未知 driver_ai 动作: {}", other)),
    }
}

pub fn driver_ai_act_dispatch(payload: &str) -> Result<String, String> {
    driver_ai_act(payload)
}

// =====================================================================================
// 测试
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_creation() {
        let mut dev = AIDevice::new(1, DeviceType::GPU, "gpu0", 0x1234, 0x5678);
        assert_eq!(dev.id, 1);
        assert_eq!(dev.name, "gpu0");
        assert!(dev.online);
        assert!(!dev.driver_attached);
    }

    #[test]
    fn device_capability() {
        let mut dev = AIDevice::new(1, DeviceType::Storage, "disk0", 1, 1);
        dev.add_capability("nvme");
        dev.add_capability("encryption");
        assert!(dev.has_capability("nvme"));
        assert!(!dev.has_capability("raid"));
    }

    #[test]
    fn device_irq_dma() {
        let mut dev = AIDevice::new(1, DeviceType::Network, "net0", 1, 1);
        dev.set_irq(10);
        dev.set_dma(100);
        assert_eq!(dev.irq, Some(10));
        assert_eq!(dev.dma_region, Some(100));
    }

    #[test]
    fn driver_load_init() {
        let mut driver = AIDriver::new(1, "nvme_driver", DeviceType::Storage, 1, 1);
        assert_eq!(driver.state, DriverState::Unloaded);
        driver.load().unwrap();
        assert_eq!(driver.state, DriverState::Loaded);
        driver.init().unwrap();
        assert_eq!(driver.state, DriverState::Active);
    }

    #[test]
    fn driver_attach() {
        let mut driver = AIDriver::new(1, "driver1", DeviceType::Input, 1, 1);
        driver.attach(10);
        driver.attach(20);
        assert_eq!(driver.attached_devices.len(), 2);
        driver.detach(10);
        assert_eq!(driver.attached_devices.len(), 1);
    }

    #[test]
    fn driver_error() {
        let mut driver = AIDriver::new(1, "driver1", DeviceType::Input, 1, 1);
        driver.record_error();
        assert_eq!(driver.state, DriverState::Error);
        assert_eq!(driver.error_count, 1);
        driver.reset();
        assert_eq!(driver.state, DriverState::Loaded);
        assert_eq!(driver.error_count, 0);
    }

    #[test]
    fn pci_bus() {
        let mut pci = PCIBus::new();
        pci.add_device(PCIDevice { bus: 0, device: 0, function: 0, vendor_id: 0x8086, device_id: 0x1234, class_code: 1, subclass: 0, prog_if: 0, revision_id: 0, irq_pin: 0, irq_line: 0 });
        pci.add_device(PCIDevice { bus: 0, device: 1, function: 0, vendor_id: 0x8086, device_id: 0x5678, class_code: 2, subclass: 0, prog_if: 0, revision_id: 0, irq_pin: 0, irq_line: 0 });
        assert_eq!(pci.device_count(), 2);
        let found = pci.find_by_vendor_device(0x8086, 0x1234);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn irq_controller() {
        let mut ic = IRQController::new();
        let id = ic.register(IRQLine(10), "keyboard");
        assert_eq!(id, 1);
        assert!(ic.invoke(IRQLine(10)));
        assert_eq!(ic.total_interrupts, 1);
        ic.disable(IRQLine(10));
        assert!(!ic.invoke(IRQLine(10)));
        ic.enable(IRQLine(10));
        assert!(ic.invoke(IRQLine(10)));
    }

    #[test]
    fn dma_manager() {
        let mut dma = DMARegionManager::new();
        let id = dma.alloc(1, 4096).unwrap();
        assert_eq!(id, 1);
        assert_eq!(dma.region_count(), 1);
        dma.free(id).unwrap();
        assert_eq!(dma.region_count(), 0);
    }

    #[test]
    fn device_tree() {
        let mut dt = DeviceTree::new();
        let root = dt.add_node("root", None);
        let child = dt.add_node("child", Some(root));
        assert_eq!(root, 1);
        assert_eq!(child, 2);
        assert_eq!(dt.root_id(), Some(1));
        dt.set_property(child, "model", "x86").unwrap();
        assert_eq!(dt.get_property(child, "model"), Some(&"x86".to_string()));
        assert_eq!(dt.node_count(), 2);
    }

    #[test]
    fn hotplug_manager() {
        let mut hp = HotPlugManager::new();
        let id1 = hp.add_device(10);
        let id2 = hp.remove_device(10);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(hp.hotplug_count, 2);
        let event = hp.poll().unwrap();
        assert_eq!(event.r#type, HotPlugEventType::Add);
    }

    #[test]
    fn driver_ai_register() {
        let mut eng = DriverAI::new();
        let dev_id = eng.register_device(DeviceType::GPU, "gpu0", 0x1234, 0x5678);
        assert_eq!(dev_id, 1);
        let drv_id = eng.register_driver("gpu_driver", DeviceType::GPU, 0x1234, 0x5678);
        assert_eq!(drv_id, 1);
        assert_eq!(eng.device_count(), 1);
        assert_eq!(eng.driver_count(), 1);
    }

    #[test]
    fn driver_ai_load_init_attach() {
        let mut eng = DriverAI::new();
        let dev_id = eng.register_device(DeviceType::Storage, "disk0", 1, 1);
        let drv_id = eng.register_driver("nvme", DeviceType::Storage, 1, 1);
        eng.load_driver(drv_id).unwrap();
        eng.init_driver(drv_id).unwrap();
        eng.attach_driver_to_device(drv_id, dev_id).unwrap();
        let dev = eng.get_device(dev_id).unwrap();
        assert!(dev.driver_attached);
    }

    #[test]
    fn driver_ai_irq_dma() {
        let mut eng = DriverAI::new();
        let dev_id = eng.register_device(DeviceType::Network, "net0", 1, 1);
        eng.set_device_irq(dev_id, 11).unwrap();
        eng.invoke_irq(11);
        let dma_id = eng.allocate_dma(dev_id, 8192).unwrap();
        assert_eq!(dma_id, 1);
    }

    #[test]
    fn driver_ai_pci() {
        let mut eng = DriverAI::new();
        eng.pci_bus.add_device(PCIDevice { bus: 0, device: 0, function: 0, vendor_id: 0x8086, device_id: 0x1000, class_code: 1, subclass: 0, prog_if: 0, revision_id: 0, irq_pin: 0, irq_line: 0 });
        assert_eq!(eng.pci_device_count(), 1);
    }

    #[test]
    fn driver_ai_status_nonempty() {
        init();
        let s = status();
        assert!(s.contains("driver_ai:"));
        assert!(s.contains("devices="));
    }

    #[test]
    fn driver_ai_act_register_device() {
        init();
        let res = driver_ai_act("register-device gpu gpu0 0x1234 0x5678").unwrap();
        assert!(res.contains("已注册设备"));
    }

    #[test]
    fn driver_ai_act_register_driver() {
        init();
        let res = driver_ai_act("register-driver nvme storage 1 1").unwrap();
        assert!(res.contains("已注册驱动"));
    }

    #[test]
    fn driver_ai_act_load_init() {
        init();
        driver_ai_act("register-driver nvme storage 1 1").unwrap();
        let res = driver_ai_act("load-driver 1").unwrap();
        assert!(res.contains("已加载驱动"));
        let res = driver_ai_act("init-driver 1").unwrap();
        assert!(res.contains("已初始化驱动"));
    }

    #[test]
    fn driver_ai_act_attach() {
        init();
        driver_ai_act("register-device disk storage 1 1").unwrap();
        driver_ai_act("register-driver nvme storage 1 1").unwrap();
        driver_ai_act("load-driver 1").unwrap();
        driver_ai_act("init-driver 1").unwrap();
        let res = driver_ai_act("attach 1 1").unwrap();
        assert!(res.contains("附加到设备"));
    }

    #[test]
    fn driver_ai_act_set_irq() {
        init();
        driver_ai_act("register-device net network 1 1").unwrap();
        let res = driver_ai_act("set-irq 1 11").unwrap();
        assert!(res.contains("IRQ"));
    }

    #[test]
    fn driver_ai_act_alloc_dma() {
        init();
        driver_ai_act("register-device disk storage 1 1").unwrap();
        let res = driver_ai_act("alloc-dma 1 8192").unwrap();
        assert!(res.contains("DMA 区域"));
    }

    #[test]
    fn driver_ai_act_invoke_irq() {
        init();
        driver_ai_act("register-device kb input 1 1").unwrap();
        driver_ai_act("set-irq 1 10").unwrap();
        let res = driver_ai_act("invoke-irq 10").unwrap();
        assert!(res.contains("IRQ 10"));
    }

    #[test]
    fn driver_ai_act_pci() {
        init();
        let res = driver_ai_act("pci-add 0 0 0 0x8086 0x1234 1").unwrap();
        assert!(res.contains("PCI 设备"));
    }

    #[test]
    fn driver_ai_act_device_tree() {
        init();
        let res = driver_ai_act("device-tree-add root").unwrap();
        assert!(res.contains("设备树节点"));
    }

    #[test]
    fn driver_ai_act_hotplug() {
        init();
        driver_ai_act("register-device disk storage 1 1").unwrap();
        let res = driver_ai_act("hotplug-add 1").unwrap();
        assert!(res.contains("热插拔事件"));
        let res = driver_ai_act("hotplug-poll").unwrap();
        assert!(res.contains("热插拔事件"));
    }

    #[test]
    fn driver_ai_act_status() {
        init();
        let res = driver_ai_act("status").unwrap();
        assert!(res.contains("driver_ai:"));
    }

    #[test]
    fn device_types() {
        assert_eq!(DeviceType::Storage as u32, 0);
        assert_eq!(DeviceType::AIAccelerator as u32, 6);
        assert_eq!(DeviceType::Unknown as u32, 7);
    }

    #[test]
    fn driver_states() {
        assert_eq!(DriverState::Unloaded as u32, 0);
        assert_eq!(DriverState::Active as u32, 4);
        assert_eq!(DriverState::Error as u32, 5);
    }

    #[test]
    fn hotplug_event_types() {
        assert_eq!(HotPlugEventType::Add as u32, 0);
        assert_eq!(HotPlugEventType::Remove as u32, 1);
    }
}

// =====================================================================================
// 设备电源管理 (DevicePowerManagement)
// =====================================================================================

/// 电源状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    On,         // 开启
    Sleep,      // 休眠
    Standby,    // 待机
    Off,        // 关闭
}

/// 设备电源状态。
#[derive(Clone, Debug)]
pub struct DevicePowerState {
    pub device_id: u64,
    pub state: PowerState,
    pub power_watts: f32,
    pub last_transition: u128,
}

/// 设备电源管理器：管理设备电源状态。
pub struct DevicePowerManager {
    pub states: BTreeMap<u64, DevicePowerState>,
    pub total_power_watts: f32,
}

impl DevicePowerManager {
    pub fn new() -> Self {
        Self {
            states: BTreeMap::new(),
            total_power_watts: 0.0,
        }
    }

    pub fn set_power(&mut self, device_id: u64, state: PowerState, power_watts: f32) {
        let old_state = self.states.get(&device_id);
        let old_power = old_state.map(|s| s.power_watts).unwrap_or(0.0);
        self.total_power_watts = self.total_power_watts - old_power + power_watts;
        self.states.insert(device_id, DevicePowerState {
            device_id,
            state,
            power_watts,
            last_transition: crate::ai_os::native::now_ns(),
        });
    }

    pub fn get_power(&self, device_id: u64) -> Option<&DevicePowerState> {
        self.states.get(&device_id)
    }

    pub fn total_power(&self) -> f32 {
        self.total_power_watts
    }

    pub fn device_count(&self) -> usize {
        self.states.len()
    }
}

// =====================================================================================
// 驱动签名验证 (DriverSignatureVerification)
// =====================================================================================

/// 签名算法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Ed25519,
    ECDsaP256,
    Rsa4096,
}

/// 签名记录。
#[derive(Clone, Debug)]
pub struct DriverSignature {
    pub driver_id: u64,
    pub algorithm: SignatureAlgorithm,
    pub signature: Vec<u8>,
    pub signer: String,
    pub valid_until: u128,
    pub verified: bool,
}

/// 驱动签名验证器：验证驱动程序签名。
pub struct DriverSignatureVerifier {
    pub signatures: BTreeMap<u64, DriverSignature>,
    pub trusted_signers: BTreeSet<String>,
    pub next_id: u64,
}

impl DriverSignatureVerifier {
    pub fn new() -> Self {
        Self {
            signatures: BTreeMap::new(),
            trusted_signers: BTreeSet::new(),
            next_id: 1,
        }
    }

    pub fn add_trusted_signer(&mut self, signer: &str) {
        self.trusted_signers.insert(signer.to_string());
    }

    pub fn sign(&mut self, driver_id: u64, algorithm: SignatureAlgorithm, signer: &str, signature: Vec<u8>, valid_until: u128) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let verified = self.trusted_signers.contains(signer);
        self.signatures.insert(id, DriverSignature {
            driver_id,
            algorithm,
            signature,
            signer: signer.to_string(),
            valid_until,
            verified,
        });
        id
    }

    pub fn verify(&self, driver_id: u64) -> Option<bool> {
        self.signatures.values()
            .find(|s| s.driver_id == driver_id)
            .map(|s| s.verified)
    }

    pub fn is_trusted(&self, signer: &str) -> bool {
        self.trusted_signers.contains(signer)
    }

    pub fn signature_count(&self) -> usize {
        self.signatures.len()
    }
}

// =====================================================================================
// 设备健康监控 (DeviceHealthMonitor)
// =====================================================================================

/// 健康状态。
#[derive=Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,      // 健康
    Warning,      // 警告
    Critical,     // 危急
    Failed,       // 失败
}

/// 设备健康记录。
#[derive=Clone, Debug)]
pub struct DeviceHealthRecord {
    pub device_id: u64,
    pub status: HealthStatus,
    pub temperature_c: f32,
    pub error_count: u64,
    pub timestamp_ns: u128,
}

/// 设备健康监控器：监控设备健康状态。
pub struct DeviceHealthMonitor {
    pub records: BTreeMap<u64, DeviceHealthRecord>,
    pub alert_threshold: f32,
}

impl DeviceHealthMonitor {
    pub fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            alert_threshold: 80.0, // 摄氏度
        }
    }

    pub fn update(&mut self, device_id: u64, status: HealthStatus, temperature_c: f32, error_count: u64) {
        self.records.insert(device_id, DeviceHealthRecord {
            device_id,
            status,
            temperature_c,
            error_count,
            timestamp_ns: crate::ai_os::native::now_ns(),
        });
    }

    pub fn get_health(&self, device_id: u64) -> Option<&DeviceHealthRecord> {
        self.records.get(&device_id)
    }

    pub fn needs_attention(&self) -> Vec<u64> {
        self.records.iter()
            .filter(|(_, r)| r.status == HealthStatus::Critical || r.status == HealthStatus::Failed || r.temperature_c > self.alert_threshold)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

// =====================================================================================
// 固件更新 (FirmwareUpdate)
// =====================================================================================

/// 更新状态。
#[derive=Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Pending,      // 待处理
    Downloading,  // 下载中
    Installing,   // 安装中
    Completed,    // 完成
    Failed,       // 失败
}

/// 固件更新记录。
#[derive=Clone, Debug)]
pub struct FirmwareUpdate {
    pub id: u64,
    pub device_id: u64,
    pub version: String,
    pub status: UpdateStatus,
    pub progress: f32,   // 0~1
    pub started_at: u128,
    pub completed_at: Option<u128>,
}

/// 固件更新管理器：管理设备固件更新。
pub struct FirmwareUpdateManager {
    pub updates: BTreeMap<u64, FirmwareUpdate>,
    pub next_id: u64,
}

impl FirmwareUpdateManager {
    pub fn new() -> Self {
        Self {
            updates: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn schedule(&mut self, device_id: u64, version: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.updates.insert(id, FirmwareUpdate {
            id,
            device_id,
            version: version.to_string(),
            status: UpdateStatus::Pending,
            progress: 0.0,
            started_at: crate::ai_os::native::now_ns(),
            completed_at: None,
        });
        id
    }

    pub fn update_progress(&mut self, id: u64, progress: f32, status: UpdateStatus) -> bool {
        if let Some(update) = self.updates.get_mut(&id) {
            update.progress = progress.clamp(0.0, 1.0);
            update.status = status;
            if status == UpdateStatus::Completed || status == UpdateStatus::Failed {
                update.completed_at = Some(crate::ai_os::native::now_ns());
            }
            true
        } else {
            false
        }
    }

    pub fn get_update(&self, id: u64) -> Option<&FirmwareUpdate> {
        self.updates.get(&id)
    }

    pub fn update_count(&self) -> usize {
        self.updates.len()
    }

    pub fn completed_count(&self) -> usize {
        self.updates.values().filter(|u| u.status == UpdateStatus::Completed).count()
    }
}

// =====================================================================================
// 驱动扩展测试
// =====================================================================================

#[cfg(test)]
mod advanced_tests {
    use super::*;

    #[test]
    fn device_power_management() {
        let mut pm = DevicePowerManager::new();
        pm.set_power(1, PowerState::On, 10.0);
        pm.set_power(2, PowerState::Sleep, 1.0);
        assert_eq!(pm.device_count(), 2);
        assert!((pm.total_power() - 11.0).abs() < 1e-6);
        let state = pm.get_power(1).unwrap();
        assert_eq!(state.state, PowerState::On);
    }

    #[test]
    fn driver_signature() {
        let mut sv = DriverSignatureVerifier::new();
        sv.add_trusted_signer("vendor_a");
        let id = sv.sign(1, SignatureAlgorithm::Ed25519, "vendor_a", vec![1, 2, 3], 9999999999);
        assert_eq!(id, 1);
        assert!(sv.is_trusted("vendor_a"));
        assert!(!sv.is_trusted("vendor_b"));
        let verified = sv.verify(1).unwrap();
        assert!(verified);
        assert_eq!(sv.signature_count(), 1);
    }

    #[test]
    fn device_health_monitor() {
        let mut hm = DeviceHealthMonitor::new();
        hm.update(1, HealthStatus::Healthy, 45.0, 0);
        hm.update(2, HealthStatus::Critical, 95.0, 10);
        assert_eq!(hm.record_count(), 2);
        let health = hm.get_health(1).unwrap();
        assert_eq!(health.status, HealthStatus::Healthy);
        let attention = hm.needs_attention();
        assert!(attention.contains(&2));
        assert!(!attention.contains(&1));
    }

    #[test]
    fn firmware_update_manager() {
        let mut fum = FirmwareUpdateManager::new();
        let id = fum.schedule(1, "v2.0.0".into());
        assert_eq!(id, 1);
        fum.update_progress(id, 0.5, UpdateStatus::Installing).unwrap();
        fum.update_progress(id, 1.0, UpdateStatus::Completed).unwrap();
        let update = fum.get_update(id).unwrap();
        assert_eq!(update.status, UpdateStatus::Completed);
        assert!((update.progress - 1.0).abs() < 1e-6);
        assert_eq!(fum.update_count(), 1);
        assert_eq!(fum.completed_count(), 1);
    }

    #[test]
    fn power_states() {
        assert_eq!(PowerState::On as u32, 0);
        assert_eq!(PowerState::Off as u32, 3);
    }

    #[test]
    fn signature_algorithms() {
        assert_eq!(SignatureAlgorithm::Ed25519 as u32, 0);
        assert_eq!(SignatureAlgorithm::Rsa4096 as u32, 2);
    }

    #[test]
    fn health_statuses() {
        assert_eq!(HealthStatus::Healthy as u32, 0);
        assert_eq!(HealthStatus::Failed as u32, 3);
    }

    #[test]
    fn update_statuses() {
        assert_eq!(UpdateStatus::Pending as u32, 0);
        assert_eq!(UpdateStatus::Failed as u32, 4);
    }
}
