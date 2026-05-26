#![no_std]
#![no_main]
extern crate alloc;

use core::arch::global_asm;

global_asm!(include_str!("entry.S"));

pub mod arch;
pub mod driver;
pub mod lang_items;
pub mod mm;
pub mod sbi;
pub mod sched;
pub mod sync;
pub mod syscall;
pub mod test;

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain(hartid: usize, dtb_ptr: usize) -> ! {
    // ── Common init (both normal & test mode) ──
    driver::uart::Uart::new(0x1000_0000).init();
    crate::console_println!("=== KarteOS v0.2.0 ===");
    crate::console_println!("  Booting on hart {}", hartid);

    arch::trap::init();
    mm::pmm::init();
    mm::vmm::init();
    mm::heap::init();

    // ── Test mode ──
    #[cfg(feature = "test_mode")]
    {
        crate::console_println!("[test] Running test suite...");
        crate::console_println!("");

        crate::mm::pmm::run_tests();
        crate::mm::vmm::run_tests();
        crate::mm::heap::run_tests();
        crate::driver::fs::run_tests();
        crate::sync::spinlock::run_tests();
        crate::sched::task::run_tests();
        crate::syscall::run_tests();

        crate::test::print_summary();
        crate::sbi::shutdown()
    }

    // ── Normal mode ──
    #[cfg(not(feature = "test_mode"))]
    {
        crate::console_println!("  DTB pointer: {:#x}", dtb_ptr);

        crate::console_println!("[init] Initializing SMP...");
        arch::smp::init_bsp(hartid);

        crate::console_println!("[init] Probing VirtIO devices...");
        driver::virtio::probe_virtio_devices();

        crate::console_println!("[init] Initializing filesystem...");
        driver::fs::init();

        crate::console_println!("[init] Probing network devices...");
        driver::net::test_net();

        crate::console_println!("[init] Enabling timer interrupts...");
        arch::trap::enable_timer_interrupt();
        crate::console_println!("[init] Setting first timer...");
        arch::trap::set_next_timer();

        crate::console_println!("[init] Initializing PLIC...");
        arch::plic::init(0);

        crate::console_println!("[init] Starting secondary harts...");
        arch::smp::start_secondary_harts(4);

        crate::console_println!("[init] Initializing scheduler...");
        sched::init();

        unsafe { riscv::register::sstatus::set_sie() };
        crate::console_println!("=== KarteOS initialized successfully ===");

        loop {
            unsafe { core::arch::asm!("wfi") };
        }
    }
}
