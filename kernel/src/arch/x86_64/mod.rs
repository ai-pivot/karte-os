//! x86_64 architecture support for KarteOS
//!
//! This module provides a complete x86_64 backend using the `x86_64` crate
//! to minimize hand-written assembly. Only two places use assembly:
//! - `boot.S`: Multiboot2 entry → 64-bit long mode transition (~80 lines)
//! - `switch.S`: Callee-saved register save/restore for context switching (~20 lines)
//!
//! Everything else (GDT, IDT, paging, port I/O, TLB, interrupts) is pure Rust
//! via the `x86_64` crate.

pub mod boot;
pub mod cet;
pub mod console;
pub mod cr3;
pub mod emergency_stack;
pub mod gdt;
pub mod idt;
pub mod ioapic;
pub mod lapic;
pub mod multiboot2;
pub mod paging;
pub mod pci;
pub mod platform;
pub mod smp;
pub mod switch;
pub mod trap;
pub mod uart;
pub mod user_return;
pub mod virtio_blk;
pub mod virtio_net;
pub mod rtc;

#[cfg(feature = "test_mode")]
pub mod test;

// Re-export core types used by the rest of the kernel
pub use trap::TrapContext;
