//! 内核同步原语 —— 替代 std::sync::{Mutex, OnceLock}。
//! 基于原子操作与自旋，适用于 no_std 裸机环境。

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use core::ops::{Deref, DerefMut};

/// 简单的自旋锁（SpinLock）。
///
/// 适用于内核早期单核阶段，暂不支持优先级反转处理。
pub struct SpinLock<T> {
    locked: AtomicU8,   // 0 = 未锁定, 1 = 锁定
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicU8::new(0),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        // 自旋等待获取锁（ acquire 内存序）
        while self
            .locked
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }
}

/// 自旋锁保护的访问守卫。
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<'a, T> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<'a, T> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<'a, T> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(0, Ordering::Release);
    }
}

/// 一次性初始化锁（替代 std::sync::OnceLock）。
pub struct OnceLock<T> {
    initialized: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send> Send for OnceLock<T> {}
unsafe impl<T: Send> Sync for OnceLock<T> {}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            value: UnsafeCell::new(None),
        }
    }

    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        if !self.initialized.load(Ordering::Acquire) {
            let val = f();
            unsafe {
                *self.value.get() = Some(val);
            }
            self.initialized.store(true, Ordering::Release);
        }
        unsafe { (*self.value.get()).as_ref().unwrap() }
    }
}

pub type SpinLockGuardType<'a, T> = SpinLockGuard<'a, T>;
