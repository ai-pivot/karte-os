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
    /// eventfd — for Go runtime polling.
    #[cfg(target_arch = "x86_64")]
    Eventfd,
    /// timerfd — for Go runtime timers.
    #[cfg(target_arch = "x86_64")]
    Timerfd,
    /// ext4 file descriptor.
    #[cfg(target_arch = "x86_64")]
    Ext4File(crate::driver::ext4::Ext4FileDesc),
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
            FdType::Eventfd => {
                crate::syscall::epoll::eventfd::eventfd_peek_by_fd(self.fd_num) > 0
            }
            #[cfg(target_arch = "x86_64")]
            FdType::Timerfd => {
                crate::syscall::epoll::timerfd_peek(self.fd_num)
            }
            FdType::PipeWrite | FdType::File | FdType::FakeFile(_) | FdType::VirtualFile => false,
            #[cfg(target_arch = "x86_64")]
            FdType::Ext4File(_) => false,
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn is_readable(&self) -> bool {
        false
    }

    /// Check if this fd is ready for writing.
    #[cfg(target_arch = "x86_64")]
    pub fn is_writable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => self.name != "stdin",
            FdType::PipeWrite => true,
            FdType::PipeRead | FdType::Eventfd | FdType::Timerfd => false,
            FdType::File | FdType::FakeFile(_) | FdType::VirtualFile => true,
            FdType::Ext4File(desc) => desc.writable,
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn is_writable(&self) -> bool {
        match &self.fd_type {
            FdType::Stdio => self.name != "stdin",
            FdType::PipeWrite => true,
            FdType::PipeRead => false,
            FdType::File | FdType::FakeFile(_) | FdType::VirtualFile => true,
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
        for (i, slot) in self.fds.iter_mut().enumerate() {
            if slot.is_none() || !slot.as_ref().map(|f| f.valid).unwrap_or(false) {
                *slot = Some(FileDescriptor {
                    name,
                    pos: 0,
                    flags,
                    valid: true,
                    fd_type: FdType::FakeFile(alloc::vec![]),
                    pipe_id: None,
                fd_num: 0,
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
            data[pos + i] = unsafe { core::ptr::read_volatile((buf + i) as *const u8) };
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
                _ => return None,
            }
        };
        // Copy data
        {
            let slot = self.fds.get(fd as usize)?;
            let desc = slot.as_ref()?;
            if let FdType::FakeFile(data) = &desc.fd_type {
                for i in 0..to_read {
                    unsafe { core::ptr::write_volatile((buf + i) as *mut u8, data[pos + i]) };
                }
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

    /// Close a file descriptor
    pub fn close(&mut self, fd: usize) -> bool {
        if let Some(slot) = self.fds.get_mut(fd) {
            if slot.as_ref().map(|f| f.valid).unwrap_or(false) {
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
