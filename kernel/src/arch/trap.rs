// kernel/src/arch/trap.rs
// S-mode trap handling framework: context save/restore, dispatch, timer interrupts

use core::arch::global_asm;

use riscv::interrupt::{
    Trap,
    supervisor::{Exception, Interrupt},
};
use riscv::register::{scause, sie, stval, stvec};

unsafe extern "C" {
    fn trap_entry();
}

/// Trap context: saved registers on trap entry.
///
/// Layout (272 bytes total):
///   x[0..32]  @ offset 0..256   — 32 general-purpose registers
///   sstatus   @ offset 256      — Supervisor Status Register
///   sepc      @ offset 264      — Supervisor Exception Program Counter
#[repr(C)]
#[derive(Debug)]
pub struct TrapContext {
    /// General-purpose registers x0–x31
    pub x: [usize; 32],
    /// Supervisor Status Register
    pub sstatus: usize,
    /// Supervisor Exception Program Counter
    pub sepc: usize,
}

impl TrapContext {
    /// Create a new trap context with the given entry point and stack pointer.
    pub fn new(entry: usize, sp: usize) -> Self {
        let mut ctx = Self {
            x: [0; 32],
            sstatus: 0,
            sepc: entry,
        };
        ctx.x[2] = sp; // sp = x2
        ctx
    }
}

/// Set up the trap vector (Direct mode).
pub fn init() {
    unsafe {
        stvec::write(stvec::Stvec::new(
            trap_entry as *const () as usize,
            stvec::TrapMode::Direct,
        ));
    }
}

/// Enable supervisor timer interrupt.
pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
    }
}

/// Set the next timer interrupt (10 ms from now).
pub fn set_next_timer() {
    // QEMU virt machine clock frequency is 10 MHz
    const CLOCK_FREQ: usize = 10_000_000;
    const TICKS_PER_MS: usize = CLOCK_FREQ / 1000;
    let next = riscv::register::time::read() + 10 * TICKS_PER_MS;
    sbi_rt::set_timer(next as u64);
}

use core::sync::atomic::{AtomicUsize, Ordering};

// Timer tick counter
static TIMER_TICKS: AtomicUsize = AtomicUsize::new(0);

/// Handle timer interrupt.
fn handle_timer() {
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks % 100 == 0 {
        crate::console_println!("[timer] tick {} ({}s)", ticks, ticks / 100);
    }
    set_next_timer();
    crate::sched::schedule();
}

/// Trap handler (called from assembly).
///
/// Dispatches based on scause and handles each trap type.
#[unsafe(no_mangle)]
extern "C" fn trap_handler(ctx: &mut TrapContext) -> &mut TrapContext {
    let scause = scause::read();
    let stval = stval::read();

    // scause::read().cause() returns Trap<usize, usize> (raw codes).
    // Convert to typed Trap<Interrupt, Exception> via try_into().
    let trap: Trap<Interrupt, Exception> = match scause.cause().try_into() {
        Ok(t) => t,
        Err(_) => {
            crate::console_println!(
                "[trap] Unknown trap: sepc={:#x}, stval={:#x}",
                ctx.sepc,
                stval
            );
            ctx.sepc += 4;
            return ctx;
        }
    };

    match trap {
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            handle_timer();
        }
        Trap::Exception(Exception::UserEnvCall) => {
            // RISC-V ecall from U-mode: a7=syscall_id, a0-a5=args
            let syscall_id = ctx.x[17]; // a7
            let args = [
                ctx.x[10], ctx.x[11], ctx.x[12], ctx.x[13], ctx.x[14], ctx.x[15],
            ];
            let result = crate::syscall::dispatch(syscall_id, args);
            ctx.x[10] = result as usize; // a0 = return value
            ctx.sepc += 4; // skip ecall instruction
        }
        Trap::Exception(e) => {
            crate::console_println!(
                "[trap] Exception {:?}: sepc={:#x}, stval={:#x}",
                e,
                ctx.sepc,
                stval
            );
            // Skip the faulting instruction
            ctx.sepc += 4;
        }
        Trap::Interrupt(Interrupt::SupervisorExternal) => {
            crate::console_println!("[trap] External interrupt");
            crate::arch::plic::handle_interrupt(0);
        }
        _ => {
            crate::console_println!(
                "[trap] Unhandled trap: sepc={:#x}, stval={:#x}",
                ctx.sepc,
                stval
            );
            ctx.sepc += 4;
        }
    }

    ctx
}

// Trap entry point (assembly linkage).
// On trap entry the hardware jumps here (Direct mode). We save all caller-
// and callee-saved registers plus sstatus/sepc onto the stack, call
// trap_handler, restore everything, and return via sret.
global_asm!(
    "
    .section .text
    .global trap_entry
trap_entry:
    // Allocate TrapContext on the stack: 32*8 + 2*8 = 272 bytes, align to 280
    addi    sp, sp, -280

    // ---- Save general-purpose registers x0..x31 ----
    // x0 (zero) is always 0 — skip
    // x2 (sp) will be restored via addi — skip
    sd      x1,  8(sp)
    sd      x3,  24(sp)
    sd      x4,  32(sp)
    sd      x5,  40(sp)
    sd      x6,  48(sp)
    sd      x7,  56(sp)
    sd      x8,  64(sp)
    sd      x9,  72(sp)
    sd      x10, 80(sp)
    sd      x11, 88(sp)
    sd      x12, 96(sp)
    sd      x13, 104(sp)
    sd      x14, 112(sp)
    sd      x15, 120(sp)
    sd      x16, 128(sp)
    sd      x17, 136(sp)
    sd      x18, 144(sp)
    sd      x19, 152(sp)
    sd      x20, 160(sp)
    sd      x21, 168(sp)
    sd      x22, 176(sp)
    sd      x23, 184(sp)
    sd      x24, 192(sp)
    sd      x25, 200(sp)
    sd      x26, 208(sp)
    sd      x27, 216(sp)
    sd      x28, 224(sp)
    sd      x29, 232(sp)
    sd      x30, 240(sp)
    sd      x31, 248(sp)

    // ---- Save sstatus and sepc ----
    csrr    t0, sepc
    sd      t0, 264(sp)
    csrr    t0, sstatus
    sd      t0, 256(sp)

    // ---- Call trap_handler(&mut TrapContext) ----
    mv      a0, sp
    call    trap_handler

    // ---- Restore sstatus and sepc ----
    ld      t0, 264(sp)
    csrw    sepc, t0
    ld      t0, 256(sp)
    csrw    sstatus, t0

    // ---- Restore general-purpose registers ----
    ld      x1,  8(sp)
    ld      x3,  24(sp)
    ld      x4,  32(sp)
    ld      x5,  40(sp)
    ld      x6,  48(sp)
    ld      x7,  56(sp)
    ld      x8,  64(sp)
    ld      x9,  72(sp)
    ld      x10, 80(sp)
    ld      x11, 88(sp)
    ld      x12, 96(sp)
    ld      x13, 104(sp)
    ld      x14, 112(sp)
    ld      x15, 120(sp)
    ld      x16, 128(sp)
    ld      x17, 136(sp)
    ld      x18, 144(sp)
    ld      x19, 152(sp)
    ld      x20, 160(sp)
    ld      x21, 168(sp)
    ld      x22, 176(sp)
    ld      x23, 184(sp)
    ld      x24, 192(sp)
    ld      x25, 200(sp)
    ld      x26, 208(sp)
    ld      x27, 216(sp)
    ld      x28, 224(sp)
    ld      x29, 232(sp)
    ld      x30, 240(sp)
    ld      x31, 248(sp)

    addi    sp, sp, 280
    sret
"
);
