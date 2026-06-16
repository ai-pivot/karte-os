//! ext4 filesystem — RISC-V block I/O callbacks (VirtIO block device).

use core::sync::atomic::Ordering;

use super::KarteBlockDevice;

fn riscv_block_read(sector: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    super::READ_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::driver::virtio::read_block(sector, buf)
}

fn riscv_block_write(sector: usize, buf: &[u8]) -> Result<(), &'static str> {
    super::WRITE_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::driver::virtio::write_block(sector, buf)
}

/// Create a KarteBlockDevice backed by VirtIO block I/O.
/// Called from `ext4.rs::init()`.
pub fn create_block_device() -> KarteBlockDevice {
    KarteBlockDevice::new(riscv_block_read, riscv_block_write)
}
