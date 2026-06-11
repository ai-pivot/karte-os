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
        ft.alloc_special_fd(
            alloc::format!("eventfd_{}", NEXT_EVENTFD.load(Ordering::Relaxed)),
            0,
            crate::driver::fs::FdType::Eventfd,
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
    fd as isize
}

/// Remove eventfd state when the owning file descriptor is closed or the
/// process exits without explicitly closing it.
pub fn close_eventfd(fd: i32) {
    EVENTFD_STATES.lock().remove(&fd);
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
        if crate::syscall::debug_second_xbot_run_active() {
            // #region agent log
            crate::console_println!(
                r#"{{"sessionId":"9230b7","runId":"pre-fix","hypothesisId":"H21,H22,H25","location":"kernel/src/syscall/epoll/eventfd.rs:eventfd_read","message":"eventfd read","data":{{"fd":{},"counter":{},"len":{},"pid":{},"proc":{},"uptime_ms":{}}},"timestamp":{}}}"#,
                fd,
                val,
                len,
                crate::process::current_pid(),
                crate::process::current_index(),
                crate::arch::platform::uptime_ms(),
                crate::arch::platform::uptime_ms(),
            );
            // #endregion
        }
        if val == 0 {
            return -11; // EAGAIN — non-blocking, nothing to read
        }
        *counter = 0;
        // Write the 8-byte value to user buffer
        crate::syscall::user_write::<u64>(buf, val);
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
    let val = crate::syscall::user_read::<u64>(buf);
    if val == u64::MAX {
        return -22; // EINVAL
    }
    {
        let mut states = EVENTFD_STATES.lock();
        if let Some(counter) = states.get_mut(&fd) {
            let old = *counter;
            *counter = counter.saturating_add(val);
            // #region agent log
            crate::console_println!(
                r#"{{"sessionId":"9230b7","runId":"pre-fix","hypothesisId":"H2","location":"kernel/src/syscall/epoll/eventfd.rs:eventfd_write","message":"eventfd write","data":{{"fd":{},"val":{},"old":{},"new":{},"pid":{},"proc":{},"uptime_ms":{}}},"timestamp":{}}}"#,
                fd,
                val,
                old,
                *counter,
                crate::process::current_pid(),
                crate::process::current_index(),
                crate::arch::platform::uptime_ms(),
                crate::arch::platform::uptime_ms(),
            );
            // #endregion
        } else {
            crate::console_println!("[eventfd] write fd={} EBADF!", fd);
            return -9; // EBADF
        }
    }
    super::wake_waiters_for_fd(fd as usize);
    8
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
