// kernel/src/driver/virtio.rs — VirtIO MMIO block device driver

use core::ptr::NonNull;

use virtio_drivers::device::blk::{SECTOR_SIZE, VirtIOBlk};
use virtio_drivers::transport::mmio::MmioTransport;
use virtio_drivers::{BufferDirection, Hal, PAGE_SIZE, PhysAddr};

use crate::mm::pmm;
use crate::sync::spinlock::SpinLock;

// QEMU virt machine: VirtIO MMIO devices start at 0x10001000, spaced 0x1000 (4KB) apart.
const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;
const VIRTIO_MMIO_MAX_DEVICES: usize = 8;

/// Global VirtIO block device instance, protected by a spinlock.
static BLK_DEVICE: SpinLock<Option<VirtIOBlk<VirtIOHal, MmioTransport<'static>>>> =
    SpinLock::new(None);

/// HAL implementation for virtio-drivers crate.
///
/// Provides DMA memory allocation and address translation for VirtIO devices.
/// Uses identity mapping (physical address == virtual address).
pub struct VirtIOHal;

unsafe impl Hal for VirtIOHal {
    /// Allocate `pages` contiguous physical pages of DMA memory, zeroed.
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        // Allocate contiguous frames for DMA
        let mut frame_list: [Option<usize>; 256] = [None; 256];
        if pages > 256 {
            crate::console_println!("[virtio] dma_alloc: too many pages requested ({})", pages);
            return (0, NonNull::dangling());
        }

        for i in 0..pages {
            match pmm::alloc_frame() {
                Some(frame) => frame_list[i] = Some(frame),
                None => {
                    // Rollback: free already allocated frames
                    for j in 0..i {
                        if let Some(f) = frame_list[j] {
                            pmm::dealloc_frame(f);
                        }
                    }
                    crate::console_println!("[virtio] dma_alloc: out of memory");
                    return (0, NonNull::dangling());
                }
            }
        }

        let base = match frame_list[0] {
            Some(f) => f,
            None => return (0, NonNull::dangling()),
        };

        // Zero the allocated memory
        let size = pages * PAGE_SIZE;
        unsafe {
            core::ptr::write_bytes(base as *mut u8, 0, size);
        }

        // Identity mapping: physical address == virtual address
        let vaddr = NonNull::new(base as *mut u8).unwrap_or(NonNull::dangling());
        (base as PhysAddr, vaddr)
    }

    /// Deallocate contiguous physical DMA memory pages.
    unsafe fn dma_dealloc(paddr: PhysAddr, _vaddr: NonNull<u8>, pages: usize) -> i32 {
        for i in 0..pages {
            pmm::dealloc_frame((paddr as usize) + i * PAGE_SIZE);
        }
        0
    }

    /// Convert a physical MMIO address to a virtual address.
    /// Uses identity mapping, so they are the same.
    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        NonNull::new(paddr as *mut u8).unwrap_or(NonNull::dangling())
    }

    /// Share a buffer with the device. Identity mapping means we just return the physical address.
    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        // With identity mapping, the physical address is the same as the virtual address.
        buffer.as_ptr() as *mut u8 as PhysAddr
    }

    /// Unshare a buffer from the device. No-op with identity mapping.
    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // No-op: identity mapping, no IOMMU, no copy-back needed
    }
}

/// Probe for VirtIO MMIO devices on the QEMU virt platform.
///
/// Scans the MMIO region starting at 0x10001000 for VirtIO block devices
/// and initializes the first one found.
pub fn probe_virtio_devices() {
    crate::console_println!("[virtio] Probing MMIO devices...");

    // Map VirtIO MMIO region into page table (identity map)
    // The region 0x10001000 - 0x10003000 needs to be mapped
    for addr in (VIRTIO_MMIO_BASE..VIRTIO_MMIO_BASE + VIRTIO_MMIO_MAX_DEVICES * VIRTIO_MMIO_STRIDE)
        .step_by(PAGE_SIZE)
    {
        // VirtIO MMIO region is mapped in mm::vmm::init()
        let _ = addr;
    }

    for i in 0..VIRTIO_MMIO_MAX_DEVICES {
        let mmio_base = VIRTIO_MMIO_BASE + i * VIRTIO_MMIO_STRIDE;

        // Read the magic value to check if a VirtIO device is present
        let magic = unsafe { core::ptr::read_volatile(mmio_base as *const u32) };
        if magic != 0x7472_6976 {
            // No device at this slot, but continue probing
            continue;
        }

        let device_id = unsafe { core::ptr::read_volatile((mmio_base + 8) as *const u32) };
        crate::console_println!(
            "[virtio] Slot {}: magic={:#x}, type={}",
            i,
            magic,
            device_id
        );

        // Device type 0 = empty transport (no real device), skip it
        if device_id == 0 {
            crate::console_println!(
                "[virtio] Slot {}: empty transport (device_id=0), skipping",
                i
            );
            continue;
        }

        // Device type 2 = Block device
        if device_id == 2 {
            crate::console_println!("[virtio] Initializing block device...");
            match init_blk_device(mmio_base, VIRTIO_MMIO_STRIDE) {
                Ok(()) => {
                    crate::console_println!("[virtio] Block device initialized successfully");
                    return; // Only need one block device for now
                }
                Err(e) => {
                    crate::console_println!("[virtio] Failed to init block device: {:?}", e);
                }
            }
        } else {
            crate::console_println!(
                "[virtio] Slot {}: unsupported device type {}, skipping",
                i,
                device_id
            );
        }
    }

    crate::console_println!("[virtio] No block device found");
}

/// Initialize a VirtIO block device at the given MMIO address.
pub fn init_blk_device(mmio_base: usize, mmio_size: usize) -> Result<(), virtio_drivers::Error> {
    let header = NonNull::new(mmio_base as *mut virtio_drivers::transport::mmio::VirtIOHeader)
        .ok_or(virtio_drivers::Error::IoError)?;

    // Safety: mmio_base points to a valid VirtIO MMIO region on the QEMU virt platform.
    // The memory is MMIO and remains valid for the lifetime of the OS.
    let transport = unsafe { MmioTransport::new(header, mmio_size) }.map_err(|e| {
        crate::console_println!("[virtio] MmioTransport error: {:?}", e);
        virtio_drivers::Error::IoError
    })?;

    let blk = VirtIOBlk::<VirtIOHal, _>::new(transport)?;

    let capacity = blk.capacity();
    let total_kb = capacity * SECTOR_SIZE as u64 / 1024;
    crate::console_println!(
        "[virtio] Block device: {} sectors, {} KB total",
        capacity,
        total_kb
    );

    *BLK_DEVICE.lock() = Some(blk);
    Ok(())
}

/// Read a single block (512 bytes) from the block device.
///
/// Returns `Ok(())` on success, or an error string on failure.
pub fn read_block(block_id: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < SECTOR_SIZE {
        return Err("buffer too small");
    }

    let mut guard = BLK_DEVICE.lock();
    let blk_device = guard.as_mut().ok_or("no block device")?;

    blk_device
        .read_blocks(block_id, buf)
        .map_err(|_| "read failed")
}

/// Write a single block (512 bytes) to the block device.
///
/// Returns `Ok(())` on success, or an error string on failure.
pub fn write_block(block_id: usize, buf: &[u8]) -> Result<(), &'static str> {
    if buf.len() < SECTOR_SIZE {
        return Err("buffer too small");
    }

    let mut guard = BLK_DEVICE.lock();
    let blk_device = guard.as_mut().ok_or("no block device")?;

    blk_device
        .write_blocks(block_id, buf)
        .map_err(|_| "write failed")
}

/// Get the capacity of the block device in sectors, if available.
pub fn capacity() -> Option<u64> {
    let guard = BLK_DEVICE.lock();
    guard.as_ref().map(|blk| blk.capacity())
}
