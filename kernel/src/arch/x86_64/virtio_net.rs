//! VirtIO Legacy PCI network device driver for x86_64.
//!
//! Uses I/O port BAR to access VirtIO Legacy registers, same transport
//! as virtio_blk.rs. Provides `init_net_device()`, `send_raw()`, `recv_raw()`
//! — the same API as the RISC-V MMIO driver in `driver/net.rs`.

use core::sync::atomic::{AtomicU16, Ordering};
use x86_64::instructions::port::Port;

// VirtIO Legacy register offsets
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

const STAT_ACK: u32 = 1;
const STAT_DRIVER: u32 = 2;
const STAT_FEAT_OK: u32 = 8;
const STAT_DRV_OK: u32 = 4;

const VIRTIO_NET_HDR_SIZE: usize = 10;
const QUEUE_SIZE: usize = 8;
const NET_MAX_PACKET_SIZE: usize = 1514 + VIRTIO_NET_HDR_SIZE;
const BUFFER_POOL_SIZE: usize = QUEUE_SIZE * NET_MAX_PACKET_SIZE;
const VRING_DESC_F_WRITE: u16 = 2;
const PAGE: usize = 4096;

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
    used_buf: [u8; 4 + 8 * QUEUE_SIZE + 2],
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
            used_buf: [0u8; 4 + 8 * QUEUE_SIZE + 2],
            data: [0u8; BUFFER_POOL_SIZE],
        }
    }
}

static mut RX_QUEUE: QueueMem = QueueMem::zeroed();
static mut TX_QUEUE: QueueMem = QueueMem::zeroed();
static mut NET_DEV: Option<NetDev> = None;

/// Round-robin counter for TX descriptor selection.
static TX_NEXT_DESC: AtomicU16 = AtomicU16::new(0);

/// Tracks the last processed index in the RX used ring.
static RX_LAST_SEEN_USED: AtomicU16 = AtomicU16::new(0);

struct NetDev {
    io_base: u16,
    _mac_addr: [u8; 6],
}

// Safe accessors for static mut (Rust 2024)
unsafe fn rx_queue() -> &'static mut QueueMem {
    core::ptr::addr_of_mut!(RX_QUEUE).as_mut().unwrap()
}
unsafe fn tx_queue() -> &'static mut QueueMem {
    core::ptr::addr_of_mut!(TX_QUEUE).as_mut().unwrap()
}
unsafe fn net_dev_ref() -> Option<&'static NetDev> {
    core::ptr::addr_of!(NET_DEV)
        .as_ref()
        .and_then(|o| o.as_ref())
}
unsafe fn set_net_dev(dev: NetDev) {
    core::ptr::addr_of_mut!(NET_DEV).write(Some(dev));
}

#[inline]
fn r32(base: u16, off: u16) -> u32 {
    unsafe { Port::<u32>::new(base + off).read() }
}
#[inline]
fn w32(base: u16, off: u16, val: u32) {
    unsafe { Port::<u32>::new(base + off).write(val) }
}
#[inline]
fn r8(base: u16, off: u16) -> u8 {
    unsafe { Port::<u8>::new(base + off).read() }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

fn init(io_base: u16) -> Result<[u8; 6], &'static str> {
    let magic = r32(io_base, REG_MAGIC);
    let devid = r32(io_base, REG_DEVID);

    crate::console_println!(
        "[virtio-net] I/O base={:#x}: magic={:#x} devid={:#x}",
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
        mac[i] = r8(io_base, REG_CONFIG + i as u16);
    }

    // VirtIO negotiation
    w32(io_base, REG_DRV_FEAT, 0);
    w32(io_base, REG_STATUS, 0);
    w32(io_base, REG_STATUS, STAT_ACK);
    w32(io_base, REG_STATUS, STAT_ACK | STAT_DRIVER);
    w32(io_base, REG_STATUS, STAT_ACK | STAT_DRIVER | STAT_FEAT_OK);

    if r32(io_base, REG_STATUS) & STAT_FEAT_OK == 0 {
        return Err("feature negotiation failed");
    }

    w32(io_base, REG_PFNSZ, PAGE as u32);

    // Setup queues
    unsafe {
        let rx = rx_queue();
        setup_queue(io_base, 0, rx);
        let tx = tx_queue();
        setup_queue(io_base, 1, tx);
    }

    w32(
        io_base,
        REG_STATUS,
        STAT_ACK | STAT_DRIVER | STAT_FEAT_OK | STAT_DRV_OK,
    );

    crate::console_println!(
        "[virtio-net] OK: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, io={:#x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        io_base
    );

    unsafe {
        set_net_dev(NetDev {
            io_base,
            _mac_addr: mac,
        });
    }

    prepare_rx();

    Ok(mac)
}

fn setup_queue(io_base: u16, queue_idx: u32, mem: &QueueMem) {
    w32(io_base, REG_QSEL, queue_idx);
    let qmax = r32(io_base, REG_QMAX) as usize;
    if qmax == 0 {
        crate::console_println!("[virtio-net] Queue {} not available", queue_idx);
        return;
    }
    let qs = core::cmp::min(qmax, QUEUE_SIZE);
    w32(io_base, REG_QNUM, qs as u32);
    w32(io_base, REG_QALIGN, PAGE as u32);
    let base_addr = &mem.desc as *const _ as usize;
    w32(io_base, REG_QPFN, (base_addr >> 12) as u32);
    crate::console_println!("[virtio-net] Queue {}: base={:#x}", queue_idx, base_addr);
}

fn prepare_rx() {
    unsafe {
        let mem = rx_queue();
        for i in 0..QUEUE_SIZE {
            let buf_offset = i * NET_MAX_PACKET_SIZE;
            let buf_addr = &mem.data[buf_offset] as *const _ as u64;
            mem.desc[i] = VringDesc {
                addr: buf_addr,
                len: NET_MAX_PACKET_SIZE as u32,
                flags: VRING_DESC_F_WRITE,
                next: 0,
            };
            let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
            let avail_idx = core::ptr::read_volatile(avail.add(2) as *mut u16);
            let slot = (avail_idx as usize) % QUEUE_SIZE;
            let ring_ptr = avail.add(4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(slot), i as u16);
            core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));
        }
        if let Some(dev) = net_dev_ref() {
            w32(dev.io_base, REG_QNOTIFY, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Send a raw Ethernet frame.
pub fn send_raw(data: &[u8]) {
    let io_base = match unsafe { net_dev_ref() } {
        Some(d) => d.io_base,
        None => return,
    };
    let total_len = VIRTIO_NET_HDR_SIZE + data.len();
    if total_len > NET_MAX_PACKET_SIZE || data.is_empty() {
        return;
    }

    unsafe {
        let mem = tx_queue();

        // Round-robin TX descriptor selection
        let desc_idx = TX_NEXT_DESC.fetch_add(1, Ordering::Relaxed) % (QUEUE_SIZE as u16);
        let desc_idx = desc_idx as usize;
        let buf_offset = desc_idx * NET_MAX_PACKET_SIZE;
        mem.data[buf_offset..buf_offset + VIRTIO_NET_HDR_SIZE].fill(0);
        mem.data[buf_offset + VIRTIO_NET_HDR_SIZE..buf_offset + total_len].copy_from_slice(data);
        let buf_addr = &mem.data[buf_offset] as *const _ as u64;
        mem.desc[desc_idx] = VringDesc {
            addr: buf_addr,
            len: total_len as u32,
            flags: 0,
            next: 0,
        };
        let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
        let avail_idx = core::ptr::read_volatile(avail.add(2) as *mut u16);
        let slot = (avail_idx as usize) % QUEUE_SIZE;
        let ring_ptr = avail.add(4) as *mut u16;
        core::ptr::write_volatile(ring_ptr.add(slot), desc_idx as u16);
        core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));
    }
    w32(io_base, REG_QNOTIFY, 1);
}

/// Receive a raw Ethernet frame. Returns Some(len) on success, None if no packet.
pub fn recv_raw(buf: &mut [u8]) -> Option<usize> {
    let io_base = unsafe { net_dev_ref() }?.io_base;
    unsafe {
        let mem = rx_queue();
        let used_ptr = mem.used_buf.as_ptr() as *const u8;

        // Track used ring index properly
        let last_seen = RX_LAST_SEEN_USED.load(Ordering::Acquire);
        let used_idx = core::ptr::read_volatile(used_ptr.add(2) as *mut u16);
        if used_idx == last_seen {
            return None; // no new completed descriptors
        }

        let slot = (last_seen as usize) % QUEUE_SIZE;
        let ring_base = used_ptr.add(4) as *const VringUsedElem;
        let elem = core::ptr::read_volatile(ring_base.add(slot));

        let total_len = elem.len as usize;
        if total_len < VIRTIO_NET_HDR_SIZE || total_len > NET_MAX_PACKET_SIZE {
            // Advance past invalid entry
            RX_LAST_SEEN_USED.store(last_seen.wrapping_add(1), Ordering::Release);
            return None;
        }
        let payload_len = total_len - VIRTIO_NET_HDR_SIZE;
        if payload_len > buf.len() {
            RX_LAST_SEEN_USED.store(last_seen.wrapping_add(1), Ordering::Release);
            return None;
        }

        let desc_id = elem.id as usize;
        buf[..payload_len].copy_from_slice(&mem.data[VIRTIO_NET_HDR_SIZE..total_len]);

        // Do NOT clear the used entry — VirtIO used ring is device-writes / driver-reads.
        // Just advance our tracking index.
        RX_LAST_SEEN_USED.store(last_seen.wrapping_add(1), Ordering::Release);

        // Re-queue descriptor
        let buf_offset = desc_id * NET_MAX_PACKET_SIZE;
        let buf_addr = &mem.data[buf_offset] as *const _ as u64;
        mem.desc[desc_id] = VringDesc {
            addr: buf_addr,
            len: NET_MAX_PACKET_SIZE as u32,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };
        let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
        let avail_idx = core::ptr::read_volatile(avail.add(2) as *mut u16);
        let slot = (avail_idx as usize) % QUEUE_SIZE;
        let ring_ptr = avail.add(4) as *mut u16;
        core::ptr::write_volatile(ring_ptr.add(slot), desc_id as u16);
        core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));
        w32(io_base, REG_QNOTIFY, 0);
        Some(payload_len)
    }
}

/// Probe PCI bus for VirtIO Net device and initialize it.
pub fn init_net_device() -> Option<[u8; 6]> {
    let dev = crate::arch::x86_64::pci::find_virtio_net()?;
    let bar0 = dev.bars[0];
    if bar0 & 1 == 0 {
        crate::console_println!("[virtio-net] BAR0 is not I/O space");
        return None;
    }
    let io_base = (bar0 & !0x3) as u16;
    match init(io_base) {
        Ok(mac) => Some(mac),
        Err(e) => {
            crate::console_println!("[virtio-net] Init failed: {}", e);
            None
        }
    }
}
