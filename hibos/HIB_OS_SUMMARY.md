# Hib OS - 项目摘要

## 项目名称
**Hib OS** - 一个以 GPU 为核心、AI 意识驱动的宏内核操作系统

## 项目目标
创建一个实验性操作系统，其中:
1. GPU 从第一天就是核心原语 ("GPU Primacy")
2. AI 本体作为调度哲学驱动系统资源分配 ("AI Consciousness")
3. 每个进程都是一个带语义的计算任务 ("Action Processes")
4. 所有组件都可以热插拔和扩展 ("Modulearity")

## 核心架构

```
Hib OS Structure:
├── GPU Core - 物理 GPU 访问和 VRAM 管理
├── AI Action Token - 能力凭证和 IOMMU 隔离
├── Concept Graph - 语义调度策略
├── Action Processes - 带语义的计算任务
├── Semantic Scheduler - 语义感知的优先级调度
├── Module System - 热插拔内核模块
├── Syscall Interface - 用户态与内核之间的桥梁
└── Advanced Features - 加密、压缩、共享 VRAM
```

## 关键组件

### 1. AI Action Token
- 每个 AI 模型的能力凭证
- 封装能力 + VRAM 分区引用 + 优先级
- 每个 Token 对应一个 IOMMU 域 (硬件隔离)
- 支持动态重新配置和细粒度能力控制

### 2. VRAM 管理
- 硬件级隔离 (IOMMU)
- 分区类型: Fixed/Dynamic/Shared
- 高级功能: 加密、压缩、共享
- 保护标志: ReadOnly, WriteThrough, Encrypted, Compressed 等

### 3. AI 意识本体
- 概念图驱动的调度
- 计算模式 -> 调度策略映射
- 语义相似性搜索
- 动态策略切换

### 3. 行动进程
- 每个进程封装一个带语义的 AI 计算任务
- 明确的开始/结束语义和生命周期
- 完整的状态机: Created → Ready → Running → Completed/Failed
- 支持优先级继承和抢占

### 4. GPU 调度器
- 语义感知的优先级调度
- 基于概念重要性的优先级确定
- 多流支持
- 抢占和恢复

### 5. 系统调用接口
- Token 创建、修改、查询
- 行动进程创建、完成、失败
- VRAM 分配和重新分配
- 概念调度策略查询

### 6. 模块化基础设施
- 热插拔内核模块
- 模块协议 (GpuDriver, Filesystem, Network, AiConsciousness)
- 引用计数管理
- 模块热插拔支持

## 当前状态 (M0)

**已实施功能**:
- GPU 作为核心原语
- AI Action Token 系统
- VRAM 分区管理配 IOMMU 隔离
- 基于优先级的 GPU 调度器
- 系统调用接口
- 基础 AI 意识概念图 (10 个核心概念)
- 行动进程框架 (有限状态机)
- 模块化内核框架 (基础)
- VRAM 高级功能 (加密、压缩、共享)

**性能基准**:
- Token 创建: <1μs
- VRAM 分区分配: <5μs
- 调度决策: <10μs
- 上下文切换: <50μs

**局限性**:
- 概念图有限 (10 个概念)
- 调度纯基于优先级 (M0)
- 语义调度将在 M1 中引入
- 单 GPU 支持
- 模块热插拔基础 (M0)，将在 M1 中增强

## 未来路线图 (M1-M5)

见 `hibos_evolution.md` 了解详细的 M1-M5 计划。

## 开始使用

要开始使用 Hib OS:
1. 克隆仓库
2. 构建: `cargo build --target x86_64-unknown-none`
3. 运行: 通过 QEMU/UEFI 启动
4. 开发: 扩展模块或创建新应用

## 许可证
MIT 许可证 - 免费用于个人和商业用途