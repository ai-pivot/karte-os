//! VirtIO PCI network device driver for x86_64.
//!
//! Supports both legacy (I/O port) and modern (MMIO) VirtIO PCI transports.
//! QEMU 8.2+ defaults to modern mode; older versions use legacy.
//! Provides `init_net_device()`, `send_raw()`, `recv_raw()`.

use core::sync::atomic::{AtomicU16, Ordering};

// ═══════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════

const VIRTIO_NET_HDR_SIZE: usize = 10;
const QUEUE_SIZE: usize = 8;
const NET_MAX_PACKET_SIZE: usize = 1514 + VIRTIO_NET_HDR_SIZE;
const BUFFER_POOL_SIZE: usize = QUEUE_SIZE * NET_MAX_PACKET_SIZE;
const PAGE: usize = 4096;

// VirtIO device status bits (modern mode)
const VIRTIO_CONFIG_S_RESET: u32 = 0;
const VIRTIO_CONFIG_S_ACKNOWLEDGE: u32 = 1;
const VIRTIO_CONFIG_S_DRIVER: u32 = 2;
const VIRTIO_CONFIG_S_FEATURES_OK: u32 = 8;
const VIRTIO_CONFIG_S_DRIVER_OK: u32 = 4;

// PCI capability types for VirtIO
const PCI_CAP_ID_VNDR: u8 = 9; // Vendor-specific capability
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// Modern common config field offsets
const COMMON_DEVICE_FEATURE_SEL: usize = 0x00;
const COMMON_DEVICE_FEATURE: usize = 0x04;
const COMMON_DRIVER_FEATURE_SEL: usize = 0x08;
const COMMON_DRIVER_FEATURE: usize = 0x0C;
const COMMON_NUM_QUEUES: usize = 0x12;
const COMMON_DEVICE_STATUS: usize = 0x14;
const COMMON_QUEUE_SELECT: usize = 0x16;
const COMMON_QUEUE_SIZE: usize = 0x18;
const COMMON_QUEUE_ENABLE: usize = 0x1C;
const COMMON_QUEUE_NOTIFY_OFF: usize = 0x1E;
const COMMON_QUEUE_DESC_LO: usize = 0x20;
const COMMON_QUEUE_DESC_HI: usize = 0x24;
const COMMON_QUEUE_AVAIL_LO: usize = 0x28;
const COMMON_QUEUE_AVAIL_HI: usize = 0x2C;
const COMMON_QUEUE_USED_LO: usize = 0x30;
const COMMON_QUEUE_USED_HI: usize = 0x34;

// ═══════════════════════════════════════════════════════════════════════
// Virtqueue structures (split format, same for legacy and modern)
// ═══════════════════════════════════════════════════════════════════════

const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct QueueMem {
    desc: [VringDesc; QUEUE_SIZE],
    avail_buf: [u8; 4 + 2 * QUEUE_SIZE + 2],
    used_buf: [u8; 6 + 8 * QUEUE_SIZE],
    data: [u8; BUFFER_POOL_SIZE],
}

impl QueueMem {
    const fn zeroed() -> Self {
        Self {
            desc: [VringDesc {
                addr: 0,
                len: 0,
                flags: 0,
                next: 0,
            }; QUEUE_SIZE],
            avail_buf: [0u8; 4 + 2 * QUEUE_SIZE + 2],
            used_buf: [0u8; 6 + 8 * QUEUE_SIZE],
            data: [0u8; BUFFER_POOL_SIZE],
        }
    }

    /// Physical address of the descriptor table
    fn desc_phys(&self) -> u64 {
        core::ptr::addr_of!(self.desc[0]) as u64
    }
    fn avail_phys(&self) -> u64 {
        core::ptr::addr_of!(self.avail_buf[0]) as u64
    }
    fn used_phys(&self) -> u64 {
        core::ptr::addr_of!(self.used_buf[0]) as u64
    }
}

static mut RX_QUEUE: QueueMem = QueueMem::zeroed();
static mut TX_QUEUE: QueueMem = QueueMem::zeroed();
static TX_NEXT_DESC: AtomicU16 = AtomicU16::new(0);
static RX_LAST_SEEN_USED: AtomicU16 = AtomicU16::new(0);

unsafe fn rx_queue() -> &'static mut QueueMem {
    core::ptr::addr_of_mut!(RX_QUEUE).as_mut().unwrap()
}
unsafe fn tx_queue() -> &'static mut QueueMem {
    core::ptr::addr_of_mut!(TX_QUEUE).as_mut().unwrap()
}

// ═══════════════════════════════════════════════════════════════════════
// Modern VirtIO PCI configuration regions
// ═══════════════════════════════════════════════════════════════════════

struct VirtioCap {
    bar: u8,
    offset: u32,
    length: u32,
    cfg_type: u8,
    // Notify-specific
    notify_off_multiplier: u32,
}

struct ModernConfig {
    common_base: usize, // MMIO address of common config
    notify_base: usize, // MMIO address of notify region
    notify_off_multiplier: u32,
    isr_base: usize,    // MMIO address of ISR config
    device_base: usize, // MMIO address of device config (MAC etc.)
}

// Global device state
struct NetDev {
    mode: NetMode,
    io_base: u16, // Legacy I/O base (0 for modern-only)
    modern: Option<&'static ModernConfig>,
}

enum NetMode {
    Legacy,
    Modern,
}

static mut MODERN_CONFIG: Option<ModernConfig> = None;
static mut NET_DEV: Option<NetDev> = None;

unsafe fn net_dev_ref() -> Option<&'static NetDev> {
    core::ptr::addr_of!(NET_DEV)
        .as_ref()
        .and_then(|o| o.as_ref())
}
unsafe fn set_net_dev(dev: NetDev) {
    core::ptr::addr_of_mut!(NET_DEV).write(Some(dev));
}

// ═══════════════════════════════════════════════════════════════════════
// I/O helpers
// ═══════════════════════════════════════════════════════════════════════

#[inline]
fn io_r32(port: u16) -> u32 {
    let val: u32;
    unsafe {
        core::arch::asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    val
}
#[inline]
fn io_w32(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("eax") val, in("dx") port, options(nomem, nostack, preserves_flags));
    }
}
#[inline]
fn io_r8(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    val
}

#[inline]
fn mmio_r32(addr: u64) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[inline]
fn mmio_w32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}
#[inline]
fn mmio_r16(addr: u64) -> u16 {
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}
#[inline]
fn mmio_w16(addr: u64, val: u16) {
    unsafe { core::ptr::write_volatile(addr as *mut u16, val) }
}
#[inline]
fn mmio_r8(addr: u64) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

// ═══════════════════════════════════════════════════════════════════════
// PCI Capability parsing
// ═══════════════════════════════════════════════════════════════════════

/// Parse VirtIO vendor-specific capabilities from PCI config space.
/// Returns Vec of found VirtIO capabilities.
fn parse_virtio_caps(bus: u8, dev: u8, func: u8) -> alloc::vec::Vec<VirtioCap> {
    let mut caps = alloc::vec::Vec::new();

    // Capabilities list starts at offset 0x34
    let cap_ptr = (crate::arch::x86_64::pci::pci_read(bus, dev, func, 0x34) & 0xFF) as u8;
    if cap_ptr == 0 {
        return caps;
    }

    let mut ptr = cap_ptr;
    for _ in 0..48 {
        // Max 48 capabilities to prevent infinite loop
        if ptr < 0x40 {
            break;
        }
        let cap_data = crate::arch::x86_64::pci::pci_read(bus, dev, func, ptr);
        let cap_id = (cap_data & 0xFF) as u8;
        let next = ((cap_data >> 8) & 0xFF) as u8;

        if cap_id == PCI_CAP_ID_VNDR {
            // Read the full VirtIO capability (at least 20 bytes)
            let cap_len = ((cap_data >> 16) & 0xFF) as u8;
            let cfg_type = ((cap_data >> 24) & 0xFF) as u8;

            if cap_len >= 16 && cfg_type >= 1 && cfg_type <= 5 {
                // VirtIO PCI cap layout:
                //   ptr+0: cap_vndr(0) cap_next(1) cap_len(2) cfg_type(3)
                //   ptr+4: bar(4) pad(5) pad(6) pad(7)
                //   ptr+8: offset(8-11) — offset within BAR
                //   ptr+12: length(12-15) — length within BAR
                //   ptr+16: notify_off_multiplier(16-19) — only for notify cap
                let bar_info = crate::arch::x86_64::pci::pci_read(bus, dev, func, ptr + 4);
                let bar = (bar_info & 0xFF) as u8;

                let offset = crate::arch::x86_64::pci::pci_read(bus, dev, func, ptr + 8);
                let length = crate::arch::x86_64::pci::pci_read(bus, dev, func, ptr + 12);

                let notify_off_mult = if cfg_type == VIRTIO_PCI_CAP_NOTIFY_CFG && cap_len >= 20 {
                    crate::arch::x86_64::pci::pci_read(bus, dev, func, ptr + 16)
                } else {
                    0
                };

                caps.push(VirtioCap {
                    bar,
                    offset,
                    length,
                    cfg_type,
                    notify_off_multiplier: notify_off_mult,
                });

                crate::console_println!(
                    "[virtio-net] Cap type={} bar={} offset={:#x} len={:#x} notify_mult={}",
                    cfg_type,
                    bar,
                    offset,
                    length,
                    notify_off_mult
                );
            }
        }

        if next == 0 {
            break;
        }
        ptr = next;
    }

    caps
}

/// Get the physical MMIO base address for a BAR
fn bar_to_mmio(bus: u8, dev: u8, func: u8, bar_idx: u8) -> usize {
    let offset = 0x10 + (bar_idx as u8) * 4;
    let bar = crate::arch::x86_64::pci::pci_read(bus, dev, func, offset);
    // Memory BAR: bit 0=0, bit 1-2 = type (0=32bit, 2=64bit), bit 3=prefetchable
    // Clear lower 4 bits for 32-bit memory BAR
    (bar & 0xFFFFFFF0) as usize
}

// ═══════════════════════════════════════════════════════════════════════
// Modern mode initialization
// ═══════════════════════════════════════════════════════════════════════

fn init_modern(bus: u8, dev: u8, func: u8, caps: &[VirtioCap]) -> Result<[u8; 6], &'static str> {
    // Find common, notify, ISR, and device config capabilities
    let mut common_cap = None;
    let mut notify_cap = None;
    let mut isr_cap = None;
    let mut device_cap = None;

    for cap in caps {
        match cap.cfg_type {
            VIRTIO_PCI_CAP_COMMON_CFG => common_cap = Some(cap),
            VIRTIO_PCI_CAP_NOTIFY_CFG => notify_cap = Some(cap),
            VIRTIO_PCI_CAP_ISR_CFG => isr_cap = Some(cap),
            VIRTIO_PCI_CAP_DEVICE_CFG => device_cap = Some(cap),
            _ => {}
        }
    }

    let common = common_cap.ok_or("no common config capability")?;
    let notify = notify_cap.ok_or("no notify capability")?;

    // Compute MMIO addresses
    let common_base = bar_to_mmio(bus, dev, func, common.bar) + common.offset as usize;
    let notify_base = bar_to_mmio(bus, dev, func, notify.bar) + notify.offset as usize;
    let isr_base = isr_cap
        .map(|c| bar_to_mmio(bus, dev, func, c.bar) + c.offset as usize)
        .unwrap_or(0);
    let device_base = device_cap
        .map(|c| bar_to_mmio(bus, dev, func, c.bar) + c.offset as usize)
        .unwrap_or(0);

    crate::console_println!(
        "[virtio-net] Modern: common={:#x} notify={:#x} isr={:#x} dev={:#x} mult={}",
        common_base,
        notify_base,
        isr_base,
        device_base,
        notify.notify_off_multiplier
    );

    // Store global modern config
    unsafe {
        core::ptr::addr_of_mut!(MODERN_CONFIG).write(Some(ModernConfig {
            common_base,
            notify_base,
            notify_off_multiplier: notify.notify_off_multiplier,
            isr_base,
            device_base,
        }));
    }

    // 1. Reset device
    mmio_w32(
        (common_base + COMMON_DEVICE_STATUS) as u64,
        VIRTIO_CONFIG_S_RESET,
    );

    // 2. Set ACKNOWLEDGE and DRIVER bits
    mmio_w32(
        (common_base + COMMON_DEVICE_STATUS) as u64,
        VIRTIO_CONFIG_S_ACKNOWLEDGE | VIRTIO_CONFIG_S_DRIVER,
    );

    // 3. Negotiate features (we only need basic networking, no offloads)
    mmio_w32((common_base + COMMON_DRIVER_FEATURE_SEL) as u64, 0);
    mmio_w32((common_base + COMMON_DRIVER_FEATURE) as u64, 0); // No extra features

    // 4. Set FEATURES_OK
    mmio_w32(
        (common_base + COMMON_DEVICE_STATUS) as u64,
        VIRTIO_CONFIG_S_ACKNOWLEDGE | VIRTIO_CONFIG_S_DRIVER | VIRTIO_CONFIG_S_FEATURES_OK,
    );

    // 5. Read back status to verify FEATURES_OK is still set
    let status = mmio_r32((common_base + COMMON_DEVICE_STATUS) as u64);
    if status & VIRTIO_CONFIG_S_FEATURES_OK == 0 {
        return Err("feature negotiation failed");
    }

    // 6. Setup RX queue (queue 0) and TX queue (queue 1)
    unsafe {
        let rx = rx_queue();
        setup_modern_queue(
            common_base,
            notify_base,
            notify.notify_off_multiplier,
            0,
            rx,
        );
        let tx = tx_queue();
        setup_modern_queue(
            common_base,
            notify_base,
            notify.notify_off_multiplier,
            1,
            tx,
        );
    }

    // 7. Set DRIVER_OK
    mmio_w32(
        (common_base + COMMON_DEVICE_STATUS) as u64,
        VIRTIO_CONFIG_S_ACKNOWLEDGE
            | VIRTIO_CONFIG_S_DRIVER
            | VIRTIO_CONFIG_S_FEATURES_OK
            | VIRTIO_CONFIG_S_DRIVER_OK,
    );

    // 8. Read MAC address from device config
    let mut mac = [0u8; 6];
    if device_base != 0 {
        for i in 0..6 {
            mac[i] = mmio_r8((device_base + i) as u64);
        }
    }

    // 9. Prepare RX buffers
    unsafe {
        prepare_rx();
    }

    Ok(mac)
}

fn setup_modern_queue(
    common_base: usize,
    notify_base: usize,
    notify_mult: u32,
    queue_idx: u16,
    mem: &QueueMem,
) {
    // Select the queue
    mmio_w16((common_base + COMMON_QUEUE_SELECT) as u64, queue_idx);

    // Read queue size
    let qsize = mmio_r16((common_base + COMMON_QUEUE_SIZE) as u64);
    crate::console_println!("[virtio-net] Queue {}: size={}", queue_idx, qsize);

    // Write queue size (we use QUEUE_SIZE or the device's max, whichever is smaller)
    let actual_size = core::cmp::min(qsize, QUEUE_SIZE as u16);
    mmio_w16((common_base + COMMON_QUEUE_SIZE) as u64, actual_size);

    // Write descriptor table address
    let desc_addr = mem.desc_phys();
    mmio_w32(
        (common_base + COMMON_QUEUE_DESC_LO) as u64,
        desc_addr as u32,
    );
    mmio_w32(
        (common_base + COMMON_QUEUE_DESC_HI) as u64,
        (desc_addr >> 32) as u32,
    );

    // Write available ring address
    let avail_addr = mem.avail_phys();
    mmio_w32(
        (common_base + COMMON_QUEUE_AVAIL_LO) as u64,
        avail_addr as u32,
    );
    mmio_w32(
        (common_base + COMMON_QUEUE_AVAIL_HI) as u64,
        (avail_addr >> 32) as u32,
    );

    // Write used ring address
    let used_addr = mem.used_phys();
    mmio_w32(
        (common_base + COMMON_QUEUE_USED_LO) as u64,
        used_addr as u32,
    );
    mmio_w32(
        (common_base + COMMON_QUEUE_USED_HI) as u64,
        (used_addr >> 32) as u32,
    );

    // Read notify offset for this queue
    let notify_off = mmio_r16((common_base + COMMON_QUEUE_NOTIFY_OFF) as u64);

    // Enable the queue
    mmio_w16((common_base + COMMON_QUEUE_ENABLE) as u64, 1);

    // Store notify address for this queue (used in send/recv)
    let notify_addr = notify_base + (notify_off as usize) * (notify_mult as usize);
    // Save notify address in the QueueMem data area at a known offset
    // (We use the last 8 bytes of the data pool for TX queue notify addr)
    if queue_idx == 0 {
        // RX queue: save notify addr
        unsafe {
            core::ptr::write_volatile(
                (rx_queue().data.as_ptr() as usize + BUFFER_POOL_SIZE - 8) as *mut u64,
                notify_addr as u64,
            );
        }
    } else {
        // TX queue: save notify addr
        unsafe {
            core::ptr::write_volatile(
                (tx_queue().data.as_ptr() as usize + BUFFER_POOL_SIZE - 8) as *mut u64,
                notify_addr as u64,
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Legacy mode initialization (fallback)
// ═══════════════════════════════════════════════════════════════════════

// Legacy register offsets
const REG_MAGIC: u16 = 0x00;
const REG_DEVID: u16 = 0x08;
const REG_DRV_FEAT: u16 = 0x20;
const REG_PFNSZ: u16 = 0x28;
const REG_QSEL: u16 = 0x30;
const REG_QMAX: u16 = 0x34;
const REG_QNUM: u16 = 0x38;
const REG_QALIGN: u16 = 0x3c;
const REG_QPFN: u16 = 0x40;
const REG_QNOTIFY: u16 = 0x50;
const REG_STATUS: u16 = 0x70;
const REG_CONFIG: u16 = 0x80;

fn init_legacy(io_base: u16) -> Result<[u8; 6], &'static str> {
    let magic = io_r32(io_base + REG_MAGIC);
    let devid = io_r32(io_base + REG_DEVID);

    crate::console_println!(
        "[virtio-net] Legacy I/O base={:#x}: magic={:#x} devid={:#x}",
        io_base,
        magic,
        devid
    );

    if magic != 0x74726976 {
        return Err("bad magic");
    }
    if devid != 1 {
        return Err("not net device");
    }

    let mut mac = [0u8; 6];
    for i in 0..6 {
        mac[i] = io_r8(io_base + REG_CONFIG + i as u16);
    }

    // VirtIO negotiation
    io_w32(io_base + REG_DRV_FEAT, 0);
    io_w32(io_base + REG_STATUS, 0);
    io_w32(io_base + REG_STATUS, 1); // ACK
    io_w32(io_base + REG_STATUS, 1 | 2); // ACK | DRIVER
    io_w32(io_base + REG_STATUS, 1 | 2 | 8); // ACK | DRIVER | FEAT_OK

    if io_r32(io_base + REG_STATUS) & 8 == 0 {
        return Err("feature negotiation failed");
    }

    io_w32(io_base + REG_PFNSZ, PAGE as u32);

    unsafe {
        let rx = rx_queue();
        setup_legacy_queue(io_base, 0, rx);
        let tx = tx_queue();
        setup_legacy_queue(io_base, 1, tx);
    }

    io_w32(io_base + REG_STATUS, 1 | 2 | 8 | 4); // ACK | DRIVER | FEAT_OK | DRV_OK

    Ok(mac)
}

fn setup_legacy_queue(io_base: u16, queue_idx: u32, mem: &QueueMem) {
    io_w32(io_base + REG_QSEL, queue_idx);
    let qmax = io_r32(io_base + REG_QMAX);
    if qmax == 0 {
        return;
    }
    let qsize = core::cmp::min(qmax as usize, QUEUE_SIZE);
    io_w32(io_base + REG_QNUM, qsize as u32);
    io_w32(io_base + REG_QALIGN, PAGE as u32);

    let pfn = (mem.desc_phys() as usize) / PAGE;
    io_w32(io_base + REG_QPFN, pfn as u32);
}

// ═══════════════════════════════════════════════════════════════════════
// RX buffer preparation
// ═══════════════════════════════════════════════════════════════════════

unsafe fn prepare_rx() {
    let mem = rx_queue();
    let avail = mem.avail_buf.as_mut_ptr();

    for i in 0..QUEUE_SIZE {
        let buf_offset = i * NET_MAX_PACKET_SIZE;
        let buf_addr = core::ptr::addr_of!(mem.data[buf_offset]) as u64;
        mem.desc[i] = VringDesc {
            addr: buf_addr,
            len: NET_MAX_PACKET_SIZE as u32,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };
    }

    // Set avail flags and index
    core::ptr::write_volatile(avail as *mut u16, 0); // flags
    core::ptr::write_volatile(avail.add(2) as *mut u16, 0); // idx
    for i in 0..QUEUE_SIZE {
        let ring = avail.add(4) as *mut u16;
        core::ptr::write_volatile(ring.add(i), i as u16);
    }
    core::ptr::write_volatile(avail.add(2) as *mut u16, QUEUE_SIZE as u16);

    // Notify the queue (legacy uses REG_QNOTIFY with queue index)
    if let Some(dev) = net_dev_ref() {
        match dev.mode {
            NetMode::Legacy => {
                io_w32(dev.io_base + REG_QNOTIFY, 0);
            }
            NetMode::Modern => {
                if let Some(cfg) = dev.modern {
                    // Read notify offset from saved data area
                    let notify_addr = core::ptr::read_volatile(
                        (rx_queue().data.as_ptr() as usize + BUFFER_POOL_SIZE - 8) as *const u64,
                    );
                    mmio_w16(notify_addr, 0);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Public API: send_raw, recv_raw
// ═══════════════════════════════════════════════════════════════════════

pub fn send_raw(data: &[u8]) {
    let dev = match unsafe { net_dev_ref() } {
        Some(d) => d,
        None => return,
    };

    let hdr = [0u8; VIRTIO_NET_HDR_SIZE];
    let total = VIRTIO_NET_HDR_SIZE + data.len();
    if total > NET_MAX_PACKET_SIZE {
        return;
    }

    unsafe {
        let mem = tx_queue();
        let desc_id = TX_NEXT_DESC.fetch_add(1, Ordering::AcqRel) as usize % QUEUE_SIZE;
        let buf_offset = desc_id * NET_MAX_PACKET_SIZE;

        // Write header
        core::ptr::copy_nonoverlapping(
            hdr.as_ptr(),
            mem.data[buf_offset..].as_mut_ptr(),
            VIRTIO_NET_HDR_SIZE,
        );
        // Write packet data
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            mem.data[buf_offset + VIRTIO_NET_HDR_SIZE..].as_mut_ptr(),
            data.len(),
        );

        mem.desc[desc_id] = VringDesc {
            addr: core::ptr::addr_of!(mem.data[buf_offset]) as u64,
            len: total as u32,
            flags: 0,
            next: 0,
        };

        // Add to avail ring
        let avail = mem.avail_buf.as_mut_ptr();
        let avail_idx = core::ptr::read_volatile(avail.add(2) as *const u16);
        let slot = (avail_idx as usize) % QUEUE_SIZE;
        let ring = avail.add(4) as *mut u16;
        core::ptr::write_volatile(ring.add(slot), desc_id as u16);
        core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));

        // Notify
        match dev.mode {
            NetMode::Legacy => {
                io_w32(dev.io_base + REG_QNOTIFY, 1); // TX queue = 1
            }
            NetMode::Modern => {
                let notify_addr = core::ptr::read_volatile(
                    (tx_queue().data.as_ptr() as usize + BUFFER_POOL_SIZE - 8) as *const u64,
                );
                mmio_w16(notify_addr, 1); // TX queue = 1
            }
        }
    }
}

pub fn recv_raw(buf: &mut [u8]) -> Option<usize> {
    let dev = match unsafe { net_dev_ref() } {
        Some(d) => d,
        None => return None,
    };

    unsafe {
        let mem = rx_queue();
        let used_ptr = mem.used_buf.as_ptr();

        let last_seen = RX_LAST_SEEN_USED.load(Ordering::Acquire);
        let used_idx = core::ptr::read_volatile(used_ptr.add(2) as *const u16);
        if last_seen == used_idx {
            return None;
        }

        let idx = last_seen as usize % QUEUE_SIZE;
        let used_elem = core::ptr::read_volatile(
            (used_ptr.add(4 + idx * 8) as *const VringUsedElem)
                .as_ref()
                .unwrap(),
        );

        let desc_id = used_elem.id as usize;
        let buf_offset = desc_id * NET_MAX_PACKET_SIZE;
        let payload_len = used_elem.len as usize;

        if payload_len < VIRTIO_NET_HDR_SIZE {
            RX_LAST_SEEN_USED.store(last_seen.wrapping_add(1), Ordering::Release);
            return None;
        }

        let pkt_len = payload_len - VIRTIO_NET_HDR_SIZE;
        let copy_len = core::cmp::min(pkt_len, buf.len());
        let pkt_data = &mem.data[buf_offset + VIRTIO_NET_HDR_SIZE..buf_offset + payload_len];
        core::ptr::copy_nonoverlapping(pkt_data.as_ptr(), buf.as_mut_ptr(), copy_len);

        RX_LAST_SEEN_USED.store(last_seen.wrapping_add(1), Ordering::Release);

        // Re-queue descriptor
        let buf_addr = &mem.data[buf_offset] as *const _ as u64;
        mem.desc[desc_id] = VringDesc {
            addr: buf_addr,
            len: NET_MAX_PACKET_SIZE as u32,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };
        let avail = mem.avail_buf.as_mut_ptr();
        let avail_idx = core::ptr::read_volatile(avail.add(2) as *mut u16);
        let slot = (avail_idx as usize) % QUEUE_SIZE;
        let ring = avail.add(4) as *mut u16;
        core::ptr::write_volatile(ring.add(slot), desc_id as u16);
        core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));

        // Re-notify RX queue
        match dev.mode {
            NetMode::Legacy => {
                io_w32(dev.io_base + REG_QNOTIFY, 0);
            }
            NetMode::Modern => {
                let notify_addr = core::ptr::read_volatile(
                    (rx_queue().data.as_ptr() as usize + BUFFER_POOL_SIZE - 8) as *const u64,
                );
                mmio_w16(notify_addr, 0);
            }
        }

        Some(copy_len)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Initialization entry point
// ═══════════════════════════════════════════════════════════════════════

pub fn init_net_device() -> Option<[u8; 6]> {
    let dev = crate::arch::x86_64::pci::find_virtio_net()?;
    crate::console_println!(
        "[virtio-net] Found PCI device did={:#x} at {:02x}:{:02x}.{}",
        dev.device_id,
        dev.bus,
        dev.device,
        dev.function
    );

    // Enable I/O space, Memory space, and Bus Master
    let cmd = crate::arch::x86_64::pci::pci_read(dev.bus, dev.device, dev.function, 0x04);
    crate::arch::x86_64::pci::pci_write(dev.bus, dev.device, dev.function, 0x04, cmd | 0x7);

    // Try modern mode first (parse PCI capabilities)
    let caps = parse_virtio_caps(dev.bus, dev.device, dev.function);
    if caps.iter().any(|c| c.cfg_type == VIRTIO_PCI_CAP_COMMON_CFG) {
        crate::console_println!("[virtio-net] Using modern (MMIO) mode");
        match init_modern(dev.bus, dev.device, dev.function, &caps) {
            Ok(mac) => {
                unsafe {
                    set_net_dev(NetDev {
                        mode: NetMode::Modern,
                        io_base: 0,
                        modern: core::ptr::addr_of!(MODERN_CONFIG)
                            .as_ref()
                            .and_then(|o| o.as_ref()),
                    });
                }
                crate::console_println!(
                    "[virtio-net] Modern init OK: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0],
                    mac[1],
                    mac[2],
                    mac[3],
                    mac[4],
                    mac[5]
                );
                return Some(mac);
            }
            Err(e) => {
                crate::console_println!("[virtio-net] Modern init failed: {}, trying legacy", e);
            }
        }
    }

    // Fall back to legacy mode
    let bar0 = crate::arch::x86_64::pci::pci_read(dev.bus, dev.device, dev.function, 0x10);
    if bar0 & 1 != 0 {
        let io_base = (bar0 & !0x3) as u16;
        crate::console_println!("[virtio-net] Trying legacy mode at I/O {:#x}", io_base);
        match init_legacy(io_base) {
            Ok(mac) => {
                unsafe {
                    set_net_dev(NetDev {
                        mode: NetMode::Legacy,
                        io_base,
                        modern: None,
                    });
                }
                crate::console_println!(
                    "[virtio-net] Legacy init OK: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0],
                    mac[1],
                    mac[2],
                    mac[3],
                    mac[4],
                    mac[5]
                );
                return Some(mac);
            }
            Err(e) => {
                crate::console_println!("[virtio-net] Legacy init failed: {}", e);
            }
        }
    }

    crate::console_println!("[virtio-net] All init modes failed");
    None
}
