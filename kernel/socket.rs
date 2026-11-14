//! 真实网络 I/O —— AI 原生「与外部世界对话」。
//!
//! DNS 解析与 TCP 连接直接落到**主机真实网络栈**（`std::net`）。
//! 感知通道：`socket/tcp`（连接/读写）、`net/dns`（域名解析）。

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub struct NetIo {
    /// 最近一次网络动作的结果快照（供 sense 感知）。
    pub last: String,
}

impl NetIo {
    fn new() -> Self {
        Self { last: "（尚未发起网络动作）".to_string() }
    }

    /// 真实 DNS 解析。
    pub fn resolve(&mut self, host: &str) -> Result<String, String> {
        let addrs: Vec<String> = (host, 0)
            .to_socket_addrs()
            .map_err(|e| format!("DNS 解析失败: {}", e))?
            .map(|a| a.ip().to_string())
            .collect();
        if addrs.is_empty() {
            return Err(format!("DNS 无结果: {}", host));
        }
        let out = format!("{} -> {}", host, addrs.join(", "));
        self.last = out.clone();
        Ok(out)
    }

    /// 真实 TCP 连接（可选发送并读取应答）。
    pub fn connect(&mut self, hostport: &str, data: Option<&str>) -> Result<String, String> {
        use std::io::{Read, Write};

        let addr = hostport
            .to_socket_addrs()
            .map_err(|e| format!("地址解析失败: {}", e))?
            .next()
            .ok_or_else(|| "无可用地址".to_string())?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map_err(|e| format!("连接失败: {}", e))?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();

        let msg = if let Some(d) = data {
            let mut w = &stream;
            w.write_all(d.as_bytes()).map_err(|e| format!("写入失败: {}", e))?;
            let mut buf = [0u8; 512];
            let mut r = &stream;
            let n = r.read(&mut buf).unwrap_or(0);
            format!("已发送 {}B, 收到 {}B: {}", d.len(), n, String::from_utf8_lossy(&buf[..n]))
        } else {
            "已建立 TCP 连接".to_string()
        };
        self.last = msg.clone();
        Ok(msg)
    }
}

static NETIO: OnceLock<Mutex<NetIo>> = OnceLock::new();

/// 全局真实网络 I/O 状态。
pub fn netio() -> &'static Mutex<NetIo> {
    NETIO.get_or_init(|| Mutex::new(NetIo::new()))
}
