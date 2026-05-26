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

#[unsafe(no_mangle)]
extern "C" fn kmain(hartid: usize, dtb_ptr: usize) -> ! {
    // Phase 1: Early init - UART + SBI console
    driver::uart::Uart::new(0x1000_0000).init();
    crate::console_println!("=== KarteOS v0.2.0 ===");
    crate::console_println!("  Booting on hart {}", hartid);
    crate::console_println!("  DTB pointer: {:#x}", dtb_ptr);

    // Phase 2: Trap initialization
    crate::console_println!("[init] Setting up trap handling...");
    arch::trap::init();

    // Phase 2.5: SMP — mark BSP as running
    crate::console_println!("[init] Initializing SMP...");
    arch::smp::init_bsp(hartid);

    // Phase 3: Physical memory
    crate::console_println!("[init] Initializing physical memory...");
    mm::pmm::init();

    // Phase 4: Virtual memory + heap
    crate::console_println!("[init] Setting up virtual memory...");
    mm::vmm::init();
    crate::console_println!("[init] Initializing kernel heap...");
    mm::heap::init();

    // Phase 5: VirtIO devices
    crate::console_println!("[init] Probing VirtIO devices...");
    driver::virtio::probe_virtio_devices();

    // Phase 6: Filesystem
    crate::console_println!("[init] Initializing filesystem...");
    driver::fs::init();

    // Phase 7: Network
    crate::console_println!("[init] Probing network devices...");
    driver::net::test_net();

    // Phase 8: Enable timer interrupt
    crate::console_println!("[init] Enabling timer interrupts...");
    arch::trap::enable_timer_interrupt();
    crate::console_println!("[init] Setting first timer...");
    arch::trap::set_next_timer();

    // Phase 9: PLIC
    crate::console_println!("[init] Initializing PLIC...");
    arch::plic::init(0);

    // Phase 9.5: Start secondary harts
    crate::console_println!("[init] Starting secondary harts...");
    arch::smp::start_secondary_harts(4);

    // Phase 10: Scheduler with context switching
    crate::console_println!("[init] Initializing scheduler...");
    sched::init();

    // Enable global S-mode interrupts
    unsafe { riscv::register::sstatus::set_sie() };
    crate::console_println!("=== KarteOS initialized successfully ===");

    // Main task loop — scheduler runs via timer interrupts
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}
