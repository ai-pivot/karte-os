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

/// Initialize virtual memory with identity mapping
pub fn init() {
    let root = PageTable::zeroed();
    let root_addr = root as *const PageTable as usize;

    // Identity map kernel (0x80200000 .. 0x80200000 + 128MB)
    let start = 0x8020_0000;
    let end = start + 128 * 1024 * 1024;
    identity_map(root, start, end, PTEFlags::KRWX);

    // Map UART MMIO (0x10000000 - 0x10001000)
    map(root, 0x1000_0000, 0x1000_0000, PTEFlags::KRW);

    // Map VirtIO MMIO devices (0x10001000 - 0x10003000)
    // 8 devices at 0x200 stride, each may access up to 0x200 bytes
    // Map 2 pages to cover all 8 devices
    for addr in (0x1000_1000..0x1000_3000).step_by(PAGE_SIZE) {
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
