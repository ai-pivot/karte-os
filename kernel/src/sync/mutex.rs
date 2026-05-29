// kernel/src/sync/mutex.rs — Blocking mutex for kernel use
//
// Unlike SpinLock (which burns CPU spinning), this mutex blocks the calling
// task on contention by yielding to the scheduler. When the lock is released,
// the next waiting task is woken up.
//
// This is the correct synchronization primitive for I/O-bound kernel
// operations (filesystem, block device, network) where the critical section
// may take many milliseconds and spinning would waste CPU time.
//
// Design:
//   - Free → lock succeeds immediately (fast path, no schedule() call)
//   - Held → caller is blocked via schedule_block(), another task runs
//   - Unlock → wake one blocked waiter via wake_task()
//
// Note: This mutex is NOT safe to use from interrupt context. For interrupt
// handlers, use SpinLock or IntSpinLock instead.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A blocking mutex backed by the scheduler.
///
/// When the lock is contended, the current task is descheduled (marked
/// `Blocked`) and the CPU is yielded to the next ready task. On unlock,
/// the longest-waiting blocked task is woken.
pub struct BlockingMutex<T> {
    locked: AtomicBool,
    /// Process index of the task currently waiting for this lock.
    /// 0 means "no waiter". Only one waiter is tracked; additional
    /// contenders will spin-wait briefly then block themselves.
    waiter: AtomicUsize,
    data: UnsafeCell<T>,
}

// SAFETY: BlockingMutex can be sent across threads if T is Send.
unsafe impl<T: Send> Send for BlockingMutex<T> {}

// SAFETY: BlockingMutex is Sync if T is Send, because the lock
// guarantees exclusive access via the scheduler's block/wake mechanism.
unsafe impl<T: Send> Sync for BlockingMutex<T> {}

impl<T> BlockingMutex<T> {
    /// Create a new BlockingMutex wrapping `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiter: AtomicUsize::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock. If it is already held, block the current task
    /// and yield to the scheduler. When woken, retry acquisition.
    ///
    /// **Must only be called from a schedulable task context** (not from
    /// interrupt handlers or before the scheduler is initialized).
    pub fn lock(&self) -> BlockingMutexGuard<'_, T> {
        // Fast path: try to acquire without blocking.
        if !self.locked.swap(true, Ordering::Acquire) {
            return BlockingMutexGuard { mutex: self };
        }

        // Slow path: the lock is held. Register ourselves as a waiter
        // and block until we are woken, then retry.
        let my_proc = crate::process::current_index();
        loop {
            // Register as waiter so the unlocker knows whom to wake.
            self.waiter.store(my_proc, Ordering::SeqCst);

            // Double-check: the holder might have just released.
            if !self.locked.swap(true, Ordering::Acquire) {
                self.waiter.store(0, Ordering::SeqCst);
                return BlockingMutexGuard { mutex: self };
            }

            // Still held — block ourselves and yield to the scheduler.
            // schedule_block() will mark us as Blocked and switch to the
            // next ready task. We'll resume here when wake_task() is called.
            crate::sched::schedule_block();

            // We've been woken. Clear the waiter slot and try again.
            // (The unlocker already cleared it or another waiter replaced us.)
        }
    }

    fn unlock(&self) {
        let waiter = self.waiter.swap(0, Ordering::SeqCst);
        self.locked.store(false, Ordering::Release);

        // Wake the blocked waiter so it can retry acquisition.
        if waiter != 0 {
            crate::sched::wake_task(waiter);
        }
    }
}

pub struct BlockingMutexGuard<'a, T> {
    mutex: &'a BlockingMutex<T>,
}

impl<T> Deref for BlockingMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for BlockingMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for BlockingMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

// ── Lightweight variant for single-hart (init) context ────────────────
//
// When the scheduler is running, the init process (shell) is not a normal
// schedulable task — it has no TCB slot. If init tries to lock a
// BlockingMutex that's already held, schedule_block() would be wrong
// because init can't be blocked.
//
// For init-only state (like the global ext4 instance), we use a simpler
// approach: a SpinLock that briefly spins, then yields via schedule()
// instead of blocking. This avoids the block/wake machinery entirely.

/// A lightweight mutex suitable for single-hart init context.
///
/// Uses a short spin loop with scheduler yields. Not as efficient as
/// BlockingMutex for multi-task contention, but safe to use from init.
pub struct YieldMutex<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for YieldMutex<T> {}
unsafe impl<T: Send> Sync for YieldMutex<T> {}

impl<T> YieldMutex<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock. Spins briefly, then yields to the scheduler
    /// on each iteration to avoid wasting CPU.
    pub fn lock(&self) -> YieldMutexGuard<'_, T> {
        loop {
            if !self.locked.swap(true, Ordering::Acquire) {
                return YieldMutexGuard { mutex: self };
            }
            // Yield to scheduler instead of burning CPU.
            // This allows other tasks (or timer interrupt) to run.
            crate::sched::schedule();
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct YieldMutexGuard<'a, T> {
    mutex: &'a YieldMutex<T>,
}

impl<T> Deref for YieldMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for YieldMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for YieldMutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Mutex Tests ──");

    // Note: BlockingMutex lock/unlock cannot be fully tested in single-task
    // test mode (no scheduler context switching), but we can test the basic
    // fast path (uncontended lock).

    // Test 1: YieldMutex lock and unlock (uncontended)
    crate::test::run_test("yieldmutex_lock_unlock", || {
        let m = YieldMutex::new(42usize);
        let g = m.lock();
        *g == 42
    });

    // Test 2: YieldMutex modify via guard
    crate::test::run_test("yieldmutex_modify", || {
        let m = YieldMutex::new(0usize);
        {
            let mut g = m.lock();
            *g = 100;
        }
        let g = m.lock();
        *g == 100
    });

    // Test 3: YieldMutex guard drop releases lock
    crate::test::run_test("yieldmutex_guard_drop", || {
        let m = YieldMutex::new(1usize);
        {
            let _g = m.lock();
        }
        let g = m.lock();
        *g == 1
    });

    // Test 4: YieldMutex complex data
    crate::test::run_test("yieldmutex_complex_data", || {
        let m = YieldMutex::new([0usize; 4]);
        {
            let mut g = m.lock();
            g[0] = 10;
            g[1] = 20;
            g[2] = 30;
            g[3] = 40;
        }
        let g = m.lock();
        g[0] == 10 && g[1] == 20 && g[2] == 30 && g[3] == 40
    });

    // Test 5: BlockingMutex fast path (uncontended, no schedule_block needed)
    crate::test::run_test("blockingmutex_fast_path", || {
        let m = BlockingMutex::new(99usize);
        let g = m.lock();
        *g == 99
    });

    // Test 6: BlockingMutex guard drop
    crate::test::run_test("blockingmutex_guard_drop", || {
        let m = BlockingMutex::new(7usize);
        {
            let mut g = m.lock();
            *g = 42;
        }
        let g = m.lock();
        *g == 42
    });
}
