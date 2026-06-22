// mmap file-backed regression test
// Verifies that first mmap access at non-zero offset loads correct page data.
// This tests the PF handler fix in kernel/src/arch/x86_64/idt.rs
// where fault_addr_val was replaced with page_addr for vma_file_info.

package main

import (
	"fmt"
	"os"
	"syscall"
	"unsafe"
)

func main() {
	f, err := os.OpenFile("/.mmap_regr_test.bin", os.O_RDWR|os.O_CREATE|os.O_TRUNC, 0644)
	if err != nil {
		fmt.Printf("FAIL open: %v\n", err)
		os.Exit(1)
	}
	defer f.Close()
	f.Truncate(4096)
	fd := int(f.Fd())

	data, err := syscall.Mmap(fd, 0, 4096, syscall.PROT_READ|syscall.PROT_WRITE, syscall.MAP_SHARED)
	if err != nil {
		fmt.Printf("FAIL mmap: %v\n", err)
		os.Exit(1)
	}
	defer syscall.Munmap(data)

	// Write known pattern at offset 0
	*(*uint32)(unsafe.Pointer(&data[0])) = 0xDEADBEEF
	// Write known pattern at offset 32
	*(*uint32)(unsafe.Pointer(&data[32])) = 0x226CD2CE

	// Verify via mmap (same session)
	if *(*uint32)(unsafe.Pointer(&data[0])) != 0xDEADBEEF {
		fmt.Printf("FAIL offset 0: got %08X\n", *(*uint32)(unsafe.Pointer(&data[0])))
		os.Exit(1)
	}
	if *(*uint32)(unsafe.Pointer(&data[32])) != 0x226CD2CE {
		fmt.Printf("FAIL offset 32: got %08X\n", *(*uint32)(unsafe.Pointer(&data[32])))
		os.Exit(1)
	}

	syscall.Munmap(data)
	f.Close()

	// Phase 2: reopen and read via mmap, first access at non-zero offset (the bug trigger)
	f2, err := os.OpenFile("/.mmap_regr_test.bin", os.O_RDWR, 0644)
	if err != nil {
		fmt.Printf("FAIL reopen: %v\n", err)
		os.Exit(1)
	}
	defer f2.Close()

	data2, err := syscall.Mmap(int(f2.Fd()), 0, 4096, syscall.PROT_READ, syscall.MAP_SHARED)
	if err != nil {
		fmt.Printf("FAIL mmap2: %v\n", err)
		os.Exit(1)
	}
	defer syscall.Munmap(data2)

	// CRITICAL: first access at offset 32 (non-zero) — this triggers the original bug
	// where vma_file_info received fault_addr_val (base+32) instead of page_addr (base)
	v32 := *(*uint32)(unsafe.Pointer(&data2[32]))
	if v32 != 0x226CD2CE {
		fmt.Printf("FAIL offset 32 first: got %08X want %08X\n", v32, uint32(0x226CD2CE))
		os.Exit(1)
	}

	// Then verify offset 0 still works (page already loaded from offset 32 above)
	v0 := *(*uint32)(unsafe.Pointer(&data2[0]))
	if v0 != 0xDEADBEEF {
		fmt.Printf("FAIL offset 0 after: got %08X want %08X\n", v0, uint32(0xDEADBEEF))
		os.Exit(1)
	}

	fmt.Println("PASS: mmap file-backed regression test")
}
