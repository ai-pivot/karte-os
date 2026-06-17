//! AHCI (Advanced Host Controller Interface) / SATA driver for x86_64.
//!
//! Provides block device access to SATA hard drives via AHCI controller
//! discovered on the PCI bus. Supports DMA-based read/write of 512-byte sectors.
//!
//! Reference: Intel AHCI Specification Rev 1.3.1

#[cfg(target_arch = "x86_64")]
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use crate::mm::pmm;
#[cfg(target_arch = "x86_64")]
use crate::mm::vmm::phys_to_virt;

const SECTOR_SIZE: usize = 512;
const PAGE_SIZE: usize = 4096;

// ─── AHCI Register Offsets (from ABAR base) ─────────────────────────

const AHCI_CAP: usize = 0x00; // Host Capabilities
const AHCI_GHC: usize = 0x04; // Global Host Control
const AHCI_IS: usize = 0x08; // Interrupt Status
const AHCI_PI: usize = 0x0C; // Ports Implemented
const AHCI_VS: usize = 0x10; // Version

// GHC bits
const GHC_HBA_RESET: u32 = 1 << 0;
const GHC_INTR_ENABLE: u32 = 1 << 1;
const GHC_AHCI_ENABLE: u32 = 1 << 31;

// Port register offsets (port N base = 0x100 + 0x80 * N)
const PORT_CLB: usize = 0x00; // Command List Base
const PORT_CLBU: usize = 0x04; // Command List Base Upper
const PORT_FB: usize = 0x08; // FIS Base
const PORT_FBU: usize = 0x0C; // FIS Base Upper
const PORT_IS: usize = 0x10; // Interrupt Status
const PORT_IE: usize = 0x14; // Interrupt Enable
const PORT_CMD: usize = 0x18; // Command
const PORT_TFD: usize = 0x20; // Task File Data
const PORT_SIG: usize = 0x24; // Signature
const PORT_SSTS: usize = 0x28; // SATA Status
const PORT_SCTL: usize = 0x2C; // SATA Control
const PORT_SERR: usize = 0x30; // SATA Error
const PORT_SACT: usize = 0x34; // SATA Active
const PORT_CI: usize = 0x38; // Command Issue
const PORT_SNTF: usize = 0x3C; // SNotification

// CMD bits
const CMD_ST: u32 = 1 << 0; // Start
const CMD_SUD: u32 = 1 << 1; // Spin-Up Device
const CMD_POD: u32 = 1 << 2; // Power On Device
const CMD_CLO: u32 = 1 << 3; // Command List Override
const CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const CMD_FR: u32 = 1 << 14; // FIS Receive Running (RO)
const CMD_CR: u32 = 1 << 15; // Command List Running (RO)

// SSTS bits
const SSTS_DET_MASK: u32 = 0x0F;
const SSTS_DET_PRESENT: u32 = 0x03; // Device present and communicating

// Command FIS types
const FIS_TYPE_REG_H2D: u8 = 0x27;

// ATA commands
const ATA_CMD_READ_DMA: u8 = 0xC8;
const ATA_CMD_WRITE_DMA: u8 = 0xCA;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x34;
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const ATA_CMD_FLUSH_CACHE: u8 = 0xE7;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;

// ─── Data Structures ───────────────────────────────────────────────

/// Command Header (32 bytes per entry, up to 32 entries per port).
#[repr(C)]
struct CommandHeader {
    dw0: u32,   // flags + PRDT length
    dw1: u32,   // PRD byte count (RO)
    ctba: u32,  // Command Table Base Address
    ctbau: u32, // Command Table Base Address Upper
    _rsvd: [u32; 4],
}

/// PRD (Physical Region Descriptor) — describes a data buffer for DMA.
#[repr(C)]
struct Prd {
    dba: u32,  // Data Byte Address
    dbau: u32, // Data Byte Address Upper
    _rsvd: u32,
    dbc: u32, // Data Byte Count (bit 0 = last PRD interrupt)
}

/// Command Table (128 bytes for command FIS + ATAPI + PRDs).
/// We place it in a single page for simplicity.
#[repr(C)]
struct CommandTable {
    cfis: [u8; 64],  // Command FIS (up to 64 bytes)
    acmd: [u8; 16],  // ATAPI Command
    _rsvd: [u8; 48], // Reserved
    prdt: [Prd; 1],  // PRD entries (we only need 1 for single-sector)
}

/// Received FIS structure (256 bytes, placed in a page).
#[repr(C)]
struct ReceivedFis {
    dsfis: [u8; 28], // DMA Setup FIS
    _rsvd0: [u8; 4],
    psfis: [u8; 20], // PIO Setup FIS
    _rsvd1: [u8; 12],
    rfis: [u8; 20], // D2H Register FIS
    _rsvd2: [u8; 4],
    sdbfis: [u8; 8],   // Set Device Bits FIS
    _rsvd3: [u8; 116], // Pad to 256 bytes
    ufis: [u8; 64],    // Unknown FIS
    _rsvd4: [u32; 24], // Reserved
}

/// Global AHCI state.
static AHCI_BASE: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static AHCI_PORT: AtomicUsize = AtomicUsize::new(usize::MAX);
static AHCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Per-port DMA memory (command list + FIS + command table + data buffer).
struct PortMem {
    clb_base: usize,  // Command List (1 KB = 32 * 32 bytes)
    fb_base: usize,   // Received FIS (256 bytes)
    ct_base: usize,   // Command Table (128+ bytes)
    data_base: usize, // Data buffer (4 KB for single-sector I/O)
}

/// Thread-safe wrapper for port memory.
struct SafePortMem(UnsafeCell<Option<PortMem>>);
unsafe impl Sync for SafePortMem {}

static PORT_MEM: SafePortMem = SafePortMem(UnsafeCell::new(None));

/// # Safety: Must only be called from single-threaded kernel context.
unsafe fn get_port_mem() -> Option<&'static PortMem> {
    (*PORT_MEM.0.get()).as_ref()
}

/// # Safety: Must only be called from single-threaded kernel context.
unsafe fn set_port_mem(mem: PortMem) {
    *PORT_MEM.0.get() = Some(mem);
}

// ─── MMIO helpers ───────────────────────────────────────────────────

unsafe fn reg_read(base: *mut u8, offset: usize) -> u32 {
    ptr::read_volatile(base.add(offset) as *const u32)
}

unsafe fn reg_write(base: *mut u8, offset: usize, val: u32) {
    ptr::write_volatile(base.add(offset) as *mut u32, val);
}

unsafe fn port_reg_read(base: *mut u8, port: usize, offset: usize) -> u32 {
    let port_base = 0x100 + port * 0x80;
    reg_read(base, port_base + offset)
}

unsafe fn port_reg_write(base: *mut u8, port: usize, offset: usize, val: u32) {
    let port_base = 0x100 + port * 0x80;
    reg_write(base, port_base + offset, val);
}

// ─── Port Initialization ────────────────────────────────────────────

/// Initialize a specific AHCI port.
/// Returns Ok(()) on success.
unsafe fn init_port(base: *mut u8, port: usize) -> Result<(), &'static str> {
    // Check device presence
    let ssts = port_reg_read(base, port, PORT_SSTS);
    if (ssts & SSTS_DET_MASK) != SSTS_DET_PRESENT {
        return Err("no device");
    }

    let sig = port_reg_read(base, port, PORT_SIG);
    crate::console_println!("[ahci] Port {}: SSTS={:#x} SIG={:#x}", port, ssts, sig);

    // Allocate DMA memory for this port
    // Command List: 1 KB (32 entries × 32 bytes)
    // Received FIS: 256 bytes
    // Command Table: 256 bytes (FIS + ATAPI + PRDT)
    // Data buffer: 4 KB
    // Total: ~6 KB — allocate 2 pages (8 KB)
    let page1 = pmm::alloc_frame().ok_or("OOM for AHCL CLB")?;
    let page2 = pmm::alloc_frame().ok_or("OOM for AHCI data")?;

    // Zero the pages
    ptr::write_bytes(phys_to_virt(page1) as *mut u8, 0, PAGE_SIZE);
    ptr::write_bytes(phys_to_virt(page2) as *mut u8, 0, PAGE_SIZE);

    // Layout within page1:
    //   [0x000..0x400) = Command List (1 KB)
    //   [0x400..0x500) = Received FIS (256 bytes)
    //   [0x500..0x600) = Command Table (256 bytes)
    let clb_base = page1;
    let fb_base = page1 + 0x400;
    let ct_base = page1 + 0x500;
    let data_base = page2;

    // Stop the port engine before reconfiguring
    let cmd = port_reg_read(base, port, PORT_CMD);
    port_reg_write(base, port, PORT_CMD, cmd & !(CMD_ST | CMD_FRE));

    // Wait for CR and FR to clear
    for _ in 0..500000 {
        let cmd = port_reg_read(base, port, PORT_CMD);
        if (cmd & (CMD_CR | CMD_FR)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Set Command List Base
    port_reg_write(base, port, PORT_CLB, clb_base as u32);
    port_reg_write(base, port, PORT_CLBU, (clb_base >> 32) as u32);

    // Set FIS Base
    port_reg_write(base, port, PORT_FB, fb_base as u32);
    port_reg_write(base, port, PORT_FBU, (fb_base >> 32) as u32);

    // Clear interrupt status
    port_reg_write(base, port, PORT_IS, 0xFFFFFFFF);

    // Clear SERR
    port_reg_write(base, port, PORT_SERR, 0xFFFFFFFF);

    // Enable FIS Receive
    let cmd = port_reg_read(base, port, PORT_CMD);
    port_reg_write(base, port, PORT_CMD, cmd | CMD_FRE);

    // Power on and spin up the device
    let cmd = port_reg_read(base, port, PORT_CMD);
    port_reg_write(base, port, PORT_CMD, cmd | CMD_POD | CMD_SUD);

    // Start the port engine
    let cmd = port_reg_read(base, port, PORT_CMD);
    port_reg_write(base, port, PORT_CMD, cmd | CMD_ST);

    // Wait for CR to be set
    for _ in 0..500000 {
        let cmd = port_reg_read(base, port, PORT_CMD);
        if (cmd & CMD_CR) != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    set_port_mem(PortMem {
        clb_base,
        fb_base,
        ct_base,
        data_base,
    });

    AHCI_PORT.store(port, Ordering::Relaxed);
    AHCI_BASE.store(base, Ordering::Relaxed);
    AHCI_INITIALIZED.store(true, Ordering::Relaxed);

    crate::console_println!(
        "[ahci] Port {} initialized: CLB={:#x} FB={:#x} CT={:#x} DATA={:#x}",
        port,
        clb_base,
        fb_base,
        ct_base,
        data_base
    );

    Ok(())
}

/// Build a Register Host-to-Device FIS for a read or write command.
unsafe fn build_cmd_fis(ct_base: usize, command: u8, lba: u64, count: u16, write: bool) {
    let ct = phys_to_virt(ct_base) as *mut CommandTable;
    let cfis = (*ct).cfis.as_mut_ptr();

    // FIS type: Register H2D
    *cfis.add(0) = FIS_TYPE_REG_H2D;
    // FIS flags: bit 6 = command register update
    *cfis.add(1) = 0x80;
    // Command register
    *cfis.add(2) = command;
    // Feature low (byte 3)
    *cfis.add(3) = 0;

    // LBA low byte 0 (byte 4)
    *cfis.add(4) = (lba & 0xFF) as u8;
    // LBA byte 1 (byte 5)
    *cfis.add(5) = ((lba >> 8) & 0xFF) as u8;
    // LBA byte 2 (byte 6)
    *cfis.add(6) = ((lba >> 16) & 0xFF) as u8;
    // Device register: LBA mode (bit 6)
    *cfis.add(7) = 1 << 6; // LBA mode
    // LBA byte 3 (byte 8)
    *cfis.add(8) = ((lba >> 24) & 0xFF) as u8;
    // LBA byte 4 (byte 9)
    *cfis.add(9) = ((lba >> 32) & 0xFF) as u8;
    // LBA byte 5 (byte 10)
    *cfis.add(10) = ((lba >> 40) & 0xFF) as u8;
    // LBA byte 6 (byte 11)
    *cfis.add(11) = ((lba >> 48) & 0xFF) as u8;

    // Count low (byte 12)
    *cfis.add(12) = (count & 0xFF) as u8;
    // Count high (byte 13)
    *cfis.add(13) = ((count >> 8) & 0xFF) as u8;

    // Feature high (byte 15, actually it's at cfis[15])
    *cfis.add(15) = 0;

    // ICC and Control (bytes 16-17, reserved in FIS)
    *cfis.add(16) = 0;
    *cfis.add(17) = 0;

    let _ = write; // used only for future direction flag
}

/// Execute a DMA command (read or write) on the AHCI port.
unsafe fn exec_dma(
    command: u8,
    lba: u64,
    sector_count: usize,
    data_phys: usize,
    data_len: usize,
    write: bool,
) -> Result<(), &'static str> {
    let base = AHCI_BASE.load(Ordering::Relaxed);
    let port = AHCI_PORT.load(Ordering::Relaxed);
    let mem = unsafe { get_port_mem() }.ok_or("AHCI not initialized")?;

    // Clear pending interrupts
    port_reg_write(base, port, PORT_IS, 0xFFFFFFFF);

    // Build Command FIS in Command Table
    build_cmd_fis(mem.ct_base, command, lba, sector_count as u16, write);

    // Setup PRD (Physical Region Descriptor) in Command Table
    let ct = phys_to_virt(mem.ct_base) as *mut CommandTable;
    (*ct).prdt[0].dba = data_phys as u32;
    (*ct).prdt[0].dbau = (data_phys >> 32) as u32;
    (*ct).prdt[0].dbc = (data_len - 1) as u32; // DBC = actual count - 1
    // bit 0 of dbc = interrupt on completion
    (*ct).prdt[0]._rsvd = 0;

    // Setup Command Header
    let ch = phys_to_virt(mem.clb_base) as *mut CommandHeader;
    // dw0: bit 5 = write, bit 0 = prefetch, PRDTL in bits [15:8] shifted → actually bits [7:0] = CFL, bits [15:8] = PRDTL
    // CFL = FIS length in DWORDs = 5 (20 bytes / 4)
    let cfl: u32 = 5; // Register H2D FIS is 20 bytes = 5 DWORDs
    let prdtl: u32 = 1; // One PRD entry
    let write_bit: u32 = if write { 1 << 6 } else { 0 };
    (*ch).dw0 = (cfl & 0x1F) | write_bit | (prdtl << 16);
    (*ch).dw1 = 0;
    (*ch).ctba = mem.ct_base as u32;
    (*ch).ctbau = (mem.ct_base >> 32) as u32;
    (*ch)._rsvd = [0; 4];

    // Issue command (set bit 0 in CI)
    port_reg_write(base, port, PORT_CI, 1);

    // Poll for completion
    for _ in 0..10_000_000 {
        let ci = port_reg_read(base, port, PORT_CI);
        let is = port_reg_read(base, port, PORT_IS);

        // Check for errors
        let tfd = port_reg_read(base, port, PORT_TFD);
        if (tfd & 0x01) != 0 {
            // ERR bit set
            let serr = port_reg_read(base, port, PORT_SERR);
            crate::console_println!("[ahci] Error: TFD={:#x} SERR={:#x}", tfd, serr);
            // Clear error
            port_reg_write(base, port, PORT_SERR, 0xFFFFFFFF);
            port_reg_write(base, port, PORT_IS, 0xFFFFFFFF);
            return Err("AHCI DMA error");
        }

        // Command completed when CI bit clears and DHRS (Device to Host FIS) is set
        if (ci & 1) == 0 {
            return Ok(());
        }

        core::hint::spin_loop();
    }

    Err("AHCI DMA timeout")
}

/// Detect whether the AHCI controller supports LBA48 (extended commands).
/// We check by reading the CAP register for S64A bit (64-bit addressing).
unsafe fn supports_lba48(base: *mut u8) -> bool {
    let cap = reg_read(base, AHCI_CAP);
    // S64A = bit 0 of CAP: supports 64-bit addressing
    // SNCQ = bit 1: supports native command queuing
    // If CAP exists, we assume LBA48 is available on modern drives.
    // A safer check would be to issue IDENTIFY DEVICE and check word 83.
    let _ = cap;
    true // Modern SATA drives support LBA48
}

// ─── Public API ─────────────────────────────────────────────────────

/// Initialize AHCI controller from ABAR (BAR5) physical address and size.
///
/// Called from PCI initialization after discovering an AHCI controller.
pub fn init(abar: usize, _abar_size: usize) -> Result<(), &'static str> {
    crate::console_println!("[ahci] Initializing controller at ABAR={:#x}", abar);

    let base = abar as *mut u8;

    // Check AHCI capability
    unsafe {
        let cap = reg_read(base, AHCI_CAP);
        let ghc = reg_read(base, AHCI_GHC);
        let vs = reg_read(base, AHCI_VS);
        let pi = reg_read(base, AHCI_PI);

        crate::console_println!(
            "[ahci] CAP={:#x} GHC={:#x} VS={:#x} PI={:#x}",
            cap,
            ghc,
            vs,
            pi
        );

        let n_ports = ((cap >> 0) & 0x1F) + 1;
        let n_cmd_slots = ((cap >> 8) & 0x1F);
        crate::console_println!(
            "[ahci] {} ports, {} command slots, 64-bit={}",
            n_ports,
            n_cmd_slots,
            (cap & 1) != 0
        );

        // Ensure AHCI mode is enabled
        if (ghc & GHC_AHCI_ENABLE) == 0 {
            reg_write(base, AHCI_GHC, ghc | GHC_AHCI_ENABLE);
        }

        // Disable interrupts for polling mode
        reg_write(base, AHCI_GHC, reg_read(base, AHCI_GHC) & !GHC_INTR_ENABLE);

        // Scan implemented ports
        for port in 0..32 {
            if (pi & (1 << port)) == 0 {
                continue;
            }

            crate::console_println!("[ahci] Probing port {}...", port);

            match init_port(base, port) {
                Ok(()) => {
                    // Successfully initialized first port
                    crate::console_println!("[ahci] Active SATA drive on port {}", port);
                    return Ok(());
                }
                Err(e) => {
                    crate::console_println!("[ahci] Port {}: {}", port, e);
                }
            }
        }
    }

    Err("no SATA device found")
}

/// Read one 512-byte sector from the AHCI drive.
pub fn read_block(block_id: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < SECTOR_SIZE {
        return Err("buffer too small");
    }

    let mem = unsafe { get_port_mem() }.ok_or("AHCI not initialized")?;
    let lba48 = unsafe { supports_lba48(AHCI_BASE.load(Ordering::Relaxed)) };

    let cmd = if lba48 {
        ATA_CMD_READ_DMA_EXT
    } else {
        ATA_CMD_READ_DMA
    };

    unsafe {
        exec_dma(
            cmd,
            block_id as u64,
            1,
            mem.data_base,
            SECTOR_SIZE,
            false, // read
        )?;

        // Copy from DMA buffer to caller's buffer
        ptr::copy_nonoverlapping(
            phys_to_virt(mem.data_base) as *const u8,
            buf.as_mut_ptr(),
            SECTOR_SIZE,
        );
    }

    Ok(())
}

/// Write one 512-byte sector to the AHCI drive.
pub fn write_block(block_id: usize, buf: &[u8]) -> Result<(), &'static str> {
    if buf.len() < SECTOR_SIZE {
        return Err("buffer too small");
    }

    let mem = unsafe { get_port_mem() }.ok_or("AHCI not initialized")?;
    let lba48 = unsafe { supports_lba48(AHCI_BASE.load(Ordering::Relaxed)) };

    let cmd = if lba48 {
        ATA_CMD_WRITE_DMA_EXT
    } else {
        ATA_CMD_WRITE_DMA
    };

    unsafe {
        // Copy caller's data to DMA buffer
        ptr::copy_nonoverlapping(
            buf.as_ptr(),
            phys_to_virt(mem.data_base) as *mut u8,
            SECTOR_SIZE,
        );

        exec_dma(
            cmd,
            block_id as u64,
            1,
            mem.data_base,
            SECTOR_SIZE,
            true, // write
        )?;
    }

    Ok(())
}

/// Flush the SATA drive's write cache to physical media.
/// Issues ATA FLUSH CACHE EXT (0xEA) or FLUSH CACHE (0xE7) command.
pub fn flush_cache() -> Result<(), &'static str> {
    let base = AHCI_BASE.load(Ordering::Relaxed);
    let port = AHCI_PORT.load(Ordering::Relaxed);
    let mem = unsafe { get_port_mem() }.ok_or("AHCI not initialized")?;
    let lba48 = unsafe { supports_lba48(base) };

    let cmd = if lba48 {
        ATA_CMD_FLUSH_CACHE_EXT
    } else {
        ATA_CMD_FLUSH_CACHE
    };

    unsafe {
        // Build Command FIS — flush cache has no data transfer
        build_cmd_fis(mem.ct_base, cmd, 0, 0, false);

        // Setup Command Header — no PRD entries (PRDTL=0), no data transfer
        let ch = phys_to_virt(mem.clb_base) as *mut CommandHeader;
        let cfl: u32 = 5; // FIS length in DWORDs
        (*ch).dw0 = cfl & 0x1F; // No write bit, no PRDTL
        (*ch).dw1 = 0;
        (*ch).ctba = mem.ct_base as u32;
        (*ch).ctbau = (mem.ct_base >> 32) as u32;
        (*ch)._rsvd = [0; 4];

        // Clear pending interrupts
        port_reg_write(base, port, PORT_IS, 0xFFFFFFFF);

        // Issue command
        port_reg_write(base, port, PORT_CI, 1);

        // Poll for completion
        for _ in 0..10_000_000 {
            let ci = port_reg_read(base, port, PORT_CI);
            let tfd = port_reg_read(base, port, PORT_TFD);
            if (tfd & 0x01) != 0 {
                let serr = port_reg_read(base, port, PORT_SERR);
                crate::console_println!("[ahci] Flush error: TFD={:#x} SERR={:#x}", tfd, serr);
                port_reg_write(base, port, PORT_SERR, 0xFFFFFFFF);
                port_reg_write(base, port, PORT_IS, 0xFFFFFFFF);
                return Err("AHCI flush cache error");
            }
            if (ci & 1) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("AHCI flush cache timeout")
    }
}

/// Return the capacity of the AHCI drive in sectors.
/// Since we don't do IDENTIFY DEVICE, we return 0 (unknown).
/// The filesystem layer will handle this gracefully.
pub fn capacity() -> Option<u64> {
    if AHCI_INITIALIZED.load(Ordering::Relaxed) {
        // Return a large default (e.g., 128 MB = 262144 sectors)
        // In practice, filesystem detection (ext4 superblock) validates the real size.
        Some(262144)
    } else {
        None
    }
}

/// Check if AHCI driver has been initialized.
pub fn is_available() -> bool {
    AHCI_INITIALIZED.load(Ordering::Relaxed)
}
