//! epoll(7) implementation for x86_64 Linux compatibility.
//!
//! Supports epoll_create1, epoll_ctl (ADD/MOD/DEL), and epoll_wait.
//! Used by Go runtime for netpoll (timer-driven goroutine scheduling).

pub mod eventfd;
pub mod timerfd;

/// Check if a timerfd has been triggered (for FdType is_readable dispatch).
pub fn timerfd_peek(fd: usize) -> bool {
    let states = TIMERFD_STATES.lock();
    states.get(&fd).copied().unwrap_or(false)
}

use alloc::collections::BTreeMap;
use spin::Mutex;

/// Tracks whether timerfd fds have been triggered.
/// Set to true by `tick_timerfds()` when the timer fires.
/// Cleared to false by `epoll_wait` (when the event is consumed) and by
/// `timerfd_read` (when the expiration count is read).
static TIMERFD_STATES: Mutex<BTreeMap<usize, bool>> = Mutex::new(BTreeMap::new());

/// Register a timerfd in the triggered-state map.
pub fn register_timerfd(fd: usize) {
    TIMERFD_STATES.lock().insert(fd, false);
}

/// Remove a timerfd from the triggered-state map.
pub fn unregister_timerfd(fd: usize) {
    TIMERFD_STATES.lock().remove(&fd);
}

/// Set the triggered flag for a timerfd (called by tick_timerfds).
pub fn set_timerfd_triggered(fd: usize, triggered: bool) {
    TIMERFD_STATES.lock().insert(fd, triggered);
}

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
static EPOLL_WAITERS: Mutex<BTreeMap<usize, alloc::vec::Vec<usize>>> = Mutex::new(BTreeMap::new());

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
    let fd = match crate::process::with_fd_table(|ft| {
        ft.alloc_special_fd(alloc::format!("epoll"), 0, crate::driver::fs::FdType::Epoll)
    }) {
        Some(fd) => fd,
        None => return -24, // EMFILE
    };

    EPOLL_INSTANCES.lock().insert(fd, EpollInstance::new());
    fd as isize
}

/// Release epoll/timer state associated with a closing process fd.
pub fn close_fd(fd: usize) {
    let mut instances = EPOLL_INSTANCES.lock();
    let removed_instance = instances.remove(&fd).is_some();
    let mut removed_entries = 0usize;
    for instance in instances.values_mut() {
        if instance.entries.remove(&fd).is_some() {
            removed_entries += 1;
        }
    }
    let removed_waiters = EPOLL_WAITERS
        .lock()
        .remove(&fd)
        .map_or(0, |slots| slots.len());
    let removed_timer = TIMERFD_STATES.lock().remove(&fd).is_some();
    let _ = (
        removed_instance,
        removed_entries,
        removed_waiters,
        removed_timer,
    );
}

/// Wake tasks blocked in epoll_wait for any epoll instance watching `fd`.
/// Called from timer interrupt context — must use try_lock to avoid deadlock.
pub fn wake_waiters_for_fd(fd: usize) {
    let mut epfds = alloc::vec::Vec::new();
    {
        let instances = match EPOLL_INSTANCES.try_lock() {
            Some(g) => g,
            None => return,
        };
        for (&epfd, instance) in instances.iter() {
            if instance.entries.contains_key(&fd) {
                epfds.push(epfd);
            }
        }
    }

    let mut waiters = match EPOLL_WAITERS.try_lock() {
        Some(g) => g,
        None => return,
    };
    for epfd in epfds {
        if let Some(slots) = waiters.get_mut(&epfd) {
            for &proc_idx in slots.iter() {
                crate::sched::wake_task(proc_idx);
            }
            slots.clear();
        }
    }
    waiters.retain(|_, slots| !slots.is_empty());
}

fn register_waiter(epfd: usize) -> usize {
    let proc_idx = crate::process::current_index();
    let mut waiters = EPOLL_WAITERS.lock();
    let slots = waiters.entry(epfd).or_insert_with(alloc::vec::Vec::new);
    if !slots.contains(&proc_idx) {
        slots.push(proc_idx);
    }
    proc_idx
}

fn unregister_waiter(epfd: usize, proc_idx: usize) {
    let mut waiters = EPOLL_WAITERS.lock();
    if let Some(slots) = waiters.get_mut(&epfd) {
        slots.retain(|&slot| slot != proc_idx);
        if slots.is_empty() {
            waiters.remove(&epfd);
        }
    }
}

fn collect_ready_events(
    epfd: usize,
    max_events: usize,
) -> Result<alloc::vec::Vec<(u32, u64)>, isize> {
    let mut ready_events: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();
    let mut instances = EPOLL_INSTANCES.lock();
    let instance = instances.get_mut(&epfd).ok_or(-9_isize)?; // EBADF

    for (&fd, entry) in &mut instance.entries {
        if ready_events.len() >= max_events {
            break;
        }

        let mut revents = 0u32;
        let fd_desc = crate::process::with_fd_table(|ft| ft.get(fd).cloned());

        if let Some(ref desc) = fd_desc {
            if entry.event.events & EPOLLIN != 0 && desc.is_readable() {
                revents |= EPOLLIN;
            }
            if entry.event.events & EPOLLOUT != 0 && desc.is_writable() {
                revents |= EPOLLOUT;
            }
            if matches!(desc.fd_type, crate::driver::fs::FdType::Timerfd) && revents & EPOLLIN != 0
            {
                TIMERFD_STATES.lock().insert(fd, false);
            }
        } else {
            revents = EPOLLHUP;
        }

        let is_et = entry.event.events & EPOLLET != 0;
        if is_et && revents == entry.last_revents && revents != EPOLLHUP {
            continue;
        }
        entry.last_revents = revents;

        if revents != 0 {
            ready_events.push((revents, entry.event.data));
        }
    }

    Ok(ready_events)
}

fn write_ready_events(events_ptr: usize, ready_events: &[(u32, u64)]) -> isize {
    for (i, &(rev, data)) in ready_events.iter().enumerate() {
        write_epoll_event(
            events_ptr + i * EPOLL_EVENT_SIZE,
            EpollEvent { events: rev, data },
        );
    }
    ready_events.len() as isize
}

fn epoll_target_supported(desc: &crate::driver::fs::FileDescriptor) -> bool {
    match &desc.fd_type {
        crate::driver::fs::FdType::Stdio
        | crate::driver::fs::FdType::PipeRead
        | crate::driver::fs::FdType::PipeWrite => true,
        crate::driver::fs::FdType::Eventfd | crate::driver::fs::FdType::Timerfd => true,
        crate::driver::fs::FdType::Epoll
        | crate::driver::fs::FdType::File
        | crate::driver::fs::FdType::FakeFile(_)
        | crate::driver::fs::FdType::VirtualFile
        | crate::driver::fs::FdType::Urandom
        | crate::driver::fs::FdType::VfsFile(_)
        | crate::driver::fs::FdType::Ext4File(_) => false,
    }
}

fn validate_epoll_target(fd: usize) -> Result<(), isize> {
    let fd_desc = crate::process::with_fd_table(|ft| ft.get(fd).cloned()).ok_or(-9_isize)?;
    if !epoll_target_supported(&fd_desc) {
        crate::klog!(
            INFO,
            "[epoll] fd={} type={:?} name={} not epollable",
            fd,
            fd_desc.fd_type,
            fd_desc.name
        );
        return Err(-1); // EPERM: regular files and directories are not epollable.
    }
    Ok(())
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
            if let Err(e) = validate_epoll_target(fd) {
                return e;
            }
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
            if let Err(e) = validate_epoll_target(fd) {
                return e;
            }
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
/// Go's netpoller relies on Linux epoll_wait semantics: timeout=0 polls,
/// timeout>0 blocks until readiness or timeout, and timeout<0 blocks until
/// readiness. Returning 0 immediately for blocking waits makes Go spin in its
/// timer loop.
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

    let ready_events = match collect_ready_events(epfd, max_events) {
        Ok(events) => events,
        Err(e) => return e,
    };
    if !ready_events.is_empty() {
        return write_ready_events(events_ptr, &ready_events);
    }

    if timeout_ms == 0 {
        return 0;
    }

    let proc_idx = register_waiter(epfd);
    let ready_events = match collect_ready_events(epfd, max_events) {
        Ok(events) => events,
        Err(e) => {
            unregister_waiter(epfd, proc_idx);
            return e;
        }
    };
    if !ready_events.is_empty() {
        unregister_waiter(epfd, proc_idx);
        return write_ready_events(events_ptr, &ready_events);
    }

    let deadline = if timeout_ms > 0 {
        Some(crate::arch::platform::uptime_ms().saturating_add(timeout_ms as u64))
    } else {
        None
    };

    loop {
        if let Some(wake_tick) = deadline {
            let now = crate::arch::platform::uptime_ms();
            if now >= wake_tick {
                unregister_waiter(epfd, proc_idx);
                return 0;
            }
            crate::sched::sleep_until(wake_tick);
        } else {
            crate::sched::schedule_block();
        }

        let ready_events = match collect_ready_events(epfd, max_events) {
            Ok(events) => events,
            Err(e) => {
                unregister_waiter(epfd, proc_idx);
                return e;
            }
        };
        if !ready_events.is_empty() {
            unregister_waiter(epfd, proc_idx);
            return write_ready_events(events_ptr, &ready_events);
        }

        if deadline.is_some() && crate::arch::platform::uptime_ms() >= deadline.unwrap_or(0) {
            unregister_waiter(epfd, proc_idx);
            return 0;
        }
    }
}
