// kernel/src/sync/spinlock.rs — Kernel spinlock implementation

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: SpinLock can be sent across threads if T is Send,
// because the lock serializes access to the inner data.
unsafe impl<T: Send> Send for SpinLock<T> {}

// SAFETY: SpinLock is Sync if T is Send, because the lock
// guarantees exclusive access via atomic swap.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self.locked.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }

    /// Attempt to acquire the lock without blocking.
    /// Returns Some(guard) if successful, None if already locked.
    /// Safe to call from ISR context.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        if self.locked.swap(true, Ordering::Acquire) {
            None
        } else {
            Some(SpinLockGuard { lock: self })
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── SpinLock Tests ──");

    // Test 1: Lock and unlock
    crate::test::run_test("spinlock_lock_unlock", || {
        let lock = SpinLock::new(42usize);
        let guard = lock.lock();
        *guard == 42
    });

    // Test 2: Modify through guard
    crate::test::run_test("spinlock_modify_via_guard", || {
        let lock = SpinLock::new(0usize);
        {
            let mut guard = lock.lock();
            *guard = 100;
        }
        let guard = lock.lock();
        *guard == 100
    });

    // Test 3: Guard drops and releases lock
    crate::test::run_test("spinlock_guard_drop_releases", || {
        let lock = SpinLock::new(1usize);
        {
            let _g = lock.lock();
        }
        // Should be able to lock again (would deadlock if not released)
        let guard = lock.lock();
        *guard == 1
    });

    // Test 4: Lock with complex data
    crate::test::run_test("spinlock_complex_data", || {
        let lock = SpinLock::new([0usize; 4]);
        {
            let mut g = lock.lock();
            g[0] = 10;
            g[1] = 20;
            g[2] = 30;
            g[3] = 40;
        }
        let g = lock.lock();
        g[0] == 10 && g[1] == 20 && g[2] == 30 && g[3] == 40
    });

    // Test 5: Multiple sequential locks
    crate::test::run_test("spinlock_sequential_locks", || {
        let lock = SpinLock::new(0usize);
        for i in 0..10usize {
            let mut g = lock.lock();
            *g = i;
        }
        let g = lock.lock();
        *g == 9
    });
}
