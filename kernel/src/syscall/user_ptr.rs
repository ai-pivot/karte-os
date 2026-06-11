//! Type-safe user-space memory access for x86_64.
//!
//! On x86_64, syscalls run under kernel CR3. User-space memory is only
//! accessible under user CR3. These types enforce at **compile time**
//! that every access to user memory goes through the correct CR3 switch.
//!
//! # Critical rule: NO heap operations inside with_user_cr3()
//!
//! `with_user_cr3()` switches to the user page table, whose identity mappings
//! may be corrupted by mmap huge-page splits. The kernel heap allocator's
//! metadata (linked list nodes at ~0x406000) relies on identity-mapped PTEs.
//! If a `Vec::push` triggers alloc/dealloc under user CR3, it may read
//! corrupted HoleList nodes → GP fault in `linked_list_allocator`.
//!
//! All methods below keep kernel buffers on the kernel page table and copy
//! bytes through the syscall user access primitives.

use alloc::string::String;
use alloc::vec::Vec;

/// A pointer to a single value in user space.
///
/// Cannot be dereferenced directly — must use `.read()` / `.write()`
/// which handle CR3 switching on x86_64.
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct UserPtr<T> {
    addr: usize,
    _marker: core::marker::PhantomData<T>,
}

impl<T: Copy + Default> UserPtr<T> {
    /// Create a UserPtr from a raw address. Returns None if addr is 0.
    #[inline]
    pub fn new(addr: usize) -> Option<Self> {
        if addr == 0 {
            None
        } else {
            Some(Self {
                addr,
                _marker: core::marker::PhantomData,
            })
        }
    }

    /// Create a UserPtr without checking for null.
    #[inline]
    pub fn new_unchecked(addr: usize) -> Self {
        Self {
            addr,
            _marker: core::marker::PhantomData,
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Read a value from user space (with CR3 switch on x86_64).
    /// No heap operations inside the CR3 switch.
    #[inline]
    pub fn read(&self) -> T {
        super::user_read::<T>(self.addr)
    }

    /// Write a value to user space (with CR3 switch on x86_64).
    /// No heap operations inside the CR3 switch.
    #[inline]
    pub fn write(&self, val: T) {
        super::user_write::<T>(self.addr, val);
    }

    /// Offset the pointer by `count` elements.
    #[inline]
    pub fn add(&self, count: usize) -> Self {
        Self {
            addr: self.addr + count * core::mem::size_of::<T>(),
            _marker: core::marker::PhantomData,
        }
    }
}

/// A borrowed slice of user memory (read-only).
#[derive(Debug, Clone, Copy)]
pub struct UserSlice {
    addr: usize,
    len: usize,
}

impl UserSlice {
    /// Create from raw address and length. Returns None if addr is 0 or len is 0.
    #[inline]
    pub fn new(addr: usize, len: usize) -> Option<Self> {
        if addr == 0 || len == 0 || len > 65536 {
            None
        } else {
            Some(Self { addr, len })
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Read all bytes from user space into a Vec.
    ///
    /// CRITICAL: kernel buffers are only mutated under kernel CR3.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        for i in 0..self.len {
            buf.push(super::user_read_u8(self.addr + i));
        }
        buf
    }

    /// Read a NUL-terminated string from user space (max 4096 bytes).
    ///
    /// CRITICAL: kernel buffers are only mutated under kernel CR3.
    pub fn read_cstring(&self) -> Option<String> {
        let max_len = self.len.min(4096);
        let mut buf = Vec::with_capacity(max_len);
        for i in 0..max_len {
            let byte = super::user_read_u8(self.addr + i);
            if byte == 0 {
                break;
            }
            buf.push(byte);
        }
        String::from_utf8(buf).ok()
    }
}

/// A mutable slice of user memory (read-write).
#[derive(Debug, Clone, Copy)]
pub struct UserSliceMut {
    addr: usize,
    len: usize,
}

impl UserSliceMut {
    /// Create from raw address and length. Returns None if addr is 0 or len is 0.
    #[inline]
    pub fn new(addr: usize, len: usize) -> Option<Self> {
        if addr == 0 || len == 0 || len > 65536 {
            None
        } else {
            Some(Self { addr, len })
        }
    }

    /// Raw address.
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Read all bytes from user space into a Vec.
    ///
    /// CRITICAL: kernel buffers are only mutated under kernel CR3.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        for i in 0..self.len {
            buf.push(super::user_read_u8(self.addr + i));
        }
        buf
    }

    /// Write bytes to user space from a source slice.
    ///
    /// CRITICAL: kernel buffers are only read under kernel CR3.
    pub fn copy_from_slice(&self, src: &[u8]) {
        let n = src.len().min(self.len);
        super::user_write_bytes(self.addr, &src[..n]);
    }

    /// Write a single byte to user space at offset.
    ///
    /// CRITICAL: kernel values are only read under kernel CR3.
    #[inline]
    pub fn write_byte_at(&self, offset: usize, byte: u8) {
        if offset >= self.len {
            return;
        }
        super::user_write_u8(self.addr + offset, byte);
    }
}
