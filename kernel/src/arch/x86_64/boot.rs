//! Boot assembly module — embeds boot.S for Multiboot2 → long mode transition.
//!
//! This file exists solely to include the boot.S assembly via `global_asm!`.
//!
//! The actual boot logic (Multiboot2 header, page table setup,
//! GDT loading, far jump to 64-bit code) is in boot.S.

use core::arch::global_asm;

global_asm!(include_str!("boot.S"), options(att_syntax),);
