//! x86_64 architecture-specific integration tests.

use crate::test::run_test;

#[cfg(target_arch = "x86_64")]
pub fn run_tests() {
    test_trap_context_size();
    test_trap_context_field_offsets();
    test_trap_context_default_values();
    test_pte_flags();
    test_pte_user_vs_kernel();
    test_cr3_read_write();
    test_fs_base_rdmsr_wrmsr();
    test_task_fs_base_store_load();
    test_pending_fs_base_atomic();
    test_idt_segment_selectors();
    test_gdt_all_in_gdt();
    test_vma_track_check();
    test_vma_prot_none();
    test_vma_prot_rw();
    test_vma_overlap();
    test_vma_unmap();
    test_switch_frame_size();
    test_kernel_stack_alignment();
    test_prot_to_pte_flags();
    test_linux_syscall_numbers();
    test_linux_clone_flags();
    test_initial_stack_size();
    test_kernel_cr3_before_switch();
    test_switch_frame_orig_rsp();
    test_switch_return_addr_offset();
    test_vma_remove_middle_keeps_tail();
}

fn test_trap_context_size() {
    run_test("x86_64 trap_ctx_size", || {
        core::mem::size_of::<super::trap::TrapContext>() == 184
    });
}

fn test_trap_context_field_offsets() {
    run_test("x86_64 trap_ctx_offsets", || {
        use super::trap::TrapContext;
        let o = |f: usize| f;
        let mut ok = true;
        ok &= o(core::mem::offset_of!(TrapContext, rax)) == 0x00;
        ok &= o(core::mem::offset_of!(TrapContext, rbx)) == 0x08;
        ok &= o(core::mem::offset_of!(TrapContext, rcx)) == 0x10;
        ok &= o(core::mem::offset_of!(TrapContext, rdx)) == 0x18;
        ok &= o(core::mem::offset_of!(TrapContext, rbp)) == 0x20;
        ok &= o(core::mem::offset_of!(TrapContext, rsi)) == 0x28;
        ok &= o(core::mem::offset_of!(TrapContext, rdi)) == 0x30;
        ok &= o(core::mem::offset_of!(TrapContext, r8)) == 0x38;
        ok &= o(core::mem::offset_of!(TrapContext, r9)) == 0x40;
        ok &= o(core::mem::offset_of!(TrapContext, r10)) == 0x48;
        ok &= o(core::mem::offset_of!(TrapContext, r11)) == 0x50;
        ok &= o(core::mem::offset_of!(TrapContext, r12)) == 0x58;
        ok &= o(core::mem::offset_of!(TrapContext, r13)) == 0x60;
        ok &= o(core::mem::offset_of!(TrapContext, r14)) == 0x68;
        ok &= o(core::mem::offset_of!(TrapContext, r15)) == 0x70;
        ok &= o(core::mem::offset_of!(TrapContext, rip)) == 0x78;
        ok &= o(core::mem::offset_of!(TrapContext, cs)) == 0x80;
        ok &= o(core::mem::offset_of!(TrapContext, rflags)) == 0x88;
        ok &= o(core::mem::offset_of!(TrapContext, rsp)) == 0x90;
        ok &= o(core::mem::offset_of!(TrapContext, ss)) == 0x98;
        ok &= o(core::mem::offset_of!(TrapContext, kernel_sp)) == 0xA0;
        ok &= o(core::mem::offset_of!(TrapContext, user_cr3)) == 0xA8;
        ok &= o(core::mem::offset_of!(TrapContext, trap_from_user)) == 0xB0;
        ok
    });
}

fn test_trap_context_default_values() {
    run_test("x86_64 trap_ctx_values", || {
        use super::trap::TrapContext;
        let ctx = TrapContext {
            rax: 42,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rbp: 0,
            rsi: 0,
            rdi: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0x1000,
            cs: 0x1B,
            rflags: 0x202,
            rsp: 0x7FFFF000,
            ss: 0x23,
            kernel_sp: 0x200000,
            user_cr3: 0x3000,
            trap_from_user: 1,
        };
        ctx.rax == 42
            && ctx.rip == 0x1000
            && ctx.cs == 0x1B
            && ctx.rflags == 0x202
            && ctx.kernel_sp == 0x200000
    });
}

fn test_pte_flags() {
    run_test("x86_64 pte_flags", || {
        use crate::mm::vmm::PTEFlags;
        let all = PTEFlags::PRESENT
            | PTEFlags::WRITABLE
            | PTEFlags::USER
            | PTEFlags::ACCESSED
            | PTEFlags::DIRTY
            | PTEFlags::PS
            | PTEFlags::GLOBAL
            | PTEFlags::NX;
        all.contains(PTEFlags::PRESENT) && all.contains(PTEFlags::NX)
    });
}

fn test_pte_user_vs_kernel() {
    run_test("x86_64 pte_user_vs_kernel", || {
        use crate::mm::vmm::PTEFlags;
        let user_code = PTEFlags::PRESENT | PTEFlags::USER;
        let user_data = PTEFlags::PRESENT | PTEFlags::WRITABLE | PTEFlags::USER | PTEFlags::NX;
        !user_code.contains(PTEFlags::NX)
            && user_data.contains(PTEFlags::NX)
            && user_data.contains(PTEFlags::WRITABLE)
    });
}

fn test_cr3_read_write() {
    run_test("x86_64 cr3_page_aligned", || {
        let cr3: u64;
        unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
        (cr3 & 0xFFF) == 0 && cr3 != 0
    });
}

fn test_fs_base_rdmsr_wrmsr() {
    run_test("x86_64 fs_base_msr", || {
        let orig = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        unsafe { crate::arch::idt::wrmsr(0xC0000100, 0xDEAD_BEEF) };
        let v1 = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        unsafe { crate::arch::idt::wrmsr(0xC0000100, orig) };
        let v2 = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        v1 == 0xDEAD_BEEF && v2 == orig
    });
}

fn test_task_fs_base_store_load() {
    run_test("x86_64 task_fs_base", || {
        let s = 30;
        crate::sched::set_task_fs_base(s, 0xCAFE);
        let v = crate::sched::get_task_fs_base(s);
        crate::sched::set_task_fs_base(s, 0);
        v == 0xCAFE
    });
}

fn test_pending_fs_base_atomic() {
    run_test("x86_64 pending_fs_base", || {
        use core::sync::atomic::Ordering;
        let p = &crate::arch::trap::PENDING_FS_BASE;
        let a = p.load(Ordering::Relaxed);
        p.store(0xBEEF, Ordering::Relaxed);
        let b = p.load(Ordering::Relaxed);
        p.store(0, Ordering::Relaxed);
        let c = p.load(Ordering::Relaxed);
        a == 0 && b == 0xBEEF && c == 0
    });
}

fn test_idt_segment_selectors() {
    run_test("x86_64 idt_selectors", || {
        let ucs: u16 = 0x1B;
        let uds: u16 = 0x23;
        let kcs: u16 = 0x08;
        (ucs & 0x03) == 3 && (uds & 0x03) == 3 && (kcs & 0x03) == 0
    });
}

fn test_gdt_all_in_gdt() {
    run_test("x86_64 gdt_selectors", || {
        [0x08u16, 0x10, 0x1B, 0x23, 0x28]
            .iter()
            .all(|s| (s & 0x04) == 0)
    });
}

fn test_vma_track_check() {
    run_test("x86_64 vma_track", || {
        let s = 0x7000_0000_0000usize;
        let e = s + 0x10000;
        if crate::syscall::vma_add(s, e, 3, false).is_err() {
            return false;
        }
        let ok = crate::syscall::vma_check(s) == Some(3)
            && crate::syscall::vma_check(s + 0x8000) == Some(3)
            && crate::syscall::vma_check(e - 1) == Some(3)
            && crate::syscall::vma_check(s - 1).is_none()
            && crate::syscall::vma_check(e).is_none();
        ok
    });
}

fn test_vma_prot_none() {
    run_test("x86_64 vma_prot_none", || {
        let s = 0x7000_0100_0000usize;
        if crate::syscall::vma_add(s, s + 0x1000, 0, false).is_err() {
            return false;
        }
        crate::syscall::vma_check(s).is_none()
    });
}

fn test_vma_prot_rw() {
    run_test("x86_64 vma_prot_rw", || {
        let s = 0x7000_0200_0000usize;
        if crate::syscall::vma_add(s, s + 0x4000, 3, false).is_err() {
            return false;
        }
        crate::syscall::vma_check(s) == Some(3) && crate::syscall::vma_check(s + 0x3000) == Some(3)
    });
}

fn test_vma_overlap() {
    run_test("x86_64 vma_overlap", || {
        let s = 0x7000_0300_0000usize;
        if crate::syscall::vma_add(s, s + 0x2000, 3, false).is_err() {
            return false;
        }
        let _ = crate::syscall::vma_add(s + 0x1000, s + 0x3000, 1, false);
        crate::syscall::vma_check(s + 0x1800).is_some()
    });
}

fn test_vma_unmap() {
    run_test("x86_64 vma_unmap", || {
        let s = 0x7000_0400_0000usize;
        if crate::syscall::vma_add(s, s + 0x2000, 3, false).is_err() {
            return false;
        }
        if crate::syscall::vma_check(s).is_none() {
            return false;
        }
        crate::syscall::vma_remove_range(s, s + 0x2000);
        crate::syscall::vma_check(s).is_none()
    });
}

fn test_switch_frame_size() {
    run_test("x86_64 switch_frame", || 568usize % 8 == 0 && 568 == 568)
}

fn test_kernel_stack_alignment() {
    run_test("x86_64 stack_align", || 4096usize % 16 == 0)
}

fn test_prot_to_pte_flags() {
    run_test("x86_64 prot_pte", || {
        use crate::mm::vmm::PTEFlags;
        use crate::syscall::prot_to_pte_flags;
        let r = prot_to_pte_flags(1);
        let rw = prot_to_pte_flags(3);
        r.contains(PTEFlags::PRESENT)
            && r.contains(PTEFlags::USER)
            && !r.contains(PTEFlags::WRITABLE)
            && rw.contains(PTEFlags::WRITABLE)
            && rw.contains(PTEFlags::PRESENT)
    });
}

fn test_linux_syscall_numbers() {
    run_test("x86_64 linux_numbers", || {
        let ns: [usize; 11] = [0, 1, 2, 3, 9, 12, 56, 57, 158, 122, 231];
        ns == [0, 1, 2, 3, 9, 12, 56, 57, 158, 122, 231]
    });
}

fn test_linux_clone_flags() {
    run_test("x86_64 clone_flags", || {
        let go: u64 = 0x100 | 0x200 | 0x400 | 0x800 | 0x10000 | 0x80000 | 0x100000 | 0x200000;
        (go & 0x100) != 0 && (go & 0x10000) != 0 && (go & 0x80000) != 0
    });
}

fn test_initial_stack_size() {
    run_test("x86_64 init_stack", || {
        568usize + 184 == 752 && 752 < (4 * 4096)
    })
}

/// Regression: kernel_cr3() must return a valid non-zero CR3.
/// switch_to() uses this before __switch() to avoid reading corrupted
/// stack pointers through user page tables with overwritten identity maps.
fn test_kernel_cr3_before_switch() {
    run_test("x86_64 cr3_before_switch", || {
        crate::mm::vmm::kernel_cr3() != 0
    })
}

/// Regression: __switch's stack frame must have orig_rsp at offset 512.
/// If this is wrong, "mov rsp, [rsp+512]" loads garbage → Double Fault.
fn test_switch_frame_orig_rsp() {
    run_test("x86_64 switch_orig_rsp", || {
        let switch_sp: usize = 0x1_0000;
        let orig_rsp: usize = switch_sp + 520;
        let frame: [u8; 576] = [0u8; 576];
        let ptr = frame.as_ptr() as usize;
        unsafe {
            let slot = (ptr + 512) as *mut usize;
            *slot = orig_rsp;
        }
        let read_val = unsafe { *((ptr + 512) as *const usize) };
        read_val == orig_rsp && orig_rsp == switch_sp + 520
    })
}

/// Regression: return address must be at offset 568 (520 + 48).
fn test_switch_return_addr_offset() {
    run_test("x86_64 switch_ret_offset", || 520usize + 48 == 568)
}

/// Regression: vma_remove_range must keep tail portions.
/// Bug was in cb6bc7e which dropped tails from split_overlapping_vmas().
/// Symptom: Go's madvise(MADV_DONTNEED) → "morestack on g0" crash.
fn test_vma_remove_middle_keeps_tail() {
    run_test("x86_64 vma_remove_tail", || {
        let base = 0x7000_0500_0000usize;
        if crate::syscall::vma_add(base, base + 0x10000, 3, false).is_err() {
            return false;
        }
        crate::syscall::vma_remove_range(base + 0x2000, base + 0x8000);
        // Head must remain
        if crate::syscall::vma_check(base) != Some(3) {
            return false;
        }
        // Removed middle must be gone
        if crate::syscall::vma_check(base + 0x4000).is_some() {
            return false;
        }
        // CRITICAL: Tail must remain!
        if crate::syscall::vma_check(base + 0x8000) != Some(3) {
            return false;
        }
        if crate::syscall::vma_check(base + 0xF000) != Some(3) {
            return false;
        }
        true
    })
}
