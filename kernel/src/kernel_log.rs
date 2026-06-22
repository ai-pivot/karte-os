// kernel/src/kernel_log.rs — no_std log facade for kernel
//
// Provides:
// 1. A minimal `log::Log` implementation for crates like ext4_rs.
// 2. A klog! macro for kernel-internal structured logging with levels.
//
// Usage of klog!:
//   klog!(ERROR, "fatal: {}", msg);   // always shown
//   klog!(WARN,  "warning: {}", msg); // shown at WARN and below
//   klog!(INFO,  "status: {}", msg);  // shown at INFO and below
//   klog!(DEBUG, "detail: {}", msg);  // shown at DEBUG and below
//   klog!(TRACE, "spam: {}", msg);    // shown at TRACE only
//
// Default level: INFO (compile-time override via KLOG_LEVEL env var).
// Runtime override: klog_set_level(LEVEL_TRACE).

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

// ─── Kernel log ring buffer ────────────────────────────────────
//
// Lock-free SPSC ring buffer that captures ALL kernel log output.
// During boot phase, logs go to both UART and this buffer.
// After filesystem is ready, the buffer can be flushed to /var/log/kernel.
// User-space dmesg reads this buffer via sys_syslog syscall.

const KLOG_BUF_SIZE: usize = 32768; // 32KB

struct LogBuffer {
    data: [u8; KLOG_BUF_SIZE],
    /// Write position (producer: console_putchar)
    head: AtomicUsize,
    /// Read position (consumer: sys_syslog / flush)
    tail: AtomicUsize,
}

impl LogBuffer {
    const fn new() -> Self {
        Self {
            data: [0u8; KLOG_BUF_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Write a single byte into the ring buffer (lock-free).
    /// Called from console_println path — must never block or panic.
    #[inline]
    fn write_byte(&self, byte: u8) {
        let head = self.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % KLOG_BUF_SIZE;
        // If buffer is full, drop the oldest byte by advancing tail
        let tail = self.tail.load(Ordering::Acquire);
        if next_head == tail {
            // Buffer full — advance tail to make room (drop oldest)
            self.tail
                .store((tail + 1) % KLOG_BUF_SIZE, Ordering::Release);
        }
        // SAFETY: head is only written by the producer (single writer).
        // We use Relaxed ordering because only one thread writes.
        unsafe {
            *(&self.data as *const u8 as *mut u8).add(head) = byte;
        }
        self.head.store(next_head, Ordering::Release);
    }

    /// Read all available bytes since last read into `buf`.
    /// Returns the number of bytes read.
    /// This is the consumer side — called from sys_syslog.
    pub fn read(&self, buf: &mut [u8]) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let available = if head >= tail {
            head - tail
        } else {
            KLOG_BUF_SIZE - tail + head
        };
        let to_read = core::cmp::min(available, buf.len());
        let mut read = 0;
        let mut pos = tail;
        while read < to_read {
            buf[read] = unsafe { *(&self.data as *const u8).add(pos) };
            pos = (pos + 1) % KLOG_BUF_SIZE;
            read += 1;
        }
        // Advance tail after reading
        self.tail
            .store((tail + to_read) % KLOG_BUF_SIZE, Ordering::Release);
        to_read
    }

    /// Read bytes WITHOUT advancing the tail (peek).
    /// Used by sys_syslog when the user buffer is smaller than available data.
    pub fn peek(&self, buf: &mut [u8], offset: usize) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        let available = if head >= tail {
            head - tail
        } else {
            KLOG_BUF_SIZE - tail + head
        };
        if offset >= available {
            return 0;
        }
        let to_read = core::cmp::min(available - offset, buf.len());
        let mut read = 0;
        let mut pos = (tail + offset) % KLOG_BUF_SIZE;
        while read < to_read {
            buf[read] = unsafe { *(&self.data as *const u8).add(pos) };
            pos = (pos + 1) % KLOG_BUF_SIZE;
            read += 1;
        }
        to_read
    }

    /// Get the total number of bytes available to read.
    pub fn available(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if head >= tail {
            head - tail
        } else {
            KLOG_BUF_SIZE - tail + head
        }
    }

    /// Clear all buffered data.
    pub fn clear(&self) {
        let head = self.head.load(Ordering::Relaxed);
        self.tail.store(head, Ordering::Release);
    }
}

/// Global kernel log buffer.
static KLOG_BUF: LogBuffer = LogBuffer::new();

/// Write a byte to the kernel log buffer.
/// Called by console_putchar on every character output.
#[inline]
pub fn log_write_byte(byte: u8) {
    KLOG_BUF.write_byte(byte);
}

/// Read kernel log buffer into user-provided buffer (for sys_syslog).
/// Returns (bytes_read, total_available).
pub fn log_read(buf: &mut [u8]) -> (usize, usize) {
    let available = KLOG_BUF.available();
    let read = KLOG_BUF.read(buf);
    (read, available)
}

/// Peek at kernel log without consuming (for partial reads).
pub fn log_peek(buf: &mut [u8], offset: usize) -> usize {
    KLOG_BUF.peek(buf, offset)
}

/// Get total bytes available in kernel log buffer.
pub fn log_available() -> usize {
    KLOG_BUF.available()
}

/// Flush kernel log buffer to file on the filesystem.
/// Called periodically after boot completes.
pub fn log_flush_to_file() {
    let available = KLOG_BUF.available();
    if available == 0 {
        return;
    }

    // Read into a temporary buffer
    let mut tmp = alloc::vec::Vec::new();
    tmp.resize(available, 0);
    let (read, _) = log_read(&mut tmp);
    if read == 0 {
        return;
    }
    tmp.truncate(read);

    // Try to write to /var/log/kernel on ext4
    let _ = crate::driver::fs::write_file_owned("/var/log/kernel", &tmp);
}

// ─── Kernel log levels ────────────────────────────────────────
pub const LEVEL_ERROR: u32 = 0;
pub const LEVEL_WARN: u32 = 1;
pub const LEVEL_INFO: u32 = 2;
pub const LEVEL_DEBUG: u32 = 3;
pub const LEVEL_TRACE: u32 = 4;

/// Default log level. Override at compile time with:
///   KLOG_LEVEL=3 cargo build ...  (DEBUG)
///   KLOG_LEVEL=4 cargo build ...  (TRACE — very noisy)
#[cfg(not(feature = "klog_trace"))]
#[cfg(not(feature = "klog_debug"))]
const DEFAULT_LEVEL: u32 = 2; // INFO

#[cfg(all(feature = "klog_debug", not(feature = "klog_trace")))]
const DEFAULT_LEVEL: u32 = 3; // DEBUG

#[cfg(feature = "klog_trace")]
const DEFAULT_LEVEL: u32 = 4; // TRACE

static KLOG_LEVEL: AtomicU32 = AtomicU32::new(DEFAULT_LEVEL);

/// Get the current log level.
pub fn klog_level() -> u32 {
    KLOG_LEVEL.load(Ordering::Relaxed)
}

/// Set the log level at runtime.
pub fn klog_set_level(level: u32) {
    KLOG_LEVEL.store(level.min(4), Ordering::Relaxed);
}

/// Check if a message at the given level would be printed.
#[inline]
pub fn klog_enabled(level: u32) -> bool {
    level <= KLOG_LEVEL.load(Ordering::Relaxed)
}

// ─── klog! macro ───────────────────────────────────────────────

#[macro_export]
macro_rules! klog {
    (ERROR, $($arg:tt)*) => {{
        if $crate::kernel_log::klog_enabled($crate::kernel_log::LEVEL_ERROR) {
            $crate::console_println!($($arg)*);
        }
    }};
    (WARN, $($arg:tt)*) => {{
        if $crate::kernel_log::klog_enabled($crate::kernel_log::LEVEL_WARN) {
            $crate::console_println!($($arg)*);
        }
    }};
    (INFO, $($arg:tt)*) => {{
        if $crate::kernel_log::klog_enabled($crate::kernel_log::LEVEL_INFO) {
            $crate::console_println!($($arg)*);
        }
    }};
    (DEBUG, $($arg:tt)*) => {{
        if $crate::kernel_log::klog_enabled($crate::kernel_log::LEVEL_DEBUG) {
            $crate::console_println!($($arg)*);
        }
    }};
    (TRACE, $($arg:tt)*) => {{
        if $crate::kernel_log::klog_enabled($crate::kernel_log::LEVEL_TRACE) {
            $crate::console_println!($($arg)*);
        }
    }};
}

// ─── `log` crate facade (for ext4_rs etc.) ────────────────────

static LOG_LEVEL: AtomicUsize = AtomicUsize::new(log::Level::Warn as usize);

struct KernelLogger;

impl log::Log for KernelLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let level = match record.level() {
                log::Level::Error => "ERROR",
                log::Level::Warn => "WARN",
                log::Level::Info => "INFO",
                log::Level::Debug => "DEBUG",
                log::Level::Trace => "TRACE",
            };
            let msg = alloc::format!("[{}] {}\n", level, record.args());
            // Write to kernel log buffer (for dmesg)
            for &b in msg.as_bytes() {
                crate::kernel_log::log_write_byte(b);
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: KernelLogger = KernelLogger;

/// Initialize the kernel logger. Call once during boot before any crate
/// that uses `log::info!` etc.
pub fn init() {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Warn))
        .expect("Failed to set kernel logger");
}
