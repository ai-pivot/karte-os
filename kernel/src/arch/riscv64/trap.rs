//! Trap handling: U-mode ↔ S-mode transitions, exception dispatch, timer interrupts.

use core::arch::global_asm;
use core::sync::atomic::{AtomicUsize, Ordering};

use riscv::register::stvec;

/// Pointer to the current trap context (set by trap_handler before dispatch).
/// Used by clone() to copy the parent's register state to the child thread.
static CURRENT_TRAP_CTX: AtomicUsize = AtomicUsize::new(0);

/// Get the current trap context pointer (valid during syscall dispatch).
pub fn current_trap_ctx() -> usize {
    CURRENT_TRAP_CTX.load(Ordering::Relaxed)
}

// Embed the trap entry/exit assembly (dual-path: S-mode + U-mode)
global_asm!(include_str!("trap_entry.S"));

unsafe extern "C" {
    fn trap_entry();
    fn trap_return_user();
}

/// Trap context: complete register state saved on trap entry.
///
/// Layout (288 bytes on stack):
///   x[0..32]  @ offset 0..256   — 32 general-purpose registers (x0 unused, x2 = sp)
///   sstatus   @ offset 256
///   sepc      @ offset 264
///   sscratch  @ offset 272      — saved sscratch (user sp if from U-mode)
///   user_satp @ offset 280      — page table to switch to before sret (0 = no switch)
///
/// `user_satp` is non-zero ONLY for a task's very first entry into U-mode (via
/// `first_task_shim`, which bypasses `trap_handler`). For all normal trap
/// returns it is 0, and the satp is managed by `trap_handler`'s tail instead.
#[repr(C)]
#[derive(Debug)]
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    pub sscratch: usize,  // If from U-mode: contains user sp; else 0
    pub user_satp: usize, // Page table SATP to switch to before sret (0 = don't switch)
}

impl TrapContext {
    /// Create a trap context for first entry into a user process.
    pub fn new_for_user(entry: usize, user_sp: usize, kernel_sp: usize) -> Self {
        let mut ctx = Self {
            x: [0; 32],
            sstatus: 0,
            sepc: entry,
            sscratch: user_sp,
            user_satp: 0,
        };
        // Set sstatus: SPP=0 (return to U-mode), SPIE=1 (enable interrupts after sret)
        // FS=Initial (1<<13) to enable FPU for Go runtime
        ctx.sstatus = 0x20 | (1 << 13); // SPIE + FS=Initial
        ctx.x[2] = kernel_sp; // kernel sp (used during trap handling)
        ctx
    }
}

/// Get the address of the trap_return_user assembly label.
/// Used by the scheduler to set up the return address for new tasks
/// so that __switch's ret jumps into the U-mode return path.
pub fn trap_return_user_addr() -> usize {
    unsafe { trap_return_user as *const () as usize }
}

/// Set up the trap vector.
pub fn init() {
    unsafe {
        stvec::write(stvec::Stvec::new(
            trap_entry as *const () as usize,
            stvec::TrapMode::Direct,
        ));
    }
}

/// Jump to user mode for the first time.
/// `ctx` is a TrapContext prepared on the kernel stack.
/// `user_satp` is the SATP register value for the user page table.
/// This function never returns.
#[inline(never)]
pub fn first_enter_user(ctx: &mut TrapContext, user_satp: usize) -> ! {
    unsafe {
        let user_sp = ctx.sscratch;
        let kernel_sp = ctx.x[2];
        let sstatus_val = ctx.sstatus;
        let sepc_val = ctx.sepc;

        // Single asm block: switch satp + sfence.vma + CSR setup + sret.
        // All in one block to prevent compiler from inserting instructions
        // between csrw satp and sfence.vma (which would use stale TLB).
        core::arch::asm!(
            "csrw satp, {satp}",
            "sfence.vma",
            "csrw sscratch, {usp}",
            "csrw sstatus, {st}",
            "csrw sepc, {epc}",
            "csrw sscratch, {ksp}",
            "mv sp, {usp}",
            "sret",
            satp = in(reg) user_satp,
            usp = in(reg) user_sp,
            st = in(reg) sstatus_val,
            epc = in(reg) sepc_val,
            ksp = in(reg) kernel_sp,
            options(noreturn)
        );
    }
}

/// Enable supervisor timer interrupt.
pub fn enable_timer_interrupt() {
    unsafe {
        riscv::register::sie::set_stimer();
    }
}

/// Set the next timer interrupt (10ms from now).
pub fn set_next_timer() {
    const CLOCK_FREQ: usize = 10_000_000;
    const TICKS_PER_MS: usize = CLOCK_FREQ / 1000;
    let next = riscv::register::time::read() + 10 * TICKS_PER_MS;
    let _ = ::sbi::timer::set_timer(next as u64);
}

/// Get the current process's page table.
/// Returns the user page table if a process is active, otherwise falls back to the kernel page table.
pub fn get_current_user_pt() -> &'static mut crate::mm::vmm::PageTable {
    let ppn = crate::process::current_page_table_ppn();
    if ppn == 0 {
        // Phase 1: shared kernel page table (no separate per-process PT yet)
        crate::mm::vmm::get_kernel_page_table()
    } else {
        unsafe { &mut *((ppn << 12) as *mut crate::mm::vmm::PageTable) }
    }
}

/// Handle timer interrupt: poll UART, advance scheduler, tick network.
fn handle_timer() {
    // Poll UART RX — safe because on_char uses an atomic guard to prevent
    // reentrant access from nested timer interrupts.
    crate::driver::tty::poll_uart();

    // Tick uptime counter
    crate::arch::platform::tick_uptime();

    // Wake up tasks whose sleep/futex timeout has expired.
    crate::sched::tick_sleep_queue();

    // Poll network stack (non-blocking) — only after init is complete
    // to avoid interfering with user program loading.
    if crate::net::iface::NetStack::is_initialized() {
        crate::net::iface::NetStack::poll();
    }

    set_next_timer();
    crate::sched::schedule();
}

/// Trap handler — called from assembly trap_entry.
#[unsafe(no_mangle)]
extern "C" fn trap_handler(ctx: &mut TrapContext) -> &mut TrapContext {
    let scause = riscv::register::scause::read();
    let stval = riscv::register::stval::read();

    // Decode scause from raw bits
    let scause_code = scause.bits();
    let is_interrupt = scause_code & (1 << 63) != 0;
    let code = scause_code & !(1 << 63);

    // If from U-mode, enable SUM so S-mode can access user pages
    let from_user = ctx.sscratch != 0;
    if from_user {
        unsafe { riscv::register::sstatus::set_sum() };
    }

    if is_interrupt {
        match code {
            5 => handle_timer(),
            9 => {
                // Supervisor External (PLIC)
                crate::arch::plic::handle_interrupt(0);
            }
            _ => {
                // Unknown S-mode interrupt — skip instruction.
                // Do NOT print — SpinLock in UART can deadlock.
                skip_trap_instruction(ctx);
            }
        }
    } else {
        match code {
            8 => {
                // User Environment Call (ecall from U-mode)
                // a7 = syscall number (x[17])
                // a0-a5 = args (x[10]-x[15])
                let syscall_id = ctx.x[17];
                let args = [
                    ctx.x[10], ctx.x[11], ctx.x[12], ctx.x[13], ctx.x[14], ctx.x[15],
                ];
                // Store trap context pointer for clone() to access parent registers
                CURRENT_TRAP_CTX.store(ctx as *const _ as usize, Ordering::Relaxed);
                let result = crate::syscall::dispatch(syscall_id, args);
                ctx.x[10] = result as usize; // a0 = return value
                ctx.sepc += 4; // skip ecall instruction
            }
            2 => {
                // Illegal instruction — silently skip.
                // This handles CSR probing by the Rust runtime (sstateen0, senvcfg,
                // stimecmp, etc.) which are not supported by QEMU -cpu rv64.
                // We must NOT print here as console_println acquires a SpinLock
                // which can deadlock if a timer interrupt fires during the lock hold.
                skip_trap_instruction(ctx);
            }
            5 | 7 => {
                // Load/Store Access Fault
                // Silently skip — happens when probing unmapped VirtIO devices
                skip_trap_instruction(ctx);
            }
            12 | 13 | 15 => {
                let fault_addr = stval;
                let page_size = crate::mm::pmm::page_size();
                let page_addr = fault_addr & !(page_size - 1);

                // A/D bit fix via direct Sv39 page table walk (3 levels).
                let satp_val: usize;
                unsafe {
                    core::arch::asm!("csrr {}, satp", out(reg) satp_val);
                }
                let satp_ppn = satp_val & ((1usize << 44) - 1);
                let satp_pa = satp_ppn << 12;

                let vpn2 = (page_addr >> 30) & 0x1FF;
                let vpn1 = (page_addr >> 21) & 0x1FF;
                let vpn0 = (page_addr >> 12) & 0x1FF;

                let l2_entry =
                    unsafe { core::ptr::read_volatile((satp_pa + vpn2 * 8) as *const usize) };
                if l2_entry & 1 != 0 {
                    let l1_pa = (l2_entry >> 10) << 12;
                    let l1_entry =
                        unsafe { core::ptr::read_volatile((l1_pa + vpn1 * 8) as *const usize) };
                    if l1_entry & 1 != 0 {
                        let l0_pa = (l1_entry >> 10) << 12;
                        let l0_entry =
                            unsafe { core::ptr::read_volatile((l0_pa + vpn0 * 8) as *const usize) };
                        if l0_entry & 1 != 0 {
                            let mut new_pte = l0_entry;
                            new_pte |= 1 << 6; // A bit
                            if code == 15 {
                                new_pte |= 1 << 7;
                            } // D bit
                            unsafe {
                                core::ptr::write_volatile(
                                    (l0_pa + vpn0 * 8) as *mut usize,
                                    new_pte,
                                );
                                core::arch::asm!("sfence.vma {0}, zero", in(reg) page_addr);
                            }
                            return ctx;
                        }
                    }
                }

                // Lazy allocation for unmapped user pages.
                // Only for legitimate user-space addresses, excluding
                // kernel identity map and MMIO regions.
                let is_kernel_or_mmio = (0x0C00_0000..0x0C40_0000).contains(&fault_addr)  // PLIC
                    || (0x1000_0000..0x1000_A000).contains(&fault_addr) // UART + VirtIO MMIO
                    || (0x8020_0000..0xC020_0000).contains(&fault_addr); // Kernel identity map
                let is_user_addr = !is_kernel_or_mmio
                    && fault_addr < crate::process::USER_MMAP_LIMIT
                    && fault_addr >= 0x10000; // Above ELF entry point
                if is_user_addr {
                    let user_pt = get_current_user_pt();
                    let heap_base = crate::process::USER_HEAP_BASE;
                    let heap_limit = crate::process::USER_HEAP_LIMIT;
                    if fault_addr >= heap_base && fault_addr < heap_limit {
                        if let Some(frame) = crate::mm::pmm::alloc_frame() {
                            unsafe {
                                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                            }
                            crate::mm::vmm::map(
                                user_pt,
                                page_addr,
                                frame,
                                crate::mm::vmm::PTEFlags::URW
                                    | crate::mm::vmm::PTEFlags::A
                                    | crate::mm::vmm::PTEFlags::D,
                            );
                            unsafe {
                                core::arch::asm!("sfence.vma {0}, zero", in(reg) page_addr);
                            }
                            let new_brk = page_addr + page_size;
                            if new_brk > crate::process::current_brk() {
                                crate::process::set_current_brk(new_brk);
                            }
                            return ctx;
                        }
                    }
                    // VMA-based lazy allocation for mmap'd regions
                    let vma_root =
                        crate::process::get_page_table_root(crate::process::current_index());
                    if let Some(prot) = crate::mm::vma::vma_query(vma_root, page_addr) {
                        if prot != 0 {
                            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                                unsafe {
                                    core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                                }
                                let flags = if prot & 2 != 0 {
                                    crate::mm::vmm::PTEFlags::URW
                                        | crate::mm::vmm::PTEFlags::A
                                        | crate::mm::vmm::PTEFlags::D
                                } else {
                                    crate::mm::vmm::PTEFlags::UR | crate::mm::vmm::PTEFlags::A
                                };
                                crate::mm::vmm::map(user_pt, page_addr, frame, flags);
                                unsafe {
                                    core::arch::asm!("sfence.vma {0}, zero", in(reg) page_addr);
                                }
                                return ctx;
                            }
                        }
                    }
                    // Final fallback: allocate RW page for ANY user-space address.
                    match crate::mm::pmm::alloc_frame() {
                        Some(frame) => {
                            unsafe {
                                core::ptr::write_bytes(frame as *mut u8, 0, page_size);
                            }
                            crate::mm::vmm::map(
                                user_pt,
                                page_addr,
                                frame,
                                crate::mm::vmm::PTEFlags::URW
                                    | crate::mm::vmm::PTEFlags::A
                                    | crate::mm::vmm::PTEFlags::D,
                            );
                            unsafe {
                                core::arch::asm!("sfence.vma {0}, zero", in(reg) page_addr);
                            }
                            return ctx;
                        }
                        None => {
                            // OOM: can't allocate page. Skip instruction to avoid crash.
                            // This is better than killing the process.
                            skip_trap_instruction(ctx);
                            return ctx;
                        }
                    }
                }

                // Page fault we couldn't handle.
                if from_user {
                    crate::console_println!(
                        "[pf-fatal] sepc={:#x} stval={:#x} code={}",
                        ctx.sepc,
                        stval,
                        code
                    );
                    crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]);
                } else {
                    skip_trap_instruction(ctx);
                }
            }
            _ => {
                // Unknown exception — silently skip.
                skip_trap_instruction(ctx);
            }
        }
    }

    // Clear SUM if we set it for user-mode trap handling
    if from_user {
        unsafe { riscv::register::sstatus::clear_sum() };
    }

    // Store user page table SATP in TrapContext for trap_return_user to switch.
    // We do NOT switch satp here because the kernel page table (active since
    // trap_entry) has correct A/D bits on all kernel pages. trap_return_user
    // will switch satp to the user page table just before sret.
    if from_user {
        let target_ppn = crate::process::current_page_table_root();
        if target_ppn != 0 {
            ctx.user_satp = (8usize << 60) | target_ppn;
        }
    }

    ctx
}

/// Skip the faulting instruction by advancing sepc.
/// Handles both 16-bit (compressed) and 32-bit RISC-V instructions.
fn skip_trap_instruction(ctx: &mut TrapContext) {
    // Always advance by 4 bytes. Reading the instruction at sepc to
    // determine compressed vs standard length (read_volatile) can cause
    // a page fault if sepc is in unmapped memory (e.g., OpenSBI region
    // during CSR probing). All our code and the Rust runtime use standard
    // 32-bit instructions for CSR probes, so 4 is always correct.
    ctx.sepc += 4;
}

/// Print a debug message safely by switching to kernel page table first.
/// This ensures UART MMIO is accessible regardless of current satp state.
#[allow(dead_code)]
pub fn safe_print(msg: &str) {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        // Save current satp
        let saved_satp: usize;
        core::arch::asm!("csrr {}, satp", out(reg) saved_satp);

        // Switch to kernel page table
        let kernel_satp = crate::mm::vmm::KERNEL_SATP.load(core::sync::atomic::Ordering::Relaxed);
        if kernel_satp != 0 && kernel_satp != saved_satp {
            core::arch::asm!(
                "csrw satp, {satp}",
                "sfence.vma",
                satp = in(reg) kernel_satp,
            );
        }

        // Print via UART directly (no locks)
        let uart = crate::driver::uart::Uart::new(0x1000_0000);
        for byte in msg.bytes() {
            if byte == b'\n' {
                uart.putc(b'\r');
            }
            uart.putc(byte);
        }

        // Restore satp
        if kernel_satp != 0 && kernel_satp != saved_satp {
            core::arch::asm!(
                "csrw satp, {satp}",
                "sfence.vma",
                satp = in(reg) saved_satp,
            );
        }
    }
}

/// Print a hex value safely.
#[allow(dead_code)]
pub fn safe_print_hex(val: usize) {
    let mut buf = [0u8; 20];
    buf[0] = b'0';
    buf[1] = b'x';
    let mut i = 2;
    let mut started = false;
    for shift in (0..64).step_by(4).rev() {
        let nibble = (val >> shift) & 0xf;
        if nibble != 0 || started || shift == 0 {
            buf[i] = if nibble < 10 {
                b'0' + nibble as u8
            } else {
                b'a' + (nibble - 10) as u8
            };
            i += 1;
            started = true;
        }
    }
    let s = core::str::from_utf8(&buf[..i]).unwrap_or("?");
    safe_print(s);
}
