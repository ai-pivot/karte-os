// efi_loader/src/main.rs — UEFI bootloader for KarteOS
//
// Mirrors Linux's EFI stub flow:
//   1. efi_main: capture GOP, call ExitBootServices
//   2. Set up page tables with dual mapping (identity + high-half)
//   3. Copy embedded kernel binary to its link address
//   4. Jump to kernel's _start64 entry point at high-half VMA
//
// The kernel is compiled separately as x86_64-unknown-none ELF and embedded
// via include_bytes!. The loader jumps to _start64 which expects:
//   - Paging enabled with identity map + high-half map
//   - A valid GDT loaded
//   - 64-bit long mode

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ffi::c_void;

// ─── Boot transition assembly ────────────────────────────────────────────
//
// Like Linux's startup_64: loads GDT, switches CR3 to dual-mapping page
// tables, sets up stack, then jumps to _start64 at high-half VMA.
// We use global_asm! to avoid PE inline-asm label issues.

global_asm!(
    ".global {sym}",
    "{sym}:",
    "    cli",
    // Debug: 'L' = loading GDT
    "    mov dx, 0x3F8",
    "    mov al, 0x4C",
    "    out dx, al",
    "    lgdt [r9]",
    "    mov ax, 0x10",
    "    mov ds, ax",
    "    mov es, ax",
    "    mov fs, ax",
    "    mov gs, ax",
    "    mov ss, ax",
    // Debug: 'C' = CR3 switch
    "    mov dx, 0x3F8",
    "    mov al, 0x43",
    "    out dx, al",
    "    mov cr3, rcx",
    // Debug: 'S' = stack
    "    mov dx, 0x3F8",
    "    mov al, 0x53",
    "    out dx, al",
    "    mov rsp, rdx",
    "    xor ebp, ebp",
    "    cld",
    // Pass multiboot2-like args: eax=0x36d76289 magic, ebx=0 (no mb2 info)
    // _start64 reads from 0x8000/0x8004, we set those before calling
    "    xor edi, edi",
    "    xor esi, esi",
    // Debug: 'K' = jump to kernel
    "    mov dx, 0x3F8",
    "    mov al, 0x4B",
    "    out dx, al",
    "    jmp r8",
    sym = sym boot_transition,
);

unsafe extern "C" {
    fn boot_transition(pml4_phys: u64, stack_top: u64, entry_addr: u64, gdt_desc: u64) -> !;
}

// ─── UEFI Types ──────────────────────────────────────────────────────────

type EfiHandle = *mut c_void;

#[repr(C)]
struct EfiGuid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct EfiTableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    _reserved: u32,
}

#[repr(C)]
struct EfiSystemTable {
    hdr: EfiTableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    _pad0: u32,
    console_in_handle: EfiHandle,
    con_in: *const c_void,
    console_out_handle: EfiHandle,
    con_out: *const c_void,
    std_err_handle: EfiHandle,
    std_err: *const c_void,
    runtime_services: *const c_void,
    boot_services: *const EfiBootServices,
    num_table_entries: u64,
    config_table: *const c_void,
}

// Verify EfiSystemTable offsets (UEFI spec §4.3)
#[allow(dead_code)]
const _ET: () = {
    use core::mem::offset_of;
    assert!(offset_of!(EfiSystemTable, boot_services) == 0x60);
    assert!(offset_of!(EfiSystemTable, con_out) == 0x40);
};

#[repr(C)]
struct EfiBootServices {
    hdr: EfiTableHeader,
    _pad0: [u8; 16], // offset 24: raise_tpl, restore_tpl
    // offset 40 (0x28): AllocatePages — returns physical address
    allocate_pages: extern "C" fn(
        u32,        // Type: 0=AllocateAnyPages
        u32,        // MemoryType: 2=EfiLoaderData
        usize,      // Pages (count of 4 KiB pages)
        *mut u64,   // Memory (out: physical address)
    ) -> usize,
    // offset 48 (0x30): FreePages
    free_pages: extern "C" fn(u64, usize) -> usize,
    // offset 56: get_memory_map
    get_memory_map: extern "C" fn(
        *mut usize, // memory_map_size (in/out)
        *mut u8,    // buffer (can be null to query size)
        *mut usize, // map_key (out)
        *mut usize, // descriptor_size (out)
        *mut u32,   // descriptor_version (out)
    ) -> usize,
    _pad2: [u8; 8],  // offset 64: allocate_pool
    _pad3: [u8; 24], // offset 72: free_pool, create_event, set_timer
    _pad4: [u8; 32], // offset 96
    _pad5: [u8; 24], // offset 128
    _pad6: [u8; 16], // offset 152
    _pad7: [u8; 16], // offset 168
    _pad8: [u8; 16], // offset 184
    _pad9: [u8; 32], // offset 200
    exit_boot_services: extern "C" fn(EfiHandle, usize) -> usize, // offset 232
    _pad10: [u8; 16],                                     // offset 240: GetMonotonicCount + Stall
    set_watchdog_timer: extern "C" fn(usize, u64, u64, *const c_void) -> usize, // offset 256
    _pad11: [u8; 16],                                     // offset 264: Connect/DisconnectController
    _pad12: [u8; 24],                                     // offset 280
    _pad13: [u8; 16],                                     // offset 304
    locate_protocol: extern "C" fn(*const EfiGuid, *const c_void, *mut *const c_void) -> usize,
}

// Verify EfiBootServices offsets at compile time (UEFI spec §4.4)
#[allow(dead_code)]
const _: () = {
    use core::mem::offset_of;
    // offset 40 (0x28) = AllocatePages
    assert!(offset_of!(EfiBootServices, allocate_pages) == 0x28);
    // offset 48 (0x30) = FreePages
    assert!(offset_of!(EfiBootServices, free_pages) == 0x30);
    // offset 56 (0x38) = GetMemoryMap
    assert!(offset_of!(EfiBootServices, get_memory_map) == 0x38);
    // offset 232 (0xE8) = ExitBootServices
    assert!(offset_of!(EfiBootServices, exit_boot_services) == 0xE8);
    // offset 256 (0x100) = SetWatchdogTimer
    assert!(offset_of!(EfiBootServices, set_watchdog_timer) == 0x100);
    // offset 320 (0x140) = LocateProtocol
    assert!(offset_of!(EfiBootServices, locate_protocol) == 0x140);
};

// ─── GOP Types ──────────────────────────────────────────────────────────

#[repr(C)]
struct EfiGopModeInfo {
    version: u32,
    hres: u32,
    vres: u32,
    pixel_format: u32,
    pixel_info: [u32; 4], // EFI_PIXEL_BITMASK = 16 bytes (RedMask, GreenMask, BlueMask, ReservedMask)
    scanline: u32,        // PixelsPerScanLine — at offset 32
}

// Verify EfiGopModeInfo offsets (UEFI spec §11.9)
#[allow(dead_code)]
const _GMI: () = {
    use core::mem::offset_of;
    assert!(offset_of!(EfiGopModeInfo, version) == 0x00);
    assert!(offset_of!(EfiGopModeInfo, hres) == 0x04);
    assert!(offset_of!(EfiGopModeInfo, vres) == 0x08);
    assert!(offset_of!(EfiGopModeInfo, pixel_format) == 0x0C);
    assert!(offset_of!(EfiGopModeInfo, pixel_info) == 0x10); // EFI_PIXEL_BITMASK = 16 bytes
    assert!(offset_of!(EfiGopModeInfo, scanline) == 0x20);   // PixelsPerScanLine at 32
};

#[repr(C)]
struct EfiGopMode {
    max_mode: u32,
    mode: u32,
    info: *const EfiGopModeInfo,
    size_of_info: u64,
    fb_base: u64,
    fb_size: u64,
}

// Verify EfiGopMode offsets (UEFI EFI_GRAPHICS_OUTPUT_PROTOCOL_MODE §11.9)
#[allow(dead_code)]
const _GM: () = {
    use core::mem::offset_of;
    assert!(offset_of!(EfiGopMode, max_mode) == 0x00);
    assert!(offset_of!(EfiGopMode, mode) == 0x04);
    assert!(offset_of!(EfiGopMode, info) == 0x08);
    assert!(offset_of!(EfiGopMode, size_of_info) == 0x10);
    assert!(offset_of!(EfiGopMode, fb_base) == 0x18);
    assert!(offset_of!(EfiGopMode, fb_size) == 0x20);
};

#[repr(C)]
struct EfiGop {
    query_mode: usize,
    set_mode: usize,
    blt: usize,
    mode: *const EfiGopMode,
}

// Verify EfiGop offsets (UEFI EFI_GRAPHICS_OUTPUT_PROTOCOL §11.9)
#[allow(dead_code)]
const _GOP_: () = {
    use core::mem::offset_of;
    assert!(offset_of!(EfiGop, query_mode) == 0x00);
    assert!(offset_of!(EfiGop, set_mode) == 0x08);
    assert!(offset_of!(EfiGop, blt) == 0x10);
    assert!(offset_of!(EfiGop, mode) == 0x18);
};

const GOP_GUID: EfiGuid = EfiGuid {
    data1: 0x9042A9DE,
    data2: 0x23DC,
    data3: 0x4A38,
    data4: [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A],
};

const EFI_SUCCESS: usize = 0;

// ─── Boot info passed to kernel ──────────────────────────────────────────
//
// Written to physical address 0x10000 as a linear array of u32/u64.
// Layout (offsets in u32 units, each u32 = 4 bytes):
//   [0]     = magic 0x474F5046
//   [1]     = has_fb
//   [2..3]  = fb_addr (u64, 8 bytes)
//   [4]     = fb_width
//   [5]     = fb_height
//   [6]     = fb_stride
//   [7]     = mem_upper_kb      (usable RAM above 1 MB, in KiB)
//   [8]     = memmap_present    (1 if memory map was captured at 0x20000)
//   [9..10] = memmap_size       (u64, bytes)
//   [11]    = memmap_desc_size  (bytes)
//   [12]    = memmap_desc_ver   (u32)
const BOOT_INFO_ADDR: usize = 0x10000;
const BOOT_INFO_MAGIC: u32 = 0x474F5046;
/// Physical address where a copy of the UEFI memory map is placed
/// before ExitBootServices.  The kernel can read it to discover total
/// RAM and reserved regions.
const MEMMAP_ADDR: usize = 0x20000;
const MEMMAP_MAX_SIZE: usize = 128 * 1024;

// ─── Constants ───────────────────────────────────────────────────────────

/// Physical base where the kernel binary expects to be loaded.
/// MUST match KERNEL_PHYS_BASE in platform.rs (0x100000 = 1MB).
const KERNEL_PHYS_BASE: usize = 0x10_0000;

/// High-half direct map base. MUST match DIRECT_MAP_BASE in platform.rs.
const DIRECT_MAP_BASE: usize = 0xFFFF_FFFF_8000_0000;

// ─── Page tables (allocated via UEFI AllocatePages) ────────────────────
//
// Use AllocatePages to get true physical addresses for page table pages.
// Never assume VA == PA — some non-x86 or future UEFI implementations
// may use non-identity mappings.  AllocatePages always returns a physical
// address usable directly for CR3 / PTE entries.
//
// Dual mapping like Linux's early_top_pgt:
//   PML4[0]   → identity map 0-8GB (1GB huge pages)
//   PML4[511] → high-half map DIRECT_MAP_BASE + 0-2GB (1GB huge pages)

const HUGE_PAGE_FLAGS: u64 = 0x83; // Present | Writable | PS

const EFI_ALLOCATE_ANY_PAGES: u32 = 0;
const EFI_LOADER_DATA: u32 = 2;

// ─── Minimal GDT ────────────────────────────────────────────────────────
//
// UEFI firmware typically sets CS to 0x38 (selector index 7) before
// calling efi_main.  Our GDT must have at least 8 entries so that
// the limit (8*8-1=63) covers CS=0x38.  Otherwise `lgdt` followed by
// any segment-reload (far jump, interrupt, exception) triggers #GP.
//
// We use #[repr(C, align(8))] instead of packed — the x86 lgdt
// instruction needs the descriptor operand to be naturally aligned.

#[repr(C, align(8))]
struct GdtDesc {
    limit: u16,
    base: u64,
    _pad: [u8; 6], // pad to 16 bytes for alignment
}

static BOOT_GDT: [u64; 8] = [
    0,                  // 0x00: null
    0x00209A0000000000, // 0x08: 64-bit code
    0x0000920000000000, // 0x10: data
    0, 0, 0, 0, 0,     // 0x18–0x38: unused (covers UEFI CS=0x38=selector 7)
];

/// GDT descriptor stored in a static so its address survives the CR3
/// switch (stack variables may not be identity-mapped after CR3 change).
static mut BOOT_GDT_DESC: GdtDesc = GdtDesc {
    limit: 0, base: 0,
    _pad: [0u8; 6],
};

// ─── Boot stack ─────────────────────────────────────────────────────────

const BOOT_STACK_SIZE: usize = 4096 * 16;
static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

// Memory map buffer (static, not stack-allocated — some UEFI stacks are small)
static mut MAP_BUF: [u8; 65536] = [0; 65536];

// ─── Embedded kernel binary ─────────────────────────────────────────────
//
// The kernel is built as a flat binary (stripped ELF) and embedded.
// At build time, OUT_DIR points to kernel/target/.../release/ where
// the kernel binary resides. The Makefile copies it as kernel.bin.

static KERNEL_BIN: &[u8] = include_bytes!(env!("KERNEL_BIN_PATH"));

// ─── Panic handler ──────────────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Write 'P' to COM1 to indicate panic
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x50", // 'P'
            "out dx, al",
            options(nomem, preserves_flags),
        );
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, preserves_flags));
        }
    }
}

// ─── UEFI ConOut (screen output before ExitBootServices) ────────────────

/// EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL: Reset at +0, OutputString at +8.
type EfiOutputString = extern "C" fn(*const c_void, *const u16) -> usize;

/// Global ConOut pointer — set once in efi_main, used by screen_print.
static mut CON_OUT: *const c_void = core::ptr::null();

/// Print a string to the UEFI text console (works before ExitBootServices).
/// After ExitBootServices this is invalid — use fb_print instead.
unsafe fn screen_print(s: &str) {
    if CON_OUT.is_null() {
        return;
    }
    // Convert ASCII → UCS-2 (CHAR16) into a stack buffer
    let mut buf = [0u16; 256];
    let proto = CON_OUT;
    let output_string: EfiOutputString = unsafe {
        let raw: *const c_void = *(proto as *const *const c_void).add(1);
        core::mem::transmute(raw)
    };

    // Output line by line (UEFI console handles \n poorly, use \r\n)
    for line in s.split('\n') {
        let mut i = 0;
        for &b in line.as_bytes() {
            if i >= 250 {
                break;
            }
            buf[i] = b as u16;
            i += 1;
        }
        buf[i] = 0; // null terminator
        if i > 0 {
            (output_string)(proto, buf.as_ptr());
        }
        // Output \r\n
        buf[0] = b'\r' as u16;
        buf[1] = b'\n' as u16;
        buf[2] = 0;
        (output_string)(proto, buf.as_ptr());
    }
}

/// Format GOP info string into buffer for screen output.
fn format_gop_info<'a>(
    buf: &'a mut [u8],
    addr: u64,
    w: u32,
    h: u32,
    stride: u32,
    fmt: u32,
) -> &'a str {
    let mut pos = 0;
    buf[pos..].copy_from_slice(b"fb=0x");
    pos += 5;
    for i in (0..16).rev() {
        let nib = ((addr >> (i * 4)) & 0xF) as u8;
        buf[pos] = if nib < 10 {
            b'0' + nib
        } else {
            b'a' + nib - 10
        };
        pos += 1;
    }
    buf[pos] = b' ';
    pos += 1;
    pos += write_dec(&mut buf[pos..], w as u64);
    buf[pos] = b'x';
    pos += 1;
    pos += write_dec(&mut buf[pos..], h as u64);
    buf[pos..].copy_from_slice(b" s=");
    pos += 3;
    pos += write_dec(&mut buf[pos..], stride as u64);
    buf[pos..].copy_from_slice(b"\n");
    pos += 1;
    unsafe { core::str::from_utf8_unchecked(&buf[..pos]) }
}

fn write_dec(buf: &mut [u8], val: u64) -> usize {
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut n = val;
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

// ─── Direct GOP framebuffer text (after ExitBootServices) ────────────────

/// 8x16 bitmap font (same as kernel's fb_console).
static FONT8X16: &[u8; 2048] = include_bytes!("../../kernel/src/arch/x86_64/font8x16.bin");

static mut FB_ADDR: u64 = 0;
static mut FB_PITCH: u64 = 0;
static mut FB_WIDTH: u64 = 0;
static mut FB_HEIGHT: u64 = 0;
static mut FB_CURSOR_X: usize = 0;
static mut FB_CURSOR_Y: usize = 0;

const GLYPH_W: usize = 8;
const GLYPH_H: usize = 16;

/// Draw a single character to the GOP framebuffer.
unsafe fn fb_draw_char(c: u8, col: usize, row: usize) {
    let fb = FB_ADDR as *mut u32;
    let pitch = FB_PITCH as usize / 4; // pitch in u32 units
    let idx = (c as usize & 0x7F) * GLYPH_H;
    let glyph = &FONT8X16[idx..idx + GLYPH_H];
    let px = col * GLYPH_W;
    let py = row * GLYPH_H;

    for y in 0..GLYPH_H {
        let bits = glyph[y];
        for x in 0..GLYPH_W {
            let on = (bits >> (7 - x)) & 1;
            let pixel = if on != 0 { 0xFFFFFFFF } else { 0x00000000 };
            let off = (py + y) * pitch + (px + x);
            *fb.add(off) = pixel;
        }
    }
}

/// Print a string directly to the GOP framebuffer (after ExitBootServices).
unsafe fn fb_print(s: &str) {
    if FB_ADDR == 0 {
        return;
    }
    let cols = FB_WIDTH as usize / GLYPH_W;
    let rows = FB_HEIGHT as usize / GLYPH_H;

    for &b in s.as_bytes() {
        match b {
            b'\n' => {
                FB_CURSOR_X = 0;
                FB_CURSOR_Y += 1;
            }
            _ => {
                if FB_CURSOR_X >= cols {
                    FB_CURSOR_X = 0;
                    FB_CURSOR_Y += 1;
                }
                if FB_CURSOR_Y >= rows {
                    // Scroll not implemented — just wrap
                    FB_CURSOR_Y = 0;
                }
                fb_draw_char(b, FB_CURSOR_X, FB_CURSOR_Y);
                FB_CURSOR_X += 1;
            }
        }
    }
}

/// Write a single pixel to the framebuffer (for bare-minimum debug output).
/// Returns silently if framebuffer is not available.
#[inline]
unsafe fn fb_pixel(x: usize, y: usize, color: u32) {
    let addr = FB_ADDR;
    if addr == 0 { return; }
    let p = FB_PITCH as usize / 4;
    if p == 0 { return; }
    *(addr as *mut u32).add(y * p + x) = color;
}

/// Print a hex value to the framebuffer.
unsafe fn fb_hex(val: u64) {
    if FB_ADDR == 0 {
        return;
    }
    let mut buf = [0u8; 19];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        let nibble = ((val >> ((15 - i) * 4)) & 0xF) as u8;
        buf[2 + i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + nibble - 10
        };
    }
    let s = core::str::from_utf8_unchecked(&buf);
    fb_print(s);
}

// ─── Serial debug helpers ───────────────────────────────────────────────

#[inline]
unsafe fn serial_putc(c: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") 0x3F8u16,
        in("al") c,
        options(nomem, preserves_flags),
    );
}

#[inline]
unsafe fn serial_puts(s: &str) {
    for &b in s.as_bytes() {
        serial_putc(b);
    }
}

#[inline]
unsafe fn serial_hex(val: u64) {
    serial_puts("0x");
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        serial_putc(if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + nibble - 10
        });
    }
}

#[inline]
unsafe fn serial_dec(val: u64) {
    if val == 0 {
        serial_putc(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = val;
    let mut i = 0usize;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        serial_putc(buf[i]);
    }
}

// ─── Entry point ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn efi_main(image_handle: EfiHandle, system_table: *const EfiSystemTable) -> ! {
    // ── Init screen output (UEFI ConOut) ──
    if !system_table.is_null() {
        let st = unsafe { &*system_table };
        unsafe {
            CON_OUT = st.con_out;
        }
    }
    unsafe {
        screen_print("KarteOS EFI Loader\n");
    }

    // ── Step 1: Capture GOP framebuffer ──
    // Write initial BootInfo (memory map will be added later)
    let bi = BOOT_INFO_ADDR as *mut u32;
    unsafe {
        *bi.add(0) = BOOT_INFO_MAGIC;
        *bi.add(1) = 0;  // has_fb = 0 initially
        *(bi.add(2) as *mut u64) = 0; // fb_addr
        *bi.add(4) = 0;  // fb_width
        *bi.add(5) = 0;  // fb_height
        *bi.add(6) = 0;  // fb_stride
    }

    if !system_table.is_null() {
        let st = unsafe { &*system_table };
        if !st.boot_services.is_null() {
            let bs = unsafe { &*st.boot_services };

            let mut gop: *const c_void = core::ptr::null();
            let status = (bs.locate_protocol)(&GOP_GUID, core::ptr::null(), &mut gop);

            if status == EFI_SUCCESS && !gop.is_null() {
                unsafe {
                    screen_print("GOP: OK\n");
                }
                let g = unsafe { &*(gop as *const EfiGop) };
                if !g.mode.is_null() {
                    let m = unsafe { &*g.mode };
                    if !m.info.is_null() {
                        let inf = unsafe { &*m.info };
                        unsafe {
                            bi.add(1).write_volatile(1);
                            *(bi.add(2) as *mut u64) = m.fb_base;
                            *bi.add(4) = inf.hres;
                            *bi.add(5) = inf.vres;
                            *bi.add(6) = inf.scanline * 4;
                        }

                        // Init direct framebuffer writer for post-EBS use
                        unsafe {
                            FB_ADDR = m.fb_base;
                            FB_PITCH = (inf.scanline * 4) as u64;
                            FB_WIDTH = inf.hres as u64;
                            FB_HEIGHT = inf.vres as u64;
                        }
                        unsafe {
                            screen_print("GOP: fb=0x");
                            let mut b = [0u8; 33]; let mut pos = 0;
                            for i in (0..16).rev() {
                                let nib = ((m.fb_base >> (i*4)) & 0xF) as u8;
                                b[pos] = if nib < 10 { b'0'+nib } else { b'a'+nib-10 }; pos += 1;
                            }
                            b[pos] = b' '; pos += 1;
                            // pixel format number
                            let fmt = inf.pixel_format;
                            b[pos] = b'f'; b[pos+1] = b'm'; b[pos+2] = b't'; b[pos+3] = b'='; pos += 4;
                            b[pos] = b'0' + fmt as u8; b[pos+1] = b'\n'; b[pos+2] = 0;
                            screen_print(core::str::from_utf8_unchecked(&b[..pos+2]));
                        }
                    }
                }
            } else {
                unsafe {
                    screen_print("GOP: NOT found!\n");
            }
        }
        }
    }

    // ── Copy kernel binary (BEFORE EBS so ConOut still works) ── (BEFORE EBS so ConOut still works) ──
    unsafe {
        screen_print("KERNEL: copying ");
        // print size
        let mut buf = [0u8; 32]; let mut pos = 0;
        let sz = KERNEL_BIN.len() as u64;
        if sz == 0 { buf[0] = b'0'; pos = 1; }
        else { let mut n = sz; let mut tmp = [0u8; 20]; let mut i = 0;
            while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
            while i > 0 { i -= 1; buf[pos] = tmp[i]; pos += 1; } }
        buf[pos] = b'\n'; buf[pos+1] = 0;
        screen_print(core::str::from_utf8_unchecked(&buf[..pos+1]));
    }
    unsafe {
        let dst = KERNEL_PHYS_BASE as *mut u8;
        let src = KERNEL_BIN.as_ptr();
        for i in 0..KERNEL_BIN.len() {
            *dst.add(i) = *src.add(i);
        }
    }
    unsafe { screen_print("OK\n"); }

    // Set up page tables using UEFI AllocatePages (get true physical addresses)
    let bs = unsafe { &*(*system_table).boot_services };
    let pml4_phys = unsafe { setup_page_tables(bs) };

    // Prepare boot args
    unsafe {
        *(0x8000 as *mut u32) = 0;
        *(0x8004 as *mut u32) = 0;
        // Store framebuffer address at 0x9000 so _start64 can draw
        // diagnostics BEFORE any kernel initialization (Rust, PMM, VMM).
        *(0x9000 as *mut u64) = FB_ADDR;
        *(0x9008 as *mut u64) = FB_PITCH;
    }

    // Fill the GDT descriptor in a static so its address remains valid
    // after the CR3 switch (stack variables may become unmapped).
    unsafe {
        BOOT_GDT_DESC = GdtDesc {
            limit: (BOOT_GDT.len() * 8 - 1) as u16,
            base: core::ptr::addr_of!(BOOT_GDT) as u64,
            _pad: [0u8; 6],
        };
    }

    let stack_top =
        unsafe { core::ptr::addr_of_mut!(BOOT_STACK[0]) as u64 + BOOT_STACK_SIZE as u64 };
    let start64_offset = get_start64_offset();
    let entry_high_half = (DIRECT_MAP_BASE + KERNEL_PHYS_BASE + start64_offset) as u64;

    // ── Capture UEFI memory map & fill BootInfo ──
    //
    // Even if ExitBootServices fails on some firmware, we always capture
    // the memory map FIRST so the kernel knows real RAM size and reserved
    // regions.  This replaces the hard-coded 512 MB limit.
    let ebs_ok = unsafe {
        fill_bootinfo_and_exit_boot_services(
            system_table,
            image_handle,
            FB_ADDR, FB_PITCH as u32, FB_WIDTH as u32, FB_HEIGHT as u32,
        )
    };

    // ── Switch CR3 to our own page tables ──
    //
    // After ExitBootServices, the firmware's identity mappings may have
    // been torn down (especially for high physical addresses like GPU
    // BARs at 0x4000000000).  Our PML4 has explicit identity maps for
    // 0-8 GB AND the GPU framebuffer if it's above 8 GB.  We must
    // switch CR3 BEFORE touching the framebuffer or the stack.
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
    }

    // R: red 50x50 square = BootInfo written, memory map captured
    unsafe {
        let fb = FB_ADDR as *mut u32;
        let p = FB_PITCH as usize / 4;
        for y in 0..50 { for x in 0..50 { *fb.add(y * p + x) = 0x00FF0000; } }
    }

    if !ebs_ok {
        // EBS failed — disable interrupts manually as fallback.
        // We already have the memory map in BootInfo, so the kernel can
        // still boot correctly even without EBS.
        // Y: yellow 50x50 square at (60,0) = EBS failed, using cli fallback
        unsafe {
            core::arch::asm!("cli", options(nomem, preserves_flags));
            let fb = FB_ADDR as *mut u32;
            let p = FB_PITCH as usize / 4;
            for y in 0..50 { for x in 0..50 { *fb.add(y * p + (60 + x)) = 0x00FFFF00; } }
        }
    } else {
        // B: blue 50x50 square at (60,0) = EBS succeeded
        unsafe {
            let fb = FB_ADDR as *mut u32;
            let p = FB_PITCH as usize / 4;
            for y in 0..50 { for x in 0..50 { *fb.add(y * p + (60 + x)) = 0x000000FF; } }
        }
    }

    // G: green 50x50 square at (120,0) = about to boot_transition
    unsafe {
        let fb = FB_ADDR as *mut u32;
        let p = FB_PITCH as usize / 4;
        for y in 0..50 { for x in 0..50 { *fb.add(y * p + (120 + x)) = 0x0000FF00; } }
    }

    // Jump to kernel
    unsafe {
        boot_transition(
            pml4_phys,
            stack_top,
            entry_high_half,
            core::ptr::addr_of!(BOOT_GDT_DESC) as u64,
        );
    }
    // unreachable: boot_transition is -> !
    loop { unsafe { core::arch::asm!("hlt"); } }
}

/// _start64 offset within kernel.bin, computed at build time from the
/// kernel ELF symbol table via `nm`.  Falls back to 0x1D8 if the symbol
/// cannot be found (e.g., during IDE analysis without a full build).
fn get_start64_offset() -> usize {
    option_env!("START64_OFFSET")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1D8)
}

// ─── UEFI memory map helpers ────────────────────────────────────────────

/// EFI Memory Descriptor (UEFI spec §7.2)
#[repr(C)]
struct EfiMemoryDescriptor {
    entry_type: u32,       // Type of memory region
    _pad: u32,             // padding for alignment
    phys_start: u64,       // Physical address start
    virt_start: u64,       // Virtual address start (unused before SetVirtualAddressMap)
    num_pages: u64,        // Number of 4 KiB pages
    attribute: u64,        // Memory attributes
}

// EFI memory types
const EFI_CONVENTIONAL_MEMORY: u32 = 7;

/// Fill BootInfo at 0x10000, capture the UEFI memory map, and attempt
/// ExitBootServices.  Returns true if EBS succeeded, false if the
/// kernel must use `cli` as fallback.
///
/// The memory map is ALWAYS captured (even if EBS fails) so the kernel
/// can discover real RAM size and reserved regions.
unsafe fn fill_bootinfo_and_exit_boot_services(
    system_table: *const EfiSystemTable,
    image_handle: EfiHandle,
    fb_addr: u64, fb_stride: u32, fb_width: u32, fb_height: u32,
) -> bool {
    if system_table.is_null() {
        return false;
    }
    let st = &*system_table;
    let bs = if st.boot_services.is_null() { return false; } else { &*st.boot_services };

    // Disable watchdog timer before EBS
    (bs.set_watchdog_timer)(0, 0, 0, core::ptr::null());

    // Use AllocatePool (offset 0x40 in BootServices) to get a buffer for
    // the memory map.  The static MAP_BUF may be too small on complex
    // firmware (desktop boards can have 500+ descriptors).
    let bs_ptr = bs as *const EfiBootServices as *const u8;
    let allocate_pool: extern "C" fn(u32, usize, *mut *mut u8) -> usize =
        core::mem::transmute((bs_ptr.add(0x40) as *const usize).read());

    // Query memory map size first (pass NULL buffer)
    let mut map_size: usize = 0;
    let mut map_key: usize = 0;
    let mut desc_size: usize = 0;
    let mut desc_version: u32 = 0;
    (bs.get_memory_map)(&mut map_size, core::ptr::null_mut(),
                         &mut map_key, &mut desc_size, &mut desc_version);

    // Allocate buffer with some headroom
    let alloc_size = (map_size + desc_size * 8).max(65536).min(MEMMAP_MAX_SIZE);
    let mut map_buf_ptr: *mut u8 = core::ptr::null_mut();
    let status = allocate_pool(EFI_LOADER_DATA as u32, // 2 = EfiLoaderData (NOT 0 = Reserved!)
                               alloc_size, &mut map_buf_ptr);

    // Use the allocated buffer, or fall back to static MAP_BUF.
    // CRITICAL: when falling back, cap map_size to the actual buffer size
    // to prevent buffer overflow.  Real firmware can have 500+ descriptors
    // (70KB+), far exceeding the 64KB static fallback.
    let map_buf = if status == 0 && !map_buf_ptr.is_null() {
        core::slice::from_raw_parts_mut(map_buf_ptr, alloc_size)
    } else {
        // Fall back to static buffer — must cap map_size!
        &mut MAP_BUF[..]
    };

    // Capture memory map — use actual buffer size, NOT alloc_size
    let actual_buf_size = map_buf.len();
    map_size = actual_buf_size;
    let mm_status = (bs.get_memory_map)(
        &mut map_size, map_buf.as_mut_ptr(),
        &mut map_key, &mut desc_size, &mut desc_version,
    );

    if mm_status != 0 {
        // Could not get memory map — write minimal BootInfo and skip EBS
        let bi = BOOT_INFO_ADDR as *mut u32;
        *bi.add(0) = BOOT_INFO_MAGIC;
        *bi.add(1) = if fb_addr != 0 { 1 } else { 0 };
        *(bi.add(2) as *mut u64) = fb_addr;
        *bi.add(4) = fb_width;
        *bi.add(5) = fb_height;
        *bi.add(6) = fb_stride;
        *bi.add(7) = 524288; // fallback: 512 MB
        *bi.add(8) = 0;      // no memmap
        return false;
    }

    // Parse memory map: sum ConventionalMemory, copy to MEMMAP_ADDR
    let desc_count = map_size / desc_size;
    let mut mem_upper_kb: u64 = 0;
    let dst = MEMMAP_ADDR as *mut u8;

    for i in 0..desc_count {
        let desc = &*(map_buf.as_ptr().add(i * desc_size) as *const EfiMemoryDescriptor);
        // Copy to fixed location for kernel access
        let src = desc as *const EfiMemoryDescriptor as *const u8;
        for j in 0..desc_size {
            *dst.add(i * desc_size + j) = *src.add(j);
        }
        // Conventional memory that starts at or above 1 MB counts as "upper"
        if desc.entry_type == EFI_CONVENTIONAL_MEMORY {
            let start = desc.phys_start;
            let end = start + desc.num_pages * 4096;
            if end > 0x10_0000 {
                let usable_start = start.max(0x10_0000);
                mem_upper_kb += (end - usable_start) / 1024;
            }
        }
    }

    // Write BootInfo
    let bi = BOOT_INFO_ADDR as *mut u32;
    *bi.add(0) = BOOT_INFO_MAGIC;
    *bi.add(1) = if fb_addr != 0 { 1 } else { 0 };
    *(bi.add(2) as *mut u64) = fb_addr;
    *bi.add(4) = fb_width;
    *bi.add(5) = fb_height;
    *bi.add(6) = fb_stride;
    *bi.add(7) = mem_upper_kb as u32;
    *bi.add(8) = 1;              // memmap_present
    *(bi.add(9) as *mut u64) = (desc_count * desc_size) as u64;
    *bi.add(11) = desc_size as u32;
    *bi.add(12) = desc_version;

    // ── ExitBootServices ──
    //
    // Must call IMMEDIATELY after GetMemoryMap — no UEFI service calls
    // (including ConOut, AllocatePool, etc.) are allowed in between,
    // otherwise map_key becomes stale and EBS returns EFI_INVALID_PARAMETER.
    //
    // Retry up to 3 times on EFI_INVALID_PARAMETER (map changed).
    // Linux retries indefinitely; 3 is practical for our use case.
    const EFI_INVALID_PARAMETER: usize = 2;
    let mut ebs_ok = false;

    for retry in 0..3 {
        let s = (bs.exit_boot_services)(image_handle, map_key);
        if s == 0 {
            ebs_ok = true;
            break;
        }
        if s != EFI_INVALID_PARAMETER {
            break; // different error — give up
        }
        if retry == 2 {
            break; // last retry exhausted
        }
        // Map changed — re-acquire
        map_size = map_buf.len();
        if (bs.get_memory_map)(&mut map_size, map_buf.as_mut_ptr(),
                               &mut map_key, &mut desc_size, &mut desc_version) != 0 {
            break;
        }
    }

    // Last resort: key=0 (some firmware accepts this)
    if !ebs_ok {
        let s = (bs.exit_boot_services)(image_handle, 0);
        ebs_ok = (s == 0);
    }

    ebs_ok
}

/// Set up identity-mapped and direct-mapped page tables using UEFI
/// AllocatePages.  Returns the physical address of PML4 for CR3.
///
/// The allocated pages are zeroed first (UEFI does not guarantee zeroed
/// memory), then filled with the dual-mapping entries.
unsafe fn setup_page_tables(bs: &EfiBootServices) -> u64 {
    // Allocate four 4 KiB pages for the page-table structures.
    // AllocatePages returns true physical addresses.  We cast them to
    // VA pointers for writing PTEs, which is valid because UEFI Boot
    // Services run with identity-mapped paging.
    let mut pml4_pa: u64 = 0;
    let mut pdp_id_pa: u64 = 0;
    let mut pdp_dir_pa: u64 = 0;
    let mut pdp_high_pa: u64 = 0;

    (bs.allocate_pages)(EFI_ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, 1, &mut pml4_pa);
    (bs.allocate_pages)(EFI_ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, 1, &mut pdp_id_pa);
    (bs.allocate_pages)(EFI_ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, 1, &mut pdp_dir_pa);
    (bs.allocate_pages)(EFI_ALLOCATE_ANY_PAGES, EFI_LOADER_DATA, 1, &mut pdp_high_pa);

    // Any allocation failure means CR3 will be 0 → triple fault.
    // Check BEFORE using the pointers.
    if pml4_pa == 0 || pdp_id_pa == 0 || pdp_dir_pa == 0 || pdp_high_pa == 0 {
        // Cannot continue — page table allocation failed.
        fb_print("FATAL: AllocatePages failed\n");
        loop { unsafe { core::arch::asm!("hlt"); } }
    }

    let pml4 = pml4_pa as *mut u64;
    let pdp_identity = pdp_id_pa as *mut u64;
    let pdp_direct = pdp_dir_pa as *mut u64;
    let pdp_high = pdp_high_pa as *mut u64;

    // Zero every entry — AllocatePages does NOT guarantee zeroed memory.
    for i in 0..512 {
        *pml4.add(i) = 0;
        *pdp_identity.add(i) = 0;
        *pdp_direct.add(i) = 0;
        *pdp_high.add(i) = 0;
    }

    // PML4[0] -> PDP_IDENTITY (identity map 0-8 GB)
    *pml4.add(0) = pdp_id_pa | 0x03;
    // PML4[511] -> PDP_DIRECT (direct map at DIRECT_MAP_BASE)
    *pml4.add(511) = pdp_dir_pa | 0x03;

    // Identity map: 0-128 GB using 1 GB huge pages.
    // On real hardware (16GB+ RAM), UEFI may load our PE image at high
    // physical addresses (> 8 GB).  If the identity map does not cover
    // the load address, the very next instruction fetch after `mov cr3`
    // will page-fault → double fault → triple fault → reboot.
    // 128 entries × 1 GB = 128 GB — covers all typical desktop RAM.
    for i in 0..128u64 {
        *pdp_identity.add(i as usize) = (i << 30) | HUGE_PAGE_FLAGS;
    }

    // Direct map: DIRECT_MAP_BASE covers 0-2 GB of physical RAM.
    *pdp_direct.add(510) = HUGE_PAGE_FLAGS;
    *pdp_direct.add(511) = 0x40000000u64 | HUGE_PAGE_FLAGS;

    // Identity-map the GOP framebuffer if it is above 128 GB.
    //
    // The identity map (above) covers 0-128 GB.  If the GPU framebuffer
    // is beyond that range (e.g., RTX 4070S BAR at 0x4000000000 = 256 GB),
    // we need an explicit PML4 / PDP entry.  Otherwise the first write to
    // the framebuffer after CR3 switch will page-fault.
    //
    // CRITICAL: do NOT overwrite PML4[pml4_idx] when pml4_idx == 0 —
    // PML4[0] already points to PDP_IDENTITY which covers 0-512 GB.
    // Instead, add the huge-page entry directly into PDP_IDENTITY.
    let fb = unsafe { FB_ADDR };
    if fb >= 0x20_0000_0000 { // 128 GB — beyond the identity map
        let pml4_idx = (fb >> 39) & 0x1FF;
        let pdp_idx = (fb >> 30) & 0x1FF;
        let huge_base = ((fb as u64) >> 30) << 30;
        let pte = huge_base | HUGE_PAGE_FLAGS;

        if pml4_idx == 0 {
            // Framebuffer is in the PML4[0] range (0-512 GB) but above
            // the 128 GB identity map.  Add it directly to PDP_IDENTITY.
            // Since fb >= 128 GB, pdp_idx >= 128, so there is no
            // conflict with the 0-128 GB identity entries.
            *pdp_identity.add(pdp_idx as usize) = pte;
        } else {
            // Framebuffer is above 512 GB — use a dedicated PDP table
            // behind a fresh PML4 entry.
            *pdp_high.add(pdp_idx as usize) = pte;
            *pml4.add(pml4_idx as usize) = pdp_high_pa | 0x03;
        }
    }

    pml4_pa
}
