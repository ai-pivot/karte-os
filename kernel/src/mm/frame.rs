//! Frame ownership types for physical memory management.
//!
//! `OwnedFrame` — exclusively owned 4KB physical frame, freed on Drop.
//! `PageTableFrame` — a frame used as a page table, freed on Drop.
//! `BorrowedFrame` — non-owning reference to a physical frame.
//!
//! These types make frame ownership explicit. When a frame is stored in a page
//! table entry, the owner must call `into_raw()` to transfer ownership and
//! prevent double-free.

use crate::mm::addr::PhysAddr;

/// An exclusively owned 4KB physical frame.
///
/// Automatically freed via PMM when dropped.
/// Use `into_raw()` when transferring ownership to a page table entry.
#[must_use]
pub struct OwnedFrame {
    addr: PhysAddr,
}

/// A physical frame used as a page table.
///
/// Same ownership semantics as `OwnedFrame` but semantically distinct:
/// page table frames are freed during page table teardown, not by general Drop.
#[must_use]
pub struct PageTableFrame {
    addr: PhysAddr,
}

/// A non-owning reference to a physical frame.
///
/// No Drop impl — the caller is responsible for ensuring the frame is freed
/// by the actual owner.
#[derive(Clone, Copy)]
pub struct BorrowedFrame {
    addr: PhysAddr,
}

// ─── OwnedFrame ───────────────────────────────────────────────────────

impl OwnedFrame {
    /// Create from a raw physical address. The caller must ensure the frame
    /// was freshly allocated and exclusively owned.
    #[inline]
    pub fn from_raw(addr: PhysAddr) -> Self {
        Self { addr }
    }

    /// The physical address of this frame.
    #[inline]
    pub fn addr(&self) -> PhysAddr {
        self.addr
    }

    /// Consume without freeing. Use when storing the frame in a page table entry.
    /// After this call, the caller is responsible for eventual deallocation.
    #[inline]
    pub fn into_raw(self) -> PhysAddr {
        let addr = self.addr;
        core::mem::forget(self);
        addr
    }
}

impl Drop for OwnedFrame {
    fn drop(&mut self) {
        crate::mm::pmm::dealloc_frame(self.addr.as_usize());
    }
}

// ─── PageTableFrame ───────────────────────────────────────────────────

impl PageTableFrame {
    /// Create from a raw physical address. The caller must ensure the frame
    /// was freshly allocated and is being used as a page table.
    #[inline]
    pub fn from_raw(addr: PhysAddr) -> Self {
        Self { addr }
    }

    /// The physical address of this frame.
    #[inline]
    pub fn addr(&self) -> PhysAddr {
        self.addr
    }

    /// Consume without freeing. Use when the page table frame is now owned
    /// by the page table hierarchy (freed by `free_user_page_table`).
    #[inline]
    pub fn into_raw(self) -> PhysAddr {
        let addr = self.addr;
        core::mem::forget(self);
        addr
    }

    /// Interpret this frame as a mutable page table reference.
    ///
    /// # Safety
    /// The frame must contain valid page table data (or be zeroed).
    /// The caller must ensure no aliasing mutable references exist.
    #[inline]
    pub unsafe fn as_page_table_mut(&self) -> &'static mut crate::mm::vmm::PageTable {
        &mut *(crate::mm::vmm::phys_to_virt(self.addr.as_usize()) as *mut crate::mm::vmm::PageTable)
    }
}

impl Drop for PageTableFrame {
    fn drop(&mut self) {
        crate::mm::pmm::dealloc_frame(self.addr.as_usize());
    }
}

// ─── BorrowedFrame ────────────────────────────────────────────────────

impl BorrowedFrame {
    /// Create from a raw physical address. No ownership transfer.
    #[inline]
    pub const fn from_raw(addr: PhysAddr) -> Self {
        Self { addr }
    }

    /// The physical address of this frame.
    #[inline]
    pub const fn addr(&self) -> PhysAddr {
        self.addr
    }
}

// ─── Allocation wrappers ──────────────────────────────────────────────
// These wrap the existing PMM alloc_frame() into typed results.

/// Allocate a new `OwnedFrame`.
/// Returns `None` if PMM has no free frames.
pub fn alloc_owned_frame() -> Option<OwnedFrame> {
    let raw = crate::mm::pmm::alloc_frame()?;
    Some(OwnedFrame::from_raw(PhysAddr::new(raw)))
}

/// Allocate a new `PageTableFrame`.
/// Returns `None` if PMM has no free frames.
pub fn alloc_page_table_frame() -> Option<PageTableFrame> {
    let raw = crate::mm::pmm::alloc_frame()?;
    Some(PageTableFrame::from_raw(PhysAddr::new(raw)))
}
