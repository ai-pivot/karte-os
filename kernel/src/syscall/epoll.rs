//! epoll(7) implementation for x86_64 Linux compatibility.
//!
//! Supports epoll_create1, epoll_ctl (ADD/MOD/DEL), and epoll_wait.
//! Used by Go runtime for netpoll (timer-driven goroutine scheduling).

use alloc::collections::BTreeMap;
use spin::Mutex;

/// epoll_event structure matching Linux ABI
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct EpollEvent {
    pub events: u32, // epoll events (EPOLLIN, EPOLLOUT, etc.)
    pub data: u64,   // user data
}

/// Per-fd epoll state
#[derive(Clone, Copy)]
struct EpollEntry {
    event: EpollEvent,
}

/// An epoll instance
pub struct EpollInstance {
    entries: BTreeMap<usize, EpollEntry>,
}

impl EpollInstance {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

/// Global epoll state: fd → EpollInstance
static EPOLL_INSTANCES: Mutex<BTreeMap<usize, EpollInstance>> = Mutex::new(BTreeMap::new());
static NEXT_EPOLL_FD: Mutex<usize> = Mutex::new(100); // Start from fd 100

// epoll constants
const EPOLL_CTL_ADD: usize = 1;
const EPOLL_CTL_DEL: usize = 2;
const EPOLL_CTL_MOD: usize = 3;

const EPOLLIN: u32 = 0x001;

/// syscall 291: epoll_create1(flags) — create an epoll instance
pub fn sys_epoll_create1(_flags: usize) -> isize {
    let mut next = NEXT_EPOLL_FD.lock();
    let fd = *next;
    *next += 1;
    drop(next);

    EPOLL_INSTANCES.lock().insert(fd, EpollInstance::new());
    fd as isize
}

/// syscall 233: epoll_ctl(epfd, op, fd, event) — control an epoll instance
pub fn sys_epoll_ctl(epfd: usize, op: usize, fd: usize, event_ptr: usize) -> isize {
    let mut instances = EPOLL_INSTANCES.lock();
    let instance = match instances.get_mut(&epfd) {
        Some(i) => i,
        None => return -9, // EBADF
    };

    match op {
        EPOLL_CTL_ADD => {
            let event = if event_ptr != 0 {
                unsafe { *(event_ptr as *const EpollEvent) }
            } else {
                return -14; // EFAULT
            };
            instance.entries.insert(fd, EpollEntry { event });
        }
        EPOLL_CTL_MOD => {
            let event = if event_ptr != 0 {
                unsafe { *(event_ptr as *const EpollEvent) }
            } else {
                return -14;
            };
            match instance.entries.get_mut(&fd) {
                Some(entry) => entry.event = event,
                None => return -2, // ENOENT
            }
        }
        EPOLL_CTL_DEL => {
            if instance.entries.remove(&fd).is_none() {
                return -2; // ENOENT
            }
        }
        _ => return -22, // EINVAL
    }
    0
}

/// syscall 232: epoll_wait(epfd, events, maxevents, timeout)
///
/// For Go runtime compatibility:
/// - Timer fds are always reported as ready (Go uses timerfd for scheduler ticks)
/// - Stdin is checked via tty buffer
/// - Other fds are reported as ready if watched for input
pub fn sys_epoll_wait(
    epfd: usize,
    events_ptr: usize,
    max_events: usize,
    timeout_ms: isize,
) -> isize {
    if max_events == 0 || max_events > 1024 {
        return -22; // EINVAL
    }
    if events_ptr == 0 {
        return -14; // EFAULT
    }

    // Validate epfd
    {
        let instances = EPOLL_INSTANCES.lock();
        if !instances.contains_key(&epfd) {
            return -9; // EBADF
        }
    }

    let output =
        unsafe { core::slice::from_raw_parts_mut(events_ptr as *mut EpollEvent, max_events) };

    // Try to collect ready events
    let mut ready_count = 0usize;

    let instances = EPOLL_INSTANCES.lock();
    if let Some(instance) = instances.get(&epfd) {
        for (&fd, entry) in &instance.entries {
            if ready_count >= max_events {
                break;
            }

            let mut revents = 0u32;

            if fd == 0 {
                // stdin — check if tty has input available
                if entry.event.events & EPOLLIN != 0 {
                    if crate::arch::uart::has_data() {
                        revents = EPOLLIN;
                    }
                }
            } else if fd >= 100 {
                // Internal fds (timerfd, eventfd, etc.) — always report as ready
                // This is critical for Go runtime scheduler: timerfd_create returns
                // fd >= 100, and Go uses timerfd for goroutine preemption ticks.
                if entry.event.events & EPOLLIN != 0 {
                    revents = EPOLLIN;
                }
            } else {
                // Other fds — report as ready for any watched events
                // Go uses this for pipe fds, socket fds, etc.
                revents = entry.event.events;
            }

            if revents != 0 {
                output[ready_count].events = revents;
                output[ready_count].data = entry.event.data;
                ready_count += 1;
            }
        }
    }

    if ready_count > 0 {
        return ready_count as isize;
    }

    // No events ready
    if timeout_ms == 0 {
        return 0; // Non-blocking
    }

    // For positive timeout or infinite wait, yield and return 0
    // Go runtime handles retrying epoll_wait in a loop
    drop(instances);
    crate::sched::schedule();
    0
}
