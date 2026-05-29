// kernel/src/mm/heap.rs — Kernel Heap Allocator

use buddy_system_allocator::LockedHeap;

use super::pmm;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap<32> = LockedHeap::empty();

// 4MB heap — ext4 filesystem metadata (superblock, block group descriptors,
// inode tables, extent trees) requires significantly more memory than the
// previous 1MB allocation.
const HEAP_PAGES: usize = 1024; // 1024 * 4KB = 4MB heap

pub fn init() {
    // Allocate contiguous physical pages for the buddy allocator.
    // We use the PMM's contiguous allocator to guarantee physical continuity,
    // which is required for correct identity-mapped DMA operations.
    let heap_start = pmm::alloc_contiguous_frames(HEAP_PAGES)
        .expect("Failed to allocate contiguous heap memory");

    unsafe {
        HEAP_ALLOCATOR
            .lock()
            .init(heap_start, HEAP_PAGES * pmm::page_size());
    }

    crate::console_println!(
        "[heap] Initialized: {} KB heap at {:#x}",
        HEAP_PAGES * pmm::page_size() / 1024,
        heap_start
    );
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Heap Tests ──");

    // Test 1: Vec allocation
    crate::test::run_test("heap_vec_alloc", || {
        let mut v: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        for i in 0..100u32 {
            v.push(i);
        }
        v.len() == 100 && v[99] == 99
    });

    // Test 2: String allocation
    crate::test::run_test("heap_string_alloc", || {
        let s: alloc::string::String = alloc::string::String::from("Hello, KarteOS!");
        s.len() == 15 && s.starts_with("Hello")
    });

    // Test 3: Large allocation
    crate::test::run_test("heap_large_alloc", || {
        let mut v: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(4096);
        for i in 0..256u16 {
            v.push(i as u8);
        }
        v.len() == 256
    });

    // Test 4: Box allocation
    crate::test::run_test("heap_box_alloc", || {
        let b = alloc::boxed::Box::new(42usize);
        *b == 42
    });

    // Test 5: Multiple Vecs
    crate::test::run_test("heap_multiple_vecs", || {
        let v1: alloc::vec::Vec<u8> = (0..50).collect();
        let v2: alloc::vec::Vec<u8> = (100..150).collect();
        let v3: alloc::vec::Vec<u8> = (200..250).collect();
        v1.len() == 50 && v2.len() == 50 && v3.len() == 50
    });

    // Test 6: Drop and realloc
    crate::test::run_test("heap_drop_realloc", || {
        {
            let _v: alloc::vec::Vec<usize> = (0..1000).collect();
        } // dropped here
        let v: alloc::vec::Vec<usize> = (0..1000).collect();
        v.len() == 1000
    });
}
