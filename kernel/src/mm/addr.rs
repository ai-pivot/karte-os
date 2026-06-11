//! Typed address newtypes for memory safety.
//!
//! Raw `usize` addresses are only allowed at architecture boundaries
//! (assembly, CR3 read/write, PTE encoding, MMIO volatile access, syscall ABI decode).
//! All other code should use these typed wrappers to prevent mixing:
//! - Physical addresses with virtual addresses
//! - User virtual addresses with kernel virtual addresses

use core::fmt;
use core::marker::PhantomData;

// ─── Address space markers ────────────────────────────────────────────

/// Marker: physical address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Phys;

/// Marker: virtual address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Virt;

/// Marker: user virtual address space.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct User;

/// Marker: kernel virtual address space.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Kernel;

// ─── Generic address wrapper ──────────────────────────────────────────

/// A typed address value.
///
/// `Kind` distinguishes physical vs virtual.
/// `Space` distinguishes user vs kernel (for virtual addresses).
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr<Kind, Space = ()> {
    raw: usize,
    _kind: PhantomData<(Kind, Space)>,
}

// ─── Type aliases ─────────────────────────────────────────────────────

pub type PhysAddr = Addr<Phys>;
pub type VirtAddr = Addr<Virt>;
pub type UserVirtAddr = Addr<Virt, User>;
pub type KernelVirtAddr = Addr<Virt, Kernel>;

// ─── Shared methods ───────────────────────────────────────────────────

impl<K, S> Addr<K, S> {
    /// Create without validation. Use only at architecture boundaries.
    #[inline]
    pub const fn new_unchecked(raw: usize) -> Self {
        Self {
            raw,
            _kind: PhantomData,
        }
    }

    /// Extract the raw `usize` value. Use only at architecture boundaries.
    #[inline]
    pub const fn as_usize(self) -> usize {
        self.raw
    }

    /// Whether the address is 4KB page-aligned.
    #[inline]
    pub const fn is_page_aligned(self) -> bool {
        self.raw & 0xfff == 0
    }

    /// Round down to the containing page boundary.
    #[inline]
    pub const fn page_align_down(self) -> Self {
        Self::new_unchecked(self.raw & !0xfff)
    }

    /// Page offset (low 12 bits).
    #[inline]
    pub const fn page_offset(self) -> usize {
        self.raw & 0xfff
    }
}

impl<K, S> fmt::Debug for Addr<K, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.raw)
    }
}

impl<K, S> fmt::Binary for Addr<K, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:b}", self.raw)
    }
}

// ─── Checked constructors ─────────────────────────────────────────────

impl UserVirtAddr {
    /// Create a `UserVirtAddr` if the value is within the user address range.
    /// Returns `None` for kernel-space addresses.
    #[inline]
    pub fn try_new(raw: usize) -> Option<Self> {
        if raw < crate::process::USER_MMAP_LIMIT {
            Some(Self::new_unchecked(raw))
        } else {
            None
        }
    }
}

impl KernelVirtAddr {
    /// Create a `KernelVirtAddr` if the value is in kernel address space.
    /// On x86_64 with identity mapping, this is >= USER_MMAP_LIMIT or
    /// in the canonical high range.
    /// Returns `None` for user-space addresses.
    #[inline]
    pub fn try_new(raw: usize) -> Option<Self> {
        if raw >= crate::process::USER_MMAP_LIMIT {
            Some(Self::new_unchecked(raw))
        } else {
            None
        }
    }
}

impl PhysAddr {
    /// Create a `PhysAddr` without validation.
    /// Physical addresses have no range restriction in the general case.
    #[inline]
    pub const fn new(raw: usize) -> Self {
        Self::new_unchecked(raw)
    }

    /// Physical page number (address >> 12).
    #[inline]
    pub const fn ppn(self) -> usize {
        self.raw >> 12
    }

    /// Create from a physical page number.
    #[inline]
    pub const fn from_ppn(ppn: usize) -> Self {
        Self::new_unchecked(ppn << 12)
    }
}
