// kernel/src/arch/emergency_stack.rs — Per-hart emergency stacks for S-mode traps
//
// When an S-mode trap occurs (e.g., page fault during kernel execution),
// the trap_entry code needs a valid stack to save the TrapContext. However,
// the current sp might be invalid (e.g., user sp after sret). To handle this
// robustly, each hart has a dedicated emergency stack used exclusively for
// S-mode exception handling.

/// 4 KB emergency stack per hart (8 harts max = 32 KB total).
/// Placed in BSS, identity-mapped, always accessible.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".bss")]
pub static mut EMERGENCY_STACKS: [u8; 4096 * 8] = [0u8; 4096 * 8];
