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
//! All methods below pre-allocate buffers under kernel CR3, then only do
//! raw `read_volatile`/`write_volatile` inside `with_user_cr3()`.

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
        let mut val = T::default();
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            val = unsafe { core::ptr::read_volatile(self.addr as *const T) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            val = core::ptr::read_volatile(self.addr as *const T);
        }
        val
    }

    /// Write a value to user space (with CR3 switch on x86_64).
    /// No heap operations inside the CR3 switch.
    #[inline]
    pub fn write(&self, val: T) {
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            unsafe { core::ptr::write_volatile(self.addr as *mut T, val) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile(self.addr as *mut T, val)
        }
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
    /// CRITICAL: Vec is pre-allocated under kernel CR3.
    /// Inside with_user_cr3(), we only write to the Vec's raw buffer
    /// via pointer — NO push/alloc/dealloc.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        // SAFETY: with_capacity allocated len bytes. We write exactly len bytes
        // via raw pointer, then set_len. No alloc/dealloc inside with_user_cr3.
        let dst = buf.as_mut_ptr() as *mut u8;
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..self.len {
                unsafe {
                    let byte = core::ptr::read_volatile((self.addr + i) as *const u8);
                    core::ptr::write(dst.add(i), byte);
                }
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..self.len {
            unsafe {
                let byte = core::ptr::read_volatile((self.addr + i) as *const u8);
                core::ptr::write(dst.add(i), byte);
            }
        }
        unsafe { buf.set_len(self.len) };
        buf
    }

    /// Read a NUL-terminated string from user space (max 4096 bytes).
    ///
    /// CRITICAL: Vec is pre-allocated under kernel CR3.
    /// Inside with_user_cr3(), we only write via raw pointer.
    pub fn read_cstring(&self) -> Option<String> {
        let max_len = self.len.min(4096);
        let mut buf = Vec::with_capacity(max_len);
        let dst = buf.as_mut_ptr() as *mut u8;
        let mut actual_len = 0usize;
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..max_len {
                let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
                if byte == 0 {
                    return;
                }
                unsafe { core::ptr::write(dst.add(actual_len), byte) };
                actual_len += 1;
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..max_len {
            let byte = unsafe { core::ptr::read_volatile((self.addr + i) as *const u8) };
            if byte == 0 {
                break;
            }
            unsafe { core::ptr::write(dst.add(actual_len), byte) };
            actual_len += 1;
        }
        unsafe { buf.set_len(actual_len) };
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
    /// CRITICAL: Same as UserSlice::read_to_vec — no heap ops inside with_user_cr3.
    pub fn read_to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.len);
        let dst = buf.as_mut_ptr() as *mut u8;
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..self.len {
                unsafe {
                    let byte = core::ptr::read_volatile((self.addr + i) as *const u8);
                    core::ptr::write(dst.add(i), byte);
                }
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..self.len {
            unsafe {
                let byte = core::ptr::read_volatile((self.addr + i) as *const u8);
                core::ptr::write(dst.add(i), byte);
            }
        }
        unsafe { buf.set_len(self.len) };
        buf
    }

    /// Write bytes to user space from a source slice.
    ///
    /// CRITICAL: Only write_volatile inside with_user_cr3 — no heap ops.
    pub fn copy_from_slice(&self, src: &[u8]) {
        let n = src.len().min(self.len);
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            for i in 0..n {
                unsafe { core::ptr::write_volatile((self.addr + i) as *mut u8, src[i]) };
            }
        });
        #[cfg(not(target_arch = "x86_64"))]
        for i in 0..n {
            unsafe { core::ptr::write_volatile((self.addr + i) as *mut u8, src[i]) };
        }
    }

    /// Write a single byte to user space at offset.
    ///
    /// CRITICAL: Only write_volatile inside with_user_cr3 — no heap ops.
    #[inline]
    pub fn write_byte_at(&self, offset: usize, byte: u8) {
        if offset >= self.len {
            return;
        }
        #[cfg(target_arch = "x86_64")]
        crate::arch::trap::with_user_cr3(|| {
            unsafe { core::ptr::write_volatile((self.addr + offset) as *mut u8, byte) };
        });
        #[cfg(not(target_arch = "x86_64"))]
        unsafe {
            core::ptr::write_volatile((self.addr + offset) as *mut u8, byte)
        }
    }
}
