#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]
extern crate alloc;
// ext4_rs depends on the `log` crate; the macro is available via this import.
#[macro_use]
extern crate log;

use core::arch::global_asm;

#[cfg(target_arch = "riscv64")]
global_asm!(include_str!("arch/riscv64/entry.S"));

pub mod arch;
pub mod driver;
pub mod env;
pub mod kernel_log;
pub mod lang_items;
pub mod mm;
#[cfg(target_arch = "riscv64")]
pub mod net;
pub mod platform;
pub mod process;
pub mod sched;
pub mod sync;
pub mod syscall;
pub mod test;

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain(hartid: usize, dtb_ptr: usize) -> ! {
    // ── Common init (both normal & test mode) ──
    #[cfg(target_arch = "riscv64")]
    driver::uart::Uart::new(0x1000_0000).init();

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::uart::init_uart();
        crate::driver::vga::init();
    }

    crate::console_println!("=== KarteOS v0.2.0 ===");
    crate::console_println!("  Booting on hart {}", hartid);

    // Initialize kernel logger before any subsystem that uses `log::`
    crate::kernel_log::init();

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
        crate::sync::int_spinlock::run_tests();
        crate::sync::mutex::run_tests();
        crate::sched::task::run_tests();
        crate::syscall::run_tests();

        crate::test::print_summary();

        #[cfg(target_arch = "riscv64")]
        crate::arch::sbi::shutdown();

        #[cfg(target_arch = "x86_64")]
        crate::arch::platform::shutdown();
    }

    // ── Normal mode ──
    #[cfg(not(feature = "test_mode"))]
    {
        #[cfg(target_arch = "riscv64")]
        crate::console_println!("  DTB pointer: {:#x}", dtb_ptr);

        crate::console_println!("[init] Initializing SMP...");
        arch::smp::init_bsp(hartid);

        #[cfg(target_arch = "riscv64")]
        {
            crate::console_println!("[init] Probing VirtIO devices...");
            driver::virtio::probe_virtio_devices();
        }

        #[cfg(target_arch = "x86_64")]
        {
            crate::console_println!("[init] Probing PCI devices...");
            arch::pci::init();

            // Try AHCI (SATA) first, then VirtIO block device
            if let Some(ahci_dev) = arch::pci::find_ahci() {
                crate::console_println!(
                    "[pci] Found AHCI controller at {:02x}:{:02x}.{} bars=[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]",
                    ahci_dev.bus,
                    ahci_dev.device,
                    ahci_dev.function,
                    ahci_dev.bars[0],
                    ahci_dev.bars[1],
                    ahci_dev.bars[2],
                    ahci_dev.bars[3],
                    ahci_dev.bars[4],
                    ahci_dev.bars[5]
                );

                ahci_dev.enable();

                // AHCI uses BAR5 (Memory-Mapped I/O)
                let abar = ahci_dev.bar_address(5) as usize;
                let abar_size = ahci_dev.bar_size(5) as usize;

                crate::console_println!("[pci] ABAR (BAR5): {:#x}, size={:#x}", abar, abar_size);

                if abar != 0 {
                    match crate::driver::ahci::init(abar, abar_size) {
                        Ok(()) => {
                            crate::console_println!("[pci] AHCI controller initialized");
                        }
                        Err(e) => {
                            crate::console_println!("[pci] AHCI init failed: {}", e);
                        }
                    }
                }
            }

            if !crate::driver::ahci::is_available() {
                // Fall back to VirtIO block device if no AHCI
                if let Some(virtio_blk) = arch::pci::find_virtio_blk() {
                    crate::console_println!(
                        "[pci] Found VirtIO block device at {:02x}:{:02x}.{} bars=[{:#x},{:#x},{:#x}]",
                        virtio_blk.bus,
                        virtio_blk.device,
                        virtio_blk.function,
                        virtio_blk.bars[0],
                        virtio_blk.bars[1],
                        virtio_blk.bars[2]
                    );

                    virtio_blk.enable();

                    let bar0 = virtio_blk.bars[0];
                    let is_io = (bar0 & 1) == 1;
                    let bar_addr = virtio_blk.bar_address(0) as usize;
                    let bar_size = virtio_blk.bar_size(0) as usize;

                    crate::console_println!(
                        "[pci] BAR0: {:#x} ({}), size={:#x}",
                        bar_addr,
                        if is_io { "I/O" } else { "MMIO" },
                        bar_size
                    );

                    if is_io && bar_addr != 0 {
                        let io_base = (bar_addr & 0xFFFC) as u16;
                        match arch::virtio_blk::init(io_base) {
                            Ok(()) => {
                                crate::console_println!("[pci] VirtIO block device initialized");
                            }
                            Err(e) => {
                                crate::console_println!("[pci] VirtIO init failed: {}", e);
                            }
                        }
                    }
                }
            }

            // Initialize PS/2 keyboard
            crate::console_println!("[init] Initializing PS/2 keyboard...");
            crate::driver::keyboard::init();

            // Try NVMe first (fastest), then AHCI (SATA), then VirtIO block
            if let Some(nvme_dev) = arch::pci::find_nvme() {
                crate::console_println!(
                    "[pci] Found NVMe controller at {:02x}:{:02x}.{} bars=[{:#x},{:#x}]",
                    nvme_dev.bus,
                    nvme_dev.device,
                    nvme_dev.function,
                    nvme_dev.bars[0],
                    nvme_dev.bars[1]
                );

                nvme_dev.enable();

                // NVMe uses BAR0 (Memory-Mapped I/O)
                let bar0 = nvme_dev.bar_address(0) as usize;
                let bar0_size = nvme_dev.bar_size(0) as usize;

                crate::console_println!("[pci] NVMe BAR0: {:#x}, size={:#x}", bar0, bar0_size);

                if bar0 != 0 {
                    match crate::driver::nvme::init(bar0, bar0_size) {
                        Ok(()) => {
                            crate::console_println!("[pci] NVMe controller initialized");
                        }
                        Err(e) => {
                            crate::console_println!("[pci] NVMe init failed: {}", e);
                        }
                    }
                }
            }
        }

        crate::console_println!("[init] Initializing filesystem...");
        driver::fs::init();

        crate::console_println!("[init] Initializing environment...");
        env::init();

        // Enable Linux RISC-V syscall compatibility layer.
        // Zero overhead for native KarteOS syscalls — the translation table
        // is only consulted when a Linux syscall number is encountered.
        crate::syscall::linux::enable();

        #[cfg(target_arch = "riscv64")]
        {
            crate::console_println!("[init] Initializing PLIC...");
            arch::plic::init(0);
        }

        crate::console_println!("[init] Initializing TTY...");
        driver::tty::init();

        #[cfg(target_arch = "riscv64")]
        {
            crate::console_println!("[init] Starting secondary harts...");
            arch::smp::start_secondary_harts(4);
        }

        #[cfg(target_arch = "x86_64")]
        {
            crate::console_println!("[init] Starting secondary harts...");
            arch::smp::start_secondary_harts(1);
        }

        crate::console_println!("[init] Initializing scheduler...");
        sched::init();

        // ── Load user program ──
        crate::console_println!("[init] Loading user program...");

        // Load init program: shell from embedded bytes.
        let init_result = { process::Process::from_elf(include_bytes!("../../user/shell.elf")) };

        // init (shell) is always loaded from embedded bytes.
        // External programs loaded via `run` will use ext4/FAT32/RamFS.
        match init_result {
            Ok(proc) => {
                crate::console_println!(
                    "[init] User process loaded: pid={}, entry={:#x}",
                    proc.pid,
                    proc.entry
                );
                #[cfg(target_arch = "x86_64")]
                {
                    let user_pt_phys = proc.page_table_root << 12;
                    let user_pt = unsafe { &mut *(user_pt_phys as *mut crate::mm::vmm::PageTable) };
                    let page_addr = proc.entry & !0xFFF;
                    if let Some(paddr) = crate::mm::vmm::translate_user(user_pt, page_addr) {
                        crate::console_println!(
                            "[init] Entry page {:#x} → phys {:#x}, first byte={:#x}",
                            page_addr,
                            paddr,
                            unsafe { *((paddr + (proc.entry & 0xFFF)) as *const u8) }
                        );
                    } else {
                        crate::console_println!(
                            "[init] WARNING: entry {:#x} NOT mapped!",
                            page_addr
                        );
                    }
                }

                crate::console_println!("[init]   user_cr3={:#x}", proc.page_table_root << 12);

                // Register process in the global process table
                #[cfg(target_arch = "riscv64")]
                unsafe {
                    riscv::register::sstatus::clear_sie()
                };

                #[cfg(target_arch = "x86_64")]
                x86_64::instructions::interrupts::disable();

                let idx = process::add_process(proc).expect("Failed to register process");
                process::set_current_index(idx);

                // Re-read the process from the table for building trap context
                let proc = process::current().unwrap();
                process::set_current_page_table_root(proc.page_table_root);

                // Initialize network AFTER user program is loaded
                #[cfg(target_arch = "riscv64")]
                {
                    crate::console_println!("[init] Initializing network...");
                    if let Some(mac) = driver::net::init_net_device() {
                        net::iface::NetStack::init(mac);
                    }
                }

                #[cfg(target_arch = "riscv64")]
                unsafe {
                    riscv::register::sstatus::set_sie()
                };

                #[cfg(target_arch = "x86_64")]
                x86_64::instructions::interrupts::enable();

                // Architecture-specific first user entry
                #[cfg(target_arch = "riscv64")]
                {
                    // Calculate user satp (Sv39 mode = 8)
                    let user_satp = if proc.page_table_root == 0 {
                        let satp: usize;
                        unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
                        satp
                    } else {
                        (8usize << 60) | proc.page_table_root
                    };

                    // Build TrapContext on kernel stack for first U-mode entry.
                    let ctx_words = core::mem::size_of::<arch::trap::TrapContext>() / 8;
                    let trap_ctx_base =
                        proc.kernel_stack_top - core::mem::size_of::<arch::trap::TrapContext>();
                    unsafe {
                        let ctx = trap_ctx_base as *mut usize;
                        for i in 0..ctx_words {
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

                #[cfg(target_arch = "x86_64")]
                {
                    // CR3 = physical address of PML4 table (ppn << 12)
                    let user_cr3 = if proc.page_table_root == 0 {
                        0u64
                    } else {
                        (proc.page_table_root << 12) as u64
                    };

                    crate::console_println!("[init] Entering user mode...");
                    crate::console_println!(
                        "[init]   user_cr3={:#x}, page_table_ppn={:#x}",
                        user_cr3,
                        proc.page_table_root
                    );

                    // Disable interrupts before first_enter_user to prevent timer ISR
                    // from interfering with the context switch sequence.
                    x86_64::instructions::interrupts::disable();

                    arch::trap::first_enter_user(
                        proc.entry,
                        proc.user_stack_top,
                        proc.kernel_stack_top,
                        user_cr3,
                    );
                }
            }
            Err(e) => {
                crate::console_println!("[init] Failed to load user program: {}", e);
            }
        }

        #[cfg(target_arch = "riscv64")]
        unsafe {
            riscv::register::sstatus::set_sie()
        };

        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::enable();

        crate::console_println!("=== KarteOS initialized successfully ===");

        loop {
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("wfi")
            };

            #[cfg(target_arch = "x86_64")]
            x86_64::instructions::hlt();
        }
    }
}
