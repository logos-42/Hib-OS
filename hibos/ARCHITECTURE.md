# Hib OS 架构说明

本文档描述 Hib OS 内核的核心设计决策，供后续维护与扩展参考。

## 一、核心设计哲学：资源即通道

系统中所有资源（文件、显存、设备、网络、桌面、执行体、信号量、租约、子智能体……）统一建模为**通道**，地址形如 `<scheme>://<resource>`，对外只有两个动词：

| 动词 | 作用 | 所需权限 |
|---|---|---|
| `sense <通道>` | 读取通道的实时快照 | 可读 (r) |
| `act <通道> [负载...]` | 向通道写入或触发一次操作 | 可写 (w) |

并发与调度**围绕通道建模，而非线程**：调度单位是“通道事件”，`wait_table` 为主结构，`ready_queue` 是派生物。

## 二、三铁律

1. **单写者租约**：同一时刻，一个通道只能由一个 owner 持有写租约；其它写入请求排队或冲突。
2. **epoch 快照 + 乐观 CAS 提交**：读取先拍 epoch 快照，提交时比对期望 epoch，不匹配则拒绝并返回当前 epoch，调用方重试。
3. **提交即事件唤醒**：租约提交（commit/force）会向该通道的所有 `wait_next` 等待者发边沿触发 `signal`，消除“检查条件 → 注册等待”窗口的丢唤醒竞态。

## 三、五调度域（多内核形态）

Hib OS 不做传统 SMP，而是以**多调度域**形态存在多个内核，每个域对应一种认知功能：

| 调度域 | 职责 | 调度特征 |
|---|---|---|
| reflex | 反射域，中断/信号级快速响应 | 最高优先级，抢占式 |
| perception | 感知域，`sense` 通道快照 | 通道事件驱动 |
| deliberation | 审议域，`act` 通道写入与租约提交 | 单写者租约串行化 |
| memory | 记忆域，状态快照与历史回溯 | 只读为主，读写并发 |
| governance | 治理域，电源/能力/策略决策 | root capability 门禁 |

每个域独立运行 MLFQ（多级反馈队列）调度，带语义权重与老化，避免低优先级饿死与时间片到期丢任务。

## 四、通道所有权

通道注册表（`kernel/channels.rs`）在 `bootstrap()` 中一次性注册全部 91 个通道，每条带 `ChannelKind` 与 `Access`（Read/Write/ReadWrite）。通道归属分类：

| 类别 | 代表通道 | 所有者 |
|---|---|---|
| Action | `exec://threads`, `sync://primitives`, `action://syscall` | 调度域 / 审议域 |
| Agent | `agent://orchestrator`, `agent://workers`, `agent://results` | 子智能体编排器 |
| Desktop | `desktop://screen`, `gui://windows`, `input://kbd`, `input://mouse` | GUI 显示服务器 |
| Memory | `mem://ram`, `mem://mmap`, `lease://channels` | 租约 / 内存域 |
| … | … | … |

新增模块的标准步骤（无需改动 shell）：
1. 新建 `kernel/xxx.rs`，暴露 `xxx() -> &'static Mutex<Xxx>`，实现 `status()` 与各 `act` 方法；
2. 在 `kernel/channels.rs` 的 `bootstrap()` 注册通道 URI 与权限；
3. 在 `kernel/sense.rs` 的 `probe` 与 `act` 加一条分支；
4. 在 `kernel/mod.rs` 声明模块。

## 五、root / 超级开发者空间

Hib OS **不沿用 `uid=0` 全权 root**。root 被重新定义为**capability 集合**：

- `sec://caps` 通道管理能力位（`CAP_SYS_ADMIN`、`CAP_NET_ADMIN`、`CAP_SYS_MODULE` 等）。
- `user://db` 管理身份（uid/name/groups），但敏感操作（电源 suspend/reboot、租约 `force`、内核参数修改……）的门禁是**当前身份持有的 capability 集合**，而非 `uid == 0`。
- “超级开发者空间” = 持有完整 capability 集合的会话；可通过 `act sec://caps grant <cap>` 逐项授权，体现最小权限原则。

## 六、GUI 栈

三层，共享同一份 32bpp 像素：

```
输入事件 ──► Compositor(窗口/z-order/焦点) ──► Surface(32bpp ARGB) ──► BMP / softbuffer 真实窗口
```

1. `surface.rs`：32bpp ARGB 表面，alpha 混合、矩形/圆角/渐变、5×7 字体光栅化、BMP 导出。
2. `window.rs`：窗口对象，每个窗口拥有独立客户端表面；标题栏 / 关闭 / 最小化 / 最大化 / 缩放；命中测试；内置终端模型（键盘 → 窗口 → 像素闭环）。
3. `compositor.rs`：壁纸、z-order、焦点、自上而下命中测试、窗口提升/置顶、光标。
4. `input.rs`：键盘/鼠标事件队列，按 z-order 路由到焦点窗口；标题栏拖动、右下角缩放、滚轮滚动。
5. `backend.rs`（`--features gui`）：winit 0.30 + softbuffer 0.4.8 真实窗口，最近邻缩放，400ms 节流实时刷新。

**终端 ANSI 预览 / `gui shot` 导出的 BMP / `gui run` 打开的真实窗口，用的是同一份像素**，不存在两套渲染代码。

## 七、电源与睡眠

`power://state` 接入 `exec::suspend_all()` / `exec::resume_all()`：

- `suspend`：把全部执行体置为 Suspended，`wait_table` 原样保留，不丢事件。
- `running` / `wake`：恢复全部执行体，它们回到原来的等待键上继续。

## 八、并发原语

| 模块 | 原语 | 特征 |
|---|---|---|
| `exec` | 真 `std::thread` 执行体 | `wait_table` 边沿触发闩锁 `signal` / 电平触发 `wake`；`suspend`/`resume`/`join` |
| `sync` | `Semaphore` / `Barrier` / `PriorityMutex` / `RwSync` | 真阻塞 FIFO+超时；epoch 认知屏障；`ThreadId` owner + 优先级继承 |
| `lease` | `Snapshot`/`Epoch`/`Lease` | 单写者租约 + 乐观 CAS 提交 + `wait_next` 事件驱动 |
| `agent` | `agent://orchestrator` | 真线程子体 + 邮箱 + 任务 DSL + 认知屏障；禁止递归 `agent://` |

## 九、并发原语（新增模块）

| 模块 | 原语 | 特征 |
|---|---|---|
| `exec` | 真 `std::thread` 执行体 | `wait_table` 边沿触发闩锁 `signal` / 电平触发 `wake`；`suspend`/`resume`/`join` |
| `sync` | `Semaphore` / `Barrier` / `PriorityMutex` / `RwSync` | 真阻塞 FIFO+超时；epoch 认知屏障；`ThreadId` owner + 优先级继承 |
| `lease` | `Snapshot`/`Epoch`/`Lease` | 单写者租约 + 乐观 CAS 提交 + `wait_next` 事件驱动 |
| `agent` | `agent://orchestrator` | 真线程子体 + 邮箱 + 任务 DSL + 认知屏障；禁止递归 `agent://` |
| `mutex2` | `NamedMutex` / `sync://mutex/<名>` | **跨进程/域可见的命名互斥锁**，FIFO 等待，持租者标识 |
| `proc2` | `Proc` / `proc://spawn|kill|list` | **子意识主体容器**：独立 `wait_table` 副本 + 独立租约集，优雅弹出时释放全部租约 |
| `irq` | `IrqBinding` / `irq://<设备>` | **中断通道**：设备事件投递为 `signal`，绑定高优先级处理器 exec-id |
| `mm` | `PageFrame` / `mem://fault` | **页故障模拟**：缺页 → 阻塞 → 分配页框 → 唤醒；OOM = 租约冲突 |
| `file_channel` | `FileNode` / `file://<path>` | **文件通道化**：每个文件 = 通道，`sense` = 读，`act` = 写，元带 last_writer/epoch |
| `loader` | `ImageMeta` / `loader://<镜像>` | **意识镜像加载器**：echo / sense-loop / act-loop / idle |

## 十、GPU 硬件保护（写租约隔离）

- `gpu://vram/<分区>` 可通过 `acquire`/`release`/`query`/`commit` 操作，实现**单写者写租约**
- `commit` 时检查租约持有者 + 期望 epoch（乐观 CAS），不匹配则 `AccessViolation`
- `VramManager` 维护 `gpu_write_leases`（分区 ID → exec-id）与 `gpu_lease_epochs`
- `force`（治理域/root 路径）可绕过租约但推进 epoch

## 十一、调度器感知驱动抢占

- `exec::signal(key)` 同时调用 `scheduler::wake_key_global(key)`，使等待该通道的调度令牌进入就绪队列
- 通道事件即调度事件，实现「感知驱动抢占」

## 十二、电源唤醒源

- `power://wake-source <通道>` 设置唤醒源
- `power://state wake` / `wake()` 恢复全部执行体后，向唤醒源通道发 `signal`

## 十三、测试基线

- `cargo test`：270 个测试全部通过。
- `cargo test --features gui`：272 个测试全部通过（含 GUI 逻辑层 40 个 + 后端缩放等）。
