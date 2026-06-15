//! timerfd implementation for Go runtime timers and TUI animations.
//!
//! Go uses timerfd + epoll to drive goroutine timers:
//! 1. timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK|TFD_CLOEXEC) → fd
//! 2. epoll_ctl(epfd, EPOLL_CTL_ADD, timerfd, {EPOLLIN, ...})
//! 3. timerfd_settime(timerfd, 0, &new_value, NULL) — arms the timer
//! 4. epoll_wait returns timerfd readable when timer expires
//! 5. read(timerfd, &buf, 8) → returns expiration count, re-arms repeating timers
//!
//! Kernel timer tick (handle_timer → tick_timerfds) checks all armed timerfds
//! on every ~10ms tick and marks them as expired.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use spin::Mutex;

/// Linux `struct itimerspec` (parsed from user pointer):
///   struct timespec { time_t tv_sec; long tv_nsec; }  // 16 bytes
///   struct itimerspec { struct it_interval; struct it_value; }  // 32 bytes
const ITIMERSPEC_SIZE: usize = 32;

/// State for a single timerfd.
#[derive(Clone, Copy)]
struct TimerfdState {
    /// Absolute uptime_ms when the timer next fires (0 = disarmed).
    next_expiry: u64,
    /// Repeat interval in ms (0 = one-shot).
    interval_ms: u64,
    /// Number of times the timer has expired since last read.
    expiry_count: u64,
}

impl TimerfdState {
    const fn disarmed() -> Self {
        Self {
            next_expiry: 0,
            interval_ms: 0,
            expiry_count: 0,
        }
    }
}

/// Global timerfd state: fd → TimerfdState.
static TIMERFDS: Mutex<BTreeMap<usize, TimerfdState>> = Mutex::new(BTreeMap::new());

/// Create a timerfd. Returns the fd number or negative errno.
///
/// `clockid` must be CLOCK_MONOTONIC (1) or CLOCK_REALTIME (0).
/// `_flags` contains TFD_NONBLOCK / TFD_CLOEXEC (ignored — we always non-block).
pub fn sys_timerfd_create(clockid: usize, _flags: usize) -> isize {
    if clockid != 0 && clockid != 1 {
        return -22; // EINVAL
    }

    let fd = match crate::process::with_fd_table(|ft| {
        ft.alloc_special_fd(
            alloc::format!("timerfd"),
            0, // O_RDONLY
            crate::driver::fs::FdType::Timerfd,
        )
    }) {
        Some(fd) => fd,
        None => return -24, // EMFILE
    };

    TIMERFDS.lock().insert(fd, TimerfdState::disarmed());

    // Also register in TIMERFD_STATES for epoll integration
    super::register_timerfd(fd);

    fd as isize
}

/// Set the timer. `new_value_ptr` points to `struct itimerspec`.
///
/// RISC-V Linux struct itimerspec layout (32 bytes):
///   it_interval: { tv_sec(i64)@0, tv_nsec(i64)@8 }
///   it_value:    { tv_sec(i64)@16, tv_nsec(i64)@24 }
///
/// When `flags & TFD_TIMER_ABSTIME` (1), it_value is absolute (we treat as relative).
pub fn sys_timerfd_settime(fd: usize, _flags: usize, new_value_ptr: usize, _old_value_ptr: usize) -> isize {
    if new_value_ptr == 0 {
        return -14; // EFAULT
    }

    // Read it_value (offset 16-31): the initial expiration
    let val_sec = crate::syscall::user_read::<i64>(new_value_ptr + 16);
    let val_nsec = crate::syscall::user_read::<i64>(new_value_ptr + 24);

    // Read it_interval (offset 0-15): the repeat interval
    let int_sec = crate::syscall::user_read::<i64>(new_value_ptr);
    let int_nsec = crate::syscall::user_read::<i64>(new_value_ptr + 8);

    let now = crate::arch::platform::uptime_ms();

    let mut state = TIMERFDS.lock();
    let tfd = match state.get_mut(&fd) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    if val_sec == 0 && val_nsec == 0 {
        // Disarm
        tfd.next_expiry = 0;
        tfd.interval_ms = 0;
        super::set_timerfd_triggered(fd, false);
        return 0;
    }

    // Convert it_value to milliseconds from now
    let initial_ms = secs_nsec_to_ms(val_sec, val_nsec);
    tfd.next_expiry = now + initial_ms;

    // Convert it_interval to milliseconds (0 if one-shot)
    if int_sec != 0 || int_nsec != 0 {
        tfd.interval_ms = secs_nsec_to_ms(int_sec, int_nsec);
    } else {
        tfd.interval_ms = 0;
    }

    tfd.expiry_count = 0;

    drop(state);
    0
}

/// Convert (seconds, nanoseconds) to milliseconds.
fn secs_nsec_to_ms(sec: i64, nsec: i64) -> u64 {
    let ms = (sec as u64).saturating_mul(1000);
    let nsec_ms = ((nsec as u64) + 999_999) / 1_000_000; // round up
    ms.saturating_add(nsec_ms)
}

/// Read from a timerfd. Returns 8 bytes (expiration count) or -EAGAIN.
pub fn timerfd_read(fd: usize, buf: usize, len: usize) -> isize {
    if len < 8 {
        return -22; // EINVAL
    }

    let mut state = TIMERFDS.lock();
    let tfd = match state.get_mut(&fd) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    if tfd.expiry_count == 0 {
        return -11; // EAGAIN (non-blocking — we always set O_NONBLOCK)
    }

    let count = tfd.expiry_count;
    tfd.expiry_count = 0;
    drop(state);

    // Clear the epoll-triggered flag
    super::set_timerfd_triggered(fd, false);

    crate::syscall::user_write::<u64>(buf, count);
    8
}

/// Close a timerfd — remove all state.
pub fn close_timerfd(fd: usize) {
    TIMERFDS.lock().remove(&fd);
    super::unregister_timerfd(fd);
}

/// Check all timerfds for expiry. Called from `handle_timer()` on every tick.
/// Must be safe to call from timer ISR — uses regular Mutex (timer ISR no
/// longer calls schedule() directly due to NEED_RESCHED pattern).
pub fn tick_timerfds() {
    let now = crate::arch::platform::uptime_ms();
    let mut expired_fds: Vec<usize> = Vec::new();

    {
        let state = TIMERFDS.lock();
        for (&fd, tfd) in state.iter() {
            if tfd.next_expiry != 0 && now >= tfd.next_expiry {
                expired_fds.push(fd);
            }
        }
    }

    if !expired_fds.is_empty() {
        let mut state = TIMERFDS.lock();
        for &fd in &expired_fds {
            if let Some(tfd) = state.get_mut(&fd) {
                tfd.expiry_count = tfd.expiry_count.saturating_add(1);
                if tfd.interval_ms > 0 {
                    tfd.next_expiry = now + tfd.interval_ms;
                } else {
                    tfd.next_expiry = 0;
                }
            }
        }
    }

    for fd in expired_fds {
        super::set_timerfd_triggered(fd, true);
        super::wake_waiters_for_fd(fd);
    }
}
