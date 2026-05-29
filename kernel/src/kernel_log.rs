// kernel/src/log.rs — no_std log facade for kernel
//
// Provides a minimal `log::Log` implementation that forwards log messages
// to the kernel console. This is required by crates like ext4_rs that
// depend on the `log` crate.

use core::sync::atomic::AtomicUsize;

/// Kernel log level filter. Only messages at or below this level are emitted.
/// Default: Warn (suppresses trace/debug/info, shows warn/error).
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
