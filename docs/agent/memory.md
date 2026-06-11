# Memory Management

## Physical Memory Manager (PMM)

- **File**: `kernel/src/mm/pmm.rs`
- **Algorithm**: Bitmap (1 bit per 4KB frame)
- **Range**: `_ekernel` → `0x80200000 + 128MB`
- **Frame size**: 4096 bytes

### Design

1. Kernel end address (`_ekernel`) obtained from linker symbol
2. Start address aligned up to page boundary
3. Bitmap placed immediately after kernel, before managed frames
4. Bitmap frames themselves marked as allocated
5. Next-fit allocation (remembers last free position)

### API
```rust
pub fn init()                     // Initialize from _ekernel
pub fn alloc_frame() -> Option<usize>  // Allocate one 4KB frame
pub fn dealloc_frame(frame: usize)     // Free one 4KB frame
pub const fn page_size() -> usize     // Returns 4096
```

### Layout
```
0x80200000    _stext (kernel start)
  ...         .text, .rodata, .data
  ...         .bss
_boot_stack   4 pages boot stack
_ekernel      kernel end
  bitmap      N bytes (1 bit per managed frame)
  ...         managed physical frames (first bitmap frames pre-marked)
0x88200000    end of 128MB RAM
```

## Virtual Memory Manager (VMM)

- **File**: `kernel/src/mm/vmm.rs`
- **Mode**: Sv39 (39-bit virtual addresses, 3-level page tables)
- **Page size**: 4KB

### Sv39 Page Table Structure

```
Virtual Address (39 bits):
  [38:30] VPN[2] (9 bits) → Level 2 index
  [29:21] VPN[1] (9 bits) → Level 1 index
  [20:12] VPN[0] (9 bits) → Level 0 index
  [11:0]  Page offset (12 bits)

Page Table Entry (64 bits):
  [63:54] Reserved
  [53:10] PPN (Physical Page Number)
  [9:8]   RSW (Reserved for Software)
  [7]     D (Dirty)
  [6]     A (Accessed)
  [5]     G (Global)
  [4]     U (User)
  [3]     X (Execute)
  [2]     W (Write)
  [1]     R (Read)
  [0]     V (Valid)
```

### PTEFlags Combinations

| Name | Bits | Usage |
|------|------|-------|
| KRWX | V+R+W+X | Kernel code pages |
| KRW | V+R+W | Kernel data/MMIO |
| KRX | V+R+X | Kernel read-only code |
| URWX | V+R+W+X+U | User code |
| URW | V+R+W+U | User data |

### API
```rust
pub fn map(root, vaddr, paddr, flags)     // Map single page
pub fn identity_map(root, start, end, flags)  // Map range VA=PA
pub fn init()                             // Setup kernel page table + activate
```

### Init Mapping
- Identity map `0x80200000..0x88200000` as KRWX
- Map UART `0x10000000` as KRW
- Map VirtIO `0x10001000..0x10003000` as KRW
- Map PLIC `0x0C000000..0x0C400000` as KRW

## Kernel Heap

- **File**: `kernel/src/mm/heap.rs`
- **Allocator**: `buddy_system_allocator::LockedHeap<32>`
- **Size**: 256 pages = 1 MB

### Setup
1. Allocate 256 contiguous frames via PMM
2. Pass to `LockedHeap::init(addr, size)`
3. `extern crate alloc;` in main.rs enables `Vec`, `String`, etc.

## Linker Script (memory.x)

```
RAM origin: 0x80200000, length: 128M
Sections: .text.entry → .text → .rodata → .data → .bss
Symbols: _sbss, _ebss, _boot_stack_top, _ekernel
```

## Type-Safe Memory Abstractions (2026-06 refactor)

### Module Structure

| Module | File | Purpose |
|--------|------|---------|
| `mm::addr` | `mm/addr.rs` | Typed address newtypes (PhysAddr, VirtAddr, UserVirtAddr, KernelVirtAddr) |
| `mm::frame` | `mm/frame.rs` | Frame ownership types (OwnedFrame, PageTableFrame, BorrowedFrame) |
| `mm::page_table` | `mm/page_table.rs` | Page-table level markers (L4/L3/L2/L1) and WalkResult enum |
| `mm::address_space` | `mm/address_space.rs` | Per-process AddressSpaceHandle with PageTableRoot |
| `mm::diagnostics` | `mm/diagnostics.rs` | Structured PageFaultEvent and PteChain for PF analysis |

### Address Types (`mm::addr`)

```rust
pub type PhysAddr = Addr<Phys>;          // Physical address
pub type VirtAddr = Addr<Virt>;          // Virtual address (unspecified space)
pub type UserVirtAddr = Addr<Virt, User>;   // User-space virtual address
pub type KernelVirtAddr = Addr<Virt, Kernel>; // Kernel virtual address
```

- `UserVirtAddr::try_new(raw)` validates user range (< USER_MMAP_LIMIT)
- `PhysAddr::ppn()` / `PhysAddr::from_ppn()` for page number conversion
- All types: `is_page_aligned()`, `page_align_down()`, `page_offset()`

### Frame Ownership (`mm::frame`)

```rust
pub struct OwnedFrame { ... }       // Exclusive frame, freed on Drop
pub struct PageTableFrame { ... }   // Page table frame, freed on Drop
pub struct BorrowedFrame { ... }    // Non-owning reference (no Drop)
```

- `OwnedFrame::into_raw()` transfers ownership (prevents Drop)
- `PageTableFrame::as_page_table_mut()` provides typed page table access
- `alloc_owned_frame()` / `alloc_page_table_frame()` wrap PMM allocation

### Page Table Walks (`mm::page_table`)

```rust
pub enum WalkResult {
    NotMapped,                                          // Entry not present
    Mapped4K { frame: PhysAddr, flags: u64 },          // 4KB leaf
    MappedHuge { frame: PhysAddr, level: usize, size: usize, flags: u64 }, // Huge page
    Invalid,                                            // Corrupted entry
}
```

- `vmm::walk_mapping()` returns `WalkResult` — callers must handle `MappedHuge` explicitly
- Prevents mprotect/munmap from silently descending into huge pages

### Address Space Handle (`mm::address_space`)

```rust
pub struct PageTableRoot(PhysAddr);  // Typed CR3/satp value
pub struct AddressSpaceHandle { root: PageTableRoot }  // Per-process state
```

- `AddressSpaceHandle::init(root)` — create + init VMA state
- VMA operations: `vma_check()`, `vma_add()`, `vma_remove_range()`, `vma_update_prot()`
- Mapping kinds: `map_elf_page()`, `map_anon_page()`, `map_stack_page()`
- Permission types: `ElfPerms`, `AnonPerms`, `KernelPerms`

### CR3 Guards (x86_64 only)

- **File**: `arch/x86_64/cr3.rs`
- `enter_kernel_cr3()` returns `Cr3Guard` — auto-restores on drop
- `enter_cr3(target)` for switching to arbitrary CR3
- RAII ensures CR3 restore even on early returns

### User-Return State (x86_64 only)

- **File**: `arch/x86_64/user_return.rs`
- `FsBase` newtype: `FsBase::ZERO` is valid and must be written explicitly
- `UserReturnState` bundles CR3 + RSP0 + FS_BASE for user return
- `restore_for_user_return()` restores all state atomically
- `restore_fs_base()` always writes MSR, even for zero value
