#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(include_str!("entry.S"));

pub mod arch;
pub mod driver;
pub mod lang_items;
pub mod mm;
pub mod sbi;
pub mod sched;
pub mod sync;

#[unsafe(no_mangle)]
extern "C" fn kmain(hartid: usize, dtb_ptr: usize) -> ! {
    // Phase 1: Early init - UART + SBI console
    driver::uart::Uart::new(0x1000_0000).init();
    crate::console_println!("=== KarteOS v0.1.0 ===");
    crate::console_println!("  Booting on hart {}", hartid);
    crate::console_println!("  DTB pointer: {:#x}", dtb_ptr);

    // Phase 2: Trap initialization
    crate::console_println!("[init] Setting up trap handling...");
    arch::trap::init();

    // Phase 3: Physical memory
    crate::console_println!("[init] Initializing physical memory...");
    mm::pmm::init();

    // Phase 4: Virtual memory + heap
    crate::console_println!("[init] Setting up virtual memory...");
    mm::vmm::init();
    crate::console_println!("[init] Initializing kernel heap...");
    mm::heap::init();

    // Phase 5: Enable timer interrupt
    crate::console_println!("[init] Enabling timer interrupts...");
    arch::trap::enable_timer_interrupt();
    crate::console_println!("[init] Setting first timer...");
    arch::trap::set_next_timer();

    // Phase 6: PLIC
    crate::console_println!("[init] Initializing PLIC...");
    arch::plic::init(0);

    // Phase 7: Scheduler
    crate::console_println!("[init] Initializing scheduler...");
    sched::init();
    // Enable global S-mode interrupts
    unsafe { riscv::register::sstatus::set_sie() };
    crate::console_println!("=== KarteOS initialized successfully ===");

    // Park main hart — scheduler runs via timer interrupts
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}
