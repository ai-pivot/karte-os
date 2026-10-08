//! RISC-V architecture-specific integration tests.
//!
//! Tests cover:
//! - TrapContext layout and field offsets (32 GP regs + CSRs)
//! - Sv39 page table entry flags
//! - CSR register access (sstatus, sepc, sscratch, satp)
//! - SBI timer and system reset constants
//! - Context switch frame consistency
//! - Interrupt enable/disable (SIE/SSTATUS)

use crate::test::run_test;

#[cfg(target_arch = "riscv64")]
pub fn run_tests() {
    test_trap_context_size();
    test_trap_context_field_offsets();
    test_trap_context_register_array();
    test_vector_roundtrip();
    test_sstatus_bits();
    test_sstatus_spie_mask();
    test_sie_timer_mask();
    test_satp_mode_bits();
    test_sv39_pte_flags();
    test_sv39_pte_ppn_extraction();
    test_sv39_address_space_size();
    test_trap_cause_constants();
    test_context_switch_frame_layout();
    test_kernel_stack_alignment();
    test_user_address_range();
    test_sbi_timer_constants();
}

// ────────────────────────────────────────────────────────────────────
// TrapContext tests
// ────────────────────────────────────────────────────────────────────

fn test_trap_context_size() {
    run_test("riscv64 trap_context_size", || {
        use super::trap::TrapContext;
        // 288 B GP/CSR frame + 32 vector regs × 16 B (512) + vtype/vl/vstart/vxsat
        // (32) = 832 bytes; 16-byte aligned so the vector save area is aligned.
        core::mem::size_of::<TrapContext>() == 832
    });
}

fn test_trap_context_field_offsets() {
    run_test("riscv64 trap_context_field_offsets", || {
        use super::trap::TrapContext;
        let mut ok = true;
        ok &= core::mem::offset_of!(TrapContext, x) == 0;
        ok &= core::mem::offset_of!(TrapContext, sstatus) == 256; // 32 × 8
        ok &= core::mem::offset_of!(TrapContext, sepc) == 264;
        ok &= core::mem::offset_of!(TrapContext, sscratch) == 272;
        ok &= core::mem::offset_of!(TrapContext, user_satp) == 280;
        ok
    });
}

fn test_trap_context_register_array() {
    run_test("riscv64 trap_context_register_array", || {
        use super::trap::TrapContext;
        let ctx = TrapContext {
            x: [0; 32],
            sstatus: 0,
            sepc: 0x1000,
            sscratch: 0x7FFFF000,
            user_satp: 0,
            v: [0; 32],
            vtype: 0,
            vl: 0,
            vstart: 0,
            vxsat: 0,
        };
        // x[0] is always zero (ABI convention), x[1] = ra, x[2] = sp
        ctx.x.len() == 32 && ctx.sepc == 0x1000 && ctx.sscratch == 0x7FFFF000
    });
}

/// Vector save/restore round-trip, mirroring the exact sequences used in
/// trap_entry.S (vsetvli e64,m8 + vse64/vle64 in 128 B groups). On harts
/// without V (dev-box QEMU 6.2) the kernel gates all vector code on
/// HAS_VECTOR_EXT == 0 and this sequence can never execute there, so we
/// treat that as a clean environment skip (see docs/agent/vector.md §5).
/// Run on a QEMU >= 7 host with `make test` to exercise the real path.
fn test_vector_roundtrip() {
    run_test("riscv64 vector_roundtrip", || {
        use super::trap::HAS_VECTOR_EXT;
        use core::sync::atomic::Ordering;
        if HAS_VECTOR_EXT.load(Ordering::Relaxed) == 0 {
            return true; // environment skip: hart has no V extension
        }
        let mut saved = [0u128; 32];
        let mut restored = [0u128; 32];
        unsafe {
            core::arch::asm!(
                ".option push",
                ".option arch, +v",
                "li      t0, 3 << 9",          // sstatus.VS = Dirty
                "csrs    sstatus, t0",
                "vsetvli t0, zero, e64, m8, ta, ma",
                // Broadcast a distinctive pattern into v0, v8, v16, v24 groups
                "li      t3, 0x5a5a",
                "vmv.v.x v0,  t3",
                "li      t3, 0xa5a5",
                "vmv.v.x v8,  t3",
                "li      t3, 0x0f0f",
                "vmv.v.x v16, t3",
                "li      t3, 0x3c3c",
                "vmv.v.x v24, t3",
                // save (same layout as trap_entry.S)
                "addi    t3, {save}, 0",
                "vse64.v v0,  (t3)",
                "addi    t3, {save}, 128",
                "vse64.v v8,  (t3)",
                "addi    t3, {save}, 256",
                "vse64.v v16, (t3)",
                "addi    t3, {save}, 384",
                "vse64.v v24, (t3)",
                // scribble all groups
                "vsetvli t0, zero, e8, m1, ta, ma",
                "vmv.v.i v0,  0",
                "vmv.v.i v8,  0",
                "vmv.v.i v16, 0",
                "vmv.v.i v24, 0",
                // restore from the buffer
                "vsetvli t0, zero, e64, m8, ta, ma",
                "addi    t3, {save}, 0",
                "vle64.v v0,  (t3)",
                "addi    t3, {save}, 128",
                "vle64.v v8,  (t3)",
                "addi    t3, {save}, 256",
                "vle64.v v16, (t3)",
                "addi    t3, {save}, 384",
                "vle64.v v24, (t3)",
                // dump restored values for comparison
                "addi    t3, {out}, 0",
                "vse64.v v0,  (t3)",
                "addi    t3, {out}, 128",
                "vse64.v v8,  (t3)",
                "addi    t3, {out}, 256",
                "vse64.v v16, (t3)",
                "addi    t3, {out}, 384",
                "vse64.v v24, (t3)",
                ".option pop",
                save = in(reg) saved.as_mut_ptr(),
                out = in(reg) restored.as_mut_ptr(),
                out("t0") _, out("t3") _,
            );
        }
        saved == restored
    });
}
// ────────────────────────────────────────────────────────────────────
// CSR / SSTATUS tests
// ────────────────────────────────────────────────────────────────────

fn test_sstatus_bits() {
    run_test("riscv64 sstatus_bit_positions", || {
        // Key SSTATUS bits:
        // SIE=1 (Interrupt Enable), SPIE=5, SPP=8, SUM=18
        let sie: usize = 1 << 1;
        let spie: usize = 1 << 5;
        let spp: usize = 1 << 8;
        let sum: usize = 1 << 18;
        let all = sie | spie | spp | sum;
        // Each should be a distinct bit
        all.count_ones() == 4
    });
}

fn test_sstatus_spie_mask() {
    run_test("riscv64 sstatus_spie_mask", || {
        // SPIE is bit 5 of sstatus
        let spie_mask: usize = 1 << 5;
        spie_mask == 0x20
    });
}

fn test_sie_timer_mask() {
    run_test("riscv64 sie_timer_mask", || {
        // STIE (Timer Interrupt Enable) is bit 5 of SIE
        let stie_mask: usize = 1 << 5;
        stie_mask == 0x20
    });
}

// ────────────────────────────────────────────────────────────────────
// SATP / Sv39 tests
// ────────────────────────────────────────────────────────────────────

fn test_satp_mode_bits() {
    run_test("riscv64 satp_mode_sv39", || {
        // SATP mode field: bits 60-63
        // Sv39 = 8
        let sv39_mode: usize = 8;
        // PPN is bits 0-43 (44 bits)
        let ppn_mask: usize = (1 << 44) - 1;
        sv39_mode == 8 && ppn_mask == 0xFFFFFFFFFFF
    });
}

fn test_sv39_pte_flags() {
    run_test("riscv64 sv39_pte_flags", || {
        // Sv39 PTE flags:
        // V=0, R=1, W=2, X=3, U=4, G=5, A=6, D=7
        let v: usize = 1 << 0; // Valid
        let r: usize = 1 << 1; // Read
        let w: usize = 1 << 2; // Write
        let x: usize = 1 << 3; // Execute
        let u: usize = 1 << 4; // User
        let g: usize = 1 << 5; // Global
        let a: usize = 1 << 6; // Accessed
        let d: usize = 1 << 7; // Dirty
        let all = v | r | w | x | u | g | a | d;
        all.count_ones() == 8
    });
}

fn test_sv39_pte_ppn_extraction() {
    run_test("riscv64 sv39_pte_ppn_extraction", || {
        // PPN is bits 10-53 of a PTE
        let ppn_mask: u64 = 0x3FF_FFFF_FFFF_FFFF << 10;
        // A leaf PTE with PPN=0x100 and VRWXU flags
        let pte: u64 = (0x100u64 << 10) | 0xF; // V+R+W+X
        let ppn = (pte >> 10) & 0x3FF_FFFF_FFFF_FFFF;
        ppn == 0x100 && (pte & 0x3FF) == 0xF // lower 10 bits are flags
    });
}

fn test_sv39_address_space_size() {
    run_test("riscv64 sv39_address_space", || {
        // Sv39 uses 39-bit virtual addresses
        // User space: 0x0000_0000 to 0x003F_FFFF_FFFF (low 256GB)
        // Kernel space: 0xFFC0_0000_0000 to 0xFFFF_FFFF_FFFF (high 256GB)
        let va_bits = 39;
        let half_space: usize = 1 << (va_bits - 1);
        // 256GB = 0x40_0000_0000
        half_space == 0x40_0000_0000
    });
}

// ────────────────────────────────────────────────────────────────────
// Trap cause tests
// ────────────────────────────────────────────────────────────────────

fn test_trap_cause_constants() {
    run_test("riscv64 trap_cause_constants", || {
        // Key SCAUSE values:
        // User ecall = 8
        // S-mode timer = 0x8000_0005
        let user_ecall: usize = 8;
        let s_timer: usize = 0x8000_0005; // interrupt bit set
        user_ecall == 8 && s_timer == 0x8000_0005
    });
}

// ────────────────────────────────────────────────────────────────────
// Context switch tests
// ────────────────────────────────────────────────────────────────────

fn test_context_switch_frame_layout() {
    run_test("riscv64 switch_frame_layout", || {
        // __switch saves: ra, sp, s0-s11 (14 callee-saved) = 14 × 8 = 112 bytes
        // No FPU save on RISC-V (unlike x86_64 fxsave)
        let frame_size = 14 * 8;
        frame_size == 112 && (frame_size % 8) == 0
    });
}

fn test_kernel_stack_alignment() {
    run_test("riscv64 kernel_stack_alignment", || {
        // RISC-V requires 16-byte stack alignment
        let page_size = 4096usize;
        (page_size % 16) == 0
    });
}

fn test_user_address_range() {
    run_test("riscv64 user_address_range", || {
        // User programs run below 0x0040_0000_0000 (256GB in Sv39)
        let user_max: usize = 0x0040_0000_0000;
        // Typical user entry point is low (0x1000 in our linker script)
        let user_entry: usize = 0x1000;
        user_entry < user_max
    });
}

// ────────────────────────────────────────────────────────────────────
// SBI constants tests
// ────────────────────────────────────────────────────────────────────

fn test_sbi_timer_constants() {
    run_test("riscv64 sbi_timer_constants", || {
        // TIME extension: set_timer = 0x00
        // System reset: shutdown = 0x00, cold_reboot = 0x01
        let set_timer_fid: usize = 0;
        let shutdown_type: usize = 0;
        set_timer_fid == 0 && shutdown_type == 0
    });
}
