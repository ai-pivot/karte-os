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
global_asm!(include_str!("switch_nofpu.S"));

unsafe extern "C" {
    /// Context switch function (full, saves/restores FPU state).
    pub fn __switch(current_sp: *mut usize, next_sp: *const usize);

    /// Lightweight context switch (no FPU save/restore).
    /// Same stack frame layout as __switch — fully compatible.
    /// Sets CR0.TS after switching; FPU state is lazily restored on #NM.
    pub fn __switch_no_fpu(current_sp: *mut usize, next_sp: *const usize);
}
