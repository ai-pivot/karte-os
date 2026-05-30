//! Architecture-specific modules.
//!
//! Each architecture provides: trap handling, SMP, interrupt controller,
//! SBI/BIOS interface, and context switching.

#[cfg(target_arch = "riscv64")]
mod riscv64;

#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

// x86_64 — reserved for another agent.
// #[cfg(target_arch = "x86_64")]
// mod x86_64;
// #[cfg(target_arch = "x86_64")]
// pub use x86_64::*;
