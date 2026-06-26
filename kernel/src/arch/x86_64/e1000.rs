//! Intel E1000 series network driver (82540EM, I217, I219, I225, etc.)
//!
//! Uses MMIO, legacy descriptor format, polling mode (no interrupts).
//! Provides the same interface as virtio_net: `init_net_device()`,
//! `send_raw()`, `recv_raw()`.

use core::sync::atomic::{AtomicU64, Ordering};

const NET_MAX_PACKET: usize = 1536; // enough for standard ethernet frame

// ─── Register offsets ──────────────────────────────────────────────────

const REG_CTRL: usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_CTRL_EXT: usize = 0x0018;
const REG_IMS: usize = 0x00D0; // Interrupt Mask Set
const REG_RCTL: usize = 0x0100;
const REG_TCTL: usize = 0x0400;
const REG_TIPG: usize = 0x0410;

// Receive descriptor registers
const REG_RDBAL: usize = 0x2800;
const REG_RDBAH: usize = 0x2804;
const REG_RDLEN: usize = 0x2808;
const REG_RDH: usize = 0x2810;
const REG_RDT: usize = 0x2818;

// Transmit descriptor registers
const REG_TDBAL: usize = 0x3800;
const REG_TDBAH: usize = 0x3804;
const REG_TDLEN: usize = 0x3808;
const REG_TDH: usize = 0x3810;
const REG_TDT: usize = 0x3818;

// MAC address registers
const REG_RAL0: usize = 0x5400;
const REG_RAH0: usize = 0x5404;

// ─── CTRL bits ─────────────────────────────────────────────────────────
const CTRL_FD: u32 = 0x0010_0000; // Full Duplex
const CTRL_SPEED_1000: u32 = 0x0040_0000;
const CTRL_SPEED_100: u32 = 0x2000_0000;
const CTRL_SLU: u32 = 0x0000_0040; // Set Link Up
const CTRL_RST: u32 = 0x0400_0000; // Device Reset
const CTRL_PHY_RST: u32 = 0x8000_0000;

// ─── RCTL bits ─────────────────────────────────────────────────────────
const RCTL_EN: u32 = 0x0000_0002;
const RCTL_SBP: u32 = 0x0000_0004; // Store Bad Packets
const RCTL_UPE: u32 = 0x0000_0008; // Unicast Promiscuous
const RCTL_MPE: u32 = 0x0000_0010; // Multicast Promiscuous
const RCTL_LPE: u32 = 0x0000_0020; // Long Packet Enable
const RCTL_BAM: u32 = 0x0000_8000; // Broadcast Accept Mode
const RCTL_BSIZE_2048: u32 = 0x0000_0000; // Buffer size 2048
const RCTL_SECRC: u32 = 0x0400_0000; // Strip CRC

// ─── TCTL bits ─────────────────────────────────────────────────────────
const TCTL_EN: u32 = 0x0000_0002;
const TCTL_PSP: u32 = 0x0000_0008; // Pad Short Packets
const TCTL_CT_SHIFT: usize = 4;

// ─── TX descriptor CMD bits ────────────────────────────────────────────
const TX_CMD_EOP: u8 = 0x01; // End of Packet
const TX_CMD_IFCS: u8 = 0x02; // Insert FCS
const TX_CMD_RS: u8 = 0x08; // Report Status

const TX_STATUS_DD: u8 = 0x01; // Descriptor Done

// ─── RX descriptor status bits ─────────────────────────────────────────
const RX_STATUS_DD: u8 = 0x01; // Descriptor Done
const RX_STATUS_EOP: u8 = 0x02; // End of Packet

// ─── Global state ──────────────────────────────────────────────────────

/// (MMIO base physical address, buffer physical address)
/// MMIO_BASE holds the physical address returned by PCI BAR0.
static STATE: AtomicU64 = AtomicU64::new(0);

// Number of descriptors per ring
const DESC_COUNT: usize = 8;

// ─── Descriptor structures (legacy format, 16 bytes each) ──────────────

#[repr(C, align(16))]
struct RxDescriptor {
    buffer_addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, align(16))]
struct TxDescriptor {
    buffer_addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

// ─── MMIO helpers ──────────────────────────────────────────────────────

fn mmio_base() -> usize {
    STATE.load(Ordering::Relaxed) as usize
}

unsafe fn reg_read(offset: usize) -> u32 {
    core::ptr::read_volatile((mmio_base() + offset) as *const u32)
}

unsafe fn reg_write(offset: usize, val: u32) {
    core::ptr::write_volatile((mmio_base() + offset) as *mut u32, val);
}

// ─── Buffer allocation ─────────────────────────────────────────────────

/// Direct physical address to a pre-allocated buffer region.
/// On x86_64 with identity mapping, phys == virt for low memory.
static BUF_PHYS: AtomicU64 = AtomicU64::new(0);

// ─── send_raw / recv_raw ───────────────────────────────────────────────

pub fn send_raw(data: &[u8]) {
    if data.len() > NET_MAX_PACKET || mmio_base() == 0 {
        return;
    }

    unsafe {
        let tx_head = reg_read(REG_TDH) as usize;
        let tx_base = BUF_PHYS.load(Ordering::Relaxed) as usize;
        // TX descriptors are after RX descriptors: DESC_COUNT * 16
        let tx_desc_off = DESC_COUNT * 16;
        let tx_idx = tx_head % DESC_COUNT;
        let desc_addr = (tx_base + tx_desc_off + tx_idx * 16) as *mut TxDescriptor;
        let buf_addr = tx_base + tx_desc_off + DESC_COUNT * 16 + tx_idx * NET_MAX_PACKET;

        // Copy packet data to TX buffer
        core::ptr::copy_nonoverlapping(data.as_ptr(), buf_addr as *mut u8, data.len());

        // Set up TX descriptor
        (*desc_addr).buffer_addr = buf_addr as u64;
        (*desc_addr).length = data.len() as u16;
        (*desc_addr).cso = 0;
        (*desc_addr).cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
        (*desc_addr).status = 0;

        // Advance tail
        let new_tail = (tx_idx + 1) % DESC_COUNT;
        reg_write(REG_TDT, new_tail as u32);

        // Wait for completion (polling)
        for _ in 0..100_000 {
            if (*desc_addr).status & TX_STATUS_DD != 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

pub fn recv_raw(buf: &mut [u8]) -> Option<usize> {
    if mmio_base() == 0 {
        return None;
    }

    unsafe {
        let rx_tail = reg_read(REG_RDT) as usize;
        let rx_base = BUF_PHYS.load(Ordering::Relaxed) as usize;
        let next_idx = (rx_tail + 1) % DESC_COUNT;
        let desc_addr = (rx_base + next_idx * 16) as *mut RxDescriptor;
        let desc = &*desc_addr;

        if desc.status & RX_STATUS_DD == 0 {
            return None; // No packet ready
        }

        let pkt_len = desc.length as usize;
        if pkt_len > buf.len() {
            return None;
        }

        // Copy from RX buffer
        let buf_src = rx_base + DESC_COUNT * 16 + next_idx * NET_MAX_PACKET;
        core::ptr::copy_nonoverlapping(buf_src as *const u8, buf.as_mut_ptr(), pkt_len);

        // Return descriptor to device
        (*desc_addr).status = 0;
        reg_write(REG_RDT, next_idx as u32);

        Some(pkt_len)
    }
}

// ─── Initialization ────────────────────────────────────────────────────

/// Initialize E1000 device, return MAC address.
pub fn init_net_device() -> Option<[u8; 6]> {
    use crate::mm::pmm;

    let dev = crate::arch::x86_64::pci::find_e1000()?;
    crate::console_println!(
        "[e1000] Found PCI device did={:#x} at {:02x}:{:02x}.{}",
        dev.device_id,
        dev.bus,
        dev.device,
        dev.function
    );

    // Enable Bus Master, Memory Space, I/O Space
    let cmd = crate::arch::x86_64::pci::pci_read(dev.bus, dev.device, dev.function, 0x04);
    crate::arch::x86_64::pci::pci_write(
        dev.bus,
        dev.device,
        dev.function,
        0x04,
        cmd | 0x7, // Bus Master + Mem + IO
    );

    // Get BAR0 (MMIO base)
    let bar0 = crate::arch::x86_64::pci::pci_read(dev.bus, dev.device, dev.function, 0x10);
    if bar0 & 1 != 0 {
        crate::console_println!("[e1000] BAR0 is I/O space, not supported");
        return None;
    }
    let mmio_phys = (bar0 & !0xF) as usize;
    crate::console_println!("[e1000] MMIO base: {:#x}", mmio_phys);

    STATE.store(mmio_phys as u64, Ordering::Relaxed);

    let mac = unsafe {
        // ── Reset device ──
        reg_write(REG_CTRL, CTRL_RST);
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }

        // ── Read MAC address ──
        let ral = reg_read(REG_RAL0);
        let rah = reg_read(REG_RAH0);
        let mac: [u8; 6] = [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ];
        crate::console_println!(
            "[e1000] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );

        // ── Allocate DMA buffers ──
        let total = DESC_COUNT * 16 * 2 + DESC_COUNT * NET_MAX_PACKET * 2;
        let num_pages = (total + 4095) / 4096;
        let buf_phys = pmm::alloc_contiguous_frames(num_pages)
            .expect("[e1000] Failed to allocate DMA buffers");
        let buf_virt = crate::mm::vmm::phys_to_virt(buf_phys);
        BUF_PHYS.store(buf_phys as u64, Ordering::Relaxed);

        core::ptr::write_bytes(buf_virt as *mut u8, 0, total);

        // RX descriptors
        reg_write(REG_RDBAL, (buf_phys & 0xFFFF_FFFF) as u32);
        reg_write(REG_RDBAH, ((buf_phys >> 32) & 0xFFFF_FFFF) as u32);
        reg_write(REG_RDLEN, (DESC_COUNT * 16) as u32);
        reg_write(REG_RDH, 0);
        reg_write(REG_RDT, 0);

        for i in 0..DESC_COUNT {
            let desc = (buf_virt + i * 16) as *mut RxDescriptor;
            let buf_addr: u64 = (buf_phys + (DESC_COUNT * 16 * 2) + i * NET_MAX_PACKET) as u64;
            (*desc).buffer_addr = buf_addr;
            (*desc).status = 0;
        }
        reg_write(REG_RDT, (DESC_COUNT - 1) as u32);

        // TX descriptors
        let tx_desc_phys: usize = buf_phys + DESC_COUNT * 16;
        reg_write(REG_TDBAL, (tx_desc_phys & 0xFFFF_FFFF) as u32);
        reg_write(REG_TDBAH, ((tx_desc_phys >> 32) & 0xFFFF_FFFF) as u32);
        reg_write(REG_TDLEN, (DESC_COUNT * 16) as u32);
        reg_write(REG_TDH, 0);
        reg_write(REG_TDT, 0);

        for i in 0..DESC_COUNT {
            let desc = (buf_virt + DESC_COUNT * 16 + i * 16) as *mut TxDescriptor;
            (*desc).buffer_addr = 0;
            (*desc).cmd = 0;
            (*desc).status = TX_STATUS_DD;
        }

        reg_write(REG_IMS, 0);
        reg_write(
            REG_RCTL,
            RCTL_EN | RCTL_SBP | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC | RCTL_BSIZE_2048,
        );
        reg_write(REG_TCTL, TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT));

        let ctrl = reg_read(REG_CTRL);
        reg_write(REG_CTRL, ctrl | CTRL_SLU | CTRL_SPEED_1000 | CTRL_FD);

        mac
    };

    crate::console_println!("[e1000] Initialized OK");
    Some(mac)
}
