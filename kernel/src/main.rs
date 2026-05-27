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

        crate::console_println!("[init] Starting secondary harts...");
        arch::smp::start_secondary_harts(4);

        crate::console_println!("[init] Initializing scheduler...");
        sched::init();

        // ── Load user program ──
        crate::console_println!("[init] Loading user program...");
        let elf_data = include_bytes!("../../user/hello.elf");
        match process::Process::from_elf(elf_data) {
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
                // Disable interrupts to prevent timer from triggering while we hold PROCESS_TABLE lock
                unsafe { riscv::register::sstatus::clear_sie() };
                let idx = process::add_process(proc).expect("Failed to register process");
                process::set_current_index(idx);

                // Re-read the process from the table for building trap context
                let proc = process::current().unwrap();
                process::set_current_page_table_root(proc.page_table_root);
                unsafe { riscv::register::sstatus::set_sie() };

                // Calculate user satp value (Sv39 mode = 8) early — needed by
                // both add_user_process and first_enter_user.
                let user_satp = if proc.page_table_root == 0 {
                    let satp: usize;
                    unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
                    satp
                } else {
                    (8usize << 60) | (proc.page_table_root)
                };

                // Register first process with the scheduler so it can
                // participate in context switching (blocking waitpid,
                // multi-process, SMP scheduling).
                sched::add_user_process(
                    proc.entry,
                    proc.user_stack_top,
                    proc.kernel_stack_top,
                    user_satp,
                    idx,
                );

                // Build TrapContext on kernel stack for first U-mode entry
                // (must be AFTER add_user_process — overwrites its layout)
                let trap_ctx_base = proc.kernel_stack_top - 280;
                unsafe {
                    let ctx = trap_ctx_base as *mut usize;
                    // Zero everything
                    for i in 0..35 {
                        *ctx.add(i) = 0;
                    }
                    // x[2] = kernel sp
                    *ctx.add(2) = proc.kernel_stack_top;
                    // sstatus at offset 32 (256/8): SPP=0, SPIE=1
                    *ctx.add(32) = 0x20;
                    // sepc at offset 33 (264/8): user entry
                    *ctx.add(33) = proc.entry;
                    // sscratch at offset 34 (272/8): user sp
                    *ctx.add(34) = proc.user_stack_top;
                }

                crate::console_println!("[init] Entering user mode...");
                crate::console_println!(
                    "[init]   user_satp={:#x}, page_table_ppn={:#x}",
                    user_satp,
                    proc.page_table_root
                );

                // Disable interrupts during the critical sret sequence
                unsafe { riscv::register::sstatus::clear_sie() };

                // NOTE: Timer interrupts will be enabled on the first user ecall

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
