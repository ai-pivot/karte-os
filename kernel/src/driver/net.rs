// kernel/src/driver/net.rs
// VirtIO Net MMIO driver for QEMU virt machine (RISC-V 64-bit)

use core::option::Option::{self, None, Some};
use core::result::Result::{self, Err, Ok};

/// VirtIO MMIO register offsets
const VIRTIO_MMIO_MAGIC: usize = 0x000;
const VIRTIO_MMIO_VERSION: usize = 0x004;
const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
#[allow(dead_code)]
const VIRTIO_MMIO_VENDOR_ID: usize = 0x00c;
const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
const VIRTIO_MMIO_GUEST_PAGE_SIZE: usize = 0x028;
const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
const VIRTIO_MMIO_QUEUE_READY: usize = 0x044;
const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
const VIRTIO_MMIO_STATUS: usize = 0x070;
const VIRTIO_MMIO_QUEUE_DESC_LOW: usize = 0x080;
const VIRTIO_MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const VIRTIO_MMIO_QUEUE_AVAIL_LOW: usize = 0x090;
const VIRTIO_MMIO_QUEUE_AVAIL_HIGH: usize = 0x094;
const VIRTIO_MMIO_QUEUE_USED_LOW: usize = 0x0a0;
const VIRTIO_MMIO_QUEUE_USED_HIGH: usize = 0x0a4;
const VIRTIO_MMIO_CONFIG: usize = 0x100;

/// VirtIO status bits
const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 0x01;
const VIRTIO_STATUS_DRIVER: u32 = 0x02;
const VIRTIO_STATUS_FEATURES_OK: u32 = 0x08;
const VIRTIO_STATUS_DRIVER_OK: u32 = 0x04;
const VIRTIO_STATUS_FAILED: u32 = 0x80;

/// VirtIO Net device ID
const VIRTIO_ID_NET: u32 = 1;

/// Expected magic value ("virt" in little-endian)
const VIRTIO_MAGIC: u32 = 0x7472_6976;

/// VirtIO MMIO base address on QEMU virt machine
const VIRTIO_MMIO_BASE: usize = 0x1000_1000;

/// Stride between consecutive VirtIO MMIO devices
const VIRTIO_MMIO_STRIDE: usize = 0x200;

/// Maximum number of VirtIO devices to probe
const VIRTIO_MAX_DEVICES: usize = 8;

/// VirtQueue descriptor flags
#[allow(dead_code)]
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

/// VirtQueue size
const QUEUE_SIZE: u16 = 8;

/// VirtIO Net header size (num_buffers + 2x offload fields = 10 bytes in legacy, 12 in modern)
/// We use the simplified 10-byte header for legacy MMIO
const VIRTIO_NET_HDR_SIZE: usize = 10;

/// Maximum packet size (standard MTU + header)
const NET_MAX_PACKET_SIZE: usize = 1514 + VIRTIO_NET_HDR_SIZE;

/// Buffer pool size for queues (queue_size * max_packet)
const BUFFER_POOL_SIZE: usize = (QUEUE_SIZE as usize) * NET_MAX_PACKET_SIZE;

// ---------------------------------------------------------------------------
// VirtQueue structures (memory layout must match VirtIO spec)
// ---------------------------------------------------------------------------

/// VirtQueue descriptor table entry
#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// VirtQueue available ring header (followed by `ring[queue_size]`)
#[allow(dead_code)]
#[repr(C)]
struct VringAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 0], // dynamically sized via pointer arithmetic
}

/// VirtQueue used ring element
#[repr(C)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

/// VirtQueue used ring header (followed by `ring[queue_size]`)
#[repr(C)]
struct VringUsed {
    flags: u16,
    idx: u16,
    ring: [VringUsedElem; 0], // dynamically sized via pointer arithmetic
}

// ---------------------------------------------------------------------------
// Memory helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Static buffer pools (linked via `spin::Mutex` to satisfy Rust 2024 rules)
// ---------------------------------------------------------------------------

/// Receive queue descriptor table, available ring, used ring, and data buffers.
struct QueueMem {
    desc: [VringDesc; QUEUE_SIZE as usize],
    // available ring: flags(2) + idx(2) + ring[QUEUE_SIZE](2*QUEUE_SIZE) + used_event(2)
    avail_buf: [u8; 4 + 2 * (QUEUE_SIZE as usize) + 2],
    // used ring: flags(2) + idx(2) + ring[QUEUE_SIZE](8*QUEUE_SIZE) + avail_event(2)
    used_buf: [u8; 4 + 8 * (QUEUE_SIZE as usize) + 2],
    /// Data buffers for packets
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
            }; QUEUE_SIZE as usize],
            avail_buf: [0u8; 4 + 2 * (QUEUE_SIZE as usize) + 2],
            used_buf: [0u8; 4 + 8 * (QUEUE_SIZE as usize) + 2],
            data: [0u8; BUFFER_POOL_SIZE],
        }
    }
}

/// Two queue memory regions: index 0 = receive, index 1 = transmit.
static RX_QUEUE_MEM: spin::Mutex<Option<QueueMem>> = spin::Mutex::new(None);
static TX_QUEUE_MEM: spin::Mutex<Option<QueueMem>> = spin::Mutex::new(None);

// ---------------------------------------------------------------------------
// VirtIONet driver
// ---------------------------------------------------------------------------

pub struct VirtIONet {
    base: usize,
    mac_addr: [u8; 6],
}

impl VirtIONet {
    /// Create a new VirtIONet bound to the given MMIO `base` address.
    pub fn new(base: usize) -> Self {
        let mut net = Self {
            base,
            mac_addr: [0u8; 6],
        };
        net.mac_addr = net.read_mac_addr();
        net
    }

    // -----------------------------------------------------------------------
    // MMIO register access
    // -----------------------------------------------------------------------

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }

    #[inline]
    fn read8(&self, offset: usize) -> u8 {
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u8) }
    }

    fn status(&self) -> u32 {
        self.read32(VIRTIO_MMIO_STATUS)
    }

    fn set_status(&self, value: u32) {
        self.write32(VIRTIO_MMIO_STATUS, value);
    }

    // -----------------------------------------------------------------------
    // Probe / discovery
    // -----------------------------------------------------------------------

    /// Scan VirtIO MMIO devices starting at 0x1000_1000 and return the first
    /// VirtIO Net device (device_id == 1), or `None`.
    pub fn probe() -> Option<Self> {
        for i in 0..VIRTIO_MAX_DEVICES {
            let base = VIRTIO_MMIO_BASE + i * VIRTIO_MMIO_STRIDE;

            let magic =
                unsafe { core::ptr::read_volatile((base + VIRTIO_MMIO_MAGIC) as *const u32) };
            if magic != VIRTIO_MAGIC {
                continue;
            }

            let version =
                unsafe { core::ptr::read_volatile((base + VIRTIO_MMIO_VERSION) as *const u32) };
            if version != 2 {
                // We expect VirtIO MMIO version 2 (legacy is 1)
                continue;
            }

            let device_id =
                unsafe { core::ptr::read_volatile((base + VIRTIO_MMIO_DEVICE_ID) as *const u32) };
            if device_id == VIRTIO_ID_NET {
                return Some(Self::new(base));
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // MAC address (from device config space)
    // -----------------------------------------------------------------------

    fn read_mac_addr(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        for i in 0..6 {
            mac[i] = self.read8(VIRTIO_MMIO_CONFIG + i);
        }
        mac
    }

    pub fn mac_addr(&self) -> [u8; 6] {
        self.mac_addr
    }

    // -----------------------------------------------------------------------
    // Device initialization (VirtIO MMIO negotiation)
    // -----------------------------------------------------------------------

    /// Perform full VirtIO initialization sequence.
    pub fn init(&mut self) {
        // Step 1: Reset device
        self.set_status(0);

        // Step 2: ACKNOWLEDGE
        self.set_status(VIRTIO_STATUS_ACKNOWLEDGE);

        // Step 3: DRIVER
        self.set_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // Step 4: Negotiate features — read device features, accept none extra
        let _device_features = self.read32(VIRTIO_MMIO_DEVICE_FEATURES);
        // We don't need any advanced features; write 0 for driver features
        self.write32(VIRTIO_MMIO_DRIVER_FEATURES, 0);

        // Step 5: FEATURES_OK
        self.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        // Verify FEATURES_OK is still set
        if self.status() & VIRTIO_STATUS_FEATURES_OK == 0 {
            crate::console_println!("[net] Feature negotiation failed");
            self.set_status(VIRTIO_STATUS_FAILED);
            return;
        }

        // Step 6: Set guest page size (4 KiB)
        self.write32(VIRTIO_MMIO_GUEST_PAGE_SIZE, 0x1000);

        // Step 6a: Configure receive queue (queue index 0)
        self.setup_queue(0, &RX_QUEUE_MEM);

        // Step 6b: Configure transmit queue (queue index 1)
        self.setup_queue(1, &TX_QUEUE_MEM);

        // Step 7: DRIVER_OK
        self.set_status(
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        crate::console_println!("[net] Device status: {:#x}", self.status());
    }

    /// Set up a single VirtQueue: allocate memory via the static pool, write
    /// descriptor / available / used ring addresses into MMIO registers.
    fn setup_queue(&self, queue_index: u32, queue_mem_lock: &spin::Mutex<Option<QueueMem>>) {
        // Select the queue
        self.write32(VIRTIO_MMIO_QUEUE_SEL, queue_index);

        let num_max = self.read32(VIRTIO_MMIO_QUEUE_NUM_MAX);
        if num_max == 0 {
            crate::console_println!("[net] Queue {} not available on device", queue_index);
            return;
        }
        crate::console_println!("[net] Queue {} max descriptors: {}", queue_index, num_max);

        // Set queue size
        self.write32(VIRTIO_MMIO_QUEUE_NUM, QUEUE_SIZE as u32);

        // Allocate queue memory
        {
            let mut guard = queue_mem_lock.lock();
            if guard.is_none() {
                *guard = Some(QueueMem::zeroed());
            }
            let mem = guard.as_ref().unwrap();

            let desc_addr = &mem.desc as *const _ as usize;
            let avail_addr = mem.avail_buf.as_ptr() as usize;
            let used_addr = mem.used_buf.as_ptr() as usize;

            // Write descriptor table address
            self.write32(VIRTIO_MMIO_QUEUE_DESC_LOW, desc_addr as u32);
            self.write32(VIRTIO_MMIO_QUEUE_DESC_HIGH, (desc_addr >> 32) as u32);

            // Write available ring address
            self.write32(VIRTIO_MMIO_QUEUE_AVAIL_LOW, avail_addr as u32);
            self.write32(VIRTIO_MMIO_QUEUE_AVAIL_HIGH, (avail_addr >> 32) as u32);

            // Write used ring address
            self.write32(VIRTIO_MMIO_QUEUE_USED_LOW, used_addr as u32);
            self.write32(VIRTIO_MMIO_QUEUE_USED_HIGH, (used_addr >> 32) as u32);

            // Mark queue ready
            self.write32(VIRTIO_MMIO_QUEUE_READY, 1);

            crate::console_println!(
                "[net] Queue {} configured: desc={:#x} avail={:#x} used={:#x}",
                queue_index,
                desc_addr,
                avail_addr,
                used_addr
            );
        }
    }

    // -----------------------------------------------------------------------
    // Packet I/O
    // -----------------------------------------------------------------------

    /// Transmit a raw Ethernet frame.
    ///
    /// The caller supplies the Ethernet payload *without* the VirtIO net header.
    /// We prepend the 10-byte header automatically.
    pub fn send_packet(&mut self, data: &[u8]) -> Result<(), ()> {
        if data.is_empty() {
            return Err(());
        }

        let total_len = VIRTIO_NET_HDR_SIZE + data.len();
        if total_len > NET_MAX_PACKET_SIZE {
            return Err(());
        }

        // Build the packet: header + payload
        let mut packet_buf: [u8; NET_MAX_PACKET_SIZE] = [0u8; NET_MAX_PACKET_SIZE];
        // VirtIO net header is all zeroes for simple transmit (no offloads)
        packet_buf[VIRTIO_NET_HDR_SIZE..total_len].copy_from_slice(data);

        // Select transmit queue (index 1)
        self.write32(VIRTIO_MMIO_QUEUE_SEL, 1);

        let mut guard = TX_QUEUE_MEM.lock();
        let mem = match guard.as_mut() {
            Some(m) => m,
            None => return Err(()),
        };

        // Find a free descriptor (simple: always use descriptor 0)
        let desc_idx: u16 = 0;

        // Copy packet into data buffer
        let buf_offset = (desc_idx as usize) * NET_MAX_PACKET_SIZE;
        mem.data[buf_offset..buf_offset + total_len].copy_from_slice(&packet_buf[..total_len]);

        // Set up descriptor
        let buf_addr = &mem.data[buf_offset] as *const _ as u64;
        mem.desc[desc_idx as usize] = VringDesc {
            addr: buf_addr,
            len: total_len as u32,
            flags: 0, // no NEXT, no WRITE (device reads from our buffer)
            next: 0,
        };

        // Add to available ring
        let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
        unsafe {
            let avail_idx = core::ptr::read_volatile(avail.add(2) as *const u16);
            let slot = (avail_idx % QUEUE_SIZE) as usize;
            // Write into ring (located at offset 4 in avail_buf)
            let ring_ptr = avail.add(4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(slot), desc_idx);
            // Advance idx
            core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));
        }

        // Notify the device
        self.write32(VIRTIO_MMIO_QUEUE_NOTIFY, 1); // queue 1 = transmit

        Ok(())
    }

    /// Attempt to receive a raw Ethernet frame.
    ///
    /// On success, returns the number of payload bytes written into `buf`
    /// (VirtIO net header is stripped automatically).
    pub fn recv_packet(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        // Select receive queue (index 0)
        self.write32(VIRTIO_MMIO_QUEUE_SEL, 0);

        let mut guard = RX_QUEUE_MEM.lock();
        let mem = match guard.as_mut() {
            Some(m) => m,
            None => return Err(()),
        };

        // Check used ring for completed descriptors
        let used = mem.used_buf.as_ptr() as *const VringUsed;
        unsafe {
            let _used_idx = core::ptr::read_volatile(&(*used).idx);
            // We track our last-seen index in the first byte of data (hacky but
            // avoids extra state). For simplicity, just check if descriptor 0
            // has been used by the device.
            let ring_base = (used as *const u8).add(4) as *const VringUsedElem;
            let elem = core::ptr::read_volatile(ring_base);
            if elem.id != 0 || elem.len == 0 {
                // No packet available yet
                return Err(());
            }

            let total_len = elem.len as usize;
            if total_len < VIRTIO_NET_HDR_SIZE || total_len > NET_MAX_PACKET_SIZE {
                return Err(());
            }

            let payload_len = total_len - VIRTIO_NET_HDR_SIZE;
            if payload_len > buf.len() {
                return Err(());
            }

            // Copy payload (skip VirtIO net header) into caller's buffer
            let data_start = VIRTIO_NET_HDR_SIZE;
            buf[..payload_len].copy_from_slice(&mem.data[data_start..total_len]);

            // Re-queue the descriptor for future receives
            let desc_idx: u16 = 0;
            let buf_offset = (desc_idx as usize) * NET_MAX_PACKET_SIZE;
            let buf_addr = &mem.data[buf_offset] as *const _ as u64;

            mem.desc[desc_idx as usize] = VringDesc {
                addr: buf_addr,
                len: NET_MAX_PACKET_SIZE as u32,
                flags: VRING_DESC_F_WRITE, // device writes into our buffer
                next: 0,
            };

            // Add back to available ring
            let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
            let avail_idx = core::ptr::read_volatile(avail.add(2) as *const u16);
            let slot = (avail_idx % QUEUE_SIZE) as usize;
            let ring_ptr = avail.add(4) as *mut u16;
            core::ptr::write_volatile(ring_ptr.add(slot), desc_idx);
            core::ptr::write_volatile(avail.add(2) as *mut u16, avail_idx.wrapping_add(1));

            // Notify device
            drop(guard);
            self.write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0); // queue 0 = receive

            Ok(payload_len)
        }
    }

    /// Prepare the receive queue with writable descriptors so the device can
    /// deliver incoming packets. Must be called after `init()`.
    pub fn prepare_rx(&mut self) {
        self.write32(VIRTIO_MMIO_QUEUE_SEL, 0);

        let mut guard = RX_QUEUE_MEM.lock();
        let mem = match guard.as_mut() {
            Some(m) => m,
            None => return,
        };

        for i in 0..(QUEUE_SIZE as usize) {
            let buf_offset = i * NET_MAX_PACKET_SIZE;
            let buf_addr = &mem.data[buf_offset] as *const _ as u64;

            mem.desc[i] = VringDesc {
                addr: buf_addr,
                len: NET_MAX_PACKET_SIZE as u32,
                flags: VRING_DESC_F_WRITE,
                next: 0,
            };
        }

        // Add all descriptors to the available ring
        let avail = mem.avail_buf.as_mut_ptr() as *mut u8;
        unsafe {
            for i in 0..(QUEUE_SIZE as usize) {
                let slot = i;
                let ring_ptr = avail.add(4) as *mut u16;
                core::ptr::write_volatile(ring_ptr.add(slot), i as u16);
            }
            core::ptr::write_volatile(avail.add(2) as *mut u16, QUEUE_SIZE);
        }

        // Notify the device
        drop(guard);
        self.write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0);
    }
}

// ---------------------------------------------------------------------------
// Test / demo entry point
// ---------------------------------------------------------------------------

/// Probe for VirtIO Net devices and print diagnostic information.
pub fn test_net() {
    crate::console_println!("[net] Probing VirtIO network devices...");
    if let Some(mut net) = VirtIONet::probe() {
        crate::console_println!(
            "[net] Found VirtIO Net at MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            net.mac_addr()[0],
            net.mac_addr()[1],
            net.mac_addr()[2],
            net.mac_addr()[3],
            net.mac_addr()[4],
            net.mac_addr()[5]
        );
        net.init();
        crate::console_println!("[net] VirtIO Net initialized");
    } else {
        crate::console_println!("[net] No VirtIO Net device found");
    }
}
