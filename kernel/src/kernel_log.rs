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
            crate::console_println!("[{}] {}", level, record.args());
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
