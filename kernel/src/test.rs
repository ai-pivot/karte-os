// kernel/src/test.rs — Kernel test framework (runs in QEMU)

use core::sync::atomic::{AtomicUsize, Ordering};

static TOTAL: AtomicUsize = AtomicUsize::new(0);
static PASSED: AtomicUsize = AtomicUsize::new(0);
static FAILED: AtomicUsize = AtomicUsize::new(0);

/// Run a single named test case. Prints TAP-style ok/not ok.
pub fn run_test(name: &str, f: impl FnOnce() -> bool) {
    TOTAL.fetch_add(1, Ordering::Relaxed);
    if f() {
        PASSED.fetch_add(1, Ordering::Relaxed);
        crate::console_println!("  ok - {}", name);
    } else {
        FAILED.fetch_add(1, Ordering::Relaxed);
        crate::console_println!("  not ok - {}", name);
    }
}

/// Print final TAP summary and shutdown marker.
pub fn print_summary() {
    let total = TOTAL.load(Ordering::Relaxed);
    let passed = PASSED.load(Ordering::Relaxed);
    let failed = FAILED.load(Ordering::Relaxed);

    crate::console_println!("");
    crate::console_println!("──────────────────────────────────────────");
    crate::console_println!(
        "  Test Results: {}/{} passed, {} failed",
        passed,
        total,
        failed
    );
    crate::console_println!("──────────────────────────────────────────");

    if failed == 0 {
        crate::console_println!("TEST_RESULT: ALL_PASSED");
    } else {
        crate::console_println!("TEST_RESULT: {}_FAILED", failed);
    }
}
