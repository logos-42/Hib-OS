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

`channels` 命令列出全部 76 个通道、类别、读写权限以及感知/行动计数。

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
| `mount://table` | `act mount://table mount tmpfs /data tmpfs ro` | 挂载表，挂载点会物化进 VFS（见下节） |

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
| `gateway://net` | `act gateway://net policy deny` | 出站策略与流量闸门 |
| `browser://gateway` | `act browser://gateway open https://example.com` | 真实抓取页面、提取标题与正文 |

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
| `power://state` | `act power://state suspend` | 电源姿态与历史；真挂起/恢复全部执行体，等待表保留 |
| `pkg://store` | `act pkg://store install curl` | 包管理 |
| `user://db` | `act user://db adduser alice` | 用户与身份 |
| `sec://caps` | `act sec://caps grant CAP_NET_ADMIN` | 能力位（root 以能力集合而非 uid=0 表示） |

### 执行体 / 同步 / 租约 / 子智能体（真并发原语）

| 通道 | 示例 | 说明 |
|---|---|---|
| `exec://threads` | `act exec://threads spawn t1 sense gpu://vram` | 真线程执行体，跑 sense/act 任务 DSL |
| `sync://primitives` | `act sync://primitives sem s1 3` / `wait` | 命名信号量（FIFO+超时）、认知屏障 |
| `lease://channels` | `act lease://channels acquire ctx://m` / `commit` | 单写者租约 + epoch 快照 + 乐观 CAS 提交 |
| `agent://results` | `sense agent://results` | 子智能体任务结果表 |

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

**与 VFS 真实联动**（不再是纯记录）：

- 挂载时自动递归创建目标目录（等价 `mkdir -p`）
- `procfs` 挂载点写入 `meminfo` / `loadavg` / `uptime` / `version` / `stat`（取自 `proc://sysinfo`）
- `sysfs` 挂载点写入 `gpu_vram` / `kernel_params` / `net_interfaces` / `judgment`
- `devfs` 挂载点按设备树写入设备节点
- 每次 `ls` / `cat` / `cd` 前刷新伪文件系统内容；`umount` 时一并移除该挂载点目录

因此启动后 `ls /` 就能看到 `proc/`、`sys/`，`cat /proc/loadavg`、`cat /sys/gpu_vram` 拿到的都是实时内容。

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

## 意志层：动机 / 信念 / 判断力

前两批通道都在描述"世界"与"能做什么"，这三个通道补上"**为什么做、有多大把握、到底做不做**"。
它们是**三个独立通道**，但数据是串起来用的：判断力读取动机与信念，自己不改它们。

```text
motive://self    我做事的倾向    curiosity / order / safety / efficacy（0-100 强度）
belief://self    我认为世界如何  命题 + 置信度 0-100 + 支持/反驳证据
judgment://self  我据此怎么裁决  判断分 = 信念置信 ×0.6 + 主导动机强度 ×0.4
                                 >=70 commit / 40-69 probe / <40 hold
```

### `motive://self` —— 动机

```sh
sense motive://self                       # 四条动机的强度条 + 主导动机
act motive://self urge efficacy +20       # 带符号增量，自动裁剪到 0-100
act motive://self set safety 95           # 绝对设定
act motive://self reset                   # 恢复默认
```

### `belief://self` —— 信念

```sh
sense belief://self
act belief://self believe GPU显存可用 90    # 新增或覆盖置信度
act belief://self observe GPU显存可用 -20  # 加一条反驳证据，置信度下降
act belief://self weak 60                 # 列出置信度低于 60% 的信念
act belief://self retract 某个命题         # 撤销
```

### `judgment://self` —— 判断力

```sh
sense judgment://self              # 策略、裁决统计、最近裁决
act judgment://self judge GPU显存可用   # 裁决指定命题
act judgment://self judge          # 省略命题：自动挑最弱信念
act judgment://self decide         # 自主判断
```

示例（动机 safety=95、信念"外部网络可达"=50%）：

```
judge "外部网络可达": score=68 verdict=probe
reason: 信念置信 50% ×0.6 + 主导动机 safety(95) ×0.4 = 68
建议: 建议先 act belief://self observe "外部网络可达" +10 补充证据
```

三个通道在 `/sys/judgment` 里也有一份快照（挂载物化时写入）。

## 外部世界：网关与浏览器

- `gateway://net`：出站策略闸门。最长域名后缀规则优先，未命中用默认策略（`allow`/`deny`），每次判定计入 `allowed` / `blocked` / `bytes`。
- `browser://gateway`：
  - `open http(s)://...` —— 先过网关策略，再用真实网络抓取，提取 `<title>`、剥离标签存正文（最多 8KB），记录历史；4xx/5xx 也保留响应体。
  - `open file:///etc/hibos.toml` —— 直接读 VFS，并同步到浏览器状态。
  - `text 200` —— 查看正文前 200 字符；`history` —— 查看最近记录。

## 真像素 GUI（`gui` feature）

Hib OS 不再只有终端 ASCII 假桌面，而是一套真正的像素 GUI 栈：

- `kernel/gui/surface.rs`：32bpp ARGB 表面，alpha 混合、矩形/圆角/渐变、5×7 字体光栅化。
- `kernel/gui/window.rs`：窗口对象，每个窗口拥有独立客户端表面；标题栏/关闭/最小化/最大化/缩放按钮；命中测试；内置终端模型（键盘 → 窗口 → 像素闭环）。
- `kernel/gui/compositor.rs`：壁纸、z-order、焦点、自上而下命中测试、窗口提升/置顶、损坏区域合成、光标。
- `kernel/gui/input.rs`：键盘/鼠标事件队列，按 z-order 路由到焦点窗口；标题栏拖动、右下角缩放、滚轮滚动。
- `kernel/gui/bmp.rs`：24 位 BMP 编码/保存，零依赖，可直接在资源管理器查看。
- `kernel/gui/backend.rs`（`--features gui`）：winit 0.30 + softbuffer 0.4.8 真实窗口，最近邻缩放，400ms 节流实时刷新。

三者共享同一份像素：终端 ANSI 预览、`gui shot` 导出的 BMP、`gui run` 打开的真实窗口，都由同一个 `Compositor` 合成。

```sh
cargo build --features gui
cargo run --features gui -- --shell
# 进入 shell 后：
gui desktop      # 显示桌面摘要 + 真彩预览
gui shot         # 导出 gui-output.bmp (960x600)
gui run          # 打开真实窗口（阻塞到关闭；窗口内 Enter 的命令由 shell 事后执行）
gui open TERMINAL
gui focus 1
gui click 100 100
gui key hello\n
```

### 通道注册表（本轮新增）

| 通道 | 类别 | 说明 |
|---|---|---|
| `exec://threads` | Action | 真线程执行体与 wait_table |
| `sync://primitives` | Action | 命名信号量 / 认知屏障 |
| `lease://channels` | Memory | 单写者租约 + epoch |
| `agent://results` | Agent | 子智能体结果表 |
| `gui://windows` | Desktop | 窗口合成状态 |
| `input://kbd` | Desktop | 键盘输入 |
| `input://mouse` | Desktop | 鼠标输入 |

## 测试

```sh
cargo test
```

当前 252 个单元测试全部通过，覆盖通道注册、权限模型、各系统模块的边界条件（重复挂载、非法前缀、MTU 越界、重复绑定、判断阈值、置信度裁剪等）。

## 设计说明

每个系统模块都是"一个全局单例 + 一组纯逻辑方法 + 若干单元测试"，由 `kernel/sense.rs` 的 `probe` / `act` 统一路由到通道，因此**新增模块不需要改动 shell**：

1. 新建 `kernel/xxx.rs`，暴露 `xxx() -> &'static Mutex<Xxx>`，实现 `status()` 与各 `act` 方法；
2. 在 `kernel/channels.rs` 的 `bootstrap()` 注册通道 URI 与权限；
3. 在 `kernel/sense.rs` 的 `probe` 与 `act` 加一条分支；
4. 在 `kernel/mod.rs` 声明模块。

需要跨模块读取时直接锁对方的全局单例即可（例如 `judgment.rs` 读 `motive::motives()` 与
`belief::beliefs()`）。注意保持单向依赖，避免两个模块互相加锁造成死锁。
若引入新的通道类别，还需同步 `ChannelKind::name()` 与 `ai_token.rs` 中的感知映射。

## 许可证

[MIT](LICENSE) © logos-42
