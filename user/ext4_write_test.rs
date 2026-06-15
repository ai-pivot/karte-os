// ext4_write_test.rs — Minimal ext4 write/read consistency test
#![no_std]
#![no_main]
#![allow(unsafe_op_in_unsafe_fn)]

#[path = "syscall.rs"]
mod syscall;

use syscall::*;

fn print_num(n: usize) {
    print_u64(n as u64);
}

const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0x100;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    print(b"[PANIC]\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}

fn pass(name: &[u8]) {
    print(b"  [PASS] ");
    print(name);
    print(b"\n");
}

fn fail(name: &[u8]) {
    print(b"  [FAIL] ");
    print(name);
    print(b"\n");
}

fn open(path: &[u8], flags: usize) -> i64 {
    let ret = unsafe { syscall3(SYS_OPEN, path.as_ptr() as usize, path.len(), flags) };
    ret as i64
}

fn write(fd: i64, data: &[u8]) -> i64 {
    let ret = unsafe { syscall3(SYS_WRITE, fd as usize, data.as_ptr() as usize, data.len()) };
    ret as i64
}

fn read(fd: i64, buf: &mut [u8]) -> i64 {
    let ret = unsafe { syscall3(SYS_READ, fd as usize, buf.as_ptr() as usize, buf.len()) };
    ret as i64
}

fn close(fd: i64) {
    unsafe { syscall1(SYS_CLOSE, fd as usize); }
}

fn unlink(path: &[u8]) {
    unsafe { syscall2(SYS_UNLINK, path.as_ptr() as usize, path.len()); }
}

fn verify(name: &[u8], expected: &[u8], actual: &[u8], actual_len: usize) -> bool {
    if actual_len != expected.len() {
        print(b"    expected ");
        print_num(expected.len());
        print(b" bytes, got ");
        print_num(actual_len);
        print(b"\n");
        fail(name);
        return false;
    }
    for i in 0..expected.len() {
        if actual[i] != expected[i] {
            print(b"    mismatch at byte ");
            print_num(i);
            print(b"\n");
            fail(name);
            return false;
        }
    }
    pass(name);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    print(b"\n=== ext4 Write/Read Test ===\n\n");

    // ── Test 1: Small write (100 bytes) ──
    {
        print(b"Test 1: Small write (100 bytes)\n");
        let path = b"test_small";
        unlink(path); // clean up if exists

        let fd = open(path, O_WRONLY | O_CREAT);
        if fd < 0 {
            fail(b"open for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }

        let mut data = [0u8; 100];
        for i in 0..100 {
            data[i] = ((i * 7 + 13) & 0xFF) as u8;
        }

        let n = write(fd, &data);
        if n != 100 {
            print(b"    write returned ");
            print_num(n as usize);
            print(b"\n");
            fail(b"write");
        }
        close(fd);

        // Read back
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            fail(b"open for read");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut buf = [0u8; 200];
        let n = read(fd, &mut buf);
        close(fd);

        verify(b"small write round-trip", &data, &buf, n as usize);
    }

    // ── Test 2: Block-size write (4096 bytes) ──
    {
        print(b"\nTest 2: Block-size write (4096 bytes)\n");
        let path = b"test_block";
        unlink(path);

        let fd = open(path, O_WRONLY | O_CREAT);
        if fd < 0 {
            fail(b"open for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }

        let mut data = [0u8; 4096];
        for i in 0..4096 {
            data[i] = ((i * 3 + 0x41) & 0xFF) as u8;
        }

        let n = write(fd, &data);
        if n != 4096 {
            print(b"    write returned ");
            print_num(n as usize);
            print(b"\n");
            fail(b"write");
        }
        close(fd);

        // Read back
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            fail(b"open for read");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut buf = [0u8; 4096];
        let n = read(fd, &mut buf);
        close(fd);

        verify(b"block write round-trip", &data, &buf, n as usize);
    }

    // ── Test 3: Multi-block write (8192 bytes = 2 blocks) ──
    {
        print(b"\nTest 3: Multi-block write (8192 bytes)\n");
        let path = b"test_multi";
        unlink(path);

        let fd = open(path, O_WRONLY | O_CREAT);
        if fd < 0 {
            fail(b"open for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }

        let mut data = [0u8; 8192];
        for i in 0..8192 {
            data[i] = ((i * 5 + 0xAA) & 0xFF) as u8;
        }

        let n = write(fd, &data);
        if n != 8192 {
            print(b"    write returned ");
            print_num(n as usize);
            print(b"\n");
            fail(b"write");
        }
        close(fd);

        // Read back
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            fail(b"open for read");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut buf = [0u8; 8192];
        let n = read(fd, &mut buf);
        close(fd);

        verify(b"multi-block write round-trip", &data, &buf, n as usize);
    }

    // ── Test 4: Sequential writes (write 4096 bytes in two calls) ──
    {
        print(b"\nTest 4: Sequential writes (2x 2048 bytes)\n");
        let path = b"test_seq";
        unlink(path);

        let fd = open(path, O_WRONLY | O_CREAT);
        if fd < 0 {
            fail(b"open for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }

        // First write: 2048 bytes
        let mut data1 = [0u8; 2048];
        for i in 0..2048 {
            data1[i] = ((i + 1) & 0xFF) as u8;
        }
        let n1 = write(fd, &data1);
        print(b"    first write returned ");
        print_num(n1 as usize);
        print(b"\n");

        // Second write: 2048 bytes
        let mut data2 = [0u8; 2048];
        for i in 0..2048 {
            data2[i] = ((i + 0x80) & 0xFF) as u8;
        }
        let n2 = write(fd, &data2);
        print(b"    second write returned ");
        print_num(n2 as usize);
        print(b"\n");

        close(fd);

        // Read back — expect 4096 bytes: data1 followed by data2
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            fail(b"open for read");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut buf = [0u8; 4096];
        let n = read(fd, &mut buf);
        close(fd);

        print(b"    read returned ");
        print_num(n as usize);
        print(b" bytes (expected 4096)\n");

        if n != 4096 {
            print(b"    NOTE: read returned wrong size, checking if sequential write works\n");
            // Even if we got fewer bytes, check what we got
        }

        // Verify first half
        let mut first_half_ok = true;
        for i in 0..2048 {
            if i < n as usize && buf[i] != data1[i] {
                print(b"    first half mismatch at byte ");
                print_num(i);
                print(b" expected ");
                print_num(data1[i] as usize);
                print(b" got ");
                print_num(buf[i] as usize);
                print(b"\n");
                first_half_ok = false;
                break;
            }
        }
        if first_half_ok {
            pass(b"first 2048 bytes correct");
        } else {
            fail(b"first 2048 bytes correct");
        }

        // Verify second half
        let mut second_half_ok = true;
        for i in 0..2048 {
            let buf_idx = 2048 + i;
            if buf_idx < n as usize && buf[buf_idx] != data2[i] {
                print(b"    second half mismatch at byte ");
                print_num(buf_idx);
                print(b" expected ");
                print_num(data2[i] as usize);
                print(b" got ");
                print_num(buf[buf_idx] as usize);
                print(b"\n");
                second_half_ok = false;
                break;
            }
        }
        if second_half_ok {
            pass(b"second 2048 bytes correct");
        } else {
            fail(b"second 2048 bytes correct");
        }
    }

    // ── Test 5: Overwrite existing file ──
    {
        print(b"\nTest 5: Overwrite (write 4096, reopen, write 100 at start)\n");
        let path = b"test_ow";
        unlink(path);

        // Initial write: 4096 bytes
        let fd = open(path, O_WRONLY | O_CREAT);
        if fd < 0 {
            fail(b"open for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut data = [0u8; 4096];
        for i in 0..4096 {
            data[i] = 0xFF;
        }
        write(fd, &data);
        close(fd);

        // Reopen and write 100 bytes at start
        let fd = open(path, O_WRONLY);
        if fd < 0 {
            fail(b"reopen for write");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut small = [0u8; 100];
        for i in 0..100 {
            small[i] = ((i * 2) & 0xFF) as u8;
        }
        write(fd, &small);
        close(fd);

        // Read back
        let fd = open(path, O_RDONLY);
        if fd < 0 {
            fail(b"open for read");
            unsafe { syscall1(SYS_EXIT, 1); }
        }
        let mut buf = [0u8; 4096];
        let n = read(fd, &mut buf);
        close(fd);

        print(b"    read returned ");
        print_num(n as usize);
        print(b" bytes\n");

        if n == 4096 {
            // Check first 100 bytes = small[], rest = 0xFF
            let mut first_ok = true;
            for i in 0..100 {
                if buf[i] != small[i] {
                    first_ok = false;
                    break;
                }
            }
            let mut rest_ok = true;
            for i in 100..4096 {
                if buf[i] != 0xFF {
                    rest_ok = false;
                    break;
                }
            }
            if first_ok && rest_ok {
                pass(b"overwrite preserves unwritten bytes");
            } else {
                if !first_ok {
                    fail(b"first 100 bytes wrong");
                }
                if !rest_ok {
                    fail(b"remaining bytes not preserved");
                }
            }
        } else {
            // If the file got truncated to 100 bytes, that's the bug!
            print(b"    FILE TRUNCATED to ");
            print_num(n as usize);
            print(b" bytes (expected 4096)\n");
            fail(b"overwrite should preserve file size");
        }
    }

    print(b"\n=== Tests complete ===\n");
    unsafe { syscall1(SYS_EXIT, 0); }
}
