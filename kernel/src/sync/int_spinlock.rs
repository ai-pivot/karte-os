// kernel/src/sync/int_spinlock.rs — Interrupt-safe spinlock for kernel use
//
// Unlike the basic SpinLock, this variant saves and restores the interrupt
// enable state (sstatus.SIE) across the critical section. This prevents
// deadlocks when a timer interrupt fires while holding a lock that the
// interrupt handler also tries to acquire.
//
// Use this for any lock that may be held during I/O or other long-running
// operations (e.g., block device access, filesystem operations).

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spinlock that disables interrupts while held.
///
/// On `lock()`, the current S-mode interrupt-enable state (sstatus.SIE) is
/// saved and interrupts are disabled. On `unlock()` (when the guard drops),
/// the saved state is restored. This is the standard approach for kernel
/// spinlocks in SMP-capable operating systems.
pub struct IntSpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: IntSpinLock can be sent across threads if T is Send,
// because the lock serializes access to the inner data.
unsafe impl<T: Send> Send for IntSpinLock<T> {}

// SAFETY: IntSpinLock is Sync if T is Send, because the lock
// guarantees exclusive access via atomic swap + interrupt masking.
unsafe impl<T: Send> Sync for IntSpinLock<T> {}

impl<T> IntSpinLock<T> {
    /// Create a new IntSpinLock wrapping `data`.
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock, disabling interrupts until the guard is dropped.
    pub fn lock(&self) -> IntSpinLockGuard<'_, T> {
        // Save interrupt state and disable interrupts before spinning.
        // This ensures that once we start acquiring, no interrupt can
        // preempt us and potentially deadlock by re-attempting this lock.
        let sie_enabled = read_sie();
        disable_interrupts();

        while self.locked.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
        // Compiler barrier: ensure no operations inside the critical section
        // are reordered before the lock acquisition.
        core::sync::atomic::fence(Ordering::SeqCst);

        IntSpinLockGuard {
            lock: self,
            sie_enabled,
        }
    }

    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct IntSpinLockGuard<'a, T> {
    lock: &'a IntSpinLock<T>,
    sie_enabled: bool,
}

impl<T> Deref for IntSpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for IntSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The guard holds the lock, so we have exclusive access.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IntSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // Release the lock first, then restore interrupt state.
        // Order matters: if we restored interrupts first, an ISR could
        // fire and deadlock trying to acquire this lock.
        self.lock.unlock();
        if self.sie_enabled {
            enable_interrupts();
        }
    }
}

// ── RISC-V CSR helpers ──────────────────────────────────────────────────
// Minimal CSR access for interrupt state management.
// We use inline assembly instead of the `riscv` crate's high-level API
// to keep this module self-contained and avoid dependency on the crate's
// sstatus abstraction which may change between versions.

#[cfg(target_arch = "riscv64")]
/// Read the SIE (Supervisor Interrupt Enable) bit from sstatus.
fn read_sie() -> bool {
    let sstatus: usize;
    unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) sstatus) };
    // SIE is bit 1 of sstatus
    (sstatus & (1 << 1)) != 0
}

#[cfg(target_arch = "riscv64")]
/// Clear the SIE bit in sstatus (disable S-mode interrupts).
fn disable_interrupts() {
    unsafe { core::arch::asm!("csrci sstatus, 2") };
}

#[cfg(target_arch = "riscv64")]
/// Set the SIE bit in sstatus (enable S-mode interrupts).
fn enable_interrupts() {
    unsafe { core::arch::asm!("csrsi sstatus, 2") };
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── IntSpinLock Tests ──");

    // Test 1: Lock and unlock
    crate::test::run_test("intspinlock_lock_unlock", || {
        let lock = IntSpinLock::new(42usize);
        let guard = lock.lock();
        *guard == 42
    });

    // Test 2: Modify through guard
    crate::test::run_test("intspinlock_modify_via_guard", || {
        let lock = IntSpinLock::new(0usize);
        {
            let mut guard = lock.lock();
            *guard = 100;
        }
        let guard = lock.lock();
        *guard == 100
    });

    // Test 3: Guard drops and releases lock
    crate::test::run_test("intspinlock_guard_drop_releases", || {
        let lock = IntSpinLock::new(1usize);
        {
            let _g = lock.lock();
        }
        let guard = lock.lock();
        *guard == 1
    });

    // Test 4: Sequential locks
    crate::test::run_test("intspinlock_sequential_locks", || {
        let lock = IntSpinLock::new(0usize);
        for i in 0..10usize {
            let mut g = lock.lock();
            *g = i;
        }
        let g = lock.lock();
        *g == 9
    });

    // Test 5: Complex data
    crate::test::run_test("intspinlock_complex_data", || {
        let lock = IntSpinLock::new([0usize; 4]);
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
}
