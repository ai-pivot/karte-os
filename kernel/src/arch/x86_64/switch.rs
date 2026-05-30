//! Context switch assembly module — embeds switch.S for callee-saved register save/restore.
//!
//! This file includes `switch.S` via `global_asm!`. The actual context
//! switch logic (push/pop callee-saved registers, swap stack pointers)
//! is in switch.S.
//!
//! The `__switch` function is declared `unsafe extern "C"` and called
//! by the scheduler in `sched/mod.rs`.

use core::arch::global_asm;

global_asm!(include_str!("switch.S"));

unsafe extern "C" {
    /// Context switch function.
    ///
    /// - `current_sp`: pointer to the current task's saved stack pointer
    ///   (will be overwritten with the current RSP)
    /// - `next_sp`: pointer to the next task's saved stack pointer
    ///   (RSP will be loaded from this location)
    ///
    /// # Safety
    /// Both pointers must point to valid `usize`-aligned memory.
    /// The next task's stack must contain a valid `__switch` frame
    /// (6 callee-saved regs × 8 bytes = 48 bytes) or a trap return
    /// sequence laid out by `add_user_process`.
    pub fn __switch(current_sp: *mut usize, next_sp: *const usize);
}
