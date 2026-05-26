// kernel/src/driver/fs.rs — Simple in-memory file system

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::sync::spinlock::SpinLock;

/// A single file with a name and binary data.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub data: Vec<u8>,
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
        Self {
            files: Vec::new(),
        }
    }

    /// Create a new empty file system.
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
        }
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
            data: Vec::new(),
        });
        Ok(())
    }

    /// Write data to a file. If the file does not exist, it is created.
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data.clear();
            file.data.extend_from_slice(data);
            Ok(())
        } else {
            self.files.push(File {
                name: String::from(name),
                data: Vec::from(data),
            });
            Ok(())
        }
    }

    /// Append data to an existing file.
    ///
    /// Returns `Ok(())` if the file exists and data was appended,
    /// or `Err(())` if the file does not exist.
    pub fn append(&mut self, name: &str, data: &[u8]) -> Result<(), ()> {
        if let Some(file) = self.files.iter_mut().find(|f| f.name == name) {
            file.data.extend_from_slice(data);
            Ok(())
        } else {
            Err(())
        }
    }

    /// Read the contents of a file by name.
    pub fn read(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|f| f.name == name).map(|f| f.data.as_slice())
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

/// Initialize the file system.
pub fn init() {
    crate::console_println!("[fs] In-memory file system initialized");
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
        fs.write("auto.txt", b"hello").is_ok()
            && fs.read("auto.txt").is_some()
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
