//! Paging support for x86_64 using the `x86_64` crate.
//!
//! x86_64 uses 4-level paging (PML4 → PDPT → PD → PT → Page).
//! The `x86_64` crate provides `OffsetPageTable`, `Mapper`, and
//! `FrameAllocator` traits that handle all the page table manipulation
//! in pure Rust.

use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags,
    PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_LEVELS: usize = 4;

/// Page table entry flags (re-export x86_64 crate flags for use by the kernel).
pub mod pte_flags {
    pub use x86_64::structures::paging::PageTableFlags as Flags;

    pub const PRESENT: Flags = Flags::PRESENT;
    pub const WRITABLE: Flags = Flags::WRITABLE;
    pub const USER: Flags = Flags::USER; // Ring 3 accessible
    pub const NO_EXECUTE: Flags = Flags::NO_EXECUTE;
    pub const HUGE: Flags = Flags::HUGE_PAGE;
    pub const GLOBAL: Flags = Flags::GLOBAL;
    pub const ACCESSED: Flags = Flags::ACCESSED;
    pub const DIRTY: Flags = Flags::DIRTY;
}

/// Activate a page table (write to CR3).
pub fn activate_page_table(root_paddr: usize) {
    let phys = PhysAddr::new(root_paddr as u64);
    let new_frame = PhysFrame::containing_address(phys);
    let (current_frame, _) = Cr3::read();
    if current_frame != new_frame {
        unsafe {
            Cr3::write(new_frame, x86_64::registers::control::Cr3Flags::empty());
        }
    }
}

/// Flush the entire TLB.
pub fn flush_tlb() {
    x86_64::instructions::tlb::flush_all();
}

/// Flush a single virtual address from the TLB.
pub fn flush_tlb_addr(addr: usize) {
    let vaddr = VirtAddr::new(addr as u64);
    x86_64::instructions::tlb::flush(vaddr);
}

/// Read the current page table root physical address from CR3.
pub fn read_page_table_root() -> usize {
    let (frame, _) = Cr3::read();
    frame.start_address().as_u64() as usize
}

/// Create a new OffsetPageTable from a physical address.
///
/// # Safety
/// Caller must ensure:
/// - `phys_root` is the physical address of a valid PML4 table
/// - `phys_mem_offset` is the virtual address at which physical memory is mapped
///   (identity map means phys_mem_offset = 0)
pub unsafe fn create_offset_page_table(
    phys_root: PhysAddr,
    phys_mem_offset: VirtAddr,
) -> OffsetPageTable<'static> {
    use x86_64::structures::paging::PageTable;
    let virt = phys_mem_offset + phys_root.as_u64();
    let table: &mut PageTable = unsafe { &mut *virt.as_mut_ptr() };
    unsafe { OffsetPageTable::new(table, phys_mem_offset) }
}

/// Map a 4KiB page using the x86_64 crate's Mapper.
///
/// # Safety
/// Caller must ensure the frame allocator returns valid frames
/// and the virtual address range is not already mapped.
pub unsafe fn map_page<M, F>(
    mapper: &mut M,
    allocator: &mut F,
    vaddr: VirtAddr,
    paddr: PhysAddr,
    flags: PageTableFlags,
) -> Result<(), ()>
where
    M: Mapper<Size4KiB>,
    F: FrameAllocator<Size4KiB>,
{
    let page = Page::containing_address(vaddr);
    let frame = PhysFrame::containing_address(paddr);
    let flush = unsafe { mapper.map_to(page, frame, flags, allocator) }.map_err(|_| ())?;
    flush.flush();
    Ok(())
}

/// Translate a virtual address to physical using the x86_64 crate.
pub fn translate(mapper: &OffsetPageTable, vaddr: VirtAddr) -> Option<PhysAddr> {
    mapper.translate_addr(vaddr)
}
