//! Architecture-specific module.
//!
//! Supports two architectures via Cargo features:
//! - Default (no feature): RISC-V 64-bit (original KarteOS target)
//! - `arch_x86_64`: x86_64 (QEMU with Multiboot2 bootloader)

#[cfg(not(feature = "arch_x86_64"))]
pub mod plic;
#[cfg(not(feature = "arch_x86_64"))]
pub mod smp;
#[cfg(not(feature = "arch_x86_64"))]
pub mod trap;

#[cfg(feature = "arch_x86_64")]
pub mod x86_64;

// Re-export key types from the active architecture module
// so the rest of the kernel can use `crate::arch::TrapContext` etc.

#[cfg(not(feature = "arch_x86_64"))]
pub use trap::TrapContext;

#[cfg(feature = "arch_x86_64")]
pub use x86_64::TrapContext;
