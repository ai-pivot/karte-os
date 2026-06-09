//! Control-flow Enforcement Technology (CET) setup.
//!
//! KarteOS does not yet provide user shadow-stack mappings or SSP context
//! switching. Keep CET disabled explicitly so firmware/emulator defaults cannot
//! leak an uninitialized shadow stack into Ring 3.

const CR4_CET_BIT: u64 = 23;
const CPUID7_ECX_CET_SS: u32 = 1 << 7;
const CPUID7_EDX_CET_IBT: u32 = 1 << 20;

const IA32_U_CET: u32 = 0x6a0;
const IA32_S_CET: u32 = 0x6a2;
const IA32_PL0_SSP: u32 = 0x6a4;
const IA32_PL1_SSP: u32 = 0x6a5;
const IA32_PL2_SSP: u32 = 0x6a6;
const IA32_PL3_SSP: u32 = 0x6a7;

#[derive(Clone, Copy)]
struct CpuidResult {
    eax: u32,
    ecx: u32,
    edx: u32,
}

fn cpuid_count(leaf: u32, subleaf: u32) -> CpuidResult {
    let eax: u32;
    let _ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inlateout("eax") leaf => eax,
            inlateout("ecx") subleaf => ecx,
            lateout("edx") edx,
            ebx_out = lateout(reg) _ebx,
            options(nomem)
        );
    }
    CpuidResult { eax, ecx, edx }
}

fn cpu_has_cet() -> bool {
    let max_leaf = cpuid_count(0, 0);
    if max_leaf.eax < 7 {
        return false;
    }
    let leaf7 = cpuid_count(7, 0);
    (leaf7.ecx & CPUID7_ECX_CET_SS) != 0 || (leaf7.edx & CPUID7_EDX_CET_IBT) != 0
}

unsafe fn write_msr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") lo, in("edx") hi);
    }
}

/// Disable CET on the current CPU.
pub fn disable() {
    unsafe {
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        let new_cr4 = cr4 & !(1u64 << CR4_CET_BIT);
        if new_cr4 != cr4 {
            core::arch::asm!("mov cr4, {}", in(reg) new_cr4, options(nomem, nostack, preserves_flags));
        }
    }

    if !cpu_has_cet() {
        return;
    }

    unsafe {
        write_msr(IA32_U_CET, 0);
        write_msr(IA32_S_CET, 0);
        write_msr(IA32_PL0_SSP, 0);
        write_msr(IA32_PL1_SSP, 0);
        write_msr(IA32_PL2_SSP, 0);
        write_msr(IA32_PL3_SSP, 0);
    }
}
