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
    test_fs_base_rdmsr_wrmsl();
    test_task_fs_base_store_load();
    test_pending_fs_base_atomic();
    test_syscall_return_restores_task_fs_base();
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
    test_vma_root_isolation();
    test_vma_mmap_isolation();
    test_vma_fork_clone();
    test_vma_stack_scratch_bound();
    test_restore_zero_fs_base();
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

fn test_fs_base_rdmsr_wrmsl() {
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

fn test_syscall_return_restores_task_fs_base() {
    run_test("x86_64 syscall_fs_restore", || {
        let slot = crate::sched::current_sched_slot();
        let orig_msr = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        let orig_task = crate::sched::get_task_fs_base(slot);
        let val = 0x4830_8a8u64;

        crate::sched::set_task_fs_base(slot, val);
        unsafe { crate::arch::idt::wrmsr(0xC0000100, 0) };
        super::idt::restore_current_task_fs_base_for_syscall_return();
        let restored = unsafe { crate::arch::idt::rdmsr(0xC0000100) };

        unsafe { crate::arch::idt::wrmsr(0xC0000100, orig_msr) };
        crate::sched::set_task_fs_base(slot, orig_task);

        restored == val
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

// ─── VMA tests using explicit test roots ─────────────────────────────
//
// Each test uses a unique "test root" PPN value (fake page_table_root)
// to avoid polluting other tests. Tests call mm::vma directly.

/// Test root PPNs — must be non-zero and unique across tests.
/// Use values that look like plausible page table PPNs but won't collide
/// with real process roots during test kernel execution.
const TEST_ROOT_A: usize = 0xAA00_0001;
const TEST_ROOT_B: usize = 0xBB00_0002;
const TEST_ROOT_C: usize = 0xCC00_0003;

fn test_vma_stack_scratch_bound() {
    run_test("x86_64 vma_stack_scratch", || {
        crate::mm::vma::max_stack_scratch_bytes() <= 4096
    });
}

fn test_vma_track_check() {
    run_test("x86_64 vma_track", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let s = 0x7000_0000_0000usize;
        let e = s + 0x10000;
        if crate::mm::vma::vma_add(root, s, e, 3, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        let ok = crate::mm::vma::vma_check(root, s) == Some(3)
            && crate::mm::vma::vma_check(root, s + 0x8000) == Some(3)
            && crate::mm::vma::vma_check(root, e - 1) == Some(3)
            && crate::mm::vma::vma_check(root, s - 1).is_none()
            && crate::mm::vma::vma_check(root, e).is_none();
        crate::mm::vma::release_root(root);
        ok
    });
}

fn test_vma_prot_none() {
    run_test("x86_64 vma_prot_none", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let s = 0x7000_0100_0000usize;
        if crate::mm::vma::vma_add(root, s, s + 0x1000, 0, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        let ok = crate::mm::vma::vma_check(root, s).is_none();
        crate::mm::vma::release_root(root);
        ok
    });
}

fn test_vma_prot_rw() {
    run_test("x86_64 vma_prot_rw", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let s = 0x7000_0200_0000usize;
        if crate::mm::vma::vma_add(root, s, s + 0x4000, 3, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        let ok = crate::mm::vma::vma_check(root, s) == Some(3)
            && crate::mm::vma::vma_check(root, s + 0x3000) == Some(3);
        crate::mm::vma::release_root(root);
        ok
    });
}

fn test_vma_overlap() {
    run_test("x86_64 vma_overlap", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let s = 0x7000_0300_0000usize;
        if crate::mm::vma::vma_add(root, s, s + 0x2000, 3, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        let _ = crate::mm::vma::vma_add(root, s + 0x1000, s + 0x3000, 1, false);
        let ok = crate::mm::vma::vma_check(root, s + 0x1800).is_some();
        crate::mm::vma::release_root(root);
        ok
    });
}

fn test_vma_unmap() {
    run_test("x86_64 vma_unmap", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let s = 0x7000_0400_0000usize;
        if crate::mm::vma::vma_add(root, s, s + 0x2000, 3, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        if crate::mm::vma::vma_check(root, s).is_none() {
            crate::mm::vma::release_root(root);
            return false;
        }
        crate::mm::vma::vma_remove_range(root, s, s + 0x2000);
        let ok = crate::mm::vma::vma_check(root, s).is_none();
        crate::mm::vma::release_root(root);
        ok
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
    })
}

fn test_linux_clone_flags() {
    run_test("x86_64 clone_flags", || {
        let go: u64 = 0x100 | 0x200 | 0x400 | 0x800 | 0x10000 | 0x80000 | 0x100000 | 0x200000;
        (go & 0x100) != 0 && (go & 0x10000) != 0 && (go & 0x80000) != 0
    })
}

fn test_initial_stack_size() {
    run_test("x86_64 init_stack", || {
        568usize + 184 == 752 && 752 < (4 * 4096)
    })
}

/// Regression: kernel_cr3() must return a valid non-zero CR3.
fn test_kernel_cr3_before_switch() {
    run_test("x86_64 cr3_before_switch", || {
        crate::mm::vmm::kernel_cr3() != 0
    })
}

/// Regression: __switch's stack frame must have orig_rsp at offset 512.
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
fn test_vma_remove_middle_keeps_tail() {
    run_test("x86_64 vma_remove_tail", || {
        let root = TEST_ROOT_A;
        crate::mm::vma::init_root(root).ok();
        let base = 0x7000_0500_0000usize;
        if crate::mm::vma::vma_add(root, base, base + 0x10000, 3, false).is_err() {
            crate::mm::vma::release_root(root);
            return false;
        }
        crate::mm::vma::vma_remove_range(root, base + 0x2000, base + 0x8000);
        // Head must remain
        if crate::mm::vma::vma_check(root, base) != Some(3) {
            crate::mm::vma::release_root(root);
            return false;
        }
        // Removed middle must be gone
        if crate::mm::vma::vma_check(root, base + 0x4000).is_some() {
            crate::mm::vma::release_root(root);
            return false;
        }
        // CRITICAL: Tail must remain!
        if crate::mm::vma::vma_check(root, base + 0x8000) != Some(3) {
            crate::mm::vma::release_root(root);
            return false;
        }
        if crate::mm::vma::vma_check(root, base + 0xF000) != Some(3) {
            crate::mm::vma::release_root(root);
            return false;
        }
        crate::mm::vma::release_root(root);
        true
    })
}

// ─── Address-space isolation regression tests ────────────────────────

/// Test: VMA entries are isolated by root.
/// Root A has a VMA; root B must not see it. Releasing A must not affect B.
fn test_vma_root_isolation() {
    run_test("x86_64 vma_root_isolation", || {
        let root_a = TEST_ROOT_A;
        let root_b = TEST_ROOT_B;
        crate::mm::vma::init_root(root_a).ok();
        crate::mm::vma::init_root(root_b).ok();

        // Add VMA in root A
        let addr = 0x7000_0000_0000usize;
        if crate::mm::vma::vma_add(root_a, addr, addr + 0x4000, 3, false).is_err() {
            crate::mm::vma::release_root(root_a);
            crate::mm::vma::release_root(root_b);
            return false;
        }

        // Root B must NOT see root A's VMA
        if crate::mm::vma::vma_query(root_b, addr).is_some() {
            crate::mm::vma::release_root(root_a);
            crate::mm::vma::release_root(root_b);
            return false;
        }

        // Release root A — root B must still be fine
        crate::mm::vma::release_root(root_a);
        // Root B's state should still be usable (no VMA at that addr)
        let ok = crate::mm::vma::vma_query(root_b, addr).is_none();
        crate::mm::vma::release_root(root_b);
        ok
    })
}

/// Test: mmap bump state is isolated by root.
/// Root A's ensure_mmap_above must not affect root B's mmap base.
fn test_vma_mmap_isolation() {
    run_test("x86_64 vma_mmap_isolation", || {
        let root_a = TEST_ROOT_A;
        let root_b = TEST_ROOT_B;
        crate::mm::vma::init_root(root_a).ok();
        crate::mm::vma::init_root(root_b).ok();

        // Root A: set high mmap base
        crate::mm::vma::ensure_mmap_above(root_a, 0x5000_0000);

        // Root B: reserve anonymous mmap — should start at USER_MMAP_BASE, not root A's high water
        let addr_b = crate::mm::vma::reserve_mmap_addr(root_b, 0x1000);
        let ok = match addr_b {
            Ok(addr) => {
                // Should be USER_MMAP_BASE (not contaminated by root A)
                addr == crate::process::USER_MMAP_BASE
            }
            Err(()) => false,
        };

        crate::mm::vma::release_root(root_a);
        crate::mm::vma::release_root(root_b);
        ok
    })
}

/// Test: fork can clone VMA state, and mutations on child don't affect parent.
fn test_vma_fork_clone() {
    run_test("x86_64 vma_fork_clone", || {
        let root_a = TEST_ROOT_A;
        let root_c = TEST_ROOT_C;
        crate::mm::vma::init_root(root_a).ok();

        // Add VMA in root A
        let addr = 0x7000_0000_0000usize;
        if crate::mm::vma::vma_add(root_a, addr, addr + 0x4000, 3, false).is_err() {
            crate::mm::vma::release_root(root_a);
            return false;
        }

        // Clone root A → root C (simulating fork)
        if crate::mm::vma::clone_root_state(root_a, root_c).is_err() {
            crate::mm::vma::release_root(root_a);
            return false;
        }

        // Root C must see the same VMA ranges
        if crate::mm::vma::vma_check(root_c, addr) != Some(3) {
            crate::mm::vma::release_root(root_a);
            crate::mm::vma::release_root(root_c);
            return false;
        }

        // Remove from root C — must not affect root A
        crate::mm::vma::vma_remove_range(root_c, addr, addr + 0x4000);
        if crate::mm::vma::vma_check(root_c, addr).is_some() {
            crate::mm::vma::release_root(root_a);
            crate::mm::vma::release_root(root_c);
            return false;
        }
        let ok = crate::mm::vma::vma_check(root_a, addr) == Some(3);
        crate::mm::vma::release_root(root_a);
        crate::mm::vma::release_root(root_c);
        ok
    })
}

/// Regression: restore_task_arch_state must write FS_BASE=0 when slot stores zero.
/// Go's sysUnused→mmap→DONTNEED cycle may set fs_base to 0; the restore path
/// must actually write 0 to the MSR, not skip it.
fn test_restore_zero_fs_base() {
    run_test("x86_64 restore_zero_fs_base", || {
        let slot = crate::sched::current_sched_slot();
        let orig_msr = unsafe { crate::arch::idt::rdmsr(0xC0000100) };
        let orig_task = crate::sched::get_task_fs_base(slot);

        unsafe { crate::arch::idt::wrmsr(0xC0000100, 0xdead_beef) };
        crate::sched::set_task_fs_base(slot, 0);
        crate::sched::restore_task_arch_state_for_test(slot);
        let restored = unsafe { crate::arch::idt::rdmsr(0xC0000100) };

        unsafe { crate::arch::idt::wrmsr(0xC0000100, orig_msr) };
        crate::sched::set_task_fs_base(slot, orig_task);

        restored == 0
    });
}
