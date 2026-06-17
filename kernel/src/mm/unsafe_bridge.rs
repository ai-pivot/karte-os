//! Quarantined unsafe operations for memory management.
//!
//! All unsafe memory operations that don't fit in a specific typed wrapper
//! are centralized here. Each function has a safety comment explaining
//! the invariant that makes the unsafe call sound.
//!
//! # Audit rule
//!
//! Every `unsafe` block in `mm/` outside this file must have a comment
//! referencing a function in this file or explaining why it's safe inline.
//! No bare `unsafe { ... }` without justification.

use crate::mm::addr::PhysAddr;
use crate::mm::vmm::PageTable;

/// Convert a physical frame address to a mutable page table reference.
///
/// # Safety
/// The caller must ensure:
/// 1. `frame_addr` is page-aligned and points to a valid `PageTable` struct
/// 2. No other mutable reference to this page table exists
/// 3. The frame is not freed while the reference is held
#[inline(always)]
pub unsafe fn frame_to_page_table_mut(frame_addr: PhysAddr) -> &'static mut PageTable {
    &mut *(crate::mm::vmm::phys_to_virt(frame_addr.as_usize()) as *mut PageTable)
}

/// Convert a physical frame address to a page table reference (read-only).
///
/// # Safety
/// The caller must ensure:
/// 1. `frame_addr` is page-aligned and points to a valid `PageTable` struct
/// 2. The frame is not freed while the reference is held
#[inline(always)]
pub unsafe fn frame_to_page_table(frame_addr: PhysAddr) -> &'static PageTable {
    &*(crate::mm::vmm::phys_to_virt(frame_addr.as_usize()) as *const PageTable)
}

/// Write to a user-space virtual address under user CR3 (x86_64).
///
/// # Safety
/// The caller must ensure:
/// 1. The kernel is currently running under the correct CR3 (user or kernel)
/// 2. `addr` is a valid user-space address
/// 3. The page at `addr` is mapped and writable
#[inline(always)]
pub unsafe fn write_volatile_usize(addr: usize, val: usize) {
    core::ptr::write_volatile(addr as *mut usize, val);
}

/// Read from a user-space virtual address under user CR3 (x86_64).
///
/// # Safety
/// The caller must ensure:
/// 1. The kernel is currently running under the correct CR3 (user or kernel)
/// 2. `addr` is a valid user-space address
/// 3. The page at `addr` is mapped and readable
#[inline(always)]
pub unsafe fn read_volatile_usize(addr: usize) -> usize {
    core::ptr::read_volatile(addr as *const usize)
}

/// Read a byte from a volatile address.
///
/// # Safety
/// The caller must ensure the address is valid and mapped.
#[inline(always)]
pub unsafe fn read_volatile_u8(addr: usize) -> u8 {
    core::ptr::read_volatile(addr as *const u8)
}

/// Write a byte to a volatile address.
///
/// # Safety
/// The caller must ensure the address is valid, mapped, and writable.
#[inline(always)]
pub unsafe fn write_volatile_u8(addr: usize, val: u8) {
    core::ptr::write_volatile(addr as *mut u8, val);
}

/// Zero a page table frame.
///
/// # Safety
/// The caller must ensure `frame_addr` points to a valid 4KB frame
/// and no other reference to it exists.
#[inline(always)]
pub unsafe fn zero_page_table(frame_addr: PhysAddr) {
    core::ptr::write_bytes(frame_addr.as_usize() as *mut u8, 0, 4096);
}

/// Copy bytes from kernel memory to user memory.
///
/// # Safety
/// The caller must ensure `dst` is a valid writable user address and
/// the kernel CR3 allows access (via with_user_cr3 or under user CR3).
#[inline(always)]
pub unsafe fn copy_to_user(dst: usize, src: *const u8, len: usize) {
    core::ptr::copy_nonoverlapping(src, dst as *mut u8, len);
}

/// Copy bytes from user memory to kernel memory.
///
/// # Safety
/// The caller must ensure `src` is a valid readable user address and
/// the kernel CR3 allows access (via with_user_cr3 or under user CR3).
#[inline(always)]
pub unsafe fn copy_from_user(dst: *mut u8, src: usize, len: usize) {
    core::ptr::copy_nonoverlapping(src as *const u8, dst, len);
}
