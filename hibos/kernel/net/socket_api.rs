//! POSIX 兼容 Socket API（kernel/net/socket_api.rs）。
//!
//! 本模块在 `kernel/net` 的 tcp/udp/ip 层之上提供 BSD 风格简化版 Socket API：
//! socket/bind/listen/accept/connect/send/recv/sendto/recvfrom/close/setsockopt/getsockopt。
//! 与现有 `kernel/net/socket.rs` 轻量 socket 表并存，面向更完整的 fd 表、
//! 阻塞/非阻模式与多协议族场景。
//!
//! 特性:
//! * SocketDomain：AF_INET / AF_UNIX
//! * SocketType：SOCK_STREAM(TCP) / SOCK_DGRAM(UDP) / SOCK_RAW
//! * Socket：socket / bind / listen / accept / connect / send / recv / sendto / recvfrom / close / setsockopt / getsockopt
//! * 文件描述符表（fd → socket 映射）
//! * 阻塞/非阻模式简化版
//! * sense/act 通道：`socket://tcp`（兼容现有）

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

// =======================================================================
// 地址类型
// ======================================================================

/// Socket 地址族。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketDomain {
    AfInet,   // IPv4
    AfUnix,   // 本地 Unix 域 socket
}

/// Socket 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketType {
    SockStream,   // TCP
    SockDgram,    // UDP
    SockRaw,      // 原始 IP
}

/// 网络端点（IPv4 + 端口）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InetAddr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl InetAddr {
    pub fn new(ip: [u8; 4], port: u16) -> Self { Self { ip, port } }

    /// 解析 `ip:port`。
    pub fn parse(s: &str) -> Result<Self, String> {
        let (ip_s, port_s) = s.rsplit_once(':')
            .ok_or_else(|| format!("地址格式应为 <ip>:<port>: {}", s))?;
        let ip = parse_ip4_api(ip_s)?;
        let port: u16 = port_s.parse().map_err(|_| format!("无效端口: {}", port_s))?;
        Ok(Self { ip, port })
    }

    pub fn fmt(&self) -> String {
        format!("{}.{}.{}.{}:{}", self.ip[0], self.ip[1], self.ip[2], self.ip[3], self.port)
    }
}

/// 通用 socket 地址（支持 IPv4 / Unix 路径）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SockAddr {
    Inet(InetAddr),
    Unix(String), // 路径名
}

impl SockAddr {
    pub fn fmt(&self) -> String {
        match self {
            SockAddr::Inet(a) => a.fmt(),
            SockAddr::Unix(p) => format!("unix:{}", p),
        }
    }
}

fn parse_ip4_api(s: &str) -> Result<[u8; 4], String> {
    let mut out = [0u8; 4];
    for (i, part) in s.split('.').enumerate().take(4) {
        out[i] = part.trim().parse::<u8>().map_err(|_| format!("无效 IPv4 地址: {}", s))?;
    }
    Ok(out)
}

// =======================================================================
// Socket 条目（TCP / UDP / 原始）
// ======================================================================

/// Socket 内部状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SockState {
    /// 初始，未绑定。
    Idle,
    /// 已绑定本地地址。
    Bound,
    /// 监听中（仅 STREAM）。
    Listening,
    /// 已连接（STREAM）或已关联对端（DGRAM）。
    Connected,
    /// 已关闭。
    Closed,
}

/// 单个 socket 的抽象。
struct SocketEntry {
    pub fd: SocketFd,
    pub domain: SocketDomain,
    pub ty: SocketType,
    pub protocol: u8,
    pub state: SockState,
    pub local: Option<SockAddr>,
    pub remote: Option<SockAddr>,
    /// TCP listen backlog（待 accept 的已完成连接 fd 队列）。
    pub backlog: u32,
    /// 接收缓冲区（已到达数据）。
    pub rx: VecDeque<Vec<u8>>,
    /// 接收源地址（与 rx 配对，DGRAM 用）。
    pub rx_src: VecDeque<SockAddr>,
    /// 发送缓冲区（待发送数据，简化为直接发送）。
    pub tx: VecDeque<Vec<u8>>,
    /// 阻塞/非阻。
    pub blocking: bool,
    /// 错误码（最近一次操作）。
    pub last_err: Option<i32>,
    /// 是否已标记可关闭（fd 表回收）。
    pub dead: bool,
}

/// 文件描述符。
pub type SocketFd = i32;

impl SocketEntry {
    fn new(fd: SocketFd, domain: SocketDomain, ty: SocketType, protocol: u8) -> Self {
        Self {
            fd, domain, ty, protocol,
            state: SockState::Idle,
            local: None, remote: None,
            backlog: 128,
            rx: VecDeque::new(),
            rx_src: VecDeque::new(),
            tx: VecDeque::new(),
            blocking: true,
            last_err: None,
            dead: false,
        }
    }

    /// 是否可读（有接收数据或已关闭）。
    pub fn readable(&self) -> bool {
        !self.rx.is_empty() || self.state == SockState::Closed
    }

    /// 是否可写（简化：非 Closed 即可写）。
    pub fn writable(&self) -> bool { self.state != SockState::Closed }
}

// =======================================================================
// Socket 选项
// ======================================================================

/// Socket 选项层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SockLevel {
    SolSocket,
    IpprotoTcp,
    IpprotoUdp,
}

/// Socket 选项名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SockOpt {
    SoReuseaddr,
    SoKeepalive,
    SoRcvbuf,
    SoSndbuf,
    SoBroadcast,
    TcpNodelay,
    Other(i32),
}

impl SockOpt {
    pub fn from_raw(level: i32, name: i32) -> Option<(SockLevel, SockOpt)> {
        let lvl = match level {
            1 => SockLevel::SolSocket,
            6 => SockLevel::IpprotoTcp,
            17 => SockLevel::IpprotoUdp,
            _ => return None,
        };
        let opt = match (level, name) {
            (1, 4) => SockOpt::SoReuseaddr,      // SO_REUSEADDR
            (1, 9) => SockOpt::SoKeepalive,      // SO_KEEPALIVE
            (1, 8) => SockOpt::SoRcvbuf,         // SO_RCVBUF
            (1, 7) => SockOpt::SoSndbuf,         // SO_SNDBUF
            (1, 6) => SockOpt::SoBroadcast,      // SO_BROADCAST
            (6, 1) => SockOpt::TcpNodelay,       // TCP_NODELAY
            _ => SockOpt::Other(name),
        };
        Some((lvl, opt))
    }
}

// =======================================================================
// Socket 表：fd -> socket（全局单例）
// ======================================================================

/// 文件描述符表：管理全部 socket 的生命周期。
pub struct SocketTable {
    sockets: HashMap<SocketFd, SocketEntry>,
    next_fd: SocketFd,
}

impl SocketTable {
    pub fn new() -> Self {
        Self { sockets: HashMap::new(), next_fd: 3 } // 从 3 开始（0/1/2 为 stdin/stdout/stderr）
    }

    fn alloc_fd(&mut self) -> SocketFd {
        let fd = self.next_fd;
        self.next_fd += 1;
        fd
    }

    pub fn len(&self) -> usize { self.sockets.len() }

    pub fn get(&self, fd: SocketFd) -> Option<&SocketEntry> { self.sockets.get(&fd) }

    pub fn get_mut(&mut self, fd: SocketFd) -> Option<&mut SocketEntry> { self.sockets.get_mut(&fd) }

    /// 插入 socket 并返回 fd。
    fn insert(&mut self, entry: SocketEntry) -> SocketFd {
        let fd = entry.fd;
        self.sockets.insert(fd, entry);
        fd
    }

    pub fn remove(&mut self, fd: SocketFd) -> Option<SocketEntry> { self.sockets.remove(&fd) }

    pub fn contains(&self, fd: SocketFd) -> bool { self.sockets.contains_key(&fd) }

    /// 全局 socket 表实例（`socket://tcp` 通道后端）。
    pub fn instance() -> &'static Mutex<SocketTable> {
        static INST: OnceLock<Mutex<SocketTable>> = OnceLock::new();
        INST.get_or_init(|| Mutex::new(Self::new()))
    }
}

// =======================================================================
// Socket API：公共函数
// ======================================================================

/// 创建一个 socket，返回文件描述符。
pub fn socket(domain: SocketDomain, ty: SocketType, protocol: u8) -> Result<SocketFd, String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let fd = st.alloc_fd();
    let entry = SocketEntry::new(fd, domain, ty, protocol);
    let _ = st.insert(entry);
    Ok(fd)
}

/// 绑定本地地址。
pub fn bind(fd: SocketFd, addr: &SockAddr) -> Result<(), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if entry.state != SockState::Idle {
        return Err("socket 已绑定或已连接".to_string());
    }
    entry.local = Some(addr.clone());
    entry.state = SockState::Bound;
    Ok(())
}

/// 监听（仅 STREAM）。
pub fn listen(fd: SocketFd, backlog: u32) -> Result<(), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if entry.ty != SocketType::SockStream {
        return Err("仅 SOCK_STREAM 可监听".to_string());
    }
    entry.state = SockState::Listening;
    entry.backlog = backlog;
    Ok(())
}

/// 接受一个连接，返回 (新 fd, 对端地址)。
pub fn accept(fd: SocketFd) -> Result<(SocketFd, SockAddr), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let (domain, local) = {
        let entry = st.get(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
        if entry.state != SockState::Listening {
            return Err("socket 未监听".to_string());
        }
        (entry.domain, entry.local.clone())
    };
    // 简化：模拟一个已连接 socket（实际应由 TCP 层驱动）。
    let new_fd = st.alloc_fd();
    let remote = SockAddr::Inet(InetAddr::new([10, 0, 0, 3], 50000));
    let mut accepted = SocketEntry::new(new_fd, domain, SocketType::SockStream, 6);
    accepted.state = SockState::Connected;
    accepted.remote = Some(remote.clone());
    accepted.local = local;
    let _ = st.insert(accepted);
    Ok((new_fd, remote))
}

/// 主动连接到远端。
pub fn connect(fd: SocketFd, addr: &SockAddr) -> Result<(), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    entry.remote = Some(addr.clone());
    entry.state = SockState::Connected;
    Ok(())
}

/// 发送数据（STREAM/Connected DGRAM，不指定目标）。
pub fn send(fd: SocketFd, buf: &[u8]) -> Result<usize, String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if entry.state != SockState::Connected {
        return Err("socket 未连接".to_string());
    }
    entry.tx.push_back(buf.to_vec());
    Ok(buf.len())
}

/// 接收数据。
pub fn recv(fd: SocketFd, buf: &mut [u8]) -> Result<usize, String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if let Some(data) = entry.rx.pop_front() {
        let take = buf.len().min(data.len());
        buf[..take].copy_from_slice(&data[..take]);
        Ok(take)
    } else if entry.state == SockState::Closed {
        Ok(0) // EOF
    } else {
        Err("无可用数据".to_string())
    }
}

/// 向指定地址发送数据报（DGRAM/RAW）。
pub fn sendto(fd: SocketFd, buf: &[u8], addr: &SockAddr) -> Result<usize, String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if entry.ty != SocketType::SockDgram && entry.ty != SocketType::SockRaw {
        return Err("仅 SOCK_DGRAM/SOCK_RAW 可用 sendto".to_string());
    }
    entry.remote = Some(addr.clone());
    entry.tx.push_back(buf.to_vec());
    Ok(buf.len())
}

/// 从接收队列取一个数据报，返回 (字节数, 源地址)。
pub fn recvfrom(fd: SocketFd, buf: &mut [u8]) -> Result<(usize, SockAddr), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    if let Some(data) = entry.rx.pop_front() {
        let src = entry.rx_src.pop_front().unwrap_or(SockAddr::Inet(InetAddr::new([0, 0, 0, 0], 0)));
        let take = buf.len().min(data.len());
        buf[..take].copy_from_slice(&data[..take]);
        Ok((take, src))
    } else if entry.state == SockState::Closed {
        Ok((0, SockAddr::Inet(InetAddr::new([0, 0, 0, 0], 0))))
    } else {
        Err("无可用数据报".to_string())
    }
}

/// 关闭 socket，释放 fd。
pub fn close(fd: SocketFd) -> Result<(), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    entry.state = SockState::Closed;
    entry.dead = true;
    st.remove(fd);
    Ok(())
}

/// 设置 socket 选项。
pub fn setsockopt(fd: SocketFd, level: i32, optname: i32, optval: &[u8]) -> Result<(), String> {
    let _ = optval;
    let (lvl, opt) = SockOpt::from_raw(level, optname).ok_or_else(|| "无效 socket 选项层/名称".to_string())?;
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    let _ = (entry, lvl, opt);
    Ok(())
}

/// 获取 socket 选项。
pub fn getsockopt(fd: SocketFd, level: i32, optname: i32) -> Result<Vec<u8>, String> {
    let (_lvl, _opt) = SockOpt::from_raw(level, optname).ok_or_else(|| "无效 socket 选项层/名称".to_string())?;
    let _st = SocketTable::instance().lock().unwrap();
    // 简化：返回空值。
    Ok(Vec::new())
}

/// 设置阻塞/非阻模式。
pub fn set_blocking(fd: SocketFd, blocking: bool) -> Result<(), String> {
    let mut st = SocketTable::instance().lock().unwrap();
    let entry = st.get_mut(fd).ok_or_else(|| format!("fd {} 不存在", fd))?;
    entry.blocking = blocking;
    Ok(())
}

/// 查询 socket 数量（供统计）。
pub fn socket_count() -> usize {
    let st = SocketTable::instance().lock().unwrap();
    st.len()
}

/// 查询 fd 是否存在。
pub fn fd_exists(fd: SocketFd) -> bool {
    let st = SocketTable::instance().lock().unwrap();
    st.contains(fd)
}

// =======================================================================
// sense/act 接口（socket://tcp）
// ======================================================================

/// 查询 socket 状态快照（供 `sense socket://tcp`）。
pub fn socket_api_sense_snapshot() -> String {
    let st = SocketTable::instance().lock().unwrap();
    format!("socket fds={}", st.len())
}

/// 对 socket 通道执行行动（供 `act socket://tcp`）。负载：`stats` / `list`。
pub fn socket_api_act_raw(payload: &str) -> Result<String, String> {
    let mut it = payload.split_whitespace();
    match it.next() {
        Some("stats") => Ok(socket_api_sense_snapshot()),
        Some("list") => {
            let st = SocketTable::instance().lock().unwrap();
            let mut out = String::new();
            for (fd, entry) in &st.sockets {
                out.push_str(&format!("  fd={} ty={:?} state={:?} local={:?} remote={:?}\n", fd, entry.ty, entry.state, entry.local, entry.remote));
            }
            Ok(out)
        }
        _ => Err("用法: act socket://tcp stats | list".to_string()),
    }
}

// =======================================================================
// 单元测试
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_bind_listen_accept() {
        let fd = socket(SocketDomain::AfInet, SocketType::SockStream, 6).unwrap();
        assert!(fd >= 3);
        assert!(bind(fd, &SockAddr::Inet(InetAddr::new([10, 0, 0, 2], 8080))).is_ok());
        assert!(listen(fd, 128).is_ok());
        let (cfd, remote) = accept(fd).unwrap();
        assert!(cfd >= 3);
        assert!(matches!(remote, SockAddr::Inet(_)));
        assert!(close(fd).is_ok());
        assert!(close(cfd).is_ok());
    }

    #[test]
    fn connect_send_recv() {
        let fd = socket(SocketDomain::AfInet, SocketType::SockStream, 6).unwrap();
        assert!(bind(fd, &SockAddr::Inet(InetAddr::new([10, 0, 0, 2], 0))).is_ok());
        assert!(connect(fd, &SockAddr::Inet(InetAddr::new([10, 0, 0, 3], 9000))).is_ok());
        let n = send(fd, b"hello").unwrap();
        assert_eq!(n, 5);
        // 接收缓冲为空。
        let mut buf = [0u8; 16];
        assert!(recv(fd, &mut buf).is_err());
        assert!(close(fd).is_ok());
    }

    #[test]
    fn sendto_recvfrom_dgram() {
        let fd = socket(SocketDomain::AfInet, SocketType::SockDgram, 17).unwrap();
        let addr = SockAddr::Inet(InetAddr::new([10, 0, 0, 3], 9001));
        let n = sendto(fd, b"ping", &addr).unwrap();
        assert_eq!(n, 4);
        // 接收：无数据报。
        let mut buf = [0u8; 16];
        assert!(recvfrom(fd, &mut buf).is_err());
        assert!(close(fd).is_ok());
    }

    #[test]
    fn setsockopt_getsockopt() {
        let fd = socket(SocketDomain::AfInet, SocketType::SockStream, 6).unwrap();
        assert!(setsockopt(fd, 1, 4, &[1]).is_ok()); // SO_REUSEADDR
        let val = getsockopt(fd, 1, 4).unwrap();
        assert_eq!(val.len(), 0); // 简化返回空
        assert!(close(fd).is_ok());
    }

    #[test]
    fn invalid_fd_errors() {
        assert!(bind(999, &SockAddr::Inet(InetAddr::new([10,0,0,2], 80))).is_err());
        assert!(send(999, b"x").is_err());
        assert!(recv(999, &mut [0u8; 4]).is_err());
        assert!(close(999).is_err());
    }

    #[test]
    fn socket_count() {
        let _ = socket(SocketDomain::AfInet, SocketType::SockStream, 6).unwrap();
        let _ = socket(SocketDomain::AfInet, SocketType::SockDgram, 17).unwrap();
        assert!(socket_count() >= 2);
    }

    #[test]
    fn set_blocking() {
        let fd = socket(SocketDomain::AfInet, SocketType::SockStream, 6).unwrap();
        assert!(set_blocking(fd, false).is_ok());
        assert!(set_blocking(fd, true).is_ok());
        assert!(close(fd).is_ok());
    }

    #[test]
    fn addr_parse_and_fmt() {
        let a = InetAddr::parse("10.0.0.2:8080").unwrap();
        assert_eq!(a.ip, [10, 0, 0, 2]);
        assert_eq!(a.port, 8080);
        assert_eq!(a.fmt(), "10.0.0.2:8080");
        assert!(InetAddr::parse("bad").is_err());
    }

    #[test]
    fn sockopt_from_raw() {
        assert_eq!(SockOpt::from_raw(1, 4), Some((SockLevel::SolSocket, SockOpt::SoReuseaddr)));
        assert_eq!(SockOpt::from_raw(6, 1), Some((SockLevel::IpprotoTcp, SockOpt::TcpNodelay)));
        assert_eq!(SockOpt::from_raw(99, 1), None);
    }

    #[test]
    fn sense_and_act() {
        let snap = socket_api_sense_snapshot();
        assert!(snap.contains("fds="));
        assert!(socket_api_act_raw("stats").is_ok());
        assert!(socket_api_act_raw("list").is_ok());
    }
}