#![no_std]
#![no_main]
extern crate alloc;

use core::arch::global_asm;

global_asm!(include_str!("entry.S"));

pub mod arch;
pub mod driver;
pub mod lang_items;
pub mod mm;
pub mod process;
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

        crate::console_println!("[init] Initializing PLIC...");
        arch::plic::init(0);

        crate::console_println!("[init] Initializing TTY...");
        driver::tty::init();

        crate::console_println!("[init] Starting secondary harts...");
        arch::smp::start_secondary_harts(4);

        crate::console_println!("[init] Initializing scheduler...");
        sched::init();

        // ── Load user program ──
        crate::console_println!("[init] Loading user program...");
        // Load /init from filesystem. Hold the FS lock during from_elf so the
        // borrowed &[u8] remains valid. from_elf copies what it needs, then
        // we can release the lock.
        let init_result = {
            let fs = driver::fs::global_fs();
            match fs.read("init") {
                Some(data) => {
                    crate::console_println!(
                        "[init] Loaded /init from filesystem ({} bytes)",
                        data.len()
                    );
                    process::Process::from_elf(data)
                }
                None => {
                    crate::console_println!("[init] WARNING: /init not found, using hello");
                    process::Process::from_elf(include_bytes!("../../user/hello.elf"))
                }
            }
        };
        match init_result {
            Ok(proc) => {
                crate::console_println!(
                    "[init] User process loaded: pid={}, entry={:#x}",
                    proc.pid,
                    proc.entry
                );
                crate::console_println!(
                    "[init]   kstack={:#x}, ustack={:#x}",
                    proc.kernel_stack_top,
                    proc.user_stack_top
                );

                // Register process in the global process table
                unsafe { riscv::register::sstatus::clear_sie() };
                let idx = process::add_process(proc).expect("Failed to register process");
                process::set_current_index(idx);

                // Re-read the process from the table for building trap context
                let proc = process::current().unwrap();
                process::set_current_page_table_root(proc.page_table_root);
                unsafe { riscv::register::sstatus::set_sie() };

                // Calculate user satp (Sv39 mode = 8)
                let user_satp = if proc.page_table_root == 0 {
                    let satp: usize;
                    unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
                    satp
                } else {
                    (8usize << 60) | proc.page_table_root
                };

                // NOTE: Init is NOT registered in the scheduler.
                // Timer returns directly to init when only init exists.
                // schedule() detects child tasks and switches to them.

                // Build TrapContext on kernel stack for first U-mode entry
                let trap_ctx_base = proc.kernel_stack_top - 280;
                unsafe {
                    let ctx = trap_ctx_base as *mut usize;
                    for i in 0..35 {
                        *ctx.add(i) = 0;
                    }
                    *ctx.add(2) = proc.kernel_stack_top;
                    *ctx.add(32) = 0x20;
                    *ctx.add(33) = proc.entry;
                    *ctx.add(34) = proc.user_stack_top;
                }

                crate::console_println!("[init] Entering user mode...");
                crate::console_println!(
                    "[init]   user_satp={:#x}, page_table_ppn={:#x}",
                    user_satp,
                    proc.page_table_root
                );

                unsafe { riscv::register::sstatus::clear_sie() };
                arch::trap::first_enter_user(
                    unsafe { &mut *(trap_ctx_base as *mut arch::trap::TrapContext) },
                    user_satp,
                );
            }
            Err(e) => {
                crate::console_println!("[init] Failed to load user program: {}", e);
            }
        }

        unsafe { riscv::register::sstatus::set_sie() };
        crate::console_println!("=== KarteOS initialized successfully ===");

        loop {
            unsafe { core::arch::asm!("wfi") };
        }
    }
}
