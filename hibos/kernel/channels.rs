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
    /// 交换分区 (swap)
    Swap,
    /// 多级页表
    PageTable,
    /// slab 对象缓存
    Slab,
    /// 伙伴系统页框分配器
    Buddy,
    /// AI 原生：模型原语（model://*）
    Model,
    /// AI 原生：上下文窗口/KV 缓存（ctx://*）
    Ctx,
    /// AI 原生：记忆文件系统/长期记忆（knowledge://*）
    Knowledge,
    /// AI 原生：工具调用层（tool://*）
    Tool,
    /// AI 原生：AI 原生 OS 总控状态（aios://*）
    Aios,
    /// AI 原生：安全沙箱与提示词注入防护（security://*）
    Security,
    /// AI 原生：死锁检测图（deadlock://*）
    Deadlock,
    /// AI 原生：模型容器沙箱（container://*）
    Container,
    /// AI 原生：意识原子层（native://*）
    Native,
    /// AI 原生：心智架构层（mind://*）
    Mind,
    /// AI 原生：注意力层（attention://*）
    Attention,
    /// AI 原生：语义空间层（semantic://*）
    Semantic,
    /// AI 原生：工作记忆层（memory://*）
    MemoryWs,
    /// AI 原生：身体图式层（body://*）
    BodySchema,
    /// AI 原生：动作空间层（action://*）
    ActionAi,
    /// AI 原生：时间处理层（temporal://*）
    Temporal,
    /// AI 原生：多智能体社会层（society://*）
    Social,
    /// AI 原生：伦理对齐层（ethics://*）
    Ethics,
    /// AI 原生：自我进化层（evolution://*）
    Evolution,
    /// AI 原生：离线巩固层（dream://*）
    Dream,
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
            ChannelKind::Swap => "swap",
            ChannelKind::PageTable => "pagetable",
            ChannelKind::Slab => "slab",
            ChannelKind::Buddy => "buddy",
            ChannelKind::Model => "model",
            ChannelKind::Ctx => "ctx",
            ChannelKind::Knowledge => "knowledge",
            ChannelKind::Tool => "tool",
            ChannelKind::Aios => "aios",
            ChannelKind::Security => "security",
            ChannelKind::Deadlock => "deadlock",
            ChannelKind::Container => "container",
            ChannelKind::Native => "native",
            ChannelKind::Mind => "mind",
            ChannelKind::Attention => "attention",
            ChannelKind::Semantic => "semantic",
            ChannelKind::MemoryWs => "memory",
            ChannelKind::BodySchema => "body",
            ChannelKind::ActionAi => "action",
            ChannelKind::Temporal => "temporal",
            ChannelKind::Social => "society",
            ChannelKind::Ethics => "ethics",
            ChannelKind::Evolution => "evolution",
            ChannelKind::Dream => "dream",
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
        (ChannelKind::Action, "exec://threads", Access::ReadWrite),
        (ChannelKind::Action, "sync://primitives", Access::ReadWrite),
        (ChannelKind::Agent, "agent://orchestrator", Access::ReadWrite),
        (ChannelKind::Agent, "agent://workers", Access::ReadWrite),
        (ChannelKind::Agent, "agent://results", Access::Read),
        (ChannelKind::Desktop, "desktop://screen", Access::Write),
        (ChannelKind::Desktop, "desktop://framebuffer", Access::Write),
        (ChannelKind::Desktop, "gui://windows", Access::ReadWrite),
        (ChannelKind::Desktop, "input://kbd", Access::ReadWrite),
        (ChannelKind::Desktop, "input://mouse", Access::ReadWrite),
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
        (ChannelKind::Memory, "lease://channels", Access::ReadWrite),
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
        // —— 子意识主体 / 进程容器 ——
        (ChannelKind::Process, "proc://spawn", Access::Write),
        (ChannelKind::Process, "proc://kill", Access::Write),
        (ChannelKind::Process, "proc://list", Access::Read),
        (ChannelKind::Process, "proc://wait_table", Access::Read),
        (ChannelKind::Process, "proc://status", Access::Read),
        // —— 命名互斥锁（跨进程/域可见）：sync://mutex/<名> ——
        (ChannelKind::Action, "sync://mutex", Access::ReadWrite),
        // —— 中断通道：irq://<设备> ——
        (ChannelKind::Device, "irq://timer", Access::ReadWrite),
        (ChannelKind::Device, "irq://keyboard", Access::ReadWrite),
        (ChannelKind::Device, "irq://gpu", Access::ReadWrite),
        // —— 内存页故障：mem://fault ——
        (ChannelKind::Memory, "mem://fault", Access::ReadWrite),
        // —— 电源唤醒源：power://wake-source ——
        (ChannelKind::Action, "power://wake-source", Access::ReadWrite),
        // —— 意识镜像加载器：loader://<镜像> ——
        (ChannelKind::Action, "loader://echo", Access::Write),
        (ChannelKind::Action, "loader://sense-loop", Access::Write),
        (ChannelKind::Action, "loader://act-loop", Access::Write),
        (ChannelKind::Action, "loader://idle", Access::Write),
        // —— 一致性 / 子意识 / 多域 / 保护 / 身份空间 / 桌面外壳 ——
        (ChannelKind::Will, "cell://self", Access::ReadWrite),
        (ChannelKind::Action, "power://hibernate", Access::ReadWrite),
        (ChannelKind::Action, "domain://table", Access::ReadWrite),
        (ChannelKind::Gpu, "gpu://domains", Access::ReadWrite),
        (ChannelKind::Gpu, "gpu://faults", Access::Read),
        (ChannelKind::Interface, "sec://acl", Access::ReadWrite),
        (ChannelKind::User, "realm://current", Access::Read),
        (ChannelKind::Desktop, "gui://shell", Access::ReadWrite),
        (ChannelKind::Desktop, "gui://wallpaper", Access::ReadWrite),
        // —— 多调度域（多内核）：sched://<域> ——
        (ChannelKind::Action, "sched://reflex", Access::ReadWrite),
        (ChannelKind::Action, "sched://perception", Access::ReadWrite),
        (ChannelKind::Action, "sched://deliberation", Access::ReadWrite),
        (ChannelKind::Action, "sched://memory", Access::ReadWrite),
        (ChannelKind::Action, "sched://governance", Access::ReadWrite),
        (ChannelKind::Action, "sched://domains", Access::Read),
        // —— 统一灵感核心：五个相位归于一个调度意志 ——
        (ChannelKind::Action, "sched://inspiration", Access::ReadWrite),
        // —— AI 原生 OS 通道：模型 / 上下文窗口 / 记忆 / 工具 / 安全 / 死锁 / 容器 / 总控 ——
        (ChannelKind::Model, "model://loader", Access::ReadWrite),
        (ChannelKind::Model, "model://instances", Access::Read),
        (ChannelKind::Model, "model://spec", Access::Read),
        (ChannelKind::Ctx, "ctx://kv", Access::Read),
        (ChannelKind::Ctx, "ctx://window", Access::Read),
        (ChannelKind::Knowledge, "knowledge://store", Access::ReadWrite),
        (ChannelKind::Knowledge, "knowledge://query", Access::Read),
        (ChannelKind::Tool, "tool://registry", Access::Read),
        (ChannelKind::Tool, "tool://exec", Access::Write),
        (ChannelKind::Aios, "aios://status", Access::Read),
        (ChannelKind::Security, "security://status", Access::Read),
        (ChannelKind::Security, "sec://caps", Access::ReadWrite),
        (ChannelKind::Deadlock, "deadlock://graph", Access::Read),
        (ChannelKind::Deadlock, "deadlock://wait", Access::ReadWrite),
        (ChannelKind::Container, "container://status", Access::Read),
        // —— AI 原生意识层通道：意识原子 / 心智 / 注意力 / 语义空间 / 工作记忆 ——
        (ChannelKind::Native, "native://qualia", Access::ReadWrite),
        (ChannelKind::Native, "native://stream", Access::Read),
        (ChannelKind::Native, "native://status", Access::Read),
        (ChannelKind::Mind, "mind://state", Access::ReadWrite),
        (ChannelKind::Mind, "mind://goal", Access::ReadWrite),
        (ChannelKind::Mind, "mind://wm", Access::ReadWrite),
        (ChannelKind::Mind, "mind://decide", Access::Write),
        (ChannelKind::Attention, "attention://lens", Access::ReadWrite),
        (ChannelKind::Attention, "attention://saliency", Access::Read),
        (ChannelKind::Attention, "attention://focus", Access::ReadWrite),
        (ChannelKind::Attention, "attention://cycle", Access::Read),
        (ChannelKind::Semantic, "semantic://index", Access::Write),
        (ChannelKind::Semantic, "semantic://search", Access::Read),
        (ChannelKind::Semantic, "semantic://concept", Access::ReadWrite),
        (ChannelKind::Semantic, "semantic://namespace", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://sensory", Access::Write),
        (ChannelKind::MemoryWs, "memory://wm", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://stm", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://ltm", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://episode", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://semantic", Access::ReadWrite),
        (ChannelKind::MemoryWs, "memory://skill", Access::ReadWrite),
        // —— AI 原生身体/动作/时间层通道：身体图式 / 动作空间 / 时间处理 ——
        (ChannelKind::BodySchema, "body://schema", Access::ReadWrite),
        (ChannelKind::BodySchema, "body://proprioception", Access::ReadWrite),
        (ChannelKind::BodySchema, "body://map", Access::Read),
        (ChannelKind::BodySchema, "body://kinematic", Access::ReadWrite),
        (ChannelKind::BodySchema, "body://state", Access::ReadWrite),
        (ChannelKind::ActionAi, "action://primitive", Access::ReadWrite),
        (ChannelKind::ActionAi, "action://command", Access::ReadWrite),
        (ChannelKind::ActionAi, "action://execute", Access::Write),
        (ChannelKind::ActionAi, "action://sequence", Access::ReadWrite),
        (ChannelKind::ActionAi, "action://constraint", Access::ReadWrite),
        (ChannelKind::Temporal, "temporal://clock", Access::Read),
        (ChannelKind::Temporal, "temporal://circadian", Access::Read),
        (ChannelKind::Temporal, "temporal://ultradian", Access::Read),
        (ChannelKind::Temporal, "temporal://sequence", Access::ReadWrite),
        (ChannelKind::Temporal, "temporal://schedule", Access::ReadWrite),
        (ChannelKind::Temporal, "temporal://memory", Access::ReadWrite),
        (ChannelKind::Temporal, "temporal://prediction", Access::ReadWrite),
        // —— AI 原生社会/伦理/进化/梦境层通道 ——
        (ChannelKind::Social, "society://register", Access::Write),
        (ChannelKind::Social, "society://send", Access::Write),
        (ChannelKind::Social, "society://poll", Access::Read),
        (ChannelKind::Social, "society://conversation", Access::ReadWrite),
        (ChannelKind::Social, "society://edge", Access::ReadWrite),
        (ChannelKind::Social, "society://negotiate", Access::Write),
        (ChannelKind::Social, "society://reputation", Access::Read),
        (ChannelKind::Ethics, "ethics://principle", Access::Write),
        (ChannelKind::Ethics, "ethics://evaluate", Access::ReadWrite),
        (ChannelKind::Ethics, "ethics://safety", Access::Write),
        (ChannelKind::Ethics, "ethics://rule", Access::Write),
        (ChannelKind::Ethics, "ethics://value", Access::Write),
        (ChannelKind::Ethics, "ethics://alignment", Access::Read),
        (ChannelKind::Ethics, "ethics://dilemma", Access::ReadWrite),
        (ChannelKind::Ethics, "ethics://audit", Access::Read),
        (ChannelKind::Evolution, "evolution://genome", Access::Write),
        (ChannelKind::Evolution, "evolution://gene", Access::Write),
        (ChannelKind::Evolution, "evolution://mutate", Access::Write),
        (ChannelKind::Evolution, "evolution://fitness", Access::ReadWrite),
        (ChannelKind::Evolution, "evolution://select", Access::Write),
        (ChannelKind::Evolution, "evolution://snapshot", Access::ReadWrite),
        (ChannelKind::Evolution, "evolution://adapt", Access::Write),
        (ChannelKind::Dream, "dream://cycle", Access::Write),
        (ChannelKind::Dream, "dream://stage", Access::Write),
        (ChannelKind::Dream, "dream://replay", Access::ReadWrite),
        (ChannelKind::Dream, "dream://synthesize", Access::Write),
        (ChannelKind::Dream, "dream://associate", Access::Write),
        (ChannelKind::Dream, "dream://insight", Access::Write),
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
        assert_eq!(reg.len(), 188);
        assert_eq!(reg.of_kind(ChannelKind::Net).count(), 5);
        assert_eq!(reg.of_kind(ChannelKind::Will).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::Context).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Action).count(), 23);
        assert_eq!(reg.of_kind(ChannelKind::Agent).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Gpu).count(), 6);
        assert_eq!(reg.of_kind(ChannelKind::Device).count(), 14);
        assert_eq!(reg.of_kind(ChannelKind::Memory).count(), 6);
        assert_eq!(reg.of_kind(ChannelKind::Process).count(), 8);
        assert_eq!(reg.of_kind(ChannelKind::User).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::App).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Browser).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::Driver).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Plugin).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Model).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Ctx).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Knowledge).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Tool).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Aios).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::Security).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::Deadlock).count(), 2);
        assert_eq!(reg.of_kind(ChannelKind::Container).count(), 1);
        assert_eq!(reg.of_kind(ChannelKind::Native).count(), 3);
        assert_eq!(reg.of_kind(ChannelKind::Mind).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::Attention).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::Semantic).count(), 4);
        assert_eq!(reg.of_kind(ChannelKind::MemoryWs).count(), 7);
        assert_eq!(reg.of_kind(ChannelKind::BodySchema).count(), 5);
        assert_eq!(reg.of_kind(ChannelKind::ActionAi).count(), 5);
        assert_eq!(reg.of_kind(ChannelKind::Temporal).count(), 7);
        assert_eq!(reg.of_kind(ChannelKind::Social).count(), 7);
        assert_eq!(reg.of_kind(ChannelKind::Ethics).count(), 8);
        assert_eq!(reg.of_kind(ChannelKind::Evolution).count(), 7);
        assert_eq!(reg.of_kind(ChannelKind::Dream).count(), 6);
        assert!(reg.find("file://vfs").is_some());
        assert!(reg.find("desktop://screen").is_some());
        assert!(reg.find("trace://exec").is_some());
        assert!(reg.find("mem://system").is_some());
        assert!(reg.find("nope://none").is_none());
        assert!(reg.find("model://loader").is_some());
        assert!(reg.find("ctx://kv").is_some());
        assert!(reg.find("knowledge://store").is_some());
        assert!(reg.find("tool://registry").is_some());
        assert!(reg.find("aios://status").is_some());
        assert!(reg.find("security://status").is_some());
        assert!(reg.find("deadlock://graph").is_some());
        assert!(reg.find("container://status").is_some());
        assert!(reg.find("native://qualia").is_some());
        assert!(reg.find("mind://state").is_some());
        assert!(reg.find("attention://lens").is_some());
        assert!(reg.find("semantic://search").is_some());
        assert!(reg.find("memory://wm").is_some());
        assert!(reg.find("body://schema").is_some());
        assert!(reg.find("action://primitive").is_some());
        assert!(reg.find("temporal://clock").is_some());
        assert!(reg.find("society://register").is_some());
        assert!(reg.find("ethics://principle").is_some());
        assert!(reg.find("evolution://genome").is_some());
        assert!(reg.find("dream://cycle").is_some());
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
