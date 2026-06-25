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
pub mod net;
pub mod platform;
pub mod process;
pub mod sched;
pub mod sync;
pub mod syscall;
pub mod test;

#[cfg(target_arch = "x86_64")]
static EARLY_FB_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain(hartid: usize, dtb_ptr: usize) -> ! {
    // ── Common init (both normal & test mode) ──
    #[cfg(target_arch = "riscv64")]
    driver::uart::Uart::new(0x1000_0000).init();

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::uart::init_uart();
        crate::driver::vga::init();

        // ── Read EFI BootInfo at 0x10000 (one read, early) ──
        // Framebuffer MUST be initialised before any console_println!
        // call, otherwise all early boot messages are silently dropped
        // (fb_console::putchar checks FB_READY which starts false).
        let efi_stub_booted = unsafe {
            core::ptr::read_volatile(0x10000usize as *const u32) == 0x474F5046
        };

        let mut efi_mem_upper_kb: u32 = 524288; // fallback 512 MB

        if efi_stub_booted {
            let bi = 0x10000usize as *const u32;
            let fb_addr =
                unsafe { core::ptr::read_volatile(bi.add(2) as *const u64) } as usize;
            let fb_width  = unsafe { core::ptr::read_volatile(bi.add(4) as *const u32) };
            let fb_height = unsafe { core::ptr::read_volatile(bi.add(5) as *const u32) };
            let fb_stride = unsafe { core::ptr::read_volatile(bi.add(6) as *const u32) };
            let mem_kb    = unsafe { core::ptr::read_volatile(bi.add(7) as *const u32) };

            if fb_addr != 0 {
                EARLY_FB_ADDR.store(fb_addr as u64, core::sync::atomic::Ordering::Relaxed);
                crate::arch::fb_console::init(fb_addr, fb_stride, fb_width, fb_height, 32);
                crate::console_println!(
                    "[gop] EFI stub FB at {:#x} {}x{} stride={}",
                    fb_addr, fb_width, fb_height, fb_stride
                );

                // Diagnostic cyan square — proves kernel writes to framebuffer
                let fb = fb_addr as *mut u32;
                let pp = fb_stride as usize / 4;
                for y in 0..50 {
                    for x in 0..50 {
                        unsafe { *fb.add(y * pp + (240 + x)) = 0xFFFFFF00u32; }
                    }
                }
            }

            if mem_kb > 0 && mem_kb < 0xFFFF_FFFF {
                efi_mem_upper_kb = mem_kb;
            }
        }

        // ── Memory ──
        let (_mb2_magic, mb2_info) = (hartid, dtb_ptr);
        let limine_booted = !crate::arch::limine::FRAMEBUFFER_REQUEST.response.is_null();

        let (mem_lower, mem_upper_kb, fb_info, efi_st_ptr) = if efi_stub_booted {
            (0u32, efi_mem_upper_kb, None, None)
        } else if limine_booted {
            // Limine native boot — get memory from Limine memmap request
            let resp = unsafe { &*crate::arch::limine::MEMMAP_REQUEST.response };
            let mut total_mem: u64 = 0;
            if resp.entry_count > 0 && !resp.entries.is_null() {
                for i in 0..resp.entry_count {
                    let entry = unsafe { &**resp.entries.add(i as usize) };
                    if entry.entry_type == 0 { // USABLE
                        total_mem += entry.length;
                    }
                }
            }
            let upper_kb = if total_mem > 0x100000 { ((total_mem - 0x100000) / 1024) as u32 } else { 0 };
            (0u32, upper_kb, None, None)
        } else {
            // Multiboot2 fallback
            let (ml, mu, fb, efi) = crate::arch::multiboot2::parse_mbi(mb2_info);
            (ml, mu, fb, efi)
        };
        let _ = mem_lower;
        if mem_upper_kb > 0 {
            let total_ram = 1024 * 1024 + (mem_upper_kb as usize) * 1024;
            crate::console_println!(
                "\x1b[92m[  OK  ]\x1b[0m Memory: {} MB total",
                total_ram / 1024 / 1024
            );
            mm::pmm::init_with_size(total_ram - 0x0020_0000);
        } else {
            mm::pmm::init();
        }

        // Framebuffer console — for non-EFI paths (EFI was done above)
        if !efi_stub_booted {
            let _fb_initialized = if crate::arch::limine::init_framebuffer() {
                crate::console_println!("[gop] Using Limine framebuffer");
                true
            } else if let Some(fb) = fb_info.or_else(|| {
                efi_st_ptr.and_then(|st| crate::arch::multiboot2::gop_from_efi(st))
            }) {
                crate::arch::fb_console::init(fb.addr, fb.pitch, fb.width, fb.height, fb.bpp);
                true
            } else {
                crate::console_println!("[gop] No framebuffer found");
                false
            };
        }
    }

    #[cfg(target_arch = "x86_64")]
    crate::arch::fb_console::boot_splash();
    crate::console_println!("\x1b[96m[ KarteOS ]\x1b[0m \x1b[1m v0.6 \x1b[0m — \x1b[90mhart {}\x1b[0m", hartid);

    // Initialize kernel logger before any subsystem that uses `log::`
    crate::kernel_log::init();

    #[cfg(target_arch = "x86_64")]
    arch::cet::disable();

    arch::trap::init();
    #[cfg(target_arch = "riscv64")]
    mm::pmm::init();
    // x86_64 pmm init is done earlier (via multiboot2 or fallback)
    mm::vmm::init();

    // Ensure GOP framebuffer is identity-mapped in the new page tables.
    // vmm::init() attempts this by re-reading BootInfo @ 0x10000 after
    // CR3 switch, but on some hardware the read may fail.  Use the
    // fb_addr saved during the early BootInfo read (while the EFI
    // loader's stable page tables were still active).
    #[cfg(target_arch = "x86_64")]
    {
        let fb = EARLY_FB_ADDR.load(core::sync::atomic::Ordering::Relaxed) as usize;
        if fb != 0 {
            let root = crate::mm::vmm::get_kernel_page_table();
            crate::mm::vmm::identity_map_region(root, fb, 64 * 1024 * 1024); // 64 MB
        }
    }

    // Re-cache kernel CR3 now that VMM is initialized (the first cache
    // during idt::init() captured 0 because KERNEL_PAGE_TABLE was null).
    #[cfg(target_arch = "x86_64")]
    crate::arch::idt::cache_kernel_cr3();

    mm::heap::init();

    // Initialize RTC for real wall clock time
    #[cfg(target_arch = "x86_64")]
    crate::arch::rtc::init_rtc();

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
        crate::process::run_tests();
        crate::syscall::run_tests();

        // Architecture-specific tests
        #[cfg(target_arch = "x86_64")]
        crate::arch::test::run_tests();

        #[cfg(target_arch = "riscv64")]
        crate::arch::test::run_tests();

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

            // Initialize XHCI USB first — taking over the controller may
            // disable BIOS legacy PS/2 emulation.  We then re-check PS/2.
            crate::console_println!("[init] Initializing XHCI USB...");
            if let Err(e) = crate::driver::xhci::init() {
                crate::console_println!("[init] XHCI: {}", e);
            } else {
                crate::driver::xhci::enumerate_keyboard();
            }

            // Initialize PS/2 keyboard AFTER XHCI.  If XHCI took over
            // the USB controller, legacy PS/2 emulation may be gone.
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

        // Load init program (shell).
        let init_result = {
            #[cfg(target_arch = "x86_64")]
            {
                process::Process::from_elf(
                    include_bytes!("../../user/shell.elf"),
                    alloc::vec![b"shell".to_vec()],
                    alloc::vec![],
                )
                .unwrap()
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                process::Process::from_elf(
                    include_bytes!("../../user/shell.elf"),
                    alloc::vec![b"shell".to_vec()],
                    alloc::vec![],
                )
                .unwrap()
            }
        };

        // init (shell or xbot) is loaded.
        {
            let proc = init_result;
            {
                crate::console_println!(
                    "[init] User process loaded: pid={}, entry={:#x}, sp={:#x}",
                    proc.pid,
                    proc.entry,
                    proc.user_stack_top
                );
                #[cfg(target_arch = "x86_64")]
                {
                    let user_pt_phys = proc.page_table_root << 12;
                    let user_pt = unsafe {
                        &mut *(crate::mm::vmm::phys_to_virt(user_pt_phys)
                            as *mut crate::mm::vmm::PageTable)
                    };
                    let page_addr = proc.entry & !0xFFF;
                    if let Some(frame) = crate::mm::vmm::translate_user(user_pt, page_addr) {
                        // Read first 16 bytes of user code via the direct map
                        let code_ptr = crate::mm::vmm::phys_to_virt(frame) as *const u8;
                        let b0 = unsafe { code_ptr.read_volatile() };
                        let b1 = unsafe { code_ptr.add(1).read_volatile() };
                        let b2 = unsafe { code_ptr.add(2).read_volatile() };
                        let b3 = unsafe { code_ptr.add(3).read_volatile() };
                        let b4 = unsafe { code_ptr.add(4).read_volatile() };
                        let b5 = unsafe { code_ptr.add(5).read_volatile() };
                        let b6 = unsafe { code_ptr.add(6).read_volatile() };
                        let b7 = unsafe { code_ptr.add(7).read_volatile() };
                        crate::console_println!(
                            "[init] entry {:#x} mapped to frame {:#x}, code: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                            page_addr,
                            frame,
                            b0, b1, b2, b3, b4, b5, b6, b7
                        );
                        // Also dump the raw PTE for the entry page
                        if let Some(raw_pte) = crate::mm::vmm::debug_pte(user_pt, page_addr) {
                            crate::console_println!(
                                "[init] PTE for {:#x} = {:#x} (P={} W={} U={} NX={})",
                                page_addr,
                                raw_pte,
                                raw_pte & 1 != 0,
                                raw_pte & 2 != 0,
                                raw_pte & 4 != 0,
                                raw_pte & (1u64 << 63) != 0,
                            );
                        }
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
                #[cfg(target_arch = "x86_64")]
                {
                    crate::console_println!("[init] Initializing network...");
                    // Try real hardware NIC first (E1000), then fall back to VirtIO
                    let e1000_mac = crate::arch::e1000::init_net_device();
                    let mac = if e1000_mac.is_some() {
                        e1000_mac
                    } else {
                        crate::arch::virtio_net::init_net_device()
                    };
                    if let Some(mac) = mac {
                        net::iface::NetStack::init(mac);
                    }
                }

                #[cfg(target_arch = "riscv64")]
                let user_page_table = if proc.page_table_root == 0 {
                    let satp: usize;
                    unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
                    satp
                } else {
                    (9usize << 60) | proc.page_table_root
                };

                #[cfg(target_arch = "x86_64")]
                let user_page_table = proc.page_table_root << 12;

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
