# GPU 写租约隔离（硬件保护）

## 设计差异

| 维度 | Linux 思维 | Hib OS 通道思维 |
|---|---|---|
| GPU 保护 | DRM/KMS + GEM 显存对象 + 提交队列 + 隔离上下文，进程通过 ioctl 操作 GPU | GPU 显存 = `gpu://vram` 通道；每个进程/意识主体对显存分区的访问 = 持有该分区的**写租约**（单写者）；GPU 调度 = `act gpu://scheduler` 提交命令缓冲，按租约 epoch 串行化；IOMMU 映射表随租约变更 |

## 通道

| 通道 | 示例 | 说明 |
|---|---|---|
| `gpu://vram` | `act gpu://vram acquire 1 42` | 对显存分区 1 获取写租约（owner=exec 42） |
| `gpu://vram` | `act gpu://vram release 1 42` | 释放分区 1 的写租约 |
| `gpu://vram` | `act gpu://vram query 1` | 查询分区 1 的租约持有者与 epoch |
| `gpu://vram` | `act gpu://vram commit 1 42 0 <数据>` | 持租者提交帧（乐观 CAS epoch，失败则冲突） |

- `commit` 时检查租约持有者 + 期望 epoch（乐观 CAS），不匹配则 `AccessViolation`
- `VramManager` 维护 `gpu_write_leases`（分区 ID → exec-id）与 `gpu_lease_epochs`
- `force`（治理域/root 路径）可绕过租约但推进 epoch

## 返回索引

[返回目录](../README.md)
