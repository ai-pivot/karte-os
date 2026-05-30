//! RISC-V 64-bit architecture support.

pub mod emergency_stack;
pub mod plic;
pub mod sbi;
pub mod smp;
pub mod trap;

// Assembly files are included via global_asm! in the modules that need them:
// - entry.S: included from main.rs
// - trap_entry.S: included from trap.rs
// - switch.S: included from sched/mod.rs
