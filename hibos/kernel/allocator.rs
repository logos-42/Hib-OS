//! 极简 bump allocator —— 供 no_std 内核堆分配使用。
//! 不支持释放（适合内核早期阶段，后续可替换为链表分配器）。

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

const HEAP_START: usize = super::HEAP_START;
const HEAP_SIZE: usize = super::HEAP_SIZE;

pub struct BumpAllocator {
    next: AtomicUsize,
}

impl BumpAllocator {
    pub const fn new() -> Self {
        Self {
            next: AtomicUsize::new(HEAP_START),
        }
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();
        let mut next = self.next.load(Ordering::Relaxed);
        let aligned = (next + align - 1) & !(align - 1);
        let end = aligned + size;
        if end > HEAP_START + HEAP_SIZE {
            return core::ptr::null_mut();
        }
        self.next.store(end, Ordering::Relaxed);
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump allocator 不支持释放
    }
}
