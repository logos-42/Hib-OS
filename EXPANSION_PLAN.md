# Hib OS 代码扩充计划：3.4 万行 → 20 万行

## 现状基线
- 源文件：97 个 `.rs` 文件
- 代码行数：约 34,049 行（`cargo test` 322 通过，`--features gui` 324 通过）
- 核心抽象：统一通道 `scheme://resource`，`sense`/`act` 读写，权限位，单写者租约 + epoch 乐观 CAS
- 模块：GPU、GUI、网络、AI 意识、调度、内存、VFS、租约、capability、syscall 等
- 定位：AI 原生内核抽象研究原型，在宿主进程用户态运行

## 目标
- 代码行数：200,000 行（新增约 166,000 行）
- 保持 `cargo test` 全绿，`--features gui` 可创建真实窗口
- 每个新增模块都要有单元测试，覆盖率不低于现有模块

## 编码风格（必须遵循）
1. 文件头使用 `//!` 模块文档注释，说明设计信条
2. 通道遵循 `sense`/`act` 对：`probe(name) -> Option<String>` 和 `act(name, payload) -> Option<Result<String, String>>`
3. 并发使用 `lease.rs` 的单写者租约 + epoch CAS，优先于悲观锁
4. 安全使用 `caps.rs` 的能力位集合，而非 uid
5. 所有公共函数都要有 doc comment，复杂逻辑要有行内注释（中文）
6. 每个新增子系统都要在 `kernel/mod.rs` 注册 `pub mod`
7. 测试用 `#[cfg(test)] mod tests` 内联在同一文件末尾

## 五个并行子任务（总计 ~16.6 万行）

### 子任务 A：内核核心与通道系统扩充（~5 万行）
负责人：Subagent-A

**交付文件：**
- `kernel/channel_ext.rs` — 通道元数据、订阅/通知、通道租约扩展
- `kernel/buddy_alloc.rs` — 伙伴系统页框分配器（模拟）
- `kernel/slab_alloc.rs` — slab 对象缓存分配器
- `kernel/page_cache.rs` — 页缓存与缓冲区缓存
- `kernel/swap_sim.rs` — swap 区模拟（内存文件作为后端）
- `kernel/page_table_sim.rs` — 页表模拟（多级页表、TLB 模拟）
- `kernel/multicore_sched.rs` — 多核调度模拟（CFS/EEVDF 简化版）
- `kernel/async_runtime.rs` — 轻量级 async 运行时（futures + reactor）
- `kernel/signal.rs` — 信号机制（进程间信号、信号处理）
- `kernel/ipc_ext.rs` — IPC 扩展（共享内存、消息队列、管道、UNIX domain socket 简化版）

**接口规范：**
- 新增通道种类在 `ChannelKind` 枚举中追加（如 `ChannelKind::Swap`, `ChannelKind::PageTable`）
- 每个分配器实现 `alloc(size) -> Result<Addr, AllocError>` / `free(addr)` / `stats()`
- 页表模拟实现 `map(vaddr, paddr, flags)` / `unmap(vaddr)` / `translate(vaddr)`
- 多核调度实现 `pick_next(core_id)` / `yield_to(core_id)` / `load_balance()`
- async 运行时实现 `spawn(future)` / `block_on(future)` / `wake(task_id)`

**行数目标：~50,000 行**

---

### 子任务 B：文件系统与存储子系统（~4 万行）
负责人：Subagent-B

**交付文件：**
- `kernel/fs/disk_img.rs` — 磁盘镜像文件（作为块设备后端）
- `kernel/fs/partition.rs` — 分区表解析（MBR/GPT 简化版）
- `kernel/fs/buffer_cache.rs` — 缓冲区缓存（LRU 淘汰、脏页写回）
- `kernel/fs/ext2_like.rs` — ext2 兼容文件系统（超级块、inode、目录块、数据块）
- `kernel/fs/journal.rs` — 日志文件系统（WAL 预写日志、崩溃恢复）
- `kernel/fs/file_lock.rs` — 文件锁（顾问锁 + 强制锁、范围锁）
- `kernel/fs/mmap_file.rs` — 文件内存映射（页故障按需加载）
- `kernel/fs/dentry_cache.rs` — 目录项缓存（dcache、inode 缓存、名称查找）
- `kernel/fs/quota.rs` — 磁盘配额（用户/组配额、块和 inode 限制）
- `kernel/fs/aio.rs` — 异步 IO（提交队列、完成队列、io_uring 简化版）

**接口规范：**
- 块设备：`read_block(sector, buf)` / `write_block(sector, buf)` / `flush()`
- 文件系统：`create(path)` / `open(path)` / `read(fd, buf)` / `write(fd, buf)` / `close(fd)` / `unlink(path)` / `mkdir(path)`
- 缓冲区：`get_blk(sector)` / `brelse(buf)` / `mark_dirty(buf)` / `sync()`
- mmap：`mmap(addr, len, prot, flags, fd, offset)` / `munmap(addr, len)`

**行数目标：~40,000 行**

---

### 子任务 C：网络栈与设备驱动（~4 万行）
负责人：Subagent-C

**交付文件：**
- `kernel/net/tcp_stack.rs` — TCP 状态机（LISTEN/SYN_SENT/ESTABLISHED/...、滑动窗口、拥塞控制 Reno/Cubic）
- `kernel/net/udp_stack.rs` — UDP 完善（校验和、多路复用、端口绑定）
- `kernel/net/icmp.rs` — ICMP（echo request/reply、不可达、超时）
- `kernel/net/dhcp.rs` — DHCP 客户端（发现/请求/租约续期）
- `kernel/net/dns.rs` — DNS 解析器（递归查询、缓存、hosts 文件）
- `kernel/net/socket_api.rs` — Socket API（socket/bind/listen/accept/connect/send/recv/close）
- `kernel/net/netfilter.rs` — Netfilter 防火墙（钩子、规则匹配、NAT 简化版）
- `kernel/drivers/net_driver.rs` — 网卡驱动框架（驱动注册、发送/接收队列、中断处理）
- `kernel/drivers/pci_enumerate.rs` — PCI 配置空间扫描（设备/厂商 ID、类代码、BAR 空间）
- `kernel/drivers/usb_stack.rs` — USB 栈简化版（设备描述符、配置、端点、传输类型）
- `kernel/drivers/irq_ctrl.rs` — 中断控制器（IDT、中断门、中断处理器注册）
- `kernel/drivers/dma.rs` — DMA 引擎（散射聚合列表、传输完成中断）

**接口规范：**
- 网络：`tcp_connect(addr, port)` / `tcp_send(fd, data)` / `tcp_recv(fd, buf)` / `udp_sendto(...)`
- 驱动：`driver_register(name, ops)` / `device_probe(bus, addr)` / `irq_handler_register(irq, handler)`
- PCI：`pci_scan_bus()` / `pci_read_config(bdf, offset)` / `pci_write_config(...)`
- 防火墙：`nf_register_hook(hook)` / `nf_match_rule(packet, rules)`

**行数目标：~40,000 行**

---

### 子任务 D：GUI 桌面环境与 AI 意识（~2.5 万行）
负责人：Subagent-D

**交付文件：**
- `kernel/gui/widget.rs` — GUI 控件库（按钮、标签、文本框、复选框、下拉菜单、滑块）
- `kernel/gui/wm.rs` — 窗口管理器（窗口装饰、标题栏、最小化/最大化/关闭、窗口层级、焦点管理）
- `kernel/gui/compositor_ext.rs` — 合成器扩展（混合模式、透明度、动画、阴影、圆角）
- `kernel/gui/file_manager.rs` — 文件管理器应用（目录浏览、复制/粘贴/删除、拖拽）
- `kernel/gui/terminal.rs` — 终端仿真器（PTY、ANSI 转义码、滚动条、选中复制）
- `kernel/gui/theme.rs` — 主题系统（配色方案、字体、图标、样式表）
- `kernel/gui/gesture.rs` — 触摸与手势（点击、双击、拖拽、缩放、滑动）
- `kernel/ai/multimodal.rs` — 多模态感知（图像/文本/音频通道统一接入）
- `kernel/ai/knowledge_graph.rs` — 知识图谱（实体、关系、三元组、图查询）
- `kernel/ai/reasoning.rs` — 推理引擎（规则链、向前/向后推导、置信度）
- `kernel/ai/model_loader.rs` — 模型加载器（GGUF/ONNX 简化版、量化、张量调度）
- `kernel/ai/metacognition.rs` — 元认知（自我监控、置信度评估、元学习）

**接口规范：**
- 控件：`widget_create(kind, parent)` / `widget_set_text(id, text)` / `widget_on_click(id, handler)`
- 窗口：`wm_create_window(rect, title)` / `wm_show(id)` / `wm_hide(id)` / `wm_focus(id)`
- 多模态：`sense_image(data)` / `sense_text(data)` / `sense_audio(data)` → 统一感知向量
- 知识图谱：`kg_add_triple(s, p, o)` / `kg_query(s, p, o)` / `kg_traverse(root, depth)`
- 推理：`reason_add_rule(antecedent, consequent)` / `reason_query(goal) → proof`

**行数目标：~25,000 行**

---

### 子任务 E：系统调用、ABI、安全、工具链（~1.1 万行）
负责人：Subagent-E

**交付文件：**
- `kernel/syscall/posix_layer.rs` — POSIX 兼容层（open/read/write/close/stat/fork/exec/wait）
- `kernel/syscall/elf_loader.rs` — ELF 加载器（程序头解析、节区映射、入口点跳转）
- `kernel/syscall/dynamic_linker.rs` — 动态链接器（符号解析、重定位、共享库加载）
- `kernel/security/sandbox.rs` — 沙箱（能力沙箱、系统调用过滤、文件系统沙箱）
- `kernel/security/audit.rs` — 审计日志（系统调用审计、文件访问审计、安全事件）
- `kernel/security/crypto.rs` — 加密原语（哈希 SHA-256、对称加密 AES、随机数生成）
- `kernel/pkg/package_manager.rs` — 包管理器（仓库、依赖解析、安装/卸载/升级）
- `kernel/debug/debugger.rs` — 调试器（断点、单步、寄存器查看、内存dump）
- `kernel/fs/procfs_ext.rs` — procfs 扩展（/proc/[pid]/ 详细信息、系统统计）

**接口规范：**
- POSIX：`posix_open(path, flags)` / `posix_read(fd, buf)` / `posix_fork()` / `posix_exec(path, argv)`
- ELF：`elf_load(path) -> EntryPoint` / `elf_resolve_symbol(name) -> addr`
- 沙箱：`sandbox_create(pid, rules)` / `sandbox_check_syscall(pid, syscall_no)`
- 审计：`audit_log(event_type, subject, object, result)` / `audit_query(filter)`
- 加密：`crypto_hash(data) -> digest` / `crypto_encrypt(key, plaintext) -> ciphertext`

**行数目标：~11,000 行**

---

## 集成要求
1. 每个子智能体在 `kernel/mod.rs` 中注册新模块
2. 新增通道种类在 `kernel/channels.rs` 的 `ChannelKind` 枚举中追加
3. 新增 `sense`/`act` 处理在 `kernel/sense.rs` 的 `probe()` / `act_raw()` 中分支处理
4. 所有新增代码都要有对应的 `#[cfg(test)]` 单元测试
5. 运行 `cargo test` 和 `cargo test --features gui` 必须全部通过
6. 不得删除或修改现有经过测试的功能

## 验收标准
- 总行数 ≥ 200,000 行
- `cargo test` 全部通过（322+ 新增测试）
- `cargo test --features gui` 全部通过
- `gui run` 仍能创建窗口并进入事件循环
- 每个新增模块都有文档注释和单元测试