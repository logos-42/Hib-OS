# Hib OS

GPU-Centric 宏内核原型：把系统资源（文件、显存、设备、网络、桌面…）统一建模为"通道"的操作系统内核，并内置 **AI 意识本体闭环**（感知 → 思考 → 行动）。

## 构建

```sh
cargo build
```

## 运行

默认进入 **AI 闭环模式（loop）**：AI 自动感知系统、思考，并自动执行回复中的 `ACTION:` 指令（覆盖全部已注册通道）。

运行需要 DeepSeek API Key，通过环境变量提供：

```sh
# PowerShell
$env:DEEPSEEK_API_KEY = "sk-你的key"
# CMD
set DEEPSEEK_API_KEY=sk-你的key

cargo run
```

未设置 Key 时，会退回普通命令行 shell（`help` 查看命令，`exit` 退出）。

## 统一感知层

内核里所有资源都是**通道**，地址形如 `<scheme>://<resource>`，对外只有两个动词：

| 动词 | 作用 | 需要的权限 |
|---|---|---|
| `sense <通道>` | 读取通道的实时快照 | 可读 (r) |
| `act <通道> [负载...]` | 向通道写入或触发一次操作 | 可写 (w) |

`channels` 命令列出全部 66 个通道、类别、读写权限以及感知/行动计数。

## 系统模块速查

下列通道都实现了真实的读写逻辑（不是占位符），可直接在 shell 里试验。

### 内核与 AI

| 通道 | 示例 | 说明 |
|---|---|---|
| `context://ontology` | `sense context://ontology` | 概念本体数量 |
| `action://scheduler` | `sense action://scheduler` | 调度器就绪队列 |
| `gpu://vram` | `sense gpu://vram` | 显存总量 / 已用 / 空闲 |
| `trace://exec` | `sense trace://exec` | 执行轨迹统计 |

### 文件系统

| 通道 | 示例 | 说明 |
|---|---|---|
| `file://vfs` | `act file://vfs hello` | 追加一行到 `/var/log/channel.log` |
| `fs://meta` | `act fs://meta chmod 755 /etc` | 权限 / 属主 / 链接 / 挂载元数据 |
| `mount://table` | `act mount://table mount tmpfs /data tmpfs ro` | 挂载表（见下节） |

### 进程、环境与资源

| 通道 | 示例 | 说明 |
|---|---|---|
| `proc://manager` | `act proc://manager spawn echo hi` | 真实派生主机进程、发信号 |
| `proc://sysinfo` | `sense proc://sysinfo` | procfs 整机快照（见下节） |
| `env://process` | `act env://process set K V` | 进程环境变量 `set/get/unset` |
| `limits://process` | `act limits://process set nofile 4096` | 资源限制 `nofile/nproc/...` |
| `cgroup://groups` | `act cgroup://groups create ai` | 控制组配额与进程挂载 |
| `tty://jobs` | `act tty://jobs fg 3` | 作业控制 |

### 网络

| 通道 | 示例 | 说明 |
|---|---|---|
| `net://eth0` | `act net://eth0 ping 10.0.0.2` | 协议栈：ping/send/arp/route/link |
| `net://interfaces` | `act net://interfaces up wlan0` | 网卡资产：IP/掩码/MTU/链路（见下节） |
| `net://dns` | `act net://dns resolve example.com` | DNS 解析 |
| `socket://tcp` | `act socket://tcp connect host:80 hi` | TCP 连接 |
| `fw://rules` | `act fw://rules policy DROP` | 防火墙规则与默认策略 |

### 设备

| 通道 | 示例 | 说明 |
|---|---|---|
| `dev://tree` | `act dev://tree attach gpu0` | 设备节点（主次设备号、驱动） |
| `dev://bus` | `act dev://bus rescan` | 总线枚举、驱动绑定、热插拔（见下节） |
| `dev://iommu0` | `sense dev://iommu0` | IOMMU 域 / 映射数量 |

### 系统设置

| 通道 | 示例 | 说明 |
|---|---|---|
| `sysctl://kernel` | `act sysctl://kernel set vm.swappiness 10` | 内核参数 |
| `config://system` | `act config://system set hostname ai-node` | 主机身份（hostname/locale/timezone） |
| `power://state` | `act power://state suspend` | 电源姿态与历史 |
| `pkg://store` | `act pkg://store install curl` | 包管理 |
| `user://db` | `act user://db adduser alice` | 用户与身份 |
| `sec://caps` | `act sec://caps grant CAP_NET_ADMIN` | 能力位 |

## 第二批系统模块（本轮新增）

### `mount://table` —— 挂载表

管理"哪个源挂到哪个目标、什么类型、只读还是可写"。初始含 5 条：`/`、`/tmp`、`/proc`、`/sys`、`/dev/gpu`。

```sh
sense mount://table                                  # 列出全部挂载点
act mount://table mount tmpfs /data tmpfs ro         # 源 目标 [类型] [ro|rw]
act mount://table remount /data rw                   # 切换只读/读写
act mount://table umount /data                       # 卸载
```

约束：目标必须是绝对路径；目标已被挂载则报错；根 `/` 不允许卸载。

### `proc://sysinfo` —— procfs 整机快照

一次 `sense` 拿到版本、运行时间、负载、内存、显存、Token 统计。

```sh
sense proc://sysinfo            # 只读快照
act proc://sysinfo refresh      # 递增采样计数后返回快照
```

输出示例：

```
version: Hib OS 0.1.0 (GPU-Centric macro-kernel)
uptime: 12ms (12345678ns)
loadavg: 0.5 0.4 0.3 (procs=1, running=1)
meminfo: total=16384MiB used=0MiB free=16384MiB
vram: total=16384MiB used=0MiB free=16384MiB
tokens: total=0 active=0 completed=0 failed=0 avg=0ns
procfs: refreshes=1
```

### `net://interfaces` —— 多网卡管理

`net://eth0` 是协议栈视角，本通道是网卡资产视角（`lo`、`eth0`、`wlan0`）。修改 `eth0` 会**同步到协议栈**，ping 立即使用新地址。

```sh
sense net://interfaces
act net://interfaces up wlan0                        # 启通链路
act net://interfaces down wlan0                      # 断开链路
act net://interfaces addr eth0 10.0.0.9/24           # 设置 IP 与前缀
act net://interfaces mtu eth0 1400                   # MTU 必须 68-65535
act net://interfaces add eth1 02:00:00:00:00:11      # 添加网卡
act net://interfaces del eth1                        # 删除网卡（lo 不可删）
```

### `dev://bus` —— 设备总线

`dev://tree` 管设备节点，本通道管总线枚举：PCI 地址、厂商/设备号、驱动绑定、热插拔。

```sh
sense dev://bus
act dev://bus bind 00:05.0 console                   # 绑定驱动
act dev://bus unbind 00:05.0                         # 解绑驱动
act dev://bus rescan                                 # 重新扫描总线
act dev://bus add 00:06.0 10de:1eb8 display          # 热插入设备
act dev://bus remove 00:06.0                         # 热拔出设备
```

约束：已绑定驱动的设备重复绑定会报错；未绑定时解绑会报错；总线地址不可重复。

## 测试

```sh
cargo test
```

当前 146 个单元测试全部通过，覆盖通道注册、权限模型、各系统模块的边界条件（重复挂载、非法前缀、MTU 越界、重复绑定等）。

## 设计说明

每个系统模块都是"一个全局单例 + 一组纯逻辑方法 + 若干单元测试"，由 `kernel/sense.rs` 的 `probe` / `act` 统一路由到通道，因此**新增模块不需要改动 shell**：

1. 新建 `kernel/xxx.rs`，暴露 `xxx() -> &'static Mutex<Xxx>`，实现 `list()` 与各 `act` 方法；
2. 在 `kernel/channels.rs` 的 `bootstrap()` 注册通道 URI 与权限；
3. 在 `kernel/sense.rs` 的 `probe` 与 `act` 加一条分支；
4. 在 `kernel/mod.rs` 声明模块。

## 许可证

[MIT](LICENSE) © logos-42
