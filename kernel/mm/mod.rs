//! 虚拟内存管理子系统 —— Hib OS 内核真实性的地基（P0-1）。
//! 
//! 设计哲学（AI 原生 + Linux 映射，见 docs/19）：
//! - 传统：进程地址空间/页表/TLB → AI 原生：模型权重(代码段) + 模型实例(运行态地址空间)
//! - 虚拟地址 → 物理页框 → token → 上下文页/KV 缓存的语义映射
//! - 缺页中断 → 上下文窗口溢出 → 调入知识页/上下文页
//! - 页面置换 (LRU/Clock) + Swap → 上下文页 LRU 淘汰 → 摘要写入长期记忆
//!
//! 本模块实现：
//! * 四级页表（PGD → PUD → PMD → PTE → 4KB 物理页），48 位虚拟地址
//! * 页框位图/伙伴分配器（简化版）
//! * 地址空间（AddressSpace）：每进程/智能体独立页表根，用户/内核空间分离
//! * 缺页中断 = `mem://fault/<pid>/<vaddr>` 通道事件
//! * 写时复制 (COW)：单写者租约共享页框，写入时复制
//! * 全部封装为 `mem://paging` 通道，可被 `sense` / `act` 操作
//!
//! 子模块：
//! * `fault` —— 旧极简页框分配器与缺页模拟（向后兼容 `alloc_pfn`/`free_pfn`/`handle_fault`/`stats`）
//! * `frame_alloc` —— 页框位图分配器（伙伴系统简化版）
//! * `pagetable` —— 四级页表项与页表遍历/映射/取消映射
//! * `address_space` —— 地址空间：独立页表根、缺页处理、COW 标记
//! * `cow` —— 写时复制：共享页框 + 写入时复制

pub mod address_space;
pub mod cow;
pub mod frame_alloc;
pub mod fault;
pub mod pagetable;

pub use address_space::{AddressSpace, create_address_space, destroy_address_space, get_address_space};
pub use address_space::{map_vaddr, translate_vaddr, unmap_vaddr, query_pte, handle_fault as as_handle_fault};
pub use cow::{CowRegion, cow_copy_on_write, cow_is_cow, cow_mark_cow};
pub use frame_alloc::{FrameAllocator, frame_allocator, alloc_phys_frame, free_phys_frame};
pub use fault::{alloc_pfn, free_pfn, handle_fault, query_pfn, reclaim_proc, stats};
pub use pagetable::{
    PageTable, PageTableEntry, PageTables, PageTableRoot, PteFlags, VAddr, PAddr,
    KERNEL_VADDR_BASE, PAGE_SIZE, PAGE_SHIFT, USER_VADDR_TOP, VADDR_MASK, VADDR_BITS,
};
