// ext4_sparse_test.rs — Reproduce SQLite db-shm sparse write pattern
// SQLite WAL mode writes 1 byte at each page boundary (4095, 8191, ..., 32767)
// This triggers ext4 block allocation for sparse files.
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;
use syscall::*;

const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_RDWR: usize = 2;
const O_CREAT: usize = 0x100;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    print(b"[PANIC]\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}

fn open(path: &[u8], flags: usize) -> i64 {
    unsafe { syscall3(SYS_OPEN, path.as_ptr() as usize, path.len(), flags) as i64 }
}
fn pwrite(fd: i64, data: &[u8], offset: usize) -> i64 {
    // Use SYS_WRITE which goes through our sys_write path with Ext4File
    // For true pwrite, we'd need the Linux syscall, but sys_write uses fd.pos.
    // So we'll use lseek + write to simulate pwrite.
    // Actually let's just use write() — the kernel Ext4File path uses fd.pos.
    // We need to set pos first via lseek.
    // But lseek is a Linux syscall (62 on riscv). Let me use it directly.
    unsafe { syscall3(62, fd as usize, offset, 0) }; // lseek(fd, offset, SEEK_SET)
    unsafe { syscall3(SYS_WRITE, fd as usize, data.as_ptr() as usize, data.len()) as i64 }
}
fn close(fd: i64) {
    unsafe { syscall1(SYS_CLOSE, fd as usize); }
}
fn unlink(path: &[u8]) {
    unsafe { syscall2(SYS_UNLINK, path.as_ptr() as usize, path.len()); }
}

fn print_num(n: i64) {
    if n < 0 { print(b"-"); print_num(-n); return; }
    print_u64(n as u64);
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    print(b"\n=== ext4 Sparse Write Test ===\n");
    print(b"Simulates SQLite db-shm write pattern:\n");
    print(b"  pwrite(fd, &1_byte, 4095)\n");
    print(b"  pwrite(fd, &1_byte, 8191)\n");
    print(b"  ... up to 32767\n\n");

    let path = b"sparse_test";
    unlink(path);

    // Create + open RDWR
    let fd = open(path, O_RDWR | O_CREAT);
    if fd < 0 {
        print(b"FAIL: open returned ");
        print_num(fd);
        print(b"\n");
        unsafe { syscall1(SYS_EXIT, 1); }
    }
    print(b"open OK, fd=");
    print_num(fd);
    print(b"\n");

    // Write 1 byte at each page boundary (last byte of each 4K page)
    // This is exactly what SQLite does for db-shm initialization
    let one_byte: [u8; 1] = [0x42];
    let offsets: [usize; 8] = [4095, 8191, 12287, 16383, 20479, 24575, 28671, 32767];

    let mut all_ok = true;
    for i in 0..8 {
        let off = offsets[i];
        let n = pwrite(fd, &one_byte, off);
        if n != 1 {
            print(b"FAIL: pwrite at offset ");
            print_num(off as i64);
            print(b" returned ");
            print_num(n);
            print(b"\n");
            all_ok = false;
            break;
        } else {
            print(b"  pwrite(off=");
            print_num(off as i64);
            print(b") -> OK\n");
        }
        
        // Immediately read back to verify
        unsafe { syscall3(62, fd as usize, off, 0) }; // lseek back
        let mut verify = [0xAA; 1]; // pre-fill with non-0x42 sentinel
        let rn = unsafe { syscall3(SYS_READ, fd as usize, verify.as_ptr() as usize, 1) };
        // Use volatile read to bypass compiler caching
        let val = unsafe { core::ptr::read_volatile(verify.as_ptr()) };
        if val == 0x42 {
            print(b"  ok\n");
        } else if val == 0xAA {
            print(b"  UNWRITTEN: still 0xAA\n");
        } else {
            print(b"  CLOBBERED: got ");
            print_num(val as i64);
            print(b"\n");
        }
        if rn != 1 || val != 0x42 {
            print(b"  VERIFY FAIL: read after write at off=");
            print_num(off as i64);
            print(b" rn=");
            print_num(rn as i64);
            if rn == 1 { print(b" val="); print_num(verify[0] as i64); }
            print(b"\n");
            all_ok = false;
        }
    }

    close(fd);

    if all_ok {
        // Read back and verify
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            print(b"FAIL: reopen for read\n");
            unsafe { syscall1(SYS_EXIT, 1); }
        }

        let mut ok = true;
        for i in 0..8 {
            let off = offsets[i];
            // lseek to offset
            unsafe { syscall3(62, fd as usize, off, 0) };
            let mut buf = [0u8; 1];
            let n = unsafe { syscall3(SYS_READ, fd as usize, buf.as_ptr() as usize, 1) };
            if n != 1 || buf[0] != 0x42 {
                print(b"FAIL: read at offset ");
                print_num(off as i64);
                print(b" got n=");
                print_num(n as i64);
                if n == 1 {
                    print(b" val=");
                    print_num(buf[0] as i64);
                }
                print(b"\n");
                ok = false;
            }
        }
        close(fd);

        if ok {
            print(b"\n[PASS] All sparse writes verified!\n");
        } else {
            print(b"\n[FAIL] Readback verification failed!\n");
        }
    } else {
        print(b"\n[FAIL] Sparse write failed!\n");
    }

    // DON'T unlink — leave the file for disk inspection
    // unlink(path);
    unsafe { syscall1(SYS_EXIT, 0); }
}
