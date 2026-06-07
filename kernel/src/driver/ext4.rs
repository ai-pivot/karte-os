//! ext4 filesystem module — routes to architecture-specific implementation.

#[cfg(target_arch = "riscv64")]
#[path = "ext4_riscv.rs"]
mod ext4_arch;

#[cfg(target_arch = "x86_64")]
#[path = "ext4_x86_64.rs"]
mod ext4_arch;

pub use ext4_arch::*;
