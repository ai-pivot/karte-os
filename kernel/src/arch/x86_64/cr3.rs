//! RAII CR3 guards for safe page table switching.
//!
//! `Cr3Guard` ensures CR3 is always restored, even on early returns or panics.
//! `with_kernel_cr3()` closures should be replaced with `enter_kernel_cr3()` guards.

use core::marker::PhantomData;

/// RAII guard that restores the previous CR3 value on drop.
///
/// Created by `enter_kernel_cr3()`. Must not be sent across harts
/// (CR3 is per-CPU state).
#[must_use]
pub struct Cr3Guard {
    previous_cr3: usize,
    _not_send: PhantomData<*mut ()>,
}

impl Cr3Guard {
    /// The CR3 value that was active before this guard was created.
    pub fn previous_cr3(&self) -> usize {
        self.previous_cr3
    }
}

impl Drop for Cr3Guard {
    fn drop(&mut self) {
        let current = read_cr3_raw();
        if current != self.previous_cr3 {
            unsafe { write_cr3_raw(self.previous_cr3) };
            flush_tlb();
        }
    }
}

/// Read the current CR3 value as a raw usize.
fn read_cr3_raw() -> usize {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) };
    cr3 as usize
}

/// Write a raw CR3 value (unsafe: must be page-aligned and valid).
unsafe fn write_cr3_raw(cr3: usize) {
    core::arch::asm!("mov cr3, {}", in(reg) cr3 as u64)
}

/// Flush the TLB by reloading CR3.
fn flush_tlb() {
    unsafe { write_cr3_raw(read_cr3_raw()) }
}

/// Get the kernel CR3 physical address.
fn kernel_cr3_raw() -> usize {
    let cached = crate::arch::idt::get_kernel_cr3_phys();
    if cached != 0 {
        cached
    } else {
        crate::mm::vmm::kernel_cr3() as usize
    }
}

/// Switch to kernel CR3, returning a guard that restores the previous CR3 on drop.
///
/// Usage:
/// ```ignore
/// let _cr3 = crate::arch::cr3::enter_kernel_cr3();
/// // ... do work under kernel CR3 ...
/// // _cr3 dropped here, restores previous CR3
/// ```
pub fn enter_kernel_cr3() -> Cr3Guard {
    let previous = read_cr3_raw();
    let kernel = kernel_cr3_raw();
    if previous != kernel {
        unsafe { write_cr3_raw(kernel) };
    }
    Cr3Guard {
        previous_cr3: previous,
        _not_send: PhantomData,
    }
}

/// Switch to a specific CR3 value, returning a guard that restores the previous CR3 on drop.
///
/// This is lower-level than `enter_kernel_cr3()` and should only be used when
/// switching to a specific non-kernel page table is required (e.g., user memory access).
pub fn enter_cr3(target: usize) -> Cr3Guard {
    let previous = read_cr3_raw();
    if previous != target {
        unsafe { write_cr3_raw(target) };
    }
    Cr3Guard {
        previous_cr3: previous,
        _not_send: PhantomData,
    }
}
