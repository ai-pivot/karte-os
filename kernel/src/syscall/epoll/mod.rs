//! epoll(7) implementation for x86_64 Linux compatibility.
//!
//! Supports epoll_create1, epoll_ctl (ADD/MOD/DEL), and epoll_wait.
//! Used by Go runtime for netpoll (timer-driven goroutine scheduling).

pub mod eventfd;

/// Check if a timerfd has been triggered (for FdType is_readable dispatch).
pub fn timerfd_peek(fd: usize) -> bool {
    let states = TIMERFD_STATES.lock();
    states.get(&fd).copied().unwrap_or(false)
}

use alloc::collections::BTreeMap;
use spin::Mutex;

/// Tracks whether timerfd/eventfd fds have been triggered (edge-triggered semantics).
/// When timerfd_settime is called or a timer fires, the flag is set to true.
/// epoll_wait reads and clears it.
static TIMERFD_STATES: Mutex<BTreeMap<usize, bool>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

const EPOLL_EVENT_SIZE: usize = 12;

fn read_epoll_event(event_ptr: usize) -> EpollEvent {
    let raw = crate::syscall::user_read_bytes(event_ptr, EPOLL_EVENT_SIZE);
    EpollEvent {
        events: u32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]),
        data: u64::from_ne_bytes([
            raw[4], raw[5], raw[6], raw[7], raw[8], raw[9], raw[10], raw[11],
        ]),
    }
}

fn write_epoll_event(event_ptr: usize, event: EpollEvent) {
    let mut raw = [0u8; EPOLL_EVENT_SIZE];
    raw[0..4].copy_from_slice(&event.events.to_ne_bytes());
    raw[4..12].copy_from_slice(&event.data.to_ne_bytes());
    crate::syscall::user_write_bytes(event_ptr, &raw);
}

const EPOLLET: u32 = 0x80000000;

/// Per-fd epoll state
#[derive(Clone, Copy)]
struct EpollEntry {
    event: EpollEvent,
    /// For EPOLLET: last reported revents. Only report again when revents changes.
    last_revents: u32,
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
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

/// syscall 291: epoll_create1(flags) — create an epoll instance
pub fn sys_epoll_create1(_flags: usize) -> isize {
    let mut next = NEXT_EPOLL_FD.lock();
    let fd = *next;
    *next += 1;
    drop(next);

    EPOLL_INSTANCES.lock().insert(fd, EpollInstance::new());
    crate::console_println!("[epoll] create1 → fd={}", fd);
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
            if event_ptr == 0 {
                return -14; // EFAULT
            }
            let event = read_epoll_event(event_ptr);
            let raw_bytes = crate::syscall::user_read_bytes(event_ptr, EPOLL_EVENT_SIZE);
            let ev_events = event.events;
            let ev_data = event.data;
            crate::console_println!(
                "[epoll_ctl] ADD epfd={:#x} fd={} events={:#x} data={:#x} ptr={:#x} raw={:02x?}",
                epfd,
                fd,
                ev_events,
                ev_data,
                event_ptr,
                &raw_bytes[..EPOLL_EVENT_SIZE]
            );
            instance.entries.insert(
                fd,
                EpollEntry {
                    event,
                    last_revents: 0,
                },
            );
        }
        EPOLL_CTL_MOD => {
            if event_ptr == 0 {
                return -14;
            }
            let event = read_epoll_event(event_ptr);
            match instance.entries.get_mut(&fd) {
                Some(entry) => {
                    entry.event = event;
                    entry.last_revents = 0; // reset ET state on MOD
                }
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

    // Collect ready events before writing to user memory
    let mut ready_events: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();

    let mut instances = EPOLL_INSTANCES.lock();
    if let Some(instance) = instances.get_mut(&epfd) {
        for (&fd, entry) in &mut instance.entries {
            if ready_events.len() >= max_events {
                break;
            }

            let mut revents = 0u32;

            // Unified fd ready check — look up FdTable and dispatch by FdType.
            let fd_desc = crate::process::with_fd_table(|ft| ft.get(fd).cloned());

            if let Some(desc) = fd_desc {
                // Readable check
                if entry.event.events & EPOLLIN != 0 && desc.is_readable() {
                    revents |= EPOLLIN;
                }
                // Writable check
                if entry.event.events & EPOLLOUT != 0 && desc.is_writable() {
                    revents |= EPOLLOUT;
                }
                // Timerfd edge-triggered: clear trigger flag after reporting
                if matches!(desc.fd_type, crate::driver::fs::FdType::Timerfd)
                    && revents & EPOLLIN != 0
                {
                    TIMERFD_STATES.lock().insert(fd, false);
                }
            } else {
                // fd closed/invalid — report EPOLLHUP so caller can clean up
                revents = EPOLLHUP;
            }

            let is_et = entry.event.events & EPOLLET != 0;

            // EPOLLET: only report when revents changes (edge-triggered semantics)
            if is_et && revents == entry.last_revents && revents != EPOLLHUP {
                continue; // no change since last report
            }
            entry.last_revents = revents;

            if revents != 0 {
                ready_events.push((revents, entry.event.data));
            }
        }
    }

    let ready_count = ready_events.len();
    if ready_count > 0 {
        for (i, &(rev, data)) in ready_events.iter().enumerate() {
            crate::console_println!(
                "[epoll_wait] event[{}] events={:#x} data={:#x}",
                i,
                rev,
                data
            );
        }
        for (i, &(rev, data)) in ready_events.iter().enumerate() {
            write_epoll_event(
                events_ptr + i * EPOLL_EVENT_SIZE,
                EpollEvent { events: rev, data },
            );
        }
        let verify = crate::syscall::user_read_bytes(events_ptr, EPOLL_EVENT_SIZE);
        crate::console_println!(
            "[epoll_wait] wrote to ptr={:#x} verify={:02x?}",
            events_ptr,
            &verify[..EPOLL_EVENT_SIZE]
        );
        return ready_count as isize;
    }

    // No events ready
    if timeout_ms == 0 {
        return 0; // Non-blocking
    }

    // Positive timeout or infinite wait: block the task until timeout
    drop(instances);
    if timeout_ms > 0 {
        let target = crate::arch::platform::uptime_ms() + timeout_ms as u64;
        crate::sched::sleep_until(target);
    } else {
        // Infinite wait: block indefinitely until an event triggers
        // (would need wake from epoll_ctl/add, for now block 100ms and retry)
        let target = crate::arch::platform::uptime_ms() + 100;
        crate::sched::sleep_until(target);
    }
    0
}
