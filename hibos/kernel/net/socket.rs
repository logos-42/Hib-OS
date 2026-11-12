//! BSD 风格 socket API，建立于 kernel/net 的 tcp/udp/ip 层之上。
//!
//! 提供 socket/bind/connect/accept/recv/send/close，并与 `socket://tcp`
//! 感知通道对齐（见 kernel/channels.rs 的 `socket://tcp` 通道注册以及
//! kernel/sense.rs 的 socket://tcp 路由，与旧版 kernel/socket.rs 基于主机栈
//! 的实现并存）。NetStack 上的 poll/process_frame/frame_and_queue 等底层方法
//! 定义在 kernel/net/mod.rs 中。

use crate::net::{net, NetStack, IP_PROTO_UDP};
use crate::net::tcp::{self, TcpConnKey};

pub type SocketFd = u32;

/// Socket 类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketType {
    Tcp,
    Udp,
}

/// 端点地址（<ip>:<port>）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketAddr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl SocketAddr {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (ip_s, port_s) = s
            .rsplit_once(':')
            .ok_or_else(|| format!("地址格式应为 <ip>:<port>: {}", s))?;
        let ip = crate::net::parse_ip(ip_s);
        let port: u16 = port_s
            .parse()
            .map_err(|_| format!("无效端口: {}", port_s))?;
        Ok(Self { ip, port })
    }
}

/// Socket 表项。
enum SockEntry {
    Tcp { key: TcpConnKey },
    Udp { port: u16, connected: Option<SocketAddr> },
}

/// Socket 描述符表。
pub struct SocketTable {
    sockets: BTreeMap<SocketFd, SockEntry>,
    next_fd: SocketFd,
}

use std::collections::BTreeMap;

impl SocketTable {
    pub fn new() -> Self {
        Self { sockets: BTreeMap::new(), next_fd: 1 }
    }

    fn alloc_fd(&mut self) -> SocketFd {
        let fd = self.next_fd;
        self.next_fd += 1;
        fd
    }
}

impl NetStack {
    // ---------------- 协议栈内部 socket 操作（由 socket.rs 的公共函数锁全局锁后调用） ----------------

    pub fn sock_socket(&mut self, ty: SocketType) -> Result<SocketFd, String> {
        let fd = self.sockets.alloc_fd();
        match ty {
            SocketType::Tcp => {
                self.sockets.sockets.insert(fd, SockEntry::Tcp {
                    key: TcpConnKey { local_port: 0, remote_ip: [0, 0, 0, 0], remote_port: 0 },
                });
            }
            SocketType::Udp => {
                self.sockets.sockets.insert(fd, SockEntry::Udp { port: 0, connected: None });
            }
        }
        Ok(fd)
    }

    pub fn sock_bind(&mut self, fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
        let our_ip = self.iface.ip;
        match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => {
                if key.local_port != 0 { return Err("已绑定".to_string()); }
                self.tcp.bind(our_ip, addr.port)?;
                *key = TcpConnKey { local_port: addr.port, remote_ip: [0, 0, 0, 0], remote_port: 0 };
                Ok(())
            }
            SockEntry::Udp { port, .. } => {
                if *port != 0 { return Err("已绑定".to_string()); }
                self.udp.bind(addr.port)?;
                *port = addr.port;
                Ok(())
            }
        }
    }

    pub fn sock_listen(&mut self, fd: SocketFd, _backlog: usize) -> Result<(), String> {
        match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => self.tcp.listen(key.local_port),
            SockEntry::Udp { .. } => Err("UDP 不支持 listen".to_string()),
        }
    }

    /// 主动打开 TCP 连接：发 SYN，以环回方式驱动握手，直到 ESTABLISHED 或超时。
    pub fn sock_connect(&mut self, fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
        let key = match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => {
                if key.local_port == 0 { return Err("未绑定本地端口".to_string()); }
                key.clone()
            }
            SockEntry::Udp { .. } => return Err("UDP 使用 connect 限定对端，请用 sock_udp_connect".to_string()),
        };
        let segments = self.tcp.connect(key.local_port, addr.ip, addr.port)?;
        for seg in segments {
            self.enqueue_tcp_segment(addr.ip, &seg)?;
        }
        // 记录对端地址
        if let SockEntry::Tcp { ref mut key } = self.sockets.sockets.get_mut(&fd).unwrap() {
            key.remote_ip = addr.ip;
            key.remote_port = addr.port;
        }
        // 环回驱动握手（无真实 NIC 时，发出的段会经 poll() 送回本机）
        for _ in 0..16 {
            if self.tcp.state_of(&TcpConnKey {
                local_port: key.local_port, remote_ip: addr.ip, remote_port: addr.port,
            }) == Some(tcp::TcpState::Established) {
                return Ok(());
            }
            self.poll();
        }
        Err(format!("connect {:?} 超时", addr))
    }

    /// UDP connect：限定对端（后续 send/recv 仅与该对端）。
    pub fn sock_udp_connect(&mut self, fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
        match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Udp { port, connected } => {
                if *port == 0 { return Err("未绑定本地端口".to_string()); }
                *connected = Some(addr.clone());
                Ok(())
            }
            SockEntry::Tcp { .. } => Err("TCP 请用 sock_connect".to_string()),
        }
    }

    pub fn sock_accept(&mut self, fd: SocketFd) -> Result<(SocketFd, SocketAddr), String> {
        let (key, remote_ip, remote_port) = match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => self.tcp.accept(key.local_port)?,
            SockEntry::Udp { .. } => return Err("UDP 不支持 accept".to_string()),
        };
        let fd2 = self.sockets.alloc_fd();
        self.sockets.sockets.insert(fd2, SockEntry::Tcp { key });
        Ok((fd2, SocketAddr { ip: remote_ip, port: remote_port }))
    }

    pub fn sock_recv(&mut self, fd: SocketFd, buf: &mut [u8]) -> Result<usize, String> {
        match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => {
                let data = self.tcp.recv(key, buf.len())?;
                buf[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            SockEntry::Udp { port, .. } => {
                let data = self.udp.recv(*port, buf.len())?;
                buf[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
        }
    }

    pub fn sock_send(&mut self, fd: SocketFd, buf: &[u8]) -> Result<usize, String> {
        match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => {
                let remote_ip = key.remote_ip;
                let segments = self.tcp.send(key, buf)?;
                for seg in segments {
                    self.enqueue_tcp_segment(remote_ip, &seg)?;
                }
                self.poll();
                Ok(buf.len())
            }
            SockEntry::Udp { port, connected } => {
                if *port == 0 { return Err("未绑定本地端口".to_string()); }
                let remote = connected.as_mut().ok_or_else(|| "UDP 未连接对端".to_string())?;
                let (rip, rport) = (remote.ip, remote.port);
                let our_ip = self.iface.ip;
                let datagram = self.udp.send(our_ip, *port, rip, rport, buf)?;
                let packets = self.ip.transmit(rip, IP_PROTO_UDP, &datagram, self.iface.mtu as usize)?;
                self.frame_and_queue(rip, packets)?;
                self.poll();
                Ok(buf.len())
            }
        }
    }

    pub fn sock_close(&mut self, fd: SocketFd) -> Result<(), String> {
        let key = match self.sockets.sockets.get_mut(&fd).ok_or_else(|| "socket 不存在".to_string())? {
            SockEntry::Tcp { key } => {
                let segments = self.tcp.close(&key)?;
                let local_port = key.local_port;
                let remote_ip = key.remote_ip;
                let remote_port = key.remote_port;
                for seg in segments {
                    self.enqueue_tcp_segment(remote_ip, &seg)?;
                }
                self.poll();
                TcpConnKey { local_port, remote_ip, remote_port }
            }
            SockEntry::Udp { .. } => TcpConnKey { local_port: 0, remote_ip: [0, 0, 0, 0], remote_port: 0 },
        };
        if key.local_port != 0 {
            self.tcp.sockets.remove(&key);
        }
        self.sockets.sockets.remove(&fd);
        Ok(())
    }
}

/// 公共 socket API（锁全局协议栈后调用）。

pub fn socket(ty: SocketType) -> Result<SocketFd, String> {
    net().lock().sock_socket(ty)
}

pub fn bind(fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
    net().lock().sock_bind(fd, addr)
}

pub fn listen(fd: SocketFd, backlog: usize) -> Result<(), String> {
    net().lock().sock_listen(fd, backlog)
}

pub fn connect(fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
    net().lock().sock_connect(fd, addr)
}

pub fn udp_connect(fd: SocketFd, addr: &SocketAddr) -> Result<(), String> {
    net().lock().sock_udp_connect(fd, addr)
}

pub fn accept(fd: SocketFd) -> Result<(SocketFd, SocketAddr), String> {
    net().lock().sock_accept(fd)
}

pub fn recv(fd: SocketFd, buf: &mut [u8]) -> Result<usize, String> {
    net().lock().sock_recv(fd, buf)
}

pub fn send(fd: SocketFd, buf: &[u8]) -> Result<usize, String> {
    net().lock().sock_send(fd, buf)
}

pub fn close(fd: SocketFd) -> Result<(), String> {
    net().lock().sock_close(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{EthernetFrame, ETH_TYPE_IPV4, ICMP_ECHO_REPLY, IP_PROTO_ICMP, IcmpPacket, Ipv4Packet};

    fn test_ip() -> [u8; 4] { [10, 0, 0, 2] }
    fn peer_ip() -> [u8; 4] { [10, 0, 0, 3] }

    fn reset_stack() {
        let mut st = net().lock();
        *st = NetStack::default();
        st.iface.ip = test_ip();
        st.iface.mask = [255, 255, 255, 0];
        st.iface.link_up = true;
    }

    fn stack_guard() -> crate::ksync::SpinLockGuard<'static, NetStack> {
        net().lock()
    }

    #[test]
    fn tcp_socket_full_duplex_loopback() {
        reset_stack();
        let sfd = socket(SocketType::Tcp).unwrap();
        bind(sfd, &SocketAddr { ip: test_ip(), port: 8080 }).unwrap();
        listen(sfd, 16).unwrap();

        let cfd = socket(SocketType::Tcp).unwrap();
        bind(cfd, &SocketAddr { ip: test_ip(), port: 50001 }).unwrap();
        // 对自身地址连接（环回），connect 内部驱动三次握手直到 ESTABLISHED
        connect(cfd, &SocketAddr { ip: test_ip(), port: 8080 }).unwrap();

        let (afd, addr) = accept(sfd).unwrap();
        assert_eq!(addr, SocketAddr { ip: test_ip(), port: 50001 });

        send(cfd, b"hello from client").unwrap();
        let mut buf = [0u8; 64];
        let n = recv(afd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello from client");

        send(afd, b"hello from server").unwrap();
        let mut buf = [0u8; 64];
        let n = recv(cfd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello from server");

        close(cfd).unwrap();
        close(afd).unwrap();
        close(sfd).unwrap();
        reset_stack();
    }

    #[test]
    fn udp_socket_send_recv_loopback() {
        reset_stack();
        let sfd = socket(SocketType::Udp).unwrap();
        bind(sfd, &SocketAddr { ip: test_ip(), port: 5000 }).unwrap();

        let cfd = socket(SocketType::Udp).unwrap();
        bind(cfd, &SocketAddr { ip: test_ip(), port: 5001 }).unwrap();
        udp_connect(cfd, &SocketAddr { ip: test_ip(), port: 5000 }).unwrap();
        send(cfd, b"hello udp").unwrap();

        // 把客户端发出的帧作为对端数据报送回协议栈
        let frame = {
            let mut st = stack_guard();
            assert_eq!(st.outbound_len(), 1);
            st.pop_outbound().unwrap()
        };
        {
            let mut st = stack_guard();
            assert!(st.process_frame(&frame).is_ok());
        }

        let mut buf = [0u8; 64];
        let n = recv(sfd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello udp");

        close(sfd).unwrap();
        close(cfd).unwrap();
        reset_stack();
    }

    #[test]
    fn udp_send_without_peer_fails() {
        reset_stack();
        let fd = socket(SocketType::Udp).unwrap();
        bind(fd, &SocketAddr { ip: test_ip(), port: 5002 }).unwrap();
        assert!(send(fd, b"x").is_err()); // 未 udp_connect 对端
        close(fd).unwrap();
        reset_stack();
    }

    #[test]
    fn socket_errors() {
        reset_stack();
        assert!(bind(999, &SocketAddr { ip: test_ip(), port: 80 }).is_err()); // 不存在的 fd
        let fd = socket(SocketType::Tcp).unwrap();
        assert!(recv(fd, &mut [0; 8]).is_err()); // 未建立连接
        assert!(close(fd).is_ok());
        assert!(close(fd).is_err()); // 重复 close
        reset_stack();
    }

    #[test]
    fn send_ip_icmp_echo_loopback() {
        reset_stack();
        {
            let mut st = stack_guard();
            assert!(st.send_ip(test_ip(), IP_PROTO_ICMP, b"ping!").is_ok());
            assert_eq!(st.outbound_len(), 1);
        }
        // 把 Echo Request 帧送回协议栈，应收到 Echo Reply
        let frame = {
            let mut st = stack_guard();
            st.pop_outbound().unwrap()
        };
        {
            let mut st = stack_guard();
            assert!(st.process_frame(&frame).is_ok());
            assert_eq!(st.outbound_len(), 1);
        }
        let reply = {
            let mut st = stack_guard();
            st.pop_outbound().unwrap()
        };
        let eth = EthernetFrame::parse(&reply).unwrap();
        let pkt = Ipv4Packet::parse(&eth.payload).unwrap();
        assert_eq!(pkt.header.protocol, IP_PROTO_ICMP);
        let icmp = IcmpPacket::parse(&pkt.payload).unwrap();
        assert_eq!(icmp.icmp_type, ICMP_ECHO_REPLY);
        assert_eq!(icmp.payload, b"ping!");
        reset_stack();
    }

    #[test]
    fn tcp_send_then_poll_delivers_loopback() {
        reset_stack();
        let sfd = socket(SocketType::Tcp).unwrap();
        bind(sfd, &SocketAddr { ip: test_ip(), port: 8080 }).unwrap();
        listen(sfd, 16).unwrap();
        let cfd = socket(SocketType::Tcp).unwrap();
        bind(cfd, &SocketAddr { ip: test_ip(), port: 50001 }).unwrap();
        connect(cfd, &SocketAddr { ip: test_ip(), port: 8080 }).unwrap();
        let (afd, _) = accept(sfd).unwrap();

        send(cfd, b"loopback data").unwrap();
        let mut buf = [0u8; 64];
        let n = recv(afd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"loopback data");
        close(cfd).unwrap();
        close(afd).unwrap();
        close(sfd).unwrap();
        reset_stack();
    }
}

