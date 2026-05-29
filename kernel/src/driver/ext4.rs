//! ext4 filesystem module backed by ext4_rs.
//!
//! Provides a `KarteBlockDevice` adapter that bridges the ext4_rs `BlockDevice`
//! trait to the VirtIO block device, an `Ext4Fs` wrapper implementing the VFS
//! `FileSystem` trait, and a set of public API functions for filesystem
//! operations: init, read/write files, list directory, delete.

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use ext4_rs::{BLOCK_SIZE, BlockDevice as Ext4BlockDevice, EXT4_INODE_MODE_FILE, Ext4, ROOT_INODE};

use crate::driver::vfs::{FileSystem, VfsDirEntry, VfsError, VfsFileType, VfsMetadata};
use crate::sync::mutex::YieldMutex;

// ─── Constants ──────────────────────────────────────────────────────────────

const SECTOR_SIZE: usize = 512;
const SECTORS_PER_BLOCK: usize = BLOCK_SIZE / SECTOR_SIZE; // 8

/// I/O operation counters for debugging ext4 performance.
static READ_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static WRITE_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// ─── KarteBlockDevice ──────────────────────────────────────────────────────

/// Adapter implementing `ext4_rs::BlockDevice` over the VirtIO block device.
///
/// Each `read_offset` / `write_offset` call operates on a 4 KB block,
/// translated internally into 8 × 512-byte sector operations.
pub struct KarteBlockDevice;

impl KarteBlockDevice {
    pub fn new() -> Self {
        Self
    }
}

impl Ext4BlockDevice for KarteBlockDevice {
    /// Read a 4 KB block at the given byte offset.
    ///
    /// The offset is aligned to `BLOCK_SIZE` (4096). Internally we read
    /// `SECTORS_PER_BLOCK` (8) consecutive 512-byte sectors and concatenate.
    /// Read a 4KB block starting at the given byte offset.
    ///
    /// ext4_rs expects `read_offset(offset)` to return BLOCK_SIZE bytes
    /// starting from `offset` (not from the containing block's start).
    fn read_offset(&self, offset: usize) -> Vec<u8> {
        let mut buf = alloc::vec![0u8; BLOCK_SIZE];
        let base_sector = offset / SECTOR_SIZE;
        let skip = offset % SECTOR_SIZE;

        // Build the buffer by reading the necessary sectors
        // The first sector may need partial data (skip leading bytes)
        let mut buf_pos = 0;
        let mut sector_idx = 0;
        while buf_pos < BLOCK_SIZE {
            let mut sector = [0u8; SECTOR_SIZE];
            match crate::driver::virtio::read_block(base_sector + sector_idx, &mut sector) {
                Ok(()) => {
                    let src_start = if sector_idx == 0 { skip } else { 0 };
                    let src_end = SECTOR_SIZE;
                    let remaining = BLOCK_SIZE - buf_pos;
                    let copy_len = core::cmp::min(src_end - src_start, remaining);
                    buf[buf_pos..buf_pos + copy_len]
                        .copy_from_slice(&sector[src_start..src_start + copy_len]);
                    buf_pos += copy_len;
                }
                Err(_) => break,
            }
            sector_idx += 1;
        }
        buf
    }

    /// Write data at the given byte offset on the block device.
    ///
    /// ext4_rs may call this with arbitrary (non-block-aligned) offsets and
    /// data shorter than BLOCK_SIZE (e.g., writing a 64-byte block group
    /// descriptor into the middle of a 4KB block). We must perform a
    /// read-modify-write to avoid clobbering surrounding data.
    fn write_offset(&self, offset: usize, data: &[u8]) {
        // Write to the disk by doing read-modify-write on affected sectors
        let mut data_pos = 0;
        let mut current_offset = offset;

        while data_pos < data.len() {
            // Determine which sector this falls in
            let sector_idx = current_offset / SECTOR_SIZE;
            let sector_offset = current_offset % SECTOR_SIZE;
            let bytes_in_sector =
                core::cmp::min(SECTOR_SIZE - sector_offset, data.len() - data_pos);

            // Read the current sector
            let mut sector = [0u8; SECTOR_SIZE];
            if crate::driver::virtio::read_block(sector_idx, &mut sector).is_err() {
                crate::console_println!("[ext4] write_offset: read sector {} failed", sector_idx);
                return;
            }

            // Merge new data into the sector
            sector[sector_offset..sector_offset + bytes_in_sector]
                .copy_from_slice(&data[data_pos..data_pos + bytes_in_sector]);

            // Write the sector back
            if let Err(e) = crate::driver::virtio::write_block(sector_idx, &sector) {
                crate::console_println!(
                    "[ext4] write_offset: write sector {} failed: {}",
                    sector_idx,
                    e
                );
                return;
            }

            data_pos += bytes_in_sector;
            current_offset += bytes_in_sector;
        }
    }
}

// Required by ext4_rs::BlockDevice trait bounds (Send + Sync + Any)
unsafe impl Send for KarteBlockDevice {}
unsafe impl Sync for KarteBlockDevice {}

// ─── Ext4Fs — VFS FileSystem wrapper ───────────────────────────────────────

/// ext4 filesystem wrapper implementing the VFS `FileSystem` trait.
///
/// Uses `YieldMutex<Ext4>` for interior mutability. ext4_rs methods take
/// `&self` while VFS write operations require `&mut self`. The YieldMutex
/// yields to the scheduler on contention instead of spinning, which is
/// appropriate for I/O-bound filesystem operations.
pub struct Ext4Fs {
    ext4: YieldMutex<Ext4>,
}

impl Ext4Fs {
    /// Create a new Ext4Fs by opening the ext4 filesystem on the block device.
    ///
    /// Returns `Err` if no valid ext4 filesystem is found on the block device.
    pub fn new() -> Result<Self, &'static str> {
        let bd = Arc::new(KarteBlockDevice::new());
        let ext4 =
            Ext4::try_open(bd).map_err(|_| "no valid ext4 filesystem found on block device")?;
        Ok(Self {
            ext4: YieldMutex::new(ext4),
        })
    }
}

impl FileSystem for Ext4Fs {
    fn name(&self) -> &str {
        "ext4"
    }

    fn root_inode(&self) -> u64 {
        ROOT_INODE as u64
    }

    fn lookup(&self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        let entries = ext4.dir_get_entries(dir as u32);
        for entry in &entries {
            if entry.get_name() == name {
                return Ok(entry.inode as u64);
            }
        }
        Err(VfsError::NotFound)
    }

    fn metadata(&self, inode: u64) -> Result<VfsMetadata, VfsError> {
        let ext4 = self.ext4.lock();
        let inode_ref = ext4.get_inode_ref(inode as u32);
        let inode_obj = inode_ref.inode;

        let mode = inode_obj.mode();
        let is_dir = (mode & 0x4000) != 0;
        let file_type = if is_dir {
            VfsFileType::Directory
        } else {
            VfsFileType::File
        };

        let size = inode_obj.size() as usize;

        Ok(VfsMetadata {
            file_type,
            size,
            name: String::new(), // name not stored in inode; caller resolves via readdir
        })
    }

    fn readdir(&self, dir: u64, idx: usize) -> Result<Option<VfsDirEntry>, VfsError> {
        let ext4 = self.ext4.lock();
        let entries = ext4.dir_get_entries(dir as u32);
        if idx >= entries.len() {
            return Ok(None);
        }
        let entry = &entries[idx];

        // Determine file type from directory entry type field
        let de_type = entry.get_de_type();
        let file_type = if de_type == 2 {
            // EXT4_DE_DIR
            VfsFileType::Directory
        } else {
            VfsFileType::File
        };

        // Get size from the child inode
        let child_inode_ref = ext4.get_inode_ref(entry.inode);
        let size = child_inode_ref.inode.size() as usize;

        Ok(Some(VfsDirEntry {
            name: entry.get_name(),
            file_type,
            size,
        }))
    }

    fn read_file(&self, inode: u64, offset: usize, buf: &mut [u8]) -> Result<usize, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.read_at(inode as u32, offset, buf) {
            Ok(n) => Ok(n),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn write_file(&mut self, inode: u64, offset: usize, data: &[u8]) -> Result<usize, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.write_at(inode as u32, offset, data) {
            Ok(n) => Ok(n),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn create_file(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.create(dir as u32, name, EXT4_INODE_MODE_FILE as u16) {
            Ok(inode_ref) => Ok(inode_ref.inode_num as u64),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn create_dir(&mut self, dir: u64, name: &str) -> Result<u64, VfsError> {
        let ext4 = self.ext4.lock();
        match ext4.create(dir as u32, name, 0x4000) {
            // 0x4000 = EXT4_INODE_MODE_DIR
            Ok(inode_ref) => Ok(inode_ref.inode_num as u64),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn unlink(&mut self, dir: u64, name: &str) -> Result<(), VfsError> {
        let ext4 = self.ext4.lock();
        let mut parent_ref = ext4.get_inode_ref(dir as u32);
        // Look up the child inode
        let entries = ext4.dir_get_entries(dir as u32);
        let child_entry = entries
            .iter()
            .find(|e| e.get_name() == name)
            .ok_or(VfsError::NotFound)?;
        let mut child_ref = ext4.get_inode_ref(child_entry.inode);
        match ext4.unlink(&mut parent_ref, &mut child_ref, name) {
            Ok(_) => Ok(()),
            Err(_) => Err(VfsError::IoError),
        }
    }

    fn set_file_size(&mut self, _inode: u64, _size: usize) -> Result<(), VfsError> {
        // ext4_rs does not expose a direct set_file_size API.
        // Truncation/extension happens implicitly through write_at.
        Ok(())
    }
}

// ─── Global State ───────────────────────────────────────────────────────────

/// Global ext4 filesystem instance.
///
/// Protected by `YieldMutex`: on contention, the caller yields to the
/// scheduler instead of burning CPU spinning. This is the correct behavior
/// for a filesystem mutex that may be held across multiple block I/O
/// operations lasting many milliseconds.
static EXT4_FS: YieldMutex<Option<Ext4Fs>> = YieldMutex::new(None);

/// Whether the ext4 filesystem has been successfully initialized.
static EXT4_AVAILABLE: AtomicBool = AtomicBool::new(false);

// ─── Public API ─────────────────────────────────────────────────────────────

/// Initialize the ext4 filesystem.
///
/// Attempts to open an ext4 filesystem on the VirtIO block device.
/// Sets `EXT4_AVAILABLE` to `true` on success.
pub fn init() -> Result<(), &'static str> {
    match Ext4Fs::new() {
        Ok(fs) => {
            crate::console_println!("[ext4] Filesystem opened successfully");
            *EXT4_FS.lock() = Some(fs);
            EXT4_AVAILABLE.store(true, Ordering::SeqCst);
            Ok(())
        }
        Err(e) => {
            crate::console_println!("[ext4] Failed to open filesystem: {}", e);
            Err(e)
        }
    }
}

/// Check whether the ext4 filesystem is available.
pub fn has_ext4() -> bool {
    EXT4_AVAILABLE.load(Ordering::SeqCst)
}

/// Read the full contents of a file from the ext4 root directory.
///
/// Returns `None` if ext4 is not initialized or the file does not exist.
pub fn read_file(name: &str) -> Option<Vec<u8>> {
    if !has_ext4() {
        return None;
    }

    let guard = EXT4_FS.lock();
    let fs = guard.as_ref()?;

    // Resolve path: look up in root directory
    let inode = fs.lookup(ROOT_INODE as u64, name).ok()?;

    // Get file size from metadata
    let metadata = fs.metadata(inode).ok()?;
    let size = metadata.size;
    if size == 0 {
        return Some(Vec::new());
    }

    let mut buf = alloc::vec![0u8; size];
    let bytes_read = fs.read_file(inode, 0, &mut buf).ok()?;
    buf.truncate(bytes_read);
    Some(buf)
}

/// Write (create or overwrite) a file in the ext4 root directory.
pub fn write_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;

    // Try to look up existing file first
    let inode = fs.lookup(ROOT_INODE as u64, name).unwrap_or_else(|_| 0);

    if inode != 0 {
        // Overwrite existing file
        fs.write_file(inode, 0, data)
            .map_err(|_| "ext4 write failed")?;
    } else {
        // Create new file
        let inode = fs
            .create_file(ROOT_INODE as u64, name)
            .map_err(|_| "ext4 create failed")?;
        fs.write_file(inode, 0, data)
            .map_err(|_| "ext4 write failed")?;
    }
    Ok(())
}

/// List files in the ext4 root directory.
///
/// Returns a vector of `(file_name, size_in_bytes)` pairs.
pub fn list_root() -> Vec<(String, usize)> {
    let mut result = Vec::new();
    if !has_ext4() {
        return result;
    }

    let guard = EXT4_FS.lock();
    let Some(fs) = guard.as_ref() else {
        return result;
    };

    let mut idx = 0;
    loop {
        match fs.readdir(ROOT_INODE as u64, idx) {
            Ok(Some(entry)) => {
                match entry.file_type {
                    VfsFileType::Directory => {
                        // Skip . and .. entries
                        if entry.name == "." || entry.name == ".." {
                            idx += 1;
                            continue;
                        }
                        let mut dname = entry.name;
                        dname.push('/');
                        result.push((dname, entry.size));
                    }
                    VfsFileType::File => {
                        result.push((entry.name, entry.size));
                    }
                    _ => {}
                }
                idx += 1;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    result
}

/// Inject a file into the ext4 root directory.
///
/// If the file already exists, it is left untouched (no overwrite).
///
/// WARNING: This operation involves many block I/O round-trips (inode
/// allocation, bitmap updates, directory entry writes). It should NOT
/// be called during boot. Use `mkdisk.sh put` on the host instead.
pub fn inject_file(name: &str, data: &[u8]) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;

    // Check if file already exists
    if fs.lookup(ROOT_INODE as u64, name).is_ok() {
        return Ok(());
    }

    // Create and write
    let inode = fs
        .create_file(ROOT_INODE as u64, name)
        .map_err(|_| "ext4 create failed")?;
    fs.write_file(inode, 0, data)
        .map_err(|_| "ext4 write failed")?;
    Ok(())
}

/// Delete a file from the ext4 root directory.
pub fn delete_file(name: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;
    fs.unlink(ROOT_INODE as u64, name)
        .map_err(|_| "ext4 unlink failed")
}

/// Create a directory in the ext4 root directory.
pub fn create_directory(name: &str) -> Result<(), &'static str> {
    if !has_ext4() {
        return Err("ext4 not initialized");
    }

    let mut guard = EXT4_FS.lock();
    let fs = guard.as_mut().ok_or("ext4 not initialized")?;

    fs.create_dir(ROOT_INODE as u64, name)
        .map_err(|_| "ext4 create_dir failed")?;
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── ext4 Tests ──");

    // Test 1: ext4 availability flag
    crate::test::run_test("ext4_available_flag", || {
        // Just verify the flag is readable without panic
        let _ = has_ext4();
        true
    });

    // Test 2: init does not panic
    crate::test::run_test("ext4_init_no_panic", || {
        // Try init — may fail if no ext4 disk, but must not panic
        let _ = init();
        true
    });

    // The following tests only run if ext4 is actually available
    if !has_ext4() {
        crate::console_println!("[ext4] Skipping file tests (no ext4 disk)");
        return;
    }

    // Test 3: list root directory does not panic
    crate::test::run_test("ext4_list_root", || {
        let entries = list_root();
        crate::console_println!("[ext4] Root has {} entries", entries.len());
        true
    });

    // Test 4: write and read back a test file
    crate::test::run_test("ext4_write_read", || {
        let test_data = b"ext4 test data from KarteOS";
        if write_file("karte_test.txt", test_data).is_err() {
            crate::console_println!("[ext4] write_file failed");
            return false;
        }
        match read_file("karte_test.txt") {
            Some(data) => {
                if data.as_slice() == test_data {
                    // Clean up
                    let _ = delete_file("karte_test.txt");
                    true
                } else {
                    crate::console_println!(
                        "[ext4] read data mismatch: expected {} bytes, got {}",
                        test_data.len(),
                        data.len()
                    );
                    let _ = delete_file("karte_test.txt");
                    false
                }
            }
            None => {
                crate::console_println!("[ext4] read_file returned None");
                false
            }
        }
    });

    // Test 5: inject_file does not overwrite
    crate::test::run_test("ext4_inject_no_overwrite", || {
        let original = b"original content";
        let modified = b"modified content";

        // Clean up any leftover
        let _ = delete_file("inject_test.txt");

        // Write original
        if write_file("inject_test.txt", original).is_err() {
            return false;
        }
        // Inject should not overwrite
        if inject_file("inject_test.txt", modified).is_err() {
            let _ = delete_file("inject_test.txt");
            return false;
        }
        // Verify original content preserved
        match read_file("inject_test.txt") {
            Some(data) => {
                let ok = data.as_slice() == original;
                let _ = delete_file("inject_test.txt");
                ok
            }
            None => false,
        }
    });
}
