// kernel/src/arch/x86_64/emergency_stack.rs — Emergency stacks for x86_64
//
// On RISC-V, emergency stacks are used when an S-mode trap occurs and the
// current sp is invalid (e.g., user sp after sret). On x86_64, this role
// is filled by the TSS IST (Interrupt Stack Table) entries, which provide
// dedicated stacks for specific exceptions like double fault.
//
// This module provides a BSS-allocated emergency stack buffer that can be
// used by IST entries (configured in gdt.rs).

/// Emergency stack for IST[0] (double fault handler).
/// 32KB, placed in BSS, identity-mapped, always accessible.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".bss")]
pub static mut EMERGENCY_STACK: [u8; 4096 * 8] = [0u8; 4096 * 8];
