//! Boot assembly module — embeds boot.S for Multiboot2 → long mode transition.
//!
//! This file exists solely to include the boot.S assembly via `global_asm!`.
//! The actual boot logic (page table setup, CR3/CR4/EFER programming,
//! GDT loading, far jump to 64-bit code) is in boot.S.

use core::arch::global_asm;

global_asm!(include_str!("boot.S"));
