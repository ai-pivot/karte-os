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

        // Parse multiboot2 info to get actual RAM size.
        // kmain params: EDI=multiboot2_magic, ESI=multiboot2_info_addr
        let (_mb2_magic, mb2_info) = (hartid, dtb_ptr);
        let (_mem_lower, mem_upper_kb) = crate::arch::multiboot2::parse_memory_size(mb2_info);
        if mem_upper_kb > 0 {
            // mem_upper_kb = KB of RAM above 1MB.
            // Total RAM = 1MB (below) + mem_upper_kb KB (above).
            // Leave 2MB for kernel/code start area.
            let total_ram = 1024 * 1024 + (mem_upper_kb as usize) * 1024;
            crate::console_println!(
                "[init] Multiboot2 memory: {} MB total",
                total_ram / 1024 / 1024
            );
            mm::pmm::init_with_size(total_ram - 0x0020_0000);
        } else {
            mm::pmm::init();
        }
    }

    crate::console_println!("=== KarteOS v0.2.0 ===");
    crate::console_println!("  Booting on hart {}", hartid);

    // Initialize kernel logger before any subsystem that uses `log::`
    crate::kernel_log::init();

    #[cfg(target_arch = "x86_64")]
    arch::cet::disable();

    arch::trap::init();
    #[cfg(target_arch = "riscv64")]
    mm::pmm::init();
    // x86_64 pmm init is done earlier (via multiboot2 or fallback)
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

        crate::console_println!("[init] Initializing virtual filesystem...");
        crate::driver::ramfs::virtual_init();

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

        // Load init program.
        // On x86_64: try xbot-cli-static from ext4 (like Linux init=).
        // Falls back to shell if not found.
        let init_result = {
            #[cfg(target_arch = "x86_64")]
            {
                // Shell is always the init process. xbot-cli-static and other
                // programs are loaded from ext4 at runtime via shell's exec command.
                if crate::driver::ext4::has_ext4() {
                    crate::console_println!(
                        "[init] ext4 ready, xbot-cli-static available for exec"
                    );
                }
                process::Process::from_elf(
                    include_bytes!("../../user/target/x86_64/shell.elf"),
                    alloc::vec![b"shell".to_vec()],
                    alloc::vec![],
                )
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                process::Process::from_elf(
                    include_bytes!("../../user/shell.elf"),
                    alloc::vec![b"shell".to_vec()],
                    alloc::vec![],
                )
            }
        };

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
                    if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
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
                let user_page_table = if proc.page_table_root == 0 {
                    let satp: usize;
                    unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
                    satp
                } else {
                    (8usize << 60) | proc.page_table_root
                };

                #[cfg(target_arch = "x86_64")]
                let user_page_table = if proc.page_table_root == 0 {
                    0usize
                } else {
                    proc.page_table_root << 12
                };

                crate::console_println!("[init] Entering user mode...");

                crate::sched::add_user_process(
                    proc.entry,
                    proc.user_stack_top,
                    proc.kernel_stack_top,
                    user_page_table,
                    idx,
                )
                .expect("Failed to register init task");

                crate::sched::start_first_task();
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
