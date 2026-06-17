// kernel/src/mm/heap.rs — Kernel Heap Allocator

use super::pmm;

/// Custom spinlock that disables interrupts while held.
///
/// This prevents timer ISR from preempting heap operations, which would
/// cause deadlock (single CPU: ISR → schedule → __switch → other task
/// tries heap alloc → spins forever on held lock) or corruption
/// (SMP: another CPU acquires the "locked" mutex → concurrent mutation).
struct IrqSafeHeap {
    inner: spin::Mutex<linked_list_allocator::Heap>,
}

impl IrqSafeHeap {
    const fn empty() -> Self {
        Self {
            inner: spin::Mutex::new(linked_list_allocator::Heap::empty()),
        }
    }

    unsafe fn init(&self, start: *mut u8, size: usize) {
        self.inner.lock().init(start, size);
    }
}

unsafe impl core::alloc::GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        // Disable interrupts before acquiring the heap lock.
        // This prevents timer ISR from preempting us while we hold it,
        // which would cause deadlock on single-CPU or data corruption on SMP.
        let irq_was_enabled = crate::arch::platform::irq_save();
        let result = self.inner.lock().allocate_first_fit(layout);
        crate::arch::platform::irq_restore(irq_was_enabled);
        result.map_or(core::ptr::null_mut(), |p| p.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        let irq_was_enabled = crate::arch::platform::irq_save();
        if let Some(p) = core::ptr::NonNull::new(ptr) {
            self.inner.lock().deallocate(p, layout);
        }
        crate::arch::platform::irq_restore(irq_was_enabled);
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: IrqSafeHeap = IrqSafeHeap::empty();

static mut HEAP_START: usize = 0;
static mut HEAP_SIZE: usize = 0;

pub fn heap_start() -> usize {
    unsafe { HEAP_START }
}
pub fn heap_size() -> usize {
    unsafe { HEAP_SIZE }
}

pub fn init() {
    // Dynamic kernel heap sizing based on physical memory.
    //
    // The kernel heap is used for: Vec/String allocations, ext4 metadata,
    // sector caches, filesystem buffers, etc. It does NOT back user-space
    // allocations (those use mmap/brk → PF handler → PMM directly).
    //
    // Strategy:
    //   - Use 1/16 of total RAM, minimum 8MB, maximum 64MB.
    //   - All remaining physical memory is available for user-space via the
    //     PMM (page fault handler allocates frames on demand for mmap/brk).
    //   - This ensures user processes can use nearly all available RAM,
    //     scaling with hardware (512MB RAM → ~448MB for user, 4GB → ~4GB).
    let (total_frames, used_frames, _) = pmm::debug_stats();
    let total_ram = total_frames * pmm::page_size();
    let heap_pages = ((total_ram / 16) / pmm::page_size())
        .max(2048) // minimum 8MB
        .min(16384); // maximum 64MB

    let heap_start_phys = pmm::alloc_contiguous_frames(heap_pages)
        .expect("Failed to allocate contiguous heap memory");

    let heap_size = heap_pages * pmm::page_size();

    // Use phys_to_virt so heap allocations are accessible under BOTH kernel CR3
    // (identity mapping) and user CR3 (direct map via shared PML4[511]).
    // Without this, any code inside with_user_cr3() that touches heap data
    // (Vec, String, etc.) would page-fault because the user page table has no
    // identity mapping of low RAM.
    #[cfg(target_arch = "x86_64")]
    let heap_start = crate::mm::vmm::phys_to_virt(heap_start_phys);
    #[cfg(not(target_arch = "x86_64"))]
    let heap_start = heap_start_phys;

    unsafe {
        HEAP_START = heap_start;
        HEAP_SIZE = heap_size;
        HEAP_ALLOCATOR.init(heap_start as *mut u8, heap_size);
    }

    crate::console_println!(
        "[heap] Initialized: {} MB heap at {:#x} ({} of {} MB RAM)",
        heap_size / 1024 / 1024,
        heap_start,
        heap_size / 1024 / 1024,
        total_ram / 1024 / 1024,
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
