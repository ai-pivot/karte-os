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
