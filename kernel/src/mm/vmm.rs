// kernel/src/mm/vmm.rs — Virtual Memory Manager
//
// Architecture-independent page table operations.
// PTE flags are mapped differently per architecture:
//   RISC-V Sv39: V=R/W/X/U/G/A/D at bits 0-7
//   x86_64 PML4: Present(0), R/W(1), U/S(2), PWT(3), PCD(4), ACCESSED(5), DIRTY(6), PS(7), GLOBAL(8), NX(63)

use bitflags::bitflags;
#[cfg(target_arch = "riscv64")]
use riscv::register::satp;

use super::pmm;

// ─── PTE Flags (architecture-specific bit assignments) ─────────────────

#[cfg(target_arch = "riscv64")]
bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlags: u64 {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;

        const KRWX = Self::V.bits() | Self::R.bits() | Self::W.bits() | Self::X.bits();
        const KRW  = Self::V.bits() | Self::R.bits() | Self::W.bits();
        const KRX  = Self::V.bits() | Self::R.bits() | Self::X.bits();
        const URWX = Self::KRWX.bits() | Self::U.bits();
        const URW  = Self::KRW.bits() | Self::U.bits();
        const UX   = Self::V.bits() | Self::X.bits() | Self::U.bits();
        const UR   = Self::V.bits() | Self::R.bits() | Self::U.bits();
    }
}

#[cfg(target_arch = "x86_64")]
bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct PTEFlags: u64 {
        const PRESENT  = 1 << 0;    // P
        const WRITABLE = 1 << 1;    // R/W
        const USER     = 1 << 2;    // U/S
        const PWT      = 1 << 3;    // Page Write Through
        const PCD      = 1 << 4;    // Page Cache Disable
        const ACCESSED = 1 << 5;    // A
        const DIRTY    = 1 << 6;    // D
        const PS       = 1 << 7;    // Page Size (huge page)
        const GLOBAL   = 1 << 8;    // G
        const NX       = 1 << 63;   // No Execute

        // Common combinations
        const KRWX = Self::PRESENT.bits() | Self::WRITABLE.bits();
        const KRW  = Self::PRESENT.bits() | Self::WRITABLE.bits();
        const KRX  = Self::PRESENT.bits(); // executable by default (no NX)
        const URWX = Self::PRESENT.bits() | Self::WRITABLE.bits() | Self::USER.bits();
        const URW  = Self::PRESENT.bits() | Self::WRITABLE.bits() | Self::USER.bits();
        const UR   = Self::PRESENT.bits() | Self::USER.bits(); // read-only user
        const UX   = Self::PRESENT.bits() | Self::USER.bits(); // execute only user (no NX)
    }
}

const PAGE_SIZE: usize = 4096;
const PTE_COUNT: usize = 512;

#[cfg(target_arch = "riscv64")]
const PT_LEVELS: usize = 3;
#[cfg(target_arch = "x86_64")]
const PT_LEVELS: usize = 4;

/// Page Table Entry — arch-specific encoding
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PTE(pub u64);

impl PTE {
    /// Create a PTE from physical page number and flags.
    #[cfg(target_arch = "riscv64")]
    pub fn new(ppn: usize, flags: PTEFlags) -> Self {
        Self(((ppn as u64) << 10) | flags.bits())
    }

    #[cfg(target_arch = "x86_64")]
    pub fn new(ppn: usize, flags: PTEFlags) -> Self {
        // x86_64: PTE = physical_address[51:12] << 12 | flags
        // For non-leaf entries, WRITABLE should be set so child entries can be modified
        let mut f = flags;
        // Ensure non-leaf entries are writable (needed for page table updates)
        f.insert(PTEFlags::WRITABLE);
        Self(((ppn as u64) << 12) | f.bits())
    }

    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits_truncate(self.0)
    }

    #[cfg(target_arch = "riscv64")]
    pub fn ppn(&self) -> usize {
        (self.0 >> 10) as usize
    }

    #[cfg(target_arch = "x86_64")]
    pub fn ppn(&self) -> usize {
        ((self.0 >> 12) & 0xF_FFFF_FFFF_F) as usize
    }

    pub fn is_valid(&self) -> bool {
        #[cfg(target_arch = "riscv64")]
        {
            self.flags().contains(PTEFlags::V)
        }
        #[cfg(target_arch = "x86_64")]
        {
            self.flags().contains(PTEFlags::PRESENT)
        }
    }

    pub fn is_leaf(&self) -> bool {
        #[cfg(target_arch = "riscv64")]
        {
            let f = self.flags();
            f.contains(PTEFlags::R) || f.contains(PTEFlags::W) || f.contains(PTEFlags::X)
        }
        #[cfg(target_arch = "x86_64")]
        {
            // x86_64: leaf if PS bit set, OR if at level 0 (PT level, always leaf)
            self.flags().contains(PTEFlags::PS) || self.is_valid()
            // Note: at PT level (level 0), all valid entries are leaf entries
            // We handle this in map() by not checking is_leaf at level 0
        }
    }
}

/// Page Table (512 entries, page-aligned)
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PTE; PTE_COUNT],
}

impl PageTable {
    pub fn zeroed() -> &'static mut Self {
        let frame = pmm::alloc_frame().expect("VMM: no frames for page table");
        let table = unsafe { &mut *(frame as *mut Self) };
        for entry in table.entries.iter_mut() {
            *entry = PTE(0);
        }
        table
    }

    #[cfg(target_arch = "riscv64")]
    fn vpn(vaddr: usize, level: usize) -> usize {
        ((vaddr >> (12 + 9 * level)) & 0x1FF)
    }

    #[cfg(target_arch = "x86_64")]
    fn vpn(vaddr: usize, level: usize) -> usize {
        ((vaddr >> (12 + 9 * level)) & 0x1FF)
    }

    pub fn entry(&self, idx: usize) -> PTE {
        self.entries[idx]
    }

    pub fn set_entry(&mut self, idx: usize, pte: PTE) {
        self.entries[idx] = pte;
    }
}

/// Map a virtual address to a physical address with given flags.
pub fn map(root: &mut PageTable, vaddr: usize, paddr: usize, flags: PTEFlags) {
    let mut table = root;

    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = &mut table.entries[vpn];

        if !entry.is_valid() {
            let new_table = PageTable::zeroed();
            let ppn = (new_table as *const PageTable as usize) >> 12;

            #[cfg(target_arch = "riscv64")]
            {
                *entry = PTE::new(ppn, PTEFlags::V);
            }
            #[cfg(target_arch = "x86_64")]
            {
                // x86_64 non-leaf: Present + Writable + User
                *entry = PTE(((ppn as u64) << 12)
                    | PTEFlags::PRESENT.bits()
                    | PTEFlags::WRITABLE.bits()
                    | PTEFlags::USER.bits());
            }
        }

        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            // Huge page at this level — must split into smaller entries.
            // This happens when identity-mapped 2MB huge pages conflict
            // with mmap's 4KB granularity. Allocate a new page table and
            // populate it with entries derived from the huge page.
            let huge_ppn = entry.ppn();
            let huge_paddr = huge_ppn << 12;
            let new_table = PageTable::zeroed();
            let new_table_ppn = (new_table as *const PageTable as usize) >> 12;
            let sub_page_size = 1 << (12 + 9 * (level - 1)); // 4KB for level 1, 2MB for level 2
            let num_entries = 512;
            for i in 0..num_entries {
                let sub_paddr = huge_paddr + i * sub_page_size;
                let sub_ppn = sub_paddr >> 12;
                #[cfg(target_arch = "x86_64")]
                {
                    new_table.entries[i] = PTE(
                        ((sub_ppn as u64) << 12)
                            | PTEFlags::PRESENT.bits()
                            | PTEFlags::WRITABLE.bits()
                            | PTEFlags::USER.bits(),
                    );
                }
            }
            // Replace the huge page entry with a pointer to the new page table
            let new_flags = PTEFlags::PRESENT.bits() | PTEFlags::WRITABLE.bits() | PTEFlags::USER.bits();
            *entry = PTE(((new_table_ppn as u64) << 12) | new_flags);
            // Continue traversal with the new table
            table = unsafe { &mut *((new_table_ppn << 12) as *mut PageTable) };
            continue;
        }

        let ppn = entry.ppn();
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };

        // x86_64: ensure non-leaf entries have User bit for Ring 3 page walks.
        // identity_map_skip creates entries without User bit; we must add it
        // when mapping user-accessible pages (e.g., user stack).
        #[cfg(target_arch = "x86_64")]
        {
            let raw = entry.0;
            if (raw & PTEFlags::USER.bits()) == 0 && flags.contains(PTEFlags::USER) {
                entry.0 = raw | PTEFlags::USER.bits();
            }
        }
    }

    // Level 0: leaf entry
    let vpn = PageTable::vpn(vaddr, 0);
    let ppn = paddr >> 12;
    #[cfg(target_arch = "riscv64")]
    {
        table.entries[vpn] = PTE::new(ppn, flags);
    }
    #[cfg(target_arch = "x86_64")]
    {
        table.entries[vpn] = PTE(((ppn as u64) << 12) | flags.bits());
    }
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

/// Identity map, but skip pages that are already mapped (preserves user ELF mappings).
#[cfg(target_arch = "x86_64")]
pub fn identity_map_skip(root: &mut PageTable, start: usize, end: usize, flags: PTEFlags) {
    let start_page = start & !(PAGE_SIZE - 1);
    let end_page = (end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let mut addr = start_page;
    let mut skip_count = 0;
    let mut map_count = 0;
    while addr < end_page {
        // Only map if not already mapped by ELF loader
        if translate_user(root, addr).is_none() {
            map(root, addr, addr, flags);
            map_count += 1;
        } else {
            skip_count += 1;
        }
        addr += PAGE_SIZE;
    }
    crate::console_println!(
        "[idmap_skip] {:#x}-{:#x}: mapped={} skipped={}",
        start_page,
        end_page,
        map_count,
        skip_count
    );
}

/// Identity map using 2MB huge pages (x86_64 only).
/// Uses P2-level entries with PS=1 to cover 2MB per entry, avoiding page table alloc overhead.
#[cfg(target_arch = "x86_64")]
pub fn identity_map_2mb(root: &mut PageTable, start: usize, end: usize, flags: PTEFlags) {
    const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024; // 2MB
    let start_aligned = start & !(HUGE_PAGE_SIZE - 1);
    let end_aligned = (end + HUGE_PAGE_SIZE - 1) & !(HUGE_PAGE_SIZE - 1);

    let mut table = root;
    // Walk P4 → P3, creating intermediate tables as needed
    for level in &[3usize, 2] {
        // For 0..512GB range, P4[0] covers it all. level=3 is P3.
        // Use vpn=0 for the low-memory identity map.
        let vpn = PageTable::vpn(start_aligned, *level);
        let entry = &mut table.entries[vpn];
        if !entry.is_valid() {
            let new_table = PageTable::zeroed();
            let ppn = (new_table as *const PageTable as usize) >> 12;
            *entry = PTE(((ppn as u64) << 12)
                | PTEFlags::PRESENT.bits()
                | PTEFlags::WRITABLE.bits()
                | PTEFlags::USER.bits());
        }
        let ppn = entry.ppn();
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }

    // table is now P2. Set 2MB entries directly.
    let mut addr = start_aligned;
    while addr < end_aligned {
        let vpn = PageTable::vpn(addr, 1); // P2 index
        let ppn = addr >> 12;
        let huge_flags = flags.bits() | PTEFlags::PS.bits();
        table.entries[vpn] = PTE(((ppn as u64) << 12) | huge_flags);
        addr += HUGE_PAGE_SIZE;
    }
}

static mut KERNEL_PAGE_TABLE: *mut PageTable = core::ptr::null_mut();

pub fn get_kernel_page_table() -> &'static mut PageTable {
    unsafe { &mut *KERNEL_PAGE_TABLE }
}

/// Get the physical address of the kernel page table root (for CR3 loading).
pub fn kernel_cr3() -> u64 {
    unsafe { (KERNEL_PAGE_TABLE as *const PageTable as u64) }
}

pub fn init() {
    let root = PageTable::zeroed();
    let root_addr = root as *const PageTable as usize;

    unsafe {
        KERNEL_PAGE_TABLE = root;
    }

    #[cfg(target_arch = "riscv64")]
    {
        let start = 0x8020_0000;
        let end = start + 128 * 1024 * 1024;
        identity_map(root, start, end, PTEFlags::KRWX);
        map(root, 0x1000_0000, 0x1000_0000, PTEFlags::KRW);
        for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
            map(root, addr, addr, PTEFlags::KRW);
        }
        for addr in (0x0C00_0000..0x0C40_0000).step_by(PAGE_SIZE) {
            map(root, addr, addr, PTEFlags::KRW);
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        // Map all physical memory for kernel access using 2MB huge pages.
        identity_map_2mb(root, 0x0, 0x2000_0000, PTEFlags::KRWX);
        // Map MMIO regions via 2MB pages (AHCI, LAPIC, IOAPIC, etc.)
        identity_map_2mb(root, 0xFE000000, 0xFF000000, PTEFlags::KRW);
    }

    let ppn = root_addr >> 12;
    #[cfg(target_arch = "riscv64")]
    unsafe {
        satp::set(satp::Mode::Sv39, 0, ppn);
        core::arch::asm!("sfence.vma");
    }

    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::trap::activate_page_table(root_addr);
    }

    crate::console_println!("[vmm] Page table activated at {:#x}", root_addr);
}

pub fn create_user_page_table() -> &'static mut PageTable {
    PageTable::zeroed()
}

pub fn map_user(root: &mut PageTable, vaddr: usize, paddr: usize, flags: PTEFlags) {
    map(root, vaddr, paddr, flags);
}

pub fn translate_user(root: &mut PageTable, vaddr: usize) -> Option<usize> {
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = &mut table.entries[vpn];
        if !entry.is_valid() {
            return None;
        }
        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            let page_offset_mask = (1 << (12 + 9 * level)) - 1;
            return Some((entry.ppn() << 12) | (vaddr & page_offset_mask));
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return None; // corrupted entry — PPN must not be zero
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = &mut table.entries[vpn];
    if !entry.is_valid() {
        return None;
    }
    Some((entry.ppn() << 12) | (vaddr & 0xFFF))
}

/// Unmap a single page from the user page table (clear PTE present bit).
/// Returns the physical address that was mapped, or None if not mapped.
pub fn unmap_user(root: &mut PageTable, vaddr: usize) -> Option<usize> {
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = &mut table.entries[vpn];
        if !entry.is_valid() {
            return None;
        }
        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            // Don't unmap huge pages
            return None;
        }
        table = unsafe { &mut *((entry.ppn() << 12) as *mut PageTable) };
    }
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = &mut table.entries[vpn];
    if !entry.is_valid() {
        return None;
    }
    let paddr = (entry.ppn() << 12) | (vaddr & 0xFFF);
    // Clear the PTE (set to zero)
    table.entries[vpn] = PTE(0);
    Some(paddr)
}

/// Change flags on a mapped user page.
/// If the page is not currently mapped, this is a no-op.
pub fn mprotect_user(root: &mut PageTable, vaddr: usize, flags: PTEFlags) -> bool {
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = &mut table.entries[vpn];
        if !entry.is_valid() {
            return false;
        }
        #[cfg(target_arch = "x86_64")]
        {
            if entry.flags().contains(PTEFlags::PS) {
                // Huge page at this level — update flags directly in this entry
                let ppn = entry.ppn();
                table.entries[vpn] = PTE(((ppn as u64) << 12) | flags.bits() | PTEFlags::PS.bits());
                return true;
            }
            // Non-leaf entries must have USER bit for Ring 3 page walks.
            // Identity-mapped pages have USER=0; we must add it here.
            let raw = entry.0;
            if (raw & PTEFlags::USER.bits()) == 0 && flags.contains(PTEFlags::USER) {
                entry.0 = raw | PTEFlags::USER.bits();
            }
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return false; // corrupted entry
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = &mut table.entries[vpn];
    if !entry.is_valid() {
        return false;
    }
    // Preserve the physical address, update flags only
    let ppn = entry.ppn();
    #[cfg(target_arch = "riscv64")]
    {
        table.entries[vpn] = PTE::new(ppn, flags);
    }
    #[cfg(target_arch = "x86_64")]
    {
        table.entries[vpn] = PTE(((ppn as u64) << 12) | flags.bits());
    }
    true
}

pub fn free_user_page_table(root_ppn: usize) {
    let root = unsafe { &mut *((root_ppn << 12) as *mut PageTable) };

    fn free_level(table: &mut PageTable, level: usize) {
        for i in 0..512 {
            let entry = table.entries[i];
            if !entry.is_valid() {
                continue;
            }
            #[cfg(target_arch = "riscv64")]
            let is_huge = false; // RISC-V Sv39 doesn't use PS bit in intermediate levels
            #[cfg(target_arch = "x86_64")]
            let is_huge = entry.flags().contains(PTEFlags::PS);

            if level > 1 && !is_huge {
                let child = unsafe { &mut *((entry.ppn() << 12) as *mut PageTable) };
                free_level(child, level - 1);
                pmm::dealloc_frame(entry.ppn() << 12);
            } else if level == 1 {
                #[cfg(target_arch = "riscv64")]
                let is_user = entry.flags().contains(PTEFlags::U);
                #[cfg(target_arch = "x86_64")]
                let is_user = entry.flags().contains(PTEFlags::USER);
                if is_user {
                    pmm::dealloc_frame(entry.ppn() << 12);
                }
            }
        }
    }

    free_level(root, PT_LEVELS);
    pmm::dealloc_frame(root_ppn << 12);
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── VMM Tests ──");

    crate::test::run_test("vmm_create_page_table", || {
        let pt = PageTable::zeroed();
        let addr = pt as *const PageTable as usize;
        addr % 4096 == 0
    });

    crate::test::run_test("vmm_map_single_page", || {
        let root = PageTable::zeroed();
        #[cfg(target_arch = "riscv64")]
        let test_addr = 0x8040_0000;
        #[cfg(target_arch = "x86_64")]
        let test_addr = 0x0040_0000;
        map(root, test_addr, test_addr, PTEFlags::KRW);
        translate_user(root, test_addr) == Some(test_addr)
    });

    crate::test::run_test("vmm_pte_flags_present", || {
        let pte = PTE::new(0x100, PTEFlags::KRWX);
        pte.is_valid()
    });

    crate::test::run_test("vmm_pte_ppn", || {
        let pte = PTE::new(0xABCD, PTEFlags::KRW);
        pte.ppn() == 0xABCD
    });
}
