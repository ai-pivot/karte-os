// kernel/src/mm/vmm.rs — Sv39 Virtual Memory Manager

use bitflags::bitflags;
use riscv::register::satp;

use super::pmm;

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlags: u64 {
        const V = 1 << 0;       // Valid
        const R = 1 << 1;       // Read
        const W = 1 << 2;       // Write
        const X = 1 << 3;       // Execute
        const U = 1 << 4;       // User
        const G = 1 << 5;       // Global
        const A = 1 << 6;       // Accessed
        const D = 1 << 7;       // Dirty

        // Common combinations
        const KRWX = Self::V.bits() | Self::R.bits() | Self::W.bits() | Self::X.bits();
        const KRW  = Self::V.bits() | Self::R.bits() | Self::W.bits();
        const KRX  = Self::V.bits() | Self::R.bits() | Self::X.bits();
        const URWX = Self::KRWX.bits() | Self::U.bits();
        const URW  = Self::KRW.bits() | Self::U.bits();
    }
}

const PAGE_SIZE: usize = 4096;
const PTE_COUNT: usize = 512;
const VPN_BITS: usize = 9;
const VPN_MASK: usize = (1 << VPN_BITS) - 1;

/// Page Table Entry
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PTE(pub u64);

impl PTE {
    pub fn new(ppn: usize, flags: PTEFlags) -> Self {
        Self(((ppn as u64) << 10) | flags.bits())
    }

    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0)
    }

    pub fn ppn(&self) -> usize {
        (self.0 >> 10) as usize
    }

    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }

    pub fn is_leaf(&self) -> bool {
        let f = self.flags();
        f.contains(PTEFlags::R) || f.contains(PTEFlags::W) || f.contains(PTEFlags::X)
    }
}

/// Page Table (512 entries)
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PTE; PTE_COUNT],
}

impl PageTable {
    pub fn zeroed() -> &'static mut Self {
        let frame = pmm::alloc_frame().expect("Failed to allocate page table");
        let table = unsafe { &mut *(frame as *mut Self) };
        for entry in table.entries.iter_mut() {
            *entry = PTE(0);
        }
        table
    }

    fn vpn(vaddr: usize, level: usize) -> usize {
        (vaddr >> (12 + VPN_BITS * level)) & VPN_MASK
    }
}

/// Map a virtual address to a physical address with given flags
pub fn map(root: &mut PageTable, vaddr: usize, paddr: usize, flags: PTEFlags) {
    let mut table = root;

    for level in (1..=2).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = &mut table.entries[vpn];

        if !entry.is_valid() {
            // Allocate new page table
            let new_table = PageTable::zeroed();
            let ppn = (new_table as *const PageTable as usize) >> 12;
            *entry = PTE::new(ppn, PTEFlags::V);
        }

        let ppn = entry.ppn();
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }

    // Level 0: leaf entry
    let vpn = PageTable::vpn(vaddr, 0);
    let ppn = paddr >> 12;
    table.entries[vpn] = PTE::new(ppn, flags);
}

/// Identity map a range of physical memory
pub fn identity_map(root: &mut PageTable, start: usize, end: usize, flags: PTEFlags) {
    let start_page = start & !(PAGE_SIZE - 1);
    let end_page = (end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let mut addr = start_page;
    while addr < end_page {
        map(root, addr, addr, flags);
        addr += PAGE_SIZE;
    }
}

/// The kernel page table root (set during init)
static mut KERNEL_PAGE_TABLE: *mut PageTable = core::ptr::null_mut();

/// Get a reference to the kernel page table.
/// Safe to call after vmm::init().
pub fn get_kernel_page_table() -> &'static mut PageTable {
    unsafe { &mut *KERNEL_PAGE_TABLE }
}

/// Initialize virtual memory with identity mapping
pub fn init() {
    let root = PageTable::zeroed();
    let root_addr = root as *const PageTable as usize;

    // Store kernel page table
    unsafe {
        KERNEL_PAGE_TABLE = root;
    }

    // Identity map kernel (0x80200000 .. 0x80200000 + 128MB)
    let start = 0x8020_0000;
    let end = start + 128 * 1024 * 1024;
    identity_map(root, start, end, PTEFlags::KRWX);

    // Map UART MMIO (0x10000000 - 0x10001000)
    map(root, 0x1000_0000, 0x1000_0000, PTEFlags::KRW);

    // Map VirtIO MMIO devices (0x10001000 - 0x10009000)
    // 8 devices at 0x1000 stride, each occupying a full page
    for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
        map(root, addr, addr, PTEFlags::KRW);
    }

    // Map PLIC (0x0C000000 - 0x0C400000) - needs multiple pages
    // Priority, enable, pending, threshold, claim/complete registers
    for addr in (0x0C00_0000..0x0C40_0000).step_by(PAGE_SIZE) {
        map(root, addr, addr, PTEFlags::KRW);
    }

    // Activate page table
    let ppn = root_addr >> 12;
    unsafe {
        satp::set(satp::Mode::Sv39, 0, ppn);
        // Flush TLB
        core::arch::asm!("sfence.vma");
    }

    crate::console_println!("[vmm] Sv39 page table activated at {:#x}", root_addr);
}

// ── User address space support ──────────────────────────────────────────

/// Create a new empty user page table.
/// Returns a mutable reference to the root page table.
/// The page table is independent from the kernel page table.
pub fn create_user_page_table() -> &'static mut PageTable {
    let root = PageTable::zeroed();
    // The root page table itself is a physical frame.
    // We return a reference to it.
    root
}

/// Map a page in a user page table (with user-accessible flag).
pub fn map_user(root: &mut PageTable, vaddr: usize, paddr: usize, flags: PTEFlags) {
    map(root, vaddr, paddr, flags)
}

/// Translate a virtual address in a user page table to physical.
/// Returns None if not mapped.
pub fn translate_user(root: &mut PageTable, vaddr: usize) -> Option<usize> {
    let vpn2 = (vaddr >> 30) & 0x1FF;
    let vpn1 = (vaddr >> 21) & 0x1FF;
    let vpn0 = (vaddr >> 12) & 0x1FF;

    let l2 = &root.entries[vpn2];
    if !l2.is_valid() {
        return None;
    }
    let l1_table = unsafe { &mut *((l2.ppn() << 12) as *mut PageTable) };
    let l1 = &l1_table.entries[vpn1];
    if !l1.is_valid() {
        return None;
    }

    // Check if L1 is a leaf (mega page)
    if l1.is_leaf() {
        return Some((l1.ppn() << 12) | (vaddr & 0x1FFFFF));
    }

    let l0_table = unsafe { &mut *((l1.ppn() << 12) as *mut PageTable) };
    let l0 = &l0_table.entries[vpn0];
    if !l0.is_valid() {
        return None;
    }

    Some((l0.ppn() << 12) | (vaddr & 0xFFF))
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── VMM Tests ──");

    // Test 1: Create empty page table
    crate::test::run_test("vmm_create_page_table", || {
        let pt = PageTable::zeroed();
        let addr = pt as *const PageTable as usize;
        // Should be page-aligned
        addr % 4096 == 0
    });

    // Test 2: Map a single page
    crate::test::run_test("vmm_map_single_page", || {
        let root = PageTable::zeroed();
        map(root, 0x8040_0000, 0x8040_0000, PTEFlags::KRW);
        // Check that L0 entry exists and is valid
        let vpn2 = (0x8040_0000 >> 30) & 0x1FF;
        let vpn1 = (0x8040_0000 >> 21) & 0x1FF;
        let vpn0 = (0x8040_0000 >> 12) & 0x1FF;

        let l2_entry = root.entries[vpn2];
        if !l2_entry.is_valid() {
            return false;
        }

        let l1_table = unsafe { &*((l2_entry.ppn() << 12) as *const PageTable) };
        let l1_entry = l1_table.entries[vpn1];
        if !l1_entry.is_valid() {
            return false;
        }

        let l0_table = unsafe { &*((l1_entry.ppn() << 12) as *const PageTable) };
        let l0_entry = l0_table.entries[vpn0];

        l0_entry.is_valid() && l0_entry.ppn() == (0x8040_0000 >> 12)
    });

    // Test 3: Identity map range
    crate::test::run_test("vmm_identity_map_range", || {
        let root = PageTable::zeroed();
        identity_map(root, 0x8050_0000, 0x8050_3000, PTEFlags::KRWX);

        // Check first and last pages
        for addr in [0x8050_0000, 0x8050_1000, 0x8050_2000] {
            let vpn2 = (addr >> 30) & 0x1FF;
            let vpn1 = (addr >> 21) & 0x1FF;
            let vpn0 = (addr >> 12) & 0x1FF;

            let l2 = root.entries[vpn2];
            if !l2.is_valid() {
                return false;
            }
            let l1t = unsafe { &*((l2.ppn() << 12) as *const PageTable) };
            let l1 = l1t.entries[vpn1];
            if !l1.is_valid() {
                return false;
            }
            let l0t = unsafe { &*((l1.ppn() << 12) as *const PageTable) };
            let l0 = l0t.entries[vpn0];
            if !l0.is_valid() || l0.ppn() != (addr >> 12) {
                return false;
            }
        }
        true
    });

    // Test 4: PTE flags are correct
    crate::test::run_test("vmm_pte_flags", || {
        let pte = PTE::new(0x12345, PTEFlags::KRWX);
        let flags = pte.flags();
        flags.contains(PTEFlags::V)
            && flags.contains(PTEFlags::R)
            && flags.contains(PTEFlags::W)
            && flags.contains(PTEFlags::X)
            && !flags.contains(PTEFlags::U)
    });

    // Test 5: PTE PPN extraction
    crate::test::run_test("vmm_pte_ppn", || {
        let pte = PTE::new(0xABCD, PTEFlags::KRW);
        pte.ppn() == 0xABCD
    });

    // Test 6: PTE is_leaf detection
    crate::test::run_test("vmm_pte_leaf_detection", || {
        let leaf = PTE::new(0x100, PTEFlags::KRW);
        let non_leaf = PTE::new(0x200, PTEFlags::V);
        leaf.is_leaf() && !non_leaf.is_leaf()
    });
}
