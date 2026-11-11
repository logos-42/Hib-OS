//! 网卡抽象层 —— 为后续接入真实 NIC（TUN/TAP、PCIe 网卡、宿主网桥）预留钩子。
//!
//! 当前提供两种实现：
//! * `LoopbackDevice`：环回设备，把发出的帧送回自己的接收队列，适用于单元测试和无 NIC 环境；
//! * `TapDevice`：TUN/TAP 骨架，方法签名已就绪，尚未接入宿主 TUN fd。

use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU64, Ordering};

/// 原生网卡能力：收发原始字节（以太网帧或预封装的 IP 包，取决于链路模式）。
pub trait NetDevice: Send + Sync {
    fn name(&self) -> &str;
    /// 链路 MTU。
    fn mtu(&self) -> u16 { 1500 }
    /// 把原始字节送出；返回写入字节数。
    fn send(&mut self, bytes: &[u8]) -> Result<usize, String>;
    /// 从接收队列取一段原始字节；队列为空返回 None。
    fn recv(&mut self) -> Option<Vec<u8>>;
    /// 累计发送字节（供统计）。
    fn tx_bytes(&self) -> u64 { 0 }
    /// 累计接收字节（供统计）。
    fn rx_bytes(&self) -> u64 { 0 }
}

#[derive(Default)]
struct DeviceStats {
    tx_bytes: AtomicU64,
    rx_bytes: AtomicU64,
}

/// 环回设备：`send` 直接写入自己的 `rx` 队列，模拟一根把 TX 连回 RX 的回环线，
/// 协议栈发出的帧会被 poll() 再次送上协议栈，无需真实网卡。
pub struct LoopbackDevice {
    name: String,
    mtu: u16,
    rx: VecDeque<Vec<u8>>,
    stats: DeviceStats,
}

impl LoopbackDevice {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mtu: 1500,
            rx: VecDeque::new(),
            stats: DeviceStats::default(),
        }
    }

    /// 由外部（协议栈 poll）把已发出的帧注入回接收队列，模拟线缆回环。
    pub fn inject_tx_echo(&mut self, bytes: Vec<u8>) {
        self.rx.push_back(bytes);
    }
}

impl NetDevice for LoopbackDevice {
    fn name(&self) -> &str { &self.name }
    fn mtu(&self) -> u16 { self.mtu }
    fn send(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let n = bytes.len();
        self.stats.tx_bytes.fetch_add(n as u64, Ordering::Relaxed);
        self.rx.push_back(bytes.to_vec());
        Ok(n)
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        let b = self.rx.pop_front()?;
        self.stats.rx_bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        Some(b)
    }
    fn tx_bytes(&self) -> u64 { self.stats.tx_bytes.load(Ordering::Relaxed) }
    fn rx_bytes(&self) -> u64 { self.stats.rx_bytes.load(Ordering::Relaxed) }
}

/// TUN/TAP 骨架：留给后续接入真实宿主 TUN/TAP 接口的钩子。
pub struct TapDevice {
    name: String,
    mtu: u16,
}

impl TapDevice {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mtu: 1500,
        }
    }

    /// TODO: 打开宿主 TUN/TAP 设备（`/dev/net/tun` 或 Windows TAP Adapter），
    /// 将读/写循环挂在内核线程上，帧收发经此设备进出协议栈。
    pub fn open(_ifname: &str) -> Result<Self, String> {
        Err("TUN/TAP 尚未接入：请先在 TapDevice::open 中绑定宿主 TUN fd".to_string())
    }
}

impl NetDevice for TapDevice {
    fn name(&self) -> &str { &self.name }
    fn mtu(&self) -> u16 { self.mtu }
    fn send(&mut self, _bytes: &[u8]) -> Result<usize, String> {
        Err("TUN/TAP 尚未接入".to_string())
    }
    fn recv(&mut self) -> Option<Vec<u8>> { None }
}