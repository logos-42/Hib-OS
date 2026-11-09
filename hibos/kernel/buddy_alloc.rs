//! 伙伴系统页框分配器 (Buddy System Page Frame Allocator)。
//!
//! 设计信条：用 2 的幂次块管理物理页框，分配时按需求阶向上分裂，
//! 释放时与空闲伙伴递归合并，从根源上压制外部碎片。这是 Linux/FreeBSD
//! 等真实内核内存管理的核心底座，也是 Hib OS 真实分页 (`paging.rs`)
//! 的物理内存供给者——分页模块要"映射"时，先向这里要一帧物理页。
//!
//! 与 `lease.rs` 的单写者租约不同，伙伴分配器是底层的、无锁友好的结构；
//! 真实多核并发应由上层持大锁或 per-CPU 空闲链表来守护。本实现提供单线程
//! 正确版本，对外暴露 `&mut self` 接口，方便上层封装同步原语。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

/// 标准页大小（字节）。Hib OS 采用 4 KiB 页。
pub const PAGE_SIZE: usize = 4096;
/// 默认最大阶：单块上限为 2^11 页 = 8 MiB（演示规模，足够覆盖原型内存）。
pub const DEFAULT_MAX_ORDER: usize = 11;

/// 分配失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// 没有足够大的连续空闲块。
    OutOfMemory,
    /// 参数非法（阶越界、零大小等）。
    InvalidArgument,
    /// 释放的块不属于本分配器或已被释放（重复释放 / 越界）。
    BadFree,
}

/// 伙伴分配器实例。
pub struct BuddyAllocator {
    /// 管理的总页框数（会被二进制分解为对齐块）。
    total_frames: usize,
    /// 最大阶：单块最多 2^max_order 页。
    max_order: usize,
    /// 每个阶对应的空闲块首帧号列表（free_lists[o] 存所有 2^o 页大小的空闲块）。
    free_lists: Vec<Vec<usize>>,
    /// 已分配块首帧 -> 其阶，释放时据此定位块大小并合并。
    allocated: HashSet<usize>,
    /// 已分配页框数（统计用）。
    allocated_frames: usize,
}

impl BuddyAllocator {
    /// 新建分配器，管理 [0, total_frames) 页框。
    /// `max_order` 为单块最大阶；传 0 时自动取能覆盖 `total_frames` 的合适阶。
    pub fn new(total_frames: usize, max_order: usize) -> Self {
        let max_order = if max_order == 0 {
            let mut o = DEFAULT_MAX_ORDER;
            // 取不超过总页框数的最大阶，避免链表越界。
            while (1usize << o) > total_frames && o > 0 {
                o -= 1;
            }
            o
        } else {
            max_order
        };
        let mut free_lists: Vec<Vec<usize>> = (0..=max_order).map(|_| Vec::new()).collect();
        // 初始化空闲块：把整段 [0, total_frames) 按"最大对齐块"做二进制分解，
        // 例如 16 页 -> 8+4+2+2，保证每个块首帧 2^o 对齐。
        let mut start = 0usize;
        let mut remaining = total_frames;
        while remaining > 0 {
            // 选最大阶 o，使 2^o <= remaining 且 start 对齐 2^o。
            let mut o = 0;
            for cand in 0..=max_order {
                if (1usize << cand) <= remaining && (start % (1usize << cand) == 0) {
                    o = cand;
                } else if (1usize << cand) > remaining {
                    break;
                }
            }
            free_lists[o].push(start);
            start += 1usize << o;
            remaining -= 1usize << o;
        }
        Self {
            total_frames,
            max_order,
            free_lists,
            allocated: HashSet::new(),
            allocated_frames: 0,
        }
    }

    /// 覆盖 `frames` 个页框所需的最小阶（向上取整到 2 的幂）。
    fn order_for_frames(frames: usize) -> usize {
        if frames == 0 {
            return 0;
        }
        let mut o = 0;
        while (1usize << o) < frames {
            o += 1;
        }
        o
    }

    /// 按"页框数"分配，返回首帧号（Addr 即帧号，物理地址 = 帧号 * PAGE_SIZE）。
    pub fn alloc_frames(&mut self, frames: usize) -> Result<usize, AllocError> {
        if frames == 0 {
            return Err(AllocError::InvalidArgument);
        }
        let order = Self::order_for_frames(frames);
        if order > self.max_order {
            return Err(AllocError::OutOfMemory);
        }
        self.alloc_order(order)
    }

    /// 按"字节大小"分配，返回首帧号（自动向上取整到整页）。
    pub fn alloc(&mut self, size: usize) -> Result<usize, AllocError> {
        let frames = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        self.alloc_frames(frames)
    }

    /// 分配指定阶的块：从 order 往上找第一个非空空闲链表，必要时向下分裂。
    fn alloc_order(&mut self, order: usize) -> Result<usize, AllocError> {
        // 找第一个能用的阶。
        let mut found = None;
        for o in order..=self.max_order {
            if !self.free_lists[o].is_empty() {
                found = Some(o);
                break;
            }
        }
        let o = found.ok_or(AllocError::OutOfMemory)?;
        let mut block = self.free_lists[o].pop().unwrap();
        // 把 [order, o) 多出来的容量二分拆出伙伴，塞进低阶空闲链表。
        for k in (order..o).rev() {
            let buddy = block + (1usize << k);
            self.free_lists[k].push(buddy);
        }
        self.allocated.insert(block);
        self.allocated_frames += 1usize << order;
        Ok(block)
    }

    /// 释放首帧号为 `frame`、阶为 `order` 的块，并与空闲伙伴递归合并升阶。
    pub fn free(&mut self, frame: usize, order: usize) -> Result<(), AllocError> {
        if order > self.max_order || frame >= self.total_frames {
            return Err(AllocError::BadFree);
        }
        if !self.allocated.remove(&frame) {
            return Err(AllocError::BadFree);
        }
        self.allocated_frames -= 1usize << order;
        let mut b = frame;
        let mut o = order;
        loop {
            // 伙伴 = 翻转第 o 位：与 b 同属一个父块，大小为 2^o。
            let buddy = b ^ (1usize << o);
            let same_parent = (b & (1usize << (o + 1))) == (buddy & (1usize << (o + 1)));
            let buddy_free = !self.allocated.contains(&buddy)
                && buddy + (1usize << o) <= self.total_frames;
            if o < self.max_order && same_parent && buddy_free {
                // 从空闲链表摘除 buddy，与 b 合并为更高一阶的块。
                if let Some(pos) = self.free_lists[o].iter().position(|&x| x == buddy) {
                    self.free_lists[o].remove(pos);
                }
                b = b.min(buddy);
                o += 1;
            } else {
                break;
            }
        }
        self.free_lists[o].push(b);
        Ok(())
    }

    /// 统计信息快照。
    pub fn stats(&self) -> BuddyStats {
        let free_frames = self.total_frames - self.allocated_frames;
        let mut per_order = vec![0usize; self.max_order + 1];
        for (o, list) in self.free_lists.iter().enumerate() {
            per_order[o] = list.len();
        }
        BuddyStats {
            total_frames: self.total_frames,
            allocated_frames: self.allocated_frames,
            free_frames,
            per_order,
        }
    }

    /// 当前最大可用连续块大小（按页框数）。
    pub fn largest_free_block(&self) -> usize {
        for o in (0..=self.max_order).rev() {
            if !self.free_lists[o].is_empty() {
                return 1usize << o;
            }
        }
        0
    }
}

/// 伙伴分配器统计快照。
#[derive(Debug, Clone)]
pub struct BuddyStats {
    pub total_frames: usize,
    pub allocated_frames: usize,
    pub free_frames: usize,
    /// 每个阶的空闲块数量。
    pub per_order: Vec<usize>,
}

/// 页地址：物理帧号的轻量包装，对外暴露 `.index()` 取首帧号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageAddr(pub usize);

impl PageAddr {
    /// 返回首帧号（即物理地址 / PAGE_SIZE）。
    pub fn index(&self) -> usize {
        self.0
    }
}

/// 全局伙伴分配器单例，管理低 4 GiB 物理页（演示规模共 2^20 页）。
static BUDDY: OnceLock<Mutex<BuddyAllocator>> = OnceLock::new();

/// 取得（按需初始化）全局分配器。
fn buddy_global() -> &'static Mutex<BuddyAllocator> {
    BUDDY.get_or_init(|| Mutex::new(BuddyAllocator::new(1 << 20, 0)))
}

/// 分配 2^order 页大小的块，返回首帧地址（供 `buddy://stats alloc <order>`）。
pub fn buddy_alloc(order: usize) -> Result<PageAddr, AllocError> {
    let mut b = buddy_global().lock().unwrap();
    b.alloc_order(order).map(PageAddr)
}

/// 释放由 `buddy_alloc` 得到的块（供 `buddy://stats free <page> <order>`）。
pub fn buddy_free(addr: PageAddr, order: usize) -> Result<(), AllocError> {
    buddy_global().lock().unwrap().free(addr.0, order)
}

/// 生成分配器统计摘要（供 `buddy://stats` 通道展示）。
pub fn buddy_summary() -> String {
    let b = buddy_global().lock().unwrap();
    let s = b.stats();
    let mut out = format!(
        "buddy: 总帧={} 已分配={} 空闲={} 最大连续块={}页\n各阶空闲块: ",
        s.total_frames,
        s.allocated_frames,
        s.free_frames,
        b.largest_free_block()
    );
    for (o, n) in s.per_order.iter().enumerate() {
        if *n > 0 {
            out.push_str(&format!("{}:{} ", o, n));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_splits_into_aligned_blocks() {
        // 16 页应被分解为若干 2 的幂对齐块。
        let b = BuddyAllocator::new(16, 0);
        let s = b.stats();
        assert_eq!(s.total_frames, 16);
        assert_eq!(s.free_frames, 16);
        assert!(s.per_order.iter().sum::<usize>() >= 1);
    }

    #[test]
    fn alloc_and_free_roundtrip() {
        let mut b = BuddyAllocator::new(64, 0);
        let f = b.alloc_frames(1).unwrap();
        assert_eq!(f, 0);
        assert_eq!(b.stats().allocated_frames, 1);
        b.free(f, 0).unwrap();
        assert_eq!(b.stats().free_frames, 64);
    }

    #[test]
    fn alloc_splits_higher_order() {
        let mut b = BuddyAllocator::new(64, 0);
        // 请求 16 页 (order 4)，初始只有 64 页整块 (order 6)，应分裂出伙伴。
        let f = b.alloc_frames(16).unwrap();
        assert_eq!(f, 0);
        assert_eq!(b.stats().allocated_frames, 16);
        // 释放后应当合并回原最大块。
        b.free(f, 4).unwrap();
        assert_eq!(b.stats().free_frames, 64);
        assert_eq!(b.largest_free_block(), 64);
    }

    #[test]
    fn buddy_merge_coalesces() {
        let mut b = BuddyAllocator::new(64, 0);
        let a = b.alloc_frames(1).unwrap(); // 占帧 0
        let c = b.alloc_frames(1).unwrap(); // 占帧 1（是 0 的 order-0 伙伴）
        assert_eq!(a, 0);
        assert_eq!(c, 1);
        b.free(a, 0).unwrap();
        // 0 的伙伴 1 仍被分配，不能合并。
        assert_eq!(b.largest_free_block(), 1);
        b.free(c, 0).unwrap();
        // 0 与 1 都空闲，合并成 order-1 块 (帧 0..2)。
        assert_eq!(b.largest_free_block(), 2);
    }

    #[test]
    fn alloc_byte_size_rounds_up() {
        let mut b = BuddyAllocator::new(128, 0);
        // 5000 字节 -> 2 页 (order 1)。
        let f = b.alloc(5000).unwrap();
        assert_eq!(f, 0);
        assert_eq!(b.stats().allocated_frames, 2);
        b.free(f, 1).unwrap();
        assert_eq!(b.stats().free_frames, 128);
    }

    #[test]
    fn out_of_memory_when_exhausted() {
        let mut b = BuddyAllocator::new(4, 0);
        let _ = b.alloc_frames(2).unwrap();
        let _ = b.alloc_frames(2).unwrap();
        assert_eq!(b.alloc_frames(1), Err(AllocError::OutOfMemory));
    }

    #[test]
    fn bad_free_rejected() {
        let mut b = BuddyAllocator::new(32, 0);
        let f = b.alloc_frames(1).unwrap();
        b.free(f, 0).unwrap();
        // 重复释放同一个块。
        assert_eq!(b.free(f, 0), Err(AllocError::BadFree));
        // 释放从未分配的地址。
        assert_eq!(b.free(100, 0), Err(AllocError::BadFree));
    }

    #[test]
    fn alloc_order_too_large_fails() {
        let mut b = BuddyAllocator::new(16, 4); // 最大 order 4 = 16 页
        assert_eq!(b.alloc_frames(32), Err(AllocError::OutOfMemory));
    }
}
