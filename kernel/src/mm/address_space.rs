//! Per-process address space management.
//!
//! Each process owns an `AddressSpace` that bundles:
//! - The page table root (`PageTableRoot`)
//! - VMA state (virtual memory areas)
//!
//! This replaces raw `usize` page_table_root with a typed wrapper and
//! moves VMA operations from global state to per-address-space methods.

use crate::mm::addr::PhysAddr;

/// Typed page table root — the physical address of the top-level page table.
///
/// This is the value loaded into CR3 (x86_64) or satp (RISC-V).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageTableRoot(PhysAddr);

impl PageTableRoot {
    /// Create from a raw physical address.
    /// Must be page-aligned and point to a valid page table.
    pub const fn new(raw: usize) -> Self {
        Self(PhysAddr::new_unchecked(raw))
    }

    /// The raw physical address.
    pub const fn as_usize(self) -> usize {
        self.0.as_usize()
    }

    /// Physical page number (for CR3/satp loading).
    pub const fn ppn(self) -> usize {
        self.0.ppn()
    }

    /// Whether this is a null/invalid root.
    pub const fn is_null(self) -> bool {
        self.0.as_usize() == 0
    }
}

/// Handle to an address space.
///
/// Wraps a `PageTableRoot` and provides access to VMA state.
/// For CLONE_VM threads, multiple processes share the same `AddressSpaceHandle`.
/// For fork/exec, each gets its own handle.
///
/// Currently delegates to the global VMA registry in `mm::vma`.
/// Future work will embed VMA state directly.
#[derive(Clone, Copy)]
pub struct AddressSpaceHandle {
    root: PageTableRoot,
}

impl AddressSpaceHandle {
    /// Create a handle for the given page table root.
    /// Also initializes VMA state for this root.
    pub fn init(root: PageTableRoot) -> Result<Self, ()> {
        crate::mm::vma::init_root(root.as_usize())?;
        Ok(Self { root })
    }

    /// Create a handle without initializing VMA state (for existing roots).
    pub fn from_existing(root: PageTableRoot) -> Self {
        Self { root }
    }

    /// The page table root.
    pub fn root(&self) -> PageTableRoot {
        self.root
    }

    /// Check if an address falls within a VMA with the given protection.
    /// Returns Some(prot) if accessible, None if unmapped or PROT_NONE.
    pub fn vma_check(&self, addr: usize) -> Option<usize> {
        crate::mm::vma::vma_check(self.root.as_usize(), addr)
    }

    /// Query detailed VMA info for an address.
    pub fn vma_query(&self, addr: usize) -> Option<usize> {
        crate::mm::vma::vma_query(self.root.as_usize(), addr)
    }

    /// Add a VMA region.
    pub fn vma_add(&self, start: usize, end: usize, prot: usize, is_elf: bool) -> Result<(), ()> {
        crate::mm::vma::vma_add(self.root.as_usize(), start, end, prot, is_elf)
    }

    /// Remove VMA regions in [start, end).
    pub fn vma_remove_range(&self, start: usize, end: usize) {
        crate::mm::vma::vma_remove_range(self.root.as_usize(), start, end)
    }

    /// Update VMA protection for regions in [start, end).
    pub fn vma_update_prot(&self, start: usize, end: usize, new_prot: usize) {
        crate::mm::vma::vma_update_prot(self.root.as_usize(), start, end, new_prot)
    }

    /// Set the mmap base address if higher than current.
    pub fn ensure_mmap_above(&self, addr: usize) {
        crate::mm::vma::ensure_mmap_above(self.root.as_usize(), addr)
    }

    /// Reserve an mmap region of the given size.
    pub fn reserve_mmap_addr(&self, size: usize) -> Result<usize, ()> {
        crate::mm::vma::reserve_mmap_addr(self.root.as_usize(), size)
    }

    /// Release all VMA state for this address space.
    pub fn release(&self) {
        crate::mm::vma::release_root(self.root.as_usize())
    }

    /// Clone VMA state from another address space (for fork).
    pub fn clone_from(&self, src: &AddressSpaceHandle) -> Result<(), ()> {
        crate::mm::vma::clone_root_state(src.root.as_usize(), self.root.as_usize())
    }

    /// Register an ELF VMA range.
    pub fn register_elf_vma(&self, start: usize, end: usize, prot: usize) {
        crate::mm::vma::register_elf_vma(self.root.as_usize(), start, end, prot)
    }
}
