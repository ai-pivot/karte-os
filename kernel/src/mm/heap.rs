// kernel/src/mm/heap.rs — Kernel Heap Allocator

use buddy_system_allocator::LockedHeap;

use super::pmm;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::empty();

const HEAP_PAGES: usize = 256; // 256 * 4KB = 1MB heap

pub fn init() {
    // Allocate contiguous heap memory
    let heap_start = allocate_contiguous_pages(HEAP_PAGES);

    unsafe {
        HEAP_ALLOCATOR.lock().init(heap_start, HEAP_PAGES * pmm::page_size());
    }

    crate::console_println!(
        "[heap] Initialized: {} KB heap at {:#x}",
        HEAP_PAGES * pmm::page_size() / 1024,
        heap_start
    );
}

/// Allocate contiguous physical pages
fn allocate_contiguous_pages(count: usize) -> usize {
    // Simple approach: try to allocate count consecutive frames.
    // The buddy allocator doesn't require contiguous physical memory
    // since it manages its own virtual address space.
    let mut first_frame: Option<usize> = None;

    for _ in 0..count {
        match pmm::alloc_frame() {
            Some(f) => {
                if first_frame.is_none() {
                    first_frame = Some(f);
                }
            }
            None => {
                panic!("Failed to allocate heap memory");
            }
        }
    }

    first_frame.expect("heap allocation returned no frames")
}
