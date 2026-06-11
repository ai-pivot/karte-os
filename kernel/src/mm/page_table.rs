//! Typed page-table level markers and walk results.
//!
//! Page-table walks return typed `WalkResult` values that distinguish
//! between 4K mappings, huge-page mappings, unmapped entries, and invalid
//! entries. This prevents `mprotect` and `munmap` from silently descending
//! into huge pages as if they were page tables.

use crate::mm::addr::PhysAddr;

// ─── Page-table level markers ─────────────────────────────────────────

/// Trait for page-table level indexing.
/// Levels are numbered from L4 (root, index 3) down to L1 (leaf parent, index 0).
pub trait Level: sealed::Sealed {
    /// VPN index shift within the page-table hierarchy.
    const INDEX: usize;
}

/// Level 4 — PML4 root (x86_64 only, index 3).
pub enum L4 {}
/// Level 3 — PDP (x86_64) / root (RISC-V Sv39), index 2.
pub enum L3 {}
/// Level 2 — PD (x86_64) / L1 (RISC-V Sv39), index 1.
pub enum L2 {}
/// Level 1 — PT (x86_64) / L0 (RISC-V Sv39), index 0.
pub enum L1 {}

impl Level for L4 {
    const INDEX: usize = 3;
}
impl Level for L3 {
    const INDEX: usize = 2;
}
impl Level for L2 {
    const INDEX: usize = 1;
}
impl Level for L1 {
    const INDEX: usize = 0;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::L4 {}
    impl Sealed for super::L3 {}
    impl Sealed for super::L2 {}
    impl Sealed for super::L1 {}
}

// ─── Walk result ──────────────────────────────────────────────────────

/// Result of a page-table walk for a virtual address.
///
/// This replaces raw `Option<usize>` from `translate_user()` with explicit
/// handling of huge pages. Code that uses `WalkResult` must handle
/// `MappedHuge` explicitly — it cannot silently descend into a huge page.
#[derive(Debug, Clone, Copy)]
pub enum WalkResult {
    /// No mapping exists at this address (entry not present).
    NotMapped,
    /// 4KB leaf mapping at PT level.
    Mapped4K {
        /// Physical frame address (page-aligned).
        frame: PhysAddr,
        /// Raw PTE flags.
        flags: u64,
    },
    /// Huge-page mapping (2MB at PD level, 1GB at PDP level).
    MappedHuge {
        /// Physical frame address (huge-page aligned).
        frame: PhysAddr,
        /// Page-table level where the huge page was found (1 = PD/2MB, 2 = PDP/1GB).
        level: usize,
        /// Size of the huge page in bytes.
        size: usize,
        /// Raw PTE flags.
        flags: u64,
    },
    /// Entry is present but corrupted (e.g., ppn=0 with PRESENT set).
    Invalid,
}

impl WalkResult {
    /// Whether this result represents a usable mapping (4K or huge).
    pub fn is_mapped(&self) -> bool {
        matches!(self, WalkResult::Mapped4K { .. } | WalkResult::MappedHuge { .. })
    }

    /// Extract the physical frame address if mapped.
    pub fn frame(&self) -> Option<PhysAddr> {
        match self {
            WalkResult::Mapped4K { frame, .. } => Some(*frame),
            WalkResult::MappedHuge { frame, .. } => Some(*frame),
            _ => None,
        }
    }
}
