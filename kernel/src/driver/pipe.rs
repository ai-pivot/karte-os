// kernel/src/driver/pipe.rs — Anonymous pipe support for inter-process communication.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::sync::spinlock::SpinLock;

/// Maximum number of concurrent pipes in the system.
pub const MAX_PIPES: usize = 16;

/// Pipe buffer size (4 KB ring buffer).
pub const PIPE_BUF_SIZE: usize = 4096;

/// Error: broken pipe (write to pipe with no reader).
pub const EPIPE: isize = -32;

/// An anonymous pipe with a ring buffer.
pub struct Pipe {
    /// Ring buffer data.
    buffer: [u8; PIPE_BUF_SIZE],
    /// Read position (consumer).
    read_pos: usize,
    /// Write position (producer).
    write_pos: usize,
    /// Number of bytes currently in the buffer.
    data_len: usize,
    /// Whether the read end has been closed.
    read_closed: bool,
    /// Whether the write end has been closed.
    write_closed: bool,
    /// Process index of the task blocked on reading (if any).
    reader_blocked: AtomicUsize, // usize::MAX = none
    /// Process index of the task blocked on writing (if any).
    writer_blocked: AtomicUsize, // usize::MAX = none
}

impl Pipe {
    pub const fn new() -> Self {
        Pipe {
            buffer: [0u8; PIPE_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            data_len: 0,
            read_closed: false,
            write_closed: false,
            reader_blocked: AtomicUsize::new(usize::MAX),
            writer_blocked: AtomicUsize::new(usize::MAX),
        }
    }

    /// Read up to `len` bytes from the pipe into user buffer at `buf`.
    /// Returns number of bytes read, 0 for EOF, EPIPE for error.
    /// May block if pipe is empty and write end is still open.
    pub fn read(&mut self, buf: usize, len: usize) -> isize {
        if len == 0 || buf == 0 {
            return 0;
        }

        if self.data_len == 0 {
            if self.write_closed {
                return 0; // EOF
            }
            return -1; // caller should block (special signal)
        }

        let to_read = core::cmp::min(len, self.data_len);
        for i in 0..to_read {
            let byte = self.buffer[self.read_pos];
            crate::syscall::user_write_u8(buf + i, byte);
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
        }
        self.data_len -= to_read;

        // If a writer was blocked, wake it up
        if to_read > 0 {
            let writer = self.writer_blocked.swap(usize::MAX, Ordering::AcqRel);
            if writer != usize::MAX {
                crate::sched::wake_task(writer);
            }
        }

        to_read as isize
    }

    /// Write up to `len` bytes from user buffer at `buf` into the pipe.
    /// Returns number of bytes written, or EPIPE if read end is closed.
    /// May block if pipe is full.
    pub fn write(&mut self, buf: usize, len: usize) -> isize {
        if len == 0 || buf == 0 {
            return 0;
        }

        if self.read_closed {
            return EPIPE;
        }

        let available = PIPE_BUF_SIZE - self.data_len;
        if available == 0 {
            return -1; // caller should block (special signal)
        }

        let to_write = core::cmp::min(len, available);
        for i in 0..to_write {
            let byte = crate::syscall::user_read_u8(buf + i);
            self.buffer[self.write_pos] = byte;
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
        }
        self.data_len += to_write;

        // If a reader was blocked, wake it up
        if to_write > 0 {
            let reader = self.reader_blocked.swap(usize::MAX, Ordering::AcqRel);
            if reader != usize::MAX {
                crate::sched::wake_task(reader);
            }
        }

        to_write as isize
    }

    /// Check if pipe has data available for reading.
    pub fn has_data(&self) -> bool {
        self.data_len > 0
    }

    /// Check if pipe has space available for writing.
    pub fn has_space(&self) -> bool {
        self.data_len < PIPE_BUF_SIZE
    }

    /// Close the read end.
    pub fn close_read(&mut self) {
        self.read_closed = true;
        // Wake any blocked writer — it will get EPIPE on next write
        let writer = self.writer_blocked.swap(usize::MAX, Ordering::AcqRel);
        if writer != usize::MAX {
            crate::sched::wake_task(writer);
        }
    }

    /// Close the write end.
    pub fn close_write(&mut self) {
        self.write_closed = true;
        // Wake any blocked reader — it will get EOF on next read
        let reader = self.reader_blocked.swap(usize::MAX, Ordering::AcqRel);
        if reader != usize::MAX {
            crate::sched::wake_task(reader);
        }
    }

    /// Set the process index of a blocked reader.
    pub fn set_reader_blocked(&self, proc_idx: usize) {
        self.reader_blocked.store(proc_idx, Ordering::Release);
    }

    /// Set the process index of a blocked writer.
    pub fn set_writer_blocked(&self, proc_idx: usize) {
        self.writer_blocked.store(proc_idx, Ordering::Release);
    }

    /// Check if pipe is fully closed (both ends).
    pub fn is_fully_closed(&self) -> bool {
        self.read_closed && self.write_closed
    }
}

/// Global pipe table.
static PIPE_TABLE: SpinLock<[Option<Pipe>; MAX_PIPES]> = SpinLock::new([const { None }; MAX_PIPES]);

/// Reference counts per pipe: number of open fds pointing to this pipe.
/// When ref count drops to 0, the pipe can be freed.
static PIPE_REFCOUNTS: SpinLock<[usize; MAX_PIPES]> = SpinLock::new([0; MAX_PIPES]);

pub fn init() {
    // Pipes are lazily allocated, no init needed
}

/// Allocate a new pipe. Returns (pipe_id, Option<(read_fd_idx, write_fd_idx)>).
/// On success, pipe starts with refcount=2 (one for each end).
pub fn alloc_pipe() -> Option<usize> {
    let mut table = PIPE_TABLE.lock();
    for (i, slot) in table.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(Pipe::new());
            let mut refs = PIPE_REFCOUNTS.lock();
            refs[i] = 2; // read end + write end
            return Some(i);
        }
    }
    None
}

/// Get a mutable reference to a pipe by id. The caller must hold no conflicting locks.
pub fn with_pipe<F, R>(pipe_id: usize, f: F) -> Option<R>
where
    F: FnOnce(&mut Pipe) -> R,
{
    let mut table = PIPE_TABLE.lock();
    table[pipe_id].as_mut().map(f)
}

/// Check how many bytes are available in a pipe without consuming them.
/// Returns 0 if pipe doesn't exist or is empty.
pub fn pipe_available(pipe_id: usize) -> usize {
    let table = PIPE_TABLE.lock();
    match &table[pipe_id] {
        Some(pipe) => pipe.data_len,
        None => 0,
    }
}

/// Increment reference count for a pipe.
pub fn inc_ref(pipe_id: usize) {
    let mut refs = PIPE_REFCOUNTS.lock();
    refs[pipe_id] += 1;
}

/// Decrement reference count for a pipe. Returns true if fully released.
pub fn dec_ref(pipe_id: usize) -> bool {
    let mut refs = PIPE_REFCOUNTS.lock();
    if refs[pipe_id] > 0 {
        refs[pipe_id] -= 1;
    }
    if refs[pipe_id] == 0 {
        // Free the pipe
        let mut table = PIPE_TABLE.lock();
        table[pipe_id] = None;
        true
    } else {
        false
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    // Pipe tests are integrated with syscall tests
}
