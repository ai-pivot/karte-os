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
    _pad1: [u8; 16], // offset 40: allocate_pages, free_pages
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

/// Written to physical address 0x10000. The kernel's kmain checks for
/// magic 0x474F5046 ("GOPF") to detect UEFI boot and read framebuffer info.
const BOOT_INFO_ADDR: usize = 0x10000;
const BOOT_INFO_MAGIC: u32 = 0x474F5046;

#[repr(C)]
struct BootInfo {
    magic: u32,
    has_fb: u32,
    fb_addr: u64,
    fb_width: u32,
    fb_height: u32,
    fb_stride: u32,
}

// Kernel reads BootInfo from 0x10000 via *const u32:
//   [0]=magic [1]=has_fb [2..3]=fb_addr [4]=fb_width [5]=fb_height [6]=fb_stride
#[allow(dead_code)]
const _BI: () = {
    use core::mem::offset_of;
    assert!(offset_of!(BootInfo, magic) == 0x00);
    assert!(offset_of!(BootInfo, has_fb) == 0x04);
    assert!(offset_of!(BootInfo, fb_addr) == 0x08);
    assert!(offset_of!(BootInfo, fb_width) == 0x10);
    assert!(offset_of!(BootInfo, fb_height) == 0x14);
    assert!(offset_of!(BootInfo, fb_stride) == 0x18);
};

// ─── Constants ───────────────────────────────────────────────────────────

/// Physical base where the kernel binary expects to be loaded.
/// MUST match KERNEL_PHYS_BASE in platform.rs (0x100000 = 1MB).
const KERNEL_PHYS_BASE: usize = 0x10_0000;

/// High-half direct map base. MUST match DIRECT_MAP_BASE in platform.rs.
const DIRECT_MAP_BASE: usize = 0xFFFF_FFFF_8000_0000;

// ─── Page tables (page-aligned) ──────────────────────────────────────────
//
// Dual mapping like Linux's early_top_pgt:
//   PML4[0]   → identity map 0-4GB (1GB huge pages)
//   PML4[511] → high-half map DIRECT_MAP_BASE + 0-2GB (1GB huge pages)

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut PML4: PageTable = PageTable([0; 512]);
static mut PDP_IDENTITY: PageTable = PageTable([0; 512]);
static mut PDP_DIRECT: PageTable = PageTable([0; 512]);

/// Additional PDP table for identity-mapping high framebuffer addresses
/// (e.g., AMD GPUs at 0x4000000000 = 256GB, beyond the 0-8GB identity map).
#[repr(C, align(4096))]
struct PdpHigh([u64; 512]);
static mut PDP_HIGH: PdpHigh = PdpHigh([0; 512]);

const HUGE_PAGE_FLAGS: u64 = 0x83; // Present | Writable | PS

// ─── Minimal GDT ────────────────────────────────────────────────────────

#[repr(C, packed)]
struct GdtDesc {
    limit: u16,
    base: u64,
}

static BOOT_GDT: [u64; 3] = [
    0,                  // null
    0x00209A0000000000, // 64-bit code (0x08)
    0x0000920000000000, // data (0x10)
];

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
    let bi = unsafe { &mut *(BOOT_INFO_ADDR as *mut BootInfo) };
    bi.magic = BOOT_INFO_MAGIC;
    bi.has_fb = 0;
    bi.fb_addr = 0;
    bi.fb_width = 0;
    bi.fb_height = 0;
    bi.fb_stride = 0;

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
                        bi.has_fb = 1;
                        bi.fb_addr = m.fb_base;
                        bi.fb_width = inf.hres;
                        bi.fb_height = inf.vres;
                        bi.fb_stride = inf.scanline * 4;

                        // Init direct framebuffer writer for post-EBS use
                        unsafe {
                            FB_ADDR = m.fb_base;
                            FB_PITCH = bi.fb_stride as u64;
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

    // Set up page tables
    unsafe { setup_page_tables(); }

    // Prepare boot args
    unsafe {
        *(0x8000 as *mut u32) = 0;
        *(0x8004 as *mut u32) = 0;
    }

    let gdt_desc = GdtDesc {
        limit: (BOOT_GDT.len() * 8 - 1) as u16,
        base: core::ptr::addr_of!(BOOT_GDT) as u64,
    };

    let pml4_phys = unsafe { core::ptr::addr_of!(PML4) as u64 };
    let stack_top =
        unsafe { core::ptr::addr_of_mut!(BOOT_STACK[0]) as u64 + BOOT_STACK_SIZE as u64 };
    let start64_offset = get_start64_offset();
    let entry_high_half = (DIRECT_MAP_BASE + KERNEL_PHYS_BASE + start64_offset) as u64;

    // Exit Boot Services — ConOut dies after this
    unsafe { screen_print("EXIT\n"); }

    // Simple EBS: just call with key=0. This worked on real hardware before.
    // The GetMemoryMap loop was causing crashes on some firmware.
    if !system_table.is_null() {
        let st = unsafe { &*system_table };
        if !st.boot_services.is_null() {
            let bs = unsafe { &*st.boot_services };
            // Disable watchdog
            (bs.set_watchdog_timer)(0, 0, 0, core::ptr::null());
            // Direct EBS call
            (bs.exit_boot_services)(image_handle, 0);
        }
    }

    // EBS done — jump to kernel
    unsafe {
        boot_transition(
            pml4_phys,
            stack_top,
            entry_high_half,
            core::ptr::addr_of!(gdt_desc) as u64,
        );
    }
    // unreachable: boot_transition is -> !
    loop { unsafe { core::arch::asm!("hlt"); } }
}

/// _start64 is at fixed offset 0x1D8 in the kernel binary.
fn get_start64_offset() -> usize { 0x1D8 }

/// Set up identity-mapped and direct-mapped page tables.
/// Also maps framebuffer region if it's above 4GB.
unsafe fn setup_page_tables() {
    let pml4 = core::ptr::addr_of_mut!(PML4).cast::<u64>();
    let pdp_identity = core::ptr::addr_of_mut!(PDP_IDENTITY).cast::<u64>();
    let pdp_direct = core::ptr::addr_of_mut!(PDP_DIRECT).cast::<u64>();

    // PML4[0] → PDP_IDENTITY (identity map)
    *pml4.add(0) = pdp_identity as u64 | 0x03;
    // PML4[511] → PDP_DIRECT (direct map at DIRECT_MAP_BASE)
    *pml4.add(511) = pdp_direct as u64 | 0x03;

    // Identity map: 0-8 GB using 1 GB huge pages (covers all PCI MMIO BARs
    // and GPU framebuffers on typical systems)
    for i in 0..8u64 {
        *pdp_identity.add(i as usize) = (i << 30) | HUGE_PAGE_FLAGS;
    }

    // Direct map: DIRECT_MAP_BASE covers 0-2 GB
    *pdp_direct.add(510) = HUGE_PAGE_FLAGS; // phys 0
    *pdp_direct.add(511) = 0x40000000u64 | HUGE_PAGE_FLAGS; // phys 1 GB

    // Identity-map the GOP framebuffer if it's above 8GB.
    // On modern GPUs (AMD/NVIDIA), the framebuffer can be at very high
    // physical addresses (e.g., 0x4000000000 = 256GB).
    // Must add to BOTH our static PML4 (for after boot_transition) AND
    // UEFI's active PML4 (for fb_print before CR3 switch).
    let fb = unsafe { FB_ADDR };
    if fb > 0x2_0000_0000 { // 8GB
        let pml4_idx = (fb >> 39) & 0x1FF;
        let pdp_idx = (fb >> 30) & 0x1FF;
        let pdp_high = core::ptr::addr_of_mut!(PDP_HIGH).cast::<u64>();
        let huge_base = ((fb as u64) >> 30) << 30;
        *pdp_high.add(pdp_idx as usize) = huge_base | HUGE_PAGE_FLAGS;

        // Add to our static PML4 (used after boot_transition CR3 switch).
        // NOTE: do NOT write to UEFI's active PML4 — it's read-only on
        // real firmware and causes a page fault → hang.
        *pml4.add(pml4_idx as usize) = pdp_high as u64 | 0x03;
    }
}
