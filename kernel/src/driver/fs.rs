// kernel/src/driver/fs.rs — Simple in-memory file system

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;

/// File data storage: either a static reference (for embedded binaries)
/// or heap-allocated (for runtime-created files).
#[derive(Clone)]
pub enum FileData {
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl FileData {
    fn as_slice(&self) -> &[u8] {
        match self {
            FileData::Static(s) => s,
            FileData::Owned(v) => v,
        }
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
}

/// A single file with a name and binary data.
#[derive(Clone)]
pub struct File {
    pub name: String,
    pub data: FileData,
}

/// A simple in-memory file system.
///
/// Files are stored as a flat list. This is not persistent across reboots,
/// but provides a working file abstraction for the kernel.
pub struct FileSystem {
    files: Vec<File>,
}

/// Global file system instance, protected by a spinlock.
static FS: SpinLock<FileSystem> = SpinLock::new(FileSystem::new_internal());

impl FileSystem {
    /// Internal constructor used for static initialization.
    const fn new_internal() -> Self {
        Self { files: Vec::new() }
    }

    /// Create a new empty file system.
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Create a new empty file with the given name.
    ///
    /// Returns `Ok(())` if the file was created, or `Err(())` if a file
    /// with that name already exists.
    pub fn create(&mut self, name: &str) -> Result<(), ()> {
        if self.files.iter().any(|f| f.name == name) {
            return Err(());
        }
        self.files.push(File {
            name: String::from(name),
            data: FileData::Owned(Vec::new()),
        });
        Ok(())
    }

    /// Write static data to a file (no heap allocation). Creates file if needed.
    pub fn write_static(&mut self, name: &str, data: &'static [u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data = FileData::Static(data);
            Ok(())
        } else {
            self.files.push(File {
                name: String::from(name),
                data: FileData::Static(data),
            });
            Ok(())
        }
    }

    /// Write data to a file. If the file does not exist, it is created.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data = FileData::Owned(data.to_vec());
            Ok(())
        } else {
            self.files.push(File {
                name: String::from(name),
                data: FileData::Owned(data.to_vec()),
            });
            Ok(())
        }
    }

    /// Append data to an existing file.
    pub fn append(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            // Convert to Owned if currently Static, then append
            let mut owned = match &file.data {
                FileData::Static(s) => s.to_vec(),
                FileData::Owned(v) => v.clone(),
            };
            owned.extend_from_slice(data);
            file.data = FileData::Owned(owned);
            Ok(())
        } else {
            Err(())
        }
    }

    /// Read the contents of a file by name.
    pub fn read(&self, name: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.data.as_slice())
    }

    /// Delete a file by name.
    ///
    /// Returns `Ok(())` if the file was removed, or `Err(())` if not found.
    pub fn delete(&mut self, name: &str) -> Result<(), ()> {
        let len_before = self.files.len();
        self.files.retain(|f| f.name != name);
        if self.files.len() < len_before {
            Ok(())
        } else {
            Err(())
        }
    }

    /// List all files in the file system.
    pub fn list(&self) -> &[File] {
        self.files.as_slice()
    }

    /// Get the number of files in the file system.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// Get a locked reference to the global file system.
pub fn global_fs() -> crate::sync::spinlock::SpinLockGuard<'static, FileSystem> {
    FS.lock()
}

/// Initialize the file system and populate with embedded binaries.
///
/// In normal mode: initializes FAT32 on the VirtIO block device, then injects
/// embedded binaries into FAT32 (if they don't already exist). This gives us
/// persistent file storage shared with the host OS.
///
/// Falls back to pure in-memory if FAT32 initialization fails or in test mode.
pub fn init() {
    // Pre-populate filesystem with embedded user programs.
    let mut fs = FS.lock();

    // Assembly test programs are RISC-V only
    #[cfg(target_arch = "riscv64")]
    {
        fs.write_static("hello", include_bytes!("../../../user/hello.elf"))
            .unwrap();
        fs.write_static("heap_test", include_bytes!("../../../user/heap_test.elf"))
            .unwrap();
        fs.write_static("file_test", include_bytes!("../../../user/file_test.elf"))
            .unwrap();
        fs.write_static("spawn_test", include_bytes!("../../../user/spawn_test.elf"))
            .unwrap();
    }

    // Shell and init are available on all architectures
    fs.write_static("shell", include_bytes!("../../../user/shell.elf"))
        .unwrap();
    fs.write_static("init", include_bytes!("../../../user/shell.elf"))
        .unwrap();

    let ram_count = fs.file_count();
    drop(fs);

    crate::console_println!("[fs] RamFS initialized ({} files)", ram_count);

    // Try ext4 first (preferred over FAT32 for modern features),
    // then fall back to FAT32, then RamFS only.
    //
    // NOTE: ext4 file injection is NOT done at boot time. Ext4 metadata
    // operations (inode allocation, bitmap updates, directory entry writes)
    // require dozens of block I/O round-trips, which is too slow for the
    // boot path. Instead, user programs are pre-loaded into the ext4 disk
    // image on the host via `tools/mkdisk.sh put <file>` before booting.
    match crate::driver::ext4::init() {
        Ok(()) => {
            crate::console_println!("[fs] ext4 filesystem mounted");
            // Register ext4 as the root filesystem in VFS so that
            // syscalls like openat(O_CREAT) can create files on ext4
            // through the VFS layer.
            #[cfg(target_arch = "x86_64")]
            match crate::driver::ext4::mount_to_vfs() {
                Ok(()) => crate::console_println!("[fs] ext4 registered in VFS"),
                Err(e) => crate::console_println!("[fs] ext4 VFS mount failed: {}", e),
            }
        }
        Err(e) => {
            crate::console_println!("[fs] ext4 unavailable ({})", e);
            // Fall back to FAT32
            match crate::driver::fat32::init() {
                Ok(()) => {
                    crate::console_println!("[fs] FAT32 filesystem mounted");
                    #[cfg(target_arch = "riscv64")]
                    let files_to_inject: &[(&str, &[u8])] = &[
                        ("hello", include_bytes!("../../../user/hello.elf")),
                        ("heap_test", include_bytes!("../../../user/heap_test.elf")),
                        ("file_test", include_bytes!("../../../user/file_test.elf")),
                        ("spawn_test", include_bytes!("../../../user/spawn_test.elf")),
                        ("shell", include_bytes!("../../../user/shell.elf")),
                        ("init", include_bytes!("../../../user/shell.elf")),
                    ];
                    #[cfg(target_arch = "x86_64")]
                    let files_to_inject: &[(&str, &[u8])] = &[
                        ("shell", include_bytes!("../../../user/shell.elf")),
                        ("init", include_bytes!("../../../user/shell.elf")),
                    ];
                    let mut injected = 0;
                    for (name, data) in files_to_inject.iter() {
                        match crate::driver::fat32::inject_file(name, data) {
                            Ok(()) => injected += 1,
                            Err(e) => {
                                crate::console_println!(
                                    "[fs] Warning: failed to inject {}: {}",
                                    name,
                                    e
                                )
                            }
                        }
                    }
                    crate::console_println!(
                        "[fs] Injected {}/{} files into FAT32",
                        injected,
                        files_to_inject.len()
                    );
                    FAT32_AVAILABLE.store(true, core::sync::atomic::Ordering::Relaxed);
                }
                Err(e2) => {
                    crate::console_println!("[fs] FAT32 unavailable ({}), using RamFS only", e2);
                }
            }
        }
    }
}

/// Whether FAT32 is available
static FAT32_AVAILABLE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Check if FAT32 backend is available
pub fn has_fat32() -> bool {
    FAT32_AVAILABLE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Read file data as an owned Vec.
/// Tries ext4 first, then FAT32, then falls back to RamFS (embedded).
pub fn read_file_owned(name: &str) -> Option<Vec<u8>> {
    // Try ext4 first
    if crate::driver::ext4::has_ext4() {
        if let Some(data) = crate::driver::ext4::read_file(name) {
            return Some(data);
        }
    }
    // Try FAT32
    if has_fat32() {
        if let Some(data) = crate::driver::fat32::read_file(name) {
            return Some(data);
        }
    }
    // Fallback to RamFS
    let fs = FS.lock();
    fs.read(name).map(|d| d.to_vec())
}

/// Write file data (creates or overwrites).
/// Tries ext4 first (persistent), else RamFS.
pub fn write_file_owned(name: &str, data: &[u8]) -> Result<(), ()> {
    if crate::driver::ext4::has_ext4() {
        crate::driver::ext4::write_file(name, data).map_err(|_| ())
    } else {
        let mut fs = FS.lock();
        fs.write(name, data)
    }
}

/// List all files from ext4/FAT32 and RamFS, deduplicated.
pub fn list_all_files() -> Vec<(String, usize)> {
    list_directory("")
}

/// List files in a directory specified by path (relative to root).
/// Falls back to list_all_files (root) for non-ext4 filesystems.
pub fn list_directory(path: &str) -> Vec<(String, usize)> {
    let mut result = Vec::new();

    // ext4 files
    if crate::driver::ext4::has_ext4() {
        for (name, size) in crate::driver::ext4::list_directory(path) {
            result.push((name, size));
        }
        // For non-root paths, skip FAT32/RamFS (only ext4 supports subdirs)
        if !path.is_empty() {
            return result;
        }
    }

    // FAT32 files next (only for root listing)
    if has_fat32() {
        for (name, size) in crate::driver::fat32::list_root() {
            if !result.iter().any(|(n, _)| n == &name) {
                result.push((name, size));
            }
        }
    }

    // RamFS files (skip duplicates, only for root listing)
    let fs = FS.lock();
    for file in fs.list() {
        if !result.iter().any(|(n, _)| n == &file.name) {
            result.push((file.name.clone(), file.data.len()));
        }
    }

    result
}

/// Create a new empty file.
/// Creates in ext4 if available, else RamFS.
pub fn create_file(name: &str) -> Result<(), ()> {
    if crate::driver::ext4::has_ext4() {
        crate::driver::ext4::write_file(name, &[]).map_err(|_| ())
    } else {
        let mut fs = FS.lock();
        fs.create(name)
    }
}

/// Create a directory. Tries ext4 first, else RamFS.
pub fn create_dir(name: &str) -> Result<(), ()> {
    if crate::driver::ext4::has_ext4() {
        crate::driver::ext4::create_directory(name).map_err(|_| ())
    } else {
        let mut fs = FS.lock();
        fs.create(name) // RamFS fallback: create as regular file entry
    }
}

/// Check if a path exists in the filesystem.
pub fn lookup_path(name: &str) -> Option<u64> {
    if crate::driver::ext4::has_ext4() {
        crate::driver::ext4::lookup_path(name)
    } else {
        let fs = FS.lock();
        if fs.read(name).is_some() {
            Some(1)
        } else {
            None
        }
    }
}

/// Delete a file from ext4 and RamFS.
pub fn delete_file(name: &str) -> Result<(), ()> {
    let mut any_ok = false;
    if crate::driver::ext4::has_ext4() {
        any_ok |= crate::driver::ext4::delete_file(name).is_ok();
    }
    {
        let mut fs = FS.lock();
        any_ok |= fs.delete(name).is_ok();
    }
    if any_ok { Ok(()) } else { Err(()) }
}

// ─── VFS Abstraction ────────────────────────────────────────────────

/// Maximum number of open file descriptors per process
pub const MAX_FDS: usize = 32;

/// File open flags
pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_CREAT: u32 = 0x100;
pub const O_TRUNC: u32 = 0x200;
pub const O_APPEND: u32 = 0x400;
pub const O_NONBLOCK: u32 = 0x800;

// ─── POSIX Byte-Range File Locking ────────────────────────────────────
// Required for SQLite WAL mode, which uses fcntl(F_SETLK/F_GETLK) for
// inter-thread coordination. Multiple Go goroutines (clone'd threads)
// concurrently access the same database file, so locks must properly
// track ownership per-fd and detect conflicts between overlapping ranges.

/// Lock types matching Linux flock struct l_type values.
pub const F_RDLCK: u16 = 0;
pub const F_WRLCK: u16 = 1;
pub const F_UNLCK: u16 = 2;

/// fcntl command numbers (Linux x86_64).
pub const F_GETFD: usize = 1;
pub const F_SETFD: usize = 2;
pub const F_GETFL: usize = 3;
pub const F_SETFL: usize = 4;
pub const F_GETLK: usize = 5;
pub const F_SETLK: usize = 6;
pub const F_SETLKW: usize = 7;

/// Close-on-exec flag (for F_GETFD/F_SETFD).
pub const FD_CLOEXEC: usize = 1;

/// Linux struct flock (64-bit), laid out for user-space compatibility.
/// 24 bytes total for 64-bit: l_type(2) + l_whence(2) + padding(4) +
/// l_start(8) + l_len(8) = 24 bytes. But Go passes a 32-byte struct
/// with l_pid at the end. We read/write fields individually by offset.
///
/// Layout (from <fcntl.h> on x86_64):
///   offset 0:  l_type   (i16)
///   offset 2:  l_whence (i16)
///   offset 4:  l_start  (i64)
///   offset 12: l_len    (i64)
///   offset 20: l_pid    (i32)
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Flock {
    pub l_type: i16,
    pub l_whence: i16,
    pub l_start: i64,
    pub l_len: i64,
    pub l_pid: i32,
}

impl Flock {
    pub fn read_from_user(buf: *const u8) -> Self {
        unsafe {
            Flock {
                l_type: core::ptr::read_unaligned(buf as *const i16),
                l_whence: core::ptr::read_unaligned(buf.add(2) as *const i16),
                l_start: core::ptr::read_unaligned(buf.add(4) as *const i64),
                l_len: core::ptr::read_unaligned(buf.add(12) as *const i64),
                l_pid: core::ptr::read_unaligned(buf.add(20) as *const i32),
            }
        }
    }

    /// Construct a Flock from a byte slice (24 bytes).
    /// Used to safely decode bytes read from user space via user_read_bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Flock {
            l_type: i16::from_ne_bytes([bytes[0], bytes[1]]),
            l_whence: i16::from_ne_bytes([bytes[2], bytes[3]]),
            l_start: i64::from_ne_bytes([
                bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
            ]),
            l_len: i64::from_ne_bytes([
                bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18],
                bytes[19],
            ]),
            l_pid: i32::from_ne_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        }
    }

    /// Serialize Flock to a byte array (24 bytes).
    /// Used to safely write back to user space via user_write_bytes.
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];
        buf[0..2].copy_from_slice(&self.l_type.to_ne_bytes());
        buf[2..4].copy_from_slice(&self.l_whence.to_ne_bytes());
        buf[4..12].copy_from_slice(&self.l_start.to_ne_bytes());
        buf[12..20].copy_from_slice(&self.l_len.to_ne_bytes());
        buf[20..24].copy_from_slice(&self.l_pid.to_ne_bytes());
        buf
    }

    pub fn write_to_user(&self, buf: *mut u8) {
        unsafe {
            core::ptr::write_unaligned(buf as *mut i16, self.l_type);
            core::ptr::write_unaligned(buf.add(2) as *mut i16, self.l_whence);
            core::ptr::write_unaligned(buf.add(4) as *mut i64, self.l_start);
            core::ptr::write_unaligned(buf.add(12) as *mut i64, self.l_len);
            core::ptr::write_unaligned(buf.add(20) as *mut i32, self.l_pid);
        }
    }

    /// Resolve the absolute byte offset from l_whence and l_start.
    /// For SQLite, l_whence is always SEEK_SET (0).
    pub fn abs_start(&self) -> i64 {
        match self.l_whence {
            0 => self.l_start, // SEEK_SET
            1 => self.l_start, // SEEK_CUR (approximate — no file pos available)
            2 => self.l_start, // SEEK_END (approximate)
            _ => self.l_start,
        }
    }

    /// The end of the lock range. l_len == 0 means "lock to end of file"
    /// which we represent as i64::MAX.
    pub fn abs_end(&self) -> i64 {
        let start = self.abs_start();
        if self.l_len == 0 {
            i64::MAX
        } else {
            start + self.l_len
        }
    }
}

/// A granted byte-range lock, owned by a specific fd + inode.
#[derive(Clone, Debug)]
struct FileLock {
    /// The fd that holds this lock.
    owner_fd: usize,
    /// Inode number of the locked file (from VFS/ext4).
    inode: u64,
    /// Lock type: F_RDLCK or F_WRLCK.
    lock_type: u16,
    /// Start byte offset (absolute).
    start: i64,
    /// End byte offset (exclusive).
    end: i64,
}

/// Global file lock table. All locks are tracked here and checked for
/// conflicts on every F_SETLK / F_GETLK call.
static FILE_LOCKS: SpinLock<Vec<FileLock>> = SpinLock::new(Vec::new());

/// Check if two ranges overlap: [a_start, a_end) ∩ [b_start, b_end) ≠ ∅
fn ranges_overlap(a_start: i64, a_end: i64, b_start: i64, b_end: i64) -> bool {
    a_start < b_end && b_start < a_end
}

/// Find a conflicting lock on the given inode, excluding locks owned by
/// `owner_fd`. Returns the first conflicting lock found.
fn find_conflict(
    inode: u64,
    owner_fd: usize,
    lock_type: u16,
    start: i64,
    end: i64,
) -> Option<FileLock> {
    let locks = FILE_LOCKS.lock();
    for lock in locks.iter() {
        if lock.inode != inode {
            continue;
        }
        if lock.owner_fd == owner_fd {
            continue; // Same fd — own locks don't conflict
        }
        if !ranges_overlap(lock.start, lock.end, start, end) {
            continue; // No range overlap
        }
        // Check type compatibility:
        // Two shared locks (F_RDLCK) can coexist.
        // Any lock + exclusive lock (F_WRLCK) = conflict.
        if lock.lock_type == F_WRLCK || lock_type == F_WRLCK {
            return Some(lock.clone());
        }
    }
    None
}

/// Remove all locks owned by `owner_fd` on `inode` in the given range.
fn remove_locks_in_range(inode: u64, owner_fd: usize, start: i64, end: i64) {
    let mut locks = FILE_LOCKS.lock();
    locks.retain(|lock| {
        if lock.inode != inode || lock.owner_fd != owner_fd {
            return true; // Keep locks on other inodes / other fds
        }
        // Remove only the overlapping portion.
        // For simplicity: if the ranges overlap, remove the lock entirely.
        // (Splitting locks at boundaries is an optimization for later.)
        !ranges_overlap(lock.start, lock.end, start, end)
    });
}

/// Add a new lock for the given fd/inode.
fn add_lock(inode: u64, owner_fd: usize, lock_type: u16, start: i64, end: i64) {
    let mut locks = FILE_LOCKS.lock();
    locks.push(FileLock {
        owner_fd,
        inode,
        lock_type,
        start,
        end,
    });
}

/// Remove all locks held by a specific fd (called on close).
pub fn release_fd_locks(fd: usize) {
    let mut locks = FILE_LOCKS.lock();
    locks.retain(|lock| lock.owner_fd != fd);
}

/// Execute a fcntl lock operation (F_GETLK / F_SETLK / F_SETLKW).
/// Returns (retval, should_write_flock).
/// - retval: syscall return value (0 = success, -1 = error)
/// - If should_write_flock, the caller should write `out_flock` to user buf.
pub fn fcntl_lock_op(cmd: usize, fd: usize, inode: u64, flock: &Flock) -> (isize, Option<Flock>) {
    let lock_type = flock.l_type as u16;
    let start = flock.abs_start();
    let end = flock.abs_end();

    match cmd {
        F_GETLK => {
            // Check for a conflicting lock. If found, write it back.
            // If not found, set l_type = F_UNLCK.
            match find_conflict(inode, fd, lock_type, start, end) {
                Some(conflict) => {
                    let out = Flock {
                        l_type: conflict.lock_type as i16,
                        l_whence: 0,
                        l_start: conflict.start,
                        l_len: if conflict.end == i64::MAX {
                            0
                        } else {
                            conflict.end - conflict.start
                        },
                        l_pid: 0, // We don't track PID per lock
                    };
                    (0, Some(out))
                }
                None => {
                    let mut out = *flock;
                    out.l_type = F_UNLCK as i16;
                    (0, Some(out))
                }
            }
        }
        F_SETLK | F_SETLKW => {
            if lock_type == F_UNLCK {
                // Unlock: remove matching locks
                remove_locks_in_range(inode, fd, start, end);
                (0, None)
            } else {
                // Lock: check for conflict
                match find_conflict(inode, fd, lock_type, start, end) {
                    Some(_conflict) => {
                        if cmd == F_SETLK {
                            // Non-blocking: return EAGAIN
                            (-1, None) // -1 = EAGAIN
                        } else {
                            // F_SETLKW: blocking — yield and retry.
                            // In practice, with cooperative goroutines,
                            // we yield CPU and let the conflicting lock
                            // holder release before retrying.
                            // Simple strategy: yield once, then try again.
                            // (A full wait-queue implementation can be added later.)
                            crate::sched::schedule();
                            match find_conflict(inode, fd, lock_type, start, end) {
                                Some(_) => (-1, None), // Still blocked
                                None => {
                                    add_lock(inode, fd, lock_type, start, end);
                                    (0, None)
                                }
                            }
                        }
                    }
                    None => {
                        // No conflict — grant the lock
                        add_lock(inode, fd, lock_type, start, end);
                        (0, None)
                    }
                }
            }
        }
        _ => (-1, None),
    }
}

/// File descriptor type — distinguishes regular files from pipe endpoints.
#[derive(Clone, PartialEq, Debug)]
pub enum FdType {
    /// Standard I/O (stdin/stdout/stderr) — routes to TTY/UART unless overridden.
    Stdio,
    /// Regular file — routes to filesystem.
    File,
    /// Pipe read endpoint.
    PipeRead,
    /// Pipe write endpoint.
    PipeWrite,
    /// Fake file with in-memory buffer — for virtual files (.xbot/*).
    FakeFile(alloc::vec::Vec<u8>),
    /// Virtual file (e.g., /proc/version) — always readable, empty content.
    VirtualFile,
    /// /dev/urandom — reads return cryptographic-quality random bytes,
    /// writes are discarded. Required by SQLite WAL mode for nonce generation.
    Urandom,
    /// eventfd — for Go runtime polling.
    Eventfd,
    /// epoll instance — for Go runtime netpoll.
    Epoll,
    /// timerfd — for Go runtime timers.
    Timerfd,
    /// ext4 file descriptor.
    Ext4File(crate::driver::ext4::Ext4FileDesc),
    /// VFS-managed file — routes to the VFS open file table.
    VfsFile(usize),
}

/// A file descriptor entry — wraps an in-memory file with seek position
#[derive(Clone)]
pub struct FileDescriptor {
    /// Name of the file in the in-memory filesystem
    pub name: String,
    /// Current seek position
    pub pos: usize,
    /// Open flags (O_RDONLY, O_WRONLY, O_RDWR)
    pub flags: u32,
    /// Whether this fd is valid
    pub valid: bool,
    /// File descriptor type.
    pub fd_type: FdType,
    /// Pipe ID (only valid when fd_type is PipeRead or PipeWrite).
    pub pipe_id: Option<usize>,
    /// The fd number itself (for routing eventfd/timerfd lookups).
    pub fd_num: usize,
}

/// Per-process file descriptor table
#[derive(Clone)]
pub struct FdTable {
    fds: [Option<FileDescriptor>; MAX_FDS],
}

impl FileDescriptor {
    /// Check if this fd has data available for reading.
    #[cfg(target_arch = "x86_64")]
    pub fn is_readable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => {
                if self.name == "stdin" {
                    crate::driver::tty::has_input()
                } else {
                    false
                }
            }
            FdType::PipeRead => {
                if let Some(pipe_id) = self.pipe_id {
                    crate::driver::pipe::pipe_available(pipe_id) > 0
                } else {
                    false
                }
            }
            #[cfg(target_arch = "x86_64")]
            FdType::Eventfd => crate::syscall::epoll::eventfd::eventfd_peek_by_fd(self.fd_num) > 0,
            #[cfg(target_arch = "x86_64")]
            FdType::Timerfd => crate::syscall::epoll::timerfd_peek(self.fd_num),
            #[cfg(target_arch = "x86_64")]
            FdType::Epoll => false,
            FdType::PipeWrite
            | FdType::File
            | FdType::FakeFile(_)
            | FdType::VirtualFile
            | FdType::VfsFile(_)
            | FdType::Urandom => false,
            #[cfg(target_arch = "x86_64")]
            FdType::Ext4File(_) => false,
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn is_readable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => {
                if self.name == "stdin" {
                    crate::driver::tty::has_input()
                } else {
                    false
                }
            }
            FdType::PipeRead => {
                if let Some(pipe_id) = self.pipe_id {
                    crate::driver::pipe::pipe_available(pipe_id) > 0
                } else {
                    false
                }
            }
            FdType::Eventfd => crate::syscall::epoll::eventfd::eventfd_peek_by_fd(self.fd_num) > 0,
            FdType::Timerfd => crate::syscall::epoll::timerfd_peek(self.fd_num),
            FdType::Epoll => false,
            FdType::PipeWrite
            | FdType::File
            | FdType::FakeFile(_)
            | FdType::VirtualFile
            | FdType::VfsFile(_)
            | FdType::Urandom => false,
            FdType::Ext4File(_) => false,
        }
    }

    /// Check if this fd is ready for writing.
    #[cfg(target_arch = "x86_64")]
    pub fn is_writable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => self.name != "stdin",
            FdType::PipeWrite => true,
            FdType::PipeRead | FdType::Eventfd | FdType::Epoll | FdType::Timerfd => false,
            FdType::File
            | FdType::FakeFile(_)
            | FdType::VirtualFile
            | FdType::VfsFile(_)
            | FdType::Urandom => true,
            FdType::Ext4File(desc) => desc.writable,
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn is_writable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => self.name != "stdin",
            FdType::PipeWrite => true,
            FdType::PipeRead => false,
            FdType::File
            | FdType::FakeFile(_)
            | FdType::VirtualFile
            | FdType::Urandom
            | FdType::VfsFile(_)
            | FdType::Ext4File(_)
            | FdType::Eventfd
            | FdType::Epoll => true,
            FdType::Timerfd => false,
        }
    }

    /// Get the fd number.
    pub fn fd_number(&self) -> usize {
        self.fd_num
    }
}

impl FdTable {
    pub fn new() -> Self {
        let mut table = FdTable {
            fds: core::array::from_fn(|_| None),
        };
        // Pre-allocate stdin/stdout/stderr slots as Stdio type
        table.fds[0] = Some(FileDescriptor {
            name: String::from("stdin"),
            pos: 0,
            flags: O_RDONLY,
            valid: true,
            fd_type: FdType::Stdio,
            pipe_id: None,
            fd_num: 0,
        });
        table.fds[1] = Some(FileDescriptor {
            name: String::from("stdout"),
            pos: 0,
            flags: O_WRONLY,
            valid: true,
            fd_type: FdType::Stdio,
            pipe_id: None,
            fd_num: 0,
        });
        table.fds[2] = Some(FileDescriptor {
            name: String::from("stderr"),
            pos: 0,
            flags: O_WRONLY,
            valid: true,
            fd_type: FdType::Stdio,
            pipe_id: None,
            fd_num: 0,
        });
        table
    }

    /// Allocate a new fd, returns the fd number or None if table full
    pub fn alloc(&mut self, name: String, flags: u32) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name,
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::File,
                    pipe_id: None,
                    fd_num: 0,
                });
                return Some(i);
            }
        }
        None
    }

    /// Allocate a fd that points to a VFS-managed file. The VFS open file
    /// table entry (at `vfs_fd`) holds the real inode/offset/flags.
    /// The FdTable entry just records the vfs_fd for routing read/write/close.
    pub fn alloc_vfs_fd(&mut self, name: String, vfs_fd: usize, flags: u32) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            let is_free = slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false);
            if is_free {
                *slot = Some(FileDescriptor {
                    name: name.clone(),
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::VfsFile(vfs_fd),
                    pipe_id: None,
                    fd_num: 0,
                });
                if i <= 5 {
                    // no debug log — holding fd_table lock
                }
                return Some(i);
            }
        }
        None
    }

    /// Allocate a fake fd for virtual files (writes go to UART/null).
    pub fn alloc_stdio_fd(&mut self, name: String, flags: u32) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name,
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::Stdio,
                    pipe_id: None,
                    fd_num: 0,
                });
                return Some(i);
            }
        }
        None
    }

    /// Allocate a fake fd with in-memory buffer for virtual files (.xbot/*).
    pub fn alloc_fake_fd(&mut self, name: String, flags: u32) -> Option<usize> {
        self.alloc_fake_fd_with_content(name, flags, alloc::vec![])
    }

    /// Allocate a FakeFile fd with pre-populated content (e.g. /etc/resolv.conf).
    pub fn alloc_fake_fd_with_content(
        &mut self,
        name: String,
        flags: u32,
        content: alloc::vec::Vec<u8>,
    ) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name,
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::FakeFile(content),
                    pipe_id: None,
                    fd_num: 0,
                });
                return Some(i);
            }
        }
        None
    }

    /// Allocate a fd for /dev/urandom — reads produce random bytes.
    pub fn alloc_urandom_fd(&mut self, flags: u32) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name: alloc::string::String::from("/dev/urandom"),
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::Urandom,
                    pipe_id: None,
                    fd_num: 0,
                });
                return Some(i);
            }
        }
        None
    }

    /// Allocate a special kernel-backed fd (eventfd, epoll, timerfd, etc.).
    pub fn alloc_special_fd(&mut self, name: String, flags: u32, fd_type: FdType) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name,
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type,
                    pipe_id: None,
                    fd_num: i,
                });
                return Some(i);
            }
        }
        None
    }

    /// Write to a FakeFile fd. Returns bytes written.
    pub fn fake_write(&mut self, fd: i32, buf: usize, len: usize) -> Option<isize> {
        let slot = self.fds.get_mut(fd as usize)?;
        let desc = slot.as_mut()?;
        let data = match &mut desc.fd_type {
            FdType::FakeFile(d) => d,
            _ => return None,
        };
        let pos = desc.pos;
        let end = pos + len;
        if end > data.len() {
            data.resize(end, 0);
        }
        for i in 0..len {
            data[pos + i] = crate::syscall::user_read_u8(buf + i);
        }
        desc.pos = end;
        Some(len as isize)
    }

    /// Read from a FakeFile fd. Returns bytes read.
    pub fn fake_read(&mut self, fd: i32, buf: usize, len: usize) -> Option<isize> {
        // First get pos and to_read
        let (pos, to_read) = {
            let slot = self.fds.get(fd as usize)?;
            let desc = slot.as_ref()?;
            match &desc.fd_type {
                FdType::FakeFile(data) => {
                    let pos = desc.pos;
                    let avail = if pos < data.len() {
                        data.len() - pos
                    } else {
                        0
                    };
                    (pos, core::cmp::min(len, avail))
                }
                FdType::Urandom => {
                    // /dev/urandom: always produces bytes, never EOF
                    (0, len)
                }
                _ => return None,
            }
        };
        // Copy data to user buffer (with CR3 switch on x86_64)
        {
            let slot = self.fds.get(fd as usize)?;
            let desc = slot.as_ref()?;
            match &desc.fd_type {
                FdType::FakeFile(data) => {
                    for i in 0..to_read {
                        crate::syscall::user_write_u8(buf + i, data[pos + i]);
                    }
                }
                FdType::Urandom => {
                    // Generate pseudo-random bytes using LCG PRNG
                    static PRNG: core::sync::atomic::AtomicU64 =
                        core::sync::atomic::AtomicU64::new(0xDEADBEEFCAFE1234);
                    for i in 0..to_read {
                        let prev = PRNG.load(core::sync::atomic::Ordering::Relaxed);
                        let next = prev
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        PRNG.store(next, core::sync::atomic::Ordering::Relaxed);
                        let byte = ((next >> ((i % 8) * 8)) & 0xFF) as u8;
                        crate::syscall::user_write_u8(buf + i, byte);
                    }
                }
                _ => {}
            }
        }
        // Update pos
        if let Some(Some(desc)) = self.fds.get_mut(fd as usize) {
            desc.pos = pos + to_read;
        }
        Some(to_read as isize)
    }

    /// Truncate a FakeFile fd to specified size.
    pub fn fake_truncate(&mut self, fd: i32, size: usize) -> bool {
        if let Some(Some(desc)) = self.fds.get_mut(fd as usize) {
            if let FdType::FakeFile(ref mut data) = desc.fd_type {
                data.resize(size, 0);
                if desc.pos > size {
                    desc.pos = size;
                }
                return true;
            }
        }
        false
    }

    /// Allocate a pipe fd. Used by sys_pipe.
    pub fn alloc_pipe_fd(&mut self, pipe_id: usize, is_read: bool) -> Option<usize> {
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name: if is_read {
                        String::from("pipe:read")
                    } else {
                        String::from("pipe:write")
                    },
                    pos: 0,
                    flags: if is_read { O_RDONLY } else { O_WRONLY },
                    valid: true,
                    fd_type: if is_read {
                        FdType::PipeRead
                    } else {
                        FdType::PipeWrite
                    },
                    pipe_id: Some(pipe_id),
                    fd_num: i,
                });
                return Some(i);
            }
        }
        None
    }

    /// Set (override) an existing fd. Used by dup2 and fd redirection.
    pub fn set_fd(&mut self, fd: usize, desc: FileDescriptor) {
        if fd < self.fds.len() {
            self.fds[fd] = Some(desc);
        }
    }

    /// Duplicate an fd: copy the descriptor from oldfd to newfd.
    /// Returns true on success, false on failure (invalid oldfd).
    pub fn dup(&mut self, oldfd: usize, newfd: usize) -> bool {
        if newfd >= self.fds.len() {
            return false;
        }
        // Get a clone of the old fd
        let desc = match self
            .fds
            .get(oldfd)
            .and_then(|opt| opt.as_ref().filter(|f| f.valid))
            .cloned()
        {
            Some(d) => d,
            None => return false,
        };
        // Close newfd if it was open
        self.fds[newfd] = Some(desc);
        true
    }

    /// Get a reference to a file descriptor
    pub fn get(&self, fd: usize) -> Option<&FileDescriptor> {
        self.fds
            .get(fd)
            .and_then(|opt| opt.as_ref().filter(|f| f.valid))
    }

    /// Get a mutable reference to a file descriptor
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FileDescriptor> {
        self.fds
            .get_mut(fd)
            .and_then(|opt| opt.as_mut().filter(|f| f.valid))
    }

    /// Remove and return a file descriptor so callers can run type-specific cleanup.
    pub fn take(&mut self, fd: usize) -> Option<FileDescriptor> {
        let slot = self.fds.get_mut(fd)?;
        if slot.as_ref().map(|f| f.valid).unwrap_or(false) {
            slot.take()
        } else {
            None
        }
    }

    /// Remove all open descriptors. Used by process exit to release kernel state.
    pub fn drain_open_fds(&mut self) -> Vec<(usize, FileDescriptor)> {
        let mut drained = Vec::new();
        for (fd, slot) in self.fds.iter_mut().enumerate() {
            if slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                if let Some(desc) = slot.take() {
                    drained.push((fd, desc));
                }
            }
        }
        drained
    }

    /// Close a file descriptor
    pub fn close(&mut self, fd: usize) -> bool {
        if let Some(slot) = self.fds.get_mut(fd) {
            if slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                if fd <= 5 {
                    // no debug log — holding fd_table lock
                }
                *slot = None;
                return true;
            }
        }
        false
    }
}

#[cfg(feature = "test_mode")]
pub fn run_tests() {
    crate::console_println!("");
    crate::console_println!("── Filesystem Tests ──");

    // Test 1: Create file
    crate::test::run_test("fs_create_file", || {
        let mut fs = FileSystem::new();
        fs.create("test.txt").is_ok()
    });

    // Test 2: Create duplicate file fails
    crate::test::run_test("fs_create_duplicate_fails", || {
        let mut fs = FileSystem::new();
        fs.create("dup.txt").is_ok() && fs.create("dup.txt").is_err()
    });

    // Test 3: Write and read
    crate::test::run_test("fs_write_and_read", || {
        let mut fs = FileSystem::new();
        fs.write("data.bin", &[1, 2, 3, 4]).is_ok()
            && fs.read("data.bin") == Some(&[1u8, 2, 3, 4][..])
    });

    // Test 4: Write to non-existent file creates it
    crate::test::run_test("fs_write_creates_file", || {
        let mut fs = FileSystem::new();
        fs.write("auto.txt", b"hello").is_ok() && fs.read("auto.txt").is_some()
    });

    // Test 5: Write overwrites existing
    crate::test::run_test("fs_write_overwrites", || {
        let mut fs = FileSystem::new();
        fs.write("f.txt", b"old").is_ok();
        fs.write("f.txt", b"new").is_ok();
        fs.read("f.txt") == Some(&b"new"[..])
    });

    // Test 6: Append to existing file
    crate::test::run_test("fs_append", || {
        let mut fs = FileSystem::new();
        fs.write("a.txt", b"hello").is_ok();
        fs.append("a.txt", b" world").is_ok();
        fs.read("a.txt") == Some(&b"hello world"[..])
    });

    // Test 7: Append to non-existent file fails
    crate::test::run_test("fs_append_nonexistent_fails", || {
        let mut fs = FileSystem::new();
        fs.append("nope.txt", b"data").is_err()
    });

    // Test 8: Delete file
    crate::test::run_test("fs_delete", || {
        let mut fs = FileSystem::new();
        fs.create("del.txt").is_ok();
        fs.delete("del.txt").is_ok() && fs.read("del.txt").is_none()
    });

    // Test 9: Delete non-existent fails
    crate::test::run_test("fs_delete_nonexistent_fails", || {
        let mut fs = FileSystem::new();
        fs.delete("nope").is_err()
    });

    // Test 10: Read non-existent returns None
    crate::test::run_test("fs_read_nonexistent_none", || {
        let fs = FileSystem::new();
        fs.read("nope").is_none()
    });

    // Test 11: List files
    crate::test::run_test("fs_list_files", || {
        let mut fs = FileSystem::new();
        fs.create("a").is_ok();
        fs.create("b").is_ok();
        fs.create("c").is_ok();
        fs.list().len() == 3
    });

    // Test 12: File count
    crate::test::run_test("fs_file_count", || {
        let mut fs = FileSystem::new();
        fs.create("x").is_ok();
        fs.create("y").is_ok();
        fs.file_count() == 2
    });

    // Test 13: Empty file has no data
    crate::test::run_test("fs_empty_file_no_data", || {
        let mut fs = FileSystem::new();
        fs.create("empty").is_ok();
        fs.read("empty") == Some(&[][..])
    });

    // Test 14: Large file write/read
    crate::test::run_test("fs_large_file", || {
        let mut fs = FileSystem::new();
        let data: alloc::vec::Vec<u8> = (0..=255u8).collect();
        fs.write("big.bin", &data).is_ok();
        let read = fs.read("big.bin");
        read.is_some() && read.unwrap().len() == 256 && read.unwrap()[255] == 255
    });

    // Test 15: Delete then recreate
    crate::test::run_test("fs_delete_recreate", || {
        let mut fs = FileSystem::new();
        fs.write("r.txt", b"v1").is_ok();
        fs.delete("r.txt").is_ok();
        fs.create("r.txt").is_ok();
        fs.read("r.txt") == Some(&[][..])
    });
}
