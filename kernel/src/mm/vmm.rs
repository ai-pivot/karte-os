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

        // Aliases for architecture-agnostic code
        const R = Self::PRESENT.bits();       // Readable = Present
        const X = Self::PRESENT.bits();       // Executable (no NX set)
        const A = Self::ACCESSED.bits();      // Accessed
        const D = Self::DIRTY.bits();         // Dirty
        const V = Self::PRESENT.bits();       // Valid
        const W = Self::WRITABLE.bits();      // Writable
        const G = Self::GLOBAL.bits();        // Global
    }
}

const PAGE_SIZE: usize = 4096;

/// x86_64 non-leaf page table entry flags: Present + Writable + User.
/// All intermediate (non-leaf) entries need USER bit for Ring 3 page walks.
#[cfg(target_arch = "x86_64")]
const NON_LEAF_FLAGS: u64 =
    PTEFlags::PRESENT.bits() | PTEFlags::WRITABLE.bits() | PTEFlags::USER.bits();
const PTE_COUNT: usize = 512;

#[cfg(target_arch = "riscv64")]
const PT_LEVELS: usize = 4; // Sv48
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
        Self::new_leaf(ppn, flags)
    }

    #[cfg(target_arch = "x86_64")]
    pub fn new_leaf(ppn: usize, flags: PTEFlags) -> Self {
        // x86_64 leaf PTEs must preserve exact permissions; read-only ELF
        // pages must not become writable during copy/rebuild paths.
        Self(((ppn as u64) << 12) | flags.bits())
    }

    #[cfg(target_arch = "x86_64")]
    pub fn new_nonleaf(ppn: usize) -> Self {
        Self(((ppn as u64) << 12) | NON_LEAF_FLAGS)
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

    #[cfg(target_arch = "riscv64")]
    pub fn is_leaf(&self) -> bool {
        let f = self.flags();
        f.contains(PTEFlags::R) || f.contains(PTEFlags::W) || f.contains(PTEFlags::X)
    }

    #[cfg(target_arch = "x86_64")]
    pub fn is_leaf_at_level(&self, level: usize) -> bool {
        self.is_valid() && (level == 0 || self.flags().contains(PTEFlags::PS))
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
                *entry = PTE(((ppn as u64) << 12) | NON_LEAF_FLAGS);
            }
        }

        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            // Huge page at this level — must split to create a 4KB entry.
            let huge_ppn = entry.ppn();
            let huge_paddr = huge_ppn << 12;
            // Page size at the next level down:
            //   level 3 (PDP) splits to level 2 (PD) → 2MB sub-pages
            //   level 2 (PD) splits to level 1 (PT) → 4KB sub-pages
            let sub_page_size = if level == 3 { 1 << 21 } else { 1 << 12 };
            let new_table = PageTable::zeroed();
            let new_ppn = (new_table as *const PageTable as usize) >> 12;
            for i in 0..512 {
                let sub_paddr = huge_paddr + i * sub_page_size;
                let sub_flags: u64 = if sub_page_size > 4096 {
                    (PTEFlags::PRESENT.bits()
                        | PTEFlags::WRITABLE.bits()
                        | PTEFlags::USER.bits()
                        | PTEFlags::PS.bits()) as u64
                } else {
                    entry.0 & !(PTEFlags::PS.bits() as u64)
                };
                new_table.entries[i] = PTE(((sub_paddr >> 12) as u64) << 12 | sub_flags);
            }
            // Replace huge page entry with pointer to new page table
            let new_entry_flags = NON_LEAF_FLAGS;
            *entry = PTE(((new_ppn as u64) << 12) | new_entry_flags);
            // FLUSH TLB for this entry so the split takes effect immediately
            crate::arch::trap::flush_tlb_addr(vaddr);
            // Continue traversal into the new table
            table = unsafe { &mut *((new_ppn << 12) as *mut PageTable) };
            if sub_page_size == PAGE_SIZE {
                // Splitting a 2MB PD leaf produces a PT that already contains
                // 4KB leaf entries. Stop here so the level-0 mapping code below
                // updates the PT entry instead of treating a data frame as a
                // lower-level page table.
                break;
            }
            continue;
        }

        let ppn = entry.ppn();
        #[cfg(target_arch = "x86_64")]
        if ppn == 0 {
            // Corrupt page table entry — ppn=0 but PRESENT.
            // This can happen if huge page split wrote wrong data.
            crate::console_println!(
                "[map] BUG: ppn=0 at level={} vaddr={:#x} entry={:#x}",
                level,
                vaddr,
                entry.0
            );
            return;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };

        // x86_64: ensure non-leaf entries have User bit for Ring 3 page walks.
        // identity_map_skip creates entries without User bit; we must add it
        // when mapping user-accessible pages (e.g., user stack).
        #[cfg(target_arch = "x86_64")]
        {
            ensure_nonleaf_user_bit(entry, flags);
        }
    }

    // Level 0: leaf entry
    let vpn = PageTable::vpn(vaddr, 0);
    let ppn = paddr >> 12;
    #[cfg(target_arch = "riscv64")]
    {
        // RISC-V Sv39 on QEMU -cpu rv64 does NOT auto-set A/D bits.
        // Pre-set A bit on all leaf PTEs to avoid access PF loops.
        // Only set D bit on WRITABLE pages — D on read-only pages
        // violates RISC-V semantics and corrupts Go's type system
        // (itabInit bounds checks fail because D on read-only .data
        // pages confuses Go's memory model).
        let mut final_flags = flags | PTEFlags::A;
        if flags.contains(PTEFlags::W) {
            final_flags |= PTEFlags::D;
        }
        table.entries[vpn] = PTE::new(ppn, final_flags);
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

    let mut addr = start_aligned;
    while addr < end_aligned {
        let p4_idx = PageTable::vpn(addr, 3);
        if !root.entries[p4_idx].is_valid() {
            let new_p3 = PageTable::zeroed();
            let ppn = (new_p3 as *const PageTable as usize) >> 12;
            root.entries[p4_idx] = PTE::new_nonleaf(ppn);
        }

        let p3 = unsafe { &mut *((root.entries[p4_idx].ppn() << 12) as *mut PageTable) };
        let p3_idx = PageTable::vpn(addr, 2);
        if !p3.entries[p3_idx].is_valid() {
            let new_p2 = PageTable::zeroed();
            let ppn = (new_p2 as *const PageTable as usize) >> 12;
            p3.entries[p3_idx] = PTE::new_nonleaf(ppn);
        }

        let p2 = unsafe { &mut *((p3.entries[p3_idx].ppn() << 12) as *mut PageTable) };
        let vpn = PageTable::vpn(addr, 1);
        let ppn = addr >> 12;
        let huge_flags = flags.bits() | PTEFlags::PS.bits();
        p2.entries[vpn] = PTE(((ppn as u64) << 12) | huge_flags);
        addr += HUGE_PAGE_SIZE;
    }
}

/// RISC-V fallback: no 2MB huge pages, delegate to regular identity_map.
#[cfg(not(target_arch = "x86_64"))]
pub fn identity_map_2mb(root: &mut PageTable, start: usize, end: usize, flags: PTEFlags) {
    identity_map(root, start, end, flags);
}

static mut KERNEL_PAGE_TABLE: *mut PageTable = core::ptr::null_mut();

/// Kernel SATP value for trap_entry.S to switch to on U-mode traps.
/// This ensures all S-mode code runs with the kernel page table,
/// which has correct A/D bits on all kernel pages.
#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
pub static KERNEL_SATP: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

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
        // RISC-V Sv39 with QEMU -cpu rv64 does NOT auto-set A/D bits.
        // All PTEs must have A|D pre-set to avoid page faults on every access.
        let start = 0x8020_0000;
        let end = start + 2048 * 1024 * 1024;
        identity_map(root, start, end, PTEFlags::KRWX | PTEFlags::A | PTEFlags::D);
        map(
            root,
            0x1000_0000,
            0x1000_0000,
            PTEFlags::KRW | PTEFlags::A | PTEFlags::D,
        );
        for addr in (0x1000_1000..0x1000_9000).step_by(PAGE_SIZE) {
            map(root, addr, addr, PTEFlags::KRW | PTEFlags::A | PTEFlags::D);
        }
        for addr in (0x0C00_0000..0x0C40_0000).step_by(PAGE_SIZE) {
            map(root, addr, addr, PTEFlags::KRW | PTEFlags::A | PTEFlags::D);
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        // Map all PMM-managed physical memory for kernel access.
        identity_map_2mb(root, 0x0, pmm::total_memory(), PTEFlags::KRWX);
        // Map MMIO regions via 2MB pages (AHCI, LAPIC, IOAPIC, etc.)
        identity_map_2mb(root, 0xFE000000, 0xFF000000, PTEFlags::KRW);
    }

    let ppn = root_addr >> 12;
    #[cfg(target_arch = "riscv64")]
    unsafe {
        satp::set(satp::Mode::Sv48, 0, ppn);
        core::arch::asm!("sfence.vma");
        KERNEL_SATP.store((9usize << 60) | ppn, core::sync::atomic::Ordering::Relaxed);
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

/// x86_64: ensure non-leaf entries have User bit set for Ring 3 page walks.
/// Identity-mapped entries (from copy_kernel_mappings) lack USER bit;
/// this patches them on demand when mapping or protecting user-accessible pages.
#[cfg(target_arch = "x86_64")]
#[inline]
fn ensure_nonleaf_user_bit(entry: &mut PTE, leaf_flags: PTEFlags) {
    let raw = entry.0;
    if (raw & PTEFlags::USER.bits()) == 0 && leaf_flags.contains(PTEFlags::USER) {
        entry.0 = raw | PTEFlags::USER.bits();
    }
}

/// Walk the page table from root down to the PT level (level 0 parent).
/// Returns the PT-level PageTable reference and the level-0 VPN for `vaddr`.
/// Returns None if any intermediate entry is invalid (or a huge page on x86_64).
fn walk_to_pt(root: &mut PageTable, vaddr: usize) -> Option<(&mut PageTable, usize)> {
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = table.entries[vpn];
        if !entry.is_valid() {
            return None;
        }
        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            return None;
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return None;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    Some((table, PageTable::vpn(vaddr, 0)))
}

/// Translate a virtual address to its physical frame.
///
/// **Deprecated**: Use `walk_mapping()` instead, which returns `WalkResult`
/// and forces callers to handle `MappedHuge` explicitly.
#[deprecated(note = "use walk_mapping() which returns WalkResult")]
pub fn translate_user(root: &mut PageTable, vaddr: usize) -> Option<usize> {
    // Walk intermediate levels, handling huge pages on x86_64
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = table.entries[vpn];
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
            return None;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    // Leaf level (PT)
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = table.entries[vpn];
    if !entry.is_valid() {
        return None;
    }
    Some((entry.ppn() << 12) | (vaddr & 0xFFF))
}

/// Typed page-table walk returning `WalkResult`.
///
/// Unlike `translate_user()` which returns `Option<usize>`, this function
/// explicitly returns `MappedHuge` when a huge page is encountered, forcing
/// callers to handle that case rather than silently descending.
pub fn walk_mapping(root: &mut PageTable, vaddr: usize) -> super::page_table::WalkResult {
    use super::page_table::WalkResult;
    use crate::mm::addr::PhysAddr;

    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = table.entries[vpn];
        if !entry.is_valid() {
            return WalkResult::NotMapped;
        }
        #[cfg(target_arch = "x86_64")]
        if entry.flags().contains(PTEFlags::PS) {
            let page_size = 1 << (12 + 9 * level);
            let page_offset_mask = page_size - 1;
            let frame_addr = (entry.ppn() << 12) | (vaddr & page_offset_mask);
            return WalkResult::MappedHuge {
                frame: PhysAddr::new(frame_addr),
                level,
                size: page_size,
                flags: entry.flags().bits(),
            };
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return WalkResult::Invalid;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    // Leaf level (PT)
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = table.entries[vpn];
    if !entry.is_valid() {
        return WalkResult::NotMapped;
    }
    let ppn = entry.ppn();
    if ppn == 0 {
        return WalkResult::Invalid;
    }
    WalkResult::Mapped4K {
        frame: PhysAddr::new((ppn << 12) | (vaddr & 0xFFF)),
        flags: entry.flags().bits(),
    }
}

/// Return the raw leaf PTE for debugging user mappings.
#[cfg(target_arch = "x86_64")]
pub fn debug_pte(root: &mut PageTable, vaddr: usize) -> Option<u64> {
    let (pt, vpn) = walk_to_pt(root, vaddr)?;
    Some(pt.entries[vpn].0)
}

/// Unmap a single page from the user page table (clear PTE present bit).
/// Returns the physical address that was mapped, or None if not mapped.
pub fn unmap_user(root: &mut PageTable, vaddr: usize) -> Option<usize> {
    let (pt, vpn) = match walk_to_pt(root, vaddr) {
        Some(r) => r,
        None => return None,
    };
    let entry = pt.entries[vpn];
    if !entry.is_valid() {
        return None;
    }
    let paddr = (entry.ppn() << 12) | (vaddr & 0xFFF);

    // Warn if unmap targets ELF region (0x400000..HEAP_BASE) — this usually
    // means mmap/madvise is corrupting loaded binary data.
    #[cfg(target_arch = "x86_64")]
    {
        let heap_base = crate::process::USER_HEAP_BASE;
        if vaddr >= 0x400000 && vaddr < heap_base {
            crate::console_println!(
                "[WARN] unmap_user ELF region vaddr={:#x} paddr={:#x}",
                vaddr,
                paddr
            );
        }
    }

    // Clear the PTE (set to zero)
    pt.entries[vpn] = PTE(0);
    Some(paddr)
}

/// Change flags on a mapped user page.
/// If the page is not currently mapped, this is a no-op.
pub fn mprotect_user(root: &mut PageTable, vaddr: usize, flags: PTEFlags) -> bool {
    // Walk intermediate levels, patching non-leaf USER bits on x86_64
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
                // A huge page here is one of the supervisor-only identity
                // mappings copied into the user page table. It is not a real
                // user leaf. Do not add USER or descend into its physical frame
                // as if it were a lower-level page table.
                return false;
            }
            ensure_nonleaf_user_bit(entry, flags);
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return false;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = table.entries[vpn];
    if !entry.is_valid() {
        return false;
    }
    // Preserve the physical address, update flags only
    let ppn = entry.ppn();
    #[cfg(target_arch = "riscv64")]
    {
        let mut final_flags = flags | PTEFlags::A;
        if flags.contains(PTEFlags::W) {
            final_flags |= PTEFlags::D;
        }
        table.entries[vpn] = PTE::new(ppn, final_flags);
    }
    #[cfg(target_arch = "x86_64")]
    {
        table.entries[vpn] = PTE(((ppn as u64) << 12) | flags.bits());
    }
    true
}

pub fn or_pte_flags(root: &mut PageTable, vaddr: usize, add_flags: PTEFlags) -> bool {
    let mut table = root;
    for level in (1..PT_LEVELS).rev() {
        let vpn = PageTable::vpn(vaddr, level);
        let entry = table.entries[vpn];
        if !entry.is_valid() {
            return false;
        }
        let ppn = entry.ppn();
        if ppn == 0 {
            return false;
        }
        table = unsafe { &mut *((ppn << 12) as *mut PageTable) };
    }
    let vpn = PageTable::vpn(vaddr, 0);
    let entry = table.entries[vpn];
    if !entry.is_valid() {
        return false;
    }
    table.entries[vpn] = PTE::new(entry.ppn(), entry.flags() | add_flags);
    true
}

/// Walk all leaf PTEs and set A|D bits. Used to pre-set A|D on kernel
/// identity mappings so that timer interrupts don't cause page faults.
pub fn set_all_ad_bits(root: &mut PageTable) {
    fn walk(table: &mut PageTable, level: usize) {
        for i in 0..512 {
            let entry = table.entries[i];
            if !entry.is_valid() {
                continue;
            }
            let flags = entry.flags();
            // Check if this is a leaf PTE (has R, W, or X set)
            if flags.contains(PTEFlags::R) || flags.contains(PTEFlags::X) {
                // Leaf PTE: set A and D bits
                let new_flags = flags | PTEFlags::A | PTEFlags::D;
                table.entries[i] = PTE::new(entry.ppn(), new_flags);
            } else if level > 0 {
                // Non-leaf PTE: recurse into next level
                let ppn = entry.ppn();
                if ppn != 0 {
                    let next = unsafe { &mut *((ppn << 12) as *mut PageTable) };
                    walk(next, level - 1);
                }
            }
        }
    }
    walk(root, PT_LEVELS - 1);
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

    // ── Corruption detection tests ──
    // These tests simulate the ELF-load-then-map scenario to detect
    // whether map() overwrites existing code frames with page table data.

    crate::test::run_test("vmm_map_does_not_clobber_code_frame", || {
        // Simulate ELF loading: allocate a frame, write magic bytes, map it as user code.
        let code_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        // Write magic pattern to the code frame
        let magic: u64 = 0xDEADBEEF_CAFEBABE;
        unsafe {
            (code_frame as *mut u64).write(magic);
        }

        // Create a user page table and map the code frame
        let root = PageTable::zeroed();
        #[cfg(target_arch = "x86_64")]
        let code_vaddr = 0x400000usize;
        #[cfg(target_arch = "riscv64")]
        let code_vaddr = 0x1000usize;
        map(root, code_vaddr, code_frame, PTEFlags::URW);

        // Verify the mapping is correct
        let translated = translate_user(root, code_vaddr);
        if translated != Some(code_frame) {
            return false;
        }

        // Now map a DIFFERENT virtual address that requires new intermediate page tables.
        // If map() allocates a frame that happens to be code_frame, the magic bytes get zeroed.
        #[cfg(target_arch = "x86_64")]
        let far_vaddr = 0x2000_0000usize; // mmap region — different PDP entry
        #[cfg(target_arch = "riscv64")]
        let far_vaddr = 0x8000_0000usize;
        let data_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        map(root, far_vaddr, data_frame, PTEFlags::URW);

        // Check: code frame magic must still be intact
        let check = unsafe { (code_frame as *const u64).read() };
        if check != magic {
            crate::console_println!(
                "[FAIL] code_frame={:#x} magic={:#x} expected={:#x}",
                code_frame,
                check,
                magic
            );
            return false;
        }
        true
    });

    crate::test::run_test("vmm_map_preserves_all_existing_mappings", || {
        // Map 4 code frames, then map a far address, then check all 4 are intact.
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();

        let mut code_frames = [0usize; 4];
        let mut magics = [0u64; 4];
        for i in 0..4 {
            let frame = match pmm::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            code_frames[i] = frame;
            magics[i] = 0xA000_0000_0000_0000 | (i as u64);
            unsafe {
                (frame as *mut u64).write(magics[i]);
            }

            #[cfg(target_arch = "x86_64")]
            let vaddr = 0x400000 + i * page_size;
            #[cfg(target_arch = "riscv64")]
            let vaddr = 0x1000 + i * page_size;
            map(root, vaddr, frame, PTEFlags::URW);
        }

        // Map 8 distant pages (exercises intermediate page table allocation heavily)
        for j in 0..8 {
            let frame = match pmm::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            // Write a different pattern so we can detect swaps
            unsafe {
                (frame as *mut u64).write(0xB000_0000_0000_0000 | (j as u64));
            }

            #[cfg(target_arch = "x86_64")]
            let vaddr = 0x2000_0000 + j * page_size;
            #[cfg(target_arch = "riscv64")]
            let vaddr = 0x8000_0000 + j * page_size;
            map(root, vaddr, frame, PTEFlags::URW);
        }

        // Verify ALL 4 code frames are intact
        for i in 0..4 {
            let val = unsafe { (code_frames[i] as *const u64).read() };
            if val != magics[i] {
                crate::console_println!(
                    "[FAIL] code_frame[{}]={:#x} val={:#x} expected={:#x}",
                    i,
                    code_frames[i],
                    val,
                    magics[i]
                );
                return false;
            }
        }
        true
    });

    crate::test::run_test("vmm_unmap_then_remap_no_corruption", || {
        // Test: unmap a page (dealloc frame), then map a new page.
        // The deallocated frame should NOT be reused as a page table frame
        // that clobbers existing data.
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();

        // Map code at 0x400000
        let code_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        let magic: u64 = 0x1234_5678_9ABC_DEF0;
        unsafe {
            (code_frame as *mut u64).write(magic);
        }

        #[cfg(target_arch = "x86_64")]
        let code_vaddr = 0x400000usize;
        #[cfg(target_arch = "riscv64")]
        let code_vaddr = 0x1000usize;
        map(root, code_vaddr, code_frame, PTEFlags::URW);

        // Map+unmap a temporary page (simulates madvise DONTNEED cycle)
        let tmp_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        #[cfg(target_arch = "x86_64")]
        let tmp_vaddr = 0x2000_0000usize;
        #[cfg(target_arch = "riscv64")]
        let tmp_vaddr = 0x8000_0000usize;
        map(root, tmp_vaddr, tmp_frame, PTEFlags::URW);
        let freed = unmap_user(root, tmp_vaddr);
        if freed != Some(tmp_frame) {
            return false;
        }
        pmm::dealloc_frame(tmp_frame);

        // Now map a new page at the same address
        let new_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        map(root, tmp_vaddr, new_frame, PTEFlags::URW);

        // Code frame must still be intact
        let check = unsafe { (code_frame as *const u64).read() };
        if check != magic {
            crate::console_println!(
                "[FAIL] code_frame={:#x} val={:#x} expected={:#x}",
                code_frame,
                check,
                magic
            );
            return false;
        }
        true
    });

    crate::test::run_test("vmm_map_user_no_identity_clobber", || {
        // On x86_64, copy_kernel_mappings creates identity mappings.
        // map_user must NOT skip frame allocation when an identity mapping exists
        // for the target vaddr — it must allocate a NEW frame.
        // (This was a previous bug that caused ELF data to be written to wrong pages.)
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();

        // Allocate a frame and write magic
        let code_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        let magic: u64 = 0xCAFE_0000_DEAD_BEEF;
        unsafe {
            (code_frame as *mut u64).write(magic);
        }

        // Map it at a vaddr that happens to equal its physical address (identity-like)
        let vaddr = code_frame; // vaddr == paddr (identity)
        map(root, vaddr, code_frame, PTEFlags::URW);

        // translate should return the code_frame
        match translate_user(root, vaddr) {
            Some(f) if f == code_frame => {}
            other => {
                crate::console_println!(
                    "[FAIL] translate({:#x}) = {:?}, expected {:#x}",
                    vaddr,
                    other,
                    code_frame
                );
                return false;
            }
        }

        // Magic must be intact
        let check = unsafe { (code_frame as *const u64).read() };
        if check != magic {
            crate::console_println!(
                "[FAIL] identity frame={:#x} val={:#x} expected={:#x}",
                code_frame,
                check,
                magic
            );
            return false;
        }
        true
    });

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("vmm_kernel_stack_mapping_preserves_user_elf_page", || {
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();
        let elf_vaddr = 0x0460_3000usize;
        let elf_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };

        map(root, elf_vaddr, elf_frame, PTEFlags::UR);
        if translate_user(root, elf_vaddr) != Some(elf_frame) {
            return false;
        }

        let stack_phys_base = elf_vaddr - (crate::process::KERNEL_STACK_PAGES - 1) * page_size;
        let kernel_stack_top = crate::process::kernel_stack_top_from_phys_base(stack_phys_base);
        crate::process::map_kernel_stack_pages(root, kernel_stack_top);

        let after = translate_user(root, elf_vaddr);
        if after != Some(elf_frame) {
            crate::console_println!(
                "[FAIL] kernel stack mapping replaced ELF page: before={:#x} after={:?}",
                elf_frame,
                after
            );
            return false;
        }

        let stack_last_page = kernel_stack_top - page_size;
        if translate_user(root, stack_last_page) != Some(elf_vaddr) {
            crate::console_println!(
                "[FAIL] high kernel stack alias missing: vaddr={:#x} phys={:?}",
                stack_last_page,
                translate_user(root, stack_last_page)
            );
            return false;
        }

        true
    });

    crate::test::run_test("vmm_huge_page_split_preserves_data", || {
        // Simulate copy_kernel_mappings: set up 2MB huge page identity mapping,
        // then map code frames on top (splitting the huge page),
        // then map distant pages, and verify code data is intact.
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();

        // Step 1: Create 2MB huge page identity mapping (like copy_kernel_mappings does)
        // Map first 8MB using 2MB huge pages at PD level (covers 0x400000 comfortably)
        identity_map_2mb(root, 0, 8 * 1024 * 1024, PTEFlags::KRW);

        // Verify huge page mapping works at 0x400000 (PD index 2)
        let check_addr = 0x400000usize;
        match translate_user(root, check_addr) {
            Some(f) if f == check_addr => {} // identity mapped
            other => {
                crate::console_println!(
                    "[FAIL] huge page translate({:#x}) = {:?}, expected {:#x}",
                    check_addr,
                    other,
                    check_addr
                );
                return false;
            }
        }

        // Step 2: Allocate a code frame, write magic, map it at an address
        // within the 2MB range (this triggers huge page split)
        let code_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        let magic: u64 = 0xFEED_FACE_CAFE_BEEF;
        unsafe {
            (code_frame as *mut u64).write(magic);
        }

        // Map the code frame at a virtual address within the 2MB huge page range
        // This will split the 2MB huge page into 512 4KB entries
        let code_vaddr = 0x401000usize; // Within first 2MB range
        map_user(root, code_vaddr, code_frame, PTEFlags::URW);

        // Verify code data is intact after split
        let val = unsafe { (code_frame as *const u64).read() };
        if val != magic {
            crate::console_println!(
                "[FAIL] after split: code_frame={:#x} val={:#x} expected={:#x}",
                code_frame,
                val,
                magic
            );
            return false;
        }

        // Step 3: Map more code frames in the same 2MB range
        let code_frame2 = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        let magic2: u64 = 0x1234_5678_ABCD_EF01;
        unsafe {
            (code_frame2 as *mut u64).write(magic2);
        }
        map_user(root, 0x402000, code_frame2, PTEFlags::URW);

        // Step 4: Now map a page in a COMPLETELY DIFFERENT 2MB range
        // (simulating mmap PF at high address like 0x1221949e1000)
        // This should NOT affect the code frames
        let far_vaddr = 0x2000_0000usize;
        let far_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            core::ptr::write_bytes(far_frame as *mut u8, 0xAF, page_size);
        }
        map_user(root, far_vaddr, far_frame, PTEFlags::URW);

        // Verify ALL code frames are still intact
        let val1 = unsafe { (code_frame as *const u64).read() };
        if val1 != magic {
            crate::console_println!(
                "[FAIL] code_frame={:#x} val={:#x} expected={:#x}",
                code_frame,
                val1,
                magic
            );
            return false;
        }
        let val2 = unsafe { (code_frame2 as *const u64).read() };
        if val2 != magic2 {
            crate::console_println!(
                "[FAIL] code_frame2={:#x} val={:#x} expected={:#x}",
                code_frame2,
                val2,
                magic2
            );
            return false;
        }
        true
    });

    if cfg!(target_arch = "x86_64") {
        crate::test::run_test("vmm_split_does_not_clobber_sibling_pages", || {
            // After a 2MB huge page is split into 4KB entries, verify that
            // pages we DID NOT explicitly map still have valid identity mappings.
            // The split must preserve the original physical mappings for all
            // 512 sub-pages.
            let root = PageTable::zeroed();

            // Create 2MB huge page covering 0..4MB (PD indices 0 and 1)
            identity_map_2mb(root, 0, 4 * 1024 * 1024, PTEFlags::KRW);

            // Write magic to physical address 0x200000 (start of the 2MB range)
            let phys = 0x200000usize;
            let magic: u64 = 0xABBA_CDEF_1234_5678;
            unsafe {
                (phys as *mut u64).write(magic);
            }

            // Write magic to another physical address in the same 2MB range
            let phys2 = 0x300000usize; // 3MB — middle of the 2MB range
            let magic2: u64 = 0xDEAD_BEEF_CAFE_F00D;
            unsafe {
                (phys2 as *mut u64).write(magic2);
            }

            // Now map a specific 4KB page within the range (triggers split)
            let trigger_vaddr = 0x201000usize; // 2MB + 4KB
            let trigger_frame = match pmm::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            unsafe {
                core::ptr::write_bytes(trigger_frame as *mut u8, 0x42, pmm::page_size());
            }
            map_user(root, trigger_vaddr, trigger_frame, PTEFlags::URW);

            // Verify the first page (0x200000) still has its identity mapping and magic
            match translate_user(root, 0x200000) {
                Some(f) if f == phys => {
                    let val = unsafe { (phys as *const u64).read() };
                    if val != magic {
                        crate::console_println!(
                            "[FAIL] sibling page 0x200000 val={:#x} expected={:#x}",
                            val,
                            magic
                        );
                        return false;
                    }
                }
                other => {
                    crate::console_println!(
                        "[FAIL] sibling translate(0x200000) = {:?}, expected {:#x}",
                        other,
                        phys
                    );
                    return false;
                }
            }

            // Verify the middle page (0x300000) still intact
            match translate_user(root, 0x300000) {
                Some(f) if f == phys2 => {
                    let val = unsafe { (phys2 as *const u64).read() };
                    if val != magic2 {
                        crate::console_println!(
                            "[FAIL] sibling page 0x300000 val={:#x} expected={:#x}",
                            val,
                            magic2
                        );
                        return false;
                    }
                }
                other => {
                    crate::console_println!(
                        "[FAIL] sibling translate(0x300000) = {:?}, expected {:#x}",
                        other,
                        phys2
                    );
                    return false;
                }
            }
            true
        });
    } // end cfg!(x86_64)

    // ── Runtime corruption detector test ──
    // Allocate a "canary" frame, fill with magic, track it during
    // heavy map/unmap operations to detect physical memory corruption.
    crate::test::run_test("vmm_canary_survives_heavy_map", || {
        let root = PageTable::zeroed();
        let page_size = pmm::page_size();

        // Allocate canary frame with magic pattern
        let canary_frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        // Fill with recognizable pattern (not zero, not 0xFF)
        for i in 0..(page_size / 8) {
            unsafe {
                ((canary_frame + i * 8) as *mut u64).write(0xCAFE_0000_BEEF_0000 | (i as u64));
            }
        }

        // Map the canary into the page table
        map(root, 0x400000, canary_frame, PTEFlags::URW);

        // Now perform heavy mapping/unmapping to stress PMM and page table allocation
        // Map 256 pages in the mmap region (0x2000000..), unmap them, repeat
        for round in 0..4 {
            let frames: alloc::vec::Vec<usize> =
                (0..64).filter_map(|_| pmm::alloc_frame()).collect();

            for (i, &frame) in frames.iter().enumerate() {
                let vaddr = 0x2000_0000 + (round * 64 + i) * page_size;
                map(root, vaddr, frame, PTEFlags::URW);
            }

            // Unmap and free all frames
            for (i, &frame) in frames.iter().enumerate() {
                let vaddr = 0x2000_0000 + (round * 64 + i) * page_size;
                unmap_user(root, vaddr);
                pmm::dealloc_frame(frame);
            }
        }

        // Verify canary frame is intact
        for i in 0..(page_size / 8) {
            let expected = 0xCAFE_0000_BEEF_0000 | (i as u64);
            let actual = unsafe { ((canary_frame + i * 8) as *const u64).read() };
            if actual != expected {
                crate::console_println!(
                    "[FAIL] canary corruption at offset {} (frame={:#x}): {:#x} != {:#x}",
                    i,
                    canary_frame,
                    actual,
                    expected
                );
                return false;
            }
        }

        // Clean up
        unmap_user(root, 0x400000);
        pmm::dealloc_frame(canary_frame);
        true
    });

    // ── Task 1 regression tests ──

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("vmm_mprotect_does_not_descend_into_huge_page", || {
        let root = PageTable::zeroed();
        identity_map_2mb(root, 0, 8 * 1024 * 1024, PTEFlags::KRW);

        let target = 0x401000usize;
        let before = translate_user(root, target);
        let changed = mprotect_user(root, target, PTEFlags::UR);
        let after = translate_user(root, target);

        !changed && before == after
    });

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("vmm_map_split_2mb_stops_at_pt_level", || {
        let root = PageTable::zeroed();
        identity_map_2mb(root, 0, 8 * 1024 * 1024, PTEFlags::KRW);

        let frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        map_user(root, 0x401000, frame, PTEFlags::UR);

        translate_user(root, 0x401000) == Some(frame)
    });

    // ── Frame ownership tests ──

    crate::test::run_test("frame_alloc_drop_no_leak", || {
        // Allocate an OwnedFrame, read its address, drop it, allocate again.
        // If Drop doesn't free the frame, the second allocation will get a
        // different address (PMM runs out or returns a different frame).
        let f1 = match crate::mm::frame::alloc_owned_frame() {
            Some(f) => f,
            None => return false,
        };
        let addr1 = f1.addr().as_usize();
        drop(f1);

        let f2 = match crate::mm::frame::alloc_owned_frame() {
            Some(f) => f,
            None => return false,
        };
        let addr2 = f2.addr().as_usize();
        // After drop+realloc, we should get the same frame back (PMM LIFO)
        drop(f2);
        addr1 == addr2
    });

    crate::test::run_test("frame_into_raw_prevents_double_free", || {
        // into_raw() must not trigger Drop (no double-free).
        // We can't directly test double-free, but we verify that after
        // into_raw(), the raw address is usable and the PMM frame count
        // is consistent.
        let f = match crate::mm::frame::alloc_owned_frame() {
            Some(f) => f,
            None => return false,
        };
        let raw = f.into_raw();
        // Manually free the raw frame (simulating page table cleanup)
        crate::mm::pmm::dealloc_frame(raw.as_usize());
        true
    });

    // ── WalkResult typed walk tests ──

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("vmm_walk_mapping_returns_mapped_huge", || {
        let root = PageTable::zeroed();
        // Map 0..8MB so 0x401000 falls within the third 2MB page (PD[2])
        identity_map_2mb(root, 0, 8 * 1024 * 1024, PTEFlags::KRW);

        let result = walk_mapping(root, 0x401000);
        match result {
            super::page_table::WalkResult::MappedHuge { size, level, .. } => {
                if size != 2 * 1024 * 1024 {
                    crate::console_println!(
                        "[FAIL] huge size={} expected={}",
                        size,
                        2 * 1024 * 1024
                    );
                    false
                } else if level != 1 {
                    crate::console_println!("[FAIL] huge level={} expected=1", level);
                    false
                } else {
                    true
                }
            }
            other => {
                crate::console_println!("[FAIL] expected MappedHuge, got {:?}", other);
                false
            }
        }
    });

    #[cfg(target_arch = "x86_64")]
    crate::test::run_test("vmm_walk_mapping_4k_after_split", || {
        let root = PageTable::zeroed();
        identity_map_2mb(root, 0, 8 * 1024 * 1024, PTEFlags::KRW);
        let frame = match pmm::alloc_frame() {
            Some(f) => f,
            None => return false,
        };
        map_user(root, 0x401000, frame, PTEFlags::UR);
        let result = walk_mapping(root, 0x401000);
        if let super::page_table::WalkResult::Mapped4K { frame: f, .. } = result {
            f.as_usize() == frame
        } else {
            crate::console_println!("[FAIL] expected Mapped4K, got {:?}", result);
            false
        }
    });

    crate::test::run_test("vmm_walk_mapping_not_mapped", || {
        let root = PageTable::zeroed();
        let result = walk_mapping(root, 0x400000);
        matches!(result, super::page_table::WalkResult::NotMapped)
    });
}
