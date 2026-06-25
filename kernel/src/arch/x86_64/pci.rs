//! PCI bus enumeration for x86_64.
//!
//! Uses I/O port-based configuration space access (0xCF8/0xCFC).
//! Enumerates all PCI devices and finds VirtIO block devices.

use alloc::vec::Vec;
use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

const PCI_VENDOR_VIRTIO: u16 = 0x1AF4;
const PCI_DEVICE_VIRTIO_BLK: u16 = 0x1001;
const PCI_DEVICE_VIRTIO_BLK_MODERN: u16 = 0x1042;
const PCI_DEVICE_VIRTIO_NET: u16 = 0x1000;
const PCI_DEVICE_VIRTIO_NET_MODERN: u16 = 0x1041;

/// Read a 32-bit value from PCI configuration space.
pub fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let addr: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | (((device as u32) & 0x1F) << 11)
        | (((function as u32) & 0x07) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDR);
        addr_port.write(addr);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);
        data_port.read()
    }
}

/// Write a 32-bit value to PCI configuration space.
pub fn pci_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let addr: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | (((device as u32) & 0x1F) << 11)
        | (((function as u32) & 0x07) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDR);
        addr_port.write(addr);
        let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);
        data_port.write(value);
    }
}

/// PCI device information.
#[derive(Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub irq_pin: u8,
    pub irq_line: u8,
    pub bars: [u32; 6],
}

impl PciDevice {
    fn from_bus_dev_fn(bus: u8, device: u8, function: u8) -> Option<Self> {
        let vendor_device = pci_read(bus, device, function, 0);
        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        // 0xFFFF means no device
        if vendor_id == 0xFFFF {
            return None;
        }

        let class_rev = pci_read(bus, device, function, 8);
        let revision = (class_rev & 0xFF) as u8;
        let prog_if = ((class_rev >> 8) & 0xFF) as u8;
        let subclass = ((class_rev >> 16) & 0xFF) as u8;
        let class_code = ((class_rev >> 24) & 0xFF) as u8;

        let irq_info = pci_read(bus, device, function, 0x3C);
        let irq_line = (irq_info & 0xFF) as u8;
        let irq_pin = ((irq_info >> 8) & 0xFF) as u8;

        let mut bars = [0u32; 6];
        for i in 0..6 {
            bars[i] = pci_read(bus, device, function, 0x10 + (i as u8) * 4);
        }

        Some(PciDevice {
            bus,
            device,
            function,
            vendor_id,
            device_id,
            class_code,
            subclass,
            prog_if,
            revision,
            irq_pin,
            irq_line,
            bars,
        })
    }

    /// Enable the device (set bus master, I/O space, memory space).
    pub fn enable(&self) {
        let cmd = pci_read(self.bus, self.device, self.function, 4);
        // Set Bus Master | Memory Space | I/O Space
        pci_write(self.bus, self.device, self.function, 4, cmd | 0x7);
    }

    /// Get the BAR address (mask out flags).
    pub fn bar_address(&self, index: usize) -> u64 {
        let bar = self.bars[index];
        if bar & 1 == 1 {
            // I/O space BAR
            (bar & 0xFFFFFFFC) as u64
        } else {
            // Memory space BAR
            let base = (bar & 0xFFFFFFF0) as u64;
            // Check if 64-bit (bits 2:1 = 0x2)
            if (bar & 0x6) == 0x4 && index + 1 < 6 {
                let upper = self.bars[index + 1] as u64;
                base | (upper << 32)
            } else {
                base
            }
        }
    }

    /// Get BAR size by writing all 1s and reading back.
    pub fn bar_size(&self, index: usize) -> u64 {
        let original = self.bars[index];
        pci_write(
            self.bus,
            self.device,
            self.function,
            (0x10 + index as u8 * 4) as u8,
            0xFFFFFFFF,
        );
        let readback = pci_read(self.bus, self.device, self.function, 0x10 + index as u8 * 4);
        pci_write(
            self.bus,
            self.device,
            self.function,
            (0x10 + index as u8 * 4) as u8,
            original,
        );

        if original & 1 == 1 {
            // I/O space
            (!(readback & 0xFFFFFFFC) as u64) + 1
        } else {
            // Memory space
            let size = (!(readback & 0xFFFFFFF0) as u64) + 1;
            // For 64-bit BARs, include upper bits
            if (original & 0x6) == 0x4 && index + 1 < 6 {
                let orig_upper = self.bars[index + 1];
                pci_write(
                    self.bus,
                    self.device,
                    self.function,
                    (0x10 + (index + 1) as u8 * 4) as u8,
                    0xFFFFFFFF,
                );
                let upper_readback = pci_read(
                    self.bus,
                    self.device,
                    self.function,
                    0x10 + (index + 1) as u8 * 4,
                );
                pci_write(
                    self.bus,
                    self.device,
                    self.function,
                    (0x10 + (index + 1) as u8 * 4) as u8,
                    orig_upper,
                );
                let upper_size = (!(upper_readback as u64)) << 32;
                size | upper_size
            } else {
                size
            }
        }
    }
}

/// Recursively enumerate PCI devices on all buses reachable from `bus`.
/// Scans functions 0-7 on multi-function devices and follows
/// PCI-to-PCI bridges into secondary buses.
fn enumerate_bus(bus: u8, devices: &mut Vec<PciDevice>, scanned_buses: &mut [bool; 256]) {
    if scanned_buses[bus as usize] {
        return;
    }
    scanned_buses[bus as usize] = true;

    for device in 0..32 {
        // Check function 0 first
        if let Some(dev) = PciDevice::from_bus_dev_fn(bus, device, 0) {
            let header_type_raw = pci_read(bus, device, 0, 0x0C);
            let header_type = ((header_type_raw >> 16) & 0x7F) as u8;
            let multi_function = (header_type_raw >> 23) & 1 == 1;

            devices.push(dev);

            // If this is a PCI-to-PCI bridge, scan its secondary bus
            if header_type == 0x01 {
                let secondary = ((pci_read(bus, device, 0, 0x18) >> 8) & 0xFF) as u8;
                if secondary != 0 && secondary != bus {
                    enumerate_bus(secondary, devices, scanned_buses);
                }
            }

            if multi_function {
                for function in 1..8 {
                    if let Some(dev) = PciDevice::from_bus_dev_fn(bus, device, function) {
                        let ht = ((pci_read(bus, device, function, 0x0C) >> 16) & 0x7F) as u8;
                        devices.push(dev);

                        if ht == 0x01 {
                            let secondary = ((pci_read(bus, device, function, 0x18) >> 8) & 0xFF) as u8;
                            if secondary != 0 && secondary != bus {
                                enumerate_bus(secondary, devices, scanned_buses);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Enumerate all PCI devices on all buses (recursive bridge traversal).
pub fn enumerate() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    let mut scanned = [false; 256];
    enumerate_bus(0, &mut devices, &mut scanned);
    devices
}

/// Find the first VirtIO block device on any bus.
pub fn find_virtio_blk() -> Option<PciDevice> {
    enumerate().into_iter().find(|dev| {
        dev.vendor_id == PCI_VENDOR_VIRTIO
            && (dev.device_id == PCI_DEVICE_VIRTIO_BLK
                || dev.device_id == PCI_DEVICE_VIRTIO_BLK_MODERN)
    })
}

/// Find the first VirtIO Net device (legacy 0x1000 or modern 0x1041).
pub fn find_virtio_net() -> Option<PciDevice> {
    enumerate().into_iter().find(|dev| {
        dev.vendor_id == PCI_VENDOR_VIRTIO
            && (dev.device_id == PCI_DEVICE_VIRTIO_NET
                || dev.device_id == PCI_DEVICE_VIRTIO_NET_MODERN)
    })
}

/// Find the first AHCI (SATA) controller on any bus.
/// AHCI: class_code = 0x01 (Mass Storage), subclass = 0x06 (SATA), prog_if = 0x01 (AHCI 1.0).
pub fn find_ahci() -> Option<PciDevice> {
    enumerate().into_iter().find(|dev| {
        dev.class_code == 0x01 && dev.subclass == 0x06 && dev.prog_if == 0x01
    })
}

/// Find an Intel E1000 series network card on any bus.
/// Supports 82540EM (QEMU default) and common I2xx variants.
pub fn find_e1000() -> Option<PciDevice> {
    const E1000_IDS: &[u16] = &[
        0x100E, 0x100F, 0x10EA, 0x1502, 0x1503,
        0x153A, 0x153B, 0x15B8, 0x15B7, 0x15F3,
    ];
    const INTEL_VENDOR: u16 = 0x8086;
    enumerate().into_iter().find(|dev| {
        dev.vendor_id == INTEL_VENDOR && E1000_IDS.contains(&dev.device_id)
    })
}

/// Find an XHCI (USB 3.0) host controller on any bus.
/// PCI class 0x0C (Serial Bus), subclass 0x03 (USB), progif 0x30 (XHCI).
pub fn find_xhci() -> Option<PciDevice> {
    enumerate().into_iter().find(|dev| {
        dev.class_code == 0x0C && dev.subclass == 0x03 && dev.prog_if == 0x30
    })
}

/// Find an NVMe controller on any bus.
pub fn find_nvme() -> Option<PciDevice> {
    enumerate().into_iter().find(|dev| {
        dev.class_code == 0x01 && dev.subclass == 0x08 && dev.prog_if == 0x02
    })
}

/// Initialize PCI and find block / network devices.
pub fn init() {
    crate::console_println!("[pci] Enumerating PCI devices...");
    let devices = enumerate();

    for dev in &devices {
        crate::console_println!(
            "[pci] {:02x}:{:02x}.{:x} vendor={:#x} device={:#x} class={:#x}/{:#x}",
            dev.bus, dev.device, dev.function,
            dev.vendor_id, dev.device_id, dev.class_code, dev.subclass
        );

        // Identity-map BARs above 4 GB.  The kernel's new page tables
        // (set up by vmm::init) only identity-map 0-4 GB.  On real
        // hardware, XHCI, AHCI, NVMe and GPU BARs are often placed at
        // high physical addresses (32 GB+).  Without an explicit
        // identity mapping, the first MMIO access to these BARs
        // triggers a page fault.
        for i in 0..6 {
            let bar = dev.bar_address(i);
            // Memory BARs are ≥ 4 KB; skip I/O BARs (< 64 KB) and zero
            if bar >= 0x1000 {
                // Map 2 MB (minimum huge-page size), aligned down
                let base = (bar as usize) & !0x1F_FFFF;
                let root = crate::mm::vmm::get_kernel_page_table();
                crate::mm::vmm::identity_map_region(root, base, 0x20_0000, crate::mm::vmm::PTEFlags::KRW_MMIO);
            }
        }
    }
    crate::console_println!("[pci] Found {} devices", devices.len());
}

