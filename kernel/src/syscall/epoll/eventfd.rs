//! eventfd2 implementation for Go runtime netpoller wakeup.
//!
//! Go uses eventfd to wake up M threads blocked in epoll_wait.
//! When a goroutine becomes runnable, Go writes to the eventfd,
//! which causes epoll_wait to return, allowing the M to pick up
//! the new goroutine.
//!
//! Implementation: a simple 64-bit counter with semaphore semantics.
//! - write(fd, &val, 8): adds val to the counter (max once, then blocks if counter would overflow)
//! - read(fd, &val, 8): returns the counter value and resets to 0

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;

/// Global eventfd state: fd -> counter value.
static EVENTFD_STATES: SpinLock<BTreeMap<i32, u64>> = SpinLock::new(BTreeMap::new());
static NEXT_EVENTFD: AtomicUsize = AtomicUsize::new(200); // Start from fd=200

/// eventfd2(initval, flags) — create an eventfd.
pub fn sys_eventfd2(initval: usize, _flags: usize) -> isize {
    // Allocate fd number from the process's fd table
    let fd = crate::process::with_fd_table(|ft| {
        ft.alloc(
            alloc::format!("eventfd_{}", NEXT_EVENTFD.load(Ordering::Relaxed)),
            0,
        )
    });
    let fd = match fd {
        Some(f) => f as i32,
        None => {
            crate::console_println!("[eventfd] FAILED to alloc fd!");
            return -24; // EMFILE
        }
    };
    NEXT_EVENTFD.fetch_add(1, Ordering::Relaxed);
    {
        let mut states = EVENTFD_STATES.lock();
        states.insert(fd, initval as u64);
    }
    // Update FdTable entry's type to Eventfd
    crate::process::with_fd_table(|ft| {
        if let Some(desc) = ft.get_mut(fd as usize) {
            desc.fd_type = crate::driver::fs::FdType::Eventfd;
            desc.fd_num = fd as usize;
        }
    });
    fd as isize
}

/// Check if an fd is an eventfd.
pub fn is_eventfd(fd: i32) -> bool {
    let states = EVENTFD_STATES.lock();
    states.contains_key(&fd)
}

/// Read from an eventfd — returns counter value and resets to 0.
/// Returns the number of bytes read (8) or negative error.
pub fn eventfd_read(fd: i32, buf: usize, len: usize) -> isize {
    if len < 8 {
        return -22; // EINVAL
    }
    let mut states = EVENTFD_STATES.lock();
    if let Some(counter) = states.get_mut(&fd) {
        let val = *counter;
        if val == 0 {
            return -11; // EAGAIN — non-blocking, nothing to read
        }
        *counter = 0;
        // Write the 8-byte value to user buffer
        unsafe {
            core::ptr::write_volatile(buf as *mut u64, val);
        }
        8
    } else {
        -9 // EBADF
    }
}

/// Write to an eventfd — adds value to counter.
/// Returns the number of bytes written (8) or negative error.
pub fn eventfd_write(fd: i32, buf: usize, len: usize) -> isize {
    if len < 8 {
        return -22; // EINVAL
    }
    let val = unsafe { core::ptr::read_volatile(buf as *const u64) };
    if val == u64::MAX {
        return -22; // EINVAL
    }
    let mut states = EVENTFD_STATES.lock();
    if let Some(counter) = states.get_mut(&fd) {
        let old = *counter;
        *counter = counter.saturating_add(val);
        8
    } else {
        crate::console_println!("[eventfd] write fd={} EBADF!", fd);
        -9 // EBADF
    }
}

/// Peek at eventfd counter without consuming it.
/// Returns 0 if fd is not an eventfd or counter is 0.
pub fn eventfd_peek(fd: i32) -> u64 {
    let states = EVENTFD_STATES.lock();
    states.get(&fd).copied().unwrap_or(0)
}

/// Alias used by FdType::is_readable dispatch.
pub fn eventfd_peek_by_fd(fd: usize) -> u64 {
    eventfd_peek(fd as i32)
}
