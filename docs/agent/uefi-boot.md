# UEFI Boot (x86_64)

## Architecture

KarteOS uses a **dual-binary** UEFI boot approach, mimicking Linux's EFI stub:

- **`efi_loader`**: A small PE/COFF binary compiled with `x86_64-unknown-uefi`. It captures the GOP framebuffer, calls `ExitBootServices`, sets up page tables, and jumps to the kernel.
- **`karte-os-kernel`**: The existing ELF kernel compiled with `x86_64-unknown-none` (high-half linking). The kernel binary is embedded in the EFI loader via `include_bytes!` and loaded at `KERNEL_PHYS_BASE` (0x100000).

```
UEFI firmware → efi_loader.efi → GOP capture → ExitBootServices
→ page table setup (identity + direct map) → copy kernel to 0x100000
→ jump to _start64 at high-half VMA → kmain → normal kernel init
```

## EFI Loader (`efi_loader/`)

- **Target**: `x86_64-unknown-uefi` (nightly, `-Z build-std=core`)
- **Entry**: `efi_main(image_handle, system_table)`
- **Source**: `efi_loader/src/main.rs`
- **Build**: Requires `KERNEL_BIN_PATH` env var pointing to kernel flat binary

### Flow

1. **GOP capture**: LocateProtocol(EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID) → read framebuffer info → store in `BootInfo` at physical 0x10000
2. **Kernel copy**: `include_bytes!` embedded kernel binary → memcpy to `KERNEL_PHYS_BASE` (0x100000)
3. **Page tables**: Set up PML4 with identity map (0-128GB, 128 × 1GB huge pages) + direct map (high-half 0-2GB). If GOP framebuffer is above 128GB, dynamically add PML4 entry using `PDP_HIGH`
4. **Exit Boot Services**: Disable watchdog timer → GetMemoryMap → ExitBootServices (Linux-style: only retry on `EFI_INVALID_PARAMETER`)
5. **boot_transition** (global_asm): `cli` → `lgdt` → reload data segments → `mov cr3` → switch stack → `jmp` to kernel `_start64` at high-half VMA

### Key UEFI struct offsets (verified with `offset_of!`)

| Struct | Field | Offset |
|--------|-------|--------|
| EfiBootServices | get_memory_map | 0x38 |
| EfiBootServices | exit_boot_services | 0xE8 |
| EfiBootServices | set_watchdog_timer | 0x100 |
| EfiBootServices | locate_protocol | 0x140 |
| EfiGopModeInfo | pixel_info (EFI_PIXEL_BITMASK) | 0x10 (16 bytes) |
| EfiGopModeInfo | scanline (PixelsPerScanLine) | 0x20 |
| EfiGopMode | fb_base | 0x18 |

## Kernel Modifications

### `cache_kernel_cr3()` fix (`kernel/src/arch/x86_64/idt.rs`)

When called from `idt::init()` before VMM init, `KERNEL_PAGE_TABLE` is null. `kernel_cr3()` computes `virt_to_phys(0)` which underflows to `0x80000000` (the GOP framebuffer address). This causes the timer ISR stub to write the framebuffer address into CR3, crashing the system.

**Fix**: Check `vmm::is_initialized()`. If false, read CR3 directly from the register.

### `user_cr3` physical address fix (`kernel/src/sched/mod.rs`)

`ctx.user_cr3 = init.user_page_table as u64` stores a **high-half virtual address** (e.g., 0xFFFF_FFFF_8xxx_xxxx). CR3 requires bits 63:52 to be zero. Writing a high-half address causes #GP on real hardware.

**Fix**: Use `virt_to_phys(init.user_page_table as usize)` to convert to physical address.

### Framebuffer identity mapping (`kernel/src/mm/vmm.rs`, `kernel/src/main.rs`)

On modern GPUs (AMD/NVIDIA), the GOP framebuffer can be at very high physical addresses (e.g., 0x4000000000 = 256GB on AMD). The VMM identity map only covers 0-4GB. After VMM init, `identity_map_region(fb_addr, fb_size)` adds explicit identity mapping for the framebuffer.

### EFI loader framebuffer mapping (`efi_loader/src/main.rs`)

The EFI loader's page tables map 0-8GB. For framebuffers above 8GB, `setup_page_tables` dynamically adds a PML4 entry using a dedicated `PDP_HIGH` table with a 1GB huge page.

### `fb_console` font glyph mask (`kernel/src/arch/x86_64/fb_console.rs`)

The font only has 128 glyphs (ASCII). Non-ASCII characters (UTF-8 sequences from shell output) cause out-of-bounds access.

**Fix**: `(c as usize & 0x7F)` mask on character index.

## Build Commands

```bash
# Build kernel ELF
cargo +nightly build --release --target x86_64-unknown-none -p karte-os-kernel -Z build-std=core,alloc

# Convert to flat binary
objcopy -O binary target/x86_64-unknown-none/release/karte-os-kernel target/x86_64-unknown-none/release/kernel.bin

# Build EFI loader (embeds kernel binary)
KERNEL_BIN_PATH=target/x86_64-unknown-none/release/kernel.bin cargo +nightly build --release --target x86_64-unknown-uefi -p efi-loader -Z build-std=core

# Create UEFI boot disk
dd if=/dev/zero of=target/boot.img bs=1M count=128
mformat -t 256 -h 16 -s 63 -F -i target/boot.img ::
mmd -i target/boot.img ::/EFI ::/EFI/BOOT
mcopy -i target/boot.img target/x86_64-unknown-uefi/release/efi-loader.efi ::/EFI/BOOT/BOOTX64.EFI

# Write to USB
sudo dd if=target/boot.img of=/dev/sdX bs=4M status=progress && sync

# QEMU test
qemu-system-x86_64 -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=/tmp/OVMF_VARS.fd \
  -drive format=raw,file=target/uefi_disk.img \
  -serial stdio -display none -m 512M -no-reboot -cpu qemu64 -smp 1
```

## USB Image (tools/mkusb.sh)

Creates a GPT-partitioned disk:
- Partition 1 (ESP, FAT32, 128MB): `EFI/BOOT/BOOTX64.EFI`
- Partition 2 (ext4, rest): User programs (shell, ls, cat, etc.)

```bash
tools/mkusb.sh image     # Create image file
tools/mkusb.sh /dev/sdX  # Write directly to USB
```

## Gotchas

- **CR3 requires physical address, not high-half virtual**: `user_cr3` MUST be `virt_to_phys()` of the user page table pointer.
- **`cache_kernel_cr3()` is called before VMM init**: Must handle null `KERNEL_PAGE_TABLE` by reading CR3 directly.
- **GOP framebuffer can be at any physical address**: 0x80000000 on QEMU/Intel, 0x4000000000 on AMD. Always identity-map dynamically.
- **UEFI PML4 is READ-ONLY on real firmware**: Do NOT write to UEFI's active CR3 page tables. QEMU allows this but real hardware marks firmware page tables as read-only, triggering a page fault → hang. Only modify your own static PML4 (used after `boot_transition `  `mov cr3 `).
- **PE/COFF `jmp r8` in global_asm! is safe**: Unlike inline asm, `global_asm!` on UEFI target is NOT affected by LLVM's SEH `ud2` replacement.
- **Map key for ExitBootServices**: Must call GetMemoryMap immediately before EBS. No UEFI service calls (including ConOut) in between.
- **`.bss` zeroing**: GRUB zeroes `.bss` automatically from ELF headers. Our EFI loader copies a flat binary → `.bss` must be zeroed in `_start64` (already done in `boot.S`).
- **Post-EBS framebuffer access MUST use own page tables**: After ExitBootServices, real firmware (AMI/Phoenix, especially with high-BAR GPUs like RTX 4070S) may shrink or dismantle the identity mapping. Any framebuffer write (debug squares, text output) must happen AFTER `mov cr3` to the loader's own PML4 that explicitly maps the GPU BAR. On QEMU/OVMF this works by accident because OVMF keeps full identity mapping post-EBS. On real hardware it causes page fault → triple fault → reboot.
- **`allocate_pool` PoolType must be EfiLoaderData (2), NOT 0**: `fill_bootinfo_and_exit_boot_services()` passes `0` (EfiReservedMemoryType) to `AllocatePool` at offset 0x40. The constant `EFI_LOADER_DATA = 2` is defined but unused. Strict firmware rejects type 0 → fallback to static MAP_BUF (64KB) which may be too small for complex memory maps (500+ descriptors on desktop boards).
- **MAP_BUF fallback overflow**: When `allocate_pool` fails, `map_size = alloc_size` (≥64KB, up to 128KB) but `MAP_BUF` is only 64KB. `GetMemoryMap` is told the buffer is larger than it is → potential buffer overflow into adjacent `.bss`.

- **GdtDesc struct layout MUST match LGDT memory operand**: `lgdt` expects 10 contiguous bytes: 2-byte limit + 8-byte base. `#[repr(C, align(8))]` on `GdtDesc { limit: u16, base: u64, _pad: [u8; 6] }` inserts 6 bytes of **implicit padding** between `limit` and `base` (u64 requires 8-byte alignment in `repr(C)`). LGDT reads these padding bytes as the high 6 bytes of the base address, only capturing the low 2 bytes of the actual GDT address. On QEMU this accidentally works when low-memory IVT/BDA bytes form valid-looking segment descriptors. On real hardware, the garbage GDT base causes segment reload failures (#NP or #GP) → UEFI exception handler runs but cannot recover → **silent hang with no reboot**. **Fix**: Use `#[repr(C, packed)]` or a raw `[u8; 10]` array.

- **BOOT_GDT only has 3 valid entries, UEFI IDT references others**: After `lgdt` replaces UEFI's GDT with `BOOT_GDT` (entries 0=null, 1=code 0x08, 2=data 0x10, 3-7=NULL), any exception that fires will look up its handler's segment selector from UEFI's IDT in the new GDT. Selectors like `0x38` (UEFI CS) map to NULL descriptor → `#GP` → `#DF` → triple fault. Even if the cached CS descriptor survives, the exception delivery path is broken. **Fix**: Either keep UEFI's GDT until the kernel loads its own, or populate `BOOT_GDT[3..7]` with the same valid descriptors (code 0x08, data 0x10).
