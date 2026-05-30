//! Block device abstraction layer.
//!
//! Defines the `BlockDevice` trait as an abstraction between VFS and concrete
//! block devices, and provides a `VirtIOBlock` adapter (RISC-V only).

use crate::sync::spinlock::SpinLock;

/// Errors that can occur during block device operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    /// An I/O error occurred.
    IoError,
    /// No block device is available.
    NoDevice,
    /// The buffer is too small for a block.
    BufferTooSmall,
    /// The block ID is out of range.
    OutOfRange,
}

/// Trait for block device operations.
///
/// All methods operate on fixed-size blocks (typically 512 bytes).
pub trait BlockDevice: Send + Sync {
    /// Read a single block into `buf`.
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> Result<(), VfsError>;
    /// Write a single block from `buf`.
    fn write_block(&self, block_id: usize, buf: &[u8]) -> Result<(), VfsError>;
    /// Return the block size in bytes (default: 512).
    fn block_size(&self) -> usize {
        512
    }
    /// Return the total number of blocks on the device.
    fn capacity_blocks(&self) -> usize;
}

/// VirtIO block device wrapper implementing `BlockDevice`.
#[cfg(target_arch = "riscv64")]
pub struct VirtIOBlock;

#[cfg(target_arch = "riscv64")]
impl BlockDevice for VirtIOBlock {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> Result<(), VfsError> {
        crate::driver::virtio::read_block(block_id, buf).map_err(|_| VfsError::IoError)
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> Result<(), VfsError> {
        crate::driver::virtio::write_block(block_id, buf).map_err(|_| VfsError::IoError)
    }

    fn capacity_blocks(&self) -> usize {
        crate::driver::virtio::capacity().unwrap_or(0) as usize
    }
}

/// Global block device registry.
static BLK_DEVICE: SpinLock<Option<&'static dyn BlockDevice>> = SpinLock::new(None);

/// Register a block device as the global block device.
pub fn set_block_device(dev: &'static dyn BlockDevice) {
    *BLK_DEVICE.lock() = Some(dev);
}

/// Get a reference to the global block device, if registered.
pub fn get_block_device() -> Option<&'static dyn BlockDevice> {
    BLK_DEVICE.lock().as_ref().copied()
}
