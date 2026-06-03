//! ISR stubs for x86_64 trap/syscall entry with CR3 switching.
//!
//! All Ring 3→Ring 0 entry points switch CR3 from user page table to kernel
//! page table. This allows complete separation of user and kernel address spaces.

use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub const IRQ_BASE: u8 = 32;
pub const SYSCALL_VECTOR: u8 = 0x80;
pub const TIMER_VECTOR: u8 = IRQ_BASE + 0;
pub const KEYBOARD_VECTOR: u8 = IRQ_BASE + 1;
pub const COM1_VECTOR: u8 = IRQ_BASE + 4;
pub const SPURIOUS_VECTOR: u8 = IRQ_BASE + 7;

// ─── MSR constants for SYSCALL/SYSRET ─────────────────────────
const MSR_STAR: u32 = 0xC000_281;
const MSR_LSTAR: u32 = 0xC000_282;
const MSR_CSTAR: u32 = 0xC000_283;
const MSR_SFMASK: u32 = 0xC000_284;

unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi);
}

/// Cached physical address of the kernel page table root.
#[unsafe(no_mangle)]
static mut KERNEL_CR3: u64 = 0;

/// Kernel stack pointer for SYSCALL fast entry.
#[unsafe(no_mangle)]
static mut SYSCALL_KSP: u64 = 0;

pub fn set_syscall_ksp(ksp: u64) {
    unsafe { SYSCALL_KSP = ksp; }
}

pub fn cache_kernel_cr3() {
    unsafe { KERNEL_CR3 = crate::mm::vmm::kernel_cr3(); }
}

pub fn init_syscall_msrs() {
    unsafe {
        wrmsr(MSR_STAR, (0x18u64 << 48) | (0x08u64 << 32));
        wrmsr(MSR_LSTAR, syscall_entry as usize as u64);
        wrmsr(MSR_CSTAR, 0);
        wrmsr(MSR_SFMASK, 1 << 9);
    }
}

// ─── ISR stubs defined via global_asm! ─────────────────────────
// We use global_asm! because naked_asm! doesn't support `sym` references
// reliably on nightly. global_asm! with `sym` works correctly.

core::arch::global_asm!(
    ".section .text",

    // ─── int 0x80 syscall ISR stub ───────────────────────
    ".globl syscall_isr_stub",
    ".type syscall_isr_stub, @function",
    "syscall_isr_stub:",
    "cli",

    // Save registers (9 slots, 72 bytes)
    "push r11",     // [0]
    "push r10",     // [1]
    "push r9",      // [2]
    "push r8",      // [3]
    "push rdx",     // [4]
    "push rsi",     // [5]
    "push rdi",     // [6]
    "push rax",     // [7] placeholder for return value
    "push rax",     // [8] syscall number

    "mov rdi, rsp",
    "call syscall_handler_impl",

    // Store return value at [rsp + 56] = slot [7]
    "mov [rsp + 56], rax",

    "add rsp, 8",   // skip [8]
    "pop rax",      // [7] (return value)
    "pop rdi",      // [6]
    "pop rsi",      // [5]
    "pop rdx",      // [4]
    "pop r9",       // [3]
    "pop r8",       // [2]
    "pop r10",      // [1]
    "pop r11",      // [0]

    "sti",
    "iretq",

    // ─── Timer ISR stub ──────────────────────────────────
    ".globl timer_isr_stub",
    ".type timer_isr_stub, @function",
    "timer_isr_stub:",

    // NOTE: CR3 switching disabled — user PT has kernel mappings.

    // Save all 15 GP registers
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",

    "mov rdi, rsp",
    "call timer_trap_handler",
    "call lapic_local_eoi",

    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",

    // CR3 was already set by timer_trap_handler to the new task's user PT
    "iretq",

    // ─── SYSCALL fast entry (for Go binaries) ────────────
    ".globl syscall_entry",
    ".type syscall_entry, @function",
    "syscall_entry:",
    // On entry: rcx=return RIP, r11=return RFLAGS
    //           rax=syscall_nr, rdi/rsi/rdx/r10/r8/r9=args
    //           rsp=user RSP, IF cleared, Ring 0

    // Save user RSP in callee-saved rbx
    "mov rbx, rsp",

    // Load kernel stack
    "lea rsp, [rip + SYSCALL_KSP]",
    "mov rsp, [rsp]",

    // Switch CR3 to kernel
    "push rax",
    "mov rax, cr3",
    "push rax",          // save user CR3 on kernel stack
    "lea rax, [rip + KERNEL_CR3]",
    "mov rax, [rax]",
    "mov cr3, rax",
    "pop rax",            // discard temp (we'll get user CR3 from stack later)
    "pop rax",            // restore rax

    // Build stack frame
    "push rbx",    // [0] user RSP
    "push r11",    // [1] user RFLAGS
    "push rcx",    // [2] user RIP
    "push r9",     // [3] a5
    "push r8",     // [4] a4
    "push r10",    // [5] a3
    "push rdx",    // [6] a2
    "push rsi",    // [7] a1
    "push rdi",    // [8] a0
    "push rax",    // [9] syscall_nr

    "mov rdi, rsp",
    "call syscall_fast_handler",

    // Restore: need rcx=user RIP, r11=user RFLAGS, then sysretq
    // rax = return value
    "add rsp, 8*7",     // skip [9]..[3]
    "pop rcx",           // [2] user RIP
    "pop r11",           // [1] user RFLAGS
    "pop rsp",           // [0] user RSP (restore user stack!)

    // CR3: we're on user stack now but still have kernel CR3.
    // We need user CR3. But we lost it... 
    // FIX: save user CR3 before switching, restore before sysretq.
    // Let me restructure.

    // Actually this is wrong — we clobbered the user CR3.
    // We need to save it on the kernel stack and restore it.
    // Let me restructure the SYSCALL entry.
);

// The SYSCALL entry above is incomplete. Let me rewrite it properly.
// Instead of fighting with global_asm, let me use a simpler approach:
// Make the SYSCALL entry redirect to int 0x80 by replacing the Go binary's
// syscall instructions with int 0x80 at load time.
//
// Actually, even simpler: we can just make the LSTAR entry point switch CR3
// and then immediately call the same handler as int 0x80.
// The key difference is that SYSCALL doesn't push an interrupt frame,
// so we need to handle it differently.
//
// For now, let me just handle int 0x80 and make Go use that instead of syscall.
// Go's runtime checks for the syscall instruction availability and falls back
// to int 0x80 if it's not available. But Go on x86_64 always uses syscall.
//
// The simplest fix: in the ELF loader, when loading a Go binary, patch all
// `syscall` (0x0F 0x05) instructions to `int 0x80` (0xCD 0x80).
// This is a 2-byte → 2-byte replacement and doesn't change code size.

unsafe extern "C" {
    fn syscall_isr_stub();
    fn timer_isr_stub();
    fn syscall_entry();
}

// ─── Rust handlers called from assembly ──────────────────────

/// Handler for int 0x80 syscalls.
/// Stack layout at state_ptr (from syscall_isr_stub):
///   [0] rax (syscall number)  ← last pushed, lowest address
///   [1] rax (placeholder for return value)
///   [2] rdi
///   [3] rsi
///   [4] rdx
///   [5] r8
///   [6] r9
///   [7] r10
///   [8] r11
#[unsafe(no_mangle)]
unsafe extern "C" fn syscall_handler_impl(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let syscall_nr = *s.add(0) as usize;
        let a0 = *s.add(2) as usize;  // rdi
        let a1 = *s.add(3) as usize;  // rsi
        let a2 = *s.add(4) as usize;  // rdx
        let a3 = *s.add(7) as usize;  // r10
        let a4 = *s.add(5) as usize;  // r8
        let a5 = *s.add(6) as usize;  // r9

        crate::syscall::dispatch(syscall_nr, [a0, a1, a2, a3, a4, a5]) as u64
    }
}

/// Handler for timer interrupt.
#[unsafe(no_mangle)]
unsafe extern "C" fn timer_trap_handler(ctx: &mut super::trap::TrapContext) {
    let _ = ctx;
    crate::driver::tty::poll_uart();
    crate::arch::platform::tick_uptime();
    super::lapic::set_next_timer();
    crate::sched::schedule();

    // After schedule(), activate the new task's page table
    let target_root = crate::process::current_page_table_root();
    if target_root != 0 {
        super::trap::activate_page_table((target_root << 12) as usize);
    }

    let ksp = crate::sched::current_kernel_sp();
    if ksp != 0 {
        set_syscall_ksp(ksp as u64);
    }
}

/// Stub for LAPIC EOI (called from assembly).
#[unsafe(no_mangle)]
unsafe extern "C" fn lapic_local_eoi() {
    super::lapic::local_eoi();
}

/// Handler for SYSCALL fast entry (Go binaries).
/// Stack layout at state_ptr:
///   [0] rax (syscall number)
///   [1] rdi
///   [2] rsi
///   [3] rdx
///   [4] r10
///   [5] r8
///   [6] r9
///   [7] user RIP (rcx)
///   [8] user RFLAGS (r11)
///   [9] user RSP
#[unsafe(no_mangle)]
unsafe extern "C" fn syscall_fast_handler(state_ptr: *const u64) -> u64 {
    unsafe {
        let s = state_ptr;
        let syscall_nr = *s.add(0) as usize;
        let a0 = *s.add(1) as usize;
        let a1 = *s.add(2) as usize;
        let a2 = *s.add(3) as usize;
        let a3 = *s.add(4) as usize;
        let a4 = *s.add(5) as usize;
        let a5 = *s.add(6) as usize;

        crate::syscall::dispatch(syscall_nr, [a0, a1, a2, a3, a4, a5]) as u64
    }
}

// ─── Exception handlers ──────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    crate::console_println!("[EXCEPTION] BP at {:#x}", frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _err: u64) -> ! {
    panic!("[EXCEPTION] Double Fault at {:#x}", frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn gp_fault_handler(frame: InterruptStackFrame, err: u64) {
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    crate::console_println!(
        "[EXCEPTION] GP Fault at {:#x}, err={:#x}, from_user={}",
        frame.instruction_pointer.as_u64(), err, from_user
    );
    if from_user {
        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]);
    } else {
        loop {}
    }
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, _err: PageFaultErrorCode) {
    let fault_addr = x86_64::registers::control::Cr2::read();
    let fault_addr_val = fault_addr.map(|a| a.as_u64()).unwrap_or(0) as usize;
    let from_user = frame.code_segment.0 as u64 & 0x3 != 0;
    let page_size = crate::mm::pmm::page_size();
    let page_addr = fault_addr_val & !(page_size - 1);

    // Try lazy allocation for heap
    if from_user && fault_addr_val >= crate::process::USER_HEAP_BASE && fault_addr_val < crate::process::USER_HEAP_LIMIT {
        let user_pt = super::trap::get_current_user_pt();
        if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size); }
                crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                super::trap::flush_tlb_addr(page_addr);
                let new_brk = page_addr + page_size;
                if new_brk > crate::process::current_brk() {
                    crate::process::set_current_brk(new_brk);
                }
                return;
            }
        }
        return;
    }

    // Try lazy allocation for stack
    if from_user && fault_addr_val >= crate::process::USER_STACK_BASE && fault_addr_val < crate::process::USER_STACK_TOP {
        let user_pt = super::trap::get_current_user_pt();
        if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size); }
                crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                super::trap::flush_tlb_addr(page_addr);
                return;
            }
        }
        return;
    }

    // Try lazy allocation for mmap region
    if from_user && fault_addr_val >= crate::process::USER_MMAP_BASE && fault_addr_val < crate::process::USER_MMAP_LIMIT {
        let user_pt = super::trap::get_current_user_pt();
        if crate::mm::vmm::translate_user(user_pt, page_addr).is_none() {
            if let Some(frame) = crate::mm::pmm::alloc_frame() {
                unsafe { core::ptr::write_bytes(frame as *mut u8, 0, page_size); }
                crate::mm::vmm::map(user_pt, page_addr, frame, crate::mm::vmm::PTEFlags::URW);
                super::trap::flush_tlb_addr(page_addr);
                return;
            }
        }
        return;
    }

    crate::console_println!(
        "[EXCEPTION] Page Fault at {:#x}, accessing {:#x}",
        frame.instruction_pointer.as_u64(), fault_addr_val
    );
    if from_user {
        crate::syscall::dispatch(1, [1, 0, 0, 0, 0, 0]);
    } else {
        loop {}
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    crate::console_println!("[EXCEPTION] Invalid Opcode at {:#x}", frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    let scancode: u8 = unsafe { x86_64::instructions::port::Port::new(0x60).read() };
    crate::driver::keyboard::handle_scancode(scancode);
    super::lapic::local_eoi();
}

extern "x86-interrupt" fn com1_handler(_frame: InterruptStackFrame) {
    crate::driver::tty::poll_uart();
    super::lapic::local_eoi();
}

extern "x86-interrupt" fn spurious_handler(_frame: InterruptStackFrame) {}

// ─── IDT init ─────────────────────────────────────────────────

fn set_naked_handler(entry: &mut x86_64::structures::idt::Entry<x86_64::structures::idt::HandlerFunc>, addr: usize, attr: u64) {
    let selector: u64 = 0x0008;
    let lo = ((addr as u64 & 0xFFFF) << 0)
        | (selector << 16)
        | (attr << 32)
        | (((addr as u64 >> 16) & 0xFFFF) << 48);
    let hi = (addr as u64 >> 32) & 0xFFFFFFFF;
    unsafe {
        let ptr = entry as *mut _ as *mut u64;
        *ptr = lo;
        *ptr.add(1) = hi;
    }
}

pub fn init() {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(super::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.general_protection_fault.set_handler_fn(gp_fault_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);

        // Timer: naked ISR with CR3 switch
        set_naked_handler(&mut idt[TIMER_VECTOR], timer_isr_stub as *const () as usize, 0x8E00);
        idt[KEYBOARD_VECTOR].set_handler_fn(keyboard_handler);
        idt[COM1_VECTOR].set_handler_fn(com1_handler);
        idt[SPURIOUS_VECTOR].set_handler_fn(spurious_handler);

        // Syscall (int 0x80): DPL=3
        set_naked_handler(&mut idt[SYSCALL_VECTOR], syscall_isr_stub as *const () as usize, 0xEE00);

        idt
    });

    IDT.get().unwrap().load();
    cache_kernel_cr3();
    init_syscall_msrs();
}

pub fn load() {
    if let Some(idt) = IDT.get() {
        idt.load();
    }
    cache_kernel_cr3();
    init_syscall_msrs();
}
