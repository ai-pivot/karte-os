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

/// Read a 32-bit value from PCI configuration space.
fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
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

/// Enumerate all PCI devices on bus 0.
pub fn enumerate() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for device in 0..32 {
        // Check function 0 first
        if let Some(dev) = PciDevice::from_bus_dev_fn(0, device, 0) {
            // Check if multi-function device
            let header_type = pci_read(0, device, 0, 0x0C);
            let multi_function = (header_type >> 23) & 1 == 1;

            devices.push(dev);

            if multi_function {
                for function in 1..8 {
                    if let Some(dev) = PciDevice::from_bus_dev_fn(0, device, function) {
                        devices.push(dev);
                    }
                }
            }
        }
    }

    devices
}

/// Find the first VirtIO block device.
pub fn find_virtio_blk() -> Option<PciDevice> {
    for device in 0..32 {
        if let Some(dev) = PciDevice::from_bus_dev_fn(0, device, 0) {
            if dev.vendor_id == PCI_VENDOR_VIRTIO
                && (dev.device_id == PCI_DEVICE_VIRTIO_BLK
                    || dev.device_id == PCI_DEVICE_VIRTIO_BLK_MODERN)
            {
                return Some(dev);
            }
        }
    }
    None
}

/// Find the first AHCI (SATA) controller.
/// AHCI: class_code = 0x01 (Mass Storage), subclass = 0x06 (SATA), prog_if = 0x01 (AHCI 1.0).
pub fn find_ahci() -> Option<PciDevice> {
    for device in 0..32 {
        // Check all functions
        let header_type = pci_read(0, device, 0, 0x0C);
        let max_fn = if (header_type >> 23) & 1 == 1 { 8 } else { 1 };
        for function in 0..max_fn {
            if let Some(dev) = PciDevice::from_bus_dev_fn(0, device, function as u8) {
                if dev.class_code == 0x01 && dev.subclass == 0x06 && dev.prog_if == 0x01 {
                    return Some(dev);
                }
            }
        }
    }
    None
}

/// Initialize PCI and try to find block devices.
pub fn init() {
    crate::console_println!("[pci] Enumerating PCI devices...");
    let devices = enumerate();

    for dev in &devices {
        crate::console_println!(
            "[pci] {:02x}:{:02x}.{:x} vendor={:#x} device={:#x} class={:#x}/{:#x}",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            dev.class_code,
            dev.subclass
        );
    }

    crate::console_println!("[pci] Found {} devices", devices.len());
}

/// Find an NVMe controller on the PCI bus.
/// NVMe: class_code = 0x01 (Mass Storage), subclass = 0x08 (NVM), prog_if = 0x02 (NVMe).
pub fn find_nvme() -> Option<PciDevice> {
    for device in 0..32 {
        let header_type = pci_read(0, device, 0, 0x0C);
        let max_fn = if (header_type >> 23) & 1 == 1 { 8 } else { 1 };
        for function in 0..max_fn {
            if let Some(dev) = PciDevice::from_bus_dev_fn(0, device, function as u8) {
                if dev.class_code == 0x01 && dev.subclass == 0x08 && dev.prog_if == 0x02 {
                    return Some(dev);
                }
            }
        }
    }
    None
}
