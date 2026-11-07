//! 统一感知通道 —— 宏内核的「统一层」。
//!
//! 设计信条：**模型上下文、行动、文件、显存、设备，对 AI 而言都是同构的通道。**
//! AI 不需要知道对面是张量、目录还是 IOMMU 寄存器，它只做两件事：
//!
//! * `sense` —— 从通道读取 (感知)
//! * `act`   —— 向通道写入 (行动)
//!
//! 与传统统一内存 (Apple Unified Memory) 只抹平 CPU/GPU 地址空间的差异不同，
//! 这里抹平的是**语义层**：上下文、动作、文件、硬件共用同一套 `sense/act` 接口、
//! 同一套权限模型 (Read/Write/ReadWrite) 与同一套可观测计数器。

/// 通道类别 —— AI 的全部感官入口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// 模型上下文 / 概念本体
    Context,
    /// 系统调用 (行动)
    Action,
    /// 文件系统
    File,
    /// 内存 / 系统资源
    Memory,
    /// 应用
    App,
    /// 浏览器 / 外部页面
    Browser,
    /// 驱动 (内核模块)
    Driver,
    /// 插件
    Plugin,
    /// 智能体 (多智能体编排)
    Agent,
    /// 桌面 / 屏幕
    Desktop,
    /// 路径 / 工作目录
    Path,
    /// 标准 IO
    Io,
    /// 网关 (外部世界入口)
    Gateway,
    /// 接口 / ABI
    Interface,
    /// 显存 / GPU
    Gpu,
    /// 设备节点
    Device,
    /// 文本 / 日志
    Text,
    /// 执行轨迹
    Trace,
    /// 网络接口
    Net,
    /// 进程
    Process,
    /// 用户 / 身份
    User,
    /// 意志 / 动机与信念
    Will,
}

impl ChannelKind {
    pub fn name(self) -> &'static str {
        match self {
            ChannelKind::Context => "context",
            ChannelKind::Action => "action",
            ChannelKind::File => "file",
            ChannelKind::Memory => "mem",
            ChannelKind::App => "app",
            ChannelKind::Browser => "browser",
            ChannelKind::Driver => "driver",
            ChannelKind::Plugin => "plugin",
            ChannelKind::Net => "net",
            ChannelKind::Agent => "agent",
            ChannelKind::Desktop => "desktop",
            ChannelKind::Path => "path",
            ChannelKind::Io => "io",
            ChannelKind::Gateway => "gateway",
            ChannelKind::Interface => "interface",
            ChannelKind::Gpu => "gpu",
            ChannelKind::Device => "device",
            ChannelKind::Text => "text",
            ChannelKind::Trace => "trace",
            ChannelKind::Process => "process",
            ChannelKind::User => "user",
            ChannelKind::Will => "will",
        }
    }
}

/// 访问权限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
}

impl Access {
    pub fn can_read(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }

    pub fn label(self) -> &'static str {
        match self {
            Access::Read => "r",
            Access::Write => "w",
            Access::ReadWrite => "rw",
        }
    }
}

/// 一个感知通道。
#[derive(Clone, Debug)]
pub struct Channel {
    pub id: u64,
    pub kind: ChannelKind,
    pub name: String,
    pub access: Access,
    /// 被感知次数。
    pub senses: u64,
    /// 被写入次数。
    pub acts: u64,
}

/// 通道错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelError {
    NotFound,
    NotReadable,
    NotWritable,
    AlreadyOpen,
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ChannelError::NotFound => "通道不存在",
            ChannelError::NotReadable => "通道不可读",
            ChannelError::NotWritable => "通道不可写",
            ChannelError::AlreadyOpen => "通道已打开",
        };
        write!(f, "{}", s)
    }
}

/// 通道注册表。
pub struct ChannelRegistry {
    channels: Vec<Channel>,
    next_id: u64,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            next_id: 1,
        }
    }

    /// 打开一个通道；同名通道已存在则返回 `AlreadyOpen`。
    pub fn open(
        &mut self,
        kind: ChannelKind,
        name: &str,
        access: Access,
    ) -> Result<u64, ChannelError> {
        if self.channels.iter().any(|c| c.name == name) {
            return Err(ChannelError::AlreadyOpen);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.channels.push(Channel {
            id,
            kind,
            name: name.to_string(),
            access,
            senses: 0,
            acts: 0,
        });
        Ok(id)
    }

    /// 关闭通道。
    pub fn close(&mut self, name: &str) -> Result<(), ChannelError> {
        let pos = self
            .channels
            .iter()
            .position(|c| c.name == name)
            .ok_or(ChannelError::NotFound)?;
        self.channels.remove(pos);
        Ok(())
    }

    pub fn find(&self, name: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// 记录一次感知；返回通道类别供上层路由。
    pub fn sense(&mut self, name: &str) -> Result<ChannelKind, ChannelError> {
        let ch = self
            .channels
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or(ChannelError::NotFound)?;
        if !ch.access.can_read() {
            return Err(ChannelError::NotReadable);
        }
        ch.senses += 1;
        Ok(ch.kind)
    }

    /// 记录一次行动；返回通道类别供上层路由。
    pub fn act(&mut self, name: &str) -> Result<ChannelKind, ChannelError> {
        let ch = self
            .channels
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or(ChannelError::NotFound)?;
        if !ch.access.can_write() {
            return Err(ChannelError::NotWritable);
        }
        ch.acts += 1;
        Ok(ch.kind)
    }

    /// 按类别筛选。
    pub fn of_kind(&self, kind: ChannelKind) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(move |c| c.kind == kind)
    }

    pub fn list(&self) -> &[Channel] {
        &self.channels
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// 总感知量 (跨通道)。
    pub fn total_senses(&self) -> u64 {
        self.channels.iter().map(|c| c.senses).sum()
    }

    /// 总行动量 (跨通道)。
    pub fn total_acts(&self) -> u64 {
        self.channels.iter().map(|c| c.acts).sum()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 打开内核默认通道集合。
///
/// 这张表就是「AI 的感官清单」：上下文、行动、文件、智能体、桌面、路径、
/// IO、网关、接口、显存、设备、文本、轨迹。
pub fn bootstrap() -> ChannelRegistry {
    let mut reg = ChannelRegistry::new();
    let defaults = [
        (ChannelKind::Context, "context://ontology", Access::ReadWrite),
        (ChannelKind::Context, "context://model", Access::ReadWrite),
        (ChannelKind::Action, "action://syscall", Access::Write),
        (ChannelKind::Action, "action://scheduler", Access::Read),
        (ChannelKind::Agent, "agent://orchestrator", Access::ReadWrite),
        (ChannelKind::Agent, "agent://workers", Access::ReadWrite),
        (ChannelKind::Desktop, "desktop://screen", Access::Write),
        (ChannelKind::Desktop, "desktop://framebuffer", Access::Write),
        (ChannelKind::Path, "path://cwd", Access::ReadWrite),
        (ChannelKind::Io, "io://stdin", Access::Read),
        (ChannelKind::Io, "io://stdout", Access::Write),
        (ChannelKind::Gateway, "gateway://shell", Access::ReadWrite),
        (ChannelKind::Gateway, "gateway://net", Access::ReadWrite),
        (ChannelKind::Interface, "interface://abi", Access::Read),
        (ChannelKind::Text, "text://log", Access::Write),
        (ChannelKind::Trace, "trace://exec", Access::ReadWrite),
        (ChannelKind::File, "file://vfs", Access::ReadWrite),
        (ChannelKind::Memory, "mem://system", Access::ReadWrite),
        (ChannelKind::Gpu, "gpu://vram", Access::ReadWrite),
        (ChannelKind::Gpu, "gpu://framebuffer", Access::Write),
        (ChannelKind::Gpu, "gpu://capabilities", Access::Read),
        (ChannelKind::Gpu, "gpu://scheduler", Access::Read),
        (ChannelKind::Device, "dev://iommu0", Access::ReadWrite),
        (ChannelKind::Device, "dev://gpu0", Access::ReadWrite),
        (ChannelKind::Device, "dev://timer", Access::Read),
        (ChannelKind::Device, "dev://console", Access::ReadWrite),
        (ChannelKind::Device, "dev://disk", Access::ReadWrite),
        (ChannelKind::Device, "dev://null", Access::ReadWrite),
        (ChannelKind::Device, "dev://monitor", Access::Read),
        (ChannelKind::App, "app://shell", Access::ReadWrite),
        (ChannelKind::App, "app://procs", Access::ReadWrite),
        (ChannelKind::Browser, "browser://gateway", Access::ReadWrite),
        (ChannelKind::Driver, "driver://gpu", Access::ReadWrite),
        (ChannelKind::Driver, "driver://iommu", Access::ReadWrite),
        (ChannelKind::Plugin, "plugin://modules", Access::ReadWrite),
        (ChannelKind::Plugin, "plugin://hotreload", Access::ReadWrite),
        (ChannelKind::Net, "net://eth0", Access::ReadWrite),
        // —— AI 原生扩展通道：进程 / 内存 / 权限 / 身份 / 真实网络 ——
        (ChannelKind::Process, "proc://manager", Access::ReadWrite),
        (ChannelKind::Memory, "mem://ram", Access::ReadWrite),
        (ChannelKind::Memory, "mem://mmap", Access::ReadWrite),
        (ChannelKind::File, "fs://meta", Access::ReadWrite),
        (ChannelKind::User, "user://db", Access::ReadWrite),
        (ChannelKind::Net, "socket://tcp", Access::ReadWrite),
        (ChannelKind::Net, "net://dns", Access::ReadWrite),
        // —— 其余核心缺口：IPC / 安全 / 设备树 / init / 时间 / 包管理 / 作业控制 ——
        (ChannelKind::Io, "ipc://pipe", Access::ReadWrite),
        (ChannelKind::Memory, "ipc://shm", Access::ReadWrite),
        (ChannelKind::Interface, "ipc://msgq", Access::ReadWrite),
        (ChannelKind::Interface, "ipc://sem", Access::ReadWrite),
        (ChannelKind::Interface, "sec://caps", Access::ReadWrite),
        (ChannelKind::Device, "dev://tree", Access::ReadWrite),
        (ChannelKind::Action, "init://services", Access::ReadWrite),
        (ChannelKind::Device, "time://clock", Access::Read),
        (ChannelKind::Device, "time://cron", Access::ReadWrite),
        (ChannelKind::App, "pkg://store", Access::ReadWrite),
        (ChannelKind::Process, "tty://jobs", Access::ReadWrite),
        // —— 系统设置类：sysctl / 身份 / 环境 / 限制 / 防火墙 / cgroup / 电源 ——
        (ChannelKind::Interface, "sysctl://kernel", Access::ReadWrite),
        (ChannelKind::Interface, "config://system", Access::ReadWrite),
        (ChannelKind::Io, "env://process", Access::ReadWrite),
        (ChannelKind::Action, "limits://process", Access::ReadWrite),
        (ChannelKind::Net, "fw://rules", Access::ReadWrite),
        (ChannelKind::Action, "cgroup://groups", Access::ReadWrite),
        (ChannelKind::Action, "power://state", Access::ReadWrite),
        // —— 第二批系统模块：挂载表 / procfs / 多网卡 / 设备总线 ——
        (ChannelKind::File, "mount://table", Access::ReadWrite),
        (ChannelKind::Process, "proc://sysinfo", Access::ReadWrite),
        (ChannelKind::Net, "net://interfaces", Access::ReadWrite),
        (ChannelKind::Device, "dev://bus", Access::ReadWrite),
        // —— 意志层：动机 / 信念 / 判断力 (三个分工配合的独立通道) ——
        (ChannelKind::Will, "motive://self", Access::ReadWrite),
        (ChannelKind::Will, "belief://self", Access::ReadWrite),
        (ChannelKind::Will, "judgment://self", Access::ReadWrite),
    ];
    for (kind, name, access) in defaults {
        let _ = reg.open(kind, name, access);
    }
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_opens_all_channels() {
        let reg = bootstrap();
        assert_eq!(reg.len(), 69);
        assert_eq!(reg.of_kind(ChannelKind::Net).count(), 5);
        assert_eq!(reg.of_kind(ChannelKind::Will).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Context).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Action).count(), 6);
        assert_eq!(reg.of_kind(ChannelKind::Agent).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Gpu).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::Device).count(), 11);
        assert_eq!(reg.of_kind(ChannelKind::Memory).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::Process).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::User).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::App).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Browser).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::Driver).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Plugin).count(), 2);
        assert!(reg.find("file://vfs").is_some());
        assert!(reg.find("desktop://screen").is_some());
        assert!(reg.find("trace://exec").is_some());
        assert!(reg.find("mem://system").is_some());
        assert!(reg.find("nope://none").is_none());
    }

    #[test]
    fn sense_and_act_counters() {
        let mut reg = bootstrap();
        assert_eq!(reg.sense("file://vfs").unwrap(), ChannelKind::File);
        assert_eq!(reg.sense("file://vfs").unwrap(), ChannelKind::File);
        reg.act("file://vfs").unwrap();
        let ch = reg.find("file://vfs").unwrap();
        assert_eq!(ch.senses, 2);
        assert_eq!(ch.acts, 1);
        assert_eq!(reg.total_senses(), 2);
        assert_eq!(reg.total_acts(), 1);
    }

    #[test]
    fn permissions_enforced() {
        let mut reg = bootstrap();
        assert_eq!(
            reg.sense("action://syscall").unwrap_err(),
            ChannelError::NotReadable
        );
        assert_eq!(
            reg.act("action://scheduler").unwrap_err(),
            ChannelError::NotWritable
        );
        assert_eq!(reg.sense("missing").unwrap_err(), ChannelError::NotFound);
        assert_eq!(reg.act("missing").unwrap_err(), ChannelError::NotFound);
    }

    #[test]
    fn open_close_and_duplicates() {
        let mut reg = ChannelRegistry::new();
        let id = reg.open(ChannelKind::File, "file://tmp", Access::Read).unwrap();
        assert_eq!(id, 1);
        assert_eq!(
            reg.open(ChannelKind::File, "file://tmp", Access::Read).unwrap_err(),
            ChannelError::AlreadyOpen
        );
        assert!(reg.find("file://tmp").unwrap().access.can_read());
        reg.close("file://tmp").unwrap();
        assert!(reg.find("file://tmp").is_none());
        assert_eq!(reg.close("file://tmp").unwrap_err(), ChannelError::NotFound);
    }

    #[test]
    fn access_helpers() {
        assert!(Access::Read.can_read() && !Access::Read.can_write());
        assert!(Access::Write.can_write() && !Access::Write.can_read());
        assert!(Access::ReadWrite.can_read() && Access::ReadWrite.can_write());
        assert!(Access::Write.can_write() && !Access::Write.can_read());
        assert_eq!(Access::ReadWrite.label(), "rw");
    }
}
